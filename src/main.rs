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
    terminal::{disable_raw_mode, enable_raw_mode, window_size},
};
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, poll};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, execvp, read, write};

const PREFIX: u8 = 0x02; // Ctrl-b
const SCROLLBACK_LINES: usize = 1_000;
const ESCAPE_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(50);
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
const MAX_KITTY_COMMAND_BYTES: usize = 64 * 1024 * 1024;
const MAX_PTY_READS_PER_TICK: usize = 32;
const MOUSE_SCROLL_LINES: usize = 3;
const ENCODED_PREFIXES: [&[u8]; 2] = [b"\x1b[98;5u", b"\x1b[27;5;98~"];

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b[?1000h\x1b[?1006h")?;
        stdout.flush()?;
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
            b"\x1b[?2026l\x1b[0 q\x1b[0m\x1b[?1l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b>",
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
    terminal: vt100::Parser<TerminalMetadata>,
    cursor_style: CursorStyleTracker,
    kitty_graphics: KittyGraphicsParser,
    pending_graphics: Vec<Vec<u8>>,
    history_mode: bool,
}

#[derive(Default)]
struct TerminalMetadata {
    title: String,
}

impl vt100::Callbacks for TerminalMetadata {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title)
            .chars()
            .filter(|character| !character.is_control())
            .collect();
    }
}

impl Window {
    fn terminal_title(&self) -> &str {
        let title = self.terminal.callbacks().title.trim();
        if title.is_empty() { &self.name } else { title }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryAction {
    Up(usize),
    Down(usize),
    Top,
    Bottom,
    Exit,
    Ignore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseAction {
    ScrollUp,
    ScrollDown,
    Other,
}

#[derive(Default)]
struct KittyGraphicsParser {
    state: KittyGraphicsState,
    utf8_continuations: u8,
}

#[derive(Default)]
enum KittyGraphicsState {
    #[default]
    Ground,
    Escape,
    ApcPrefix {
        command: Vec<u8>,
        c1: bool,
    },
    Graphics {
        command: Vec<u8>,
        escape: bool,
        c1: bool,
    },
    Discard {
        escape: bool,
        c1: bool,
    },
    PassthroughApc {
        escape: bool,
        c1: bool,
    },
}

#[derive(Default)]
struct KittyGraphicsOutput {
    commands: Vec<Vec<u8>>,
    terminal: Vec<u8>,
}

impl KittyGraphicsParser {
    fn process(&mut self, bytes: &[u8]) -> KittyGraphicsOutput {
        let mut output = KittyGraphicsOutput {
            commands: Vec::new(),
            terminal: Vec::with_capacity(bytes.len().min(4096)),
        };
        for &byte in bytes {
            if matches!(self.state, KittyGraphicsState::Ground) {
                if self.utf8_continuations > 0 {
                    if byte & 0xc0 == 0x80 {
                        output.terminal.push(byte);
                        self.utf8_continuations -= 1;
                        continue;
                    }
                    self.utf8_continuations = 0;
                }
                self.utf8_continuations = match byte {
                    0xc2..=0xdf => 1,
                    0xe0..=0xef => 2,
                    0xf0..=0xf4 => 3,
                    _ => 0,
                };
                if self.utf8_continuations > 0 {
                    output.terminal.push(byte);
                    continue;
                }
            }
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                KittyGraphicsState::Ground => match byte {
                    0x1b => KittyGraphicsState::Escape,
                    0x9f => KittyGraphicsState::ApcPrefix {
                        command: vec![0x9f],
                        c1: true,
                    },
                    _ => {
                        output.terminal.push(byte);
                        KittyGraphicsState::Ground
                    }
                },
                KittyGraphicsState::Escape => match byte {
                    b'_' => KittyGraphicsState::ApcPrefix {
                        command: vec![0x1b, b'_'],
                        c1: false,
                    },
                    0x1b => {
                        output.terminal.push(0x1b);
                        KittyGraphicsState::Escape
                    }
                    0x9f => {
                        output.terminal.push(0x1b);
                        KittyGraphicsState::ApcPrefix {
                            command: vec![0x9f],
                            c1: true,
                        }
                    }
                    _ => {
                        output.terminal.extend_from_slice(&[0x1b, byte]);
                        KittyGraphicsState::Ground
                    }
                },
                KittyGraphicsState::ApcPrefix { mut command, c1 } => {
                    if byte == b'G' {
                        command.push(byte);
                        KittyGraphicsState::Graphics {
                            command,
                            escape: false,
                            c1,
                        }
                    } else {
                        output.terminal.extend_from_slice(&command);
                        output.terminal.push(byte);
                        if c1 && byte == 0x9c {
                            KittyGraphicsState::Ground
                        } else {
                            KittyGraphicsState::PassthroughApc {
                                escape: byte == 0x1b,
                                c1,
                            }
                        }
                    }
                }
                KittyGraphicsState::Graphics {
                    mut command,
                    escape,
                    c1,
                } => {
                    command.push(byte);
                    let terminated = (c1 && byte == 0x9c) || (!c1 && escape && byte == b'\\');
                    if terminated {
                        output.commands.push(command);
                        KittyGraphicsState::Ground
                    } else if command.len() >= MAX_KITTY_COMMAND_BYTES {
                        KittyGraphicsState::Discard {
                            escape: byte == 0x1b,
                            c1,
                        }
                    } else {
                        KittyGraphicsState::Graphics {
                            command,
                            escape: byte == 0x1b,
                            c1,
                        }
                    }
                }
                KittyGraphicsState::Discard { escape, c1 } => {
                    let terminated = (c1 && byte == 0x9c) || (!c1 && escape && byte == b'\\');
                    if terminated {
                        KittyGraphicsState::Ground
                    } else {
                        KittyGraphicsState::Discard {
                            escape: byte == 0x1b,
                            c1,
                        }
                    }
                }
                KittyGraphicsState::PassthroughApc { escape, c1 } => {
                    output.terminal.push(byte);
                    let terminated = (c1 && byte == 0x9c) || (!c1 && escape && byte == b'\\');
                    if terminated {
                        KittyGraphicsState::Ground
                    } else {
                        KittyGraphicsState::PassthroughApc {
                            escape: byte == 0x1b,
                            c1,
                        }
                    }
                }
            };
        }
        output
    }
}

#[derive(Default)]
struct CursorStyleTracker {
    state: CursorSequenceState,
    style: u8,
}

#[derive(Default)]
enum CursorSequenceState {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
}

impl CursorStyleTracker {
    fn process(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                CursorSequenceState::Ground => match byte {
                    0x1b => CursorSequenceState::Escape,
                    0x9b => CursorSequenceState::Csi(Vec::new()),
                    _ => CursorSequenceState::Ground,
                },
                CursorSequenceState::Escape => match byte {
                    b'[' => CursorSequenceState::Csi(Vec::new()),
                    0x1b => CursorSequenceState::Escape,
                    _ => CursorSequenceState::Ground,
                },
                CursorSequenceState::Csi(mut parameters) => {
                    if byte == 0x1b {
                        CursorSequenceState::Escape
                    } else if (0x40..=0x7e).contains(&byte) {
                        if byte == b'q' {
                            self.apply_decscusr(&parameters);
                        }
                        CursorSequenceState::Ground
                    } else if parameters.len() < 16 {
                        parameters.push(byte);
                        CursorSequenceState::Csi(parameters)
                    } else {
                        CursorSequenceState::Ground
                    }
                }
            };
        }
    }

    fn apply_decscusr(&mut self, parameters: &[u8]) {
        let Some(digits) = parameters.strip_suffix(b" ") else {
            return;
        };
        if !digits.iter().all(u8::is_ascii_digit) {
            return;
        }
        let style = digits.iter().fold(0_u16, |value, digit| {
            value
                .saturating_mul(10)
                .saturating_add(u16::from(digit - b'0'))
        });
        if style <= 6 {
            self.style = style as u8;
        }
    }
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
            if self.pending == [0x1b]
                || (self.pending.starts_with(b"\x1b[")
                    && !self.pending[2..]
                        .iter()
                        .any(|byte| (0x40..=0x7e).contains(byte)))
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
    renderer: Renderer,
    terminal_size: (u16, u16),
    terminal_pixels: (u16, u16),
    terminal_identity: String,
    redraw_deadline: Option<Instant>,
}

impl App {
    fn new() -> Result<Self> {
        let outer = window_size()?;
        let terminal_size = (outer.columns, outer.rows);
        let mut app = Self {
            windows: Vec::new(),
            active: 0,
            next_id: 1,
            prefix_pending: false,
            input_decoder: InputDecoder::default(),
            renderer: Renderer::default(),
            terminal_size,
            terminal_pixels: (outer.width, outer.height),
            terminal_identity: outer_terminal_identity(),
            redraw_deadline: None,
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
        let winsize = content_winsize(self.terminal_size, self.terminal_pixels);
        let (columns, rows) = (winsize.ws_col, winsize.ws_row);

        // SAFETY: the child immediately calls execvp and _exit, both of which are
        // async-signal-safe; all application bookkeeping remains in the parent.
        match unsafe { forkpty(&winsize, None) }? {
            ForkptyResult::Parent { child, master } => {
                let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL)?);
                fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
                let id = self.next_id;
                self.next_id += 1;
                self.windows.push(Window {
                    id,
                    name,
                    master,
                    child,
                    terminal: vt100::Parser::new_with_callbacks(
                        rows,
                        columns,
                        SCROLLBACK_LINES,
                        TerminalMetadata::default(),
                    ),
                    cursor_style: CursorStyleTracker::default(),
                    kitty_graphics: KittyGraphicsParser::default(),
                    pending_graphics: Vec::new(),
                    history_mode: false,
                });
                self.active = self.windows.len() - 1;
                self.renderer.invalidate();
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
        let mut output = [0_u8; 64 * 1024];

        while !self.windows.is_empty() {
            self.update_size()?;
            self.flush_scheduled_redraw()?;
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

            match poll(&mut poll_fds, self.poll_timeout()) {
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
                    for _ in 0..MAX_PTY_READS_PER_TICK {
                        match read(&self.windows[index].master, &mut output) {
                            Ok(0) | Err(Errno::EIO) | Err(Errno::EAGAIN) => break,
                            Ok(count) => self.process_pty_output(index, &output[..count])?,
                            Err(error) => return Err(error.into()),
                        }
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
            self.flush_scheduled_redraw()?;
        }
        Ok(())
    }

    fn handle_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let decoded = self.input_decoder.push(bytes);
        self.handle_decoded_input(&decoded)
    }

    fn handle_decoded_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let mut passthrough = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some((mouse, consumed)) = decode_sgr_mouse(&bytes[index..]) {
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                self.prefix_pending = false;
                self.apply_mouse_action(mouse)?;
                index += consumed;
                continue;
            } else if self.prefix_pending {
                self.prefix_pending = false;
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                match byte {
                    b'c' => self.create_window()?,
                    b'n' => self.select_relative(1)?,
                    b'p' => self.select_relative(-1)?,
                    b'[' => self.enter_history_mode()?,
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
            } else if self.windows[self.active].history_mode {
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                let (action, consumed) =
                    decode_history_action(&bytes[index..], self.history_page_rows());
                self.apply_history_action(action)?;
                index += consumed;
                continue;
            } else {
                passthrough.push(byte);
            }
            index += 1;
        }
        if !passthrough.is_empty() && !self.windows.is_empty() {
            self.write_active(&passthrough)?;
        }
        Ok(true)
    }

    fn history_page_rows(&self) -> usize {
        usize::from(content_size(self.terminal_size).1.saturating_sub(1).max(1))
    }

    fn enter_history_mode(&mut self) -> Result<()> {
        let window = &mut self.windows[self.active];
        if !window.history_mode {
            window.history_mode = true;
            self.redraw()?;
        }
        Ok(())
    }

    fn apply_history_action(&mut self, action: HistoryAction) -> Result<()> {
        let window = &mut self.windows[self.active];
        let current = window.terminal.screen().scrollback();
        let mut exit = false;
        let requested = match action {
            HistoryAction::Up(rows) => current.saturating_add(rows),
            HistoryAction::Down(rows) => current.saturating_sub(rows),
            HistoryAction::Top => usize::MAX,
            HistoryAction::Bottom => 0,
            HistoryAction::Exit => {
                exit = true;
                0
            }
            HistoryAction::Ignore => return Ok(()),
        };
        window.terminal.screen_mut().set_scrollback(requested);
        let changed = current != window.terminal.screen().scrollback() || exit;
        if exit {
            window.history_mode = false;
        }
        if changed {
            self.redraw()?;
        }
        Ok(())
    }

    fn apply_mouse_action(&mut self, action: MouseAction) -> Result<()> {
        let window = &mut self.windows[self.active];
        let current = window.terminal.screen().scrollback();
        let was_history_mode = window.history_mode;
        match action {
            MouseAction::ScrollUp => {
                window.history_mode = true;
                window
                    .terminal
                    .screen_mut()
                    .set_scrollback(current.saturating_add(MOUSE_SCROLL_LINES));
            }
            MouseAction::ScrollDown if window.history_mode => {
                window
                    .terminal
                    .screen_mut()
                    .set_scrollback(current.saturating_sub(MOUSE_SCROLL_LINES));
                if window.terminal.screen().scrollback() == 0 {
                    window.history_mode = false;
                }
            }
            MouseAction::ScrollDown | MouseAction::Other => return Ok(()),
        }
        if was_history_mode != window.history_mode
            || current != window.terminal.screen().scrollback()
        {
            self.redraw()?;
        }
        Ok(())
    }

    fn process_pty_output(&mut self, index: usize, output: &[u8]) -> Result<()> {
        let parsed = self.windows[index].kitty_graphics.process(output);
        let terminal_changed = !parsed.terminal.is_empty();
        self.windows[index].cursor_style.process(&parsed.terminal);
        let mut graphics_responses = Vec::new();
        let mut graphics_changed = false;
        let mut flush_graphics_immediately = false;
        for command in parsed.commands {
            if let Some(response) = kitty_graphics_query_response(&command) {
                graphics_responses.extend_from_slice(&response);
            } else {
                flush_graphics_immediately |= kitty_graphics_uses_shared_memory(&command);
                self.windows[index].pending_graphics.push(command);
                graphics_changed = true;
            }
        }
        self.windows[index].terminal.process(&parsed.terminal);
        let screen = self.windows[index].terminal.screen();
        graphics_responses.extend_from_slice(&terminal_responses(
            &parsed.terminal,
            content_winsize(self.terminal_size, self.terminal_pixels),
            &self.terminal_identity,
            screen.cursor_position(),
            screen.bracketed_paste(),
        ));
        if !graphics_responses.is_empty() {
            write_fd(&self.windows[index].master, &graphics_responses)?;
        }
        if index == self.active && (terminal_changed || graphics_changed) {
            if flush_graphics_immediately {
                // A shared-memory object must be opened by the outer terminal
                // promptly. Flush every older graphics command with it so a
                // queued delete cannot overtake and erase the new upload.
                self.redraw()?;
            } else {
                self.schedule_redraw();
            }
        }
        Ok(())
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
            self.renderer.invalidate();
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
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        self.redraw_deadline = None;
        if self.windows.is_empty() {
            return Ok(());
        }
        let graphics = std::mem::take(&mut self.windows[self.active].pending_graphics);
        let frame = self
            .renderer
            .render(&self.windows, self.active, self.terminal_size, &graphics);
        if frame.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(&frame)?;
        stdout.flush()?;
        Ok(())
    }

    fn schedule_redraw(&mut self) {
        self.redraw_deadline
            .get_or_insert_with(|| Instant::now() + FRAME_INTERVAL);
    }

    fn flush_scheduled_redraw(&mut self) -> Result<()> {
        if self
            .redraw_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.redraw()?;
        }
        Ok(())
    }

    fn poll_timeout(&self) -> u16 {
        let Some(deadline) = self.redraw_deadline else {
            return 100;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return 0;
        }
        remaining.as_millis().clamp(1, 100) as u16
    }

    fn show_help(&self) -> Result<()> {
        let mut stdout = io::stdout();
        write!(
            stdout,
            "\r\n\x1b[1m[rustmux] Ctrl-b commands:\x1b[0m c=new  n=next  p=previous  [=history  &=close  d=detach/quit  Ctrl-b=send prefix\r\n"
        )?;
        stdout.flush()?;
        Ok(())
    }

    fn update_size(&mut self) -> Result<()> {
        let outer = window_size()?;
        let new_size = (outer.columns, outer.rows);
        let new_pixels = (outer.width, outer.height);
        if new_size == self.terminal_size && new_pixels == self.terminal_pixels {
            return Ok(());
        }
        self.terminal_size = new_size;
        self.terminal_pixels = new_pixels;
        let winsize = content_winsize(new_size, new_pixels);
        let (columns, rows) = (winsize.ws_col, winsize.ws_row);
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
        self.renderer.invalidate();
        self.redraw()?;
        Ok(())
    }

    fn reap_children(&mut self) -> Result<()> {
        let mut removed_any = false;
        let mut index = 0;
        while index < self.windows.len() {
            let pid = self.windows[index].child;
            match waitpid(pid, Some(WaitPidFlag::WNOHANG))? {
                WaitStatus::StillAlive => index += 1,
                _ => {
                    removed_any = true;
                    self.windows.remove(index);
                    if index < self.active {
                        self.active -= 1;
                    }
                }
            }
        }
        if !self.windows.is_empty() {
            self.active = self.active.min(self.windows.len() - 1);
            if removed_any {
                self.renderer.invalidate();
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

fn decode_sgr_mouse(bytes: &[u8]) -> Option<(MouseAction, usize)> {
    if !bytes.starts_with(b"\x1b[<") {
        return None;
    }
    let final_offset = bytes[3..]
        .iter()
        .position(|byte| *byte == b'M' || *byte == b'm')?;
    let final_index = final_offset + 3;
    let button = bytes[3..final_index]
        .split(|byte| *byte == b';')
        .next()
        .and_then(|digits| {
            digits.iter().try_fold(0_u16, |value, digit| {
                digit
                    .is_ascii_digit()
                    .then(|| value.saturating_mul(10) + u16::from(digit - b'0'))
            })
        });
    let action = match button {
        Some(button) if button & 64 != 0 && button & 3 == 0 => MouseAction::ScrollUp,
        Some(button) if button & 64 != 0 && button & 3 == 1 => MouseAction::ScrollDown,
        _ => MouseAction::Other,
    };
    Some((action, final_index + 1))
}

fn decode_history_action(bytes: &[u8], page_rows: usize) -> (HistoryAction, usize) {
    match bytes[0] {
        b'k' | 0x10 => (HistoryAction::Up(1), 1),
        b'j' | 0x0e => (HistoryAction::Down(1), 1),
        b'u' | 0x15 => (HistoryAction::Up(page_rows), 1),
        b'd' | 0x04 => (HistoryAction::Down(page_rows), 1),
        b'g' => (HistoryAction::Top, 1),
        b'G' => (HistoryAction::Bottom, 1),
        b'q' => (HistoryAction::Exit, 1),
        0x1b if bytes.len() == 1 || bytes.get(1) != Some(&b'[') => (HistoryAction::Exit, 1),
        0x1b => {
            let Some(final_offset) = bytes[2..]
                .iter()
                .position(|byte| (0x40..=0x7e).contains(byte))
            else {
                return (HistoryAction::Ignore, bytes.len());
            };
            let final_index = final_offset + 2;
            let consumed = final_index + 1;
            let action = match bytes[final_index] {
                b'A' => HistoryAction::Up(1),
                b'B' => HistoryAction::Down(1),
                b'H' => HistoryAction::Top,
                b'F' => HistoryAction::Bottom,
                b'~' => match bytes[2..final_index].split(|byte| *byte == b';').next() {
                    Some(b"5") => HistoryAction::Up(page_rows),
                    Some(b"6") => HistoryAction::Down(page_rows),
                    Some(b"1" | b"7") => HistoryAction::Top,
                    Some(b"4" | b"8") => HistoryAction::Bottom,
                    _ => HistoryAction::Ignore,
                },
                _ => HistoryAction::Ignore,
            };
            (action, consumed)
        }
        _ => (HistoryAction::Ignore, 1),
    }
}

#[derive(Default)]
struct Renderer {
    previous: Option<FrameSnapshot>,
}

impl Renderer {
    fn invalidate(&mut self) {
        self.previous = None;
    }

    fn render(
        &mut self,
        windows: &[Window],
        active: usize,
        terminal_size: (u16, u16),
        graphics: &[Vec<u8>],
    ) -> Vec<u8> {
        let current = FrameSnapshot::capture(windows, active, terminal_size);
        let Some(previous) = &self.previous else {
            let output = render_frame(windows, active, terminal_size, graphics);
            self.previous = Some(current);
            return output;
        };

        if previous.terminal_size != current.terminal_size
            || previous.active_id != current.active_id
            || previous.tabs != current.tabs
            || previous.cells.len() != current.cells.len()
        {
            let output = render_frame(windows, active, terminal_size, graphics);
            self.previous = Some(current);
            return output;
        }

        let (content_columns, content_rows) = content_size(terminal_size);
        let columns = usize::from(content_columns);
        let mut changes = Vec::new();
        for row in 0..usize::from(content_rows) {
            let start = row * columns;
            let before = &previous.cells[start..start + columns];
            let after = &current.cells[start..start + columns];
            let Some(mut first) = before.iter().zip(after).position(|(a, b)| a != b) else {
                continue;
            };
            let last = before
                .iter()
                .zip(after)
                .rposition(|(a, b)| a != b)
                .expect("a changed row has a final changed cell");
            if after[first].wide_continuation && first > 0 {
                first -= 1;
            }
            changes.push((row, first, last));
        }

        let cells_changed = !changes.is_empty();
        let state_changed = previous.terminal_state != current.terminal_state;
        let graphics_changed = !graphics.is_empty();
        let history_changed = previous.history_mode != current.history_mode
            || previous.history_offset != current.history_offset;
        let title_changed = previous.terminal_title != current.terminal_title;
        let mut output = Vec::new();
        // Keep potentially multi-megabyte image uploads outside synchronized
        // text updates. Some terminals cap or time out synchronized buffers;
        // the following text frame will expose the uploaded image atomically
        // when it paints the Unicode placeholders.
        if graphics_changed && !previous.terminal_state.hide_cursor {
            output.extend_from_slice(b"\x1b[?25l");
        }
        append_graphics(&mut output, graphics, &current.terminal_state);
        if cells_changed || state_changed || graphics_changed || history_changed || title_changed {
            // DEC synchronized output makes the terminal display this diff as one
            // frame. Unknown DEC private modes are safely ignored by terminals
            // which do not implement mode 2026.
            output.extend_from_slice(b"\x1b[?2026h");
        }
        if (cells_changed || history_changed)
            && !graphics_changed
            && !previous.terminal_state.hide_cursor
        {
            output.extend_from_slice(b"\x1b[?25l");
        }
        if history_changed {
            let _ = write!(output, "\x1b[1;1H\x1b[32m");
            draw_window_bar(&mut output, windows, active, terminal_size.0);
        }
        if title_changed {
            let _ = write!(output, "\x1b[2;1H\x1b[32m");
            draw_terminal_border(&mut output, &current.terminal_title, terminal_size.0);
        }
        for (row, first, last) in changes {
            let _ = write!(output, "\x1b[{};{}H", row + 3, first + 2);
            let mut previous_style = None;
            for cell in &current.cells[row * columns + first..=row * columns + last] {
                if cell.wide_continuation {
                    continue;
                }
                if previous_style != Some(cell.style) {
                    write_cell_style(&mut output, cell.style);
                    previous_style = Some(cell.style);
                }
                if cell.contents.is_empty() {
                    output.push(b' ');
                } else {
                    output.extend_from_slice(cell.contents.as_bytes());
                }
            }
        }

        if cells_changed || state_changed || graphics_changed || history_changed || title_changed {
            append_terminal_state_diff(
                &mut output,
                &previous.terminal_state,
                &current.terminal_state,
                cells_changed || graphics_changed || history_changed || title_changed,
            );
            output.extend_from_slice(b"\x1b[?2026l");
        }
        self.previous = Some(current);
        output
    }
}

#[derive(Eq, PartialEq)]
struct FrameSnapshot {
    terminal_size: (u16, u16),
    active_id: usize,
    tabs: Vec<(usize, String)>,
    terminal_title: String,
    history_mode: bool,
    history_offset: usize,
    cells: Vec<CellSnapshot>,
    terminal_state: TerminalState,
}

impl FrameSnapshot {
    fn capture(windows: &[Window], active: usize, terminal_size: (u16, u16)) -> Self {
        let screen = windows[active].terminal.screen();
        let (columns, rows) = content_size(terminal_size);
        let mut cells = Vec::with_capacity(usize::from(columns) * usize::from(rows));
        for row in 0..rows {
            for column in 0..columns {
                let cell = screen.cell(row, column).expect("cell is within screen");
                cells.push(CellSnapshot {
                    contents: cell.contents().to_owned(),
                    style: CellStyle::from(cell),
                    wide_continuation: cell.is_wide_continuation(),
                });
            }
        }
        Self {
            terminal_size,
            active_id: windows[active].id,
            tabs: windows
                .iter()
                .map(|window| (window.id, window.name.clone()))
                .collect(),
            terminal_title: windows[active].terminal_title().to_owned(),
            history_mode: windows[active].history_mode,
            history_offset: screen.scrollback(),
            cells,
            terminal_state: TerminalState::capture(
                screen,
                windows[active].cursor_style.style,
                windows[active].history_mode,
            ),
        }
    }
}

#[derive(Eq, PartialEq)]
struct CellSnapshot {
    contents: String,
    style: CellStyle,
    wide_continuation: bool,
}

#[derive(Eq, PartialEq)]
struct TerminalState {
    cursor: (u16, u16),
    application_cursor: bool,
    bracketed_paste: bool,
    hide_cursor: bool,
    cursor_style: u8,
}

impl TerminalState {
    fn capture(screen: &vt100::Screen, cursor_style: u8, force_hide_cursor: bool) -> Self {
        Self {
            cursor: screen.cursor_position(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            hide_cursor: force_hide_cursor || screen.hide_cursor(),
            cursor_style,
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

fn render_frame(
    windows: &[Window],
    active: usize,
    terminal_size: (u16, u16),
    graphics: &[Vec<u8>],
) -> Vec<u8> {
    let (width, height) = terminal_size;
    let (content_columns, content_rows) = content_size(terminal_size);
    let screen = windows[active].terminal.screen();
    let mut output = Vec::with_capacity(usize::from(width) * usize::from(height) * 2);

    output.extend_from_slice(b"\x1b[?25l\x1b[2J");
    let state = TerminalState::capture(
        screen,
        windows[active].cursor_style.style,
        windows[active].history_mode,
    );
    append_graphics(&mut output, graphics, &state);
    output.extend_from_slice(b"\x1b[H");
    draw_window_bar(&mut output, windows, active, width);
    let _ = write!(output, "\x1b[2;1H\x1b[32m");
    draw_terminal_border(&mut output, windows[active].terminal_title(), width);

    for row in 0..content_rows {
        let _ = write!(output, "\x1b[{};1H\x1b[32m│\x1b[0m", row + 3);
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
        let _ = write!(output, "\x1b[{height};1H\x1b[32m└");
        for _ in 0..width.saturating_sub(2) {
            output.extend_from_slice("─".as_bytes());
        }
        if width > 1 {
            output.extend_from_slice("┘".as_bytes());
        }
    }

    append_terminal_state(&mut output, &state, windows[active].id);
    output
}

fn append_graphics(output: &mut Vec<u8>, graphics: &[Vec<u8>], state: &TerminalState) {
    if graphics.is_empty() {
        return;
    }
    output.reserve(graphics.iter().map(Vec::len).sum());
    let (row, column) = state.cursor;
    let _ = write!(output, "\x1b[{};{}H", row + 3, column + 2);
    for command in graphics {
        output.extend_from_slice(command);
    }
}

fn append_terminal_state(output: &mut Vec<u8>, state: &TerminalState, active_id: usize) {
    let (cursor_row, cursor_column) = state.cursor;
    let _ = write!(
        output,
        "\x1b[0m\x1b]0;rustmux:{}\x07\x1b[?1{}\x1b[?2004{}\x1b[{} q\x1b[{};{}H\x1b[?25{}",
        active_id,
        if state.application_cursor { 'h' } else { 'l' },
        if state.bracketed_paste { 'h' } else { 'l' },
        state.cursor_style,
        cursor_row + 3,
        cursor_column + 2,
        if state.hide_cursor { 'l' } else { 'h' },
    );
}

fn append_terminal_state_diff(
    output: &mut Vec<u8>,
    previous: &TerminalState,
    current: &TerminalState,
    cells_changed: bool,
) {
    output.extend_from_slice(b"\x1b[0m");
    if previous.application_cursor != current.application_cursor {
        let _ = write!(
            output,
            "\x1b[?1{}",
            if current.application_cursor { 'h' } else { 'l' }
        );
    }
    if previous.bracketed_paste != current.bracketed_paste {
        let _ = write!(
            output,
            "\x1b[?2004{}",
            if current.bracketed_paste { 'h' } else { 'l' }
        );
    }
    if previous.cursor_style != current.cursor_style {
        let _ = write!(output, "\x1b[{} q", current.cursor_style);
    }
    if cells_changed || previous.cursor != current.cursor {
        let (row, column) = current.cursor;
        let _ = write!(output, "\x1b[{};{}H", row + 3, column + 2);
    }
    if cells_changed || previous.hide_cursor != current.hide_cursor {
        let _ = write!(
            output,
            "\x1b[?25{}",
            if current.hide_cursor { 'l' } else { 'h' }
        );
    }
}

fn draw_window_bar(output: &mut Vec<u8>, windows: &[Window], active: usize, width: u16) {
    if width == 0 {
        return;
    }
    // Reset to the outer terminal's default background before erasing. EL
    // paints with the current background, which may otherwise leak a child
    // application's grey background into the transparent tab-bar cells.
    output.extend_from_slice(b"\x1b[0;49m\x1b[2K");
    let inner_width = usize::from(width);
    let mut used = 0;
    for (index, window) in windows.iter().enumerate() {
        if used + 2 > inner_width {
            break;
        }
        let label = if index == active && window.history_mode {
            format!(
                " {} {} [history {}] ",
                window.id,
                window.name,
                window.terminal.screen().scrollback()
            )
        } else {
            format!(" {} {} ", window.id, window.name)
        };
        let available = inner_width.saturating_sub(used).saturating_sub(2);
        let label: String = label.chars().take(available).collect();
        if index == active {
            // Use the matching left-facing cap instead of Zellij's reversed
            // right separator: a default background can remain transparent,
            // while a default foreground cannot be made transparent.
            output.extend_from_slice("\x1b[0;32;49m".as_bytes());
            output.extend_from_slice(b"\x1b[1;30;42m");
        } else {
            output.extend_from_slice("\x1b[0;38;2;205;214;244;49m".as_bytes());
            output.extend_from_slice(b"\x1b[1;30;48;2;205;214;244m");
        }
        output.extend_from_slice(label.as_bytes());
        if index == active {
            output.extend_from_slice("\x1b[0;32;49m".as_bytes());
        } else {
            output.extend_from_slice("\x1b[0;38;2;205;214;244;49m".as_bytes());
        }
        used += label.chars().count() + 2;
    }
    output.extend_from_slice(b"\x1b[0m");
}

fn draw_terminal_border(output: &mut Vec<u8>, title: &str, width: u16) {
    if width == 0 {
        return;
    }
    output.extend_from_slice("\x1b[0;49m\x1b[2K\x1b[32m┌".as_bytes());
    let inner_width = usize::from(width.saturating_sub(2));
    let decorated = format!("─ {title} ");
    let title: String = decorated.chars().take(inner_width).collect();
    output.extend_from_slice(title.as_bytes());
    for _ in title.chars().count()..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    if width > 1 {
        output.extend_from_slice("┐".as_bytes());
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

fn terminal_responses(
    output: &[u8],
    window: Winsize,
    terminal_identity: &str,
    cursor: (u16, u16),
    bracketed_paste: bool,
) -> Vec<u8> {
    const MODE_QUERIES: &[(&[u8], u16)] = &[
        (b"\x1b[?69$p", 69),
        (b"\x1b[?2004$p", 2004),
        (b"\x1b[?2026$p", 2026),
        (b"\x1b[?2027$p", 2027),
        (b"\x1b[?2031$p", 2031),
        (b"\x1b[?2048$p", 2048),
    ];
    let mut responses = Vec::new();
    for position in output
        .iter()
        .enumerate()
        .filter_map(|(position, byte)| (*byte == 0x1b).then_some(position))
    {
        let query = &output[position..];
        let mode = MODE_QUERIES
            .iter()
            .find_map(|(request, mode)| query.starts_with(request).then_some(*mode));
        if let Some(mode) = mode {
            let status = match mode {
                2004 if bracketed_paste => 1,
                2004 | 2026 => 2,
                _ => 0,
            };
            let _ = write!(responses, "\x1b[?{mode};{status}$y");
        } else if query.starts_with(b"\x1bP$qm\x1b\\") {
            responses.extend_from_slice(b"\x1bP1$r0m\x1b\\");
        } else if query.starts_with(b"\x1b[0c") || query.starts_with(b"\x1b[c") {
            responses.extend_from_slice(b"\x1b[?1;2c");
        } else if query.starts_with(b"\x1b[?u") {
            responses.extend_from_slice(b"\x1b[?0u");
        } else if query.starts_with(b"\x1b[5n") {
            responses.extend_from_slice(b"\x1b[0n");
        } else if query.starts_with(b"\x1b[6n") {
            let _ = write!(responses, "\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1);
        } else if query.starts_with(b"\x1b[>0q") || query.starts_with(b"\x1b[>q") {
            let _ = write!(responses, "\x1bP>|{terminal_identity}\x1b\\");
        } else if query.starts_with(b"\x1b]11;?\x1b\\") || query.starts_with(b"\x1b]11;?\x07") {
            responses.extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x1b\\");
        } else if window.ws_xpixel > 0 && window.ws_ypixel > 0 && query.starts_with(b"\x1b[14t") {
            let _ = write!(
                responses,
                "\x1b[4;{};{}t",
                window.ws_ypixel, window.ws_xpixel
            );
        } else if window.ws_xpixel > 0 && window.ws_ypixel > 0 && query.starts_with(b"\x1b[16t") {
            let cell_width = window.ws_xpixel / window.ws_col.max(1);
            let cell_height = window.ws_ypixel / window.ws_row.max(1);
            let _ = write!(responses, "\x1b[6;{cell_height};{cell_width}t");
        }
    }
    responses
}

fn outer_terminal_identity() -> String {
    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();
    let term = env::var("TERM").unwrap_or_default();
    let fingerprint = format!("{term_program} {term}").to_ascii_lowercase();
    let name = if env::var_os("KITTY_WINDOW_ID").is_some() || fingerprint.contains("kitty") {
        "kitty"
    } else if fingerprint.contains("ghostty") {
        "ghostty"
    } else if fingerprint.contains("wezterm") {
        "wezterm"
    } else {
        "rustmux"
    };
    let version = env::var("TERM_PROGRAM_VERSION")
        .ok()
        .filter(|version| {
            !version.is_empty()
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    format!("{name} {version}")
}

fn kitty_graphics_query_response(command: &[u8]) -> Option<Vec<u8>> {
    let control_start = if command.starts_with(b"\x1b_G") {
        3
    } else if command.starts_with(b"\x9fG") {
        2
    } else {
        return None;
    };
    let control_end = command[control_start..]
        .iter()
        .position(|&byte| byte == b';')?
        + control_start;
    let control = &command[control_start..control_end];
    if !control
        .split(|&byte| byte == b',')
        .any(|field| field == b"a=q")
    {
        return None;
    }

    let mut response = b"\x1b_G".to_vec();
    let mut first = true;
    for field in control.split(|&byte| byte == b',') {
        if field.starts_with(b"i=") || field.starts_with(b"I=") {
            if !first {
                response.push(b',');
            }
            response.extend_from_slice(field);
            first = false;
        }
    }
    response.extend_from_slice(b";OK\x1b\\");
    Some(response)
}

fn kitty_graphics_uses_shared_memory(command: &[u8]) -> bool {
    let control_start = if command.starts_with(b"\x1b_G") {
        3
    } else if command.starts_with(b"\x9fG") {
        2
    } else {
        return false;
    };
    let control_end = command[control_start..]
        .iter()
        .position(|&byte| byte == b';')
        .map_or(command.len(), |end| control_start + end);
    command[control_start..control_end]
        .split(|&byte| byte == b',')
        .any(|field| field == b"t=s")
}

fn write_fd(fd: &OwnedFd, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        match write(fd, bytes) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
            Ok(count) => bytes = &bytes[count..],
            Err(Errno::EINTR) => {}
            Err(Errno::EAGAIN) => {
                let mut poll_fd = [PollFd::new(fd.as_fd(), PollFlags::POLLOUT)];
                poll(&mut poll_fd, 100_u16)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn content_size((columns, rows): (u16, u16)) -> (u16, u16) {
    (
        columns.saturating_sub(2).max(1),
        rows.saturating_sub(3).max(1),
    )
}

fn content_winsize(terminal_size: (u16, u16), terminal_pixels: (u16, u16)) -> Winsize {
    let (columns, rows) = content_size(terminal_size);
    let (outer_columns, outer_rows) = terminal_size;
    let (outer_width, outer_height) = terminal_pixels;
    let cell_width = outer_width.checked_div(outer_columns).unwrap_or(0);
    let cell_height = outer_height.checked_div(outer_rows).unwrap_or(0);
    Winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: cell_width.saturating_mul(columns),
        ws_ypixel: cell_height.saturating_mul(rows),
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
            terminal: vt100::Parser::new_with_callbacks(
                rows,
                columns,
                SCROLLBACK_LINES,
                TerminalMetadata::default(),
            ),
            cursor_style: CursorStyleTracker::default(),
            kitty_graphics: KittyGraphicsParser::default(),
            pending_graphics: Vec::new(),
            history_mode: false,
        }
    }

    #[test]
    fn content_winsize_excludes_border_cells_and_preserves_cell_pixels() {
        let value = content_winsize((218, 62), (3706, 2046));
        assert_eq!(value.ws_col, 216);
        assert_eq!(value.ws_row, 59);
        assert_eq!(value.ws_xpixel, 3672);
        assert_eq!(value.ws_ypixel, 1947);
    }

    #[test]
    fn history_keys_support_lines_pages_and_boundaries() {
        assert_eq!(decode_history_action(b"k", 20), (HistoryAction::Up(1), 1));
        assert_eq!(
            decode_history_action(b"\x1b[B", 20),
            (HistoryAction::Down(1), 3)
        );
        assert_eq!(
            decode_history_action(b"\x1b[5~", 20),
            (HistoryAction::Up(20), 4)
        );
        assert_eq!(
            decode_history_action(b"\x1b[6~", 20),
            (HistoryAction::Down(20), 4)
        );
        assert_eq!(decode_history_action(b"g", 20), (HistoryAction::Top, 1));
        assert_eq!(decode_history_action(b"\x1b", 20), (HistoryAction::Exit, 1));
    }

    #[test]
    fn sgr_mouse_decoder_recognizes_vertical_wheel_events() {
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<64;10;5M"),
            Some((MouseAction::ScrollUp, 11))
        );
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<69;10;5M"),
            Some((MouseAction::ScrollDown, 11))
        );
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<66;10;5M"),
            Some((MouseAction::Other, 11))
        );
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<0;10;5M"),
            Some((MouseAction::Other, 10))
        );
        assert_eq!(decode_sgr_mouse(b"\x1b[<64;10"), None);
    }

    #[test]
    fn history_mode_renders_offset_without_clearing_the_screen() {
        let mut window = test_window(1, "fish", 3, 18);
        window.terminal.process(b"one\r\ntwo\r\nthree\r\nfour");
        let mut windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (20, 5), &[]);

        windows[0].history_mode = true;
        windows[0].terminal.screen_mut().set_scrollback(1);
        let history = renderer.render(&windows, 0, (20, 5), &[]);
        let history_text = String::from_utf8_lossy(&history);
        assert!(history_text.contains("[history 1"));
        assert!(history_text.contains("\x1b[?25l"));
        assert!(!history.windows(4).any(|part| part == b"\x1b[2J"));

        windows[0].history_mode = false;
        windows[0].terminal.screen_mut().set_scrollback(0);
        let live = renderer.render(&windows, 0, (20, 5), &[]);
        let live_text = String::from_utf8_lossy(&live);
        assert!(!live_text.contains("[history"));
        assert!(live_text.contains("\x1b[?25h"));
        assert!(!live.windows(4).any(|part| part == b"\x1b[2J"));
    }

    #[test]
    fn frame_has_green_border_tabs_and_terminal_contents() {
        let mut first = test_window(1, "fish", 3, 18);
        first.terminal.process(b"hello \x1b[38;2;1;2;3mcolor");
        first.terminal.process(b"\x1b]2;nvim project\x07");
        let second = test_window(2, "fish", 3, 18);

        let frame = render_frame(&[first, second], 0, (20, 5), &[]);
        let frame = String::from_utf8(frame).expect("rendered frame is UTF-8");

        assert!(frame.contains("\x1b[32m"));
        assert!(frame.contains('┌'));
        assert!(frame.contains('┘'));
        assert!(frame.contains(" 1 fish "));
        assert!(frame.contains(" 2 fish "));
        assert!(frame.contains("\x1b[0;32;49m\x1b[1;30;42m 1 fish \x1b[0;32;49m"));
        assert!(frame.contains(
            "\x1b[0;38;2;205;214;244;49m\x1b[1;30;48;2;205;214;244m 2 fish \x1b[0;38;2;205;214;244;49m"
        ));
        assert!(frame.contains("─ nvim project "));
        assert!(frame.contains("hello"));
        assert!(frame.contains("\x1b[38;2;1;2;3m"));
    }

    #[test]
    fn incremental_render_does_not_clear_the_screen() {
        let window = test_window(1, "fish", 3, 18);
        let mut windows = vec![window];
        let mut renderer = Renderer::default();

        let initial = renderer.render(&windows, 0, (20, 5), &[]);
        assert!(initial.windows(4).any(|part| part == b"\x1b[2J"));

        windows[0].terminal.process(b"x");
        let update = renderer.render(&windows, 0, (20, 5), &[]);
        assert!(!update.windows(4).any(|part| part == b"\x1b[2J"));
        assert!(update.starts_with(b"\x1b[?2026h"));
        assert!(update.ends_with(b"\x1b[?2026l"));
        assert!(update.contains(&b'x'));
        assert!(update.len() < initial.len());

        assert!(renderer.render(&windows, 0, (20, 5), &[]).is_empty());

        windows[0].terminal.process(b"\x1b]2;nvim\x07");
        let title_update = renderer.render(&windows, 0, (20, 5), &[]);
        let title_update = String::from_utf8(title_update).expect("title update is UTF-8");
        assert!(title_update.contains("\x1b[2;1H"));
        assert!(title_update.contains("─ nvim "));
        assert!(!title_update.contains("\x1b[2J"));
    }

    #[test]
    fn adjacent_cell_changes_are_written_as_one_run() {
        let window = test_window(1, "fish", 3, 18);
        let mut windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (20, 5), &[]);

        windows[0].terminal.process(b"abcdef");
        let update = renderer.render(&windows, 0, (20, 5), &[]);

        // One CUP starts the changed run and one restores the application cursor.
        assert_eq!(update.iter().filter(|&&byte| byte == b'H').count(), 2);
        assert!(update.windows(6).any(|part| part == b"abcdef"));
    }

    #[test]
    fn cursor_style_tracker_handles_split_decscusr_sequences() {
        let mut tracker = CursorStyleTracker::default();

        tracker.process(b"ignored\x1b[5");
        assert_eq!(tracker.style, 0);
        tracker.process(b" q");
        assert_eq!(tracker.style, 5);

        tracker.process(b"\x1b[2 q");
        assert_eq!(tracker.style, 2);
        tracker.process(b"\x1b[99 q");
        assert_eq!(tracker.style, 2);
    }

    #[test]
    fn renderer_forwards_cursor_style_changes() {
        let window = test_window(1, "fish", 3, 18);
        let mut windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (20, 5), &[]);

        windows[0].cursor_style.process(b"\x1b[6 q");
        let update = renderer.render(&windows, 0, (20, 5), &[]);

        assert!(update.windows(5).any(|part| part == b"\x1b[6 q"));
    }

    #[test]
    fn kitty_graphics_parser_handles_chunked_apc_commands() {
        let mut parser = KittyGraphicsParser::default();
        let command = b"\x1b_Ga=T,f=100,m=0;YWJj\x1b\\";

        let first = parser.process(b"text\x1b_Ga=T,f=100,");
        assert!(first.commands.is_empty());
        assert_eq!(first.terminal, b"text");
        let middle = parser.process(b"m=0;YWJj\x1b");
        assert!(middle.commands.is_empty());
        assert!(middle.terminal.is_empty());
        let last = parser.process(b"\\tail");
        assert_eq!(last.commands, vec![command.to_vec()]);
        assert_eq!(last.terminal, b"tail");

        let passthrough = parser.process(b"\x1b_not-kitty\x1b\\");
        assert!(passthrough.commands.is_empty());
        assert_eq!(passthrough.terminal, b"\x1b_not-kitty\x1b\\");
        let c1 = parser.process(b"\x9fGa=d,d=A\x9c");
        assert_eq!(c1.commands, vec![b"\x9fGa=d,d=A\x9c".to_vec()]);
        assert!(c1.terminal.is_empty());
    }

    #[test]
    fn kitty_graphics_parser_does_not_treat_utf8_continuations_as_c1_controls() {
        let mut parser = KittyGraphicsParser::default();
        let command = b"\x1b_Gq=2,a=T,t=s,i=42;/yazi-image\x1b\\";

        let first = parser.process(&[0xf0]);
        assert_eq!(first.terminal, [0xf0]);
        assert!(first.commands.is_empty());

        let mut remainder = vec![0x9f, 0x93, 0x84]; // U+1F4C4 PAGE FACING UP
        remainder.extend_from_slice(command);
        let second = parser.process(&remainder);
        assert_eq!(second.terminal, [0x9f, 0x93, 0x84]);
        assert_eq!(second.commands, vec![command.to_vec()]);
    }

    #[test]
    fn kitty_graphics_query_is_acknowledged_before_device_attributes() {
        let query = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";
        let mut responses = kitty_graphics_query_response(query).expect("query response");
        responses.extend_from_slice(&terminal_responses(
            b"\x1b[c",
            content_winsize((80, 24), (1360, 792)),
            "kitty 0.40.0",
            (0, 0),
            false,
        ));

        assert_eq!(responses, b"\x1b_Gi=31;OK\x1b\\\x1b[?1;2c");
        assert!(kitty_graphics_query_response(b"\x1b_Ga=p,i=31;\x1b\\").is_none());
    }

    #[test]
    fn kitty_shared_memory_uploads_are_detected_for_immediate_forwarding() {
        assert!(kitty_graphics_uses_shared_memory(
            b"\x1b_Gq=2,a=T,t=s,S=534240,i=31;L3lhemktaW1hZ2U=\x1b\\"
        ));
        assert!(!kitty_graphics_uses_shared_memory(
            b"\x1b_Gq=2,a=T,t=d,f=24,i=31;AAAA\x1b\\"
        ));
    }

    #[test]
    fn large_kitty_payload_is_forwarded_without_text_parsing() {
        let payload = vec![b'A'; 9 * 1024 * 1024];
        let mut command = b"\x1b_Ga=T,f=100,m=0;".to_vec();
        command.extend_from_slice(&payload);
        command.extend_from_slice(b"\x1b\\");

        let mut parser = KittyGraphicsParser::default();
        let parsed = parser.process(&command);

        assert_eq!(parsed.commands, vec![command]);
        assert!(parsed.terminal.is_empty());
    }

    #[test]
    fn renderer_preserves_kitty_delete_upload_and_placeholder_order() {
        let window = test_window(1, "fish", 3, 18);
        let mut windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (20, 5), &[]);

        let placeholder = "\u{10eeee}\u{0305}\u{0305}";
        let contents = format!("\x1b[38;2;0;0;42m{placeholder}");
        windows[0].terminal.process(contents.as_bytes());
        let delete = b"\x1b_Gq=2,a=d,d=A;\x1b\\";
        let graphics = b"\x1b_Ga=T,t=s,U=1,i=42,c=1,r=1;/rustmux-image\x1b\\";
        let graphics_commands = vec![delete.to_vec(), graphics.to_vec()];
        let update = renderer.render(&windows, 0, (20, 5), &graphics_commands);

        let delete_at = update
            .windows(delete.len())
            .position(|part| part == delete)
            .expect("graphics deletion is forwarded");
        let graphics_at = update
            .windows(graphics.len())
            .position(|part| part == graphics)
            .expect("graphics command is forwarded");
        let synchronized_update_at = update
            .windows(b"\x1b[?2026h".len())
            .position(|part| part == b"\x1b[?2026h")
            .expect("text update is synchronized");
        let placeholder = placeholder.as_bytes();
        let placeholder_at = update
            .windows(placeholder.len())
            .position(|part| part == placeholder)
            .expect("unicode placeholder is rendered");
        assert!(delete_at < graphics_at);
        assert!(graphics_at < synchronized_update_at);
        assert!(synchronized_update_at < placeholder_at);
        let image_id_color = b"\x1b[38;2;0;0;42m";
        assert!(
            update
                .windows(image_id_color.len())
                .any(|part| part == image_id_color)
        );
    }

    #[test]
    fn terminal_queries_receive_local_responses() {
        let responses = terminal_responses(
            b"\x1b[?2004$p\x1b[?2026$p\x1bP$qm\x1b\\\x1b[?u\x1b[5n\x1b[6n\x1b[>q\x1b]11;?\x1b\\\x1b[0c\x1b[14t\x1b[16t",
            content_winsize((80, 24), (1360, 792)),
            "kitty 0.40.0",
            (7, 11),
            false,
        );

        assert!(responses.windows(7).any(|part| part == b"\x1b[?1;2c"));
        assert!(responses.windows(5).any(|part| part == b"\x1b[?0u"));
        assert!(responses.windows(4).any(|part| part == b"\x1b[0n"));
        assert!(responses.windows(7).any(|part| part == b"\x1b[8;12R"));
        let paste_mode = b"\x1b[?2004;2$y";
        assert!(
            responses
                .windows(paste_mode.len())
                .any(|part| part == paste_mode)
        );
        let sync_mode = b"\x1b[?2026;2$y";
        assert!(
            responses
                .windows(sync_mode.len())
                .any(|part| part == sync_mode)
        );
        let sgr_status = b"\x1bP1$r0m\x1b\\";
        assert!(
            responses
                .windows(sgr_status.len())
                .any(|part| part == sgr_status)
        );
        let terminal_identity = b"\x1bP>|kitty 0.40.0\x1b\\";
        assert!(
            responses
                .windows(terminal_identity.len())
                .any(|part| part == terminal_identity)
        );
        let background = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
        assert!(
            responses
                .windows(background.len())
                .any(|part| part == background)
        );
        let bell_background = terminal_responses(
            b"\x1b]11;?\x07",
            content_winsize((80, 24), (1360, 792)),
            "kitty 0.40.0",
            (0, 0),
            false,
        );
        assert_eq!(bell_background, background);
        assert!(
            responses
                .windows(b"\x1b[4;693;1326t".len())
                .any(|part| part == b"\x1b[4;693;1326t")
        );
        assert!(
            responses
                .windows(b"\x1b[6;33;17t".len())
                .any(|part| part == b"\x1b[6;33;17t")
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

    #[test]
    fn decoder_keeps_split_arrow_sequence_together() {
        let mut decoder = InputDecoder::default();

        assert!(decoder.push(b"\x1b[").is_empty());
        assert_eq!(decoder.push(b"A"), b"\x1b[A");
    }
}
