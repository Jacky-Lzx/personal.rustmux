use std::env;
use std::fs;
use std::io::{self, Read, Write};
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

use super::{
    CLIENT_DISCONNECT, CLIENT_INPUT, CLIENT_QUERY_STATUS, CLIENT_RENAME_SESSION, CLIENT_RESIZE,
    CLIENT_SHUTDOWN, Config, EARLY_DISCONNECT_RETRY, Result, SERVER_SWITCH_SESSION_PREFIX,
    TerminalGuard,
};
use crate::app::App;
use crate::layout::validate_terminal_size;

struct SocketGuard(PathBuf);

#[derive(Debug, Eq, PartialEq)]
enum ClientExit {
    Disconnected,
    SwitchSession(String),
}

#[derive(Default)]
pub(super) struct ServerOutputDecoder {
    pending: Vec<u8>,
}

impl ServerOutputDecoder {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(Vec<u8>, Option<String>)> {
        self.pending.extend_from_slice(bytes);
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

        let retained = (1..SERVER_SWITCH_SESSION_PREFIX.len())
            .rev()
            .find(|length| {
                self.pending
                    .ends_with(&SERVER_SWITCH_SESSION_PREFIX[..*length])
            })
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
                start_server(&socket, size)?;
                connect_with_retry(&socket)?
            }
        };

        let started = Instant::now();
        if let ClientExit::SwitchSession(name) = run_client(stream)? {
            current_name = name;
            create_current = true;
            continue;
        }
        if started.elapsed() < EARLY_DISCONNECT_RETRY {
            // Servers started by an older rustmux build could accidentally apply a
            // previous client's POLLHUP to a newly accepted connection. Retrying
            // once preserves that in-memory session while recovering transparently.
            thread::sleep(Duration::from_millis(20));
            if let Ok(stream) = UnixStream::connect(&socket)
                && let ClientExit::SwitchSession(name) = run_client(stream)?
            {
                current_name = name;
                create_current = true;
                continue 'sessions;
            }
        }
        return Ok(());
    }
}

pub(super) fn run_server(socket: PathBuf, values: &[String]) -> Result<()> {
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
    let mut app = App::new(
        (columns, rows),
        (width, height),
        config,
        &session_name,
        socket.clone(),
    )?;
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

pub(super) fn available_session_info(local: Option<SessionInfo>) -> Result<Vec<SessionInfo>> {
    let mut sessions = Vec::new();
    for name in available_sessions()? {
        if let Some(info) = local.as_ref().filter(|info| info.name == name) {
            sessions.push(info.clone());
            continue;
        }
        let response = control_session(&name, &[CLIENT_QUERY_STATUS]).unwrap_or_default();
        let fields = String::from_utf8_lossy(&response)
            .trim()
            .split('\t')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        sessions.push(SessionInfo {
            name,
            tabs: fields
                .first()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            panes: fields
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            connected: fields.get(2).is_some_and(|value| value == "1"),
            created_at: fields
                .get(3)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        });
    }
    Ok(sessions)
}

pub(super) fn disconnect_session(name: &str) -> Result<()> {
    control_session(name, &[CLIENT_DISCONNECT]).map(|_| ())
}

pub(super) fn rename_session(name: &str, new_name: &str) -> Result<()> {
    validate_session_name(new_name)?;
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
    control_session(name, &[CLIENT_SHUTDOWN]).map(|_| ())
}
