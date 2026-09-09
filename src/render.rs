use std::io::Write;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{TextSelection, Window, selection_contains};
use crate::layout::{
    FloatingLayout, content_size_for, floating_layout_for, pane_pty_size, tiled_content_rect_for,
};

#[derive(Default)]
pub(super) struct Renderer {
    previous: Option<FrameSnapshot>,
    border_status: Option<String>,
    compact: bool,
    mode_hints: Vec<String>,
}

impl Renderer {
    pub(super) fn invalidate(&mut self) {
        self.previous = None;
    }

    pub(super) fn set_border_status(&mut self, status: Option<&str>) {
        self.border_status = status.map(str::to_owned);
    }

    pub(super) fn set_ui(&mut self, compact: bool, mode_hints: Vec<String>) {
        self.compact = compact;
        self.mode_hints = mode_hints;
    }

    pub(super) fn render(
        &mut self,
        windows: &[Window],
        active: usize,
        terminal_size: (u16, u16),
        mode: &str,
        selection: Option<&TextSelection>,
        graphics: &[Vec<u8>],
    ) -> Vec<u8> {
        let current = FrameSnapshot::capture_with_ui(
            windows,
            active,
            terminal_size,
            RenderUi {
                mode,
                compact: self.compact,
                mode_hints: &self.mode_hints,
                border_status: self.border_status.as_deref(),
            },
            selection,
        );
        let Some(previous) = &self.previous else {
            let output = render_frame(windows, active, &current, graphics);
            self.previous = Some(current);
            return output;
        };

        if previous.terminal_size != current.terminal_size
            || previous.content_size != current.content_size
            || previous.content_origin != current.content_origin
            || previous.outer_border != current.outer_border
            || previous.compact != current.compact
            || previous.active_id != current.active_id
            || previous.tabs != current.tabs
            || previous.cells.len() != current.cells.len()
        {
            let output = render_frame(windows, active, &current, graphics);
            self.previous = Some(current);
            return output;
        }

        let (content_columns, content_rows) = current.content_size;
        let columns = usize::from(content_columns);
        let mut changes = Vec::new();
        for row in 0..usize::from(content_rows) {
            let start = row * columns;
            let before = &previous.cells[start..start + columns];
            let after = &current.cells[start..start + columns];
            let Some(mut first) = before.iter().zip(after).position(|(a, b)| a != b) else {
                continue;
            };
            let last = before
                .iter()
                .zip(after)
                .rposition(|(a, b)| a != b)
                .expect("a changed row has a final changed cell");
            if after[first].wide_continuation && first > 0 {
                first -= 1;
            }
            changes.push((row, first, last));
        }

        let cells_changed = !changes.is_empty();
        let state_changed = previous.terminal_state != current.terminal_state;
        let graphics_changed = !graphics.is_empty();
        let history_changed = previous.history_mode != current.history_mode
            || previous.history_offset != current.history_offset;
        let mode_changed = previous.mode != current.mode;
        let title_changed = previous.terminal_title != current.terminal_title;
        let status_changed = previous.border_status != current.border_status
            || previous.mode_hints != current.mode_hints;
        let mut output = Vec::new();
        // Keep potentially multi-megabyte image uploads outside synchronized
        // text updates. Some terminals cap or time out synchronized buffers;
        // the following text frame will expose the uploaded image atomically
        // when it paints the Unicode placeholders.
        if graphics_changed && !previous.terminal_state.hide_cursor {
            output.extend_from_slice(b"\x1b[?25l");
        }
        append_graphics(
            &mut output,
            graphics,
            &current.terminal_state,
            current.content_origin,
        );
        if cells_changed
            || state_changed
            || graphics_changed
            || history_changed
            || mode_changed
            || title_changed
            || status_changed
        {
            // DEC synchronized output makes the terminal display this diff as one
            // frame. Unknown DEC private modes are safely ignored by terminals
            // which do not implement mode 2026.
            output.extend_from_slice(b"\x1b[?2026h");
        }
        if (cells_changed || history_changed)
            && !graphics_changed
            && !previous.terminal_state.hide_cursor
        {
            output.extend_from_slice(b"\x1b[?25l");
        }
        if history_changed || mode_changed || (status_changed && current.compact) {
            let _ = write!(output, "\x1b[1;1H\x1b[32m");
            draw_window_bar(&mut output, windows, active, terminal_size.0, &current);
        }
        if title_changed && current.outer_border {
            let _ = write!(output, "\x1b[2;1H\x1b[32m");
            draw_terminal_border(&mut output, &current.terminal_title, terminal_size.0);
        }
        if (status_changed || mode_changed || history_changed)
            && !current.compact
            && terminal_size.1 > 1
        {
            draw_bottom_status(&mut output, &current);
        }
        for (row, first, last) in changes {
            let _ = write!(
                output,
                "\x1b[{};{}H",
                row + usize::from(current.content_origin.1),
                first + usize::from(current.content_origin.0)
            );
            let mut previous_style = None;
            for cell in &current.cells[row * columns + first..=row * columns + last] {
                if cell.wide_continuation {
                    continue;
                }
                if previous_style != Some(cell.style) {
                    write_cell_style(&mut output, cell.style);
                    previous_style = Some(cell.style);
                }
                if cell.contents.is_empty() {
                    output.push(b' ');
                } else {
                    output.extend_from_slice(cell.contents.as_bytes());
                }
            }
        }

        if cells_changed
            || state_changed
            || graphics_changed
            || history_changed
            || mode_changed
            || title_changed
            || status_changed
        {
            append_terminal_state_diff(
                &mut output,
                &previous.terminal_state,
                &current.terminal_state,
                current.content_origin,
                cells_changed
                    || graphics_changed
                    || history_changed
                    || mode_changed
                    || title_changed
                    || status_changed,
            );
            output.extend_from_slice(b"\x1b[?2026l");
        }
        self.previous = Some(current);
        output
    }
}

#[derive(Eq, PartialEq)]
pub(super) struct FrameSnapshot {
    terminal_size: (u16, u16),
    pub(super) content_size: (u16, u16),
    pub(super) content_origin: (u16, u16),
    pub(super) outer_border: bool,
    active_id: usize,
    pub(super) tabs: Vec<(usize, String)>,
    mode: String,
    terminal_title: String,
    border_status: Option<String>,
    compact: bool,
    mode_hints: Vec<String>,
    history_mode: bool,
    history_offset: usize,
    cells: Vec<CellSnapshot>,
    terminal_state: TerminalState,
}

impl FrameSnapshot {
    #[cfg(test)]
    pub(super) fn capture(
        windows: &[Window],
        active: usize,
        terminal_size: (u16, u16),
        mode: &str,
        selection: Option<&TextSelection>,
        border_status: Option<&str>,
    ) -> Self {
        Self::capture_with_ui(
            windows,
            active,
            terminal_size,
            RenderUi {
                mode,
                compact: false,
                mode_hints: &[],
                border_status,
            },
            selection,
        )
    }

    fn capture_with_ui(
        windows: &[Window],
        active: usize,
        terminal_size: (u16, u16),
        ui: RenderUi<'_>,
        selection: Option<&TextSelection>,
    ) -> Self {
        let RenderUi {
            mode,
            compact,
            mode_hints,
            border_status,
        } = ui;
        let base = render_base_index(windows, active);
        let tab_id = windows[base].tab_id;
        let tab_panes = windows
            .iter()
            .enumerate()
            .filter(|(_, window)| !window.floating && window.tab_id == tab_id)
            .collect::<Vec<_>>();
        let outer_border = tab_panes.len() == 1;
        let (columns, rows) = if outer_border {
            content_size_for(terminal_size, compact)
        } else {
            let rect = tiled_content_rect_for(terminal_size, compact);
            (rect.width, rect.height)
        };
        let content_origin = if outer_border { (2, 3) } else { (1, 2) };
        let mut cells = (0..usize::from(columns) * usize::from(rows))
            .map(|_| CellSnapshot::blank())
            .collect::<Vec<_>>();
        for (index, window) in tab_panes {
            if window.pane_framed {
                overlay_pane_cells(
                    &mut cells,
                    (columns, rows),
                    window,
                    index == base,
                    selection,
                );
            } else {
                let screen = window.terminal.screen();
                for row in 0..rows {
                    for column in 0..columns {
                        let cell = screen.cell(row, column).expect("cell is within screen");
                        let mut style = CellStyle::from(cell);
                        if selection_contains(selection, window.id, row, column, columns) {
                            style.inverse = !style.inverse;
                        }
                        cells[usize::from(row) * usize::from(columns) + usize::from(column)] =
                            CellSnapshot {
                                contents: cell.contents().to_owned(),
                                style,
                                wide_continuation: cell.is_wide_continuation(),
                            };
                    }
                }
            }
        }
        if windows[active].floating {
            overlay_floating_cells(
                &mut cells,
                (columns, rows),
                &windows[active],
                floating_layout_for(terminal_size, compact),
                selection,
                content_origin,
            );
        }
        let active_screen = windows[active].terminal.screen();
        let mut terminal_state = TerminalState::capture(
            active_screen,
            windows[active].cursor_style.style,
            windows[active].history_mode,
        );
        if windows[active].floating {
            let layout = floating_layout_for(terminal_size, compact);
            terminal_state.cursor.0 = terminal_state.cursor.0.saturating_add(
                layout
                    .row
                    .saturating_add(1)
                    .saturating_sub(content_origin.1),
            );
            terminal_state.cursor.1 = terminal_state.cursor.1.saturating_add(
                layout
                    .column
                    .saturating_add(1)
                    .saturating_sub(content_origin.0),
            );
        } else if windows[active].pane_framed {
            terminal_state.cursor.0 = terminal_state
                .cursor
                .0
                .saturating_add(windows[active].pane_rect.row + 1);
            terminal_state.cursor.1 = terminal_state
                .cursor
                .1
                .saturating_add(windows[active].pane_rect.column + 1);
        }
        Self {
            terminal_size,
            content_size: (columns, rows),
            content_origin,
            outer_border,
            active_id: windows[active].id,
            tabs: windows.iter().filter(|window| !window.floating).fold(
                Vec::new(),
                |mut tabs, window| {
                    if !tabs.iter().any(|(id, _)| *id == window.tab_id) {
                        tabs.push((window.tab_id, window.name.clone()));
                    }
                    tabs
                },
            ),
            mode: mode.to_owned(),
            terminal_title: windows[base].terminal_title().to_owned(),
            border_status: border_status.map(str::to_owned),
            compact,
            mode_hints: mode_hints.to_vec(),
            history_mode: windows[active].history_mode,
            history_offset: active_screen.scrollback(),
            cells,
            terminal_state,
        }
    }
}

#[derive(Clone, Copy)]
struct RenderUi<'a> {
    mode: &'a str,
    compact: bool,
    mode_hints: &'a [String],
    border_status: Option<&'a str>,
}

#[derive(Eq, PartialEq)]
pub(super) struct CellSnapshot {
    pub(super) contents: String,
    style: CellStyle,
    pub(super) wide_continuation: bool,
}

pub(super) fn render_base_index(windows: &[Window], active: usize) -> usize {
    if !windows[active].floating {
        return active;
    }
    windows[active]
        .return_to_window
        .and_then(|id| {
            windows
                .iter()
                .position(|window| window.id == id && !window.floating)
        })
        .or_else(|| windows.iter().position(|window| !window.floating))
        .expect("a floating window always has a regular base window")
}

fn overlay_pane_cells(
    cells: &mut [CellSnapshot],
    canvas_size: (u16, u16),
    window: &Window,
    active: bool,
    selection: Option<&TextSelection>,
) {
    let (columns, rows) = canvas_size;
    let rect = window.pane_rect;
    let border_cell = |contents: char| CellSnapshot {
        contents: contents.to_string(),
        style: if active {
            CellStyle::active_border()
        } else {
            CellStyle::border()
        },
        wide_continuation: false,
    };
    let replace = |cells: &mut [CellSnapshot], row: u16, column: u16, cell: CellSnapshot| {
        let row = rect.row.saturating_add(row);
        let column = rect.column.saturating_add(column);
        if row < rows && column < columns {
            cells[usize::from(row) * usize::from(columns) + usize::from(column)] = cell;
        }
    };
    for row in 0..rect.height {
        for column in 0..rect.width {
            let border = match (row, column) {
                (0, 0) => Some('┌'),
                (0, column) if column + 1 == rect.width => Some('┐'),
                (row, 0) if row + 1 == rect.height => Some('└'),
                (row, column) if row + 1 == rect.height && column + 1 == rect.width => Some('┘'),
                _ if row == 0 || row + 1 == rect.height => Some('─'),
                _ if column == 0 || column + 1 == rect.width => Some('│'),
                _ => None,
            };
            if let Some(character) = border {
                replace(cells, row, column, border_cell(character));
            }
        }
    }
    let title = format!("─ {} ", window.terminal_title());
    let title_cells = styled_text_cells(
        &title,
        usize::from(rect.width.saturating_sub(2)),
        if active {
            CellStyle::active_border()
        } else {
            CellStyle::border()
        },
    );
    for (offset, cell) in title_cells.into_iter().enumerate() {
        replace(cells, 0, offset as u16 + 1, cell);
    }
    let screen = window.terminal.screen();
    let (content_columns, content_rows) = pane_pty_size(rect, true);
    for row in 0..content_rows {
        for column in 0..content_columns {
            let Some(cell) = screen.cell(row, column) else {
                continue;
            };
            let mut style = CellStyle::from(cell);
            if selection_contains(selection, window.id, row, column, content_columns) {
                style.inverse = !style.inverse;
            }
            replace(
                cells,
                row + 1,
                column + 1,
                CellSnapshot {
                    contents: cell.contents().to_owned(),
                    style,
                    wide_continuation: cell.is_wide_continuation(),
                },
            );
        }
    }
}

fn overlay_floating_cells(
    cells: &mut [CellSnapshot],
    canvas_size: (u16, u16),
    window: &Window,
    layout: FloatingLayout,
    selection: Option<&TextSelection>,
    content_origin: (u16, u16),
) {
    let (columns, rows) = canvas_size;
    let first_row = layout.row.saturating_sub(content_origin.1);
    let first_column = layout.column.saturating_sub(content_origin.0);
    let last_column = first_column
        .saturating_add(layout.width.saturating_sub(1))
        .min(columns.saturating_sub(1));
    for row in first_row..first_row.saturating_add(layout.height).min(rows) {
        let row_start = usize::from(row) * usize::from(columns);
        let first = row_start + usize::from(first_column);
        if cells[first].wide_continuation && first_column > 0 {
            cells[first - 1] = CellSnapshot::blank();
        }
        if last_column + 1 < columns {
            let after = row_start + usize::from(last_column + 1);
            if cells[after].wide_continuation {
                cells[after] = CellSnapshot::blank();
            }
        }
        for column in first_column..=last_column {
            cells[row_start + usize::from(column)] = CellSnapshot::blank();
        }
    }

    let replace =
        |cells: &mut [CellSnapshot], local_row: u16, local_column: u16, cell: CellSnapshot| {
            let row = layout
                .row
                .saturating_sub(content_origin.1)
                .saturating_add(local_row);
            let column = layout
                .column
                .saturating_sub(content_origin.0)
                .saturating_add(local_column);
            if row < rows && column < columns {
                cells[usize::from(row) * usize::from(columns) + usize::from(column)] = cell;
            }
        };
    let border_cell = |contents: char| CellSnapshot {
        contents: contents.to_string(),
        style: CellStyle::border(),
        wide_continuation: false,
    };

    for row in 0..layout.height {
        for column in 0..layout.width {
            let border = match (row, column) {
                (0, 0) => Some('┌'),
                (0, column) if column + 1 == layout.width => Some('┐'),
                (row, 0) if row + 1 == layout.height => Some('└'),
                (row, column) if row + 1 == layout.height && column + 1 == layout.width => {
                    Some('┘')
                }
                _ if row == 0 || row + 1 == layout.height => Some('─'),
                _ if column == 0 || column + 1 == layout.width => Some('│'),
                _ => None,
            };
            if let Some(character) = border {
                replace(cells, row, column, border_cell(character));
            }
        }
    }

    let title = format!("─ {} ", window.terminal_title());
    let title_cells = styled_text_cells(
        &title,
        usize::from(layout.width.saturating_sub(2)),
        CellStyle::border(),
    );
    for (offset, cell) in title_cells.into_iter().enumerate() {
        replace(cells, 0, offset as u16 + 1, cell);
    }

    let screen = window.terminal.screen();
    let (content_columns, content_rows) = layout.content_size();
    for row in 0..content_rows {
        for column in 0..content_columns {
            let cell = screen
                .cell(row, column)
                .expect("floating cell is within screen");
            let mut style = CellStyle::from(cell);
            if selection_contains(selection, window.id, row, column, content_columns) {
                style.inverse = !style.inverse;
            }
            replace(
                cells,
                row + 1,
                column + 1,
                CellSnapshot {
                    contents: cell.contents().to_owned(),
                    style,
                    wide_continuation: cell.is_wide_continuation(),
                },
            );
        }
    }
}

impl CellSnapshot {
    fn blank() -> Self {
        Self {
            contents: String::new(),
            style: CellStyle::plain(),
            wide_continuation: false,
        }
    }
}

pub(super) fn styled_text_cells(
    text: &str,
    max_width: usize,
    style: CellStyle,
) -> Vec<CellSnapshot> {
    let mut cells: Vec<CellSnapshot> = Vec::new();
    for grapheme in text.graphemes(true) {
        let width = UnicodeWidthStr::width(grapheme);
        if width == 0 {
            if let Some(cell) = cells.iter_mut().rev().find(|cell| !cell.wide_continuation) {
                cell.contents.push_str(grapheme);
            }
            continue;
        }
        if cells.len() + width > max_width {
            break;
        }
        cells.push(CellSnapshot {
            contents: grapheme.to_owned(),
            style,
            wide_continuation: false,
        });
        cells.extend((1..width).map(|_| CellSnapshot {
            contents: String::new(),
            style,
            wide_continuation: true,
        }));
    }
    cells
}

pub(super) fn truncate_to_display_width(text: &str, max_width: usize) -> (String, usize) {
    let cells = styled_text_cells(text, max_width, CellStyle::plain());
    let value = cells
        .iter()
        .filter(|cell| !cell.wide_continuation)
        .map(|cell| cell.contents.as_str())
        .collect();
    (value, cells.len())
}

#[derive(Eq, PartialEq)]
struct TerminalState {
    cursor: (u16, u16),
    application_cursor: bool,
    bracketed_paste: bool,
    hide_cursor: bool,
    cursor_style: u8,
}

impl TerminalState {
    fn capture(screen: &vt100::Screen, cursor_style: u8, force_hide_cursor: bool) -> Self {
        Self {
            cursor: screen.cursor_position(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            hide_cursor: force_hide_cursor || screen.hide_cursor(),
            cursor_style,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct CellStyle {
    foreground: vt100::Color,
    background: vt100::Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl From<&vt100::Cell> for CellStyle {
    fn from(cell: &vt100::Cell) -> Self {
        Self {
            foreground: cell.fgcolor(),
            background: cell.bgcolor(),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }
}

impl CellStyle {
    fn plain() -> Self {
        Self {
            foreground: vt100::Color::Default,
            background: vt100::Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }

    pub(super) fn border() -> Self {
        Self {
            foreground: vt100::Color::Idx(2),
            ..Self::plain()
        }
    }

    fn active_border() -> Self {
        Self {
            foreground: vt100::Color::Idx(10),
            bold: true,
            ..Self::plain()
        }
    }
}

pub(super) fn render_frame(
    windows: &[Window],
    active: usize,
    snapshot: &FrameSnapshot,
    graphics: &[Vec<u8>],
) -> Vec<u8> {
    let terminal_size = snapshot.terminal_size;
    let (width, height) = terminal_size;
    let (content_columns, content_rows) = snapshot.content_size;
    let mut output = Vec::with_capacity(usize::from(width) * usize::from(height) * 2);

    output.extend_from_slice(b"\x1b[?25l\x1b[2J");
    append_graphics(
        &mut output,
        graphics,
        &snapshot.terminal_state,
        snapshot.content_origin,
    );
    output.extend_from_slice(b"\x1b[H");
    draw_window_bar(&mut output, windows, active, width, snapshot);
    if snapshot.outer_border {
        let _ = write!(output, "\x1b[2;1H\x1b[32m");
        draw_terminal_border(&mut output, &snapshot.terminal_title, width);
    }

    for row in 0..content_rows {
        let _ = write!(
            output,
            "\x1b[{};{}H",
            row + snapshot.content_origin.1,
            if snapshot.outer_border {
                1
            } else {
                snapshot.content_origin.0
            }
        );
        if snapshot.outer_border {
            output.extend_from_slice(b"\x1b[32m\xE2\x94\x82\x1b[0m");
        }
        let mut previous_style = None;
        for column in 0..content_columns {
            let cell = &snapshot.cells
                [usize::from(row) * usize::from(content_columns) + usize::from(column)];
            if cell.wide_continuation {
                continue;
            }
            if previous_style != Some(cell.style) {
                write_cell_style(&mut output, cell.style);
                previous_style = Some(cell.style);
            }
            if !cell.contents.is_empty() {
                output.extend_from_slice(cell.contents.as_bytes());
            } else {
                output.push(b' ');
            }
        }
        if snapshot.outer_border {
            output.extend_from_slice("\x1b[0;32m│".as_bytes());
        }
    }

    if !snapshot.compact && height > 1 {
        if snapshot.outer_border && height > 2 {
            let _ = write!(output, "\x1b[{};1H\x1b[32m", height - 1);
            draw_bottom_border(&mut output, width);
        }
        draw_bottom_status(&mut output, snapshot);
    }

    append_terminal_state(
        &mut output,
        &snapshot.terminal_state,
        windows[active].id,
        snapshot.content_origin,
    );
    output
}

fn append_graphics(
    output: &mut Vec<u8>,
    graphics: &[Vec<u8>],
    state: &TerminalState,
    origin: (u16, u16),
) {
    if graphics.is_empty() {
        return;
    }
    output.reserve(graphics.iter().map(Vec::len).sum());
    let (row, column) = state.cursor;
    let _ = write!(output, "\x1b[{};{}H", row + origin.1, column + origin.0);
    for command in graphics {
        output.extend_from_slice(command);
    }
}

fn append_terminal_state(
    output: &mut Vec<u8>,
    state: &TerminalState,
    active_id: usize,
    origin: (u16, u16),
) {
    let (cursor_row, cursor_column) = state.cursor;
    let _ = write!(
        output,
        "\x1b[0m\x1b]0;rustmux:{}\x07\x1b[?1{}\x1b[?2004{}\x1b[{} q\x1b[{};{}H\x1b[?25{}",
        active_id,
        if state.application_cursor { 'h' } else { 'l' },
        if state.bracketed_paste { 'h' } else { 'l' },
        state.cursor_style,
        cursor_row + origin.1,
        cursor_column + origin.0,
        if state.hide_cursor { 'l' } else { 'h' },
    );
}

fn append_terminal_state_diff(
    output: &mut Vec<u8>,
    previous: &TerminalState,
    current: &TerminalState,
    origin: (u16, u16),
    cells_changed: bool,
) {
    output.extend_from_slice(b"\x1b[0m");
    if previous.application_cursor != current.application_cursor {
        let _ = write!(
            output,
            "\x1b[?1{}",
            if current.application_cursor { 'h' } else { 'l' }
        );
    }
    if previous.bracketed_paste != current.bracketed_paste {
        let _ = write!(
            output,
            "\x1b[?2004{}",
            if current.bracketed_paste { 'h' } else { 'l' }
        );
    }
    if previous.cursor_style != current.cursor_style {
        let _ = write!(output, "\x1b[{} q", current.cursor_style);
    }
    if cells_changed || previous.cursor != current.cursor {
        let (row, column) = current.cursor;
        let _ = write!(output, "\x1b[{};{}H", row + origin.1, column + origin.0);
    }
    if cells_changed || previous.hide_cursor != current.hide_cursor {
        let _ = write!(
            output,
            "\x1b[?25{}",
            if current.hide_cursor { 'l' } else { 'h' }
        );
    }
}

fn draw_window_bar(
    output: &mut Vec<u8>,
    windows: &[Window],
    active: usize,
    width: u16,
    snapshot: &FrameSnapshot,
) {
    if width == 0 {
        return;
    }
    // Reset to the outer terminal's default background before erasing. EL
    // paints with the current background, which may otherwise leak a child
    // application's grey background into the transparent tab-bar cells.
    output.extend_from_slice(b"\x1b[0;49m\x1b[2K");
    let inner_width = usize::from(width);
    let compact_status = snapshot.compact.then(|| {
        let mode = mode_label(&snapshot.mode, snapshot.history_offset);
        snapshot
            .border_status
            .as_deref()
            .map(|status| format!("{mode} │ {status}"))
            .unwrap_or(mode)
    });
    let compact_width = compact_status
        .as_deref()
        .map(|status| UnicodeWidthStr::width(status) + 2)
        .unwrap_or(0)
        .min(inner_width);
    let tabs_width = inner_width.saturating_sub(compact_width);
    let mut used = 0;
    let base = render_base_index(windows, active);
    let active_tab = windows[base].tab_id;
    let mut seen = Vec::new();
    for window in windows.iter().filter(|window| !window.floating) {
        if seen.contains(&window.tab_id) {
            continue;
        }
        seen.push(window.tab_id);
        let display_index = seen.len();
        if used >= tabs_width {
            break;
        }
        let label = format!(" {} {} ", display_index, window.name);
        let available = tabs_width.saturating_sub(used).saturating_sub(1);
        let (label, label_width) = truncate_to_display_width(&label, available);
        if window.tab_id == active_tab {
            output.extend_from_slice(b"\x1b[1;30;42m");
        } else {
            output.extend_from_slice(b"\x1b[1;30;48;2;205;214;244m");
        }
        output.extend_from_slice(label.as_bytes());
        output.extend_from_slice(b"\x1b[0;49m ");
        used += label_width + 1;
    }
    if let Some(status) = compact_status {
        let label = format!(" {status} ");
        let (label, label_width) = truncate_to_display_width(&label, compact_width);
        let column = inner_width.saturating_sub(label_width) + 1;
        let _ = write!(output, "\x1b[1;{column}H\x1b[1;30;42m");
        output.extend_from_slice(label.as_bytes());
    }
    output.extend_from_slice(b"\x1b[0m");
}

fn mode_label(mode: &str, history_offset: usize) -> String {
    if mode == "scroll" && history_offset > 0 {
        format!("{} {history_offset}", mode.to_ascii_uppercase())
    } else {
        mode.to_ascii_uppercase()
    }
}

fn status_text(snapshot: &FrameSnapshot) -> String {
    let mut parts = vec![mode_label(&snapshot.mode, snapshot.history_offset)];
    if let Some(status) = &snapshot.border_status {
        parts.push(status.clone());
    }
    parts.extend(snapshot.mode_hints.iter().cloned());
    format!(" {} ", parts.join(" │ "))
}

fn draw_bottom_status(output: &mut Vec<u8>, snapshot: &FrameSnapshot) {
    let (width, height) = snapshot.terminal_size;
    let status = status_text(snapshot);
    let _ = write!(output, "\x1b[{height};1H");
    output.extend_from_slice(b"\x1b[0;49m\x1b[2K\x1b[1;30;42m");
    let (status, _) = truncate_to_display_width(&status, usize::from(width));
    output.extend_from_slice(status.as_bytes());
    output.extend_from_slice(b"\x1b[0m");
}

fn draw_terminal_border(output: &mut Vec<u8>, title: &str, width: u16) {
    if width == 0 {
        return;
    }
    output.extend_from_slice("\x1b[0;49m\x1b[2K\x1b[32m┌".as_bytes());
    let inner_width = usize::from(width.saturating_sub(2));
    let decorated = format!("─ {title} ");
    let (title, title_width) = truncate_to_display_width(&decorated, inner_width);
    output.extend_from_slice(title.as_bytes());
    for _ in title_width..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    if width > 1 {
        output.extend_from_slice("┐".as_bytes());
    }
}

fn draw_bottom_border(output: &mut Vec<u8>, width: u16) {
    if width == 0 {
        return;
    }
    output.extend_from_slice("└".as_bytes());
    let inner_width = usize::from(width.saturating_sub(2));
    for _ in 0..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    if width > 1 {
        output.extend_from_slice("┘".as_bytes());
    }
}

fn write_cell_style(output: &mut Vec<u8>, style: CellStyle) {
    output.extend_from_slice(b"\x1b[0m");
    if style.bold {
        output.extend_from_slice(b"\x1b[1m");
    }
    if style.dim {
        output.extend_from_slice(b"\x1b[2m");
    }
    if style.italic {
        output.extend_from_slice(b"\x1b[3m");
    }
    if style.underline {
        output.extend_from_slice(b"\x1b[4m");
    }
    if style.inverse {
        output.extend_from_slice(b"\x1b[7m");
    }
    write_color(output, style.foreground, true);
    write_color(output, style.background, false);
}

fn write_color(output: &mut Vec<u8>, color: vt100::Color, foreground: bool) {
    let base = if foreground { 38 } else { 48 };
    match color {
        vt100::Color::Default => {}
        vt100::Color::Idx(index) => {
            let _ = write!(output, "\x1b[{base};5;{index}m");
        }
        vt100::Color::Rgb(red, green, blue) => {
            let _ = write!(output, "\x1b[{base};2;{red};{green};{blue}m");
        }
    }
}
