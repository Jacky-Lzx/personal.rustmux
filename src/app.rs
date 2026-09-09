use std::env;
use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, poll};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, execvp, read, tcgetpgrp, write};

use crate::config::{Action, Config, config_path};
use crate::input::{
    DecodedKey, InputDecoder, MouseAction, MousePosition, decode_key, decode_sgr_mouse,
};
use crate::layout::{
    Direction, PaneNode, PaneRect, SplitAxis, content_rect_for, directional_distance,
    floating_layout_for, pane_ids, pane_pty_size, pane_rects, rect_in_direction, remove_pane,
    split_pane, tiled_content_rect_for, validate_terminal_size, window_winsize_for,
};
use crate::render::{Renderer, render_base_index};
use crate::session::ensure_session_dir;
use crate::terminal::{
    CursorStyleTracker, KittyGraphicsParser, SemanticOutputCapture, TerminalMetadata,
    base64_encode, format_duration, kitty_graphics_query_response,
    kitty_graphics_uses_shared_memory, kitty_notification, outer_terminal_identity,
    terminal_responses,
};
use crate::{
    CLIENT_INPUT, CLIENT_RESIZE, CLIENT_SHUTDOWN, CLIPBOARD_STATUS, CLIPBOARD_STATUS_DURATION,
    FRAME_INTERVAL, MAX_CLIENT_MESSAGE_BYTES, MAX_PTY_READS_PER_TICK, MOUSE_SCROLL_LINES, PREFIX,
    Result, SCROLLBACK_LINES,
};

pub(super) struct Window {
    pub(super) id: usize,
    pub(super) tab_id: usize,
    pub(super) name: String,
    pub(super) floating: bool,
    pub(super) pane_rect: PaneRect,
    pub(super) pane_framed: bool,
    pub(super) master: OwnedFd,
    pub(super) child: Pid,
    pub(super) terminal: vt100::Parser<TerminalMetadata>,
    pub(super) cursor_style: CursorStyleTracker,
    pub(super) kitty_graphics: KittyGraphicsParser,
    pub(super) pending_graphics: Vec<Vec<u8>>,
    pub(super) history_mode: bool,
    pub(super) command_output: SemanticOutputCapture,
    pub(super) temporary_file: Option<PathBuf>,
    pub(super) return_to_window: Option<usize>,
}

#[derive(Clone, Debug)]
struct Tab {
    id: usize,
    root: PaneNode,
}

struct SpawnOptions {
    temporary_file: Option<PathBuf>,
    return_to_window: Option<usize>,
    floating: bool,
    tab_id: usize,
}

impl Window {
    pub(super) fn terminal_title(&self) -> &str {
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TextSelection {
    pub(super) window_id: usize,
    pub(super) start: MousePosition,
    pub(super) end: MousePosition,
}

impl TextSelection {
    pub(super) fn spans_multiple_cells(self) -> bool {
        self.start != self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RenameEdit {
    Continue,
    Confirm,
    Cancel,
}

pub(super) fn edit_window_name(name: &mut String, key: &DecodedKey) -> RenameEdit {
    match key.name.as_str() {
        "enter" => RenameEdit::Confirm,
        "esc" => RenameEdit::Cancel,
        "backspace" => {
            name.pop();
            RenameEdit::Continue
        }
        _ => {
            if let Ok(text) = std::str::from_utf8(&key.raw)
                && text.chars().all(|character| !character.is_control())
                && name.chars().count() + text.chars().count() <= 64
            {
                name.push_str(text);
            }
            RenameEdit::Continue
        }
    }
}

pub(super) fn rename_tab(windows: &mut [Window], tab_id: usize, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    for window in windows
        .iter_mut()
        .filter(|window| !window.floating && window.tab_id == tab_id)
    {
        window.name = name.to_owned();
    }
}

pub(super) struct App {
    windows: Vec<Window>,
    tabs: Vec<Tab>,
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
    rename_input: Option<String>,
    client: Option<UnixStream>,
    client_input: Vec<u8>,
}

impl App {
    pub(super) fn new(
        terminal_size: (u16, u16),
        terminal_pixels: (u16, u16),
        config: Config,
        session_name: &str,
    ) -> Result<Self> {
        let mode = config.default_mode.clone();
        let mut renderer = Renderer::default();
        renderer.set_session_name(session_name);
        let mut app = Self {
            windows: Vec::new(),
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            next_notification_id: 1,
            input_decoder: InputDecoder::default(),
            config,
            mode,
            selection: None,
            renderer,
            terminal_size,
            terminal_pixels,
            terminal_identity: outer_terminal_identity(),
            redraw_deadline: None,
            clipboard_status_until: None,
            rename_input: None,
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
        let tab_id = self.next_id;
        let pane_id = self.spawn_window(
            name.clone(),
            shell.clone(),
            vec![shell],
            SpawnOptions {
                temporary_file: None,
                return_to_window: None,
                floating: false,
                tab_id,
            },
        )?;
        self.tabs.push(Tab {
            id: tab_id,
            root: PaneNode::Leaf(pane_id),
        });
        self.resize_windows()?;
        self.redraw()
    }

    fn toggle_floating_terminal(&mut self) -> Result<()> {
        if self.windows[self.active].floating {
            let return_to = self.windows[self.active].return_to_window;
            self.active = return_to
                .and_then(|id| self.windows.iter().position(|window| window.id == id))
                .or_else(|| self.windows.iter().position(|window| !window.floating))
                .unwrap_or(self.active);
            self.selection = None;
            return self.redraw();
        }

        let return_to = self.windows[self.active].id;
        if let Some(index) = self.windows.iter().position(|window| window.floating) {
            self.windows[index].return_to_window = Some(return_to);
            self.windows[index].tab_id = self.windows[self.active].tab_id;
            self.active = index;
            self.selection = None;
            return self.redraw();
        }

        let shell = env::var("RUSTMUX_SHELL").unwrap_or_else(|_| "fish".to_owned());
        let name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell)
            .to_owned();
        let shell = CString::new(shell)?;
        let tab_id = self.windows[self.active].tab_id;
        self.spawn_window(
            name,
            shell.clone(),
            vec![shell],
            SpawnOptions {
                temporary_file: None,
                return_to_window: Some(return_to),
                floating: true,
                tab_id,
            },
        )?;
        self.redraw()
    }

    fn spawn_window(
        &mut self,
        name: String,
        program: CString,
        arguments: Vec<CString>,
        options: SpawnOptions,
    ) -> Result<usize> {
        let winsize = window_winsize_for(
            self.terminal_size,
            self.terminal_pixels,
            options.floating,
            self.config.compact(),
        );
        let (columns, rows) = (winsize.ws_col, winsize.ws_row);

        // SAFETY: the child immediately calls execvp and _exit, both of which are
        // async-signal-safe; all application bookkeeping remains in the parent.
        match unsafe { forkpty(&winsize, None) }? {
            ForkptyResult::Parent { child, master } => {
                let setup = (|| {
                    let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL)?);
                    fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
                    Ok::<(), Errno>(())
                })();
                if let Err(error) = setup {
                    let _ = kill(child, Signal::SIGKILL);
                    let _ = waitpid(child, None);
                    return Err(error.into());
                }
                let id = self.next_id;
                self.next_id += 1;
                self.windows.push(Window {
                    id,
                    tab_id: options.tab_id,
                    name,
                    floating: options.floating,
                    pane_rect: content_rect_for(self.terminal_size, self.config.compact()),
                    pane_framed: false,
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
                    temporary_file: options.temporary_file,
                    return_to_window: options.return_to_window,
                });
                self.active = self.windows.len() - 1;
                self.renderer.invalidate();
                Ok(id)
            }
            ForkptyResult::Child => {
                let _ = execvp(&program, &arguments);
                // SAFETY: exiting directly is required after fork if exec fails.
                unsafe { nix::libc::_exit(127) };
            }
        }
    }

    pub(super) fn run_server(&mut self, listener: UnixListener) -> Result<()> {
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
                if event.contains(PollFlags::POLLIN) {
                    match self.read_client_messages() {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(_) => self.detach_client(),
                    }
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
            if self.rename_input.is_some() {
                let (key, consumed) = decode_key(&bytes[index..]);
                self.handle_rename_key(&key)?;
                index += consumed;
                continue;
            }
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
                Action::RenameWindow => self.begin_window_rename()?,
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
                Action::ToggleFloatingTerminal => self.toggle_floating_terminal()?,
                Action::NewPaneRight => self.new_pane(SplitAxis::Vertical)?,
                Action::NewPaneDown => self.new_pane(SplitAxis::Horizontal)?,
                Action::FocusLeft => self.focus_pane(Direction::Left)?,
                Action::FocusRight => self.focus_pane(Direction::Right)?,
                Action::FocusUp => self.focus_pane(Direction::Up)?,
                Action::FocusDown => self.focus_pane(Direction::Down)?,
                Action::FocusNextPane => self.focus_next_pane()?,
                Action::ClosePane => self.close_pane()?,
            }
        }
        Ok(true)
    }

    fn begin_window_rename(&mut self) -> Result<()> {
        self.rename_input = Some(String::new());
        self.sync_rename_prompt();
        self.redraw()
    }

    fn handle_rename_key(&mut self, key: &DecodedKey) -> Result<()> {
        let edit = edit_window_name(
            self.rename_input.as_mut().expect("rename input is active"),
            key,
        );
        match edit {
            RenameEdit::Continue => {
                self.sync_rename_prompt();
            }
            RenameEdit::Confirm => {
                let name = self.rename_input.take().unwrap();
                let tab_id = self.windows[self.active].tab_id;
                rename_tab(&mut self.windows, tab_id, &name);
                self.finish_window_rename();
            }
            RenameEdit::Cancel => {
                self.rename_input = None;
                self.finish_window_rename();
            }
        }
        self.redraw()
    }

    fn sync_rename_prompt(&mut self) {
        self.renderer
            .set_rename_prompt(self.rename_input.as_deref());
    }

    fn finish_window_rename(&mut self) {
        self.rename_input = None;
        self.renderer.set_rename_prompt(None);
        self.mode = self.config.default_mode.clone();
        self.renderer.invalidate();
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
        self.rename_input = None;
        self.renderer.set_border_status(None);
        self.renderer.set_rename_prompt(None);
        for window in &mut self.windows {
            window.history_mode = false;
            window.terminal.screen_mut().set_scrollback(0);
        }
    }

    fn history_page_rows(&self) -> usize {
        usize::from(self.active_content_size().1.saturating_sub(1).max(1))
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
        let tab_id = self.next_id;
        let pane_id = match self.spawn_window(
            label.to_owned(),
            shell,
            arguments,
            SpawnOptions {
                temporary_file: Some(path.clone()),
                return_to_window: Some(return_to),
                floating: false,
                tab_id,
            },
        ) {
            Ok(id) => id,
            Err(error) => {
                let _ = fs::remove_file(path);
                return Err(error);
            }
        };
        self.tabs.push(Tab {
            id: tab_id,
            root: PaneNode::Leaf(pane_id),
        });
        self.resize_windows()?;
        self.redraw()
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
                let text = selected_text(
                    &self.windows[self.active],
                    selection,
                    self.active_content_size(),
                );
                if selection.spans_multiple_cells() && !text.is_empty() {
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
        let (origin_column, origin_row, columns, rows) = if self.windows[self.active].floating {
            let layout = floating_layout_for(self.terminal_size, self.config.compact());
            let (columns, rows) = layout.content_size();
            (layout.column + 1, layout.row + 1, columns, rows)
        } else {
            let window = &self.windows[self.active];
            let inset = u16::from(window.pane_framed);
            let (columns, rows) = pane_pty_size(window.pane_rect, window.pane_framed);
            let (base_column, base_row) = if window.pane_framed { (1, 2) } else { (2, 3) };
            (
                base_column + window.pane_rect.column + inset,
                base_row + window.pane_rect.row + inset,
                columns,
                rows,
            )
        };
        let column = i32::from(position.column) - i32::from(origin_column);
        let row = i32::from(position.row) - i32::from(origin_row);
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

    fn active_content_size(&self) -> (u16, u16) {
        if self.windows[self.active].floating {
            floating_layout_for(self.terminal_size, self.config.compact()).content_size()
        } else {
            let window = &self.windows[self.active];
            pane_pty_size(window.pane_rect, window.pane_framed)
        }
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
        let (screen_rows, screen_columns) = screen.size();
        let cell_width = self.terminal_pixels.0 / self.terminal_size.0.max(1);
        let cell_height = self.terminal_pixels.1 / self.terminal_size.1.max(1);
        graphics_responses.extend_from_slice(&terminal_responses(
            &parsed.terminal,
            Winsize {
                ws_col: screen_columns,
                ws_row: screen_rows,
                ws_xpixel: screen_columns.saturating_mul(cell_width),
                ws_ypixel: screen_rows.saturating_mul(cell_height),
            },
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
        let base_tab = self.windows[render_base_index(&self.windows, self.active)].tab_id;
        let visible = index == self.active
            || (!self.windows[index].floating && self.windows[index].tab_id == base_tab);
        if visible && (terminal_changed || graphics_changed) {
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

    fn new_pane(&mut self, axis: SplitAxis) -> Result<()> {
        if self.windows[self.active].floating {
            return self.notify("hide the floating terminal before splitting a pane");
        }
        let tab_id = self.windows[self.active].tab_id;
        let active_id = self.windows[self.active].id;
        let rect = self.windows[self.active].pane_rect;
        let enough_space = match axis {
            SplitAxis::Vertical => rect.width >= 12,
            SplitAxis::Horizontal => rect.height >= 6,
        };
        if !enough_space {
            return self.notify("not enough space to split this pane");
        }
        let shell_name = env::var("RUSTMUX_SHELL").unwrap_or_else(|_| "fish".to_owned());
        let name = Path::new(&shell_name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell_name)
            .to_owned();
        let shell = CString::new(shell_name)?;
        let new_id = self.spawn_window(
            name,
            shell.clone(),
            vec![shell],
            SpawnOptions {
                temporary_file: None,
                return_to_window: None,
                floating: false,
                tab_id,
            },
        )?;
        let tab = self.tabs.iter_mut().find(|tab| tab.id == tab_id).unwrap();
        if !split_pane(&mut tab.root, active_id, new_id, axis) {
            let index = self
                .windows
                .iter()
                .position(|window| window.id == new_id)
                .unwrap();
            terminate_window(self.windows.remove(index));
            return Err("active pane is missing from its tab layout".into());
        }
        self.active = self
            .windows
            .iter()
            .position(|window| window.id == new_id)
            .unwrap();
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn focus_next_pane(&mut self) -> Result<()> {
        if self.windows[self.active].floating {
            return Ok(());
        }
        let tab_id = self.windows[self.active].tab_id;
        let ids = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| pane_ids(&tab.root))
            .unwrap_or_default();
        if ids.len() > 1 {
            let current = ids
                .iter()
                .position(|id| *id == self.windows[self.active].id)
                .unwrap_or(0);
            let target = ids[(current + 1) % ids.len()];
            self.active = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            self.selection = None;
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn focus_pane(&mut self, direction: Direction) -> Result<()> {
        if self.windows[self.active].floating {
            return Ok(());
        }
        let tab_id = self.windows[self.active].tab_id;
        let active_id = self.windows[self.active].id;
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return Ok(());
        };
        let rects = pane_rects(
            &tab.root,
            content_rect_for(self.terminal_size, self.config.compact()),
        );
        let Some((_, current)) = rects.iter().find(|(id, _)| *id == active_id) else {
            return Ok(());
        };
        if let Some((target, _)) = rects
            .iter()
            .filter(|(id, rect)| *id != active_id && rect_in_direction(*current, *rect, direction))
            .min_by_key(|(_, rect)| directional_distance(*current, *rect, direction))
        {
            self.active = self
                .windows
                .iter()
                .position(|window| window.id == *target)
                .unwrap();
            self.selection = None;
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn close_pane(&mut self) -> Result<()> {
        if self.windows[self.active].floating {
            return self.close_active();
        }
        let tab_id = self.windows[self.active].tab_id;
        let pane_id = self.windows[self.active].id;
        let pane_count = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| pane_ids(&tab.root).len())
            .unwrap_or(1);
        if pane_count == 1 {
            return self.close_active();
        }
        let window = self.windows.remove(self.active);
        terminate_window(window);
        let tab = self.tabs.iter_mut().find(|tab| tab.id == tab_id).unwrap();
        tab.root = remove_pane(tab.root.clone(), pane_id).expect("another pane remains");
        let target = pane_ids(&tab.root)[0];
        self.active = self
            .windows
            .iter()
            .position(|window| window.id == target)
            .unwrap();
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn resize_windows(&mut self) -> Result<()> {
        let mut sizes = Vec::new();
        for tab in &self.tabs {
            let pane_count = pane_ids(&tab.root).len();
            let framed = pane_count > 1;
            let content = if framed {
                tiled_content_rect_for(self.terminal_size, self.config.compact())
            } else {
                content_rect_for(self.terminal_size, self.config.compact())
            };
            let rects = pane_rects(&tab.root, content);
            for (id, rect) in rects {
                sizes.push((id, rect, framed, pane_pty_size(rect, framed)));
            }
        }
        for index in 0..self.windows.len() {
            let (columns, rows) = if self.windows[index].floating {
                floating_layout_for(self.terminal_size, self.config.compact()).content_size()
            } else {
                let (_, rect, framed, size) = sizes
                    .iter()
                    .find(|(id, _, _, _)| *id == self.windows[index].id)
                    .copied()
                    .unwrap_or((0, PaneRect::default(), false, (1, 1)));
                self.windows[index].pane_rect = rect;
                self.windows[index].pane_framed = framed;
                size
            };
            resize_window(
                &mut self.windows[index],
                columns,
                rows,
                self.terminal_size,
                self.terminal_pixels,
            )?;
        }
        Ok(())
    }

    fn select_relative(&mut self, offset: isize) -> Result<()> {
        if self.tabs.len() > 1 {
            let tab_id = self.windows[render_base_index(&self.windows, self.active)].tab_id;
            let current = self
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .unwrap_or(0);
            let next = (current as isize + offset).rem_euclid(self.tabs.len() as isize) as usize;
            let target = pane_ids(&self.tabs[next].root)[0];
            self.active = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            self.selection = None;
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn select_window(&mut self, index: usize) -> Result<()> {
        if let Some(tab) = self.tabs.get(index.saturating_sub(1)) {
            let target = pane_ids(&tab.root)[0];
            self.active = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            self.selection = None;
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn close_active(&mut self) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        if self.windows[self.active].floating {
            let window = self.windows.remove(self.active);
            let return_to = window.return_to_window;
            terminate_window(window);
            self.active = return_to
                .and_then(|id| self.windows.iter().position(|window| window.id == id))
                .unwrap_or(0);
            self.renderer.invalidate();
            return self.redraw();
        }
        let tab_id = self.windows[self.active].tab_id;
        let tab_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .unwrap_or(0);
        let return_to = self
            .windows
            .iter()
            .find(|window| window.tab_id == tab_id && window.return_to_window.is_some())
            .and_then(|window| window.return_to_window);
        for index in (0..self.windows.len()).rev() {
            if self.windows[index].tab_id == tab_id {
                terminate_window(self.windows.remove(index));
            }
        }
        self.tabs.retain(|tab| tab.id != tab_id);
        if self.tabs.is_empty() {
            for window in self.windows.drain(..) {
                terminate_window(window);
            }
            return Ok(());
        }
        let target = return_to
            .filter(|id| self.windows.iter().any(|window| window.id == *id))
            .unwrap_or_else(|| pane_ids(&self.tabs[tab_index.min(self.tabs.len() - 1)].root)[0]);
        self.active = self
            .windows
            .iter()
            .position(|window| window.id == target)
            .unwrap();
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn redraw(&mut self) -> Result<()> {
        self.redraw_deadline = None;
        if self.windows.is_empty() || self.client.is_none() {
            return Ok(());
        }
        self.renderer
            .set_ui(self.config.compact(), self.config.describe_mode(&self.mode));
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
        validate_terminal_size(new_size)?;
        if new_size == self.terminal_size && new_pixels == self.terminal_pixels {
            return Ok(());
        }
        self.terminal_size = new_size;
        self.terminal_pixels = new_pixels;
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()?;
        Ok(())
    }

    fn reap_children(&mut self) -> Result<()> {
        let mut removed_any = false;
        let active_id = self.windows.get(self.active).map(|window| window.id);
        let mut preferred = None;
        let mut preferred_tab = None;
        let mut index = 0;
        while index < self.windows.len() {
            let pid = self.windows[index].child;
            match waitpid(pid, Some(WaitPidFlag::WNOHANG))? {
                WaitStatus::StillAlive => index += 1,
                _ => {
                    removed_any = true;
                    let window = self.windows.remove(index);
                    if Some(window.id) == active_id {
                        preferred = window.return_to_window;
                        preferred_tab = Some(window.tab_id);
                    }
                    if let Some(path) = window.temporary_file {
                        let _ = fs::remove_file(path);
                    }
                    if !window.floating
                        && let Some(tab_index) =
                            self.tabs.iter().position(|tab| tab.id == window.tab_id)
                    {
                        let root = remove_pane(self.tabs[tab_index].root.clone(), window.id);
                        if let Some(root) = root {
                            self.tabs[tab_index].root = root;
                        } else {
                            self.tabs.remove(tab_index);
                        }
                    }
                }
            }
        }
        if self.tabs.is_empty() {
            for window in self.windows.drain(..) {
                terminate_window(window);
            }
            return Ok(());
        }
        if !self.windows.is_empty() {
            let target = active_id
                .filter(|id| self.windows.iter().any(|window| window.id == *id))
                .or_else(|| {
                    preferred.filter(|id| self.windows.iter().any(|window| window.id == *id))
                })
                .or_else(|| {
                    preferred_tab.and_then(|tab_id| {
                        self.tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .map(|tab| pane_ids(&tab.root)[0])
                    })
                })
                .unwrap_or_else(|| pane_ids(&self.tabs[0].root)[0]);
            self.active = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            if removed_any {
                self.selection = None;
                self.resize_windows()?;
                self.renderer.invalidate();
                self.redraw()?;
            }
        }
        Ok(())
    }

    pub(super) fn shutdown(&mut self) {
        for window in self.windows.drain(..) {
            terminate_window(window);
        }
    }
}

pub(super) fn window_history(window: &mut Window) -> String {
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

pub(super) fn selected_text(
    window: &Window,
    selection: TextSelection,
    content_size: (u16, u16),
) -> String {
    if selection.window_id != window.id {
        return String::new();
    }
    let (columns, _) = content_size;
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

pub(super) fn selection_contains(
    selection: Option<&TextSelection>,
    window_id: usize,
    row: u16,
    column: u16,
    columns: u16,
) -> bool {
    let Some(selection) = selection.filter(|selection| selection.window_id == window_id) else {
        return false;
    };
    if !selection.spans_multiple_cells() {
        return false;
    }
    let position = usize::from(row) * usize::from(columns) + usize::from(column);
    let start = usize::from(selection.start.row) * usize::from(columns)
        + usize::from(selection.start.column);
    let end =
        usize::from(selection.end.row) * usize::from(columns) + usize::from(selection.end.column);
    (start.min(end)..=start.max(end)).contains(&position)
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

fn resize_window(
    window: &mut Window,
    columns: u16,
    rows: u16,
    terminal_size: (u16, u16),
    terminal_pixels: (u16, u16),
) -> Result<()> {
    let cell_width = terminal_pixels.0 / terminal_size.0.max(1);
    let cell_height = terminal_pixels.1 / terminal_size.1.max(1);
    let winsize = Winsize {
        ws_col: columns,
        ws_row: rows,
        ws_xpixel: columns.saturating_mul(cell_width),
        ws_ypixel: rows.saturating_mul(cell_height),
    };
    // SAFETY: master is an open PTY descriptor and winsize is valid.
    if unsafe { nix::libc::ioctl(window.master.as_raw_fd(), nix::libc::TIOCSWINSZ, &winsize) } == -1
    {
        return Err(io::Error::last_os_error().into());
    }
    window.terminal.screen_mut().set_size(rows, columns);
    Ok(())
}
