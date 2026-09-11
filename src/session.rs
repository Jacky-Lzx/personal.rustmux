use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::terminal::window_size;
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use nix::unistd::read;
use serde::{Deserialize, Serialize};

use super::{
    CLIENT_DISCONNECT, CLIENT_INPUT, CLIENT_QUERY_STATUS, CLIENT_RENAME_SESSION, CLIENT_RESIZE,
    CLIENT_SHUTDOWN, Config, EARLY_DISCONNECT_RETRY, Result, SERVER_SWITCH_SESSION_PREFIX,
    TerminalGuard,
};
use crate::app::App;
use crate::layout::{PaneNode, validate_terminal_size};

const SNAPSHOT_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SessionSnapshot {
    pub(super) version: u32,
    pub(super) tabs: Vec<SnapshotTab>,
    pub(super) floating: Option<SnapshotFloating>,
    pub(super) active_pane: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SnapshotTab {
    pub(super) id: usize,
    pub(super) name: String,
    pub(super) root: PaneNode,
    pub(super) panes: Vec<SnapshotPane>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SnapshotPane {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) command: Option<String>,
    pub(super) id: usize,
    pub(super) cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct SnapshotFloating {
    pub(super) cwd: Option<PathBuf>,
    pub(super) visible: bool,
    pub(super) return_to: Option<usize>,
}

impl SessionSnapshot {
    pub(super) fn new(
        tabs: Vec<SnapshotTab>,
        floating: Option<SnapshotFloating>,
        active_pane: usize,
    ) -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            tabs,
            floating,
            active_pane,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.version != SNAPSHOT_VERSION {
            return Err(format!("unsupported session snapshot version {}", self.version).into());
        }
        if self.tabs.is_empty() {
            return Err("session snapshot has no tabs".into());
        }
        let mut all_panes = Vec::new();
        for tab in &self.tabs {
            let mut root_ids = crate::layout::pane_ids(&tab.root);
            let mut pane_ids = tab.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
            root_ids.sort_unstable();
            pane_ids.sort_unstable();
            if root_ids != pane_ids || pane_ids.iter().any(|id| all_panes.contains(id)) {
                return Err(format!("invalid pane layout in saved tab '{}'", tab.name).into());
            }
            all_panes.extend(pane_ids);
        }
        if !all_panes.contains(&self.active_pane) {
            return Err("saved active pane does not exist".into());
        }
        Ok(())
    }
}

struct SocketGuard(PathBuf);

#[derive(Debug, Eq, PartialEq)]
enum ClientExit {
    Disconnected,
    Busy,
    SwitchSession(String),
}

#[derive(Default)]
pub(super) struct ServerOutputDecoder {
    pending: Vec<u8>,
    pub(super) busy: bool,
}

impl ServerOutputDecoder {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(Vec<u8>, Option<String>)> {
        self.pending.extend_from_slice(bytes);
        if let Some(start) = find_bytes(&self.pending, crate::SERVER_SESSION_BUSY) {
            let visible = self.pending.drain(..start).collect();
            self.pending.drain(..crate::SERVER_SESSION_BUSY.len());
            self.busy = true;
            return Ok((visible, None));
        }
        if let Some(start) = find_bytes(&self.pending, SERVER_SWITCH_SESSION_PREFIX) {
            let name_start = start + SERVER_SWITCH_SESSION_PREFIX.len();
            let Some(end_offset) = self.pending[name_start..]
                .iter()
                .position(|byte| *byte == b'\x07')
            else {
                return Ok((self.pending.drain(..start).collect(), None));
            };
            let end = name_start + end_offset;
            let name = std::str::from_utf8(&self.pending[name_start..end])?.to_owned();
            validate_session_name(&name)?;
            let visible = self.pending.drain(..start).collect();
            self.pending.drain(..=end - start);
            return Ok((visible, Some(name)));
        }

        let retained = [SERVER_SWITCH_SESSION_PREFIX, crate::SERVER_SESSION_BUSY]
            .into_iter()
            .flat_map(|prefix| (1..prefix.len()).map(move |length| &prefix[..length]))
            .filter(|prefix| self.pending.ends_with(prefix))
            .map(<[u8]>::len)
            .max()
            .unwrap_or(0);
        let visible_length = self.pending.len().saturating_sub(retained);
        Ok((self.pending.drain(..visible_length).collect(), None))
    }

    fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn session_dir() -> PathBuf {
    let uid = unsafe { nix::libc::getuid() };
    env::temp_dir().join(format!("rustmux-{uid}"))
}

fn snapshot_dir() -> PathBuf {
    let state = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    state.join("rustmux/sessions")
}

fn snapshot_path(name: &str) -> Result<PathBuf> {
    validate_session_name(name)?;
    Ok(snapshot_dir().join(format!("{name}.toml")))
}

pub(super) fn save_session_snapshot(name: &str, snapshot: &SessionSnapshot) -> Result<()> {
    snapshot.validate()?;
    let directory = snapshot_dir();
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let path = snapshot_path(name)?;
    let temporary = directory.join(format!(".{name}.toml.tmp-{}", std::process::id()));
    fs::write(&temporary, toml::to_string_pretty(snapshot)?)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub(super) fn load_session_snapshot(name: &str) -> Result<Option<SessionSnapshot>> {
    let path = snapshot_path(name)?;
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let snapshot: SessionSnapshot = toml::from_str(&source)?;
    snapshot.validate()?;
    Ok(Some(snapshot))
}

pub(super) fn delete_session_snapshot(name: &str) -> Result<()> {
    let path = snapshot_path(name)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn available_snapshots() -> Result<Vec<String>> {
    let directory = snapshot_dir();
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut names = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "toml")
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
    Ok(names)
}

pub(super) fn rename_session_snapshot(old_name: &str, new_name: &str) -> Result<()> {
    let old_path = snapshot_path(old_name)?;
    if !old_path.exists() {
        return Ok(());
    }
    let new_path = snapshot_path(new_name)?;
    if new_path.exists() {
        return Err(format!("saved session '{new_name}' already exists").into());
    }
    fs::rename(old_path, new_path)?;
    Ok(())
}

pub(super) fn ensure_session_dir() -> Result<PathBuf> {
    let directory = session_dir();
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}

pub(super) fn validate_session_name(name: &str) -> Result<()> {
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

pub(super) fn session_socket(name: &str) -> Result<PathBuf> {
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

fn run_client(mut stream: UnixStream) -> Result<ClientExit> {
    let mut size = window_size()?;
    send_resize(&mut stream, &size)?;
    let _terminal = TerminalGuard::enter()?;
    let stdin = io::stdin();
    let mut input = [0_u8; 4096];
    let mut output = [0_u8; 64 * 1024];
    let mut output_decoder = ServerOutputDecoder::default();

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
                    let (visible, switch) = output_decoder.push(&output[..count])?;
                    if output_decoder.busy {
                        return Ok(ClientExit::Busy);
                    }
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(&visible)?;
                    stdout.flush()?;
                    if let Some(name) = switch {
                        return Ok(ClientExit::SwitchSession(name));
                    }
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
    let remaining = output_decoder.finish();
    if !remaining.is_empty() {
        let mut stdout = io::stdout().lock();
        stdout.write_all(&remaining)?;
        stdout.flush()?;
    }
    Ok(ClientExit::Disconnected)
}

fn start_server(
    socket: &Path,
    size: crossterm::terminal::WindowSize,
    layout: Option<&Path>,
) -> Result<()> {
    let config = Config::load()?;
    crate::shell::resolve_shell(config.shell.as_deref())?;
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
        .env(super::RUSTMUX_ENV, socket)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(layout) = layout {
        command.arg("--startup-layout").arg(layout);
    }
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let mut ready = String::new();
    io::BufReader::new(
        child
            .stdout
            .take()
            .ok_or("missing server startup channel")?,
    )
    .read_line(&mut ready)?;
    if ready.trim() != "READY" {
        let output = child.wait_with_output()?;
        return Err(format!(
            "session failed to start: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
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

pub(super) fn new_session(name: &str, detached: bool, layout: Option<&Path>) -> Result<()> {
    if !detached && layout.is_none() {
        return attach_or_create(name, true);
    }
    let socket = session_socket(name)?;
    if let Ok(mut stream) = UnixStream::connect(&socket) {
        stream.write_all(&[CLIENT_QUERY_STATUS])?;
        if layout.is_some() {
            return Err(format!(
                "session '{name}' is already running; use a new name for the layout"
            )
            .into());
        }
        return if detached {
            Ok(())
        } else {
            attach_or_create(name, false)
        };
    }
    let layout = layout.map(fs::canonicalize).transpose()?;
    if let Some(layout) = &layout {
        crate::project::load(layout)?;
    }
    let size = if detached {
        crossterm::terminal::WindowSize {
            columns: 120,
            rows: 40,
            width: 0,
            height: 0,
        }
    } else {
        window_size()?
    };
    start_server(&socket, size, layout.as_deref())?;
    let mut stream = connect_with_retry(&socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&[CLIENT_QUERY_STATUS])?;
    let mut status = String::new();
    stream.read_to_string(&mut status)?;
    if status.is_empty() {
        return Err("session failed to start".into());
    }
    if detached {
        Ok(())
    } else {
        attach_or_create(name, false)
    }
}

fn warn_session_busy(name: &str) {
    eprintln!(
        "warning: session '{name}' is already attached to another client; detach that client before attaching here"
    );
}

pub(super) fn attach_or_create(name: &str, create: bool) -> Result<()> {
    Config::load().map_err(|error| format!("configuration error: {error}"))?;
    let mut current_name = name.to_owned();
    let mut create_current = create;
    'sessions: loop {
        let socket = session_socket(&current_name)?;
        let stream = match UnixStream::connect(&socket) {
            Ok(stream) => stream,
            Err(error) if !create_current => {
                return Err(format!("session '{current_name}' not found: {error}").into());
            }
            Err(_) => {
                let size = window_size()?;
                start_server(&socket, size, None)?;
                connect_with_retry(&socket)?
            }
        };

        let started = Instant::now();
        match run_client(stream)? {
            ClientExit::SwitchSession(name) => {
                current_name = name;
                create_current = true;
                continue;
            }
            ClientExit::Busy => {
                warn_session_busy(&current_name);
                return Ok(());
            }
            ClientExit::Disconnected => {}
        }
        if started.elapsed() < EARLY_DISCONNECT_RETRY {
            // Servers started by an older rustmux build could accidentally apply a
            // previous client's POLLHUP to a newly accepted connection. Retrying
            // once preserves that in-memory session while recovering transparently.
            thread::sleep(Duration::from_millis(20));
            if let Ok(stream) = UnixStream::connect(&socket) {
                match run_client(stream)? {
                    ClientExit::SwitchSession(name) => {
                        current_name = name;
                        create_current = true;
                        continue 'sessions;
                    }
                    ClientExit::Busy => {
                        warn_session_busy(&current_name);
                        return Ok(());
                    }
                    ClientExit::Disconnected => {}
                }
            }
        }
        return Ok(());
    }
}

pub(super) fn run_server(socket: PathBuf, values: &[String], layout: Option<&Path>) -> Result<()> {
    if values.len() != 4 {
        return Err("invalid server arguments".into());
    }
    let columns = values[0].parse()?;
    let rows = values[1].parse()?;
    let width = values[2].parse()?;
    let height = values[3].parse()?;
    let session_name = socket
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("session socket does not have a valid UTF-8 name")?
        .to_owned();
    validate_session_name(&session_name)?;
    validate_terminal_size((columns, rows))?;
    ensure_session_dir()?;
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let _socket_guard = SocketGuard(socket.clone());
    let config = Config::load().map_err(|error| format!("configuration error: {error}"))?;
    let layout = layout.map(crate::project::load).transpose()?;
    let mut app = App::new(
        (columns, rows),
        (width, height),
        config,
        &session_name,
        socket.clone(),
        layout,
    )?;
    println!("READY");
    io::stdout().flush()?;
    let result = app.run_server(listener);
    app.shutdown();
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionInfo {
    pub(super) name: String,
    pub(super) tabs: usize,
    pub(super) panes: usize,
    pub(super) connected: bool,
    pub(super) created_at: u64,
    pub(super) saved: bool,
}

fn control_session(name: &str, request: &[u8]) -> Result<Vec<u8>> {
    let socket = session_socket(name)?;
    let mut stream = UnixStream::connect(&socket)
        .map_err(|error| format!("session '{name}' not found: {error}"))?;
    stream.write_all(request)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(response)
}

fn send_control_session(name: &str, request: &[u8]) -> Result<()> {
    let socket = session_socket(name)?;
    let stream = UnixStream::connect(&socket)
        .map_err(|error| format!("session '{name}' not found: {error}"))?;
    send_control_request(stream, request)
}

fn send_control_request(mut stream: UnixStream, request: &[u8]) -> Result<()> {
    stream.write_all(request)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(())
}

pub(super) fn available_session_info(local: Option<SessionInfo>) -> Result<Vec<SessionInfo>> {
    let mut sessions = Vec::new();
    let live = available_sessions()?;
    let saved = available_snapshots()?;
    let mut names = live.iter().chain(&saved).cloned().collect::<Vec<_>>();
    names.sort();
    names.dedup();
    for name in names {
        if let Some(info) = local.as_ref().filter(|info| info.name == name) {
            sessions.push(info.clone());
            continue;
        }
        let is_live = live.contains(&name);
        let response = if is_live {
            control_session(&name, &[CLIENT_QUERY_STATUS]).unwrap_or_default()
        } else {
            Vec::new()
        };
        let fields = String::from_utf8_lossy(&response)
            .trim()
            .split('\t')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let snapshot = load_session_snapshot(&name).ok().flatten();
        let is_saved = saved.contains(&name);
        sessions.push(SessionInfo {
            name,
            tabs: fields
                .first()
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| snapshot.as_ref().map_or(0, |value| value.tabs.len())),
            panes: fields
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| {
                    snapshot.as_ref().map_or(0, |value| {
                        value.tabs.iter().map(|tab| tab.panes.len()).sum()
                    })
                }),
            connected: fields.get(2).is_some_and(|value| value == "1"),
            created_at: fields
                .get(3)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            saved: is_saved,
        });
    }
    Ok(sessions)
}

pub(super) fn disconnect_session(name: &str) -> Result<()> {
    control_session(name, &[CLIENT_DISCONNECT]).map(|_| ())
}

pub(super) fn rename_session(name: &str, new_name: &str) -> Result<()> {
    validate_session_name(new_name)?;
    if !session_socket(name)?.exists() {
        return rename_session_snapshot(name, new_name);
    }
    let length = u8::try_from(new_name.len())?;
    let mut request = vec![CLIENT_RENAME_SESSION, length];
    request.extend_from_slice(new_name.as_bytes());
    let response = control_session(name, &request)?;
    if response == b"OK" {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&response).into_owned().into())
    }
}

pub(super) fn list_sessions() -> Result<()> {
    let names = available_sessions()?;
    if names.is_empty() {
        println!("no sessions");
    } else {
        for name in names {
            println!("{name}");
        }
    }
    Ok(())
}

pub(super) fn available_sessions() -> Result<Vec<String>> {
    let directory = session_dir();
    if !directory.exists() {
        return Ok(Vec::new());
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
    Ok(names)
}

pub(super) fn kill_session(name: &str) -> Result<()> {
    send_control_session(name, &[CLIENT_SHUTDOWN])
}

pub(super) fn delete_session(name: &str) -> Result<()> {
    if session_socket(name)?.exists() {
        send_control_session(name, &[crate::CLIENT_DELETE_SESSION])
    } else {
        delete_session_snapshot(name)
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::layout::SplitAxis;

    #[test]
    fn shutdown_request_does_not_wait_for_a_response() {
        let (client, mut server) = UnixStream::pair().unwrap();

        send_control_request(client, &[CLIENT_SHUTDOWN]).unwrap();

        let mut request = Vec::new();
        server.read_to_end(&mut request).unwrap();
        assert_eq!(request, [CLIENT_SHUTDOWN]);
    }

    #[test]
    fn session_snapshot_round_trips_through_toml() {
        let snapshot = SessionSnapshot::new(
            vec![SnapshotTab {
                id: 7,
                name: "editor".to_owned(),
                root: PaneNode::Split {
                    axis: SplitAxis::Vertical,
                    ratio: 650,
                    first: Box::new(PaneNode::Leaf(10)),
                    second: Box::new(PaneNode::Leaf(11)),
                },
                panes: vec![
                    SnapshotPane {
                        command: None,
                        id: 10,
                        cwd: Some(PathBuf::from("/tmp/project")),
                    },
                    SnapshotPane {
                        id: 11,
                        cwd: None,
                        command: None,
                    },
                ],
            }],
            Some(SnapshotFloating {
                cwd: Some(PathBuf::from("/tmp")),
                visible: true,
                return_to: Some(11),
            }),
            11,
        );

        let encoded = toml::to_string_pretty(&snapshot).unwrap();
        let decoded: SessionSnapshot = toml::from_str(&encoded).unwrap();

        assert_eq!(decoded, snapshot);
        decoded.validate().unwrap();
    }

    #[test]
    fn session_snapshot_rejects_layouts_with_missing_panes() {
        let snapshot = SessionSnapshot::new(
            vec![SnapshotTab {
                id: 1,
                name: "broken".to_owned(),
                root: PaneNode::Leaf(1),
                panes: vec![SnapshotPane {
                    id: 2,
                    cwd: None,
                    command: None,
                }],
            }],
            None,
            2,
        );

        assert!(snapshot.validate().is_err());
    }
}
