use std::env;
use std::error::Error;
use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
    },
};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, execvp, read, write};

const PREFIX: u8 = 0x02; // Ctrl-b
const HISTORY_LIMIT: usize = 1024 * 1024;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

struct Window {
    id: usize,
    master: OwnedFd,
    child: Pid,
    history: Vec<u8>,
}

impl Window {
    fn push_history(&mut self, bytes: &[u8]) {
        self.history.extend_from_slice(bytes);
        if self.history.len() > HISTORY_LIMIT {
            let excess = self.history.len() - HISTORY_LIMIT;
            self.history.drain(..excess);
        }
    }
}

struct App {
    windows: Vec<Window>,
    active: usize,
    next_id: usize,
    prefix_pending: bool,
    terminal_size: (u16, u16),
}

impl App {
    fn new() -> Result<Self> {
        let terminal_size = size()?;
        let mut app = Self {
            windows: Vec::new(),
            active: 0,
            next_id: 1,
            prefix_pending: false,
            terminal_size,
        };
        app.create_window()?;
        Ok(app)
    }

    fn create_window(&mut self) -> Result<()> {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        let shell = CString::new(shell)?;
        let winsize = winsize(self.terminal_size);

        // SAFETY: the child immediately calls execvp and _exit, both of which are
        // async-signal-safe; all application bookkeeping remains in the parent.
        match unsafe { forkpty(&winsize, None) }? {
            ForkptyResult::Parent { child, master } => {
                let id = self.next_id;
                self.next_id += 1;
                self.windows.push(Window {
                    id,
                    master,
                    child,
                    history: Vec::new(),
                });
                self.active = self.windows.len() - 1;
                self.redraw()?;
            }
            ForkptyResult::Child => {
                let args = [&shell];
                let _ = execvp(&shell, &args);
                // SAFETY: exiting directly is required after fork if exec fails.
                unsafe { nix::libc::_exit(127) };
            }
        }
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        let stdin = io::stdin();
        let mut input = [0_u8; 4096];
        let mut output = [0_u8; 16 * 1024];

        while !self.windows.is_empty() {
            self.update_size()?;

            let mut poll_fds = Vec::with_capacity(self.windows.len() + 1);
            poll_fds.push(PollFd::new(stdin.as_fd(), PollFlags::POLLIN));
            for window in &self.windows {
                poll_fds.push(PollFd::new(
                    window.master.as_fd(),
                    PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                ));
            }

            poll(&mut poll_fds, 100_u16)?;
            let stdin_ready = poll_fds[0]
                .revents()
                .unwrap_or_else(PollFlags::empty)
                .contains(PollFlags::POLLIN);
            let pty_events: Vec<PollFlags> = poll_fds[1..]
                .iter()
                .map(|fd| fd.revents().unwrap_or_else(PollFlags::empty))
                .collect();
            drop(poll_fds);

            for (index, event) in pty_events.into_iter().enumerate() {
                if event.intersects(PollFlags::POLLIN | PollFlags::POLLHUP) {
                    match read(&self.windows[index].master, &mut output) {
                        Ok(0) | Err(Errno::EIO) => {}
                        Ok(count) => {
                            self.windows[index].push_history(&output[..count]);
                            if index == self.active {
                                io::stdout().write_all(&output[..count])?;
                                io::stdout().flush()?;
                            }
                        }
                        Err(Errno::EAGAIN) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            }

            self.reap_children()?;
            if stdin_ready {
                let count = read(stdin.as_fd(), &mut input)?;
                if count == 0 || !self.handle_input(&input[..count])? {
                    break;
                }
            }
        }
        Ok(())
    }

    fn handle_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let mut passthrough = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            if self.prefix_pending {
                self.prefix_pending = false;
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                match byte {
                    b'c' => self.create_window()?,
                    b'n' => self.select_relative(1)?,
                    b'p' => self.select_relative(-1)?,
                    b'&' => self.close_active()?,
                    b'd' => return Ok(false),
                    b'?' => self.show_help()?,
                    PREFIX => passthrough.push(PREFIX),
                    _ => {}
                }
            } else if byte == PREFIX {
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                self.prefix_pending = true;
            } else {
                passthrough.push(byte);
            }
        }
        if !passthrough.is_empty() && !self.windows.is_empty() {
            self.write_active(&passthrough)?;
        }
        Ok(true)
    }

    fn write_active(&self, mut bytes: &[u8]) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        while !bytes.is_empty() {
            let count = write(&self.windows[self.active].master, bytes)?;
            bytes = &bytes[count..];
        }
        Ok(())
    }

    fn select_relative(&mut self, offset: isize) -> Result<()> {
        if self.windows.len() > 1 {
            self.active =
                (self.active as isize + offset).rem_euclid(self.windows.len() as isize) as usize;
            self.redraw()?;
        }
        Ok(())
    }

    fn close_active(&mut self) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        let window = self.windows.remove(self.active);
        let _ = kill(window.child, Signal::SIGHUP);
        let _ = waitpid(window.child, None);
        if !self.windows.is_empty() {
            self.active = self.active.min(self.windows.len() - 1);
            self.redraw()?;
        }
        Ok(())
    }

    fn redraw(&self) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        let window = &self.windows[self.active];
        let mut stdout = io::stdout();
        write!(
            stdout,
            "\x1b[2J\x1b[H\x1b]0;rustmux:{}\x07\x1b[2m[rustmux window {} | Ctrl-b ?]\x1b[0m\r\n",
            window.id, window.id
        )?;
        stdout.write_all(&window.history)?;
        stdout.flush()?;
        Ok(())
    }

    fn show_help(&self) -> Result<()> {
        let mut stdout = io::stdout();
        write!(
            stdout,
            "\r\n\x1b[1m[rustmux] Ctrl-b commands:\x1b[0m c=new  n=next  p=previous  &=close  d=detach/quit  Ctrl-b=send prefix\r\n"
        )?;
        stdout.flush()?;
        Ok(())
    }

    fn update_size(&mut self) -> Result<()> {
        let new_size = size()?;
        if new_size == self.terminal_size {
            return Ok(());
        }
        self.terminal_size = new_size;
        let winsize = winsize(new_size);
        for window in &self.windows {
            // SAFETY: master is an open PTY descriptor and winsize is valid.
            let result = unsafe {
                nix::libc::ioctl(window.master.as_raw_fd(), nix::libc::TIOCSWINSZ, &winsize)
            };
            if result == -1 {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(())
    }

    fn reap_children(&mut self) -> Result<()> {
        let mut removed_active = false;
        let active_pid = self.windows.get(self.active).map(|window| window.child);
        let mut index = 0;
        while index < self.windows.len() {
            let pid = self.windows[index].child;
            match waitpid(pid, Some(WaitPidFlag::WNOHANG))? {
                WaitStatus::StillAlive => index += 1,
                _ => {
                    removed_active |= Some(pid) == active_pid;
                    self.windows.remove(index);
                    if index < self.active {
                        self.active -= 1;
                    }
                }
            }
        }
        if !self.windows.is_empty() {
            self.active = self.active.min(self.windows.len() - 1);
            if removed_active {
                self.redraw()?;
            }
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        for window in &self.windows {
            let _ = kill(window.child, Signal::SIGHUP);
        }
        for window in self.windows.drain(..) {
            let _ = waitpid(window.child, None);
        }
    }
}

fn winsize((columns, rows): (u16, u16)) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn main() -> Result<()> {
    if let Some(argument) = env::args().nth(1) {
        match argument.as_str() {
            "-h" | "--help" => {
                println!(
                    "rustmux {}\n\nA minimal terminal multiplexer.\n\nUSAGE:\n    rustmux\n\nInside rustmux, press Ctrl-b ? for key bindings.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            "-V" | "--version" => {
                println!("rustmux {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {
                return Err(format!("unknown argument: {argument} (try --help)").into());
            }
        }
    }
    let _terminal = TerminalGuard::enter()?;
    let mut app = App::new()?;
    let result = app.run();
    app.shutdown();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    #[test]
    fn history_is_bounded() {
        // SAFETY: dup returns a new descriptor owned solely by this test.
        let fd = unsafe { nix::libc::dup(nix::libc::STDOUT_FILENO) };
        assert!(fd >= 0);
        // SAFETY: fd is a valid, newly duplicated descriptor.
        let master = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut window = Window {
            id: 1,
            master,
            child: Pid::from_raw(1),
            history: Vec::new(),
        };

        window.push_history(&vec![b'a'; HISTORY_LIMIT]);
        window.push_history(b"tail");

        assert_eq!(window.history.len(), HISTORY_LIMIT);
        assert_eq!(&window.history[HISTORY_LIMIT - 4..], b"tail");
    }

    #[test]
    fn winsize_maps_columns_and_rows() {
        let value = winsize((120, 40));
        assert_eq!(value.ws_col, 120);
        assert_eq!(value.ws_row, 40);
    }
}
