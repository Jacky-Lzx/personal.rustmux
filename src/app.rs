use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, poll};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, pipe, read, tcgetpgrp, write};

use crate::config::{Action, Config, ConfigReloader, config_path};
use crate::input::{
    DecodedKey, InputDecoder, MouseAction, MousePosition, decode_focus_event, decode_key,
    decode_sgr_mouse, sgr_mouse_at,
};
use crate::layout::{
    Direction, PaneNode, PaneRect, PaneResizeHandle, SplitAxis, content_rect_for,
    directional_distance, floating_layout_for, move_item, pane_ids, pane_pty_size, pane_rects,
    pane_resize_handle, rect_in_direction, remove_pane, resize_pane, resize_pane_to, split_pane,
    swap_panes, tiled_content_rect_for, validate_terminal_size, window_winsize_for,
};
use crate::persistence::{
    HistoryCache, PreparedSnapshot, SaveCompletion, SaveRequest, SnapshotWriter,
};
use crate::render::{HelpView, Renderer, SessionManagerView, render_base_index};
use crate::session::{
    ScrollbackFormat, SessionInfo, SessionSnapshot, SnapshotFloating, SnapshotPane, SnapshotTab,
    available_session_info, delete_session, delete_session_snapshot, disconnect_session,
    ensure_session_dir, load_session_snapshot, rename_session, rename_session_snapshot,
    validate_session_name,
};
use crate::terminal::{
    CursorStyleTracker, HyperlinkTracker, InputModeTracker, KittyDndEvent, KittyDndParser,
    KittyDndRegistration, KittyGraphicsParser, KittyIpcParser, SemanticOutputCapture,
    TerminalMetadata, TerminalOscTracker, base64_encode, format_duration, kitty_dnd_command,
    kitty_dnd_data_response, kitty_dnd_drag_start_position, kitty_dnd_for_child, kitty_dnd_id,
    kitty_dnd_registration, kitty_dnd_with_id, kitty_graphics_query_response,
    kitty_graphics_uses_shared_memory, kitty_ipc_for_child, kitty_ipc_is_clipboard,
    kitty_ipc_with_pane, kitty_notification, outer_terminal_identity, terminal_parser_size,
    terminal_responses,
};
use crate::{
    CLIENT_DISCONNECT, CLIENT_INPUT, CLIENT_QUERY_STATUS, CLIENT_RENAME_SESSION, CLIENT_RESIZE,
    CLIENT_SHUTDOWN, CLIPBOARD_STATUS, CLIPBOARD_STATUS_DURATION, FRAME_INTERVAL,
    MAX_CLIENT_MESSAGE_BYTES, MAX_PTY_READS_PER_TICK, MOUSE_SCROLL_LINES, NOTIFICATION_DURATION,
    PREFIX, Result, SERVER_SWITCH_SESSION_PREFIX,
};

pub(super) struct Window {
    pub(super) id: usize,
    pub(super) tab_id: usize,
    pub(super) name: String,
    pub(super) floating: bool,
    pub(super) pane_rect: PaneRect,
    pub(super) pane_framed: bool,
    pub(super) zoomed: bool,
    pub(super) master: OwnedFd,
    pub(super) child: Pid,
    pub(super) spawn_directory: Option<PathBuf>,
    pub(super) startup_command: Option<String>,
    pub(super) terminal: vt100::Parser<TerminalMetadata>,
    pub(super) history_cache: HistoryCache,
    pub(super) cursor_style: CursorStyleTracker,
    pub(super) input_modes: InputModeTracker,
    pub(super) terminal_osc: TerminalOscTracker,
    pub(super) kitty_graphics: KittyGraphicsParser,
    pub(super) graphics_cache: crate::graphics::GraphicsCache,
    pub(super) graphics_replay_pending: bool,
    pub(super) kitty_dnd: KittyDndParser,
    pub(super) kitty_ipc: KittyIpcParser,
    pub(super) hyperlinks: HyperlinkTracker,
    pub(super) dnd_drag_registration: Option<Vec<u8>>,
    pub(super) dnd_drop_registration: Option<Vec<u8>>,
    pub(super) pending_graphics: Vec<Vec<u8>>,
    pub(super) history_mode: bool,
    pub(super) bell_pending: bool,
    pub(super) command_output: SemanticOutputCapture,
    pub(super) notification_applications: Vec<String>,
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
    current_directory: Option<PathBuf>,
}

const MAX_INTERNAL_DND_DATA_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct InternalDndBridge {
    source_window_id: Option<usize>,
    source_mimes: Vec<String>,
    source_data: HashMap<usize, Vec<u8>>,
    source_data_bytes: usize,
    current_pre_sent_index: Option<usize>,
    target_window_id: Option<usize>,
    target_mimes: Vec<String>,
}

impl InternalDndBridge {
    pub(super) fn begin_source(&mut self, window_id: usize, payload: &[u8]) {
        *self = Self::default();
        self.source_window_id = Some(window_id);
        self.source_mimes = mime_types(payload);
    }

    pub(super) fn cache_pre_sent_data(
        &mut self,
        window_id: usize,
        index: Option<i32>,
        more: bool,
        payload: &[u8],
    ) {
        if self.source_window_id != Some(window_id) {
            return;
        }
        let index = index
            .and_then(|index| usize::try_from(index).ok())
            .or(self.current_pre_sent_index);
        let Some(index) = index else {
            return;
        };
        if self.source_data_bytes.saturating_add(payload.len()) > MAX_INTERNAL_DND_DATA_BYTES {
            self.clear();
            return;
        }
        self.source_data
            .entry(index)
            .or_default()
            .extend_from_slice(payload);
        self.source_data_bytes += payload.len();
        self.current_pre_sent_index = more.then_some(index);
    }

    pub(super) fn begin_target(&mut self, window_id: usize, payload: &[u8]) {
        if self
            .source_window_id
            .is_some_and(|source| source != window_id)
        {
            self.target_window_id = Some(window_id);
            self.target_mimes = mime_types(payload);
        } else {
            self.target_window_id = None;
            self.target_mimes.clear();
        }
    }

    pub(super) fn data_response(&self, window_id: usize, request_index: i32) -> Option<Vec<u8>> {
        (self.target_window_id == Some(window_id)).then_some(())?;
        let target_index = usize::try_from(request_index.checked_sub(1)?).ok()?;
        let mime = self.target_mimes.get(target_index)?;
        let source_index = self.source_mimes.iter().position(|source| source == mime)?;
        let data = self.source_data.get(&source_index)?;
        kitty_dnd_data_response(request_index, data)
    }

    pub(super) fn routes_response_to_source(&self, window_id: usize, kind: Option<char>) -> bool {
        self.target_window_id == Some(window_id) && matches!(kind, Some('m' | 'r'))
    }

    fn source_window_id(&self) -> Option<usize> {
        self.source_window_id
    }

    fn accepts_pre_sent_continuation(&self, window_id: usize) -> bool {
        self.source_window_id == Some(window_id) && self.current_pre_sent_index.is_some()
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

fn mime_types(payload: &[u8]) -> Vec<String> {
    std::str::from_utf8(payload)
        .ok()
        .into_iter()
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
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
    pub(super) keyboard: bool,
}

impl TextSelection {
    pub(super) fn spans_multiple_cells(self) -> bool {
        self.start != self.end
    }

    pub(super) fn is_visible(self) -> bool {
        self.keyboard || self.spans_multiple_cells()
    }
}

struct HistorySearchState {
    query: String,
    matches: Vec<usize>,
    selected: usize,
    editing: bool,
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
            if let Some(text) = key.text()
                && name.chars().count() + text.chars().count() <= 64
            {
                name.push_str(text);
            }
            RenameEdit::Continue
        }
    }
}

pub(super) fn rename_tab(windows: &mut [Window], tab_id: usize, name: &str) {
    for window in windows
        .iter_mut()
        .filter(|window| !window.floating && window.tab_id == tab_id)
    {
        window.name = name.to_owned();
    }
}

fn remap_pane_node(node: &PaneNode, ids: &HashMap<usize, usize>) -> Result<PaneNode> {
    Ok(match node {
        PaneNode::Leaf(id) => PaneNode::Leaf(
            ids.get(id)
                .copied()
                .ok_or("saved pane layout references a missing pane")?,
        ),
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => PaneNode::Split {
            axis: *axis,
            ratio: *ratio,
            first: Box::new(remap_pane_node(first, ids)?),
            second: Box::new(remap_pane_node(second, ids)?),
        },
    })
}

struct RenameState {
    tab_id: usize,
    original_names: Vec<(usize, String)>,
    input: String,
}

struct SessionManagerState {
    searching: bool,
    query: String,
    sessions: Vec<SessionInfo>,
    selected: usize,
    rename_input: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MouseDrag {
    WindowBar,
    PaneResize {
        tab_id: usize,
        handle: PaneResizeHandle,
    },
}

pub(super) fn default_session_selection(sessions: &[SessionInfo], current: &str) -> usize {
    sessions
        .iter()
        .position(|session| !session.connected && session.name != current)
        .unwrap_or(0)
}

pub(super) fn matching_session_info(sessions: &[SessionInfo], query: &str) -> Vec<SessionInfo> {
    let query = query.to_ascii_lowercase();
    sessions
        .iter()
        .filter(|session| session.name.to_ascii_lowercase().contains(&query))
        .cloned()
        .collect()
}

pub(super) struct App {
    windows: Vec<Window>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: usize,
    next_notification_id: u64,
    input_decoder: InputDecoder,
    dnd_input: KittyDndParser,
    internal_dnd: InternalDndBridge,
    ipc_input: KittyIpcParser,
    config: Config,
    config_reloader: ConfigReloader,
    mode: String,
    selection: Option<TextSelection>,
    renderer: Renderer,
    session_name: String,
    socket_path: PathBuf,
    created_at: u64,
    last_connected_at: u64,
    terminal_size: (u16, u16),
    terminal_pixels: (u16, u16),
    terminal_identity: String,
    redraw_deadline: Option<Instant>,
    clipboard_status_until: Option<Instant>,
    notification_until: Option<Instant>,
    rename_state: Option<RenameState>,
    history_search: Option<HistorySearchState>,
    session_manager: Option<SessionManagerState>,
    help_mode: Option<String>,
    mouse_drag: Option<MouseDrag>,
    status_mouse_down: bool,
    last_autosave_check: Instant,
    last_saved_snapshot: Option<Arc<PreparedSnapshot>>,
    snapshot_writer: SnapshotWriter,
    client: Option<UnixStream>,
    outer_dnd_window: Option<usize>,
    outer_keyboard_flags: Option<u8>,
    outer_pointer_shape: Option<String>,
    outer_rich_paste: Option<bool>,
    client_input: Vec<u8>,
}

impl App {
    pub(super) fn new(
        terminal_size: (u16, u16),
        terminal_pixels: (u16, u16),
        config: Config,
        session_name: &str,
        socket_path: PathBuf,
        layout: Option<SessionSnapshot>,
    ) -> Result<Self> {
        let mode = config.default_mode.clone();
        let config_reloader = ConfigReloader::new(config_path());
        let mut renderer = Renderer::default();
        renderer.set_session_name(session_name);
        let mut app = Self {
            windows: Vec::new(),
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            next_notification_id: 1,
            input_decoder: InputDecoder::default(),
            dnd_input: KittyDndParser::default(),
            internal_dnd: InternalDndBridge::default(),
            ipc_input: KittyIpcParser::default(),
            config,
            config_reloader,
            mode,
            selection: None,
            renderer,
            session_name: session_name.to_owned(),
            socket_path,
            last_connected_at: crate::session::last_connected_at(session_name),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            terminal_size,
            terminal_pixels,
            terminal_identity: outer_terminal_identity(),
            redraw_deadline: None,
            clipboard_status_until: None,
            notification_until: None,
            rename_state: None,
            history_search: None,
            session_manager: None,
            help_mode: None,
            mouse_drag: None,
            status_mouse_down: false,
            last_autosave_check: Instant::now(),
            last_saved_snapshot: None,
            snapshot_writer: SnapshotWriter::default(),
            client: None,
            outer_dnd_window: None,
            outer_keyboard_flags: None,
            outer_pointer_shape: None,
            outer_rich_paste: None,
            client_input: Vec::new(),
        };
        let snapshot = match layout {
            Some(snapshot) => Some(snapshot),
            None => load_session_snapshot(session_name)?,
        };
        if let Some(snapshot) = snapshot {
            if let Err(error) = app.restore_session(snapshot) {
                for window in app.windows.drain(..) {
                    terminate_window(window);
                }
                return Err(error);
            }
        } else {
            app.create_window()?;
        }
        Ok(app)
    }

    fn restore_session(&mut self, snapshot: SessionSnapshot) -> Result<()> {
        let shell_name = crate::shell::resolve_shell(self.config.shell.as_deref())?;
        let shell_label = Path::new(&shell_name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell_name)
            .to_owned();
        let shell = CString::new(shell_name)?;
        let mut pane_ids_by_saved_id = HashMap::new();
        let mut saved_histories = Vec::new();
        for saved_tab in snapshot.tabs {
            let tab_id = self.next_id;
            for pane in &saved_tab.panes {
                let arguments = match &pane.command {
                    Some(command) => vec![
                        shell.clone(),
                        CString::new("-c")?,
                        CString::new(command.as_str())?,
                    ],
                    None => vec![shell.clone()],
                };
                let pane_id = self.spawn_window(
                    saved_tab.name.clone(),
                    shell.clone(),
                    arguments,
                    SpawnOptions {
                        temporary_file: None,
                        return_to_window: None,
                        floating: false,
                        tab_id,
                        current_directory: pane.cwd.clone(),
                    },
                )?;
                self.windows
                    .last_mut()
                    .expect("spawned pane")
                    .startup_command = pane.command.clone();
                pane_ids_by_saved_id.insert(pane.id, pane_id);
                if self.config.save_scrollback
                    && let Some(lines) = &pane.scrollback
                {
                    saved_histories.push((pane_id, lines.clone(), pane.scrollback_format));
                }
            }
            self.tabs.push(Tab {
                id: tab_id,
                root: remap_pane_node(&saved_tab.root, &pane_ids_by_saved_id)?,
            });
        }
        let active_id = pane_ids_by_saved_id
            .get(&snapshot.active_pane)
            .copied()
            .ok_or("saved active pane could not be restored")?;
        self.active = self
            .windows
            .iter()
            .position(|window| window.id == active_id)
            .ok_or("restored active pane is missing")?;

        if let Some(floating) = snapshot.floating {
            let scrollback = floating.scrollback;
            let return_to = floating
                .return_to
                .and_then(|id| pane_ids_by_saved_id.get(&id).copied())
                .unwrap_or(active_id);
            let tab_id = self
                .windows
                .iter()
                .find(|window| window.id == return_to)
                .map(|window| window.tab_id)
                .unwrap_or(self.tabs[0].id);
            let floating_id = self.spawn_window(
                shell_label,
                shell.clone(),
                vec![shell],
                SpawnOptions {
                    temporary_file: None,
                    return_to_window: Some(return_to),
                    floating: true,
                    tab_id,
                    current_directory: floating.cwd,
                },
            )?;
            if self.config.save_scrollback
                && let Some(lines) = scrollback
            {
                saved_histories.push((floating_id, lines, floating.scrollback_format));
            }
            if !floating.visible {
                self.active = self
                    .windows
                    .iter()
                    .position(|window| window.id == active_id)
                    .unwrap_or(0);
            }
        }
        self.resize_windows()?;
        for (id, lines, format) in saved_histories {
            let window = self
                .windows
                .iter_mut()
                .find(|window| window.id == id)
                .unwrap();
            restore_scrollback(
                &mut window.terminal,
                &lines,
                self.config.scrollback_lines(),
                format,
            );
        }
        Ok(())
    }

    fn saved_scrollback_format(&self) -> ScrollbackFormat {
        if self.config.save_scrollback && self.config.save_scrollback_colors {
            ScrollbackFormat::Ansi
        } else {
            ScrollbackFormat::Plain
        }
    }

    fn session_snapshot(&mut self) -> Result<PreparedSnapshot> {
        let active_pane = if self.windows[self.active].floating {
            self.windows[self.active]
                .return_to_window
                .ok_or("floating terminal has no return pane")?
        } else {
            self.windows[self.active].id
        };
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| {
                !self
                    .windows
                    .iter()
                    .any(|window| window.tab_id == tab.id && window.temporary_file.is_some())
            })
            .map(|tab| {
                let panes = pane_ids(&tab.root)
                    .into_iter()
                    .filter_map(|id| self.windows.iter().find(|window| window.id == id))
                    .map(|window| SnapshotPane {
                        scrollback_format: self.saved_scrollback_format(),
                        scrollback: None,
                        command: window.startup_command.clone(),
                        id: window.id,
                        cwd: self.window_directory(window),
                    })
                    .collect::<Vec<_>>();
                let name = self
                    .windows
                    .iter()
                    .find(|window| !window.floating && window.tab_id == tab.id)
                    .map(|window| window.name.clone())
                    .unwrap_or_else(|| "shell".to_owned());
                SnapshotTab {
                    id: tab.id,
                    name,
                    root: tab.root.clone(),
                    panes,
                }
            })
            .collect();
        let saved_ids: Vec<_> = tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(|pane| pane.id))
            .collect();
        let active_pane = if saved_ids.contains(&active_pane) {
            active_pane
        } else {
            *saved_ids.first().ok_or("no persistent panes to save")?
        };
        let floating = self
            .windows
            .iter()
            .find(|window| window.floating)
            .map(|window| SnapshotFloating {
                scrollback_format: self.saved_scrollback_format(),
                scrollback: None,
                cwd: self.window_directory(window),
                visible: self.windows[self.active].id == window.id,
                return_to: window
                    .return_to_window
                    .filter(|id| saved_ids.contains(id))
                    .or(Some(active_pane)),
            });
        let mut snapshot = PreparedSnapshot::new(SessionSnapshot::new(tabs, floating, active_pane));
        if self.config.save_scrollback {
            for window in &mut self.windows {
                if window.floating || saved_ids.contains(&window.id) {
                    let history = window.history_cache.capture(
                        window.terminal.screen(),
                        self.config.scrollback_lines(),
                        self.config.save_scrollback_colors,
                    );
                    snapshot.add_history((!window.floating).then_some(window.id), history);
                }
            }
        }
        Ok(snapshot)
    }

    fn queue_session_save(&mut self, notify: bool, reply: Option<UnixStream>) -> Result<()> {
        let snapshot = Arc::new(self.session_snapshot()?);
        self.snapshot_writer.submit(SaveRequest {
            name: self.session_name.clone(),
            snapshot,
            notify,
            replies: reply.into_iter().collect(),
        })?;
        Ok(())
    }

    fn save_current_session(&mut self) -> Result<()> {
        self.queue_session_save(true, None)?;
        self.notify("saving session…")
    }

    fn complete_save(&mut self, completion: SaveCompletion) {
        match completion.result {
            Ok(()) => {
                self.last_saved_snapshot = Some(completion.snapshot);
                if self.session_manager.is_some() {
                    let _ = self.refresh_session_manager();
                }
                if completion.notify {
                    let _ = self.notify("session saved");
                }
            }
            Err(error) => {
                eprintln!("session save failed: {error}");
                let _ = self.notify(&format!("session save failed: {error}"));
            }
        }
    }

    fn poll_session_saves(&mut self) {
        while let Some(completion) = self.snapshot_writer.poll() {
            self.complete_save(completion);
        }
    }

    fn flush_session_saves(&mut self) {
        for completion in self.snapshot_writer.flush() {
            self.complete_save(completion);
        }
    }

    fn autosave_session(&mut self, force: bool) {
        let seconds = self.config.autosave_interval_seconds;
        if seconds == 0
            || self.windows.is_empty()
            || self.tabs.is_empty()
            || (!force && self.last_autosave_check.elapsed() < Duration::from_secs(seconds))
        {
            return;
        }
        self.last_autosave_check = Instant::now();
        let result = (|| -> Result<()> {
            let snapshot = self.session_snapshot()?;
            let latest = self
                .snapshot_writer
                .latest_snapshot()
                .or(self.last_saved_snapshot.as_deref());
            if latest != Some(&snapshot) {
                self.snapshot_writer.submit(SaveRequest {
                    name: self.session_name.clone(),
                    snapshot: Arc::new(snapshot),
                    notify: false,
                    replies: Vec::new(),
                })?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = self.notify(&format!("session autosave failed: {error}"));
        }
    }

    fn create_window(&mut self) -> Result<()> {
        let current_directory = self.active_spawn_directory();
        let shell = crate::shell::resolve_shell(self.config.shell.as_deref())?;
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
                current_directory,
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
            let target = return_to
                .and_then(|id| self.windows.iter().position(|window| window.id == id))
                .or_else(|| self.windows.iter().position(|window| !window.floating))
                .unwrap_or(self.active);
            self.set_active(target);
            self.selection = None;
            return self.redraw();
        }

        let return_to = self.windows[self.active].id;
        if let Some(index) = self.windows.iter().position(|window| window.floating) {
            self.windows[index].return_to_window = Some(return_to);
            self.windows[index].tab_id = self.windows[self.active].tab_id;
            self.set_active(index);
            self.selection = None;
            return self.redraw();
        }

        let shell = crate::shell::resolve_shell(self.config.shell.as_deref())?;
        let name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell)
            .to_owned();
        let shell = CString::new(shell)?;
        let tab_id = self.windows[self.active].tab_id;
        let current_directory = self.active_spawn_directory();
        self.spawn_window(
            name,
            shell.clone(),
            vec![shell],
            SpawnOptions {
                temporary_file: None,
                return_to_window: Some(return_to),
                floating: true,
                tab_id,
                current_directory,
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
        let mut winsize = window_winsize_for(
            self.terminal_size,
            self.terminal_pixels,
            options.floating,
            self.config.compact(),
        );
        (winsize.ws_col, winsize.ws_row) = terminal_parser_size(winsize.ws_col, winsize.ws_row);
        let (columns, rows) = (winsize.ws_col, winsize.ws_row);
        let spawn_directory = options
            .current_directory
            .clone()
            .filter(|path| path.is_dir())
            .or_else(|| std::env::current_dir().ok());
        let current_directory = spawn_directory
            .as_ref()
            .map(|path| CString::new(path.as_os_str().as_encoded_bytes()))
            .transpose()?;

        let (exec_status_read, exec_status_write) = pipe()?;
        for fd in [&exec_status_read, &exec_status_write] {
            fcntl(fd, FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC))?;
        }
        // Prepare argv before fork: the save worker can be allocating memory.
        // nix::execvp builds this pointer vector internally, which is unsafe in
        // the child of a multithreaded process before exec.
        let argv: Vec<_> = arguments
            .iter()
            .map(|argument| argument.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        // SAFETY: the child immediately calls execvp and _exit, both of which are
        // async-signal-safe; all application bookkeeping remains in the parent.
        match unsafe { forkpty(&winsize, None) }? {
            ForkptyResult::Parent { child, master } => {
                drop(exec_status_write);
                let mut status = Vec::new();
                fs::File::from(exec_status_read).read_to_end(&mut status)?;
                if !status.is_empty() {
                    let _ = waitpid(child, None);
                    let code = i32::from_ne_bytes(
                        status
                            .try_into()
                            .map_err(|_| "invalid child startup response")?,
                    );
                    return Err(format!(
                        "could not start {}: {}",
                        program.to_string_lossy(),
                        io::Error::from_raw_os_error(code)
                    )
                    .into());
                }
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
                if !self.windows.is_empty() {
                    self.send_focus_event(self.active, false);
                }
                self.windows.push(Window {
                    id,
                    tab_id: options.tab_id,
                    name,
                    floating: options.floating,
                    pane_rect: content_rect_for(self.terminal_size, self.config.compact()),
                    pane_framed: false,
                    zoomed: false,
                    master,
                    child,
                    spawn_directory,
                    startup_command: None,
                    terminal: vt100::Parser::new_with_callbacks(
                        rows,
                        columns,
                        self.config.scrollback_lines(),
                        TerminalMetadata::default(),
                    ),
                    history_cache: HistoryCache::default(),
                    cursor_style: CursorStyleTracker::default(),
                    input_modes: InputModeTracker::default(),
                    terminal_osc: TerminalOscTracker::default(),
                    kitty_graphics: KittyGraphicsParser::default(),
                    graphics_cache: crate::graphics::GraphicsCache::default(),
                    graphics_replay_pending: false,
                    kitty_dnd: KittyDndParser::default(),
                    kitty_ipc: KittyIpcParser::default(),
                    hyperlinks: HyperlinkTracker::default(),
                    dnd_drag_registration: None,
                    dnd_drop_registration: None,
                    pending_graphics: Vec::new(),
                    history_mode: false,
                    bell_pending: false,
                    command_output: SemanticOutputCapture::default(),
                    notification_applications: Vec::new(),
                    temporary_file: options.temporary_file,
                    return_to_window: options.return_to_window,
                });
                self.active = self.windows.len() - 1;
                self.sync_keyboard_protocol();
                self.sync_rich_paste_protocol();
                self.send_focus_event(self.active, true);
                self.renderer.invalidate();
                Ok(id)
            }
            ForkptyResult::Child => {
                if let Some(directory) = current_directory
                    && unsafe { nix::libc::chdir(directory.as_ptr()) } == -1
                {
                    let code = Errno::last() as i32;
                    let _ = write(&exec_status_write, &code.to_ne_bytes());
                    // SAFETY: exiting directly is required after fork if setup fails.
                    unsafe { nix::libc::_exit(127) };
                }
                // SAFETY: program and argv are NUL-terminated, prepared before
                // fork, and remain alive until exec replaces the child.
                unsafe { nix::libc::execvp(program.as_ptr(), argv.as_ptr()) };
                let code = Errno::last() as i32;
                let _ = write(&exec_status_write, &code.to_ne_bytes());
                // SAFETY: exiting directly is required after fork if exec fails.
                unsafe { nix::libc::_exit(127) };
            }
        }
    }

    fn active_spawn_directory(&self) -> Option<PathBuf> {
        self.windows
            .get(self.active)
            .and_then(|window| self.window_directory(window))
    }

    fn window_directory(&self, window: &Window) -> Option<PathBuf> {
        let tracked = window
            .terminal
            .callbacks()
            .current_directory
            .clone()
            .filter(|path| path.is_dir());
        let foreground = tcgetpgrp(&window.master).ok();
        let application = foreground.and_then(process_name);
        let process_directory = if tracked.is_none()
            || application
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case("yazi"))
        {
            foreground
                .and_then(process_current_directory)
                .filter(|path| path.is_dir())
                .or_else(|| process_current_directory(window.child).filter(|path| path.is_dir()))
        } else {
            None
        };
        preferred_spawn_directory(tracked, application.as_deref(), process_directory)
            .or_else(|| window.spawn_directory.clone().filter(|path| path.is_dir()))
    }

    pub(super) fn run_server(&mut self, listener: UnixListener) -> Result<()> {
        listener.set_nonblocking(true)?;
        let mut output = [0_u8; 64 * 1024];

        while !self.windows.is_empty() {
            self.poll_session_saves();
            self.reload_config_if_changed()?;
            self.autosave_session(false);
            self.track_foreground_applications();
            self.flush_scheduled_redraw()?;
            let mut expired_input = Vec::new();
            let expired_ipc = self.ipc_input.flush_if_expired();
            if !expired_ipc.is_empty() {
                if !self.handle_dnd_input(&expired_ipc)? {
                    self.detach_client();
                }
                let terminal = self.dnd_input.flush();
                expired_input.extend(self.input_decoder.push(&terminal));
                // IPC already waited for the shared escape-sequence timeout;
                // release ambiguity in the downstream parsers immediately.
                expired_input.extend(self.input_decoder.flush());
            }
            let expired_dnd = self.dnd_input.flush_if_expired();
            if !expired_dnd.is_empty() {
                expired_input.extend(self.input_decoder.push(&expired_dnd));
                // The DnD parser already waited long enough to disambiguate its
                // partial OSC prefix, so do not impose a second Esc timeout.
                expired_input.extend(self.input_decoder.flush());
            }
            expired_input.extend(self.input_decoder.flush_if_expired());
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
                    Ok((stream, _)) => match self.accept_connection(stream) {
                        Ok(true) => {}
                        Ok(false) => break,
                        // A client that disconnects before its first message or
                        // sends a malformed control request must not stop the
                        // entire session server.
                        Err(_) => {}
                    },
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            self.flush_scheduled_redraw()?;
        }
        Ok(())
    }

    fn reload_config_if_changed(&mut self) -> Result<()> {
        let Some(result) = self.config_reloader.poll(Instant::now()) else {
            return Ok(());
        };
        let config = match result {
            Ok(config) => config,
            Err(error) => {
                let _ = self.notify(&format!("config reload failed: {error}"));
                return Ok(());
            }
        };
        let compact_changed = self.config.compact() != config.compact();
        if !config.has_mode(&self.mode) {
            if self.mode == "scroll" && !self.windows.is_empty() {
                let window = &mut self.windows[self.active];
                window.history_mode = false;
                window.terminal.screen_mut().set_scrollback(0);
            }
            self.mode = config.default_mode.clone();
        }
        self.config = config;
        if !self.config.mouse_hover_cursor() {
            self.sync_pointer_protocol();
        }
        self.sync_session_manager();
        if let Some(help_mode) = self.help_mode.clone() {
            if self.config.has_mode(&help_mode) {
                self.renderer.set_help(Some(HelpView {
                    mode: help_mode.clone(),
                    hints: self.config.describe_help_mode(&help_mode),
                }));
            } else {
                self.help_mode = None;
                self.renderer.set_help(None);
            }
        }
        if compact_changed {
            self.resize_windows()?;
            self.renderer.invalidate();
        }
        self.redraw()
    }

    fn track_foreground_applications(&mut self) {
        for window in &mut self.windows {
            if window.command_output.command_started_at.is_none() {
                continue;
            }
            let Ok(foreground) = tcgetpgrp(&window.master) else {
                continue;
            };
            if foreground == window.child {
                continue;
            }
            if let Some(application) = process_name(foreground)
                && !window
                    .notification_applications
                    .iter()
                    .any(|observed| observed.eq_ignore_ascii_case(&application))
            {
                window.notification_applications.push(application);
            }
        }
    }

    fn attach_client(&mut self, stream: UnixStream) -> Result<()> {
        self.status_mouse_down = false;
        // The listener is nonblocking so it can share the server poll loop. On
        // macOS an accepted socket can retain that mode; a large initial frame
        // may then return EAGAIN, which must not be mistaken for a disconnect.
        stream.set_nonblocking(false)?;
        self.client = Some(stream);
        // TerminalGuard re-enters the alternate screen on every connection,
        // clearing the outer terminal's uploads even though child apps stay alive.
        for window in &mut self.windows {
            window.graphics_replay_pending = true;
        }
        self.last_connected_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Err(error) =
            crate::session::record_connection(&self.session_name, self.last_connected_at)
        {
            let _ = self.notify(&format!("could not save connection time: {error}"));
        }
        self.outer_dnd_window = None;
        self.outer_keyboard_flags = None;
        self.outer_pointer_shape = None;
        self.outer_rich_paste = None;
        self.client_input.clear();
        self.input_decoder = InputDecoder::default();
        self.dnd_input = KittyDndParser::default();
        self.internal_dnd.clear();
        self.ipc_input = KittyIpcParser::default();
        self.reset_mode();
        self.renderer.invalidate();
        self.sync_keyboard_protocol();
        self.sync_pointer_protocol();
        self.sync_rich_paste_protocol();
        self.send_focus_event(self.active, true);
        self.redraw()
    }

    fn accept_connection(&mut self, mut stream: UnixStream) -> Result<bool> {
        // The nonblocking listener can produce a nonblocking accepted socket on
        // macOS. A client may be accepted just before its first resize message
        // arrives, so switch to blocking mode before classifying the connection.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        let mut kind = [0_u8; 1];
        stream.read_exact(&mut kind)?;
        match kind[0] {
            crate::control::REQUEST => {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                let command = crate::control::read_frame(&mut stream).and_then(|source| {
                    Ok(toml::from_str::<crate::control::ControlCommand>(&source)?)
                });
                let result = match command {
                    Ok(crate::control::ControlCommand::SaveSession { .. }) => {
                        match self.queue_session_save(true, Some(stream.try_clone()?)) {
                            Ok(()) => return Ok(true),
                            Err(error) => Err(error),
                        }
                    }
                    Ok(command) => self.control_command(command),
                    Err(error) => Err(error),
                };
                let response = match result {
                    Ok(output) => crate::control::Response { ok: true, output },
                    Err(error) => crate::control::Response {
                        ok: false,
                        output: error.to_string(),
                    },
                };
                crate::control::write_frame(&mut stream, &toml::to_string(&response)?)?;
                Ok(true)
            }
            CLIENT_QUERY_STATUS => {
                let tabs = self.tabs.len();
                let panes = self
                    .windows
                    .iter()
                    .filter(|window| !window.floating)
                    .count();
                let connected = u8::from(self.client.is_some());
                writeln!(
                    stream,
                    "{tabs}\t{panes}\t{connected}\t{}\t{}",
                    self.created_at, self.last_connected_at
                )?;
                Ok(true)
            }
            CLIENT_DISCONNECT => {
                self.detach_client();
                stream.write_all(b"OK")?;
                Ok(true)
            }
            CLIENT_RENAME_SESSION => {
                let mut length = [0_u8; 1];
                stream.read_exact(&mut length)?;
                let mut name = vec![0_u8; usize::from(length[0])];
                stream.read_exact(&mut name)?;
                let name = std::str::from_utf8(&name)?;
                let result = self.rename_current_session(name);
                match result {
                    Ok(()) => stream.write_all(b"OK")?,
                    Err(error) => write!(stream, "{error}")?,
                }
                Ok(true)
            }
            crate::CLIENT_DELETE_SESSION => {
                self.flush_session_saves();
                delete_session_snapshot(&self.session_name)?;
                self.config.autosave_interval_seconds = 0;
                Ok(false)
            }
            CLIENT_SHUTDOWN => Ok(false),
            CLIENT_RESIZE => {
                // Consume the initial resize before closing a rejected socket, so
                // unread request bytes cannot turn the warning into a reset.
                let mut resize = [0_u8; 9];
                resize[0] = CLIENT_RESIZE;
                stream.read_exact(&mut resize[1..])?;
                if self.client.is_some() {
                    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                    stream.write_all(crate::SERVER_SESSION_BUSY)?;
                    stream.shutdown(std::net::Shutdown::Write)?;
                    return Ok(true);
                }
                let columns = u16::from_be_bytes([resize[1], resize[2]]);
                let rows = u16::from_be_bytes([resize[3], resize[4]]);
                let width = u16::from_be_bytes([resize[5], resize[6]]);
                let height = u16::from_be_bytes([resize[7], resize[8]]);
                self.update_size((columns, rows), (width, height))?;
                stream.set_read_timeout(None)?;
                self.attach_client(stream)?;
                Ok(true)
            }
            _ => Err("unknown client connection request".into()),
        }
    }

    fn pane_index(&self, id: Option<usize>) -> Result<usize> {
        match id {
            Some(id) => self
                .windows
                .iter()
                .position(|window| window.id == id)
                .ok_or_else(|| format!("pane {id} does not exist").into()),
            None if !self.windows.is_empty() => Ok(self.active),
            None => Err("session has no panes".into()),
        }
    }

    fn control_command(&mut self, command: crate::control::ControlCommand) -> Result<String> {
        use crate::control::ControlCommand;
        match command {
            ControlCommand::ListPanes { toml: machine, .. } => {
                #[derive(serde::Serialize)]
                struct PaneInfo {
                    id: usize,
                    window: usize,
                    active: bool,
                    floating: bool,
                    name: String,
                    title: String,
                    cwd: Option<PathBuf>,
                }
                #[derive(serde::Serialize)]
                struct PaneList {
                    panes: Vec<PaneInfo>,
                }
                let panes: Vec<_> = self
                    .windows
                    .iter()
                    .enumerate()
                    .map(|(index, pane)| PaneInfo {
                        id: pane.id,
                        window: self
                            .tabs
                            .iter()
                            .position(|tab| tab.id == pane.tab_id)
                            .map_or(0, |i| i + 1),
                        active: index == self.active,
                        floating: pane.floating,
                        name: pane.name.clone(),
                        title: pane.terminal_title().to_owned(),
                        cwd: self.window_directory(pane),
                    })
                    .collect();
                if machine {
                    return Ok(toml::to_string_pretty(&PaneList { panes })?);
                }
                let clean = |text: &str| text.replace(['\t', '\n', '\r'], " ");
                Ok(panes
                    .iter()
                    .map(|pane| {
                        format!(
                            "{}\t{}\t{}\t{}\n",
                            pane.id,
                            pane.window,
                            clean(&pane.title),
                            clean(
                                &pane
                                    .cwd
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default()
                            )
                        )
                    })
                    .collect())
            }
            ControlCommand::NewWindow { name, .. } => {
                self.create_window()?;
                if let Some(name) = name {
                    let tab = self.windows[self.active].tab_id;
                    rename_tab(&mut self.windows, tab, &name);
                    self.redraw()?;
                }
                Ok(format!("{}\n", self.windows[self.active].id))
            }
            ControlCommand::SplitPane { target, down } => {
                let index = self.pane_index(target.pane)?;
                let pane = &self.windows[index];
                if pane.floating {
                    return Err("cannot split a floating pane".into());
                }
                if (down && pane.pane_rect.height < 6) || (!down && pane.pane_rect.width < 12) {
                    return Err("not enough space to split this pane".into());
                }
                self.select_tab_id(pane.tab_id)?;
                self.set_active(index);
                self.new_pane(if down {
                    SplitAxis::Horizontal
                } else {
                    SplitAxis::Vertical
                })?;
                Ok(format!("{}\n", self.windows[self.active].id))
            }
            ControlCommand::SendKeys {
                target,
                literal,
                enter,
                keys,
            } => {
                let index = self.pane_index(target.pane)?;
                let mut bytes = if literal {
                    keys.join(" ").into_bytes()
                } else {
                    keys.iter()
                        .map(|key| crate::config::parse_send_key(key))
                        .collect::<std::result::Result<Vec<_>, _>>()?
                        .concat()
                };
                if enter {
                    bytes.push(b'\r');
                }
                if bytes.len() > 4096 {
                    return Err("send-keys input exceeds 4096 bytes".into());
                }
                if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
                    self.windows[index].notification_applications.clear();
                    self.windows[index].command_output.command_submitted();
                }
                write_control_input(&self.windows[index].master, &bytes)?;
                Ok(String::new())
            }
            ControlCommand::CapturePane { target, history } => {
                let index = self.pane_index(target.pane)?;
                let text = if history {
                    window_history(&mut self.windows[index])
                } else {
                    self.windows[index].terminal.screen().contents()
                };
                Ok(format!("{text}\n"))
            }
            ControlCommand::JoinPane {
                target,
                to_pane,
                down,
            } => {
                let index = self.pane_index(target.pane)?;
                let destination = self.pane_index(Some(to_pane))?;
                self.relocate_pane(index, Some(destination), down, None)?;
                Ok(format!("{}\n", self.windows[index].id))
            }
            ControlCommand::BreakPane { target, name } => {
                let index = self.pane_index(target.pane)?;
                self.relocate_pane(index, None, false, name)?;
                Ok(format!("{}\n", self.windows[index].id))
            }
            ControlCommand::SaveSession { .. } => {
                self.save_current_session()?;
                Ok(String::new())
            }
        }
    }

    fn move_pane_to_relative_window(&mut self, offset: isize) -> Result<()> {
        if self.tabs.len() < 2 {
            return self.notify("no other window to move the pane to");
        }
        let current = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.windows[self.active].tab_id)
            .ok_or("current window missing")?;
        let next = (current as isize + offset).rem_euclid(self.tabs.len() as isize) as usize;
        let destination = self.pane_index(Some(pane_ids(&self.tabs[next].root)[0]))?;
        if let Err(error) = self.relocate_pane(self.active, Some(destination), false, None) {
            self.notify(&error.to_string())?;
        }
        Ok(())
    }

    fn relocate_pane(
        &mut self,
        index: usize,
        destination: Option<usize>,
        down: bool,
        name: Option<String>,
    ) -> Result<()> {
        let source = &self.windows[index];
        if source.floating || source.temporary_file.is_some() {
            return Err("only regular tiled panes can be moved between windows".into());
        }
        let pane_id = source.id;
        let old_tab = source.tab_id;
        let old_root = self
            .tabs
            .iter()
            .find(|tab| tab.id == old_tab)
            .ok_or("source window missing")?
            .root
            .clone();
        let remaining = remove_pane(old_root, pane_id);
        let (new_tab, new_root, new_name) = if let Some(destination) = destination {
            let target = &self.windows[destination];
            if index == destination || target.floating || target.temporary_file.is_some() {
                return Err("destination must be a different regular tiled pane".into());
            }
            if (down && target.pane_rect.height < 6) || (!down && target.pane_rect.width < 12) {
                return Err("not enough space in the destination pane".into());
            }
            let mut root = if target.tab_id == old_tab {
                remaining
                    .clone()
                    .ok_or("source window has no remaining panes")?
            } else {
                self.tabs
                    .iter()
                    .find(|tab| tab.id == target.tab_id)
                    .ok_or("destination window missing")?
                    .root
                    .clone()
            };
            if !split_pane(
                &mut root,
                target.id,
                pane_id,
                if down {
                    SplitAxis::Horizontal
                } else {
                    SplitAxis::Vertical
                },
            ) {
                return Err("destination pane is missing from its layout".into());
            }
            (target.tab_id, root, target.name.clone())
        } else {
            (
                self.next_id,
                PaneNode::Leaf(pane_id),
                name.unwrap_or_else(|| source.name.clone()),
            )
        };
        // Commit the layout only after validating both source and destination.
        if old_tab != new_tab {
            if let Some(root) = remaining {
                self.tabs
                    .iter_mut()
                    .find(|tab| tab.id == old_tab)
                    .unwrap()
                    .root = root;
            } else {
                self.tabs.retain(|tab| tab.id != old_tab);
            }
        }
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == new_tab) {
            tab.root = new_root;
        } else {
            self.tabs.push(Tab {
                id: new_tab,
                root: new_root,
            });
            self.next_id += 1;
        }
        self.windows[index].tab_id = new_tab;
        self.windows[index].name = new_name;
        for window in &mut self.windows {
            if window.floating && window.return_to_window == Some(pane_id) {
                window.tab_id = new_tab;
            }
            if window.tab_id == old_tab || window.tab_id == new_tab {
                window.zoomed = false;
            }
        }
        self.reset_mode();
        self.select_tab_id(new_tab)?;
        self.set_active(index);
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn detach_client(&mut self) {
        self.status_mouse_down = false;
        self.autosave_session(true);
        if !self.windows.is_empty() {
            self.send_focus_event(self.active, false);
        }
        self.clear_outer_dnd_registration();
        self.client = None;
        self.client_input.clear();
        self.input_decoder = InputDecoder::default();
        self.dnd_input = KittyDndParser::default();
        self.internal_dnd.clear();
        self.ipc_input = KittyIpcParser::default();
        self.outer_keyboard_flags = None;
        self.outer_pointer_shape = None;
        self.outer_rich_paste = None;
        self.reset_mode();
        self.redraw_deadline = None;
        self.clipboard_status_until = None;
        self.notification_until = None;
        self.renderer.set_border_status(None);
        self.renderer.set_notification(None);
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
        let ipc = self.ipc_input.process(bytes);
        for command in ipc.commands {
            self.route_kitty_ipc_input(&command)?;
        }
        self.handle_dnd_input(&ipc.terminal)
    }

    fn handle_dnd_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let output = self.dnd_input.process(bytes);
        for event in output.events {
            match event {
                KittyDndEvent::Terminal(bytes) => {
                    let decoded = self.input_decoder.push(&bytes);
                    if !decoded.is_empty() && !self.handle_decoded_input(&decoded)? {
                        return Ok(false);
                    }
                }
                KittyDndEvent::Command(command) => self.route_kitty_dnd_input(&command)?,
            }
        }
        Ok(true)
    }

    fn route_kitty_ipc_input(&mut self, command: &[u8]) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        if let Some((pane_id, command)) = kitty_ipc_for_child(command) {
            if let Some(window) = self.windows.iter().find(|window| window.id == pane_id) {
                write_fd(&window.master, &command)?;
            }
        } else if kitty_ipc_is_clipboard(command)
            && self.windows[self.active].input_modes.rich_clipboard_paste()
        {
            write_fd(&self.windows[self.active].master, command)?;
        }
        Ok(())
    }

    fn route_kitty_dnd_input(&mut self, command: &[u8]) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        let parsed = kitty_dnd_command(command);
        let drop_position = parsed.and_then(|command| {
            matches!(command.kind, Some('m' | 'M'))
                .then(|| command.x.zip(command.y))
                .flatten()
                .filter(|(x, y)| *x >= 0 && *y >= 0)
        });
        let index = match kitty_dnd_drag_start_position(command) {
            Some(position) => {
                let Some(index) = self.dnd_drag_target(position) else {
                    return Ok(());
                };
                index
            }
            None if drop_position.is_some() => {
                let Some(index) = self.dnd_drop_target(drop_position.expect("position exists"))
                else {
                    return Ok(());
                };
                index
            }
            None => match kitty_dnd_id(command) {
                Some(id) => {
                    let Some(index) = self
                        .windows
                        .iter()
                        .position(|window| window.id == id as usize)
                    else {
                        return Ok(());
                    };
                    index
                }
                None => self.active,
            },
        };
        if let Some(parsed) = parsed.filter(|command| matches!(command.kind, Some('m' | 'M'))) {
            self.internal_dnd
                .begin_target(self.windows[index].id, parsed.payload);
        }
        let (origin, size) = self.dnd_geometry(index);
        let cell_pixels = (
            self.terminal_pixels.0 / self.terminal_size.0.max(1),
            self.terminal_pixels.1 / self.terminal_size.1.max(1),
        );
        if let Some(command) = kitty_dnd_for_child(command, origin, size, cell_pixels) {
            write_fd(&self.windows[index].master, &command)?;
        }
        if parsed.is_some_and(|command| {
            (command.kind == Some('e') && command.x == Some(4))
                || (command.kind == Some('E') && command.payload != b"OK")
        }) {
            self.internal_dnd.clear();
        }
        Ok(())
    }

    fn dnd_drag_target(&self, position: (i32, i32)) -> Option<usize> {
        self.dnd_target(position, true)
    }

    fn dnd_drop_target(&self, position: (i32, i32)) -> Option<usize> {
        self.dnd_target(position, false)
    }

    fn dnd_target(&self, position: (i32, i32), drag: bool) -> Option<usize> {
        let active = self.windows.get(self.active)?;
        let registered = |window: &Window| {
            if drag {
                window.dnd_drag_registration.is_some()
            } else {
                window.dnd_drop_registration.is_some()
            }
        };
        let contains = |index: usize| {
            let (origin, size) = self.dnd_geometry(index);
            position.0 >= origin.0
                && position.1 >= origin.1
                && position.0 < origin.0 + i32::from(size.0)
                && position.1 < origin.1 + i32::from(size.1)
        };
        if (active.floating || active.zoomed) && registered(active) && contains(self.active) {
            return Some(self.active);
        }
        self.windows.iter().enumerate().find_map(|(index, window)| {
            (!window.floating
                && window.tab_id == active.tab_id
                && registered(window)
                && contains(index))
            .then_some(index)
        })
    }

    fn dnd_geometry(&self, index: usize) -> ((i32, i32), (u16, u16)) {
        let window = &self.windows[index];
        if window.floating {
            let layout = floating_layout_for(self.terminal_size, self.config.compact());
            return (
                (i32::from(layout.column), i32::from(layout.row)),
                layout.content_size(),
            );
        }
        if window.pane_framed {
            return (
                (
                    i32::from(window.pane_rect.column) + 1,
                    i32::from(window.pane_rect.row) + 2,
                ),
                pane_pty_size(window.pane_rect, true),
            );
        }
        ((1, 2), pane_pty_size(window.pane_rect, false))
    }

    fn handle_window_dnd_command(&mut self, index: usize, command: &[u8]) -> Result<()> {
        let window_id = self.windows[index].id;
        if let Some(parsed) = kitty_dnd_command(command) {
            if parsed.kind == Some('o') && parsed.operation.is_some_and(|operation| operation > 0) {
                self.internal_dnd.begin_source(window_id, parsed.payload);
            } else if parsed.kind == Some('p')
                || (parsed.kind.is_none()
                    && self.internal_dnd.accepts_pre_sent_continuation(window_id))
            {
                self.internal_dnd.cache_pre_sent_data(
                    window_id,
                    parsed.x,
                    parsed.more,
                    parsed.payload,
                );
            } else if parsed.kind == Some('r')
                && let Some(request_index) = parsed.x
                && let Some(response) = self.internal_dnd.data_response(window_id, request_index)
            {
                write_fd(&self.windows[index].master, &response)?;
                return Ok(());
            }
        }
        let internal_target_response = kitty_dnd_command(command).is_some_and(|parsed| {
            self.internal_dnd
                .routes_response_to_source(window_id, parsed.kind)
        });
        let routed_window_id = if internal_target_response {
            self.internal_dnd.source_window_id().unwrap_or(window_id)
        } else {
            window_id
        };
        let Ok(id) = u32::try_from(routed_window_id) else {
            return Ok(());
        };
        let Some(command) = kitty_dnd_with_id(command, id) else {
            return Ok(());
        };
        match kitty_dnd_registration(&command) {
            Some(KittyDndRegistration::Drag(true)) => {
                self.windows[index].dnd_drag_registration = Some(command.clone());
            }
            Some(KittyDndRegistration::Drag(false)) => {
                self.windows[index].dnd_drag_registration = None;
            }
            Some(KittyDndRegistration::Drop(true)) => {
                self.windows[index].dnd_drop_registration = Some(command.clone());
            }
            Some(KittyDndRegistration::Drop(false)) => {
                self.windows[index].dnd_drop_registration = None;
            }
            None => {}
        }
        if index == self.active {
            self.write_client_protocol(&command);
            self.outer_dnd_window = (self.windows[index].dnd_drag_registration.is_some()
                || self.windows[index].dnd_drop_registration.is_some())
            .then_some(self.windows[index].id);
        } else if internal_target_response {
            self.write_client_protocol(&command);
        }
        Ok(())
    }

    fn write_client_protocol(&mut self, bytes: &[u8]) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if client
            .write_all(bytes)
            .and_then(|()| client.flush())
            .is_err()
        {
            self.detach_client();
        }
    }

    fn sync_keyboard_protocol(&mut self) {
        if self.windows.is_empty() || self.client.is_none() {
            return;
        }
        let flags = self.windows[self.active].input_modes.keyboard_flags();
        if self.outer_keyboard_flags == Some(flags) {
            return;
        }
        self.write_client_protocol(format!("\x1b[={flags}u").as_bytes());
        self.outer_keyboard_flags = self.client.is_some().then_some(flags);
    }

    fn sync_pointer_protocol(&mut self) {
        if self.windows.is_empty() || self.client.is_none() {
            return;
        }
        let shape = self.windows[self.active]
            .terminal_osc
            .pointer_shape()
            .to_owned();
        self.set_outer_pointer_shape(&shape);
    }

    fn sync_rich_paste_protocol(&mut self) {
        if self.windows.is_empty() || self.client.is_none() {
            return;
        }
        let enabled = self.windows[self.active].input_modes.rich_clipboard_paste();
        if self.outer_rich_paste == Some(enabled) {
            return;
        }
        self.write_client_protocol(if enabled {
            b"\x1b[?5522h"
        } else {
            b"\x1b[?5522l"
        });
        self.outer_rich_paste = self.client.is_some().then_some(enabled);
    }

    fn set_outer_pointer_shape(&mut self, shape: &str) {
        if self.client.is_none() || self.outer_pointer_shape.as_deref() == Some(shape) {
            return;
        }
        self.write_client_protocol(format!("\x1b]22;{shape}\x1b\\").as_bytes());
        self.outer_pointer_shape = self.client.is_some().then(|| shape.to_owned());
    }

    fn update_pointer_for_position(&mut self, position: MousePosition) {
        if !self.config.mouse_hover_cursor() {
            self.sync_pointer_protocol();
            return;
        }
        let shape = if self.mouse_drag.is_some() {
            "grabbing"
        } else if self
            .renderer
            .status_click_at((position.column, position.row))
            .is_some()
            || self
                .renderer
                .window_tab_at((position.column, position.row))
                .is_some()
        {
            "pointer"
        } else if let Some((_, handle)) = self.pane_resize_at(position) {
            match handle.axis() {
                SplitAxis::Vertical => "ew-resize",
                SplitAxis::Horizontal => "ns-resize",
            }
        } else {
            self.windows[self.active].terminal_osc.pointer_shape()
        }
        .to_owned();
        self.set_outer_pointer_shape(&shape);
    }

    fn send_focus_event(&self, index: usize, focused: bool) {
        if self.client.is_some()
            && self
                .windows
                .get(index)
                .is_some_and(|window| window.input_modes.focus_reporting())
        {
            let _ = write_fd(
                &self.windows[index].master,
                if focused { b"\x1b[I" } else { b"\x1b[O" },
            );
        }
    }

    fn set_active(&mut self, index: usize) {
        if index == self.active || index >= self.windows.len() {
            return;
        }
        let previous = self.active;
        self.send_focus_event(previous, false);
        self.active = index;
        self.sync_keyboard_protocol();
        self.sync_pointer_protocol();
        self.sync_rich_paste_protocol();
        self.send_focus_event(index, true);
    }

    fn clear_outer_dnd_registration(&mut self) {
        let Some(id) = self.outer_dnd_window.take() else {
            return;
        };
        let Some(client) = self.client.as_mut() else {
            return;
        };
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        for disable in [b"\x1b]72;t=A\x1b\\".as_slice(), b"\x1b]72;t=o:x=2\x1b\\"] {
            if let Some(command) = kitty_dnd_with_id(disable, id) {
                let _ = client.write_all(&command);
            }
        }
        let _ = client.flush();
    }

    fn sync_kitty_dnd_registration(&mut self) {
        if self.client.is_none() || self.windows.is_empty() {
            return;
        }
        let active = &self.windows[self.active];
        let desired = (active.dnd_drag_registration.is_some()
            || active.dnd_drop_registration.is_some())
        .then_some(active.id);
        if desired == self.outer_dnd_window {
            return;
        }
        if let Some(previous) = self.outer_dnd_window
            && let Ok(id) = u32::try_from(previous)
        {
            for disable in [b"\x1b]72;t=A\x1b\\".as_slice(), b"\x1b]72;t=o:x=2\x1b\\"] {
                if let Some(command) = kitty_dnd_with_id(disable, id) {
                    self.write_client_protocol(&command);
                }
            }
        }
        if let Some(id) = desired {
            let registrations = self
                .windows
                .iter()
                .find(|window| window.id == id)
                .map(|window| {
                    [
                        window.dnd_drag_registration.clone(),
                        window.dnd_drop_registration.clone(),
                    ]
                })
                .unwrap_or_default();
            for registration in registrations.into_iter().flatten() {
                self.write_client_protocol(&registration);
            }
        }
        self.outer_dnd_window = desired.filter(|_| self.client.is_some());
    }

    fn execute_status_click(&mut self, click: crate::render::StatusClick) -> Result<bool> {
        use crate::render::StatusClick;
        match click {
            StatusClick::Key { mode, key } => {
                if mode != self.mode {
                    return Ok(true);
                }
                let Some(actions) = self.config.actions(&mode, &key).map(<[Action]>::to_vec) else {
                    return Ok(true);
                };
                self.help_mode = None;
                self.renderer.set_help(None);
                self.redraw()?;
                self.execute_actions(&actions)
            }
            StatusClick::Help => {
                self.show_help()?;
                Ok(true)
            }
            StatusClick::CloseSessionManager => {
                self.close_session_manager();
                self.redraw()?;
                Ok(true)
            }
        }
    }

    fn handle_decoded_input(&mut self, bytes: &[u8]) -> Result<bool> {
        let mut passthrough = Vec::with_capacity(bytes.len());
        let mut index = 0;
        let mut selection_cleared = false;
        while index < bytes.len() {
            if let Some((focused, consumed)) = decode_focus_event(&bytes[index..]) {
                if !passthrough.is_empty() {
                    self.write_active(&passthrough)?;
                    passthrough.clear();
                }
                self.send_focus_event(self.active, focused);
                index += consumed;
                continue;
            }
            if let Some((mouse, consumed)) = decode_sgr_mouse(&bytes[index..]) {
                let position = (mouse.position().column, mouse.position().row);
                let captured =
                    self.status_mouse_down && !matches!(mouse, MouseAction::SelectStart(_));
                let dragging_selection = self.selection.is_some()
                    && matches!(
                        mouse,
                        MouseAction::SelectExtend(_) | MouseAction::SelectEnd(_)
                    );
                let on_bar = self.renderer.status_bar_contains(position)
                    && self.mouse_drag.is_none()
                    && !dragging_selection;
                if captured || on_bar {
                    if !passthrough.is_empty() {
                        self.write_active(&passthrough)?;
                        passthrough.clear();
                    }
                    self.update_pointer_for_position(mouse.position());
                    if matches!(mouse, MouseAction::SelectEnd(_)) {
                        self.status_mouse_down = false;
                    }
                    if !captured && matches!(mouse, MouseAction::SelectStart(_)) {
                        self.status_mouse_down = true;
                        if let Some(click) = self.renderer.status_click_at(position)
                            && !self.execute_status_click(click)?
                        {
                            return Ok(false);
                        }
                    }
                    index += consumed;
                    continue;
                }
                // Modal overlays must not decode mouse escape sequences as text keys.
                if self.help_mode.is_some()
                    || self.session_manager.is_some()
                    || self.rename_state.is_some()
                    || self
                        .history_search
                        .as_ref()
                        .is_some_and(|search| search.editing)
                {
                    index += consumed;
                    continue;
                }
            }
            if let Some(help_mode) = self.help_mode.clone() {
                let (key, consumed) = decode_key(&bytes[index..]);
                if matches!(key.name.as_str(), "esc" | "?") {
                    self.help_mode = None;
                    self.renderer.set_help(None);
                    self.redraw()?;
                } else if let Some(actions) = self
                    .config
                    .actions(&help_mode, &key.name)
                    .map(<[Action]>::to_vec)
                {
                    self.help_mode = None;
                    self.renderer.set_help(None);
                    self.redraw()?;
                    if !self.execute_actions(&actions)? {
                        return Ok(false);
                    }
                }
                index += consumed;
                continue;
            }
            if self.session_manager.is_some() {
                let (key, consumed) = decode_key(&bytes[index..]);
                if !self.handle_session_manager_key(&key)? {
                    return Ok(false);
                }
                index += consumed;
                continue;
            }
            if self.rename_state.is_some() {
                let (key, consumed) = decode_key(&bytes[index..]);
                self.handle_rename_key(&key)?;
                index += consumed;
                continue;
            }
            if self
                .history_search
                .as_ref()
                .is_some_and(|search| search.editing)
            {
                let (key, consumed) = decode_key(&bytes[index..]);
                self.handle_history_search_key(&key)?;
                index += consumed;
                continue;
            }
            if let Some((mouse, consumed)) = decode_sgr_mouse(&bytes[index..]) {
                self.update_pointer_for_position(mouse.position());
                if self.mouse_drag.is_some() {
                    if !passthrough.is_empty() {
                        self.write_active(&passthrough)?;
                        passthrough.clear();
                    }
                    self.update_mouse_drag(mouse)?;
                } else if matches!(mouse, MouseAction::SelectStart(_))
                    && let Some(tab_id) = self
                        .renderer
                        .window_tab_at((mouse.position().column, mouse.position().row))
                {
                    if !passthrough.is_empty() {
                        self.write_active(&passthrough)?;
                        passthrough.clear();
                    }
                    self.mouse_drag = Some(MouseDrag::WindowBar);
                    self.select_tab_id(tab_id)?;
                } else if matches!(mouse, MouseAction::SelectStart(_))
                    && let Some((tab_id, handle)) = self.pane_resize_at(mouse.position())
                {
                    if !passthrough.is_empty() {
                        self.write_active(&passthrough)?;
                        passthrough.clear();
                    }
                    self.mouse_drag = Some(MouseDrag::PaneResize { tab_id, handle });
                } else if self.windows[self.active].history_mode {
                    if !passthrough.is_empty() {
                        self.write_active(&passthrough)?;
                        passthrough.clear();
                    }
                    self.apply_mouse_action(mouse)?;
                } else if self.mode == "locked" {
                    if matches!(mouse, MouseAction::SelectStart(_))
                        && let Some(target) = pane_at(&self.windows, self.active, mouse.position())
                        && target != self.active
                    {
                        if !passthrough.is_empty() {
                            self.write_active(&passthrough)?;
                            passthrough.clear();
                        }
                        self.set_active(target);
                        self.selection = None;
                        self.redraw()?;
                    }
                    if self.windows[self.active]
                        .terminal
                        .screen()
                        .mouse_protocol_mode()
                        != vt100::MouseProtocolMode::None
                        && let Some(position) =
                            self.content_position_for(self.active, mouse.position(), false)
                        && let Some(sequence) =
                            sgr_mouse_at(&bytes[index..index + consumed], position)
                    {
                        passthrough.extend_from_slice(&sequence);
                    }
                }
                index += consumed;
                continue;
            }

            if !selection_cleared && self.selection.take().is_some() {
                selection_cleared = true;
                self.redraw()?;
            }

            let (key, consumed) = decode_key(&bytes[index..]);
            if key.event_type == 3 && self.mode != "locked" {
                index += consumed;
                continue;
            }
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
                Action::MoveWindowLeft => self.move_active_window(-1)?,
                Action::MoveWindowRight => self.move_active_window(1)?,
                Action::SwitchSession => self.open_session_manager()?,
                Action::GoToWindow(index) => self.select_window(*index)?,
                Action::CloseWindow => self.close_active()?,
                Action::Detach => return Ok(false),
                Action::ShowHelp => self.show_help()?,
                Action::ScrollUp => self.apply_history_action(HistoryAction::Up(1))?,
                Action::ScrollDown => self.apply_history_action(HistoryAction::Down(1))?,
                Action::PageUp => {
                    self.selection = None;
                    self.apply_history_action(HistoryAction::Up(self.history_page_rows()))?;
                }
                Action::PageDown => {
                    self.selection = None;
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
                Action::MovePaneLeft => self.move_active_pane(Direction::Left)?,
                Action::MovePaneRight => self.move_active_pane(Direction::Right)?,
                Action::MovePaneUp => self.move_active_pane(Direction::Up)?,
                Action::BreakPane => {
                    if let Err(error) = self.relocate_pane(self.active, None, false, None) {
                        self.notify(&error.to_string())?;
                    }
                }
                Action::MovePaneNextWindow => self.move_pane_to_relative_window(1)?,
                Action::MovePanePreviousWindow => self.move_pane_to_relative_window(-1)?,
                Action::MovePaneDown => self.move_active_pane(Direction::Down)?,
                Action::ClosePane => self.close_pane()?,
                Action::ResizePaneLeft => self.resize_active_pane(Direction::Left)?,
                Action::ResizePaneRight => self.resize_active_pane(Direction::Right)?,
                Action::ResizePaneUp => self.resize_active_pane(Direction::Up)?,
                Action::ResizePaneDown => self.resize_active_pane(Direction::Down)?,
                Action::TogglePaneZoom => self.toggle_pane_zoom()?,
                Action::SearchHistory => self.begin_history_search()?,
                Action::NextSearchMatch => self.select_history_match(1)?,
                Action::PreviousSearchMatch => self.select_history_match(-1)?,
                Action::ToggleHistorySelection => self.toggle_history_selection()?,
                Action::SelectionLeft => self.move_history_selection(-1)?,
                Action::SelectionRight => self.move_history_selection(1)?,
                Action::CopySelection => self.copy_history_selection()?,
            }
        }
        Ok(true)
    }

    fn open_session_manager(&mut self) -> Result<()> {
        let sessions = available_session_info(Some(self.local_session_info()))?;
        let selected = default_session_selection(&sessions, &self.session_name);
        self.session_manager = Some(SessionManagerState {
            searching: false,
            query: String::new(),
            sessions,
            selected,
            rename_input: None,
        });
        self.sync_session_manager();
        self.redraw()
    }

    fn handle_session_manager_key(&mut self, key: &DecodedKey) -> Result<bool> {
        if self
            .session_manager
            .as_ref()
            .is_some_and(|state| state.rename_input.is_some())
        {
            return self.handle_session_rename_key(key);
        }
        let searching = self
            .session_manager
            .as_ref()
            .is_some_and(|state| state.searching);
        let action = self
            .config
            .session_manager_action(&key.name, searching)
            .unwrap_or("")
            .to_owned();
        match action.as_str() {
            "cancel" => {
                if searching {
                    let state = self
                        .session_manager
                        .as_mut()
                        .expect("session manager is open");
                    state.searching = false;
                    state.query.clear();
                    state.selected = 0;
                    self.sync_session_manager();
                } else {
                    self.close_session_manager();
                }
                self.redraw()?;
            }
            "search" => {
                self.session_manager
                    .as_mut()
                    .expect("session manager is open")
                    .searching = true;
                self.sync_session_manager();
                self.redraw()?;
            }
            "up" | "down" => {
                let state = self
                    .session_manager
                    .as_mut()
                    .expect("session manager is open");
                let count = matching_session_info(&state.sessions, &state.query).len();
                if count > 0 {
                    state.selected = if action == "up" {
                        state.selected.checked_sub(1).unwrap_or(count - 1)
                    } else {
                        (state.selected + 1) % count
                    };
                }
                self.sync_session_manager();
                self.redraw()?;
            }
            "complete" if searching => {
                let state = self
                    .session_manager
                    .as_mut()
                    .expect("session manager is open");
                let matches = matching_session_info(&state.sessions, &state.query);
                if let Some(session) = matches.get(state.selected) {
                    state.query.clone_from(&session.name);
                    state.selected = 0;
                }
                self.sync_session_manager();
                self.redraw()?;
            }
            "open" => {
                let state = self
                    .session_manager
                    .as_ref()
                    .expect("session manager is open");
                let matches = matching_session_info(&state.sessions, &state.query);
                let target = matches
                    .get(state.selected)
                    .map(|session| session.name.clone())
                    .unwrap_or_else(|| state.query.clone());
                if target.is_empty() {
                    return Ok(true);
                }
                validate_session_name(&target)?;
                if target == self.session_name {
                    self.close_session_manager();
                    self.redraw()?;
                    return Ok(true);
                }
                self.request_session_switch(&target)?;
                return Ok(false);
            }
            "rename" => {
                let Some(target) = self.selected_session_name() else {
                    return Ok(true);
                };
                self.session_manager
                    .as_mut()
                    .expect("session manager is open")
                    .rename_input = Some(target);
                self.sync_session_manager();
                self.redraw()?;
            }
            "save" => self.save_current_session()?,
            "delete" => {
                let Some(target) = self.selected_session_name() else {
                    return Ok(true);
                };
                if target == self.session_name {
                    self.flush_session_saves();
                    delete_session_snapshot(&target)?;
                    for window in self.windows.drain(..) {
                        terminate_window(window);
                    }
                    return Ok(false);
                }
                delete_session(&target)?;
                self.session_manager
                    .as_mut()
                    .expect("session manager is open")
                    .query
                    .clear();
                self.refresh_session_manager()?;
                self.redraw()?;
            }
            "disconnect" => {
                let Some(target) = self.selected_session_name() else {
                    return Ok(true);
                };
                if target != self.session_name {
                    disconnect_session(&target)?;
                    self.refresh_session_manager()?;
                    self.redraw()?;
                }
            }
            "backspace" if searching => {
                let state = self
                    .session_manager
                    .as_mut()
                    .expect("session manager is open");
                state.query.pop();
                state.selected = 0;
                self.sync_session_manager();
                self.redraw()?;
            }
            _ => {
                if searching
                    && let Some(text) = key.text()
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                {
                    let state = self
                        .session_manager
                        .as_mut()
                        .expect("session manager is open");
                    if state.query.len() + text.len() <= 64 {
                        state.query.push_str(text);
                        state.selected = 0;
                    }
                }
                self.sync_session_manager();
                self.redraw()?;
            }
        }
        Ok(true)
    }

    fn sync_session_manager(&mut self) {
        let view = self.session_manager.as_mut().map(|state| {
            let sessions = matching_session_info(&state.sessions, &state.query);
            state.selected = state.selected.min(sessions.len().saturating_sub(1));
            SessionManagerView {
                searching: state.searching,
                keys: self.config.session_manager_keys(),
                query: state.query.clone(),
                sessions,
                selected: state.selected,
                current: self.session_name.clone(),
                rename_input: state.rename_input.clone(),
            }
        });
        self.renderer.set_session_manager(view);
    }

    fn selected_session_name(&self) -> Option<String> {
        let state = self.session_manager.as_ref()?;
        matching_session_info(&state.sessions, &state.query)
            .get(state.selected)
            .map(|session| session.name.clone())
    }

    fn refresh_session_manager(&mut self) -> Result<()> {
        let sessions = available_session_info(Some(self.local_session_info()))?;
        let selected_name = self.selected_session_name();
        if let Some(state) = self.session_manager.as_mut() {
            state.sessions = sessions;
            let matches = matching_session_info(&state.sessions, &state.query);
            state.selected = matches
                .iter()
                .position(|session| Some(&session.name) == selected_name.as_ref())
                .unwrap_or_else(|| default_session_selection(&matches, &self.session_name));
        }
        self.sync_session_manager();
        Ok(())
    }

    fn local_session_info(&self) -> SessionInfo {
        SessionInfo {
            name: self.session_name.clone(),
            tabs: self.tabs.len(),
            panes: self
                .windows
                .iter()
                .filter(|window| !window.floating)
                .count(),
            connected: self.client.is_some(),
            running: true,
            created_at: self.created_at,
            last_connected_at: self.last_connected_at,
            saved: load_session_snapshot(&self.session_name)
                .ok()
                .flatten()
                .is_some(),
        }
    }

    fn handle_session_rename_key(&mut self, key: &DecodedKey) -> Result<bool> {
        match self
            .config
            .session_manager_action(&key.name, true)
            .unwrap_or("")
        {
            "cancel" => {
                self.session_manager
                    .as_mut()
                    .expect("session manager is open")
                    .rename_input = None;
            }
            "open" => {
                let old_name = self.selected_session_name().unwrap_or_default();
                let new_name = self
                    .session_manager
                    .as_ref()
                    .and_then(|state| state.rename_input.clone())
                    .unwrap_or_default();
                validate_session_name(&new_name)?;
                if old_name != new_name {
                    if old_name == self.session_name {
                        self.rename_current_session(&new_name)?;
                    } else {
                        rename_session(&old_name, &new_name)?;
                    }
                }
                if let Some(state) = self.session_manager.as_mut() {
                    if state.searching {
                        state.query.clone_from(&new_name);
                    } else {
                        state.query.clear();
                    }
                    state.rename_input = None;
                }
                self.refresh_session_manager()?;
            }
            "backspace" => {
                self.session_manager
                    .as_mut()
                    .expect("session manager is open")
                    .rename_input
                    .as_mut()
                    .expect("rename input exists")
                    .pop();
            }
            _ => {
                if let Ok(text) = std::str::from_utf8(&key.raw)
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                {
                    let input = self
                        .session_manager
                        .as_mut()
                        .expect("session manager is open")
                        .rename_input
                        .as_mut()
                        .expect("rename input exists");
                    if input.len() + text.len() <= 64 {
                        input.push_str(text);
                    }
                }
            }
        }
        self.sync_session_manager();
        self.redraw()?;
        Ok(true)
    }

    fn rename_current_session(&mut self, new_name: &str) -> Result<()> {
        self.flush_session_saves();
        let old_name = self.session_name.clone();
        let new_path = self.socket_path.with_file_name(format!("{new_name}.sock"));
        if new_path.exists() {
            return Err(format!("session '{new_name}' already exists").into());
        }
        fs::rename(&self.socket_path, &new_path)?;
        if let Err(error) = rename_session_snapshot(&old_name, new_name) {
            let _ = fs::rename(&new_path, &self.socket_path);
            return Err(error);
        }
        self.socket_path = new_path;
        self.session_name = new_name.to_owned();
        self.renderer.set_session_name(new_name);
        Ok(())
    }

    fn close_session_manager(&mut self) {
        self.session_manager = None;
        self.renderer.set_session_manager(None);
        self.mode = self.config.default_mode.clone();
        self.renderer.invalidate();
    }

    fn request_session_switch(&mut self, target: &str) -> Result<()> {
        let Some(client) = self.client.as_mut() else {
            return Ok(());
        };
        client.write_all(SERVER_SWITCH_SESSION_PREFIX)?;
        client.write_all(target.as_bytes())?;
        client.write_all(b"\x07")?;
        client.flush()?;
        Ok(())
    }

    fn begin_window_rename(&mut self) -> Result<()> {
        let tab_id = self.windows[self.active].tab_id;
        let original_names = self
            .windows
            .iter()
            .filter(|window| !window.floating && window.tab_id == tab_id)
            .map(|window| (window.id, window.name.clone()))
            .collect();
        self.rename_state = Some(RenameState {
            tab_id,
            original_names,
            input: String::new(),
        });
        rename_tab(&mut self.windows, tab_id, "");
        self.sync_rename_prompt();
        self.redraw()
    }

    fn handle_rename_key(&mut self, key: &DecodedKey) -> Result<()> {
        let edit = edit_window_name(
            &mut self
                .rename_state
                .as_mut()
                .expect("rename input is active")
                .input,
            key,
        );
        match edit {
            RenameEdit::Continue => {
                let state = self.rename_state.as_ref().expect("rename input is active");
                rename_tab(&mut self.windows, state.tab_id, &state.input);
                self.sync_rename_prompt();
            }
            RenameEdit::Confirm => {
                self.rename_state = None;
                self.finish_window_rename();
            }
            RenameEdit::Cancel => {
                let state = self.rename_state.take().expect("rename input is active");
                for (window_id, name) in state.original_names {
                    if let Some(window) = self
                        .windows
                        .iter_mut()
                        .find(|window| window.id == window_id)
                    {
                        window.name = name;
                    }
                }
                self.finish_window_rename();
            }
        }
        self.redraw()
    }

    fn sync_rename_prompt(&mut self) {
        self.renderer
            .set_rename_prompt(self.rename_state.as_ref().map(|state| state.input.as_str()));
    }

    fn finish_window_rename(&mut self) {
        self.rename_state = None;
        self.renderer.set_rename_prompt(None);
        self.mode = self.config.default_mode.clone();
    }

    fn switch_mode(&mut self, mode: &str) -> Result<()> {
        if !self.config.has_mode(mode) {
            return Err(format!("unknown mode '{mode}'").into());
        }
        if self.mode == "scroll" && mode != "scroll" && !self.windows.is_empty() {
            let window = &mut self.windows[self.active];
            window.history_mode = false;
            window.terminal.screen_mut().set_scrollback(0);
            self.selection = None;
            self.history_search = None;
            self.renderer.set_history_search_prompt(None);
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
        self.notification_until = None;
        self.mouse_drag = None;
        if let Some(state) = self.rename_state.take() {
            for (window_id, name) in state.original_names {
                if let Some(window) = self
                    .windows
                    .iter_mut()
                    .find(|window| window.id == window_id)
                {
                    window.name = name;
                }
            }
        }
        self.renderer.set_border_status(None);
        self.renderer.set_notification(None);
        self.renderer.set_rename_prompt(None);
        self.history_search = None;
        self.renderer.set_history_search_prompt(None);
        self.session_manager = None;
        self.renderer.set_session_manager(None);
        self.help_mode = None;
        self.renderer.set_help(None);
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
        let current_directory = self.active_spawn_directory();
        let pane_id = match self.spawn_window(
            label.to_owned(),
            shell,
            arguments,
            SpawnOptions {
                temporary_file: Some(path.clone()),
                return_to_window: Some(return_to),
                floating: false,
                tab_id,
                current_directory,
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
        if self.client.is_none() {
            return Ok(());
        }
        self.notification_until = Some(Instant::now() + NOTIFICATION_DURATION);
        self.renderer.set_notification(Some(message));
        self.redraw()
    }

    fn apply_history_action(&mut self, action: HistoryAction) -> Result<()> {
        if self.selection.is_some_and(|selection| selection.keyboard) {
            let columns = isize::try_from(self.active_content_size().0).unwrap_or(isize::MAX);
            match action {
                HistoryAction::Up(rows) => {
                    return self.move_history_selection_cells(
                        -isize::try_from(rows).unwrap_or(isize::MAX) * columns,
                    );
                }
                HistoryAction::Down(rows) => {
                    return self.move_history_selection_cells(
                        isize::try_from(rows).unwrap_or(isize::MAX) * columns,
                    );
                }
                HistoryAction::Top | HistoryAction::Bottom => self.selection = None,
            }
        }
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

    fn begin_history_search(&mut self) -> Result<()> {
        if !self.windows[self.active].history_mode {
            return Ok(());
        }
        self.selection = None;
        self.history_search = Some(HistorySearchState {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            editing: true,
        });
        self.renderer.set_history_search_prompt(Some(""));
        self.redraw()
    }

    fn handle_history_search_key(&mut self, key: &DecodedKey) -> Result<()> {
        match key.name.as_str() {
            "esc" => {
                self.history_search = None;
                self.renderer.set_history_search_prompt(None);
            }
            "enter" => {
                if let Some(search) = self.history_search.as_mut() {
                    search.editing = false;
                }
                self.renderer.set_history_search_prompt(None);
            }
            "backspace" => {
                self.history_search
                    .as_mut()
                    .expect("history search is active")
                    .query
                    .pop();
                self.refresh_history_search();
            }
            _ => {
                if let Some(text) = key.text() {
                    let search = self
                        .history_search
                        .as_mut()
                        .expect("history search is active");
                    if search.query.len() + text.len() <= 256 {
                        search.query.push_str(text);
                        self.refresh_history_search();
                    }
                }
            }
        }
        self.sync_history_search_prompt();
        self.redraw()
    }

    fn refresh_history_search(&mut self) {
        let query = self
            .history_search
            .as_ref()
            .map(|search| search.query.clone())
            .unwrap_or_default();
        let lines = history_lines(&mut self.windows[self.active]);
        let matches = matching_history_lines(&lines, &query);
        if let Some(search) = self.history_search.as_mut() {
            search.matches = matches;
            search.selected = 0;
        }
        self.jump_to_history_match(lines.len());
    }

    fn select_history_match(&mut self, offset: isize) -> Result<()> {
        self.selection = None;
        let Some(search) = self.history_search.as_mut() else {
            return Ok(());
        };
        if search.matches.is_empty() {
            return Ok(());
        }
        search.selected =
            (search.selected as isize + offset).rem_euclid(search.matches.len() as isize) as usize;
        let line_count = history_lines(&mut self.windows[self.active]).len();
        self.jump_to_history_match(line_count);
        self.redraw()
    }

    fn jump_to_history_match(&mut self, line_count: usize) {
        let Some(line) = self
            .history_search
            .as_ref()
            .and_then(|search| search.matches.get(search.selected).copied())
        else {
            return;
        };
        let visible_rows = usize::from(self.windows[self.active].terminal.screen().size().0);
        let scrollback_rows = line_count.saturating_sub(visible_rows);
        self.windows[self.active]
            .terminal
            .screen_mut()
            .set_scrollback(scrollback_rows.saturating_sub(line.min(scrollback_rows)));
    }

    fn sync_history_search_prompt(&mut self) {
        self.renderer.set_history_search_prompt(
            self.history_search
                .as_ref()
                .filter(|search| search.editing)
                .map(|search| search.query.as_str()),
        );
    }

    fn toggle_history_selection(&mut self) -> Result<()> {
        if self.selection.is_some_and(|selection| selection.keyboard) {
            self.selection = None;
        } else {
            let (row, column) = self.windows[self.active]
                .terminal
                .screen()
                .cursor_position();
            let (columns, rows) = self.active_content_size();
            let position = MousePosition {
                column: column.min(columns.saturating_sub(1)),
                row: row.min(rows.saturating_sub(1)),
            };
            self.selection = Some(TextSelection {
                window_id: self.windows[self.active].id,
                start: position,
                end: position,
                keyboard: true,
            });
        }
        self.redraw()
    }

    fn move_history_selection(&mut self, offset: isize) -> Result<()> {
        if self.selection.is_none() {
            return self.toggle_history_selection();
        }
        self.move_history_selection_cells(offset)
    }

    fn move_history_selection_cells(&mut self, offset: isize) -> Result<()> {
        let (columns, rows) = self.active_content_size();
        let Some(mut selection) = self.selection.filter(|selection| selection.keyboard) else {
            return Ok(());
        };
        let width = usize::from(columns);
        let last = width.saturating_mul(usize::from(rows)).saturating_sub(1);
        let current = usize::from(selection.end.row)
            .saturating_mul(width)
            .saturating_add(usize::from(selection.end.column));
        let updated = current.saturating_add_signed(offset).min(last);
        selection.end = MousePosition {
            column: u16::try_from(updated % width).unwrap_or(u16::MAX),
            row: u16::try_from(updated / width).unwrap_or(u16::MAX),
        };
        self.selection = Some(selection);
        self.redraw()
    }

    fn copy_history_selection(&mut self) -> Result<()> {
        let Some(selection) = self.selection.filter(|selection| selection.keyboard) else {
            return self.notify("no history selection; press v to start selecting");
        };
        let text = selected_text(
            &self.windows[self.active],
            selection,
            self.active_content_size(),
        );
        if !text.is_empty() {
            self.copy_to_clipboard(&text)?;
            self.clipboard_status_until = Some(Instant::now() + CLIPBOARD_STATUS_DURATION);
            self.renderer.set_border_status(Some(CLIPBOARD_STATUS));
        }
        let mode = self.config.default_mode.clone();
        self.switch_mode(&mode)
    }

    fn pane_resize_at(&self, position: MousePosition) -> Option<(usize, PaneResizeHandle)> {
        let base = render_base_index(&self.windows, self.active);
        let active = self.windows.get(base)?;
        if active.floating || active.zoomed || !active.pane_framed {
            return None;
        }
        let tab = self.tabs.iter().find(|tab| tab.id == active.tab_id)?;
        let canvas_position = mouse_canvas_position(position)?;
        let rect = tiled_content_rect_for(self.terminal_size, self.config.compact());
        pane_resize_handle(&tab.root, rect, canvas_position).map(|handle| (tab.id, handle))
    }

    fn update_mouse_drag(&mut self, action: MouseAction) -> Result<()> {
        let pointer_position = action.position();
        let drag = self.mouse_drag.clone();
        let finished = matches!(action, MouseAction::SelectEnd(_));
        if let (
            Some(MouseDrag::PaneResize { tab_id, handle }),
            MouseAction::SelectExtend(position) | MouseAction::SelectEnd(position),
        ) = (drag, action)
            && let Some(position) = mouse_canvas_position(position)
            && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id)
            && resize_pane_to(&mut tab.root, &handle, position)
        {
            self.selection = None;
            self.resize_windows()?;
            self.renderer.invalidate();
            self.redraw()?;
        }
        if finished {
            self.mouse_drag = None;
            self.update_pointer_for_position(pointer_position);
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
                keyboard: false,
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

        let selection_cleared = matches!(
            action,
            MouseAction::ScrollUp(_) | MouseAction::ScrollDown(_)
        ) && self.selection.take().is_some();
        let window = &mut self.windows[self.active];
        let current = window.terminal.screen().scrollback();
        let was_history_mode = window.history_mode;
        match action {
            MouseAction::ScrollUp(_) => {
                window.history_mode = true;
                if self.config.has_mode("scroll") {
                    self.mode = "scroll".to_owned();
                }
                window
                    .terminal
                    .screen_mut()
                    .set_scrollback(current.saturating_add(MOUSE_SCROLL_LINES));
            }
            MouseAction::ScrollDown(_) if window.history_mode => {
                window
                    .terminal
                    .screen_mut()
                    .set_scrollback(current.saturating_sub(MOUSE_SCROLL_LINES));
            }
            MouseAction::ScrollDown(_) => {
                if selection_cleared {
                    self.redraw()?;
                }
                return Ok(());
            }
            MouseAction::Other(_) => return Ok(()),
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
        self.content_position_for(self.active, position, clamp)
    }

    fn content_position_for(
        &self,
        index: usize,
        position: MousePosition,
        clamp: bool,
    ) -> Option<MousePosition> {
        let (origin_column, origin_row, columns, rows) = if self.windows[index].floating {
            let layout = floating_layout_for(self.terminal_size, self.config.compact());
            let (columns, rows) = layout.content_size();
            (layout.column + 1, layout.row + 1, columns, rows)
        } else {
            let window = &self.windows[index];
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
        let bell_was_pending = self.windows[index].bell_pending;
        let parsed = self.windows[index].kitty_graphics.process(output);
        let dnd = self.windows[index].kitty_dnd.process(&parsed.terminal);
        let mut terminal = Vec::new();
        for event in dnd.events {
            match event {
                KittyDndEvent::Terminal(bytes) => terminal.extend(bytes),
                KittyDndEvent::Command(command) => {
                    self.handle_window_dnd_command(index, &command)?;
                }
            }
        }
        let ipc = self.windows[index].kitty_ipc.process(&terminal);
        for command in ipc.commands {
            if let Some(command) = kitty_ipc_with_pane(&command, self.windows[index].id) {
                self.write_client_protocol(&command);
            }
        }
        let terminal_changed = !ipc.terminal.is_empty();
        if terminal_changed {
            self.windows[index].history_cache.invalidate();
        }
        let previous_keyboard_flags = self.windows[index].input_modes.keyboard_flags();
        let previous_rich_paste = self.windows[index].input_modes.rich_clipboard_paste();
        let mode_responses = self.windows[index].input_modes.process(&ipc.terminal);
        let alternate_screen = self.windows[index].input_modes.alternate_screen();
        self.windows[index]
            .terminal_osc
            .set_alternate_screen(alternate_screen);
        let osc_output = self.windows[index].terminal_osc.process(&ipc.terminal);
        self.windows[index].cursor_style.process(&ipc.terminal);
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
        let command_was_running = self.windows[index]
            .command_output
            .command_started_at
            .is_some();
        let completions = self.windows[index].command_output.process(&ipc.terminal);
        if self.windows[index].command_output.take_bell() {
            self.windows[index].bell_pending = true;
        }
        if !command_was_running
            && self.windows[index]
                .command_output
                .command_started_at
                .is_some()
        {
            self.windows[index].notification_applications.clear();
        }
        {
            let window = &mut self.windows[index];
            window
                .hyperlinks
                .process(&ipc.terminal, &mut window.terminal);
        }
        let screen = self.windows[index].terminal.screen();
        let (screen_rows, screen_columns) = screen.size();
        let cell_width = self.terminal_pixels.0 / self.terminal_size.0.max(1);
        let cell_height = self.terminal_pixels.1 / self.terminal_size.1.max(1);
        graphics_responses.extend_from_slice(&terminal_responses(
            &ipc.terminal,
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
        graphics_responses.extend_from_slice(&mode_responses);
        graphics_responses.extend_from_slice(&osc_output.responses);
        if !graphics_responses.is_empty() {
            write_fd(&self.windows[index].master, &graphics_responses)?;
        }
        if index == self.active
            && previous_keyboard_flags != self.windows[index].input_modes.keyboard_flags()
        {
            self.sync_keyboard_protocol();
        }
        if index == self.active
            && previous_rich_paste != self.windows[index].input_modes.rich_clipboard_paste()
        {
            self.sync_rich_paste_protocol();
        }
        if index == self.active && osc_output.pointer_changed {
            self.sync_pointer_protocol();
        }
        if let Some(seconds) = self.config.command_notification_seconds() {
            let threshold = Duration::from_secs(seconds);
            let excluded = self.config.notification_excludes_any_application(
                self.windows[index]
                    .notification_applications
                    .iter()
                    .map(String::as_str),
            );
            for &duration in &completions {
                if duration >= threshold && !excluded {
                    self.notify_command_finished(index, duration)?;
                }
            }
        }
        if !completions.is_empty() {
            self.windows[index].notification_applications.clear();
        }
        let bell_changed = bell_was_pending != self.windows[index].bell_pending;
        let base_tab = self.windows[render_base_index(&self.windows, self.active)].tab_id;
        let visible = index == self.active
            || (!self.windows[index].floating && self.windows[index].tab_id == base_tab);
        if bell_changed || (visible && (terminal_changed || graphics_changed)) {
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
        self.windows[index].bell_pending = true;
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
            self.windows[self.active].notification_applications.clear();
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
        let shell_name = crate::shell::resolve_shell(self.config.shell.as_deref())?;
        let name = Path::new(&shell_name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell_name)
            .to_owned();
        let shell = CString::new(shell_name)?;
        let current_directory = self.active_spawn_directory();
        let new_id = self.spawn_window(
            name,
            shell.clone(),
            vec![shell],
            SpawnOptions {
                temporary_file: None,
                return_to_window: None,
                floating: false,
                tab_id,
                current_directory,
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
        let target = self
            .windows
            .iter()
            .position(|window| window.id == new_id)
            .unwrap();
        self.set_active(target);
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn focus_next_pane(&mut self) -> Result<()> {
        if self.windows[self.active].floating || self.windows[self.active].zoomed {
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
            let target = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            self.set_active(target);
            self.selection = None;
            self.redraw()?;
        }
        Ok(())
    }

    fn focus_pane(&mut self, direction: Direction) -> Result<()> {
        if let Some(target) = self.pane_in_direction(direction) {
            let target = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            self.set_active(target);
            self.selection = None;
            self.redraw()?;
        }
        Ok(())
    }

    fn pane_in_direction(&self, direction: Direction) -> Option<usize> {
        let active = self.windows.get(self.active)?;
        if active.floating || active.zoomed {
            return None;
        }
        self.windows
            .iter()
            .filter(|candidate| {
                !candidate.floating
                    && candidate.tab_id == active.tab_id
                    && candidate.id != active.id
                    && rect_in_direction(active.pane_rect, candidate.pane_rect, direction)
            })
            .min_by_key(|candidate| {
                directional_distance(active.pane_rect, candidate.pane_rect, direction)
            })
            .map(|window| window.id)
    }

    fn move_active_pane(&mut self, direction: Direction) -> Result<()> {
        let Some(target) = self.pane_in_direction(direction) else {
            return self.notify("no pane in that direction");
        };
        let active_id = self.windows[self.active].id;
        let tab_id = self.windows[self.active].tab_id;
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return Ok(());
        };
        if swap_panes(&mut tab.root, active_id, target) {
            self.selection = None;
            self.resize_windows()?;
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn resize_active_pane(&mut self, direction: Direction) -> Result<()> {
        if self.windows[self.active].floating || self.windows[self.active].zoomed {
            return Ok(());
        }
        let tab_id = self.windows[self.active].tab_id;
        let pane_id = self.windows[self.active].id;
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return Ok(());
        };
        if !resize_pane(&mut tab.root, pane_id, direction) {
            return self.notify("no pane boundary in that direction");
        }
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn toggle_pane_zoom(&mut self) -> Result<()> {
        if self.windows[self.active].floating {
            return Ok(());
        }
        let tab_id = self.windows[self.active].tab_id;
        let zoom = !self.windows[self.active].zoomed;
        for window in &mut self.windows {
            if window.tab_id == tab_id {
                window.zoomed = false;
            }
        }
        self.windows[self.active].zoomed = zoom;
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
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
        self.send_focus_event(self.active, false);
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
        self.sync_keyboard_protocol();
        self.send_focus_event(self.active, true);
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
            } else if self.windows[index].zoomed {
                let rect = content_rect_for(self.terminal_size, self.config.compact());
                self.windows[index].pane_rect = rect;
                self.windows[index].pane_framed = false;
                pane_pty_size(rect, false)
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
            let target = self
                .windows
                .iter()
                .position(|window| window.id == target)
                .unwrap();
            self.set_active(target);
            self.selection = None;
            self.renderer.invalidate();
            self.redraw()?;
        }
        Ok(())
    }

    fn move_active_window(&mut self, offset: isize) -> Result<()> {
        let tab_id = self.windows[render_base_index(&self.windows, self.active)].tab_id;
        let current = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .unwrap_or(0);
        if !move_item(&mut self.tabs, current, offset) {
            return self.notify("window is already at the edge");
        }

        let active_id = self.windows[self.active].id;
        let positions = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| (tab.id, index))
            .collect::<HashMap<_, _>>();
        self.windows
            .sort_by_key(|window| positions.get(&window.tab_id).copied().unwrap_or(usize::MAX));
        self.active = self
            .windows
            .iter()
            .position(|window| window.id == active_id)
            .unwrap();
        self.selection = None;
        self.renderer.invalidate();
        self.redraw()
    }

    fn select_window(&mut self, index: usize) -> Result<()> {
        if let Some(tab) = self.tabs.get(index.saturating_sub(1)) {
            self.select_tab_id(tab.id)?;
        }
        Ok(())
    }

    fn select_tab_id(&mut self, tab_id: usize) -> Result<()> {
        let base = render_base_index(&self.windows, self.active);
        if self.windows[base].tab_id == tab_id {
            return Ok(());
        }
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return Ok(());
        };
        let target = pane_ids(&tab.root)[0];
        if self.windows[self.active].history_mode {
            self.windows[self.active].history_mode = false;
            self.windows[self.active]
                .terminal
                .screen_mut()
                .set_scrollback(0);
            self.history_search = None;
            self.renderer.set_history_search_prompt(None);
            self.mode = self.config.default_mode.clone();
        }
        let target = self
            .windows
            .iter()
            .position(|window| window.id == target)
            .expect("tab layout references an existing pane");
        self.set_active(target);
        self.selection = None;
        self.renderer.invalidate();
        self.redraw()
    }

    fn close_active(&mut self) -> Result<()> {
        if self.windows.is_empty() {
            return Ok(());
        }
        if self.windows[self.active].floating {
            self.send_focus_event(self.active, false);
            let window = self.windows.remove(self.active);
            let return_to = window.return_to_window;
            terminate_window(window);
            self.active = return_to
                .and_then(|id| self.windows.iter().position(|window| window.id == id))
                .unwrap_or(0);
            self.sync_keyboard_protocol();
            self.send_focus_event(self.active, true);
            self.renderer.invalidate();
            return self.redraw();
        }
        let tab_id = self.windows[self.active].tab_id;
        self.send_focus_event(self.active, false);
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
        self.sync_keyboard_protocol();
        self.send_focus_event(self.active, true);
        self.selection = None;
        self.resize_windows()?;
        self.renderer.invalidate();
        self.redraw()
    }

    fn redraw(&mut self) -> Result<()> {
        self.redraw_deadline = None;
        if let Some(window) = self.windows.get_mut(self.active) {
            window.bell_pending = false;
        }
        if self.windows.is_empty() || self.client.is_none() {
            return Ok(());
        }
        self.sync_kitty_dnd_registration();
        if self.client.is_none() {
            return Ok(());
        }
        self.renderer.set_theme(self.config.theme);
        self.renderer.set_ui(
            self.config.compact(),
            self.config.describe_status_mode(&self.mode),
        );
        // Cache only commands already sent. Replay that state before output
        // queued while detached, including continuations of chunked uploads.
        let window = &mut self.windows[self.active];
        let pending = std::mem::take(&mut window.pending_graphics);
        let mut graphics = if std::mem::take(&mut window.graphics_replay_pending) {
            window.graphics_cache.replay()
        } else {
            Vec::new()
        };
        for command in &pending {
            window.graphics_cache.record(command);
        }
        graphics.extend(pending);
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
            .notification_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.notification_until = None;
            self.renderer.set_notification(None);
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
        let deadline = [
            self.redraw_deadline,
            self.clipboard_status_until,
            self.notification_until,
            self.ipc_input.flush_deadline(),
            self.dnd_input.flush_deadline(),
            self.input_decoder.flush_deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(deadline) = deadline else {
            return 100;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return 0;
        }
        remaining.as_millis().clamp(1, 100) as u16
    }

    fn show_help(&mut self) -> Result<()> {
        self.help_mode = Some(self.mode.clone());
        self.renderer.set_help(Some(HelpView {
            mode: self.mode.clone(),
            hints: self.config.describe_help_mode(&self.mode),
        }));
        self.redraw()
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
                self.sync_keyboard_protocol();
                self.send_focus_event(self.active, true);
                self.selection = None;
                self.resize_windows()?;
                self.renderer.invalidate();
                self.redraw()?;
            }
        }
        Ok(())
    }

    pub(super) fn shutdown(&mut self) {
        self.autosave_session(true);
        self.flush_session_saves();
        self.clear_outer_dnd_registration();
        for window in self.windows.drain(..) {
            terminate_window(window);
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub(super) fn pane_at(windows: &[Window], active: usize, position: MousePosition) -> Option<usize> {
    let active_window = windows.get(active)?;
    if active_window.floating || active_window.zoomed || !active_window.pane_framed {
        return Some(active);
    }
    windows.iter().enumerate().find_map(|(index, window)| {
        if window.floating || window.tab_id != active_window.tab_id {
            return None;
        }
        let left = window.pane_rect.column.saturating_add(1);
        let top = window.pane_rect.row.saturating_add(2);
        let right = left.saturating_add(window.pane_rect.width);
        let bottom = top.saturating_add(window.pane_rect.height);
        (position.column >= left
            && position.column < right
            && position.row >= top
            && position.row < bottom)
            .then_some(index)
    })
}

fn mouse_canvas_position(position: MousePosition) -> Option<(u16, u16)> {
    Some((
        position.column.checked_sub(1)?,
        position.row.checked_sub(2)?,
    ))
}

pub(super) fn window_history(window: &mut Window) -> String {
    let mut lines = history_lines(window);
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut history = lines.join("\n");
    history.push('\n');
    history
}

pub(super) fn history_lines(window: &mut Window) -> Vec<String> {
    screen_history_lines(window.terminal.screen_mut())
}

fn screen_history_lines(screen: &mut vt100::Screen) -> Vec<String> {
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

    lines
}

#[cfg(any(test, feature = "benchmarks"))]
pub(super) fn snapshot_scrollback(
    screen: &vt100::Screen,
    limit: usize,
    colored: bool,
) -> Vec<String> {
    snapshot_owned_scrollback(screen.clone(), limit, colored)
}

pub(super) fn snapshot_owned_scrollback(
    screen: vt100::Screen,
    limit: usize,
    colored: bool,
) -> Vec<String> {
    // The save worker owns this frozen screen; never mutate the live terminal.
    let mut parser = vt100::Parser::new(1, 1, 0);
    *parser.screen_mut() = screen;
    // Select the primary buffer without resetting it or replaying application output.
    parser.process(b"\x1b[?47l");
    let screen = parser.screen_mut();
    let mut lines = if colored {
        screen.set_scrollback(usize::MAX);
        let history_rows = screen.scrollback();
        let mut lines = Vec::new();
        for offset in (1..=history_rows).rev() {
            screen.set_scrollback(offset);
            lines.push(crate::scrollback::colored_row(screen, 0));
        }
        screen.set_scrollback(0);
        for row in 0..screen.size().0 {
            lines.push(crate::scrollback::colored_row(screen, row));
        }
        lines
    } else {
        screen_history_lines(screen)
    };
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.drain(..lines.len().saturating_sub(limit));
    lines
}

pub(super) fn restore_scrollback(
    terminal: &mut vt100::Parser<TerminalMetadata>,
    lines: &[String],
    limit: usize,
    format: ScrollbackFormat,
) {
    if lines.is_empty() {
        return;
    }
    for line in &lines[lines.len().saturating_sub(limit)..] {
        // Snapshot text is data, including when a user edits the TOML file.
        let text = crate::scrollback::sanitize_saved_line(line, format == ScrollbackFormat::Ansi);
        terminal.process(text.as_bytes());
        terminal.process(b"\x1b[0m");
        terminal.process(b"\r\n");
    }
    // Move restored text entirely into scrollback, leaving a fresh live screen.
    for _ in 1..terminal.screen().size().0 {
        terminal.process(b"\r\n");
    }
    terminal.process(b"\x1b[H");
}

pub(super) fn matching_history_lines(lines: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let query = query.to_lowercase();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.to_lowercase().contains(&query).then_some(index))
        .collect()
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
    if !selection.is_visible() {
        return false;
    }
    let position = usize::from(row) * usize::from(columns) + usize::from(column);
    let start = usize::from(selection.start.row) * usize::from(columns)
        + usize::from(selection.start.column);
    let end =
        usize::from(selection.end.row) * usize::from(columns) + usize::from(selection.end.column);
    (start.min(end)..=start.max(end)).contains(&position)
}

fn signal_window_processes(foreground: Option<Pid>, child: Pid, signal: Signal) {
    if let Some(foreground) = foreground {
        let _ = kill(Pid::from_raw(-foreground.as_raw()), signal);
    }
    let _ = kill(Pid::from_raw(-child.as_raw()), signal);
    let _ = kill(child, signal);
}

pub(super) fn preferred_spawn_directory(
    tracked: Option<PathBuf>,
    foreground_application: Option<&str>,
    foreground_directory: Option<PathBuf>,
) -> Option<PathBuf> {
    if foreground_application.is_some_and(|name| name.eq_ignore_ascii_case("yazi")) {
        foreground_directory.or(tracked)
    } else {
        tracked.or(foreground_directory)
    }
}

#[cfg(target_os = "macos")]
pub(super) fn process_current_directory(pid: Pid) -> Option<PathBuf> {
    // SAFETY: proc_vnodepathinfo is a plain C data structure that may be zero-initialized.
    let mut info = unsafe { std::mem::zeroed::<nix::libc::proc_vnodepathinfo>() };
    let size = std::mem::size_of_val(&info);
    // SAFETY: proc_pidinfo writes at most `size` bytes to the valid `info` buffer
    // and does not retain its pointer.
    let length = unsafe {
        nix::libc::proc_pidinfo(
            pid.as_raw(),
            nix::libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut nix::libc::proc_vnodepathinfo).cast(),
            size.try_into().ok()?,
        )
    };
    if usize::try_from(length).ok()? < size {
        return None;
    }
    let path = info
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .map(|byte| *byte as u8)
        .take_while(|byte| *byte != 0)
        .collect::<Vec<_>>();
    (!path.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(path)))
}

#[cfg(target_os = "linux")]
pub(super) fn process_current_directory(pid: Pid) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{}/cwd", pid.as_raw())).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn process_current_directory(_: Pid) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
pub(super) fn process_name(pid: Pid) -> Option<String> {
    let mut buffer = [0_u8; 256];
    // SAFETY: proc_name only writes up to the provided buffer size and does not
    // retain the pointer after returning.
    let length = unsafe {
        nix::libc::proc_name(
            pid.as_raw(),
            buffer.as_mut_ptr().cast(),
            buffer.len().try_into().ok()?,
        )
    };
    if length <= 0 {
        return None;
    }
    let length = usize::try_from(length).ok()?.min(buffer.len());
    let end = buffer[..length]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(length);
    (!buffer[..end].is_empty()).then(|| String::from_utf8_lossy(&buffer[..end]).into_owned())
}

#[cfg(target_os = "linux")]
pub(super) fn process_name(pid: Pid) -> Option<String> {
    let name = fs::read_to_string(format!("/proc/{}/comm", pid.as_raw())).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn process_name(_: Pid) -> Option<String> {
    None
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
    let child = window.child;
    let foreground = tcgetpgrp(&window.master).ok();
    let temporary_file = window.temporary_file.clone();
    signal_window_processes(foreground, child, Signal::SIGHUP);
    drop(window);

    // Waiting for an interactive shell to acknowledge SIGHUP can take hundreds
    // of milliseconds. Keep that work off the input/render loop so closing a
    // window updates the UI immediately while the child is still reaped safely.
    let _ = thread::Builder::new()
        .name("rustmux-window-reaper".to_owned())
        .spawn(move || {
            if !wait_for_child(child, Duration::from_millis(200)) {
                signal_window_processes(foreground, child, Signal::SIGTERM);
                if !wait_for_child(child, Duration::from_millis(200)) {
                    signal_window_processes(foreground, child, Signal::SIGKILL);
                    let _ = wait_for_child(child, Duration::from_secs(1));
                }
            }
            if let Some(path) = temporary_file {
                let _ = fs::remove_file(path);
            }
        });
}

fn write_control_input(fd: &OwnedFd, mut bytes: &[u8]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(250);
    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err("pane input buffer is full; some input may have been sent".into());
        }
        match write(fd, bytes) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
            Ok(count) => bytes = &bytes[count..],
            Err(Errno::EINTR) => {}
            Err(Errno::EAGAIN) => {
                let mut fds = [PollFd::new(fd.as_fd(), PollFlags::POLLOUT)];
                poll(&mut fds, 10_u16)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
    let (columns, rows) = terminal_parser_size(columns, rows);
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
    if window.terminal.screen().size() != (rows, columns) {
        window.history_cache.invalidate();
    }
    window.terminal.screen_mut().set_size(rows, columns);
    window.hyperlinks.resize(rows, columns);
    Ok(())
}
