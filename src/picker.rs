use std::io::{self, Write};
use std::ops::ControlFlow;
use std::os::fd::AsFd;

use crossterm::terminal::window_size;
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use nix::unistd::read;

use crate::app::{default_session_selection, matching_session_info};
use crate::config::Config;
use crate::input::{DecodedKey, InputDecoder, decode_key};
use crate::render::{SessionManagerView, render_session_picker};
use crate::session::{SessionInfo, available_session_info, validate_session_name};
use crate::{Result, TerminalGuard};

pub(super) fn choose_session(sessions: Vec<SessionInfo>) -> Result<Option<String>> {
    let config = Config::load()?;
    let mut keys = config.session_manager_keys();
    // Only advertise actions available before attaching to a server.
    keys.retain(|action, _| {
        matches!(
            action.as_str(),
            "up" | "down" | "search" | "create" | "complete" | "open" | "cancel" | "backspace"
        )
    });
    let mut view = SessionManagerView {
        searching: false,
        keys,
        query: String::new(),
        selected: default_session_selection(&sessions, ""),
        sessions: sessions.clone(),
        current: String::new(),
        rename_input: None,
        create_input: None,
    };
    let _terminal = TerminalGuard::enter()?;
    let stdin = io::stdin();
    let mut decoder = InputDecoder::default();
    let mut input = [0; 4096];
    let mut previous_size = (0, 0);
    let mut dirty = true;
    let mut error = None;
    loop {
        let size = window_size()?;
        let size = (size.columns, size.rows);
        if dirty || size != previous_size {
            let mut stdout = io::stdout().lock();
            stdout.write_all(&render_session_picker(
                &config.theme,
                size,
                &view,
                error.as_deref(),
            ))?;
            stdout.flush()?;
            previous_size = size;
            dirty = false;
        }
        let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, 20_u16) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
        }
        let event = fds[0].revents().unwrap_or_else(PollFlags::empty);
        let mut bytes = decoder.flush_if_expired();
        if event.contains(PollFlags::POLLIN) {
            let count = read(stdin.as_fd(), &mut input)?;
            if count == 0 {
                return Ok(None);
            }
            bytes.extend(decoder.push(&input[..count]));
        } else if event.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
            return Ok(None);
        }
        let mut offset = 0;
        while offset < bytes.len() {
            let (key, consumed) = decode_key(&bytes[offset..]);
            offset += consumed;
            let creating = view.create_input.is_some();
            if let ControlFlow::Break(selected) =
                handle_key(&config, &sessions, &mut view, &mut error, &key)
            {
                // The picker can stay open while another process creates a
                // session. Recheck names before handing creation to the client.
                if creating
                    && let Some(name) = &selected
                    && available_session_info(None)?
                        .iter()
                        .any(|session| session.name == *name)
                {
                    error = Some(format!("session '{name}' already exists"));
                } else {
                    return Ok(selected);
                }
            }
            dirty = true;
        }
    }
}

fn handle_key(
    config: &Config,
    sessions: &[SessionInfo],
    view: &mut SessionManagerView,
    error: &mut Option<String>,
    key: &DecodedKey,
) -> ControlFlow<Option<String>> {
    if key.event_type == 3 {
        return ControlFlow::Continue(());
    }
    if key.name == "ctrl c" {
        return ControlFlow::Break(None);
    }
    *error = None;
    if let Some(name) = view.create_input.as_mut() {
        match config.session_manager_action(&key.name, true).unwrap_or("") {
            "cancel" => view.create_input = None,
            "open" => {
                if name.is_empty() {
                    return ControlFlow::Continue(());
                }
                if let Err(invalid) = validate_session_name(name) {
                    *error = Some(invalid.to_string());
                } else if sessions.iter().any(|session| session.name == *name) {
                    *error = Some(format!("session '{name}' already exists"));
                } else {
                    return ControlFlow::Break(Some(name.clone()));
                }
            }
            "backspace" => {
                name.pop();
            }
            _ => {
                if let Some(text) = key.text()
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                    && name.len() + text.len() <= 64
                {
                    name.push_str(text);
                }
            }
        }
        return ControlFlow::Continue(());
    }
    let action = config
        .session_manager_action(&key.name, view.searching)
        .unwrap_or("");
    match action {
        "cancel" if view.searching => {
            view.searching = false;
            view.query.clear();
            view.sessions = sessions.to_vec();
            view.selected = default_session_selection(&view.sessions, "");
        }
        "cancel" => return ControlFlow::Break(None),
        "search" => view.searching = true,
        "create" => view.create_input = Some(String::new()),
        "up" if !view.sessions.is_empty() => {
            view.selected = view
                .selected
                .checked_sub(1)
                .unwrap_or(view.sessions.len() - 1);
        }
        "down" if !view.sessions.is_empty() => {
            view.selected = (view.selected + 1) % view.sessions.len()
        }
        "open" => {
            if let Some(session) = view.sessions.get(view.selected) {
                return ControlFlow::Break(Some(session.name.clone()));
            }
        }
        "complete" if view.searching => {
            if let Some(session) = view.sessions.get(view.selected) {
                view.query = session.name.clone();
            }
        }
        "backspace" if view.searching => {
            view.query.pop();
        }
        _ if view.searching => {
            if let Some(text) = key.text() {
                view.query.push_str(text);
            }
        }
        _ => {}
    }
    if view.searching {
        view.sessions = matching_session_info(sessions, &view.query);
        view.selected = view.selected.min(view.sessions.len().saturating_sub(1));
    }
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(config: &Config) -> SessionManagerView {
        SessionManagerView {
            searching: false,
            keys: config.session_manager_keys(),
            query: String::new(),
            sessions: vec![SessionInfo {
                name: "work".to_owned(),
                tabs: 1,
                panes: 1,
                connected: false,
                running: true,
                created_at: 1,
                last_connected_at: 1,
                saved: false,
            }],
            selected: 0,
            current: String::new(),
            rename_input: None,
            create_input: None,
        }
    }

    fn press(
        config: &Config,
        view: &mut SessionManagerView,
        bytes: &[u8],
    ) -> ControlFlow<Option<String>> {
        let sessions = vec![SessionInfo {
            name: "work".to_owned(),
            tabs: 1,
            panes: 1,
            connected: false,
            running: true,
            created_at: 1,
            last_connected_at: 1,
            saved: false,
        }];
        let mut offset = 0;
        while offset < bytes.len() {
            let (key, count) = decode_key(&bytes[offset..]);
            offset += count;
            let result = handle_key(config, &sessions, view, &mut None, &key);
            if result.is_break() {
                return result;
            }
        }
        ControlFlow::Continue(())
    }

    #[test]
    fn create_uses_its_own_name_and_search_never_creates() {
        let config = Config::test_defaults();
        let mut view = view(&config);
        assert!(press(&config, &mut view, b"/missing\ra").is_continue());
        assert_eq!(view.query, "missinga");
        assert!(view.create_input.is_none());
        assert!(press(&config, &mut view, b"\x1b").is_continue());
        assert!(press(&config, &mut view, b"a\r").is_continue());
        assert_eq!(view.create_input.as_deref(), Some(""));
        // Name entry accepts Kitty text, suppresses releases, and edits normally.
        assert_eq!(
            press(&config, &mut view, b"\x1b[97u\x1b[97;1:3unew\x7f\r"),
            ControlFlow::Break(Some("ane".to_owned()))
        );
    }

    #[test]
    fn duplicate_names_are_reported_and_escape_returns_to_picker() {
        let config = Config::test_defaults();
        let mut view = view(&config);
        assert!(press(&config, &mut view, b"awork").is_continue());
        let mut error = None;
        let sessions = view.sessions.clone();
        assert!(
            handle_key(
                &config,
                &sessions,
                &mut view,
                &mut error,
                &decode_key(b"\r").0
            )
            .is_continue()
        );
        assert_eq!(error.as_deref(), Some("session 'work' already exists"));
        let frame = render_session_picker(&config.theme, (100, 24), &view, error.as_deref());
        assert!(String::from_utf8_lossy(&frame).contains("already exists"));
        assert!(press(&config, &mut view, b"\x1b").is_continue());
        assert!(view.create_input.is_none());
        assert_eq!(
            press(&config, &mut view, b"\r"),
            ControlFlow::Break(Some("work".to_owned()))
        );
        assert_eq!(press(&config, &mut view, b"\x1b"), ControlFlow::Break(None));
    }

    #[test]
    fn configured_create_key_stays_text_during_name_entry() {
        let path =
            std::env::temp_dir().join(format!("rustmux-picker-config-{}.toml", std::process::id()));
        std::fs::write(&path, "").unwrap();
        let mut reloader = crate::config::ConfigReloader::new(path.clone());
        std::fs::write(&path, "[session_manager]\ncreate=['n']").unwrap();
        let config = reloader
            .poll(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        std::fs::remove_file(path).unwrap();
        let mut view = view(&config);
        assert!(press(&config, &mut view, b"a").is_continue());
        assert!(view.create_input.is_none());
        assert_eq!(
            press(&config, &mut view, b"nname\r"),
            ControlFlow::Break(Some("name".to_owned()))
        );
    }
}
