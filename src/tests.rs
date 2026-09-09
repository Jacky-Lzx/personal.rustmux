use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Instant;

use nix::unistd::Pid;

use super::*;
use crate::app::{TextSelection, Window, selected_text, selection_contains, window_history};
use crate::input::{InputDecoder, MouseAction, MousePosition, decode_key, decode_sgr_mouse};
use crate::layout::{
    FloatingLayout, PaneNode, PaneRect, SplitAxis, content_size_for, content_winsize_for,
    floating_layout_for, pane_ids, pane_rects, remove_pane, split_pane, tiled_content_rect_for,
    validate_terminal_size, window_winsize_for,
};
use crate::render::{
    CellStyle, FrameSnapshot, Renderer, render_frame, styled_text_cells, truncate_to_display_width,
};
use crate::terminal::{
    CursorStyleTracker, KittyGraphicsParser, SemanticOutputCapture, TerminalMetadata,
    base64_encode, kitty_graphics_query_response, kitty_graphics_uses_shared_memory,
    kitty_notification, terminal_responses,
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
    let value = content_winsize_for((218, 62), (3706, 2046), false);
    assert_eq!(value.ws_col, 216);
    assert_eq!(value.ws_row, 59);
    assert_eq!(value.ws_xpixel, 3672);
    assert_eq!(value.ws_ypixel, 1947);
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
    };

    assert_eq!(selected_text(&window, selection, (5, 2)), "abcdef");
}

#[test]
fn selection_must_span_multiple_cells_before_copying() {
    let single_cell = TextSelection {
        window_id: 9,
        start: MousePosition { column: 2, row: 1 },
        end: MousePosition { column: 2, row: 1 },
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
    assert!(shown.contains("└─ LOCKED │ copied to system clipboard "));
    assert!(!shown.contains("\x1b[2J"));

    renderer.set_border_status(None);
    let cleared = renderer.render(&windows, 0, (40, 6), "locked", None, &[]);
    let cleared = String::from_utf8(cleared).unwrap();
    assert!(cleared.contains("\x1b[6;1H"));
    assert!(!cleared.contains(CLIPBOARD_STATUS));
}

#[test]
fn status_line_shows_mode_and_key_hints() {
    let window = test_window(1, "fish", 2, 18);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(false, vec!["Ctrl b=mode:normal".to_owned()]);

    let frame = renderer.render(&windows, 0, (20, 5), "locked", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("LOCKED │ Ctrl b="));
    assert!(frame.contains("\x1b[5;1H"));
}

#[test]
fn compact_layout_reclaims_status_row_and_shows_mode_at_top_right() {
    assert_eq!(content_size_for((20, 5), false), (18, 2));
    assert_eq!(content_size_for((20, 5), true), (18, 3));
    assert_eq!(tiled_content_rect_for((20, 5), false).height, 3);
    assert_eq!(tiled_content_rect_for((20, 5), true).height, 4);

    let window = test_window(1, "fish", 3, 18);
    let windows = vec![window];
    let mut renderer = Renderer::default();
    renderer.set_ui(true, Vec::new());

    let frame = renderer.render(&windows, 0, (20, 5), "normal", None, &[]);
    let frame = String::from_utf8(frame).unwrap();

    assert!(frame.contains("\x1b[1;13H\x1b[1;30;42m NORMAL "));
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
fn frame_has_green_border_tabs_and_terminal_contents() {
    let mut first = test_window(1, "fish", 3, 18);
    first.terminal.process(b"hello \x1b[38;2;1;2;3mcolor");
    first.terminal.process(b"\x1b]2;nvim project\x07");
    let second = test_window(2, "fish", 3, 18);

    let windows = [first, second];
    let snapshot = FrameSnapshot::capture(&windows, 0, (20, 5), "locked", None, None);
    let frame = render_frame(&windows, 0, &snapshot, &[]);
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
    assert!(frame.contains(" 1 base "));
    assert!(!frame.contains(" 2 float "));
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
