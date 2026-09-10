use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Instant;

use nix::unistd::Pid;

use super::*;
use crate::app::{
    RenameEdit, TextSelection, Window, edit_window_name, matching_history_lines,
    matching_session_info, pane_at, process_name, rename_tab, selected_text, selection_contains,
    window_history,
};
use crate::input::{
    DecodedKey, InputDecoder, MouseAction, MousePosition, decode_key, decode_sgr_mouse,
    sgr_mouse_at,
};
use crate::layout::{
    Direction, FloatingLayout, PaneNode, PaneRect, SplitAxis, content_size_for,
    content_winsize_for, floating_layout_for, move_item, pane_ids, pane_rects, pane_resize_handle,
    remove_pane, resize_pane, resize_pane_to, split_pane, swap_panes, tiled_content_rect_for,
    validate_terminal_size, window_winsize_for,
};
use crate::render::{
    CellStyle, FrameSnapshot, HelpView, Renderer, SessionManagerView, compact_status_hints,
    help_rect, notification_rect, render_frame, session_manager_rect, styled_text_cells,
    truncate_to_display_width,
};
use crate::session::ServerOutputDecoder;
use crate::session::SessionInfo;
use crate::terminal::{
    CursorStyleTracker, KittyDndParser, KittyDndRegistration, KittyGraphicsParser,
    SemanticOutputCapture, TerminalMetadata, base64_encode, format_duration, kitty_dnd_for_child,
    kitty_dnd_id, kitty_dnd_registration, kitty_dnd_with_id, kitty_graphics_query_response,
    kitty_graphics_uses_shared_memory, kitty_notification, osc7_path, terminal_parser_size,
    terminal_responses,
};

fn test_window(id: usize, name: &str, rows: u16, columns: u16) -> Window {
    // SAFETY: dup returns a new descriptor owned solely by this test.
    let fd = unsafe { nix::libc::dup(nix::libc::STDOUT_FILENO) };
    assert!(fd >= 0);
    // SAFETY: fd is a valid, newly duplicated descriptor.
    let master = unsafe { OwnedFd::from_raw_fd(fd) };
    Window {
        id,
        tab_id: id,
        name: name.to_owned(),
        floating: false,
        pane_rect: PaneRect {
            column: 0,
            row: 0,
            width: columns,
            height: rows,
        },
        pane_framed: false,
        zoomed: false,
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
        kitty_dnd: KittyDndParser::default(),
        dnd_drag_registration: None,
        dnd_drop_registration: None,
        pending_graphics: Vec::new(),
        history_mode: false,
        bell_pending: false,
        command_output: SemanticOutputCapture::default(),
        notification_applications: Vec::new(),
        temporary_file: None,
        return_to_window: None,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn process_name_resolves_the_running_process() {
    assert!(process_name(Pid::this()).is_some());
}

#[test]
fn content_winsize_excludes_border_cells_and_preserves_cell_pixels() {
    let value = content_winsize_for((218, 62), (3706, 2046), false);
    assert_eq!(value.ws_col, 216);
    assert_eq!(value.ws_row, 58);
    assert_eq!(value.ws_xpixel, 3672);
    assert_eq!(value.ws_ypixel, 1914);
}

#[test]
fn terminal_size_validation_bounds_frame_allocations() {
    assert!(validate_terminal_size((1, 1)).is_ok());
    assert!(validate_terminal_size((1_000, 1_000)).is_ok());
    assert!(validate_terminal_size((0, 24)).is_err());
    assert!(validate_terminal_size((80, 0)).is_err());
    assert!(validate_terminal_size((1_001, 1_000)).is_err());
    assert!(validate_terminal_size((u16::MAX, u16::MAX)).is_err());
}

#[test]
fn terminal_parser_has_a_safe_minimum_for_wrapping() {
    assert_eq!(terminal_parser_size(1, 1), (2, 2));
    assert_eq!(terminal_parser_size(80, 24), (80, 24));

    let (columns, rows) = terminal_parser_size(1, 1);
    let mut parser = vt100::Parser::new(rows, columns, 0);
    parser.process(b"ttZ");
}

#[test]
fn osc7_tracks_the_current_working_directory() {
    assert_eq!(
        osc7_path(b"file://localhost/Users/example/My%20Project"),
        Some(PathBuf::from("/Users/example/My Project"))
    );
    assert_eq!(osc7_path(b"https://example.com/tmp"), None);
    assert_eq!(osc7_path(b"file://localhost/tmp/%GG"), None);

    let mut terminal = vt100::Parser::new_with_callbacks(2, 10, 0, TerminalMetadata::default());
    terminal.process(b"\x1b]7;file://host/tmp/work%20tree\x1b\\");
    assert_eq!(
        terminal.callbacks().current_directory,
        Some(PathBuf::from("/tmp/work tree"))
    );
}

#[test]
fn terminal_guard_isolates_rustmux_from_shell_scrollback() {
    assert!(TERMINAL_ENTER_SEQUENCE.starts_with(b"\x1b[?1049h"));
    assert!(TERMINAL_EXIT_SEQUENCE.ends_with(b"\x1b[?1049l"));
    assert_eq!(
        TERMINAL_ENTER_SEQUENCE
            .windows(b"\x1b[?1049h".len())
            .filter(|window| *window == b"\x1b[?1049h")
            .count(),
        1
    );
    assert_eq!(
        TERMINAL_EXIT_SEQUENCE
            .windows(b"\x1b[?1049l".len())
            .filter(|window| *window == b"\x1b[?1049l")
            .count(),
        1
    );
}

#[test]
fn floating_layout_is_centered_and_has_a_smaller_pty() {
    let layout = floating_layout_for((80, 24), false);
    let winsize = window_winsize_for((80, 24), (800, 480), true, false);

    assert_eq!(
        layout,
        FloatingLayout {
            column: 12,
            row: 6,
            width: 58,
            height: 14,
        }
    );
    assert_eq!((winsize.ws_col, winsize.ws_row), layout.content_size());
    assert_eq!((winsize.ws_xpixel, winsize.ws_ypixel), (560, 240));
}

#[test]
fn key_decoder_names_control_navigation_and_modified_keys() {
    assert_eq!(decode_key(b"\x02").0.name, "ctrl b");
    assert_eq!(decode_key(b"\x1b[B").0.name, "down");
    assert_eq!(decode_key(b"\x1b[5~").0.name, "pageup");
    assert_eq!(decode_key(b"\x1b[3~").0.name, "delete");
    assert_eq!(decode_key(b"\x1b[103;5u").0.name, "ctrl g");
    assert_eq!(decode_key(b"\x1b[27;3;120~").0.name, "alt x");
    assert_eq!(decode_key(b"G").0.name, "G");
}

#[test]
fn window_name_editor_accepts_text_backspace_confirm_and_cancel() {
    let mut name = "fish".to_owned();
    let text = DecodedKey {
        name: "终".to_owned(),
        raw: "终".as_bytes().to_vec(),
    };
    assert_eq!(edit_window_name(&mut name, &text), RenameEdit::Continue);
    assert_eq!(name, "fish终");

    let (backspace, _) = decode_key(b"\x7f");
    assert_eq!(
        edit_window_name(&mut name, &backspace),
        RenameEdit::Continue
    );
    assert_eq!(name, "fish");

    let (enter, _) = decode_key(b"\r");
    assert_eq!(edit_window_name(&mut name, &enter), RenameEdit::Confirm);
    let (escape, _) = decode_key(b"\x1b");
    assert_eq!(edit_window_name(&mut name, &escape), RenameEdit::Cancel);
}

#[test]
fn renaming_a_window_updates_all_panes_in_the_tab() {
    let first = test_window(1, "fish", 2, 10);
    let mut second = test_window(2, "fish", 2, 10);
    second.tab_id = first.tab_id;
    let third = test_window(3, "other", 2, 10);
    let mut windows = vec![first, second, third];

    rename_tab(&mut windows, 1, "editor");

    assert_eq!(windows[0].name, "editor");
    assert_eq!(windows[1].name, "editor");
    assert_eq!(windows[2].name, "other");

    rename_tab(&mut windows, 1, "");
    assert_eq!(windows[0].name, "");
    assert_eq!(windows[1].name, "");
}

#[test]
fn sgr_mouse_decoder_recognizes_wheel_and_selection_events() {
    assert_eq!(
        decode_sgr_mouse(b"\x1b[<64;10;5M"),
        Some((
            MouseAction::ScrollUp(MousePosition { column: 10, row: 5 }),
            11
        ))
    );
    assert_eq!(
        decode_sgr_mouse(b"\x1b[<69;10;5M"),
        Some((
            MouseAction::ScrollDown(MousePosition { column: 10, row: 5 }),
            11
        ))
    );
    assert_eq!(
        decode_sgr_mouse(b"\x1b[<66;10;5M"),
        Some((MouseAction::Other(MousePosition { column: 10, row: 5 }), 11))
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

    assert_eq!(
        sgr_mouse_at(b"\x1b[<32;12;6M", MousePosition { column: 2, row: 3 }),
        Some(b"\x1b[<32;3;4M".to_vec())
    );
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
        keyboard: false,
    };

    assert_eq!(
        selected_text(&window, selection, (18, 3)),
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
            (18, 3)
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
        keyboard: false,
    };

    assert_eq!(selected_text(&window, selection, (5, 2)), "abcdef");
}

#[test]
fn selection_must_span_multiple_cells_before_copying() {
    let single_cell = TextSelection {
        window_id: 9,
        start: MousePosition { column: 2, row: 1 },
        end: MousePosition { column: 2, row: 1 },
        keyboard: false,
    };
    let multiple_cells = TextSelection {
        end: MousePosition { column: 3, row: 1 },
        ..single_cell
    };

    assert!(!single_cell.spans_multiple_cells());
    assert!(multiple_cells.spans_multiple_cells());
    assert!(!selection_contains(Some(&single_cell), 9, 1, 2, 5));
    assert!(selection_contains(Some(&multiple_cells), 9, 1, 2, 5));
    assert!(selection_contains(Some(&multiple_cells), 9, 1, 3, 5));

    let keyboard_cell = TextSelection {
        keyboard: true,
        ..single_cell
    };
    assert!(keyboard_cell.is_visible());
    assert!(selection_contains(Some(&keyboard_cell), 9, 1, 2, 5));
}

#[test]
fn history_search_matches_lines_case_insensitively() {
    let lines = vec![
        "cargo check".to_owned(),
        "Finished release".to_owned(),
        "cargo test".to_owned(),
    ];
    assert_eq!(matching_history_lines(&lines, "CARGO"), vec![0, 2]);
    assert!(matching_history_lines(&lines, "").is_empty());
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
        keyboard: false,
    };

    let update = renderer.render(&windows, 0, (20, 6), "locked", Some(&selection), &[]);

    assert!(update.windows(4).any(|part| part == b"\x1b[7m"));
    assert!(!update.windows(4).any(|part| part == b"\x1b[2J"));
}

#[test]
fn clipboard_status_is_drawn_below_the_bottom_border_incrementally() {
    let window = test_window(3, "fish", 2, 38);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
    renderer.set_border_status(Some(CLIPBOARD_STATUS));

    let shown = renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
    let shown = String::from_utf8(shown).unwrap();
    assert!(shown.contains("\x1b[6;1H"));
    assert!(shown.contains("copied to system clipboard"));
    assert!(shown.contains(''));
    assert!(!shown.contains('└'));
    assert!(!shown.contains("\x1b[2J"));

    renderer.set_border_status(None);
    let cleared = renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
    let cleared = String::from_utf8(cleared).unwrap();
    assert!(cleared.contains("\x1b[6;1H"));
    assert!(!cleared.contains(CLIPBOARD_STATUS));
}

#[test]
fn warning_is_drawn_in_a_centered_floating_box_and_clears_cleanly() {
    let windows = vec![test_window(1, "fish", 8, 58)];
    let mut renderer = Renderer::default();
    renderer.render(&windows, 0, (60, 12), "scroll", None, &[]);
    renderer.set_notification(Some("no previous command output"));

    let warning = renderer.render(&windows, 0, (60, 12), "scroll", None, &[]);
    let warning = String::from_utf8(warning).unwrap();
    assert!(warning.contains("Rustmux Warning"));
    assert!(warning.contains("no previous command output"));
    assert_eq!(notification_rect((58, 8), "short"), (22, 2, 14, 3));

    renderer.set_notification(None);
    let cleared = renderer.render(&windows, 0, (60, 12), "scroll", None, &[]);
    assert!(cleared.starts_with(b"\x1b[?25l\x1b[2J"));
}

#[test]
fn status_line_shows_mode_and_key_hints() {
    let window = test_window(1, "fish", 1, 18);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(false, vec!["Ctrl b=mode:normal".to_owned()]);

    let frame = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("Ctrl b"));
    assert!(frame.contains("UNLOCK"));
    assert!(frame.contains(''));
    assert!(!frame.contains(''));
    assert!(frame.contains("\x1b[4;1H\x1b[0;38;2;166;227;161;49m└"));
    assert!(frame.contains("\x1b[5;1H"));
}

#[test]
fn status_hints_group_aliases_and_window_shortcuts() {
    let hints = [
        "&=close-window + mode:locked",
        "x=close-window + mode:locked",
        ",=rename-window",
        "1=window:1 + mode:locked",
        "2=window:2 + mode:locked",
        "n=next-window + mode:locked",
        "p=previous-window + mode:locked",
        "?=show-help + mode:locked",
    ]
    .map(str::to_owned);

    let compact = compact_status_hints(&hints);

    assert!(compact.contains(&("&/x".to_owned(), "CLOSE WINDOW".to_owned())));
    assert!(compact.contains(&("1-2".to_owned(), "WINDOW".to_owned())));
    assert!(compact.contains(&("n/p".to_owned(), "WINDOW".to_owned())));
    assert!(compact.contains(&("?".to_owned(), "HELP".to_owned())));
    assert!(compact.iter().all(|(_, action)| !action.contains("LOCK")));
}

#[test]
fn overflowing_status_hints_end_with_more_instead_of_a_partial_action() {
    let windows = vec![test_window(1, "fish", 2, 58)];
    let mut renderer = Renderer::default();
    renderer.set_ui(
        false,
        vec![
            "c=new-window + mode:locked".to_owned(),
            ",=rename-window".to_owned(),
            "n=next-window + mode:locked".to_owned(),
            "p=previous-window + mode:locked".to_owned(),
            "s=switch-session".to_owned(),
            "?=show-help + mode:locked".to_owned(),
        ],
    );

    let frame = renderer.render(&windows, 0, (60, 6), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("MORE (+"));
    assert!(!frame.contains("RENAME WINDO"));
}

#[test]
fn help_is_drawn_in_a_centered_two_column_box() {
    let windows = vec![test_window(1, "fish", 16, 98)];
    let mut renderer = Renderer::default();
    renderer.set_help(Some(HelpView {
        mode: "normal".to_owned(),
        hints: vec![
            "c=new-window + mode:locked".to_owned(),
            ",=rename-window".to_owned(),
            "n=next-window + mode:locked".to_owned(),
            "p=previous-window + mode:locked".to_owned(),
        ],
    }));

    let frame = renderer.render(&windows, 0, (100, 20), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("Keybindings"));
    assert!(frame.contains("Mode: NORMAL"));
    assert!(frame.contains("\x1b[1;38;2;245;194;231;48;2;30;30;46mc"));
    assert!(frame.contains("\x1b[0;38;2;205;214;244;48;2;30;30;46m  NEW WINDOW + LOCK"));
    assert!(frame.contains("Esc close"));
    assert!(frame.contains("press a key to run"));
    assert_eq!(help_rect((98, 16), 4), (12, 4, 73, 7));
}

#[test]
fn rename_prompt_replaces_bottom_key_hints() {
    let window = test_window(1, "fish", 1, 38);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(false, vec!["Ctrl b=mode:normal".to_owned()]);
    renderer.set_rename_prompt(Some("editor"));

    let frame = renderer.render(&windows, 0, (40, 5), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("RENAME: editor_"));
    assert!(!frame.contains("Ctrl b"));
}

#[test]
fn history_search_prompt_replaces_bottom_key_hints() {
    let window = test_window(1, "fish", 2, 18);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(false, vec!["q=quit".to_owned()]);
    renderer.set_history_search_prompt(Some("cargo"));

    let frame = renderer.render(&windows, 0, (20, 5), "scroll", None, &[]);
    let frame = String::from_utf8(frame).unwrap();
    assert!(frame.contains("SEARCH: cargo_"));
    assert!(!frame.contains("QUIT"));
}

#[test]
fn window_bar_shows_the_session_name_before_the_first_window() {
    let windows = vec![
        test_window(1, "fish", 2, 78),
        test_window(2, "editor", 2, 78),
    ];
    let mut renderer = Renderer::default();
    renderer.set_session_name("personal");

    let frame = renderer.render(&windows, 0, (80, 6), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    let session = frame.find(" Rustmux (personal) ").unwrap();
    let first_window = frame.find(" 1 fish ").unwrap();
    assert!(session < first_window);
    assert!(frame.contains("38;2;205;214;244;48;2;30;30;46"));
}

#[test]
fn window_bar_marks_a_tab_when_one_of_its_panes_has_a_pending_bell() {
    let mut first = test_window(1, "shell", 10, 18);
    first.pane_framed = true;
    first.pane_rect = PaneRect {
        column: 0,
        row: 0,
        width: 20,
        height: 12,
    };
    let mut second = test_window(2, "shell", 10, 18);
    second.tab_id = first.tab_id;
    second.pane_framed = true;
    second.pane_rect = PaneRect {
        column: 20,
        row: 0,
        width: 20,
        height: 12,
    };
    second.bell_pending = true;
    let mut renderer = Renderer::default();
    let mut windows = vec![first, second];

    let frame = renderer.render(&windows, 0, (40, 15), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();
    assert!(frame.contains(" 1 shell [!] "));
    assert!(frame.contains("─ shell [!] "));
    assert!(frame.contains("38;2;250;179;135"));

    windows[1].bell_pending = false;
    let frame = renderer.render(&windows, 0, (40, 15), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();
    assert!(!frame.contains("[!]"));
    assert!(!frame.contains("38;2;250;179;135"));
}

#[test]
fn window_bar_tabs_are_clickable_across_their_powerline_segments() {
    let windows = vec![
        test_window(11, "fish", 2, 78),
        test_window(22, "editor", 2, 78),
    ];
    let mut renderer = Renderer::default();
    renderer.set_session_name("work");
    renderer.render(&windows, 0, (80, 6), "locked", None, &[]);

    let hits = (1..=80)
        .filter_map(|column| {
            renderer
                .window_tab_at((column, 1))
                .map(|tab_id| (column, tab_id))
        })
        .collect::<Vec<_>>();
    assert!(hits.iter().any(|(_, tab_id)| *tab_id == 11));
    assert!(hits.iter().any(|(_, tab_id)| *tab_id == 22));
    assert_eq!(hits.first().map(|(_, tab_id)| *tab_id), Some(11));
    assert_eq!(hits.last().map(|(_, tab_id)| *tab_id), Some(22));
    assert_eq!(renderer.window_tab_at((1, 1)), None);
    assert_eq!(renderer.window_tab_at((20, 2)), None);
}

#[test]
fn window_name_changes_redraw_the_bar_without_clearing_the_screen() {
    let window = test_window(1, "fish", 1, 38);
    let mut windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.render(&windows, 0, (40, 5), "normal", None, &[]);

    windows[0].name = "editor".to_owned();
    let update = renderer.render(&windows, 0, (40, 5), "normal", None, &[]);
    let update_text = String::from_utf8_lossy(&update);

    assert!(update_text.contains("\x1b[1;1H"));
    assert!(update_text.contains(" 1 editor "));
    assert!(!update.windows(4).any(|part| part == b"\x1b[2J"));
}

#[test]
fn compact_layout_reclaims_status_row_and_shows_mode_at_top_right() {
    assert_eq!(content_size_for((20, 5), false), (18, 1));
    assert_eq!(content_size_for((20, 5), true), (18, 3));
    assert_eq!(tiled_content_rect_for((20, 5), false).height, 3);
    assert_eq!(tiled_content_rect_for((20, 5), true).height, 4);

    let window = test_window(1, "fish", 3, 18);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(true, Vec::new());

    let frame = renderer.render(&windows, 0, (20, 5), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("\x1b[1;11H"));
    assert!(frame.contains(""));
    assert!(!frame.contains(""));
    assert!(frame.contains(" NORMAL "));
    assert!(!frame.contains('└'));
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
    assert!(history_text.contains("SCROLL 1"));
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
fn frame_has_catppuccin_powerline_tabs_and_terminal_contents() {
    let mut first = test_window(1, "fish", 3, 18);
    first.terminal.process(b"hello \x1b[38;2;1;2;3mcolor");
    first.terminal.process(b"\x1b]2;nvim project\x07");
    let second = test_window(2, "fish", 3, 18);

    let windows = [first, second];
    let snapshot = FrameSnapshot::capture(&windows, 0, (20, 5), "locked", None, None);
    let frame = render_frame(&windows, 0, &snapshot, &[]);
    let frame = String::from_utf8(frame).expect("rendered frame is UTF-8");

    assert!(frame.contains("38;2;166;227;161"));
    assert!(frame.contains("48;2;30;30;46"));
    assert!(frame.contains('┌'));
    assert!(frame.contains('┘'));
    assert!(frame.contains(''));
    assert!(!frame.contains(''));
    assert!(frame.contains("\x1b[0;38;2;30;30;46;48;2;166;227;161m"));
    assert!(frame.contains("\x1b[0;38;2;166;227;161;48;2;30;30;46m"));
    assert!(frame.contains(" 1 fish "));
    assert!(frame.contains(" 2 fish "));
    assert!(frame.contains("48;2;166;227;161m 1 fish "));
    assert!(frame.contains("48;2;205;214;244m 2 fish "));
    assert!(frame.contains("─ nvim project "));
    assert!(frame.contains("hello"));
    assert!(frame.contains("\x1b[38;2;1;2;3m"));
}

#[test]
fn labels_are_truncated_by_terminal_column_width() {
    let cells = styled_text_cells("a界b", 3, CellStyle::border());
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0].contents, "a");
    assert_eq!(cells[1].contents, "界");
    assert!(cells[2].wide_continuation);

    let (label, width) = truncate_to_display_width("e\u{301}界x", 3);
    assert_eq!(label, "e\u{301}界");
    assert_eq!(width, 3);
}

#[test]
fn floating_terminal_is_composited_over_the_active_tab() {
    let mut base = test_window(1, "base", 12, 38);
    base.terminal.process(b"base contents");
    let mut floating = test_window(2, "float", 6, 26);
    floating.floating = true;
    floating.return_to_window = Some(1);
    floating.terminal.process(b"floating contents");
    let windows = vec![base, floating];
    let mut renderer = Renderer::default();

    let frame = renderer.render(&windows, 1, (40, 15), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("base contents"));
    assert!(frame.contains("┌─ float "));
    assert!(frame.contains("floating contents"));
    assert!(frame.contains("\x1b[1m\x1b[38;2;166;227;161m┌─ float "));
    assert!(frame.contains("\x1b[0;38;2;108;112;134;49m┌─ base "));
    assert!(frame.contains(" 1 base "));
    assert!(!frame.contains(" 2 float "));

    let restored = renderer.render(&windows[..1], 0, (40, 15), "locked", None, &[]);
    let restored = String::from_utf8(restored).unwrap();
    assert!(restored.contains("\x1b[0;38;2;166;227;161;49m┌─ base "));
}

#[test]
fn floating_terminal_unfocuses_every_tiled_pane() {
    let mut left = test_window(1, "left", 10, 18);
    left.tab_id = 1;
    left.pane_framed = true;
    left.pane_rect = PaneRect {
        column: 0,
        row: 0,
        width: 20,
        height: 12,
    };
    let mut right = test_window(2, "right", 10, 18);
    right.tab_id = 1;
    right.pane_framed = true;
    right.pane_rect = PaneRect {
        column: 20,
        row: 0,
        width: 20,
        height: 12,
    };
    let mut floating = test_window(3, "float", 6, 26);
    floating.tab_id = 1;
    floating.floating = true;
    floating.return_to_window = Some(2);
    let windows = vec![left, right, floating];

    let snapshot = FrameSnapshot::capture(&windows, 2, (40, 15), "locked", None, None);
    let frame = String::from_utf8(render_frame(&windows, 2, &snapshot, &[])).unwrap();

    assert!(frame.contains("┌─ left "));
    assert!(frame.contains("┌─ right "));
    assert!(!frame.contains("\x1b[1m\x1b[38;2;166;227;161m┌─ left "));
    assert!(!frame.contains("\x1b[1m\x1b[38;2;166;227;161m┌─ right "));
    assert!(frame.contains("\x1b[1m\x1b[38;2;166;227;161m┌─ float "));
}

#[test]
fn pane_layout_splits_the_active_leaf_and_collapses_after_removal() {
    let mut root = PaneNode::Leaf(1);
    assert!(split_pane(&mut root, 1, 2, SplitAxis::Vertical));
    assert!(split_pane(&mut root, 2, 3, SplitAxis::Horizontal));
    let rects = pane_rects(
        &root,
        PaneRect {
            column: 0,
            row: 0,
            width: 80,
            height: 20,
        },
    );

    assert_eq!(
        rects,
        vec![
            (
                1,
                PaneRect {
                    column: 0,
                    row: 0,
                    width: 40,
                    height: 20
                }
            ),
            (
                2,
                PaneRect {
                    column: 40,
                    row: 0,
                    width: 40,
                    height: 10
                }
            ),
            (
                3,
                PaneRect {
                    column: 40,
                    row: 10,
                    width: 40,
                    height: 10
                }
            ),
        ]
    );
    let root = remove_pane(root, 2).unwrap();
    assert_eq!(pane_ids(&root), vec![1, 3]);
}

#[test]
fn pane_layout_swaps_contents_without_changing_the_split_tree() {
    let mut root = PaneNode::Split {
        axis: SplitAxis::Vertical,
        ratio: 420,
        first: Box::new(PaneNode::Leaf(1)),
        second: Box::new(PaneNode::Split {
            axis: SplitAxis::Horizontal,
            ratio: 630,
            first: Box::new(PaneNode::Leaf(2)),
            second: Box::new(PaneNode::Leaf(3)),
        }),
    };

    assert!(swap_panes(&mut root, 1, 3));
    assert_eq!(pane_ids(&root), vec![3, 2, 1]);
    assert_eq!(
        root,
        PaneNode::Split {
            axis: SplitAxis::Vertical,
            ratio: 420,
            first: Box::new(PaneNode::Leaf(3)),
            second: Box::new(PaneNode::Split {
                axis: SplitAxis::Horizontal,
                ratio: 630,
                first: Box::new(PaneNode::Leaf(2)),
                second: Box::new(PaneNode::Leaf(1)),
            }),
        }
    );
    assert!(!swap_panes(&mut root, 1, 99));
    assert!(!swap_panes(&mut root, 1, 1));
}

#[test]
fn move_item_swaps_with_an_adjacent_item_without_wrapping() {
    let mut items = vec!["one", "two", "three"];

    assert!(move_item(&mut items, 1, -1));
    assert_eq!(items, vec!["two", "one", "three"]);
    assert!(!move_item(&mut items, 0, -1));
    assert!(!move_item(&mut items, 2, 1));
}

#[test]
fn pane_layout_resizes_the_nearest_boundary() {
    let mut root = PaneNode::Leaf(1);
    assert!(split_pane(&mut root, 1, 2, SplitAxis::Vertical));
    assert!(split_pane(&mut root, 2, 3, SplitAxis::Horizontal));
    assert!(resize_pane(&mut root, 2, Direction::Down));
    assert!(resize_pane(&mut root, 2, Direction::Left));
    let rects = pane_rects(
        &root,
        PaneRect {
            column: 0,
            row: 0,
            width: 100,
            height: 20,
        },
    );
    assert_eq!(rects[0].1.width, 45);
    assert_eq!(
        rects[1].1,
        PaneRect {
            column: 45,
            row: 0,
            width: 55,
            height: 11
        }
    );
    assert_eq!(
        rects[2].1,
        PaneRect {
            column: 45,
            row: 11,
            width: 55,
            height: 9
        }
    );
}

#[test]
fn pane_border_drag_resizes_from_either_shared_border_cell() {
    let rect = PaneRect {
        column: 0,
        row: 0,
        width: 80,
        height: 20,
    };
    for grabbed_column in [39, 40] {
        let mut root = PaneNode::Split {
            axis: SplitAxis::Vertical,
            ratio: 500,
            first: Box::new(PaneNode::Leaf(1)),
            second: Box::new(PaneNode::Leaf(2)),
        };
        let handle = pane_resize_handle(&root, rect, (grabbed_column, 10))
            .expect("both copies of a shared border should be draggable");
        assert!(resize_pane_to(
            &mut root,
            &handle,
            (grabbed_column + 10, 10)
        ));

        let panes = pane_rects(&root, rect);
        assert_eq!(panes[0].1.width, 50);
        assert_eq!(panes[1].1.column, 50);
        assert_eq!(panes[1].1.width, 30);
    }
    let root = PaneNode::Split {
        axis: SplitAxis::Vertical,
        ratio: 500,
        first: Box::new(PaneNode::Leaf(1)),
        second: Box::new(PaneNode::Leaf(2)),
    };
    assert_eq!(pane_resize_handle(&root, rect, (20, 10)), None);

    let mut horizontal = PaneNode::Split {
        axis: SplitAxis::Horizontal,
        ratio: 500,
        first: Box::new(PaneNode::Leaf(1)),
        second: Box::new(PaneNode::Leaf(2)),
    };
    let handle = pane_resize_handle(&horizontal, rect, (20, 10))
        .expect("a horizontal shared border should be draggable");
    assert!(resize_pane_to(&mut horizontal, &handle, (20, 14)));
    let panes = pane_rects(&horizontal, rect);
    assert_eq!(panes[0].1.height, 14);
    assert_eq!(panes[1].1.row, 14);
}

#[test]
fn zoomed_pane_is_rendered_as_the_only_full_size_pane() {
    let mut first = test_window(1, "left", 6, 38);
    first.tab_id = 1;
    let mut second = test_window(2, "right", 6, 38);
    second.tab_id = 1;
    second.zoomed = true;
    second.terminal.process(b"zoomed contents");
    let windows = vec![first, second];

    let snapshot = FrameSnapshot::capture(&windows, 1, (40, 10), "locked", None, None);
    let frame = String::from_utf8(render_frame(&windows, 1, &snapshot, &[])).unwrap();
    assert!(snapshot.outer_border);
    assert_eq!(snapshot.content_size, (38, 6));
    assert!(frame.contains("zoomed contents"));
    assert!(!frame.contains("┌─ left"));
}

#[test]
fn tiled_panes_are_composited_inside_one_tab() {
    let mut left = test_window(1, "fish", 12, 18);
    left.tab_id = 1;
    left.pane_framed = true;
    left.pane_rect = PaneRect {
        column: 0,
        row: 0,
        width: 20,
        height: 14,
    };
    left.terminal.process(b"left pane");
    let mut right = test_window(2, "fish", 12, 18);
    right.tab_id = 1;
    right.pane_framed = true;
    right.pane_rect = PaneRect {
        column: 20,
        row: 0,
        width: 20,
        height: 14,
    };
    right.terminal.process(b"right pane");
    let windows = vec![left, right];

    let snapshot = FrameSnapshot::capture(&windows, 0, (40, 15), "locked", None, None);
    let frame = String::from_utf8(render_frame(&windows, 0, &snapshot, &[])).unwrap();

    assert!(frame.contains("left pane"));
    assert!(frame.contains("right pane"));
    assert!(frame.contains("┌─ fish"));
    assert_eq!(frame.matches("┌─ fish").count(), 2);
    assert!(frame.contains(" 1 fish "));
    assert!(!frame.contains(" 2 fish "));
    assert_eq!(snapshot.tabs, vec![(1, "fish".to_owned())]);
    assert!(!snapshot.outer_border);
    assert_eq!(snapshot.content_size, (40, 13));
    assert_eq!(snapshot.content_origin, (1, 2));
}

#[test]
fn mouse_position_selects_a_visible_pane() {
    let mut left = test_window(1, "left", 10, 18);
    left.tab_id = 1;
    left.pane_framed = true;
    left.pane_rect = PaneRect {
        column: 0,
        row: 0,
        width: 20,
        height: 12,
    };
    let mut right = test_window(2, "right", 10, 18);
    right.tab_id = 1;
    right.pane_framed = true;
    right.pane_rect = PaneRect {
        column: 20,
        row: 0,
        width: 20,
        height: 12,
    };
    let mut other_tab = test_window(3, "other", 10, 38);
    other_tab.tab_id = 3;
    let windows = vec![left, right, other_tab];

    assert_eq!(
        pane_at(&windows, 0, MousePosition { column: 5, row: 5 }),
        Some(0)
    );
    assert_eq!(
        pane_at(&windows, 0, MousePosition { column: 25, row: 5 }),
        Some(1)
    );
    assert_eq!(
        pane_at(&windows, 0, MousePosition { column: 1, row: 1 }),
        None
    );
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
fn kitty_dnd_parser_extracts_fragmented_osc_72_sequences() {
    let mut parser = KittyDndParser::default();

    let first = parser.process(b"before\x1b]7");
    assert_eq!(first.terminal, b"before");
    assert!(first.commands.is_empty());
    assert!(parser.flush_deadline().is_some());

    let second = parser.process(b"2;t=a;text/uri-list\x1b");
    assert!(second.terminal.is_empty());
    assert!(second.commands.is_empty());
    assert!(parser.flush_deadline().is_none());

    let third = parser.process(b"\\after");
    assert_eq!(third.commands, [b"\x1b]72;t=a;text/uri-list\x1b\\"]);
    assert_eq!(third.terminal, b"after");

    let ordinary = parser.process(b"\x1b]2;title\x1b\\");
    assert_eq!(ordinary.terminal, b"\x1b]2;title\x1b\\");
    assert!(ordinary.commands.is_empty());
}

#[test]
fn kitty_dnd_parser_releases_an_ambiguous_escape() {
    let mut parser = KittyDndParser::default();

    let output = parser.process(b"\x1b");
    assert!(output.terminal.is_empty());
    assert!(output.commands.is_empty());
    assert!(parser.flush_deadline().is_some());
    assert_eq!(parser.flush(), b"\x1b");
    assert!(parser.flush_deadline().is_none());
}

#[test]
fn kitty_dnd_commands_are_tagged_for_multiplexer_routing() {
    let tagged = kitty_dnd_with_id(b"\x1b]72;t=o:x=1;machine\x1b\\", 42).unwrap();
    assert_eq!(tagged, b"\x1b]72;t=o:x=1:i=42;machine\x1b\\");
    assert_eq!(kitty_dnd_id(&tagged), Some(42));
    assert_eq!(
        kitty_dnd_registration(&tagged),
        Some(KittyDndRegistration::Drag(true))
    );

    let replaced = kitty_dnd_with_id(b"\x1b]72;m=1:i=9;YWJj\x1b\\", 42).unwrap();
    assert_eq!(replaced, b"\x1b]72;m=1:i=42;YWJj\x1b\\");
    assert_eq!(
        kitty_dnd_registration(b"\x1b]72;t=A:i=42\x1b\\"),
        Some(KittyDndRegistration::Drop(false))
    );
}

#[test]
fn kitty_dnd_events_are_translated_to_pane_coordinates() {
    let event = b"\x1b]72;t=m:i=42:x=11:y=7:X=110:Y=140:o=3;text/uri-list\x1b\\";
    let translated = kitty_dnd_for_child(event, (10, 5), (20, 10), (10, 20)).unwrap();
    assert_eq!(
        translated,
        b"\x1b]72;t=m:x=1:y=2:X=10:Y=40:o=3;text/uri-list\x1b\\"
    );
    assert_eq!(kitty_dnd_id(&translated), None);

    let outside = kitty_dnd_for_child(event, (20, 5), (20, 10), (10, 20)).unwrap();
    assert_eq!(
        outside,
        b"\x1b]72;t=m:x=-1:y=-1:X=110:Y=140:o=3;text/uri-list\x1b\\"
    );

    let data = kitty_dnd_for_child(
        b"\x1b]72;t=r:i=42:x=2:m=0;YWJj\x1b\\",
        (10, 5),
        (20, 10),
        (10, 20),
    )
    .unwrap();
    assert_eq!(data, b"\x1b]72;t=r:x=2:m=0;YWJj\x1b\\");
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
        content_winsize_for((80, 24), (1360, 792), false),
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
        content_winsize_for((80, 24), (1360, 792), false),
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
        content_winsize_for((80, 24), (1360, 792), false),
        "kitty 0.40.0",
        (0, 0),
        false,
    );
    assert_eq!(bell_background, background);
    assert!(
        responses
            .windows(b"\x1b[4;660;1326t".len())
            .any(|part| part == b"\x1b[4;660;1326t")
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
fn server_output_decoder_intercepts_split_session_switch_messages() {
    let mut decoder = ServerOutputDecoder::default();
    let (visible, target) = decoder.push(b"frame\x1b]777;rustmux-switch-").unwrap();
    assert_eq!(visible, b"frame");
    assert_eq!(target, None);

    let (visible, target) = decoder.push(b"session=work\x07").unwrap();
    assert!(visible.is_empty());
    assert_eq!(target.as_deref(), Some("work"));
}

#[test]
fn session_manager_searches_case_insensitively() {
    let sessions = ["alpha", "Personal", "work"].map(|name| SessionInfo {
        name: name.to_owned(),
        tabs: 1,
        panes: 1,
        connected: false,
        created_at: 1,
        saved: false,
    });
    assert_eq!(matching_session_info(&sessions, "son")[0].name, "Personal");
    assert_eq!(matching_session_info(&sessions, "W")[0].name, "work");
    assert!(matching_session_info(&sessions, "new").is_empty());
}

#[test]
fn session_manager_renders_search_results_and_actions() {
    let window = test_window(1, "fish", 8, 58);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_session_manager(Some(SessionManagerView {
        query: "wo".to_owned(),
        sessions: vec![SessionInfo {
            name: "work".to_owned(),
            tabs: 2,
            panes: 3,
            connected: false,
            created_at: 1,
            saved: true,
        }],
        selected: 0,
        current: "personal".to_owned(),
        rename_input: None,
    }));

    let frame = renderer.render(&windows, 0, (60, 12), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("Session Manager"));
    assert!(frame.contains("Session: wo_"));
    assert!(frame.contains(">   work  2 tabs, 3 panes  saved"));
    assert!(frame.contains("ATTACH / CREATE"));
    assert!(frame.contains("\x1b[?25l"));
}

#[test]
fn session_manager_renders_rename_input() {
    let windows = vec![test_window(1, "fish", 8, 58)];
    let mut renderer = Renderer::default();
    renderer.set_session_manager(Some(SessionManagerView {
        query: String::new(),
        sessions: vec![SessionInfo {
            name: "work".to_owned(),
            tabs: 1,
            panes: 1,
            connected: true,
            created_at: 1,
            saved: false,
        }],
        selected: 0,
        current: "work".to_owned(),
        rename_input: Some("renamed".to_owned()),
    }));

    let frame = renderer.render(&windows, 0, (60, 12), "normal", None, &[]);
    let frame = String::from_utf8_lossy(&frame);

    assert!(frame.contains("Rename: renamed_"));
}

#[test]
fn session_manager_is_centered_at_half_the_content_size() {
    assert_eq!(session_manager_rect((100, 40)), (25, 10, 50, 20));
    assert_eq!(session_manager_rect((60, 20)), (10, 5, 40, 10));
    assert_eq!(session_manager_rect((30, 6)), (0, 0, 30, 6));
}

#[test]
fn decoder_preserves_large_pastes() {
    let mut decoder = InputDecoder::default();
    let input = vec![b'x'; 64 * 1024];

    assert_eq!(decoder.push(&input), input);
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
    assert!(decoder.flush_deadline().is_some());
    assert_eq!(decoder.push(b"A"), b"\x1b[A");
    assert!(decoder.flush_deadline().is_none());
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
fn kitty_notification_uses_osc_99_with_base64_content_and_bell() {
    let notification = kitty_notification("rustmux-1-2", "done", "finished in 10.0s");
    let notification = String::from_utf8(notification).unwrap();

    assert_eq!(notification.matches("\x1b]99;").count(), 4);
    assert!(notification.contains("i=rustmux-1-2:d=0:f=cnVzdG11eA==:o=always;"));
    assert!(notification.contains("d=0:e=1:p=title;ZG9uZQ=="));
    assert!(notification.contains("d=0:e=1:p=body;ZmluaXNoZWQgaW4gMTAuMHM="));
    assert!(notification.ends_with("\x1b]99;i=rustmux-1-2;\x1b\\\x07"));
}

#[test]
fn semantic_output_capture_reports_only_terminal_bells() {
    let mut capture = SemanticOutputCapture::default();

    capture.process(b"before\x07after");
    assert!(capture.take_bell());
    assert!(!capture.take_bell());

    capture.process(b"\x1b]0;title\x07\x1bPpayload\x07\x1b\\");
    assert!(!capture.take_bell());
}

#[test]
fn long_semantic_command_notification_reaches_the_outer_terminal() {
    let mut capture = SemanticOutputCapture::default();
    capture.process(b"\x1b]133;C;cmdline_url=sleep%2011\x1b\\");
    capture.command_started_at = Some(Instant::now() - Duration::from_secs(11));
    let duration = capture
        .process(b"\x1b]133;D;0\x1b\\")
        .into_iter()
        .next()
        .expect("OSC 133 command completion should produce a duration");
    assert!(duration >= Duration::from_secs(10));

    let notification = kitty_notification(
        "rustmux-test",
        "rustmux: command finished",
        &format!("Window 1 (fish) completed in {}", format_duration(duration)),
    );
    let mut decoder = ServerOutputDecoder::default();
    let split = notification.len() / 2;
    let (first, switch) = decoder.push(&notification[..split]).unwrap();
    assert_eq!(switch, None);
    let (second, switch) = decoder.push(&notification[split..]).unwrap();
    assert_eq!(switch, None);
    let mut visible = first;
    visible.extend(second);

    assert_eq!(visible, notification);
    assert_eq!(visible.last(), Some(&0x07));
    assert_eq!(
        visible
            .windows(5)
            .filter(|part| *part == b"\x1b]99;")
            .count(),
        4
    );
}
