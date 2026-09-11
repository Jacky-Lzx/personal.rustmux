//! Stable entry points used by the out-of-tree `cargo-fuzz` harnesses.

use std::fs::File;
use std::os::fd::OwnedFd;

use nix::pty::Winsize;
use nix::unistd::Pid;

use crate::app::Window;
use crate::input::{InputDecoder, MousePosition, decode_key, decode_sgr_mouse, sgr_mouse_at};
use crate::layout::{PaneRect, content_size_for};
use crate::render::Renderer;
use crate::terminal::{
    CursorStyleTracker, HyperlinkTracker, InputModeTracker, KittyDndEvent, KittyDndParser,
    KittyGraphicsParser, KittyIpcParser, SemanticOutputCapture, TerminalMetadata,
    TerminalOscTracker, kitty_dnd_for_child, kitty_dnd_id, kitty_dnd_registration,
    kitty_dnd_with_id, kitty_graphics_query_response, kitty_graphics_uses_shared_memory, osc7_path,
    terminal_parser_size, terminal_responses,
};

fn chunks(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let size = data.first().map_or(1, |byte| usize::from(byte % 31) + 1);
    data.chunks(size)
}

pub fn input(data: &[u8]) {
    let mut decoder = InputDecoder::default();
    let mut decoded = Vec::new();
    for chunk in chunks(data) {
        decoded.extend(decoder.push(chunk));
    }
    decoded.extend(decoder.flush());

    let mut remaining = decoded.as_slice();
    while !remaining.is_empty() {
        let (_, consumed) = decode_key(remaining);
        remaining = &remaining[consumed.max(1).min(remaining.len())..];
    }
    let _ = decode_sgr_mouse(data);
    let column = u16::from(data.first().copied().unwrap_or_default());
    let row = u16::from(data.get(1).copied().unwrap_or_default());
    let _ = sgr_mouse_at(data, MousePosition { column, row });
}

pub fn kitty_graphics(data: &[u8]) {
    let mut parser = KittyGraphicsParser::default();
    for chunk in chunks(data) {
        let output = parser.process(chunk);
        for command in output.commands {
            let _ = kitty_graphics_query_response(&command);
            let _ = kitty_graphics_uses_shared_memory(&command);
        }
    }
    let _ = parser.process(&[]);
    let _ = kitty_graphics_query_response(data);
    let _ = kitty_graphics_uses_shared_memory(data);
}

pub fn kitty_dnd(data: &[u8]) {
    let mut parser = KittyDndParser::default();
    for chunk in chunks(data) {
        let output = parser.process(chunk);
        for event in output.events {
            if let KittyDndEvent::Command(command) = event {
                exercise_dnd_command(&command);
            }
        }
    }
    exercise_dnd_command(data);
    let _ = parser.flush();
}

fn exercise_dnd_command(command: &[u8]) {
    let _ = kitty_dnd_id(command);
    let _ = kitty_dnd_registration(command);
    let _ = kitty_dnd_with_id(command, 42);
    let _ = kitty_dnd_for_child(command, (-10, 7), (240, 120), (9, 18));
}

pub fn osc(data: &[u8]) {
    let mut capture = SemanticOutputCapture::default();
    let mut terminal = vt100::Parser::new_with_callbacks(24, 80, 128, TerminalMetadata::default());
    let mut cursor = CursorStyleTracker::default();
    for chunk in chunks(data) {
        let _ = capture.process(chunk);
        terminal.process(chunk);
        cursor.process(chunk);
    }
    let _ = capture.last_output();
    let _ = osc7_path(data);
    let _ = terminal_responses(
        data,
        Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: u16::from(data.first().copied().unwrap_or_default()),
            ws_ypixel: u16::from(data.get(1).copied().unwrap_or_default()),
        },
        "rustmux fuzz",
        terminal.screen().cursor_position(),
        terminal.screen().bracketed_paste(),
    );
}

pub fn render(data: &[u8]) {
    let columns = u16::from(data.first().copied().unwrap_or_default() % 96) + 4;
    let rows = u16::from(data.get(1).copied().unwrap_or_default() % 40) + 4;
    let payload = data.get(2..).unwrap_or_default();
    let (content_columns, content_rows) = content_size_for((columns, rows), false);
    let (parser_columns, parser_rows) = terminal_parser_size(content_columns, content_rows);
    let mut terminal = vt100::Parser::new_with_callbacks(
        parser_rows,
        parser_columns,
        128,
        TerminalMetadata::default(),
    );
    let split = payload.len() / 2;
    terminal.process(&payload[..split]);

    let master: OwnedFd = File::open("/dev/null")
        .expect("/dev/null is available")
        .into();
    let mut window = Window {
        id: 1,
        tab_id: 1,
        name: String::from_utf8_lossy(payload.get(..payload.len().min(32)).unwrap_or_default())
            .into_owned(),
        floating: false,
        pane_rect: PaneRect {
            column: 0,
            row: 0,
            width: content_columns,
            height: content_rows,
        },
        pane_framed: false,
        zoomed: false,
        master,
        child: Pid::from_raw(1),
        spawn_directory: None,
        terminal,
        cursor_style: CursorStyleTracker::default(),
        input_modes: InputModeTracker::default(),
        terminal_osc: TerminalOscTracker::default(),
        kitty_graphics: KittyGraphicsParser::default(),
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
        temporary_file: None,
        return_to_window: None,
    };
    let mut renderer = Renderer::default();
    renderer.set_session_name("fuzz");
    renderer.set_ui(false, vec!["?=show-help".to_owned()]);
    let _ = renderer.render(
        std::slice::from_ref(&window),
        0,
        (columns, rows),
        "locked",
        None,
        &[],
    );

    window.terminal.process(&payload[split..]);
    window.cursor_style.process(&payload[split..]);
    let mut graphics = KittyGraphicsParser::default().process(payload).commands;
    graphics.truncate(4);
    let _ = renderer.render(&[window], 0, (columns, rows), "locked", None, &graphics);
}

#[cfg(test)]
mod tests {
    #[test]
    fn render_handles_minimum_terminal_with_invalid_utf8() {
        super::render(&[0, 0, b't', 0x8c, b't', b'Z']);
    }
}
