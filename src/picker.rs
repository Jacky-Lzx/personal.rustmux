use std::io::{self, Write};
use std::os::fd::AsFd;

use crossterm::terminal::window_size;
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use nix::unistd::read;

use crate::app::{default_session_selection, matching_session_info};
use crate::config::Config;
use crate::input::{InputDecoder, decode_key};
use crate::render::{SessionManagerView, render_session_picker};
use crate::session::SessionInfo;
use crate::{Result, TerminalGuard};

pub(super) fn choose_session(sessions: Vec<SessionInfo>) -> Result<Option<String>> {
    let config = Config::load()?;
    let mut keys = config.session_manager_keys();
    // Only advertise actions available before attaching to a server.
    keys.retain(|action, _| {
        matches!(
            action.as_str(),
            "up" | "down" | "search" | "complete" | "open" | "cancel" | "backspace"
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
    };
    let _terminal = TerminalGuard::enter()?;
    let stdin = io::stdin();
    let mut decoder = InputDecoder::default();
    let mut input = [0; 4096];
    let mut previous_size = (0, 0);
    let mut dirty = true;
    loop {
        let size = window_size()?;
        let size = (size.columns, size.rows);
        if dirty || size != previous_size {
            let mut stdout = io::stdout().lock();
            stdout.write_all(&render_session_picker(&config.theme, size, &view))?;
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
            if key.event_type == 3 {
                continue;
            }
            if key.name == "ctrl c" {
                return Ok(None);
            }
            let action = config
                .session_manager_action(&key.name, view.searching)
                .unwrap_or("");
            match action {
                "cancel" if view.searching => {
                    view.searching = false;
                    view.query.clear();
                    view.sessions = sessions.clone();
                    view.selected = default_session_selection(&view.sessions, "");
                }
                "cancel" => return Ok(None),
                "search" => view.searching = true,
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
                        return Ok(Some(session.name.clone()));
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
                view.sessions = matching_session_info(&sessions, &view.query);
                view.selected = view.selected.min(view.sessions.len().saturating_sub(1));
            }
            dirty = true;
        }
    }
}
