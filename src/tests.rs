use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Instant;

use nix::unistd::Pid;

use super::*;
use crate::app::{
    InternalDndBridge, RenameEdit, TextSelection, Window, edit_window_name, matching_history_lines,
    matching_session_info, pane_at, preferred_spawn_directory, process_current_directory,
    process_name, rename_tab, selected_text, selection_contains, window_history,
};
use crate::input::{
    DecodedKey, InputDecoder, MouseAction, MousePosition, decode_focus_event, decode_key,
    decode_sgr_mouse, sgr_mouse_at,
};
use crate::layout::{
    Direction, FloatingLayout, PaneNode, PaneRect, SplitAxis, content_size_for,
    content_winsize_for, floating_layout_for, move_item, pane_ids, pane_rects, pane_resize_handle,
    rect_in_direction, remove_pane, resize_pane, resize_pane_to, split_pane, swap_panes,
    tiled_content_rect_for, validate_terminal_size, window_winsize_for,
};
use crate::render::{
    CellStyle, FrameSnapshot, HelpView, Renderer, SessionManagerView, compact_status_hints,
    help_rect, notification_rect, render_frame, session_manager_rect, styled_text_cells,
    truncate_to_display_width,
};
use crate::session::ServerOutputDecoder;
use crate::session::SessionInfo;
use crate::terminal::{
    CursorStyleTracker, HyperlinkTracker, InputModeTracker, KittyDndEvent, KittyDndParser,
    KittyDndRegistration, KittyGraphicsParser, KittyIpcParser, SemanticOutputCapture,
    TerminalMetadata, TerminalOscTracker, base64_encode, format_duration, kitty_dnd_command,
    kitty_dnd_data_response, kitty_dnd_drag_start_position, kitty_dnd_for_child, kitty_dnd_id,
    kitty_dnd_registration, kitty_dnd_with_id, kitty_graphics_query_response,
    kitty_graphics_uses_shared_memory, kitty_ipc_for_child, kitty_ipc_with_pane,
    kitty_notification, osc7_path, terminal_parser_size, terminal_responses,
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
        spawn_directory: None,
        startup_command: None,
        terminal: vt100::Parser::new_with_callbacks(
            rows,
            columns,
            SCROLLBACK_LINES,
            TerminalMetadata::default(),
        ),
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
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn process_name_resolves_the_running_process() {
    assert!(process_name(Pid::this()).is_some());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn process_current_directory_resolves_the_running_process() {
    assert_eq!(
        process_current_directory(Pid::this()),
        std::env::current_dir().ok()
    );
}

#[test]
fn yazi_directory_overrides_stale_terminal_directory() {
    let tracked = PathBuf::from("/shell/directory");
    let foreground = PathBuf::from("/yazi/directory");

    assert_eq!(
        preferred_spawn_directory(
            Some(tracked.clone()),
            Some("yazi"),
            Some(foreground.clone())
        ),
        Some(foreground)
    );
    assert_eq!(
        preferred_spawn_directory(
            Some(tracked.clone()),
            Some("nvim"),
            Some(PathBuf::from("/editor/directory"))
        ),
        Some(tracked.clone())
    );
    assert_eq!(
        preferred_spawn_directory(Some(tracked.clone()), Some("yazi"), None),
        Some(tracked)
    );
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
fn nested_session_detection_only_applies_to_session_entry_commands() {
    for arguments in [
        &["rustmux"][..],
        &["rustmux", "--session", "work"],
        &["rustmux", "new-session", "work"],
        &["rustmux", "attach", "work"],
    ] {
        assert!(starts_session(&Cli::try_parse_from(arguments).unwrap()));
    }

    for arguments in [
        &["rustmux", "list-sessions"][..],
        &["rustmux", "kill-session", "work"],
        &["rustmux", "default-config"],
        &["rustmux", "check-config"],
    ] {
        assert!(!starts_session(&Cli::try_parse_from(arguments).unwrap()));
    }
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
    assert_eq!(decode_key(b"\x1b[97:65;10:3;65u").0.name, "super A");
    assert_eq!(decode_key(b"\x1b[97:65;10:3;65u").0.event_type, 3);
    assert_eq!(decode_key(b"\x1b[1;2:2A").0.name, "up");
    assert_eq!(decode_key(b"\x1b[27;3;120~").0.name, "alt x");
    assert_eq!(decode_key(b"G").0.name, "G");
}

#[test]
fn window_name_editor_accepts_text_backspace_confirm_and_cancel() {
    let mut name = "fish".to_owned();
    let text = DecodedKey {
        name: "终".to_owned(),
        raw: "终".as_bytes().to_vec(),
        event_type: 1,
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
fn window_name_editor_accepts_kitty_text_and_ignores_key_releases() {
    let mut name = String::new();
    let (press, _) = decode_key(b"\x1b[121u");
    let (release, _) = decode_key(b"\x1b[121;1:3u");

    assert_eq!(edit_window_name(&mut name, &press), RenameEdit::Continue);
    assert_eq!(name, "y");
    assert_eq!(edit_window_name(&mut name, &release), RenameEdit::Continue);
    assert_eq!(name, "y");
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
    let window = test_window(1, "fish", 1, 38);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(false, vec!["Ctrl b=mode:normal".to_owned()]);

    let frame = renderer.render(&windows, 0, (40, 5), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains(" LOCKED "));
    assert!(frame.contains("48;2;243;139;168m"));
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
    assert!(compact.contains(&("1/2".to_owned(), "WINDOW".to_owned())));
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
    let cells = styled_text_cells(
        "a界b",
        3,
        CellStyle::border(&crate::theme::Theme::default()),
    );
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
fn directional_focus_only_selects_panes_on_the_requested_axis() {
    let left = PaneRect {
        column: 0,
        row: 0,
        width: 40,
        height: 20,
    };
    let upper_right = PaneRect {
        column: 40,
        row: 0,
        width: 40,
        height: 10,
    };
    let lower_right = PaneRect {
        column: 40,
        row: 10,
        width: 40,
        height: 10,
    };

    assert!(rect_in_direction(left, upper_right, Direction::Right));
    assert!(!rect_in_direction(left, lower_right, Direction::Down));
    assert!(!rect_in_direction(left, upper_right, Direction::Up));
    assert!(rect_in_direction(upper_right, lower_right, Direction::Down));
    assert!(rect_in_direction(lower_right, left, Direction::Left));
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
fn changing_focused_pane_is_rendered_incrementally() {
    let mut left = test_window(1, "left", 10, 18);
    left.tab_id = 1;
    left.pane_framed = true;
    left.pane_rect = PaneRect {
        column: 0,
        row: 0,
        width: 20,
        height: 12,
    };
    left.terminal.process(b"left pane");
    let mut right = test_window(2, "right", 10, 18);
    right.tab_id = 1;
    right.pane_framed = true;
    right.pane_rect = PaneRect {
        column: 20,
        row: 0,
        width: 20,
        height: 12,
    };
    right.terminal.process(b"right pane");
    let windows = vec![left, right];
    let mut renderer = Renderer::default();

    renderer.render(&windows, 0, (40, 15), "locked", None, &[]);
    let update = renderer.render(&windows, 1, (40, 15), "locked", None, &[]);

    assert!(!update.windows(4).any(|part| part == b"\x1b[2J"));
    assert!(
        update
            .windows(b"\x1b]0;rustmux:2\x07".len())
            .any(|part| part == b"\x1b]0;rustmux:2\x07")
    );
    assert!(update.starts_with(b"\x1b[?2026h"));
    assert!(update.ends_with(b"\x1b[?2026l"));
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
fn renderer_uses_kitty_cursor_default_until_a_child_overrides_it() {
    let window = test_window(1, "fish", 3, 18);
    let mut windows = vec![window];
    let mut renderer = Renderer::default();
    let initial = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);

    assert!(initial.windows(7).any(|part| part == b"\x1b]112\x1b\\"));
    assert!(
        !initial
            .windows(b"\x1b]12;".len())
            .any(|part| part == b"\x1b]12;")
    );

    windows[0].terminal_osc.process(b"\x1b]12;#123456\x1b\\");
    let custom = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
    assert!(
        custom
            .windows(b"\x1b]12;#123456\x1b\\".len())
            .any(|part| part == b"\x1b]12;#123456\x1b\\")
    );

    windows[0].terminal_osc.process(b"\x1b]112\x1b\\");
    let reset = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
    assert!(reset.windows(7).any(|part| part == b"\x1b]112\x1b\\"));
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
    assert_eq!(first.events, [KittyDndEvent::Terminal(b"before".to_vec())]);
    assert!(parser.flush_deadline().is_some());

    let second = parser.process(b"2;t=a;text/uri-list\x1b");
    assert!(second.events.is_empty());
    assert!(parser.flush_deadline().is_none());

    let third = parser.process(b"\\after");
    assert_eq!(
        third.events,
        [
            KittyDndEvent::Command(b"\x1b]72;t=a;text/uri-list\x1b\\".to_vec()),
            KittyDndEvent::Terminal(b"after".to_vec()),
        ]
    );

    let ordinary = parser.process(b"\x1b]2;title\x1b\\");
    assert_eq!(
        ordinary.events,
        [KittyDndEvent::Terminal(b"\x1b]2;title\x1b\\".to_vec())]
    );
}

#[test]
fn kitty_dnd_parser_preserves_mouse_press_before_drag_offer() {
    let mut parser = KittyDndParser::default();
    let mouse = b"\x1b[<0;31;9M";
    let offer = b"\x1b]72;t=o:i=1:x=30:y=8:X=300:Y=160\x1b\\";
    let output = parser.process(&[mouse.as_slice(), offer.as_slice()].concat());

    assert_eq!(
        output.events,
        [
            KittyDndEvent::Terminal(mouse.to_vec()),
            KittyDndEvent::Command(offer.to_vec()),
        ]
    );
}

#[test]
fn kitty_dnd_parser_releases_an_ambiguous_escape() {
    let mut parser = KittyDndParser::default();

    let output = parser.process(b"\x1b");
    assert!(output.events.is_empty());
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
fn kitty_dnd_drag_start_uses_position_instead_of_a_stale_pane_id() {
    let stale = b"\x1b]72;t=o:i=1:x=56:y=8:X=560:Y=160\x1b\\";

    assert_eq!(kitty_dnd_drag_start_position(stale), Some((56, 8)));
    assert_eq!(kitty_dnd_id(stale), Some(1));
    assert_eq!(
        kitty_dnd_drag_start_position(b"\x1b]72;t=o:o=3:i=2;text/uri-list\x1b\\"),
        None
    );
}

#[test]
fn kitty_dnd_commands_expose_data_transfer_metadata() {
    let command = kitty_dnd_command(b"\x1b]72;t=p:x=0:m=1:i=2;ZmlsZTovLy90bXAvYQ==\x1b\\").unwrap();
    assert_eq!(command.kind, Some('p'));
    assert_eq!(command.client_id, Some(2));
    assert_eq!(command.x, Some(0));
    assert_eq!(command.y, None);
    assert!(command.more);
    assert_eq!(command.payload, b"ZmlsZTovLy90bXAvYQ==");
    assert_eq!(
        kitty_dnd_data_response(1, command.payload).unwrap(),
        b"\x1b]72;t=r:x=1:m=1;ZmlsZTovLy90bXAvYQ==\x1b\\\x1b]72;t=r:x=1:m=0;\x1b\\"
    );
}

#[test]
fn internal_dnd_bridge_serves_yazi_uri_list_to_another_pane() {
    let mut bridge = InternalDndBridge::default();
    bridge.begin_source(1, b"text/uri-list text/plain");
    bridge.cache_pre_sent_data(1, Some(0), false, b"ZmlsZTovLy90bXAvYQ==");
    bridge.begin_target(2, b"text/uri-list");

    assert!(bridge.routes_response_to_source(2, Some('m')));
    assert!(bridge.routes_response_to_source(2, Some('r')));
    assert!(!bridge.routes_response_to_source(1, Some('m')));
    assert_eq!(
        bridge.data_response(2, 1).unwrap(),
        b"\x1b]72;t=r:x=1:m=1;ZmlsZTovLy90bXAvYQ==\x1b\\\x1b]72;t=r:x=1:m=0;\x1b\\"
    );
    assert!(bridge.data_response(1, 1).is_none());
}

#[test]
fn internal_dnd_bridge_matches_target_mime_indexes_to_source_indexes() {
    let mut bridge = InternalDndBridge::default();
    bridge.begin_source(7, b"text/plain text/uri-list");
    bridge.cache_pre_sent_data(7, Some(1), true, b"ZmlsZTov");
    bridge.cache_pre_sent_data(7, None, false, b"Ly90bXAvYg==");
    bridge.begin_target(9, b"text/uri-list text/plain");

    assert_eq!(
        bridge.data_response(9, 1).unwrap(),
        b"\x1b]72;t=r:x=1:m=1;ZmlsZTovLy90bXAvYg==\x1b\\\x1b]72;t=r:x=1:m=0;\x1b\\"
    );
    assert!(bridge.data_response(9, 2).is_none());
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
fn kitty_keyboard_modes_are_scoped_to_each_screen_and_answer_queries() {
    let mut modes = InputModeTracker::default();
    assert!(modes.process(b"\x1b[=3u\x1b[").is_empty());
    assert_eq!(modes.process(b"?u"), b"\x1b[?3u");
    assert_eq!(modes.keyboard_flags(), 3);

    modes.process(b"\x1b[>7u");
    assert_eq!(modes.keyboard_flags(), 7);
    modes.process(b"\x1b[<u");
    assert_eq!(modes.keyboard_flags(), 3);

    modes.process(b"\x1b[?1049h\x1b[=31u\x1b[?1004h");
    assert_eq!(modes.keyboard_flags(), 31);
    assert!(modes.focus_reporting());
    modes.process(b"\x1b[?1049l");
    assert_eq!(modes.keyboard_flags(), 3);
}

#[test]
fn terminal_osc_colors_and_pointer_shapes_are_stateful() {
    let mut osc = TerminalOscTracker::default();
    assert_eq!(osc.cursor(), None);
    assert!(osc.process(b"\x1b]11;?").responses.is_empty());
    let response = osc.process(b"\x1b\\");
    assert_eq!(response.responses, b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\");

    osc.process(b"\x1b]30001\x1b\\\x1b]11;#123456\x07");
    assert_eq!(osc.background(), (0x12, 0x34, 0x56));
    osc.process(b"\x1b]30101\x1b\\");
    assert_eq!(osc.background(), (30, 30, 46));

    osc.process(b"\x1b]12;#123456\x1b\\");
    assert_eq!(osc.cursor(), Some((0x12, 0x34, 0x56)));
    osc.process(b"\x1b]112\x1b\\");
    assert_eq!(osc.cursor(), None);

    let changed = osc.process(b"\x1b]22;>pointer\x1b\\");
    assert!(changed.pointer_changed);
    assert_eq!(osc.pointer_shape(), "pointer");
    assert_eq!(
        osc.process(b"\x1b]22;?__current__\x1b\\").responses,
        b"\x1b]22;pointer\x1b\\"
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
        last_connected_at: 1,
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
        searching: true,
        keys: Config::test_defaults().session_manager_keys(),
        query: "wo".to_owned(),
        sessions: vec![SessionInfo {
            name: "work".to_owned(),
            tabs: 2,
            panes: 3,
            connected: false,
            created_at: 1,
            last_connected_at: 1,
            saved: true,
        }],
        selected: 0,
        current: "personal".to_owned(),
        rename_input: None,
    }));

    let frame = renderer.render(&windows, 0, (60, 12), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("Session Manager"));
    assert!(frame.contains("1 SESSION"));
    assert!(frame.contains("Search: "));
    assert!(frame.contains("wo"));
    assert!(frame.contains("SESSION"));
    assert!(frame.contains("LAYOUT"));
    assert!(frame.contains("work"));
    assert!(frame.contains("2t · 3p"));
    assert!(frame.contains("[SAVED]"));
    assert!(frame.contains("<Enter>"));
    assert!(frame.contains("Open/Create"));
    assert!(!frame.contains('├'));
    assert!(!frame.contains('┤'));
    assert!(frame.contains("\x1b[?25l"));
}

#[test]
fn session_manager_sorts_by_connection_and_selects_recent_detached_session() {
    let make = |name: &str, connected, last_connected_at| SessionInfo {
        name: name.to_owned(),
        tabs: 1,
        panes: 1,
        connected,
        created_at: 0,
        last_connected_at,
        saved: false,
    };
    let mut sessions = vec![
        make("unknown", false, 0),
        make("recent", false, 30),
        make("current", true, 10),
        make("attached", true, 20),
        make("older", false, 15),
        make("attached-recent", true, 40),
    ];
    crate::session::sort_session_info(&mut sessions, "current");
    assert_eq!(
        sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        [
            "current",
            "attached-recent",
            "attached",
            "recent",
            "older",
            "unknown"
        ]
    );
    assert_eq!(
        crate::app::default_session_selection(&sessions, "current"),
        3
    );
    assert_eq!(
        crate::app::default_session_selection(&sessions[..3], "current"),
        0
    );
    assert_eq!(crate::app::default_session_selection(&[], "current"), 0);
}

#[test]
fn session_manager_hides_search_input_until_searching() {
    let windows = vec![test_window(1, "fish", 20, 98)];
    let mut renderer = Renderer::default();
    let mut keys = Config::test_defaults().session_manager_keys();
    keys.insert("search".to_owned(), vec!["ctrl f".to_owned()]);
    renderer.set_session_manager(Some(SessionManagerView {
        searching: false,
        keys,
        query: String::new(),
        sessions: vec![SessionInfo {
            name: "first".to_owned(),
            tabs: 1,
            panes: 1,
            connected: false,
            created_at: 1,
            last_connected_at: 1,
            saved: false,
        }],
        selected: 0,
        current: "other".to_owned(),
        rename_input: None,
    }));
    let frame = renderer.render(&windows, 0, (100, 24), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();
    assert!(!frame.contains("Search: "));
    assert!(!frame.contains("Session: "));
    assert!(frame.contains("<Ctrl f>"));
    assert!(frame.contains("first"));
    let windows = vec![test_window(1, "fish", 36, 213)];
    let frame = renderer.render(&windows, 0, (215, 40), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();
    assert!(frame.contains("LAST CONNECTED"));
    assert!(frame.contains("CREATED"));
}

#[test]
fn session_manager_renders_rename_input() {
    let windows = vec![test_window(1, "fish", 8, 58)];
    let mut renderer = Renderer::default();
    renderer.set_session_manager(Some(SessionManagerView {
        searching: true,
        keys: Config::test_defaults().session_manager_keys(),
        query: String::new(),
        sessions: vec![SessionInfo {
            name: "work".to_owned(),
            tabs: 1,
            panes: 1,
            connected: true,
            created_at: 1,
            last_connected_at: 1,
            saved: false,
        }],
        selected: 0,
        current: "work".to_owned(),
        rename_input: Some("renamed".to_owned()),
    }));

    let frame = renderer.render(&windows, 0, (60, 12), "normal", None, &[]);
    let frame = String::from_utf8_lossy(&frame);

    assert!(frame.contains("Rename: "));
    assert!(frame.contains("renamed"));
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
fn focus_events_are_identified_before_key_passthrough() {
    let mut decoder = InputDecoder::default();
    assert!(decoder.push(b"\x1b[").is_empty());
    let decoded = decoder.push(b"I\x1b[Otext");

    assert_eq!(decode_focus_event(&decoded), Some((true, 3)));
    assert_eq!(decode_focus_event(&decoded[3..]), Some((false, 3)));
    assert_eq!(decode_focus_event(&decoded[6..]), None);
    assert_eq!(decode_focus_event(b"\x9bI"), Some((true, 2)));
    assert_eq!(decode_focus_event(b"\x9bO"), Some((false, 2)));
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
fn kitty_ipc_parser_extracts_split_clipboard_and_file_commands() {
    let mut parser = KittyIpcParser::default();
    assert_eq!(
        parser.process(b"text\x1b]5522;type=read:id=a;").terminal,
        b"text"
    );
    let output = parser.process(b"\x1b\\tail\x1b]5113;id=b;action=send\x1b\\");

    assert_eq!(output.terminal, b"tail");
    assert_eq!(output.commands.len(), 2);
    assert!(output.commands[0].starts_with(b"\x1b]5522;"));
    assert!(output.commands[1].starts_with(b"\x1b]5113;"));
}

#[test]
fn kitty_ipc_parser_releases_an_ambiguous_escape() {
    let mut parser = KittyIpcParser::default();

    let output = parser.process(b"\x1b");
    assert!(output.terminal.is_empty());
    assert!(output.commands.is_empty());
    assert!(parser.flush_deadline().is_some());
    assert_eq!(parser.flush(), b"\x1b");
    assert!(parser.flush_deadline().is_none());
}

#[test]
fn kitty_ipc_ids_round_trip_through_a_pane_namespace() {
    for command in [
        b"\x1b]5522;type=write:mime=text/plain:id=clip;aGVsbG8=\x1b\\".as_slice(),
        b"\x1b]5113;action=send;id=file;name=Zm9v\x1b\\".as_slice(),
        b"\x1b]5522;type=read:mime=text/plain;\x1b\\".as_slice(),
    ] {
        let tagged = kitty_ipc_with_pane(command, 42).expect("valid Kitty IPC");
        let (pane, restored) = kitty_ipc_for_child(&tagged).expect("namespaced response");
        assert_eq!(pane, 42);
        assert_eq!(restored, command);
    }
}

#[test]
fn input_modes_virtualize_rich_clipboard_paste_and_answer_queries() {
    let mut modes = InputModeTracker::default();
    assert_eq!(modes.process(b"\x1b[?5522$p"), b"\x1b[?5522;2$y");

    modes.process(b"\x1b[?5522h");
    assert!(modes.rich_clipboard_paste());
    assert_eq!(modes.process(b"\x1b[?5522$p"), b"\x1b[?5522;1$y");

    modes.process(b"\x1b[?5522l");
    assert!(!modes.rich_clipboard_paste());
}

#[test]
fn osc8_hyperlinks_follow_rendered_cells_and_are_namespaced_per_pane() {
    let mut terminal = vt100::Parser::new_with_callbacks(2, 20, 0, TerminalMetadata::default());
    let mut hyperlinks = HyperlinkTracker::default();
    hyperlinks.process(
        b"\x1b]8;id=source;https://example.com\x1b\\link\x1b]8;;\x1b\\ plain",
        &mut terminal,
    );

    let first = hyperlinks.osc8_at(0, 0, 7).expect("linked cell");
    assert!(first.contains("\x1b]8;id=rustmux-7-"));
    assert!(first.ends_with(";https://example.com\x1b\\"));
    assert_eq!(hyperlinks.osc8_at(0, 5, 7), None);
}

#[test]
fn osc8_hyperlink_tracker_handles_a_fragmented_introducer() {
    let mut terminal = vt100::Parser::new_with_callbacks(2, 20, 0, TerminalMetadata::default());
    let mut hyperlinks = HyperlinkTracker::default();
    hyperlinks.process(b"plain\x1b]8", &mut terminal);
    hyperlinks.process(b";;https://example.com\x1b\\linked", &mut terminal);

    assert_eq!(hyperlinks.osc8_at(0, 0, 3), None);
    assert!(hyperlinks.osc8_at(0, 5, 3).is_some());
}

#[test]
fn rendered_frame_reemits_and_closes_osc8_hyperlinks() {
    let mut window = test_window(9, "links", 3, 30);
    let output = b"\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\";
    window.hyperlinks.process(output, &mut window.terminal);

    let mut renderer = Renderer::default();
    let frame =
        String::from_utf8(renderer.render(&[window], 0, (32, 7), "normal", None, &[])).unwrap();
    assert!(frame.contains("\x1b]8;id=rustmux-9-"));
    assert!(frame.contains(";https://example.com\x1b\\"));
    assert!(frame.contains("click\x1b]8;;\x1b\\"));
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

#[test]
fn directory_falls_back_to_process_without_shell_integration() {
    let directory = PathBuf::from("/workspace");
    assert_eq!(
        preferred_spawn_directory(None, Some("sh"), Some(directory.clone())),
        Some(directory)
    );
    assert_eq!(preferred_spawn_directory(None, None, None), None);
}

#[test]
fn detached_creation_is_allowed_inside_an_existing_session() {
    let cli = Cli::try_parse_from(["rustmux", "new-session", "other", "--detached"]).unwrap();
    assert!(!starts_session(&cli));
}

#[test]
fn theme_changes_redraw_ui_and_preserve_application_colors_and_cursor() {
    let mut window = test_window(1, "shell", 20, 78);
    window.terminal.process(b"\x1b[38;2;7;8;9mapplication");
    window.terminal_osc.process(b"\x1b]12;#0a0b0c\x07");
    let windows = [window];
    let mut renderer = Renderer::default();
    renderer.render(&windows, 0, (80, 24), "locked", None, &[]);
    let theme = crate::theme::Theme {
        accent: (1, 2, 3),
        background: (4, 5, 6),
        ..Default::default()
    };
    renderer.set_theme(theme);
    let frame = renderer.render(&windows, 0, (80, 24), "locked", None, &[]);
    let text = String::from_utf8_lossy(&frame);
    assert!(
        text.contains("\x1b[2J"),
        "theme updates must repaint unchanged UI"
    );
    assert!(text.contains("38;2;1;2;3"));
    assert!(text.contains("48;2;4;5;6"));
    assert!(text.contains("38;2;7;8;9"));
    assert!(text.contains("\x1b]12;#0a0b0c"));
    assert!(!text.contains("\x1b]10;") && !text.contains("\x1b]11;"));
    renderer.set_theme(theme);
    assert!(
        renderer
            .render(&windows, 0, (80, 24), "locked", None, &[])
            .is_empty()
    );
    renderer.set_help(Some(HelpView {
        mode: "locked".to_owned(),
        hints: vec!["Ctrl b: normal".to_owned()],
    }));
    let frame = renderer.render(&windows, 0, (80, 24), "locked", None, &[]);
    assert!(String::from_utf8_lossy(&frame).contains("48;2;4;5;6"));
    renderer.set_theme(crate::theme::Theme::default());
    let frame = renderer.render(&windows, 0, (80, 24), "locked", None, &[]);
    assert!(String::from_utf8_lossy(&frame).contains("48;2;30;30;46"));
}

#[test]
fn session_busy_response_is_recognized_at_every_packet_boundary() {
    for split in 0..=SERVER_SESSION_BUSY.len() {
        let mut decoder = ServerOutputDecoder::default();
        let (first, target) = decoder.push(&SERVER_SESSION_BUSY[..split]).unwrap();
        assert!(first.is_empty() && target.is_none());
        let (second, target) = decoder.push(&SERVER_SESSION_BUSY[split..]).unwrap();
        assert!(second.is_empty() && target.is_none());
        assert!(decoder.busy);
    }
}

#[test]
fn status_click_targets_match_rendered_aliases_numbers_and_wide_keys() {
    use crate::render::StatusClick;
    let width = 240;
    let windows = [test_window(1, "shell", 22, width - 2)];
    let mut renderer = Renderer::default();
    renderer.set_ui(
        false,
        [
            "n=next-window + mode:locked",
            "p=previous-window + mode:locked",
            "1=window:1",
            "2=window:2",
            "3=window:3",
            "界=close-pane",
            "==focus-next-pane",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    let frame = renderer.render(&windows, 0, (width, 24), "normal", None, &[]);
    let mut parser = vt100::Parser::new(24, width, 0);
    parser.process(&frame);
    let bottom = parser.screen().rows(0, width).last().unwrap();
    let column = |label: &str| -> u16 {
        let offset = bottom.find(label).unwrap();
        unicode_width::UnicodeWidthStr::width(&bottom[..offset]) as u16 + 1
    };
    for (label, offset, key) in [
        ("n/p", 0, "n"),
        ("n/p", 2, "p"),
        ("1/2/3", 0, "1"),
        ("1/2/3", 2, "2"),
        ("1/2/3", 4, "3"),
        ("界", 0, "界"),
        ("界", 1, "界"),
        (" = ", 1, "="),
    ] {
        assert_eq!(
            renderer.status_click_at((column(label) + offset, 24)),
            Some(StatusClick::Key {
                mode: "normal".to_owned(),
                key: key.to_owned()
            })
        );
    }
    assert_eq!(
        renderer.status_click_at((column("CLOSE PANE"), 24)),
        Some(StatusClick::Key {
            mode: "normal".to_owned(),
            key: "界".to_owned()
        })
    );
    assert!(renderer.status_click_at((1, 24)).is_none());
    assert!(renderer.status_click_at((240, 24)).is_none());
    assert!(renderer.status_click_at((column("n/p"), 23)).is_none());
    renderer.set_ui(true, vec!["n=next-window".to_owned()]);
    renderer.render(&windows, 0, (width, 24), "normal", None, &[]);
    assert!(!renderer.status_bar_contains((10, 24)));
}

#[test]
fn status_overflow_clicks_open_help_and_hidden_hints_are_not_targets() {
    use crate::render::StatusClick;
    let windows = [test_window(1, "shell", 20, 58)];
    let mut renderer = Renderer::default();
    renderer.set_ui(
        false,
        [
            "c=new-window",
            "x=close-window",
            "d=detach",
            "s=switch-session",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    let frame = renderer.render(&windows, 0, (60, 24), "normal", None, &[]);
    let mut parser = vt100::Parser::new(24, 60, 0);
    parser.process(&frame);
    let bottom = parser.screen().rows(0, 60).last().unwrap();
    let offset = bottom.find("MORE").unwrap();
    let column = unicode_width::UnicodeWidthStr::width(&bottom[..offset]) as u16 + 1;
    assert_eq!(
        renderer.status_click_at((column, 24)),
        Some(StatusClick::Help)
    );
    assert!((1..=60).all(|column| !matches!(renderer.status_click_at((column, 24)), Some(StatusClick::Key { key, .. }) if key == "d")));
    renderer.set_ui(false, vec![]);
    renderer.render(&windows, 0, (60, 24), "normal", None, &[]);
    assert!(renderer.status_click_at((column, 24)).is_none());
}

#[test]
fn saved_scrollback_uses_primary_screen_without_disturbing_alternate_screen() {
    let mut terminal = vt100::Parser::new_with_callbacks(3, 40, 10, TerminalMetadata::default());
    terminal.process(b"old\r\nkeep one\r\nkeep two\r\nkeep three");
    terminal.process(b"\x1b[?1049h\x1b[2J\x1b[Htemporary TUI");
    let before = terminal.screen().state_formatted();
    let lines = crate::app::snapshot_scrollback(terminal.screen(), 3, false);
    assert_eq!(lines, ["keep one", "keep two", "keep three"]);
    assert_eq!(terminal.screen().state_formatted(), before);
    assert!(terminal.screen().alternate_screen());
}

#[test]
fn restored_scrollback_is_bounded_and_leaves_a_clean_live_screen() {
    let mut terminal = vt100::Parser::new_with_callbacks(3, 40, 3, TerminalMetadata::default());
    let lines = vec![
        "discard".to_owned(),
        "中文 output".to_owned(),
        "last line".to_owned(),
    ];
    crate::app::restore_scrollback(
        &mut terminal,
        &lines,
        2,
        crate::session::ScrollbackFormat::Plain,
    );
    assert_eq!(terminal.screen().contents(), "");
    assert_eq!(terminal.screen().cursor_position(), (0, 0));
    assert_eq!(
        crate::app::snapshot_scrollback(terminal.screen(), 3, false),
        ["中文 output", "last line"]
    );
    terminal.process(b"fresh shell");
    assert_eq!(terminal.screen().contents(), "fresh shell");
}

#[test]
fn restored_scrollback_does_not_interpret_embedded_terminal_controls() {
    let mut terminal = vt100::Parser::new_with_callbacks(3, 80, 10, TerminalMetadata::default());
    crate::app::restore_scrollback(
        &mut terminal,
        &["before\x1b[?1049h\x07after".to_owned()],
        10,
        crate::session::ScrollbackFormat::Plain,
    );
    assert!(!terminal.screen().alternate_screen());
    assert_eq!(
        crate::app::snapshot_scrollback(terminal.screen(), 10, false),
        ["before[?1049hafter"]
    );
}

#[test]
fn colored_scrollback_preserves_cell_styles_unicode_and_colored_spaces() {
    use crate::session::ScrollbackFormat;
    let mut source = vt100::Parser::new_with_callbacks(3, 40, 10, TerminalMetadata::default());
    source.process(
        "\x1b[1;2;3;4;7;38;2;12;34;56;48;5;123m中e\u{301} \x1b[0m plain\r\n\x1b[44m\x1b[2K\x1b[0m"
            .as_bytes(),
    );
    let expected = source.screen().clone();
    source.process(b"\x1b[?1049h\x1b[2Jtemporary application");
    let saved = crate::app::snapshot_scrollback(source.screen(), 10, true);
    assert_eq!(saved.len(), 2);
    let mut restored = vt100::Parser::new_with_callbacks(3, 40, 10, TerminalMetadata::default());
    crate::app::restore_scrollback(&mut restored, &saved, 10, ScrollbackFormat::Ansi);
    assert_eq!(restored.screen().contents(), "");
    assert_eq!(restored.screen().fgcolor(), vt100::Color::Default);
    assert_eq!(restored.screen().bgcolor(), vt100::Color::Default);
    restored.screen_mut().set_scrollback(usize::MAX);
    for row in 0..2 {
        for column in 0..40 {
            let actual = restored.screen().cell(row, column).unwrap();
            let expected = expected.cell(row, column).unwrap();
            assert_eq!(CellStyle::from(actual), CellStyle::from(expected));
            assert_eq!(actual.contents().trim_end(), expected.contents().trim_end());
            assert_eq!(
                actual.is_wide_continuation(),
                expected.is_wide_continuation()
            );
        }
    }
}

#[test]
fn colored_scrollback_encodes_style_runs_and_respects_the_line_limit() {
    let mut source = vt100::Parser::new_with_callbacks(3, 80, 10, TerminalMetadata::default());
    source.process(b"discard\r\n\x1b[31mred red red\x1b[32mgreen green\x1b[0m\r\nlast");
    let saved = crate::app::snapshot_scrollback(source.screen(), 2, true);
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0].matches("38;5;").count(), 2);
    assert_eq!(saved[1], "last");
    assert_eq!(
        crate::app::snapshot_scrollback(source.screen(), 2, false),
        ["red red redgreen green", "last"]
    );
}

#[test]
fn colored_scrollback_restoration_accepts_only_sgr_and_keeps_the_live_screen_plain() {
    let mut terminal = vt100::Parser::new_with_callbacks(3, 80, 10, TerminalMetadata::default());
    crate::app::restore_scrollback(
        &mut terminal,
        &["\x1b[31mred\x1b[?1049h\x1b]52;c;AAAA\x07".to_owned()],
        10,
        crate::session::ScrollbackFormat::Ansi,
    );
    assert!(!terminal.screen().alternate_screen());
    assert_eq!(terminal.screen().fgcolor(), vt100::Color::Default);
    assert_eq!(terminal.screen().cursor_position(), (0, 0));
    terminal.process(b"new shell");
    assert_eq!(
        terminal.screen().cell(0, 0).unwrap().fgcolor(),
        vt100::Color::Default
    );
    terminal.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(
        terminal.screen().cell(0, 0).unwrap().fgcolor(),
        vt100::Color::Idx(1)
    );
}
