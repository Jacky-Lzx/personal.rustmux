use std::env;
use std::error::Error;
use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::{
    cursor::Show,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, size},
};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, execvp, read, write};

const PREFIX: u8 = 0x02; // Ctrl-b
const SCROLLBACK_LINES: usize = 1_000;
const ESCAPE_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(50);
const ENCODED_PREFIXES: [&[u8]; 2] = [b"\x1b[98;5u", b"\x1b[27;5;98~"];

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Inner programs such as fish and Vim may change cursor visibility.
        // Do not manage the alternate screen here: terminal alternate buffers
        // are not nestable, so an inner program leaving one would also eject
        // rustmux from its own buffer.
        let _ = io::stdout().write_all(
            b"\x1b[0m\x1b[?1l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b>",
        );
        let _ = execute!(io::stdout(), Show);
        let _ = disable_raw_mode();
    }
}

struct Window {
    id: usize,
    name: String,
    master: OwnedFd,
    child: Pid,
    terminal: vt100::Parser,
}

#[derive(Default)]
struct InputDecoder {
    pending: Vec<u8>,
    pending_since: Option<Instant>,
}

impl InputDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let decoded = self.decode_complete();
        if self.pending.is_empty() {
            self.pending_since = None;
        } else if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
        decoded
    }

    fn flush_if_expired(&mut self) -> Vec<u8> {
        if self
            .pending_since
            .is_some_and(|since| since.elapsed() >= ESCAPE_SEQUENCE_TIMEOUT)
        {
            self.flush()
        } else {
            Vec::new()
        }
    }

    fn flush(&mut self) -> Vec<u8> {
        self.pending_since = None;
        std::mem::take(&mut self.pending)
    }

    fn decode_complete(&mut self) -> Vec<u8> {
        let mut decoded = Vec::with_capacity(self.pending.len());
        loop {
            if self.pending.is_empty() {
                break;
            }
            if let Some(sequence) = ENCODED_PREFIXES
                .iter()
                .find(|sequence| self.pending.starts_with(sequence))
            {
                decoded.push(PREFIX);
                self.pending.drain(..sequence.len());
                continue;
            }
            if ENCODED_PREFIXES
                .iter()
                .any(|sequence| sequence.starts_with(&self.pending))
            {
                break;
            }
            decoded.push(self.pending.remove(0));
        }
        decoded
    }
}

struct App {
    windows: Vec<Window>,
    active: usize,
    next_id: usize,
    prefix_pending: bool,
    input_decoder: InputDecoder,
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
            input_decoder: InputDecoder::default(),
            terminal_size,
        };
        app.create_window()?;
        Ok(app)
    }

    fn create_window(&mut self) -> Result<()> {
        let shell = env::var("RUSTMUX_SHELL").unwrap_or_else(|_| "fish".to_owned());
        let name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell)
            .to_owned();
        let shell = CString::new(shell)?;
        let (columns, rows) = content_size(self.terminal_size);
        let winsize = winsize((columns, rows));

        // SAFETY: the child immediately calls execvp and _exit, both of which are
        // async-signal-safe; all application bookkeeping remains in the parent.
        match unsafe { forkpty(&winsize, None) }? {
            ForkptyResult::Parent { child, master } => {
                let id = self.next_id;
                self.next_id += 1;
                self.windows.push(Window {
                    id,
                    name,
                    master,
                    child,
                    terminal: vt100::Parser::new(rows, columns, SCROLLBACK_LINES),
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
            let expired_input = self.input_decoder.flush_if_expired();
            if !expired_input.is_empty() && !self.handle_decoded_input(&expired_input)? {
                break;
            }

            let mut poll_fds = Vec::with_capacity(self.windows.len() + 1);
            poll_fds.push(PollFd::new(stdin.as_fd(), PollFlags::POLLIN));
            for window in &self.windows {
                poll_fds.push(PollFd::new(
                    window.master.as_fd(),
                    PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                ));
            }

            match poll(&mut poll_fds, 100_u16) {
                Ok(_) => {}
                Err(Errno::EINTR) => continue,
                Err(error) => return Err(error.into()),
            }
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
                            self.windows[index].terminal.process(&output[..count]);
                            let responses = terminal_responses(&output[..count]);
                            if !responses.is_empty() {
                                write_fd(&self.windows[index].master, &responses)?;
                            }
                            if index == self.active {
                                self.redraw()?;
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
        let decoded = self.input_decoder.push(bytes);
        self.handle_decoded_input(&decoded)
    }

    fn handle_decoded_input(&mut self, bytes: &[u8]) -> Result<bool> {
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

    fn write_active(&self, bytes: &[u8]) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        write_fd(&self.windows[self.active].master, bytes)
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
        let frame = render_frame(&self.windows, self.active, self.terminal_size);
        let mut stdout = io::stdout().lock();
        stdout.write_all(&frame)?;
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
        let (columns, rows) = content_size(new_size);
        let winsize = winsize((columns, rows));
        for window in &self.windows {
            // SAFETY: master is an open PTY descriptor and winsize is valid.
            let result = unsafe {
                nix::libc::ioctl(window.master.as_raw_fd(), nix::libc::TIOCSWINSZ, &winsize)
            };
            if result == -1 {
                return Err(io::Error::last_os_error().into());
            }
        }
        for window in &mut self.windows {
            window.terminal.screen_mut().set_size(rows, columns);
        }
        self.redraw()?;
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

#[derive(Clone, Copy, Eq, PartialEq)]
struct CellStyle {
    foreground: vt100::Color,
    background: vt100::Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl From<&vt100::Cell> for CellStyle {
    fn from(cell: &vt100::Cell) -> Self {
        Self {
            foreground: cell.fgcolor(),
            background: cell.bgcolor(),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }
}

fn render_frame(windows: &[Window], active: usize, terminal_size: (u16, u16)) -> Vec<u8> {
    let (width, height) = terminal_size;
    let (content_columns, content_rows) = content_size(terminal_size);
    let screen = windows[active].terminal.screen();
    let mut output = Vec::with_capacity(usize::from(width) * usize::from(height) * 2);

    output.extend_from_slice(b"\x1b[?25l\x1b[2J\x1b[H\x1b[32m");
    draw_top_bar(&mut output, windows, active, width);

    for row in 0..content_rows {
        let _ = write!(output, "\x1b[{};1H\x1b[32m│\x1b[0m", row + 2);
        let mut previous_style = None;
        for column in 0..content_columns {
            let cell = screen.cell(row, column).expect("cell is within screen");
            if cell.is_wide_continuation() {
                continue;
            }
            let style = CellStyle::from(cell);
            if previous_style != Some(style) {
                write_cell_style(&mut output, style);
                previous_style = Some(style);
            }
            if cell.has_contents() {
                output.extend_from_slice(cell.contents().as_bytes());
            } else {
                output.push(b' ');
            }
        }
        output.extend_from_slice("\x1b[0;32m│".as_bytes());
    }

    if height > 1 {
        let _ = write!(output, "\x1b[{height};1H\x1b[32m╰");
        for _ in 0..width.saturating_sub(2) {
            output.extend_from_slice("─".as_bytes());
        }
        if width > 1 {
            output.extend_from_slice("╯".as_bytes());
        }
    }

    let (cursor_row, cursor_column) = screen.cursor_position();
    let _ = write!(
        output,
        "\x1b[0m\x1b]0;rustmux:{}\x07\x1b[?1{}\x1b[?2004{}\x1b[{};{}H\x1b[?25{}",
        windows[active].id,
        if screen.application_cursor() {
            'h'
        } else {
            'l'
        },
        if screen.bracketed_paste() { 'h' } else { 'l' },
        cursor_row + 2,
        cursor_column + 2,
        if screen.hide_cursor() { 'l' } else { 'h' },
    );
    output
}

fn draw_top_bar(output: &mut Vec<u8>, windows: &[Window], active: usize, width: u16) {
    if width == 0 {
        return;
    }
    output.extend_from_slice("╭".as_bytes());
    let inner_width = usize::from(width.saturating_sub(2));
    let mut used = 0;
    for (index, window) in windows.iter().enumerate() {
        if used >= inner_width {
            break;
        }
        if used > 0 {
            output.extend_from_slice("─".as_bytes());
            used += 1;
        }
        let label = format!(" {}:{} ", window.id, window.name);
        let available = inner_width.saturating_sub(used);
        let label: String = label.chars().take(available).collect();
        if index == active {
            output.extend_from_slice(b"\x1b[1;30;42m");
        } else {
            output.extend_from_slice(b"\x1b[0;32m");
        }
        output.extend_from_slice(label.as_bytes());
        output.extend_from_slice(b"\x1b[0;32m");
        used += label.chars().count();
    }
    for _ in used..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    if width > 1 {
        output.extend_from_slice("╮".as_bytes());
    }
}

fn write_cell_style(output: &mut Vec<u8>, style: CellStyle) {
    output.extend_from_slice(b"\x1b[0m");
    if style.bold {
        output.extend_from_slice(b"\x1b[1m");
    }
    if style.dim {
        output.extend_from_slice(b"\x1b[2m");
    }
    if style.italic {
        output.extend_from_slice(b"\x1b[3m");
    }
    if style.underline {
        output.extend_from_slice(b"\x1b[4m");
    }
    if style.inverse {
        output.extend_from_slice(b"\x1b[7m");
    }
    write_color(output, style.foreground, true);
    write_color(output, style.background, false);
}

fn write_color(output: &mut Vec<u8>, color: vt100::Color, foreground: bool) {
    let base = if foreground { 38 } else { 48 };
    match color {
        vt100::Color::Default => {}
        vt100::Color::Idx(index) => {
            let _ = write!(output, "\x1b[{base};5;{index}m");
        }
        vt100::Color::Rgb(red, green, blue) => {
            let _ = write!(output, "\x1b[{base};2;{red};{green};{blue}m");
        }
    }
}

fn terminal_responses(output: &[u8]) -> Vec<u8> {
    let mut responses = Vec::new();
    if output.windows(4).any(|window| window == b"\x1b[0c") {
        responses.extend_from_slice(b"\x1b[?1;2c");
    }
    if output.windows(4).any(|window| window == b"\x1b[?u") {
        responses.extend_from_slice(b"\x1b[?0u");
    }
    if output.windows(5).any(|window| window == b"\x1b[>0q") {
        responses.extend_from_slice(b"\x1bP>|rustmux 0.1.0\x1b\\");
    }
    if output.windows(8).any(|window| window == b"\x1b]11;?\x1b\\") {
        responses.extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x1b\\");
    }
    responses
}

fn write_fd(fd: &OwnedFd, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        let count = write(fd, bytes)?;
        bytes = &bytes[count..];
    }
    Ok(())
}

fn content_size((columns, rows): (u16, u16)) -> (u16, u16) {
    (
        columns.saturating_sub(2).max(1),
        rows.saturating_sub(2).max(1),
    )
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

    fn test_window(id: usize, name: &str, rows: u16, columns: u16) -> Window {
        // SAFETY: dup returns a new descriptor owned solely by this test.
        let fd = unsafe { nix::libc::dup(nix::libc::STDOUT_FILENO) };
        assert!(fd >= 0);
        // SAFETY: fd is a valid, newly duplicated descriptor.
        let master = unsafe { OwnedFd::from_raw_fd(fd) };
        Window {
            id,
            name: name.to_owned(),
            master,
            child: Pid::from_raw(1),
            terminal: vt100::Parser::new(rows, columns, SCROLLBACK_LINES),
        }
    }

    #[test]
    fn winsize_maps_columns_and_rows() {
        let value = winsize((120, 40));
        assert_eq!(value.ws_col, 120);
        assert_eq!(value.ws_row, 40);
    }

    #[test]
    fn frame_has_green_border_tabs_and_terminal_contents() {
        let mut first = test_window(1, "fish", 3, 18);
        first.terminal.process(b"hello \x1b[38;2;1;2;3mcolor");
        let second = test_window(2, "fish", 3, 18);

        let frame = render_frame(&[first, second], 0, (20, 5));
        let frame = String::from_utf8(frame).expect("rendered frame is UTF-8");

        assert!(frame.contains("\x1b[32m"));
        assert!(frame.contains('╭'));
        assert!(frame.contains('╯'));
        assert!(frame.contains(" 1:fish "));
        assert!(frame.contains(" 2:fish "));
        assert!(frame.contains("hello"));
        assert!(frame.contains("\x1b[38;2;1;2;3m"));
    }

    #[test]
    fn terminal_queries_receive_local_responses() {
        let responses = terminal_responses(b"\x1b[?u\x1b[>0q\x1b]11;?\x1b\\\x1b[0c");

        assert!(responses.windows(7).any(|part| part == b"\x1b[?1;2c"));
        assert!(responses.windows(5).any(|part| part == b"\x1b[?0u"));
        let background = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
        assert!(
            responses
                .windows(background.len())
                .any(|part| part == background)
        );
    }

    #[test]
    fn decoder_accepts_all_prefix_encodings() {
        let mut decoder = InputDecoder::default();
        let input = b"a\x02b\x1b[98;5uc\x1b[27;5;98~d";

        assert_eq!(decoder.push(input), b"a\x02b\x02c\x02d");
        assert!(decoder.flush().is_empty());
    }

    #[test]
    fn decoder_handles_split_kitty_sequence() {
        let mut decoder = InputDecoder::default();

        assert!(decoder.push(b"\x1b[98;").is_empty());
        assert_eq!(decoder.push(b"5u"), b"\x02");
    }

    #[test]
    fn decoder_preserves_unrecognized_escape_sequences() {
        let mut decoder = InputDecoder::default();

        assert_eq!(decoder.push(b"\x1b[A"), b"\x1b[A");
        assert!(decoder.flush().is_empty());
    }
}
