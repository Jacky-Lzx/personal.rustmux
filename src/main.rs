mod config;

use std::env;
use std::error::Error;
use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
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
use nix::unistd::{Pid, execvp, read, tcgetpgrp, write};

use config::{Action, Config, DEFAULT_CONFIG_TOML, config_path};

const PREFIX: u8 = 0x02; // Ctrl-b
const SCROLLBACK_LINES: usize = 1_000;
const ESCAPE_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(50);
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
const EARLY_DISCONNECT_RETRY: Duration = Duration::from_millis(100);
const MAX_KITTY_COMMAND_BYTES: usize = 64 * 1024 * 1024;
const MAX_PTY_READS_PER_TICK: usize = 32;
const MOUSE_SCROLL_LINES: usize = 3;
const ENCODED_PREFIXES: [&[u8]; 2] = [b"\x1b[98;5u", b"\x1b[27;5;98~"];
const CLIENT_INPUT: u8 = b'I';
const CLIENT_RESIZE: u8 = b'R';
const CLIENT_SHUTDOWN: u8 = b'Q';
const MAX_CLIENT_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const CLIPBOARD_STATUS: &str = "copied to system clipboard";
const CLIPBOARD_STATUS_DURATION: Duration = Duration::from_secs(2);

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b[?1002h\x1b[?1006h")?;
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
    command_output: SemanticOutputCapture,
    temporary_file: Option<PathBuf>,
    return_to_window: Option<usize>,
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

#[derive(Default)]
struct SemanticOutputCapture {
    state: TextCaptureState,
    capturing: bool,
    semantic_boundaries: bool,
    current: Vec<u8>,
    last: Vec<u8>,
    command_started_at: Option<Instant>,
}

#[derive(Default)]
enum TextCaptureState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
}

impl SemanticOutputCapture {
    fn process(&mut self, bytes: &[u8]) -> Vec<Duration> {
        let mut completions = Vec::new();
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                TextCaptureState::Ground => match byte {
                    0x1b => TextCaptureState::Escape,
                    b'\r' => TextCaptureState::Ground,
                    b'\n' | b'\t' if self.capturing => {
                        self.push(byte);
                        TextCaptureState::Ground
                    }
                    0x08 if self.capturing => {
                        self.current.pop();
                        TextCaptureState::Ground
                    }
                    0x20..=0x7e | 0x80..=0xff if self.capturing => {
                        self.push(byte);
                        TextCaptureState::Ground
                    }
                    _ => TextCaptureState::Ground,
                },
                TextCaptureState::Escape => match byte {
                    b'[' => TextCaptureState::Csi,
                    b']' => TextCaptureState::Osc(Vec::new()),
                    _ => TextCaptureState::Ground,
                },
                TextCaptureState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        TextCaptureState::Ground
                    } else {
                        TextCaptureState::Csi
                    }
                }
                TextCaptureState::Osc(mut control) => match byte {
                    0x07 => {
                        if let Some(duration) = self.finish_osc(&control) {
                            completions.push(duration);
                        }
                        TextCaptureState::Ground
                    }
                    0x1b => TextCaptureState::OscEscape(control),
                    _ => {
                        if control.len() < 1024 {
                            control.push(byte);
                        }
                        TextCaptureState::Osc(control)
                    }
                },
                TextCaptureState::OscEscape(mut control) => {
                    if byte == b'\\' {
                        if let Some(duration) = self.finish_osc(&control) {
                            completions.push(duration);
                        }
                        TextCaptureState::Ground
                    } else {
                        if control.len() < 1024 {
                            control.extend_from_slice(&[0x1b, byte]);
                        }
                        TextCaptureState::Osc(control)
                    }
                }
            };
        }
        completions
    }

    fn finish_osc(&mut self, control: &[u8]) -> Option<Duration> {
        let marker = control
            .strip_prefix(b"133;")
            .and_then(|value| value.first().copied());
        match marker {
            Some(b'C') => {
                self.current.clear();
                self.capturing = true;
                self.semantic_boundaries = true;
                self.command_started_at = Some(Instant::now());
                None
            }
            Some(b'D') if self.capturing => self.finish_command(),
            Some(b'A') if self.capturing => self.finish_command(),
            _ => None,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.current.len() < MAX_CAPTURE_BYTES {
            self.current.push(byte);
        }
    }

    fn finish_command(&mut self) -> Option<Duration> {
        while self.current.last().is_some_and(u8::is_ascii_whitespace) {
            self.current.pop();
        }
        self.last = std::mem::take(&mut self.current);
        self.capturing = false;
        self.semantic_boundaries = false;
        self.command_started_at
            .take()
            .map(|started| started.elapsed())
    }

    fn last_output(&self) -> String {
        if self.capturing && !self.semantic_boundaries {
            fallback_command_output(&self.current)
        } else {
            String::from_utf8_lossy(&self.last).into_owned()
        }
    }

    fn command_submitted(&mut self) {
        if self.capturing && self.semantic_boundaries {
            return;
        }
        if self.capturing && !self.semantic_boundaries {
            self.last = fallback_command_output(&self.current).into_bytes();
        }
        self.current.clear();
        self.capturing = true;
        self.semantic_boundaries = false;
        self.command_started_at = Some(Instant::now());
    }
}

fn fallback_command_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().collect::<Vec<_>>();
    if !lines.is_empty() {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.pop();
    }
    lines.join("\n").trim_end().to_owned()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryAction {
    Up(usize),
    Down(usize),
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseAction {
    ScrollUp,
    ScrollDown,
    SelectStart(MousePosition),
    SelectExtend(MousePosition),
    SelectEnd(MousePosition),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MousePosition {
    column: u16,
    row: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSelection {
    window_id: usize,
    start: MousePosition,
    end: MousePosition,
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
    next_notification_id: u64,
    input_decoder: InputDecoder,
    config: Config,
    mode: String,
    selection: Option<TextSelection>,
    renderer: Renderer,
    terminal_size: (u16, u16),
    terminal_pixels: (u16, u16),
    terminal_identity: String,
    redraw_deadline: Option<Instant>,
    clipboard_status_until: Option<Instant>,
    client: Option<UnixStream>,
    client_input: Vec<u8>,
}

impl App {
    fn new(terminal_size: (u16, u16), terminal_pixels: (u16, u16), config: Config) -> Result<Self> {
        let mode = config.default_mode.clone();
        let mut app = Self {
            windows: Vec::new(),
            active: 0,
            next_id: 1,
            next_notification_id: 1,
            input_decoder: InputDecoder::default(),
            config,
            mode,
            selection: None,
            renderer: Renderer::default(),
            terminal_size,
            terminal_pixels,
            terminal_identity: outer_terminal_identity(),
            redraw_deadline: None,
            clipboard_status_until: None,
            client: None,
            client_input: Vec::new(),
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
        self.spawn_window(name, shell.clone(), vec![shell], None, None)
    }

    fn spawn_window(
        &mut self,
        name: String,
        program: CString,
        arguments: Vec<CString>,
        temporary_file: Option<PathBuf>,
        return_to_window: Option<usize>,
    ) -> Result<()> {
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
                    command_output: SemanticOutputCapture::default(),
                    temporary_file,
                    return_to_window,
                });
                self.active = self.windows.len() - 1;
                self.renderer.invalidate();
                self.redraw()?;
            }
            ForkptyResult::Child => {
                let _ = execvp(&program, &arguments);
                // SAFETY: exiting directly is required after fork if exec fails.
                unsafe { nix::libc::_exit(127) };
            }
        }
        Ok(())
    }

    fn run_server(&mut self, listener: UnixListener) -> Result<()> {
        listener.set_nonblocking(true)?;
        let mut output = [0_u8; 64 * 1024];

        while !self.windows.is_empty() {
            self.flush_scheduled_redraw()?;
            let expired_input = self.input_decoder.flush_if_expired();
            if !expired_input.is_empty() && !self.handle_decoded_input(&expired_input)? {
                self.detach_client();
            }

            let mut poll_fds = Vec::with_capacity(self.windows.len() + 2);
            poll_fds.push(PollFd::new(listener.as_fd(), PollFlags::POLLIN));
            if let Some(client) = &self.client {
                poll_fds.push(PollFd::new(
                    client.as_fd(),
                    PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                ));
            }
            let pty_start = poll_fds.len();
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
            let listener_ready = poll_fds[0]
                .revents()
                .unwrap_or_else(PollFlags::empty)
                .contains(PollFlags::POLLIN);
            let client_event = self
                .client
                .as_ref()
                .map(|_| poll_fds[1].revents().unwrap_or_else(PollFlags::empty));
            let pty_events: Vec<PollFlags> = poll_fds[pty_start..]
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
            if let Some(event) = client_event {
                if event.contains(PollFlags::POLLIN) && !self.read_client_messages()? {
                    break;
                }
                if event.intersects(PollFlags::POLLHUP | PollFlags::POLLERR)
                    && self.client.is_some()
                {
                    self.detach_client();
                }
            }
            if listener_ready {
                match listener.accept() {
                    Ok((stream, _)) => self.attach_client(stream)?,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            self.flush_scheduled_redraw()?;
        }
        Ok(())
    }

    fn attach_client(&mut self, stream: UnixStream) -> Result<()> {
        // The listener is nonblocking so it can share the server poll loop. On
        // macOS an accepted socket can retain that mode; a large initial frame
        // may then return EAGAIN, which must not be mistaken for a disconnect.
        stream.set_nonblocking(false)?;
        self.client = Some(stream);
        self.client_input.clear();
        self.input_decoder = InputDecoder::default();
        self.reset_mode();
        self.renderer.invalidate();
        self.redraw()
    }

    fn detach_client(&mut self) {
        self.client = None;
        self.client_input.clear();
        self.input_decoder = InputDecoder::default();
        self.reset_mode();
        self.redraw_deadline = None;
        self.clipboard_status_until = None;
        self.renderer.set_border_status(None);
        self.renderer.invalidate();
    }

    fn read_client_messages(&mut self) -> Result<bool> {
        let mut input = [0_u8; 64 * 1024];
        let count = match self
            .client
            .as_mut()
            .expect("client exists")
            .read(&mut input)
        {
            Ok(0) => {
                self.detach_client();
                return Ok(true);
            }
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(true),
            Err(error) => return Err(error.into()),
        };
        self.client_input.extend_from_slice(&input[..count]);

        while let Some(&kind) = self.client_input.first() {
            match kind {
                CLIENT_INPUT => {
                    if self.client_input.len() < 5 {
                        break;
                    }
                    let length = u32::from_be_bytes(
                        self.client_input[1..5]
                            .try_into()
                            .expect("four-byte length"),
                    ) as usize;
                    if length > MAX_CLIENT_MESSAGE_BYTES {
                        return Err("client input message is too large".into());
                    }
                    if self.client_input.len() < 5 + length {
                        break;
                    }
                    let bytes = self.client_input[5..5 + length].to_vec();
                    self.client_input.drain(..5 + length);
                    if !self.handle_input(&bytes)? {
                        self.detach_client();
                        break;
                    }
                }
                CLIENT_RESIZE => {
                    if self.client_input.len() < 9 {
                        break;
                    }
                    let value = &self.client_input[1..9];
                    let columns = u16::from_be_bytes([value[0], value[1]]);
                    let rows = u16::from_be_bytes([value[2], value[3]]);
                    let width = u16::from_be_bytes([value[4], value[5]]);
                    let height = u16::from_be_bytes([value[6], value[7]]);
                    self.client_input.drain(..9);
                    self.update_size((columns, rows), (width, height))?;
                }
                CLIENT_SHUTDOWN => return Ok(false),
                _ => return Err("invalid client message".into()),
            }
        }
        Ok(true)
    }

    fn handle_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let decoded = self.input_decoder.push(bytes);
        self.handle_decoded_input(&decoded)
    }

    fn handle_decoded_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let mut passthrough = Vec::with_capacity(bytes.len());
        let mut index = 0;
        let mut selection_cleared = false;
        while index < bytes.len() {
            if let Some((mouse, consumed)) = decode_sgr_mouse(&bytes[index..]) {
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                self.apply_mouse_action(mouse)?;
                index += consumed;
                continue;
            }

            if !selection_cleared && self.selection.take().is_some() {
                selection_cleared = true;
                self.redraw()?;
            }

            let (key, consumed) = decode_key(&bytes[index..]);
            if let Some(actions) = self
                .config
                .actions(&self.mode, &key.name)
                .map(<[Action]>::to_vec)
            {
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                if !self.execute_actions(&actions)? {
                    return Ok(false);
                }
            } else if self.mode == "locked" {
                passthrough.extend_from_slice(&key.raw);
            }
            index += consumed;
        }
        if !passthrough.is_empty() && !self.windows.is_empty() {
            self.write_active(&passthrough)?;
        }
        Ok(true)
    }

    fn execute_actions(&mut self, actions: &[Action]) -> Result<bool> {
        for action in actions {
            match action {
                Action::SwitchMode(mode) => self.switch_mode(mode)?,
                Action::SendPrefix => self.write_active(&[PREFIX])?,
                Action::SendKey(bytes) => self.write_active(bytes)?,
                Action::NewWindow => self.create_window()?,
                Action::NextWindow => self.select_relative(1)?,
                Action::PreviousWindow => self.select_relative(-1)?,
                Action::GoToWindow(index) => self.select_window(*index)?,
                Action::CloseWindow => self.close_active()?,
                Action::Detach => return Ok(false),
                Action::ShowHelp => self.show_help()?,
                Action::ScrollUp => self.apply_history_action(HistoryAction::Up(1))?,
                Action::ScrollDown => self.apply_history_action(HistoryAction::Down(1))?,
                Action::PageUp => {
                    self.apply_history_action(HistoryAction::Up(self.history_page_rows()))?;
                }
                Action::PageDown => {
                    self.apply_history_action(HistoryAction::Down(self.history_page_rows()))?;
                }
                Action::ScrollTop => self.apply_history_action(HistoryAction::Top)?,
                Action::ScrollBottom => self.apply_history_action(HistoryAction::Bottom)?,
                Action::EditHistory => self.open_history_in_editor()?,
                Action::EditLastOutput => self.open_last_output_in_editor()?,
                Action::CopyLastOutput => self.copy_last_output()?,
            }
        }
        Ok(true)
    }

    fn switch_mode(&mut self, mode: &str) -> Result<()> {
        if !self.config.has_mode(mode) {
            return Err(format!("unknown mode '{mode}'").into());
        }
        if self.mode == "scroll" && mode != "scroll" && !self.windows.is_empty() {
            let window = &mut self.windows[self.active];
            window.history_mode = false;
            window.terminal.screen_mut().set_scrollback(0);
        }
        self.mode = mode.to_owned();
        if mode == "scroll" && !self.windows.is_empty() {
            self.windows[self.active].history_mode = true;
        }
        self.renderer.invalidate();
        self.redraw()
    }

    fn reset_mode(&mut self) {
        self.mode = self.config.default_mode.clone();
        self.selection = None;
        self.clipboard_status_until = None;
        self.renderer.set_border_status(None);
        for window in &mut self.windows {
            window.history_mode = false;
            window.terminal.screen_mut().set_scrollback(0);
        }
    }

    fn history_page_rows(&self) -> usize {
        usize::from(content_size(self.terminal_size).1.saturating_sub(1).max(1))
    }

    fn open_history_in_editor(&mut self) -> Result<()> {
        let text = window_history(&mut self.windows[self.active]);
        self.open_text_in_editor("history", &text)
    }

    fn open_last_output_in_editor(&mut self) -> Result<()> {
        let text = self.windows[self.active].command_output.last_output();
        if text.is_empty() {
            return self.notify("no previous command output (OSC 133 shell integration required)");
        }
        self.open_text_in_editor("last-output", &text)
    }

    fn open_text_in_editor(&mut self, label: &str, text: &str) -> Result<()> {
        let directory = ensure_session_dir()?;
        let path = directory.join(format!(
            "editor-{}-{}-{label}.txt",
            std::process::id(),
            self.next_id
        ));
        fs::write(&path, text)?;
        let shell = CString::new("/bin/sh")?;
        let arguments = vec![
            shell.clone(),
            CString::new("-c")?,
            CString::new("exec ${VISUAL:-${EDITOR:-vi}} \"$1\"")?,
            CString::new("rustmux-editor")?,
            CString::new(path.as_os_str().as_encoded_bytes())?,
        ];
        let return_to = self.windows[self.active].id;
        if let Err(error) = self.spawn_window(
            label.to_owned(),
            shell,
            arguments,
            Some(path.clone()),
            Some(return_to),
        ) {
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(())
    }

    fn copy_last_output(&mut self) -> Result<()> {
        let text = self.windows[self.active].command_output.last_output();
        if text.is_empty() {
            return self.notify("no previous command output (OSC 133 shell integration required)");
        }
        self.copy_to_clipboard(&text)?;
        self.notify("previous command output copied to clipboard")
    }

    fn copy_to_clipboard(&mut self, text: &str) -> Result<()> {
        let encoded = base64_encode(text.as_bytes());
        if let Some(client) = self.client.as_mut() {
            client.write_all(b"\x1b]52;c;")?;
            client.write_all(encoded.as_bytes())?;
            client.write_all(b"\x07")?;
        }
        Ok(())
    }

    fn notify(&mut self, message: &str) -> Result<()> {
        if let Some(client) = self.client.as_mut() {
            writeln!(client, "\r\x1b[1m[rustmux]\x1b[0m {message}\r")?;
        }
        Ok(())
    }

    fn apply_history_action(&mut self, action: HistoryAction) -> Result<()> {
        let window = &mut self.windows[self.active];
        let current = window.terminal.screen().scrollback();
        let requested = match action {
            HistoryAction::Up(rows) => current.saturating_add(rows),
            HistoryAction::Down(rows) => current.saturating_sub(rows),
            HistoryAction::Top => usize::MAX,
            HistoryAction::Bottom => 0,
        };
        window.terminal.screen_mut().set_scrollback(requested);
        if current != window.terminal.screen().scrollback() {
            self.redraw()?;
        }
        Ok(())
    }

    fn apply_mouse_action(&mut self, action: MouseAction) -> Result<()> {
        if let MouseAction::SelectStart(position) = action {
            let Some(position) = self.content_position(position, false) else {
                if self.selection.take().is_some() {
                    self.redraw()?;
                }
                return Ok(());
            };
            self.selection = Some(TextSelection {
                window_id: self.windows[self.active].id,
                start: position,
                end: position,
            });
            return self.redraw();
        }
        if let MouseAction::SelectExtend(position) | MouseAction::SelectEnd(position) = action {
            let Some(mut selection) = self.selection else {
                return Ok(());
            };
            if selection.window_id != self.windows[self.active].id {
                self.selection = None;
                return Ok(());
            }
            selection.end = self
                .content_position(position, true)
                .expect("clamped content position is always available");
            self.selection = Some(selection);
            if matches!(action, MouseAction::SelectEnd(_)) {
                let text = selected_text(&self.windows[self.active], selection, self.terminal_size);
                if !text.is_empty() {
                    self.copy_to_clipboard(&text)?;
                    self.clipboard_status_until = Some(Instant::now() + CLIPBOARD_STATUS_DURATION);
                    self.renderer.set_border_status(Some(CLIPBOARD_STATUS));
                }
            }
            return self.redraw();
        }

        let selection_cleared = matches!(action, MouseAction::ScrollUp | MouseAction::ScrollDown)
            && self.selection.take().is_some();
        let window = &mut self.windows[self.active];
        let current = window.terminal.screen().scrollback();
        let was_history_mode = window.history_mode;
        match action {
            MouseAction::ScrollUp => {
                window.history_mode = true;
                if self.config.has_mode("scroll") {
                    self.mode = "scroll".to_owned();
                }
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
                    self.mode = self.config.default_mode.clone();
                }
            }
            MouseAction::ScrollDown => {
                if selection_cleared {
                    self.redraw()?;
                }
                return Ok(());
            }
            MouseAction::Other => return Ok(()),
            MouseAction::SelectStart(_)
            | MouseAction::SelectExtend(_)
            | MouseAction::SelectEnd(_) => unreachable!(),
        }
        if was_history_mode != window.history_mode
            || current != window.terminal.screen().scrollback()
            || selection_cleared
        {
            self.redraw()?;
        }
        Ok(())
    }

    fn content_position(&self, position: MousePosition, clamp: bool) -> Option<MousePosition> {
        let (columns, rows) = content_size(self.terminal_size);
        let column = i32::from(position.column) - 2;
        let row = i32::from(position.row) - 3;
        if !clamp
            && (column < 0 || row < 0 || column >= i32::from(columns) || row >= i32::from(rows))
        {
            return None;
        }
        Some(MousePosition {
            column: column.clamp(0, i32::from(columns) - 1) as u16,
            row: row.clamp(0, i32::from(rows) - 1) as u16,
        })
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
        let completions = self.windows[index].command_output.process(&parsed.terminal);
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
        if let Some(seconds) = self.config.command_notification_seconds() {
            let threshold = Duration::from_secs(seconds);
            for duration in completions {
                if duration >= threshold {
                    self.notify_command_finished(index, duration)?;
                }
            }
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

    fn notify_command_finished(&mut self, index: usize, duration: Duration) -> Result<()> {
        if self.client.is_none() {
            return Ok(());
        }
        let notification_id = format!(
            "rustmux-{}-{}",
            std::process::id(),
            self.next_notification_id
        );
        self.next_notification_id = self.next_notification_id.wrapping_add(1).max(1);
        let window = &self.windows[index];
        let title = "rustmux: command finished";
        let window_title = window.terminal_title().chars().take(80).collect::<String>();
        let body = format!(
            "Window {} ({}) completed in {}",
            window.id,
            window_title,
            format_duration(duration)
        );
        let notification = kitty_notification(&notification_id, title, &body);
        let client = self.client.as_mut().expect("client was checked above");
        let result = client
            .write_all(&notification)
            .and_then(|()| client.flush());
        if result.is_err() {
            self.detach_client();
        }
        Ok(())
    }

    fn write_active(&mut self, bytes: &[u8]) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
            self.windows[self.active].command_output.command_submitted();
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

    fn select_window(&mut self, index: usize) -> Result<()> {
        if index <= self.windows.len() {
            self.active = index - 1;
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
        let return_to = window.return_to_window;
        terminate_window(window);
        if !self.windows.is_empty() {
            self.active = return_to
                .and_then(|id| self.windows.iter().position(|window| window.id == id))
                .unwrap_or_else(|| self.active.min(self.windows.len() - 1));
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        self.redraw_deadline = None;
        if self.windows.is_empty() || self.client.is_none() {
            return Ok(());
        }
        let graphics = std::mem::take(&mut self.windows[self.active].pending_graphics);
        let frame = self.renderer.render(
            &self.windows,
            self.active,
            self.terminal_size,
            &self.mode,
            self.selection.as_ref(),
            &graphics,
        );
        if frame.is_empty() {
            return Ok(());
        }
        let result = self
            .client
            .as_mut()
            .expect("client exists")
            .write_all(&frame);
        if result.is_err() {
            self.detach_client();
        }
        Ok(())
    }

    fn schedule_redraw(&mut self) {
        self.redraw_deadline
            .get_or_insert_with(|| Instant::now() + FRAME_INTERVAL);
    }

    fn flush_scheduled_redraw(&mut self) -> Result<()> {
        if self
            .clipboard_status_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clipboard_status_until = None;
            self.renderer.set_border_status(None);
            self.redraw()?;
        }
        if self
            .redraw_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.redraw()?;
        }
        Ok(())
    }

    fn poll_timeout(&self) -> u16 {
        let deadline = match (self.redraw_deadline, self.clipboard_status_until) {
            (Some(redraw), Some(status)) => Some(redraw.min(status)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        };
        let Some(deadline) = deadline else {
            return 100;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return 0;
        }
        remaining.as_millis().clamp(1, 100) as u16
    }

    fn show_help(&self) -> Result<()> {
        if let Some(mut client) = self.client.as_ref() {
            let bindings = self.config.describe_mode(&self.mode).join("  ");
            let result = write!(
                client,
                "\r\n\x1b[1m[rustmux] mode {}:\x1b[0m {bindings}\r\nconfig: {}\r\n",
                self.mode,
                config_path().display()
            );
            if result.is_err() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn update_size(&mut self, new_size: (u16, u16), new_pixels: (u16, u16)) -> Result<()> {
        if new_size == self.terminal_size && new_pixels == self.terminal_pixels {
            return Ok(());
        }
        self.terminal_size = new_size;
        self.terminal_pixels = new_pixels;
        self.selection = None;
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
                    let window = self.windows.remove(index);
                    if let Some(path) = window.temporary_file {
                        let _ = fs::remove_file(path);
                    }
                    if index == self.active {
                        if let Some(return_to) = window.return_to_window {
                            self.active = self
                                .windows
                                .iter()
                                .position(|window| window.id == return_to)
                                .unwrap_or(index);
                        }
                    } else if index < self.active {
                        self.active -= 1;
                    }
                }
            }
        }
        if !self.windows.is_empty() {
            self.active = self.active.min(self.windows.len() - 1);
            if removed_any {
                self.selection = None;
                self.renderer.invalidate();
                self.redraw()?;
            }
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        for window in self.windows.drain(..) {
            terminate_window(window);
        }
    }
}

fn window_history(window: &mut Window) -> String {
    let screen = window.terminal.screen_mut();
    let original_offset = screen.scrollback();
    let (_, columns) = screen.size();
    screen.set_scrollback(usize::MAX);
    let scrollback_rows = screen.scrollback();
    let mut lines = Vec::with_capacity(scrollback_rows + usize::from(screen.size().0));

    for row in 0..scrollback_rows {
        screen.set_scrollback(scrollback_rows - row);
        lines.push(screen.rows(0, columns).next().unwrap_or_default());
    }
    screen.set_scrollback(0);
    lines.extend(screen.rows(0, columns));
    screen.set_scrollback(original_offset);

    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut history = lines.join("\n");
    history.push('\n');
    history
}

fn selected_text(window: &Window, selection: TextSelection, terminal_size: (u16, u16)) -> String {
    if selection.window_id != window.id {
        return String::new();
    }
    let (columns, _) = content_size(terminal_size);
    let start_index = usize::from(selection.start.row) * usize::from(columns)
        + usize::from(selection.start.column);
    let end_index =
        usize::from(selection.end.row) * usize::from(columns) + usize::from(selection.end.column);
    let (start, end) = if start_index <= end_index {
        (selection.start, selection.end)
    } else {
        (selection.end, selection.start)
    };
    let screen = window.terminal.screen();
    let mut text = String::new();
    for row in start.row..=end.row {
        let first_column = if row == start.row { start.column } else { 0 };
        let last_column = if row == end.row {
            end.column
        } else {
            columns - 1
        };
        let mut line = String::new();
        for column in first_column..=last_column {
            let cell = screen
                .cell(row, column)
                .expect("selection is within screen");
            if cell.is_wide_continuation() {
                continue;
            }
            if cell.has_contents() {
                line.push_str(cell.contents());
            } else {
                line.push(' ');
            }
        }
        let wrapped = screen.row_wrapped(row) && row != end.row;
        if wrapped {
            text.push_str(&line);
        } else {
            text.push_str(line.trim_end_matches(' '));
        }
        if row != end.row && !wrapped {
            text.push('\n');
        }
    }
    text
}

fn selection_contains(
    selection: Option<&TextSelection>,
    window_id: usize,
    row: u16,
    column: u16,
    columns: u16,
) -> bool {
    let Some(selection) = selection.filter(|selection| selection.window_id == window_id) else {
        return false;
    };
    let position = usize::from(row) * usize::from(columns) + usize::from(column);
    let start = usize::from(selection.start.row) * usize::from(columns)
        + usize::from(selection.start.column);
    let end =
        usize::from(selection.end.row) * usize::from(columns) + usize::from(selection.end.column);
    (start.min(end)..=start.max(end)).contains(&position)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(char::from(TABLE[((value >> 18) & 0x3f) as usize]));
        encoded.push(char::from(TABLE[((value >> 12) & 0x3f) as usize]));
        encoded.push(if chunk.len() > 1 {
            char::from(TABLE[((value >> 6) & 0x3f) as usize])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(TABLE[(value & 0x3f) as usize])
        } else {
            '='
        });
    }
    encoded
}

fn kitty_notification(identifier: &str, title: &str, body: &str) -> Vec<u8> {
    let title = base64_encode(title.as_bytes());
    let body = base64_encode(body.as_bytes());
    format!(
        "\x1b]99;i={identifier}:p=title:d=0:e=1;{title}\x1b\\\x1b]99;i={identifier}:p=body:d=1:e=1;{body}\x1b\\"
    )
    .into_bytes()
}

fn format_duration(duration: Duration) -> String {
    if duration >= Duration::from_secs(60) {
        let total_seconds = duration.as_secs();
        format!("{}m {}s", total_seconds / 60, total_seconds % 60)
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

fn signal_window(window: &Window, signal: Signal) {
    if let Ok(foreground) = tcgetpgrp(&window.master) {
        let _ = kill(Pid::from_raw(-foreground.as_raw()), signal);
    }
    let _ = kill(Pid::from_raw(-window.child.as_raw()), signal);
    let _ = kill(window.child, signal);
}

fn wait_for_child(child: Pid, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match waitpid(child, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(WaitStatus::StillAlive) => return false,
            Ok(_) | Err(Errno::ECHILD) => return true,
            Err(Errno::EINTR) => {}
            Err(_) => return true,
        }
    }
}

fn terminate_window(window: Window) {
    let temporary_file = window.temporary_file.clone();
    signal_window(&window, Signal::SIGHUP);
    if !wait_for_child(window.child, Duration::from_millis(200)) {
        signal_window(&window, Signal::SIGTERM);
        if !wait_for_child(window.child, Duration::from_millis(200)) {
            signal_window(&window, Signal::SIGKILL);
            let _ = wait_for_child(window.child, Duration::from_secs(1));
        }
    }
    if let Some(path) = temporary_file {
        let _ = fs::remove_file(path);
    }
}

struct DecodedKey {
    name: String,
    raw: Vec<u8>,
}

fn decode_key(bytes: &[u8]) -> (DecodedKey, usize) {
    let byte = bytes[0];
    let single = |name: &str| DecodedKey {
        name: name.to_owned(),
        raw: vec![byte],
    };
    match byte {
        b'\r' | b'\n' => (single("enter"), 1),
        b'\t' => (single("tab"), 1),
        0x7f => (single("backspace"), 1),
        0x01..=0x1a => (single(&format!("ctrl {}", char::from(b'a' + byte - 1))), 1),
        0x1b if bytes.get(1) == Some(&b'[') => {
            let Some(final_offset) = bytes[2..]
                .iter()
                .position(|value| (0x40..=0x7e).contains(value))
            else {
                return (single("esc"), 1);
            };
            let consumed = final_offset + 3;
            let raw = bytes[..consumed].to_vec();
            let name = decode_csi_key_name(&raw).unwrap_or_else(|| "unbound-csi".to_owned());
            (DecodedKey { name, raw }, consumed)
        }
        0x1b if bytes.get(1).is_some_and(u8::is_ascii_graphic) => {
            let raw = bytes[..2].to_vec();
            let name = format!("alt {}", char::from(bytes[1]));
            (DecodedKey { name, raw }, 2)
        }
        0x1b => (single("esc"), 1),
        0x20..=0x7e => (single(&char::from(byte).to_string()), 1),
        _ => {
            let width = std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.chars().next())
                .map(char::len_utf8)
                .unwrap_or(1);
            let raw = bytes[..width.min(bytes.len())].to_vec();
            let name = String::from_utf8_lossy(&raw).into_owned();
            (DecodedKey { name, raw }, width.min(bytes.len()))
        }
    }
}

fn decode_csi_key_name(sequence: &[u8]) -> Option<String> {
    let final_byte = *sequence.last()?;
    let parameters = std::str::from_utf8(&sequence[2..sequence.len() - 1]).ok()?;
    match (parameters, final_byte) {
        ("", b'A') => Some("up".to_owned()),
        ("", b'B') => Some("down".to_owned()),
        ("", b'C') => Some("right".to_owned()),
        ("", b'D') => Some("left".to_owned()),
        ("5", b'~') => Some("pageup".to_owned()),
        ("6", b'~') => Some("pagedown".to_owned()),
        (_, b'u') => decode_kitty_key(parameters),
        (_, b'~') if parameters.starts_with("27;") => decode_xterm_modified_key(parameters),
        _ => None,
    }
}

fn decode_kitty_key(parameters: &str) -> Option<String> {
    let mut fields = parameters.split(';');
    let codepoint = fields.next()?.split(':').next()?.parse::<u32>().ok()?;
    let modifiers = fields
        .next()
        .and_then(|value| value.split(':').next())
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    let character = char::from_u32(codepoint)?;
    if modifiers & 4 != 0 && character.is_ascii_alphabetic() {
        Some(format!("ctrl {}", character.to_ascii_lowercase()))
    } else if modifiers & 2 != 0 {
        Some(format!("alt {character}"))
    } else {
        Some(character.to_string())
    }
}

fn decode_xterm_modified_key(parameters: &str) -> Option<String> {
    let mut fields = parameters.split(';');
    (fields.next()? == "27").then_some(())?;
    let modifiers = fields.next()?.parse::<u8>().ok()?.saturating_sub(1);
    let character = char::from_u32(fields.next()?.parse().ok()?)?;
    if modifiers & 4 != 0 && character.is_ascii_alphabetic() {
        Some(format!("ctrl {}", character.to_ascii_lowercase()))
    } else if modifiers & 2 != 0 {
        Some(format!("alt {character}"))
    } else {
        Some(character.to_string())
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
    let mut fields = bytes[3..final_index].split(|byte| *byte == b';');
    let parse_number = |digits: &[u8]| {
        digits.iter().try_fold(0_u16, |value, digit| {
            digit
                .is_ascii_digit()
                .then(|| value.saturating_mul(10) + u16::from(digit - b'0'))
        })
    };
    let button = parse_number(fields.next()?)?;
    let position = MousePosition {
        column: parse_number(fields.next()?)?,
        row: parse_number(fields.next()?)?,
    };
    if fields.next().is_some() {
        return None;
    }
    let released = bytes[final_index] == b'm';
    let action = match button {
        button if button & 64 != 0 && button & 3 == 0 => MouseAction::ScrollUp,
        button if button & 64 != 0 && button & 3 == 1 => MouseAction::ScrollDown,
        button if released && button & 3 == 0 => MouseAction::SelectEnd(position),
        button if button & 32 != 0 && button & 3 == 0 => MouseAction::SelectExtend(position),
        button if button & 32 == 0 && button & 3 == 0 => MouseAction::SelectStart(position),
        _ => MouseAction::Other,
    };
    Some((action, final_index + 1))
}

#[derive(Default)]
struct Renderer {
    previous: Option<FrameSnapshot>,
    border_status: Option<String>,
}

impl Renderer {
    fn invalidate(&mut self) {
        self.previous = None;
    }

    fn set_border_status(&mut self, status: Option<&str>) {
        self.border_status = status.map(str::to_owned);
    }

    fn render(
        &mut self,
        windows: &[Window],
        active: usize,
        terminal_size: (u16, u16),
        mode: &str,
        selection: Option<&TextSelection>,
        graphics: &[Vec<u8>],
    ) -> Vec<u8> {
        let current = FrameSnapshot::capture(
            windows,
            active,
            terminal_size,
            mode,
            selection,
            self.border_status.as_deref(),
        );
        let Some(previous) = &self.previous else {
            let output = render_frame(
                windows,
                active,
                terminal_size,
                mode,
                selection,
                self.border_status.as_deref(),
                graphics,
            );
            self.previous = Some(current);
            return output;
        };

        if previous.terminal_size != current.terminal_size
            || previous.active_id != current.active_id
            || previous.tabs != current.tabs
            || previous.cells.len() != current.cells.len()
        {
            let output = render_frame(
                windows,
                active,
                terminal_size,
                mode,
                selection,
                self.border_status.as_deref(),
                graphics,
            );
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
        let mode_changed = previous.mode != current.mode;
        let title_changed = previous.terminal_title != current.terminal_title;
        let status_changed = previous.border_status != current.border_status;
        let mut output = Vec::new();
        // Keep potentially multi-megabyte image uploads outside synchronized
        // text updates. Some terminals cap or time out synchronized buffers;
        // the following text frame will expose the uploaded image atomically
        // when it paints the Unicode placeholders.
        if graphics_changed && !previous.terminal_state.hide_cursor {
            output.extend_from_slice(b"\x1b[?25l");
        }
        append_graphics(&mut output, graphics, &current.terminal_state);
        if cells_changed
            || state_changed
            || graphics_changed
            || history_changed
            || mode_changed
            || title_changed
            || status_changed
        {
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
        if history_changed || mode_changed {
            let _ = write!(output, "\x1b[1;1H\x1b[32m");
            draw_window_bar(&mut output, windows, active, terminal_size.0, mode);
        }
        if title_changed {
            let _ = write!(output, "\x1b[2;1H\x1b[32m");
            draw_terminal_border(&mut output, &current.terminal_title, terminal_size.0);
        }
        if status_changed && terminal_size.1 > 1 {
            let _ = write!(output, "\x1b[{};1H\x1b[32m", terminal_size.1);
            draw_bottom_border(
                &mut output,
                terminal_size.0,
                current.border_status.as_deref(),
            );
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

        if cells_changed
            || state_changed
            || graphics_changed
            || history_changed
            || mode_changed
            || title_changed
            || status_changed
        {
            append_terminal_state_diff(
                &mut output,
                &previous.terminal_state,
                &current.terminal_state,
                cells_changed
                    || graphics_changed
                    || history_changed
                    || mode_changed
                    || title_changed
                    || status_changed,
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
    mode: String,
    terminal_title: String,
    border_status: Option<String>,
    history_mode: bool,
    history_offset: usize,
    cells: Vec<CellSnapshot>,
    terminal_state: TerminalState,
}

impl FrameSnapshot {
    fn capture(
        windows: &[Window],
        active: usize,
        terminal_size: (u16, u16),
        mode: &str,
        selection: Option<&TextSelection>,
        border_status: Option<&str>,
    ) -> Self {
        let screen = windows[active].terminal.screen();
        let (columns, rows) = content_size(terminal_size);
        let mut cells = Vec::with_capacity(usize::from(columns) * usize::from(rows));
        for row in 0..rows {
            for column in 0..columns {
                let cell = screen.cell(row, column).expect("cell is within screen");
                let mut style = CellStyle::from(cell);
                if selection_contains(selection, windows[active].id, row, column, columns) {
                    style.inverse = !style.inverse;
                }
                cells.push(CellSnapshot {
                    contents: cell.contents().to_owned(),
                    style,
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
            mode: mode.to_owned(),
            terminal_title: windows[active].terminal_title().to_owned(),
            border_status: border_status.map(str::to_owned),
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
    mode: &str,
    selection: Option<&TextSelection>,
    border_status: Option<&str>,
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
    draw_window_bar(&mut output, windows, active, width, mode);
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
            let mut style = CellStyle::from(cell);
            if selection_contains(selection, windows[active].id, row, column, content_columns) {
                style.inverse = !style.inverse;
            }
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
        let _ = write!(output, "\x1b[{height};1H\x1b[32m");
        draw_bottom_border(&mut output, width, border_status);
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

fn draw_window_bar(
    output: &mut Vec<u8>,
    windows: &[Window],
    active: usize,
    width: u16,
    mode: &str,
) {
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
        if used >= inner_width {
            break;
        }
        let label = if index == active && window.history_mode {
            format!(
                " {} {} [{mode} {}] ",
                window.id,
                window.name,
                window.terminal.screen().scrollback()
            )
        } else if index == active && mode != "locked" {
            format!(" {} {} [{mode}] ", window.id, window.name)
        } else {
            format!(" {} {} ", window.id, window.name)
        };
        let available = inner_width.saturating_sub(used).saturating_sub(1);
        let label: String = label.chars().take(available).collect();
        if index == active {
            output.extend_from_slice(b"\x1b[1;30;42m");
        } else {
            output.extend_from_slice(b"\x1b[1;30;48;2;205;214;244m");
        }
        output.extend_from_slice(label.as_bytes());
        output.extend_from_slice(b"\x1b[0;49m ");
        used += label.chars().count() + 1;
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

fn draw_bottom_border(output: &mut Vec<u8>, width: u16, status: Option<&str>) {
    if width == 0 {
        return;
    }
    output.extend_from_slice("└".as_bytes());
    let inner_width = usize::from(width.saturating_sub(2));
    let label = status
        .map(|status| format!("─ {status} "))
        .unwrap_or_default();
    let label = label.chars().take(inner_width).collect::<String>();
    output.extend_from_slice(label.as_bytes());
    for _ in label.chars().count()..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    if width > 1 {
        output.extend_from_slice("┘".as_bytes());
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

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn session_dir() -> PathBuf {
    let uid = unsafe { nix::libc::getuid() };
    env::temp_dir().join(format!("rustmux-{uid}"))
}

fn ensure_session_dir() -> Result<PathBuf> {
    let directory = session_dir();
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}

fn validate_session_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("session names may contain only letters, numbers, '-' and '_'".into());
    }
    Ok(())
}

fn session_socket(name: &str) -> Result<PathBuf> {
    validate_session_name(name)?;
    Ok(session_dir().join(format!("{name}.sock")))
}

fn send_resize(stream: &mut UnixStream, size: &crossterm::terminal::WindowSize) -> Result<()> {
    let mut message = [0_u8; 9];
    message[0] = CLIENT_RESIZE;
    message[1..3].copy_from_slice(&size.columns.to_be_bytes());
    message[3..5].copy_from_slice(&size.rows.to_be_bytes());
    message[5..7].copy_from_slice(&size.width.to_be_bytes());
    message[7..9].copy_from_slice(&size.height.to_be_bytes());
    stream.write_all(&message)?;
    Ok(())
}

fn send_input(stream: &mut UnixStream, bytes: &[u8]) -> Result<()> {
    let length = u32::try_from(bytes.len())?;
    let mut header = [0_u8; 5];
    header[0] = CLIENT_INPUT;
    header[1..].copy_from_slice(&length.to_be_bytes());
    stream.write_all(&header)?;
    stream.write_all(bytes)?;
    Ok(())
}

fn run_client(mut stream: UnixStream) -> Result<()> {
    let mut size = window_size()?;
    send_resize(&mut stream, &size)?;
    let _terminal = TerminalGuard::enter()?;
    let stdin = io::stdin();
    let mut input = [0_u8; 4096];
    let mut output = [0_u8; 64 * 1024];

    loop {
        let (stdin_event, server_event) = {
            let mut poll_fds = [
                PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
                PollFd::new(
                    stream.as_fd(),
                    PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
                ),
            ];
            match poll(&mut poll_fds, 100_u16) {
                Ok(_) => {}
                Err(Errno::EINTR) => continue,
                Err(error) => return Err(error.into()),
            }
            (
                poll_fds[0].revents().unwrap_or_else(PollFlags::empty),
                poll_fds[1].revents().unwrap_or_else(PollFlags::empty),
            )
        };

        if server_event.contains(PollFlags::POLLIN) {
            match stream.read(&mut output) {
                Ok(0) => break,
                Ok(count) => {
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(&output[..count])?;
                    stdout.flush()?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        if server_event.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
            break;
        }
        if stdin_event.contains(PollFlags::POLLIN) {
            let count = read(stdin.as_fd(), &mut input)?;
            if count == 0 {
                break;
            }
            send_input(&mut stream, &input[..count])?;
        }

        let current_size = window_size()?;
        if (
            current_size.columns,
            current_size.rows,
            current_size.width,
            current_size.height,
        ) != (size.columns, size.rows, size.width, size.height)
        {
            size = current_size;
            send_resize(&mut stream, &size)?;
        }
    }
    Ok(())
}

fn start_server(socket: &Path, size: crossterm::terminal::WindowSize) -> Result<()> {
    ensure_session_dir()?;
    if socket.exists() {
        fs::remove_file(socket)?;
    }
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--server")
        .arg(socket)
        .arg(size.columns.to_string())
        .arg(size.rows.to_string())
        .arg(size.width.to_string())
        .arg(size.height.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()?;
    Ok(())
}

fn connect_with_retry(socket: &Path) -> Result<UnixStream> {
    let mut last_error = None;
    for _ in 0..100 {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "session not found"))
        .into())
}

fn attach_or_create(name: &str, create: bool) -> Result<()> {
    Config::load().map_err(|error| format!("configuration error: {error}"))?;
    let socket = session_socket(name)?;
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(error) if !create => {
            return Err(format!("session '{name}' not found: {error}").into());
        }
        Err(_) => {
            let size = window_size()?;
            start_server(&socket, size)?;
            connect_with_retry(&socket)?
        }
    };

    let started = Instant::now();
    run_client(stream)?;
    if started.elapsed() < EARLY_DISCONNECT_RETRY {
        // Servers started by an older rustmux build could accidentally apply a
        // previous client's POLLHUP to a newly accepted connection. Retrying
        // once preserves that in-memory session while recovering transparently.
        thread::sleep(Duration::from_millis(20));
        if let Ok(stream) = UnixStream::connect(&socket) {
            run_client(stream)?;
        }
    }
    Ok(())
}

fn run_server(socket: PathBuf, values: &[String]) -> Result<()> {
    if values.len() != 4 {
        return Err("invalid server arguments".into());
    }
    let columns = values[0].parse()?;
    let rows = values[1].parse()?;
    let width = values[2].parse()?;
    let height = values[3].parse()?;
    ensure_session_dir()?;
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let _socket_guard = SocketGuard(socket);
    let config = Config::load().map_err(|error| format!("configuration error: {error}"))?;
    let mut app = App::new((columns, rows), (width, height), config)?;
    let result = app.run_server(listener);
    app.shutdown();
    result
}

fn list_sessions() -> Result<()> {
    let directory = session_dir();
    if !directory.exists() {
        println!("no sessions");
        return Ok(());
    }
    let mut names = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "sock")
        })
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        println!("no sessions");
    } else {
        for name in names {
            println!("{name}");
        }
    }
    Ok(())
}

fn kill_session(name: &str) -> Result<()> {
    let socket = session_socket(name)?;
    let mut stream = UnixStream::connect(&socket)
        .map_err(|error| format!("session '{name}' not found: {error}"))?;
    stream.write_all(&[CLIENT_SHUTDOWN])?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut response = [0_u8; 4096];
    loop {
        match stream.read(&mut response) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "rustmux {}\n\nA minimal terminal multiplexer.\n\nUSAGE:\n    rustmux [new-session [-s NAME]]\n    rustmux attach-session [-t NAME]\n    rustmux list-sessions\n    rustmux kill-session [-t NAME]\n    rustmux check-config\n    rustmux default-config\n\nConfig: {}\n",
        env!("CARGO_PKG_VERSION"),
        config_path().display(),
    );
}

fn main() -> Result<()> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => attach_or_create("default", true),
        [flag] if flag == "-h" || flag == "--help" => {
            print_help();
            Ok(())
        }
        [flag] if flag == "-V" || flag == "--version" => {
            println!("rustmux {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command] if command == "default-config" => {
            print!("{DEFAULT_CONFIG_TOML}");
            Ok(())
        }
        [command] if command == "check-config" => {
            Config::load().map_err(|error| format!("configuration error: {error}"))?;
            let path = config_path();
            if path.exists() {
                println!("{}: ok", path.display());
            } else {
                println!("{}: not found; built-in defaults are valid", path.display());
            }
            Ok(())
        }
        [command] if command == "new" || command == "new-session" => {
            attach_or_create("default", true)
        }
        [command, flag, name] if (command == "new" || command == "new-session") && flag == "-s" => {
            attach_or_create(name, true)
        }
        [command] if command == "attach" || command == "attach-session" => {
            attach_or_create("default", false)
        }
        [command, flag, name]
            if (command == "attach" || command == "attach-session") && flag == "-t" =>
        {
            attach_or_create(name, false)
        }
        [command] if command == "ls" || command == "list-sessions" => list_sessions(),
        [command] if command == "kill-session" => kill_session("default"),
        [command, flag, name] if command == "kill-session" && flag == "-t" => kill_session(name),
        [command, socket, values @ ..] if command == "--server" => {
            run_server(PathBuf::from(socket), values)
        }
        _ => Err("invalid arguments (try --help)".into()),
    }
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
            command_output: SemanticOutputCapture::default(),
            temporary_file: None,
            return_to_window: None,
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
    fn key_decoder_names_control_navigation_and_modified_keys() {
        assert_eq!(decode_key(b"\x02").0.name, "ctrl b");
        assert_eq!(decode_key(b"\x1b[B").0.name, "down");
        assert_eq!(decode_key(b"\x1b[5~").0.name, "pageup");
        assert_eq!(decode_key(b"\x1b[103;5u").0.name, "ctrl g");
        assert_eq!(decode_key(b"\x1b[27;3;120~").0.name, "alt x");
        assert_eq!(decode_key(b"G").0.name, "G");
    }

    #[test]
    fn sgr_mouse_decoder_recognizes_wheel_and_selection_events() {
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
            Some((
                MouseAction::SelectStart(MousePosition { column: 10, row: 5 }),
                10
            ))
        );
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<32;12;6M"),
            Some((
                MouseAction::SelectExtend(MousePosition { column: 12, row: 6 }),
                11
            ))
        );
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<0;14;6m"),
            Some((
                MouseAction::SelectEnd(MousePosition { column: 14, row: 6 }),
                10
            ))
        );
        assert_eq!(decode_sgr_mouse(b"\x1b[<64;10"), None);
    }

    #[test]
    fn selected_text_spans_rows_and_ignores_terminal_padding() {
        let mut window = test_window(7, "fish", 3, 18);
        window
            .terminal
            .process(b"hello world\r\nsecond line\r\nthird");
        let selection = TextSelection {
            window_id: 7,
            start: MousePosition { column: 0, row: 0 },
            end: MousePosition { column: 5, row: 1 },
        };

        assert_eq!(
            selected_text(&window, selection, (20, 6)),
            "hello world\nsecond"
        );
        assert_eq!(
            selected_text(
                &window,
                TextSelection {
                    start: selection.end,
                    end: selection.start,
                    ..selection
                },
                (20, 6)
            ),
            "hello world\nsecond"
        );
    }

    #[test]
    fn selected_text_does_not_insert_newlines_at_soft_wraps() {
        let mut window = test_window(9, "sh", 2, 5);
        window.terminal.process(b"abcdef");
        let selection = TextSelection {
            window_id: 9,
            start: MousePosition { column: 0, row: 0 },
            end: MousePosition { column: 0, row: 1 },
        };

        assert_eq!(selected_text(&window, selection, (7, 5)), "abcdef");
    }

    #[test]
    fn selection_highlight_is_rendered_incrementally() {
        let mut window = test_window(3, "fish", 3, 18);
        window.terminal.process(b"select me");
        let windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (20, 6), "locked", None, &[]);
        let selection = TextSelection {
            window_id: 3,
            start: MousePosition { column: 0, row: 0 },
            end: MousePosition { column: 5, row: 0 },
        };

        let update = renderer.render(&windows, 0, (20, 6), "locked", Some(&selection), &[]);

        assert!(update.windows(4).any(|part| part == b"\x1b[7m"));
        assert!(!update.windows(4).any(|part| part == b"\x1b[2J"));
    }

    #[test]
    fn clipboard_status_is_drawn_in_the_bottom_border_incrementally() {
        let window = test_window(3, "fish", 3, 38);
        let windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
        renderer.set_border_status(Some(CLIPBOARD_STATUS));

        let shown = renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
        let shown = String::from_utf8(shown).unwrap();
        assert!(shown.contains("\x1b[6;1H"));
        assert!(shown.contains("└─ copied to system clipboard "));
        assert!(!shown.contains("\x1b[2J"));

        renderer.set_border_status(None);
        let cleared = renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
        let cleared = String::from_utf8(cleared).unwrap();
        assert!(cleared.contains("\x1b[6;1H"));
        assert!(!cleared.contains(CLIPBOARD_STATUS));
    }

    #[test]
    fn history_mode_renders_offset_without_clearing_the_screen() {
        let mut window = test_window(1, "fish", 3, 18);
        window.terminal.process(b"one\r\ntwo\r\nthree\r\nfour");
        let mut windows = vec![window];
        let mut renderer = Renderer::default();
        renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

        windows[0].history_mode = true;
        windows[0].terminal.screen_mut().set_scrollback(1);
        let history = renderer.render(&windows, 0, (20, 5), "scroll", None, &[]);
        let history_text = String::from_utf8_lossy(&history);
        assert!(history_text.contains("[scroll 1"));
        assert!(history_text.contains("\x1b[?25l"));
        assert!(!history.windows(4).any(|part| part == b"\x1b[2J"));

        windows[0].history_mode = false;
        windows[0].terminal.screen_mut().set_scrollback(0);
        let live = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
        let live_text = String::from_utf8_lossy(&live);
        assert!(!live_text.contains("[scroll"));
        assert!(live_text.contains("\x1b[?25h"));
        assert!(!live.windows(4).any(|part| part == b"\x1b[2J"));
    }

    #[test]
    fn frame_has_green_border_tabs_and_terminal_contents() {
        let mut first = test_window(1, "fish", 3, 18);
        first.terminal.process(b"hello \x1b[38;2;1;2;3mcolor");
        first.terminal.process(b"\x1b]2;nvim project\x07");
        let second = test_window(2, "fish", 3, 18);

        let frame = render_frame(&[first, second], 0, (20, 5), "locked", None, None, &[]);
        let frame = String::from_utf8(frame).expect("rendered frame is UTF-8");

        assert!(frame.contains("\x1b[32m"));
        assert!(frame.contains('┌'));
        assert!(frame.contains('┘'));
        assert!(frame.contains(" 1 fish "));
        assert!(frame.contains(" 2 fish "));
        assert!(frame.contains("\x1b[1;30;42m 1 fish \x1b[0;49m "));
        assert!(frame.contains("\x1b[1;30;48;2;205;214;244m 2 fish \x1b[0;49m "));
        assert!(!frame.contains(''));
        assert!(!frame.contains(''));
        assert!(frame.contains("─ nvim project "));
        assert!(frame.contains("hello"));
        assert!(frame.contains("\x1b[38;2;1;2;3m"));
    }

    #[test]
    fn incremental_render_does_not_clear_the_screen() {
        let window = test_window(1, "fish", 3, 18);
        let mut windows = vec![window];
        let mut renderer = Renderer::default();

        let initial = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
        assert!(initial.windows(4).any(|part| part == b"\x1b[2J"));

        windows[0].terminal.process(b"x");
        let update = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
        assert!(!update.windows(4).any(|part| part == b"\x1b[2J"));
        assert!(update.starts_with(b"\x1b[?2026h"));
        assert!(update.ends_with(b"\x1b[?2026l"));
        assert!(update.contains(&b'x'));
        assert!(update.len() < initial.len());

        assert!(
            renderer
                .render(&windows, 0, (20, 5), "locked", None, &[])
                .is_empty()
        );

        windows[0].terminal.process(b"\x1b]2;nvim\x07");
        let title_update = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
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
        renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

        windows[0].terminal.process(b"abcdef");
        let update = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

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
        renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

        windows[0].cursor_style.process(b"\x1b[6 q");
        let update = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

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
        renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

        let placeholder = "\u{10eeee}\u{0305}\u{0305}";
        let contents = format!("\x1b[38;2;0;0;42m{placeholder}");
        windows[0].terminal.process(contents.as_bytes());
        let delete = b"\x1b_Gq=2,a=d,d=A;\x1b\\";
        let graphics = b"\x1b_Ga=T,t=s,U=1,i=42,c=1,r=1;/rustmux-image\x1b\\";
        let graphics_commands = vec![delete.to_vec(), graphics.to_vec()];
        let update = renderer.render(&windows, 0, (20, 5), "locked", None, &graphics_commands);

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

    #[test]
    fn semantic_output_capture_tracks_the_previous_command() {
        let mut capture = SemanticOutputCapture::default();
        capture.process(b"\x1b]133;C\x07hello \x1b[31mred\x1b[0m\r\n");
        capture.process(b"second line\x1b]133;D;0\x1b");
        capture.process(b"\\prompt");

        assert_eq!(capture.last_output(), "hello red\nsecond line");
    }

    #[test]
    fn semantic_output_capture_reports_command_duration() {
        let mut capture = SemanticOutputCapture::default();
        let _ = capture.process(b"\x1b]133;C\x07running");
        capture.command_started_at = Some(Instant::now() - Duration::from_secs(11));

        let completions = capture.process(b"\x1b]133;D;0\x07");

        assert_eq!(completions.len(), 1);
        assert!(completions[0] >= Duration::from_secs(11));
        assert!(completions[0] < Duration::from_secs(12));
    }

    #[test]
    fn semantic_command_timer_is_not_reset_by_interactive_input() {
        let mut capture = SemanticOutputCapture::default();
        let _ = capture.process(b"\x1b]133;C\x07");
        let started = capture.command_started_at;

        capture.command_submitted();

        assert_eq!(capture.command_started_at, started);
        assert!(capture.semantic_boundaries);
    }

    #[test]
    fn command_output_has_a_fallback_without_shell_integration() {
        let mut capture = SemanticOutputCapture::default();
        capture.command_submitted();
        capture.process(b"printf test\r\ntest\r\n$ ");

        assert_eq!(capture.last_output(), "test");
    }

    #[test]
    fn history_export_includes_scrollback_and_visible_rows() {
        let mut window = test_window(1, "fish", 3, 18);
        window.terminal.process(b"one\r\ntwo\r\nthree\r\nfour");

        assert_eq!(window_history(&mut window), "one\ntwo\nthree\nfour\n");
    }

    #[test]
    fn osc_52_payload_uses_standard_base64() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode("你好".as_bytes()), "5L2g5aW9");
    }

    #[test]
    fn kitty_notification_uses_osc_99_with_base64_title_and_body() {
        let notification = kitty_notification("rustmux-1-2", "done", "finished in 10.0s");
        let notification = String::from_utf8(notification).unwrap();

        assert_eq!(notification.matches("\x1b]99;").count(), 2);
        assert!(notification.contains("i=rustmux-1-2:p=title:d=0:e=1;ZG9uZQ=="));
        assert!(notification.contains("p=body:d=1:e=1;ZmluaXNoZWQgaW4gMTAuMHM="));
        assert!(notification.ends_with("\x1b\\"));
    }
}
