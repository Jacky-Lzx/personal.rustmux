use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{TextSelection, Window, selection_contains};
use crate::layout::{
    FloatingLayout, content_size_for, floating_layout_for, pane_pty_size, tiled_content_rect_for,
};
use crate::session::SessionInfo;

type Rgb = (u8, u8, u8);

const MOCHA_CRUST: Rgb = (17, 17, 27);
const MOCHA_BASE: Rgb = (30, 30, 46);
const MOCHA_SURFACE_0: Rgb = (49, 50, 68);
const MOCHA_SURFACE_1: Rgb = (69, 71, 90);
const MOCHA_OVERLAY_0: Rgb = (108, 112, 134);
const MOCHA_SUBTEXT_0: Rgb = (166, 173, 200);
const MOCHA_TEXT: Rgb = (205, 214, 244);
const MOCHA_RED: Rgb = (243, 139, 168);
const MOCHA_GREEN: Rgb = (166, 227, 161);
const MOCHA_PEACH: Rgb = (250, 179, 135);
const MOCHA_YELLOW: Rgb = (249, 226, 175);
const MOCHA_BLUE: Rgb = (137, 180, 250);
const MOCHA_LAVENDER: Rgb = (180, 190, 254);
const MOCHA_PINK: Rgb = (245, 194, 231);
const MOCHA_MAUVE: Rgb = (203, 166, 247);
const MOCHA_TEAL: Rgb = (148, 226, 213);
const POWERLINE_RIGHT: &str = "";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionManagerView {
    pub(super) searching: bool,
    pub(super) keys: std::collections::HashMap<String, Vec<String>>,
    pub(super) query: String,
    pub(super) sessions: Vec<SessionInfo>,
    pub(super) selected: usize,
    pub(super) current: String,
    pub(super) rename_input: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HelpView {
    pub(super) mode: String,
    pub(super) hints: Vec<String>,
}

pub(super) fn session_manager_rect((columns, rows): (u16, u16)) -> (u16, u16, u16, u16) {
    let width = (columns / 2).max(40).min(columns);
    let height = (rows / 2).max(8).min(rows);
    (
        columns.saturating_sub(width) / 2,
        rows.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(super) fn notification_rect(
    (columns, rows): (u16, u16),
    message: &str,
) -> (u16, u16, u16, u16) {
    let message_width = UnicodeWidthStr::width(message).min(68);
    let width = u16::try_from(message_width.saturating_add(4))
        .unwrap_or(u16::MAX)
        .max(14)
        .min(columns);
    let height = 3.min(rows);
    (
        columns.saturating_sub(width) / 2,
        rows.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(super) fn help_rect((columns, rows): (u16, u16), hint_count: usize) -> (u16, u16, u16, u16) {
    let width = (columns.saturating_mul(3) / 4).max(48).min(columns);
    let columns_per_row = if width >= 48 { 2 } else { 1 };
    let hint_rows = hint_count.div_ceil(columns_per_row);
    let height = u16::try_from(hint_rows.saturating_add(4))
        .unwrap_or(u16::MAX)
        .max(7)
        .min(rows);
    (
        columns.saturating_sub(width) / 2,
        rows.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[derive(Default)]
pub(super) struct Renderer {
    previous: Option<FrameSnapshot>,
    border_status: Option<String>,
    compact: bool,
    mode_hints: Vec<String>,
    session_name: String,
    rename_prompt: Option<String>,
    history_search_prompt: Option<String>,
    session_manager: Option<SessionManagerView>,
    notification: Option<String>,
    help: Option<HelpView>,
}

impl Renderer {
    pub(super) fn invalidate(&mut self) {
        self.previous = None;
    }

    pub(super) fn set_border_status(&mut self, status: Option<&str>) {
        self.border_status = status.map(str::to_owned);
    }

    pub(super) fn set_session_name(&mut self, session_name: &str) {
        self.session_name = session_name.to_owned();
    }

    pub(super) fn set_rename_prompt(&mut self, name: Option<&str>) {
        self.rename_prompt = name.map(str::to_owned);
    }

    pub(super) fn set_history_search_prompt(&mut self, query: Option<&str>) {
        self.history_search_prompt = query.map(str::to_owned);
    }

    pub(super) fn set_session_manager(&mut self, view: Option<SessionManagerView>) {
        self.session_manager = view;
    }

    pub(super) fn set_notification(&mut self, notification: Option<&str>) {
        let notification = notification.map(str::to_owned);
        if self.notification != notification {
            self.notification = notification;
            self.invalidate();
        }
    }

    pub(super) fn set_help(&mut self, help: Option<HelpView>) {
        if self.help != help {
            self.help = help;
            self.invalidate();
        }
    }

    pub(super) fn set_ui(&mut self, compact: bool, mode_hints: Vec<String>) {
        self.compact = compact;
        self.mode_hints = mode_hints;
    }

    pub(super) fn window_tab_at(&self, position: (u16, u16)) -> Option<usize> {
        if position.1 != 1 {
            return None;
        }
        self.previous
            .as_ref()
            .and_then(|snapshot| window_tab_at(snapshot, position.0))
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
                session_name: &self.session_name,
                rename_prompt: self.rename_prompt.as_deref(),
                history_search_prompt: self.history_search_prompt.as_deref(),
                session_manager: self.session_manager.as_ref(),
                notification: self.notification.as_deref(),
                help: self.help.as_ref(),
            },
            selection,
        );
        let Some(previous) = &self.previous else {
            let output = render_frame(windows, active, &current, graphics);
            self.previous = Some(current);
            return output;
        };
        let active_changed = previous.active_id != current.active_id;
        let active_change_requires_full_frame = active_changed
            && !windows
                .iter()
                .find(|window| window.id == previous.active_id)
                .zip(windows.get(active))
                .is_some_and(|(previous, current)| {
                    !previous.floating
                        && !previous.zoomed
                        && !current.floating
                        && !current.zoomed
                        && previous.tab_id == current.tab_id
                });

        if previous.terminal_size != current.terminal_size
            || previous.content_size != current.content_size
            || previous.content_origin != current.content_origin
            || previous.outer_border != current.outer_border
            || previous.compact != current.compact
            || active_change_requires_full_frame
            || previous.session_name != current.session_name
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
        let tabs_changed = previous.tabs != current.tabs;
        let title_changed = previous.terminal_title != current.terminal_title;
        let status_changed = previous.border_status != current.border_status
            || previous.mode_hints != current.mode_hints
            || previous.rename_prompt != current.rename_prompt
            || previous.history_search_prompt != current.history_search_prompt
            || previous.session_manager != current.session_manager
            || previous.notification != current.notification
            || previous.help != current.help;
        let manager_changed = previous.session_manager != current.session_manager;
        let notification_changed = previous.notification != current.notification;
        let help_changed = previous.help != current.help;
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
            || tabs_changed
            || title_changed
            || active_changed
            || status_changed
        {
            // DEC synchronized output makes the terminal display this diff as one
            // frame. Unknown DEC private modes are safely ignored by terminals
            // which do not implement mode 2026.
            output.extend_from_slice(b"\x1b[?2026h");
        }
        if active_changed {
            let _ = write!(output, "\x1b]0;rustmux:{}\x07", current.active_id);
        }
        if (cells_changed || history_changed)
            && !graphics_changed
            && !previous.terminal_state.hide_cursor
        {
            output.extend_from_slice(b"\x1b[?25l");
        }
        if history_changed || mode_changed || tabs_changed || (status_changed && current.compact) {
            let _ = write!(output, "\x1b[1;1H");
            draw_window_bar(&mut output, windows, active, terminal_size.0, &current);
        }
        if title_changed && current.outer_border {
            let _ = write!(output, "\x1b[2;1H");
            draw_terminal_border(
                &mut output,
                &current.terminal_title,
                terminal_size.0,
                !windows[active].floating,
            );
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
            let mut previous_hyperlink = None;
            for cell in &current.cells[row * columns + first..=row * columns + last] {
                if cell.wide_continuation {
                    continue;
                }
                let hyperlink = cell.hyperlink.as_deref();
                if previous_hyperlink != hyperlink {
                    write_hyperlink(&mut output, hyperlink);
                    previous_hyperlink = hyperlink;
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
            if previous_hyperlink.is_some() {
                write_hyperlink(&mut output, None);
            }
        }
        if manager_changed || (cells_changed && current.session_manager.is_some()) {
            draw_session_manager(&mut output, &current);
        }
        if notification_changed || (cells_changed && current.notification.is_some()) {
            draw_notification(&mut output, &current);
        }
        if help_changed || (cells_changed && current.help.is_some()) {
            draw_help(&mut output, &current);
        }

        if cells_changed
            || state_changed
            || graphics_changed
            || history_changed
            || mode_changed
            || tabs_changed
            || title_changed
            || active_changed
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
                    || tabs_changed
                    || title_changed
                    || active_changed
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
    session_name: String,
    rename_prompt: Option<String>,
    history_search_prompt: Option<String>,
    session_manager: Option<SessionManagerView>,
    notification: Option<String>,
    help: Option<HelpView>,
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
                session_name: "",
                rename_prompt: None,
                history_search_prompt: None,
                session_manager: None,
                notification: None,
                help: None,
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
            session_name,
            rename_prompt,
            history_search_prompt,
            session_manager,
            notification,
            help,
        } = ui;
        let base = render_base_index(windows, active);
        let tab_id = windows[base].tab_id;
        let tab_panes = windows
            .iter()
            .enumerate()
            .filter(|(index, window)| {
                !window.floating
                    && window.tab_id == tab_id
                    && (!windows[active].zoomed || *index == active)
            })
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
                    index == active,
                    selection,
                );
            } else {
                let screen = window.terminal.screen();
                for row in 0..rows {
                    for column in 0..columns {
                        let cell = screen.cell(row, column).expect("cell is within screen");
                        let mut style = CellStyle::from_window(cell, window);
                        if selection_contains(selection, window.id, row, column, columns) {
                            style.inverse = !style.inverse;
                        }
                        cells[usize::from(row) * usize::from(columns) + usize::from(column)] =
                            CellSnapshot {
                                contents: cell.contents().to_owned(),
                                style,
                                wide_continuation: cell.is_wide_continuation(),
                                hyperlink: (!cell.contents().is_empty())
                                    .then(|| window.hyperlinks.osc8_at(row, column, window.id))
                                    .flatten(),
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
            windows[active].terminal_osc.cursor(),
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
        if session_manager.is_some() || notification.is_some() || help.is_some() {
            terminal_state.hide_cursor = true;
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
                        let bell_pending = windows.iter().any(|candidate| {
                            candidate.tab_id == window.tab_id && candidate.bell_pending
                        });
                        let name = if bell_pending {
                            format!("{} [!]", window.name)
                        } else {
                            window.name.clone()
                        };
                        tabs.push((window.tab_id, name));
                    }
                    tabs
                },
            ),
            mode: mode.to_owned(),
            terminal_title: windows[base].terminal_title().to_owned(),
            border_status: border_status.map(str::to_owned),
            compact,
            mode_hints: mode_hints.to_vec(),
            session_name: session_name.to_owned(),
            rename_prompt: rename_prompt.map(str::to_owned),
            history_search_prompt: history_search_prompt.map(str::to_owned),
            session_manager: session_manager.cloned(),
            notification: notification.map(str::to_owned),
            help: help.cloned(),
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
    session_name: &'a str,
    rename_prompt: Option<&'a str>,
    history_search_prompt: Option<&'a str>,
    session_manager: Option<&'a SessionManagerView>,
    notification: Option<&'a str>,
    help: Option<&'a HelpView>,
}

#[derive(Eq, PartialEq)]
pub(super) struct CellSnapshot {
    pub(super) contents: String,
    style: CellStyle,
    pub(super) wide_continuation: bool,
    hyperlink: Option<String>,
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
    let border_style = if active {
        CellStyle::active_border()
    } else if window.bell_pending {
        CellStyle::bell_border()
    } else {
        CellStyle::border()
    };
    let border_cell = |contents: char| CellSnapshot {
        contents: contents.to_string(),
        style: border_style,
        wide_continuation: false,
        hyperlink: None,
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
    let title = if window.bell_pending {
        format!("─ {} [!] ", window.terminal_title())
    } else {
        format!("─ {} ", window.terminal_title())
    };
    let title_cells = styled_text_cells(
        &title,
        usize::from(rect.width.saturating_sub(2)),
        border_style,
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
            let mut style = CellStyle::from_window(cell, window);
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
                    hyperlink: (!cell.contents().is_empty())
                        .then(|| window.hyperlinks.osc8_at(row, column, window.id))
                        .flatten(),
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
        style: CellStyle::active_border(),
        wide_continuation: false,
        hyperlink: None,
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
        CellStyle::active_border(),
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
            let mut style = CellStyle::from_window(cell, window);
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
                    hyperlink: (!cell.contents().is_empty())
                        .then(|| window.hyperlinks.osc8_at(row, column, window.id))
                        .flatten(),
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
            hyperlink: None,
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
            hyperlink: None,
        });
        cells.extend((1..width).map(|_| CellSnapshot {
            contents: String::new(),
            style,
            wide_continuation: true,
            hyperlink: None,
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
    cursor_color: Option<Rgb>,
}

impl TerminalState {
    fn capture(
        screen: &vt100::Screen,
        cursor_style: u8,
        cursor_color: Option<Rgb>,
        force_hide_cursor: bool,
    ) -> Self {
        Self {
            cursor: screen.cursor_position(),
            application_cursor: screen.application_cursor(),
            bracketed_paste: screen.bracketed_paste(),
            hide_cursor: force_hide_cursor || screen.hide_cursor(),
            cursor_style,
            cursor_color,
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
    fn from_window(cell: &vt100::Cell, window: &Window) -> Self {
        let mut style = Self::from(cell);
        if style.foreground == vt100::Color::Default {
            let (red, green, blue) = window.terminal_osc.foreground();
            style.foreground = vt100::Color::Rgb(red, green, blue);
        }
        if style.background == vt100::Color::Default {
            let (red, green, blue) = window.terminal_osc.background();
            style.background = vt100::Color::Rgb(red, green, blue);
        }
        style
    }

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
            foreground: vt100::Color::Rgb(MOCHA_OVERLAY_0.0, MOCHA_OVERLAY_0.1, MOCHA_OVERLAY_0.2),
            ..Self::plain()
        }
    }

    fn active_border() -> Self {
        Self {
            foreground: vt100::Color::Rgb(MOCHA_GREEN.0, MOCHA_GREEN.1, MOCHA_GREEN.2),
            bold: true,
            ..Self::plain()
        }
    }

    fn bell_border() -> Self {
        Self {
            foreground: vt100::Color::Rgb(MOCHA_PEACH.0, MOCHA_PEACH.1, MOCHA_PEACH.2),
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
        let _ = write!(output, "\x1b[2;1H");
        draw_terminal_border(
            &mut output,
            &snapshot.terminal_title,
            width,
            !windows[active].floating,
        );
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
            write_rgb_style(
                &mut output,
                if windows[active].floating {
                    MOCHA_OVERLAY_0
                } else {
                    MOCHA_GREEN
                },
                None,
                false,
            );
            output.extend_from_slice("│\x1b[0m".as_bytes());
        }
        let mut previous_style = None;
        let mut previous_hyperlink = None;
        for column in 0..content_columns {
            let cell = &snapshot.cells
                [usize::from(row) * usize::from(content_columns) + usize::from(column)];
            if cell.wide_continuation {
                continue;
            }
            let hyperlink = cell.hyperlink.as_deref();
            if previous_hyperlink != hyperlink {
                write_hyperlink(&mut output, hyperlink);
                previous_hyperlink = hyperlink;
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
        if previous_hyperlink.is_some() {
            write_hyperlink(&mut output, None);
        }
        if snapshot.outer_border {
            write_rgb_style(
                &mut output,
                if windows[active].floating {
                    MOCHA_OVERLAY_0
                } else {
                    MOCHA_GREEN
                },
                None,
                false,
            );
            output.extend_from_slice("│".as_bytes());
        }
    }

    if snapshot.session_manager.is_some() {
        draw_session_manager(&mut output, snapshot);
    }
    if snapshot.notification.is_some() {
        draw_notification(&mut output, snapshot);
    }
    if snapshot.help.is_some() {
        draw_help(&mut output, snapshot);
    }

    if !snapshot.compact && height > 1 {
        if snapshot.outer_border && height > 2 {
            let _ = write!(output, "\x1b[{};1H", height - 1);
            draw_bottom_border(&mut output, width, !windows[active].floating);
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
    let _ = write!(output, "\x1b[0m\x1b]0;rustmux:{}\x07", active_id,);
    write_cursor_color(output, state.cursor_color);
    let _ = write!(
        output,
        "\x1b[?1{}\x1b[?2004{}\x1b[{} q\x1b[{};{}H\x1b[?25{}",
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
    if previous.cursor_color != current.cursor_color {
        write_cursor_color(output, current.cursor_color);
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

fn write_cursor_color(output: &mut Vec<u8>, color: Option<Rgb>) {
    if let Some((red, green, blue)) = color {
        let _ = write!(output, "\x1b]12;#{red:02x}{green:02x}{blue:02x}\x1b\\");
    } else {
        output.extend_from_slice(b"\x1b]112\x1b\\");
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
    write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
    output.extend_from_slice(b"\x1b[2K");
    let inner_width = usize::from(width);
    let compact_status = compact_status(snapshot);
    let compact_width = compact_status
        .as_deref()
        .map(|status| UnicodeWidthStr::width(status) + 4)
        .unwrap_or(0)
        .min(inner_width);
    let tabs_width = inner_width.saturating_sub(compact_width);
    let session_label = (!snapshot.session_name.is_empty())
        .then(|| format!(" Rustmux ({}) ", snapshot.session_name));
    let session_width = session_label
        .as_deref()
        .map(|label| {
            let available = tabs_width.saturating_sub(6);
            let (label, label_width) = truncate_to_display_width(label, available);
            write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), true);
            output.extend_from_slice(label.as_bytes());
            label_width
        })
        .unwrap_or(0);
    let tabs_width = tabs_width.saturating_sub(session_width);
    let base = render_base_index(windows, active);
    let active_tab = windows[base].tab_id;
    let mut tabs = Vec::new();
    for (index, (tab_id, name)) in snapshot.tabs.iter().enumerate() {
        let display_index = index + 1;
        let color = if *tab_id == active_tab {
            MOCHA_GREEN
        } else {
            MOCHA_TEXT
        };
        tabs.push((format!("{display_index} {name}"), color));
    }
    draw_powerline_segments(output, &tabs, tabs_width, MOCHA_BASE);
    if let Some(status) = compact_status {
        let label = format!(" {status} ");
        let (label, label_width) =
            truncate_to_display_width(&label, compact_width.saturating_sub(2));
        let column = inner_width.saturating_sub(label_width + 2) + 1;
        let _ = write!(output, "\x1b[1;{column}H");
        write_rgb_style(output, MOCHA_BASE, Some(MOCHA_GREEN), false);
        output.extend_from_slice(POWERLINE_RIGHT.as_bytes());
        write_rgb_style(output, MOCHA_CRUST, Some(MOCHA_GREEN), true);
        output.extend_from_slice(label.as_bytes());
        write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), false);
        output.extend_from_slice(POWERLINE_RIGHT.as_bytes());
    }
    output.extend_from_slice(b"\x1b[0m");
}

fn compact_status(snapshot: &FrameSnapshot) -> Option<String> {
    snapshot.compact.then(|| {
        if let Some(name) = &snapshot.rename_prompt {
            return format!("RENAME: {name}_");
        }
        if let Some(query) = &snapshot.history_search_prompt {
            return format!("SEARCH: {query}_");
        }
        let mode = mode_label(&snapshot.mode, snapshot.history_offset);
        snapshot
            .border_status
            .as_deref()
            .map(|status| format!("{mode} │ {status}"))
            .unwrap_or(mode)
    })
}

fn window_tab_at(snapshot: &FrameSnapshot, column: u16) -> Option<usize> {
    if column == 0 {
        return None;
    }
    let inner_width = usize::from(snapshot.terminal_size.0);
    let compact_width = compact_status(snapshot)
        .as_deref()
        .map(|status| UnicodeWidthStr::width(status) + 4)
        .unwrap_or(0)
        .min(inner_width);
    let mut tabs_width = inner_width.saturating_sub(compact_width);
    let session_width = (!snapshot.session_name.is_empty())
        .then(|| format!(" Rustmux ({}) ", snapshot.session_name))
        .map(|label| {
            let available = tabs_width.saturating_sub(6);
            truncate_to_display_width(&label, available).1
        })
        .unwrap_or(0);
    tabs_width = tabs_width.saturating_sub(session_width);

    let pointer = usize::from(column - 1);
    let mut used = session_width;
    for (index, (tab_id, name)) in snapshot.tabs.iter().enumerate() {
        let available = tabs_width.saturating_sub(used.saturating_sub(session_width) + 2);
        let (_, label_width) =
            truncate_to_display_width(&format!(" {} {} ", index + 1, name), available);
        if label_width == 0 {
            break;
        }
        let segment_width = label_width + 2;
        if pointer >= used && pointer < used.saturating_add(segment_width) {
            return Some(*tab_id);
        }
        used = used.saturating_add(segment_width);
        if used.saturating_sub(session_width) >= tabs_width {
            break;
        }
    }
    None
}

fn mode_label(mode: &str, history_offset: usize) -> String {
    if mode == "scroll" && history_offset > 0 {
        format!("{} {history_offset}", mode.to_ascii_uppercase())
    } else {
        mode.to_ascii_uppercase()
    }
}

fn action_hint_label(action: &str) -> String {
    action
        .split(" + ")
        .map(|action| match action.strip_prefix("mode:") {
            Some("normal") => "UNLOCK".to_owned(),
            Some("locked") => "LOCK".to_owned(),
            Some(mode) => mode.to_ascii_uppercase(),
            None => action.replace(['-', ':'], " ").to_ascii_uppercase(),
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

pub(super) fn compact_status_hints(hints: &[String]) -> Vec<(String, String)> {
    let has_window_navigation = hints
        .iter()
        .any(|hint| hint == "n=next-window + mode:locked")
        && hints
            .iter()
            .any(|hint| hint == "p=previous-window + mode:locked");
    let mut grouped: Vec<(Vec<String>, String)> = Vec::new();
    let mut window_keys = Vec::new();
    for hint in hints {
        let Some((key, action)) = hint.split_once('=') else {
            continue;
        };
        let action = action.strip_suffix(" + mode:locked").unwrap_or(action);
        if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() && action.starts_with("window:") {
            window_keys.push(key.to_owned());
            continue;
        }
        if has_window_navigation
            && matches!(
                (key, action),
                ("n", "next-window") | ("p", "previous-window")
            )
        {
            continue;
        }
        let label = match action {
            "show-help" => "HELP".to_owned(),
            _ => action_hint_label(action),
        };
        if let Some((keys, _)) = grouped.iter_mut().find(|(_, existing)| *existing == label) {
            keys.push(key.to_owned());
        } else {
            grouped.push((vec![key.to_owned()], label));
        }
    }
    if has_window_navigation {
        grouped.push((vec!["n/p".to_owned()], "WINDOW".to_owned()));
    }
    if !window_keys.is_empty() {
        window_keys.sort();
        let key = if window_keys.len() > 1 {
            format!("{}-{}", window_keys[0], window_keys[window_keys.len() - 1])
        } else {
            window_keys.remove(0)
        };
        grouped.push((vec![key], "WINDOW".to_owned()));
    }

    let mut hints = grouped
        .into_iter()
        .map(|(mut keys, action)| {
            keys.sort_by_key(|key| (UnicodeWidthStr::width(key.as_str()), key.clone()));
            keys.dedup();
            let hidden_aliases = keys.len().saturating_sub(2);
            let mut key = keys.into_iter().take(2).collect::<Vec<_>>().join("/");
            if hidden_aliases > 0 {
                key.push_str("/…");
            }
            (key, action)
        })
        .collect::<Vec<_>>();
    hints.sort_by_key(|(key, action)| {
        let priority = match action.as_str() {
            "UNLOCK" => 0,
            "NEW WINDOW" => 10,
            "RENAME WINDOW" => 20,
            "WINDOW" | "NEXT WINDOW" | "PREVIOUS WINDOW" => 30,
            "SWITCH SESSION" => 40,
            "PANE" => 50,
            "CLOSE WINDOW" | "CLOSE PANE" => 70,
            "DETACH" => 80,
            "HELP" => 250,
            _ => 100,
        };
        (priority, key.clone())
    });
    hints
}

fn powerline_segment_width(text: &str) -> usize {
    UnicodeWidthStr::width(text).saturating_add(4)
}

fn status_segments_width(segments: &[(String, Rgb)]) -> usize {
    segments
        .iter()
        .map(|(text, _)| powerline_segment_width(text))
        .sum()
}

fn fit_status_hints(
    base: Vec<(String, Rgb)>,
    hints: Vec<(String, String)>,
    width: usize,
) -> Vec<(String, Rgb)> {
    for shown in (0..=hints.len()).rev() {
        let hidden = hints.len() - shown;
        let mut segments = base.clone();
        for (index, (key, action)) in hints.iter().take(shown).enumerate() {
            segments.push((key.clone(), MOCHA_PINK));
            segments.push((
                action.clone(),
                if index % 2 == 0 {
                    MOCHA_LAVENDER
                } else {
                    MOCHA_BLUE
                },
            ));
        }
        if hidden > 0 {
            segments.push(("?".to_owned(), MOCHA_PINK));
            segments.push((format!("MORE (+{hidden})"), MOCHA_LAVENDER));
        }
        if status_segments_width(&segments) <= width {
            return segments;
        }
    }
    base
}

fn status_segments(snapshot: &FrameSnapshot, width: usize) -> Vec<(String, Rgb)> {
    if snapshot.session_manager.is_some() {
        return vec![
            ("SESSION MANAGER".to_owned(), MOCHA_GREEN),
            ("Esc".to_owned(), MOCHA_PINK),
            ("CLOSE".to_owned(), MOCHA_LAVENDER),
        ];
    }
    if let Some(name) = &snapshot.rename_prompt {
        return vec![(format!("RENAME: {name}_"), MOCHA_YELLOW)];
    }
    if let Some(query) = &snapshot.history_search_prompt {
        return vec![(format!("SEARCH: {query}_"), MOCHA_YELLOW)];
    }
    let mode_color = if snapshot.mode == "locked" {
        MOCHA_RED
    } else {
        MOCHA_GREEN
    };
    let mut segments = vec![(
        mode_label(&snapshot.mode, snapshot.history_offset),
        mode_color,
    )];
    if let Some(status) = &snapshot.border_status {
        segments.push((status.clone(), MOCHA_YELLOW));
    }
    let hints = compact_status_hints(&snapshot.mode_hints);
    fit_status_hints(segments, hints, width)
}

fn draw_session_manager(output: &mut Vec<u8>, snapshot: &FrameSnapshot) {
    let Some(manager) = &snapshot.session_manager else {
        return;
    };
    let (canvas_columns, canvas_rows) = snapshot.content_size;
    let (box_column, box_row, columns, rows) = session_manager_rect(snapshot.content_size);
    let (canvas_origin_column, canvas_origin_row) = snapshot.content_origin;
    let origin_column = canvas_origin_column + box_column;
    let origin_row = canvas_origin_row + box_row;
    if canvas_columns == 0 || canvas_rows == 0 || columns == 0 || rows == 0 {
        return;
    }
    let blank = " ".repeat(usize::from(columns));
    for row in 0..rows {
        let _ = write!(output, "\x1b[{};{}H", origin_row + row, origin_column);
        write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
        output.extend_from_slice(blank.as_bytes());
    }
    if columns < 4 || rows < 3 {
        return;
    }

    let inner_width = usize::from(columns.saturating_sub(2));
    let title = "─ Session Manager ";
    let session_count = format!(
        " {} SESSION{} ",
        manager.sessions.len(),
        if manager.sessions.len() == 1 { "" } else { "S" }
    );
    let (title, title_width) = truncate_to_display_width(title, inner_width);
    let (session_count, session_count_width) =
        truncate_to_display_width(&session_count, inner_width.saturating_sub(title_width));
    let show_session_count = title_width + session_count_width <= inner_width;
    let _ = write!(output, "\x1b[{origin_row};{origin_column}H");
    write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), true);
    output.extend_from_slice("┌".as_bytes());
    output.extend_from_slice(title.as_bytes());
    let rule_width = if show_session_count {
        inner_width.saturating_sub(session_count_width)
    } else {
        inner_width
    };
    for _ in title_width..rule_width {
        output.extend_from_slice("─".as_bytes());
    }
    if show_session_count {
        write_rgb_style(output, MOCHA_LAVENDER, Some(MOCHA_BASE), true);
        output.extend_from_slice(session_count.as_bytes());
        write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), true);
    }
    output.extend_from_slice("┐".as_bytes());

    for row in 1..rows - 1 {
        let _ = write!(output, "\x1b[{};{}H", origin_row + row, origin_column);
        write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), false);
        output.extend_from_slice("│".as_bytes());
        let _ = write!(
            output,
            "\x1b[{};{}H",
            origin_row + row,
            origin_column + columns - 1
        );
        output.extend_from_slice("│".as_bytes());
    }
    let _ = write!(
        output,
        "\x1b[{};{}H└{}┘",
        origin_row + rows - 1,
        origin_column,
        "─".repeat(inner_width)
    );

    let key_label = |action: &str| {
        manager
            .keys
            .get(action)
            .map(|keys| {
                keys.iter()
                    .take(1)
                    .map(|key| {
                        let mut chars = key.chars();
                        let label = if key.chars().count() > 1 {
                            format!("{}{}", chars.next().unwrap().to_uppercase(), chars.as_str())
                        } else {
                            key.clone()
                        };
                        format!("<{label}>")
                    })
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default()
    };
    if manager.searching || manager.rename_input.is_some() {
        let (query_label, query_value) = if let Some(name) = &manager.rename_input {
            ("Rename: ", name.as_str())
        } else {
            ("Search: ", manager.query.as_str())
        };
        let _ = write!(output, "\x1b[{};{}H", origin_row + 1, origin_column + 2);
        write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), true);
        output.extend_from_slice(query_label.as_bytes());
        let query_width = UnicodeWidthStr::width(query_label);
        let (query_value, _) = truncate_to_display_width(
            query_value,
            inner_width.saturating_sub(query_width.saturating_add(3)),
        );
        write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), true);
        output.extend_from_slice(query_value.as_bytes());
        write_rgb_style(output, MOCHA_PINK, Some(MOCHA_BASE), true);
        output.extend_from_slice(b"_");
    } else {
        draw_session_manager_help_line(
            output,
            origin_row + 1,
            origin_column + 2,
            inner_width.saturating_sub(2),
            "",
            &[
                (key_label("down").as_str(), "Down"),
                (key_label("up").as_str(), "Up"),
                (key_label("search").as_str(), "Search"),
            ],
        );
    }

    if rows < 8 {
        let available_rows = usize::from(rows.saturating_sub(3));
        let first_visible = manager
            .selected
            .saturating_sub(available_rows.saturating_sub(1));
        for (visible_index, (index, session)) in manager
            .sessions
            .iter()
            .enumerate()
            .skip(first_visible)
            .take(available_rows)
            .enumerate()
        {
            let label = format!(
                "{} {}  {}t · {}p",
                if index == manager.selected {
                    "›"
                } else {
                    " "
                },
                session.name,
                session.tabs,
                session.panes
            );
            let (label, _) = truncate_to_display_width(&label, inner_width.saturating_sub(2));
            let _ = write!(
                output,
                "\x1b[{};{}H",
                origin_row + 2 + visible_index as u16,
                origin_column + 2
            );
            write_rgb_style(
                output,
                if index == manager.selected {
                    MOCHA_GREEN
                } else {
                    MOCHA_TEXT
                },
                Some(if index == manager.selected {
                    MOCHA_SURFACE_0
                } else {
                    MOCHA_BASE
                }),
                index == manager.selected,
            );
            output.extend_from_slice(label.as_bytes());
        }
        return;
    }

    let body_width = inner_width.saturating_sub(2);
    let _ = write!(output, "\x1b[{};{}H", origin_row + 2, origin_column + 2);
    write_rgb_style(output, MOCHA_SURFACE_1, Some(MOCHA_BASE), false);
    output.extend_from_slice("─".repeat(body_width).as_bytes());

    let marker_width = 2;
    let created_width = if body_width >= 65 { 15 } else { 0 };
    let layout_width = if body_width >= 44 { 17 } else { 12 };
    let status_width = if body_width >= 30 { 11 } else { 0 };
    let available_name_width =
        body_width.saturating_sub(marker_width + layout_width + status_width + created_width);
    let preferred_name_width = manager
        .sessions
        .iter()
        .map(|session| UnicodeWidthStr::width(session.name.as_str()))
        .max()
        .unwrap_or(0)
        .max(UnicodeWidthStr::width("SESSION"))
        .saturating_add(3)
        .min(28);
    let name_width = preferred_name_width.min(available_name_width);
    let trailing_width = body_width
        .saturating_sub(marker_width + name_width + layout_width + status_width + created_width);

    let _ = write!(output, "\x1b[{};{}H", origin_row + 3, origin_column + 2);
    write_session_manager_field(output, "", marker_width, MOCHA_SUBTEXT_0, MOCHA_BASE, false);
    write_session_manager_field(
        output,
        "SESSION",
        name_width,
        MOCHA_SUBTEXT_0,
        MOCHA_BASE,
        true,
    );
    write_session_manager_field(
        output,
        "LAYOUT",
        layout_width,
        MOCHA_SUBTEXT_0,
        MOCHA_BASE,
        true,
    );
    if status_width > 0 {
        write_session_manager_field(
            output,
            "STATUS",
            status_width,
            MOCHA_SUBTEXT_0,
            MOCHA_BASE,
            true,
        );
    }
    if created_width > 0 {
        write_session_manager_field(
            output,
            "CREATED",
            created_width,
            MOCHA_SUBTEXT_0,
            MOCHA_BASE,
            true,
        );
    }
    write_session_manager_field(
        output,
        "",
        trailing_width,
        MOCHA_SUBTEXT_0,
        MOCHA_BASE,
        false,
    );

    let available_rows = usize::from(rows.saturating_sub(7));
    let first_visible = manager
        .selected
        .saturating_sub(available_rows.saturating_sub(1));
    for (visible_index, (index, session)) in manager
        .sessions
        .iter()
        .enumerate()
        .skip(first_visible)
        .take(available_rows)
        .enumerate()
    {
        let selected = index == manager.selected;
        let current = session.name == manager.current;
        let background = if selected {
            MOCHA_SURFACE_0
        } else {
            MOCHA_BASE
        };
        let (state, state_color) = if current {
            ("[CURRENT]", MOCHA_GREEN)
        } else if session.connected {
            ("[ATTACH]", MOCHA_PEACH)
        } else if session.saved {
            ("[SAVED]", MOCHA_MAUVE)
        } else {
            ("[DETACHED]", MOCHA_OVERLAY_0)
        };
        let layout = if layout_width >= 17 {
            format!("{} tabs · {} panes", session.tabs, session.panes)
        } else {
            format!("{}t · {}p", session.tabs, session.panes)
        };
        let _ = write!(
            output,
            "\x1b[{};{}H",
            origin_row + 4 + visible_index as u16,
            origin_column + 2
        );
        write_session_manager_field(
            output,
            if selected { "› " } else { "  " },
            marker_width,
            if selected {
                MOCHA_GREEN
            } else {
                MOCHA_OVERLAY_0
            },
            background,
            selected,
        );
        write_session_manager_field(
            output,
            &session.name,
            name_width,
            if current { MOCHA_GREEN } else { MOCHA_TEAL },
            background,
            true,
        );
        write_session_manager_field(output, &layout, layout_width, MOCHA_TEXT, background, false);
        if status_width > 0 {
            write_session_manager_field(output, state, status_width, state_color, background, true);
        }
        if created_width > 0 {
            write_session_manager_field(
                output,
                &session_created(session.created_at),
                created_width,
                MOCHA_SUBTEXT_0,
                background,
                false,
            );
        }
        write_session_manager_field(output, "", trailing_width, MOCHA_TEXT, background, false);
    }

    if manager.sessions.is_empty() {
        let message = if manager.query.is_empty() {
            "No sessions available".to_owned()
        } else {
            format!("No matches · Enter to create ‘{}’", manager.query)
        };
        let (message, _) = truncate_to_display_width(&message, body_width);
        let _ = write!(output, "\x1b[{};{}H", origin_row + 4, origin_column + 2);
        write_rgb_style(output, MOCHA_PEACH, Some(MOCHA_BASE), false);
        output.extend_from_slice(message.as_bytes());
    }

    let editing = manager.rename_input.is_some();
    let first_actions = if editing {
        vec![
            ("open", "Rename"),
            ("cancel", "Cancel"),
            ("backspace", "Erase"),
        ]
    } else {
        vec![
            ("open", "Open/Create"),
            ("rename", "Rename"),
            ("delete", "Delete"),
        ]
    };
    let second_actions = if editing {
        vec![]
    } else if manager.searching {
        vec![
            ("complete", "Complete"),
            ("cancel", "Back"),
            ("save", "Save"),
            ("disconnect", "Disconnect"),
        ]
    } else {
        vec![
            ("save", "Save"),
            ("disconnect", "Disconnect"),
            ("cancel", "Close"),
        ]
    };
    for (row, actions) in [first_actions, second_actions].iter().enumerate() {
        let labels: Vec<_> = actions
            .iter()
            .map(|(action, description)| (key_label(action), *description))
            .filter(|(key, _)| !key.is_empty())
            .collect();
        let hints: Vec<_> = labels
            .iter()
            .map(|(key, description)| (key.as_str(), *description))
            .collect();
        draw_session_manager_help_line(
            output,
            origin_row + rows - 3 + row as u16,
            origin_column + 2,
            body_width,
            if row == 0 { "Help: " } else { "      " },
            &hints,
        );
    }
}

fn write_session_manager_field(
    output: &mut Vec<u8>,
    text: &str,
    width: usize,
    foreground: Rgb,
    background: Rgb,
    bold: bool,
) {
    if width == 0 {
        return;
    }
    let (text, text_width) = truncate_to_display_width(text, width);
    write_rgb_style(output, foreground, Some(background), bold);
    output.extend_from_slice(text.as_bytes());
    output.extend_from_slice(" ".repeat(width.saturating_sub(text_width)).as_bytes());
}

fn draw_session_manager_help_line(
    output: &mut Vec<u8>,
    row: u16,
    column: u16,
    width: usize,
    prefix: &str,
    hints: &[(&str, &str)],
) {
    let _ = write!(output, "\x1b[{row};{column}H");
    let (prefix, prefix_width) = truncate_to_display_width(prefix, width);
    write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
    output.extend_from_slice(prefix.as_bytes());
    let mut used = prefix_width;
    for (index, (key, description)) in hints.iter().enumerate() {
        let separator = if index == 0 { "" } else { "  " };
        let required = UnicodeWidthStr::width(separator)
            + UnicodeWidthStr::width(*key)
            + 1
            + UnicodeWidthStr::width(*description);
        if used + required > width {
            continue;
        }
        write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
        output.extend_from_slice(separator.as_bytes());
        write_rgb_style(output, MOCHA_PINK, Some(MOCHA_BASE), true);
        output.extend_from_slice(key.as_bytes());
        write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
        output.extend_from_slice(b" ");
        output.extend_from_slice(description.as_bytes());
        used += required;
    }
}

fn session_created(created_at: u64) -> String {
    if created_at == 0 {
        return "created unknown".to_owned();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let seconds = now.saturating_sub(created_at);
    if seconds < 60 {
        format!("created {seconds}s ago")
    } else if seconds < 3_600 {
        format!("created {}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("created {}h ago", seconds / 3_600)
    } else {
        format!("created {}d ago", seconds / 86_400)
    }
}

fn draw_help(output: &mut Vec<u8>, snapshot: &FrameSnapshot) {
    let Some(help) = &snapshot.help else {
        return;
    };
    let (box_column, box_row, columns, rows) = help_rect(snapshot.content_size, help.hints.len());
    if columns < 4 || rows < 7 {
        return;
    }
    let origin_column = snapshot.content_origin.0 + box_column;
    let origin_row = snapshot.content_origin.1 + box_row;
    let inner_width = usize::from(columns - 2);
    let blank = " ".repeat(usize::from(columns));
    for row in 0..rows {
        let _ = write!(output, "\x1b[{};{}H", origin_row + row, origin_column);
        write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
        output.extend_from_slice(blank.as_bytes());
    }

    let title = "─ Keybindings ";
    let (title, title_width) = truncate_to_display_width(title, inner_width);
    let _ = write!(output, "\x1b[{origin_row};{origin_column}H");
    write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), true);
    output.extend_from_slice("┌".as_bytes());
    output.extend_from_slice(title.as_bytes());
    for _ in title_width..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    output.extend_from_slice("┐".as_bytes());

    for row in 1..rows - 1 {
        let _ = write!(output, "\x1b[{};{}H", origin_row + row, origin_column);
        write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), false);
        output.extend_from_slice("│".as_bytes());
        let _ = write!(
            output,
            "\x1b[{};{}H",
            origin_row + row,
            origin_column + columns - 1
        );
        output.extend_from_slice("│".as_bytes());
    }

    let mode = format!("Mode: {}", help.mode.to_ascii_uppercase());
    let (mode, _) = truncate_to_display_width(&mode, inner_width.saturating_sub(2));
    let _ = write!(output, "\x1b[{};{}H", origin_row + 1, origin_column + 2);
    write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), true);
    output.extend_from_slice(mode.as_bytes());

    let available_rows = usize::from(rows - 4);
    let column_count = if columns >= 48 { 2 } else { 1 };
    let column_width = inner_width / column_count;
    let visible_hints = help
        .hints
        .iter()
        .take(available_rows * column_count)
        .collect::<Vec<_>>();
    let key_widths = (0..column_count)
        .map(|column| {
            visible_hints
                .iter()
                .skip(column * available_rows)
                .take(available_rows)
                .filter_map(|hint| hint.split_once('=').map(|(key, _)| key))
                .map(UnicodeWidthStr::width)
                .max()
                .unwrap_or(0)
                .min(column_width.saturating_sub(4))
        })
        .collect::<Vec<_>>();
    for (index, hint) in visible_hints.into_iter().enumerate() {
        let row = index % available_rows;
        let column = index / available_rows;
        let available = column_width.saturating_sub(2);
        let _ = write!(
            output,
            "\x1b[{};{}H",
            origin_row + 2 + u16::try_from(row).unwrap_or(u16::MAX),
            origin_column + 2 + u16::try_from(column * column_width).unwrap_or(u16::MAX)
        );
        if let Some((key, action)) = hint.split_once('=') {
            let (key, key_width) = truncate_to_display_width(key, key_widths[column]);
            write_rgb_style(output, MOCHA_PINK, Some(MOCHA_BASE), true);
            output.extend_from_slice(key.as_bytes());

            let padding = key_widths[column]
                .saturating_sub(key_width)
                .saturating_add(2)
                .min(available.saturating_sub(key_width));
            write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
            output.extend_from_slice(" ".repeat(padding).as_bytes());
            let action = action_hint_label(action);
            let (action, _) =
                truncate_to_display_width(&action, available.saturating_sub(key_width + padding));
            output.extend_from_slice(action.as_bytes());
        } else {
            let (label, _) = truncate_to_display_width(hint, available);
            write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
            output.extend_from_slice(label.as_bytes());
        }
    }

    let footer = "─ Esc close · press a key to run ";
    let (footer, footer_width) = truncate_to_display_width(footer, inner_width);
    let _ = write!(output, "\x1b[{};{}H", origin_row + rows - 1, origin_column);
    write_rgb_style(output, MOCHA_GREEN, Some(MOCHA_BASE), true);
    output.extend_from_slice("└".as_bytes());
    output.extend_from_slice(footer.as_bytes());
    for _ in footer_width..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    output.extend_from_slice("┘".as_bytes());
}

fn draw_notification(output: &mut Vec<u8>, snapshot: &FrameSnapshot) {
    let Some(message) = &snapshot.notification else {
        return;
    };
    let (box_column, box_row, columns, rows) = notification_rect(snapshot.content_size, message);
    if columns < 4 || rows < 3 {
        return;
    }
    let origin_column = snapshot.content_origin.0 + box_column;
    let origin_row = snapshot.content_origin.1 + box_row;
    let inner_width = usize::from(columns - 2);

    for row in 0..rows {
        let _ = write!(output, "\x1b[{};{}H", origin_row + row, origin_column);
        write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
        output.extend_from_slice(" ".repeat(usize::from(columns)).as_bytes());
    }

    let title = "─ Rustmux Warning ";
    let (title, title_width) = truncate_to_display_width(title, inner_width);
    let _ = write!(output, "\x1b[{origin_row};{origin_column}H");
    write_rgb_style(output, MOCHA_YELLOW, Some(MOCHA_BASE), true);
    output.extend_from_slice("┌".as_bytes());
    output.extend_from_slice(title.as_bytes());
    for _ in title_width..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    output.extend_from_slice("┐".as_bytes());

    let _ = write!(output, "\x1b[{};{}H", origin_row + 1, origin_column);
    write_rgb_style(output, MOCHA_YELLOW, Some(MOCHA_BASE), false);
    output.extend_from_slice("│".as_bytes());
    let (message, _) = truncate_to_display_width(message, inner_width.saturating_sub(2));
    let _ = write!(output, "\x1b[{};{}H", origin_row + 1, origin_column + 2);
    write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
    output.extend_from_slice(message.as_bytes());
    let _ = write!(
        output,
        "\x1b[{};{}H",
        origin_row + 1,
        origin_column + columns - 1
    );
    write_rgb_style(output, MOCHA_YELLOW, Some(MOCHA_BASE), false);
    output.extend_from_slice("│".as_bytes());

    let _ = write!(
        output,
        "\x1b[{};{}H└{}┘",
        origin_row + 2,
        origin_column,
        "─".repeat(inner_width)
    );
}

fn draw_bottom_status(output: &mut Vec<u8>, snapshot: &FrameSnapshot) {
    let (width, height) = snapshot.terminal_size;
    let _ = write!(output, "\x1b[{height};1H");
    write_rgb_style(output, MOCHA_TEXT, Some(MOCHA_BASE), false);
    output.extend_from_slice(b"\x1b[2K");
    draw_powerline_segments(
        output,
        &status_segments(snapshot, usize::from(width)),
        usize::from(width),
        MOCHA_BASE,
    );
    output.extend_from_slice(b"\x1b[0m");
}

fn draw_terminal_border(output: &mut Vec<u8>, title: &str, width: u16, active: bool) {
    if width == 0 {
        return;
    }
    output.extend_from_slice(b"\x1b[0;49m\x1b[2K");
    write_rgb_style(
        output,
        if active { MOCHA_GREEN } else { MOCHA_OVERLAY_0 },
        None,
        false,
    );
    output.extend_from_slice("┌".as_bytes());
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

fn draw_bottom_border(output: &mut Vec<u8>, width: u16, active: bool) {
    if width == 0 {
        return;
    }
    write_rgb_style(
        output,
        if active { MOCHA_GREEN } else { MOCHA_OVERLAY_0 },
        None,
        false,
    );
    output.extend_from_slice("└".as_bytes());
    let inner_width = usize::from(width.saturating_sub(2));
    for _ in 0..inner_width {
        output.extend_from_slice("─".as_bytes());
    }
    if width > 1 {
        output.extend_from_slice("┘".as_bytes());
    }
}

fn write_rgb_style(output: &mut Vec<u8>, foreground: Rgb, background: Option<Rgb>, bold: bool) {
    let (foreground_red, foreground_green, foreground_blue) = foreground;
    let _ = write!(
        output,
        "\x1b[{};38;2;{foreground_red};{foreground_green};{foreground_blue}",
        if bold { 1 } else { 0 }
    );
    if let Some((background_red, background_green, background_blue)) = background {
        let _ = write!(
            output,
            ";48;2;{background_red};{background_green};{background_blue}"
        );
    } else {
        output.extend_from_slice(b";49");
    }
    output.push(b'm');
}

fn draw_powerline_segments(
    output: &mut Vec<u8>,
    segments: &[(String, Rgb)],
    width: usize,
    bar_background: Rgb,
) {
    let mut used = 0;
    for (text, background) in segments {
        if used >= width {
            break;
        }
        let available = width.saturating_sub(used).saturating_sub(2);
        let label = format!(" {text} ");
        let (label, label_width) = truncate_to_display_width(&label, available);
        if label_width == 0 {
            break;
        }
        write_rgb_style(output, bar_background, Some(*background), false);
        output.extend_from_slice(POWERLINE_RIGHT.as_bytes());
        write_rgb_style(output, MOCHA_CRUST, Some(*background), true);
        output.extend_from_slice(label.as_bytes());
        write_rgb_style(output, *background, Some(bar_background), false);
        output.extend_from_slice(POWERLINE_RIGHT.as_bytes());
        used += label_width + 2;
    }
}

fn write_hyperlink(output: &mut Vec<u8>, hyperlink: Option<&str>) {
    if let Some(open) = hyperlink {
        output.extend_from_slice(open.as_bytes());
    } else {
        output.extend_from_slice(b"\x1b]8;;\x1b\\");
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
