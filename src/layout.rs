use nix::pty::Winsize;

use super::Result;

const MAX_TERMINAL_CELLS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PaneNode {
    Leaf(usize),
    Split {
        axis: SplitAxis,
        ratio: u16,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PaneRect {
    pub(super) column: u16,
    pub(super) row: u16,
    pub(super) width: u16,
    pub(super) height: u16,
}

pub(super) fn content_size_for((columns, rows): (u16, u16), compact: bool) -> (u16, u16) {
    (
        columns.saturating_sub(2).max(1),
        rows.saturating_sub(if compact { 2 } else { 4 }).max(1),
    )
}

pub(super) fn validate_terminal_size((columns, rows): (u16, u16)) -> Result<()> {
    if columns == 0 || rows == 0 {
        return Err("terminal dimensions must be non-zero".into());
    }
    if usize::from(columns) * usize::from(rows) > MAX_TERMINAL_CELLS {
        return Err(format!(
            "terminal dimensions {columns}x{rows} exceed the supported canvas size"
        )
        .into());
    }
    Ok(())
}

pub(super) fn content_rect_for(terminal_size: (u16, u16), compact: bool) -> PaneRect {
    let (width, height) = content_size_for(terminal_size, compact);
    PaneRect {
        column: 0,
        row: 0,
        width,
        height,
    }
}

pub(super) fn tiled_content_rect_for((columns, rows): (u16, u16), compact: bool) -> PaneRect {
    PaneRect {
        column: 0,
        row: 0,
        width: columns.max(1),
        height: rows.saturating_sub(if compact { 1 } else { 2 }).max(1),
    }
}

pub(super) fn split_pane(
    node: &mut PaneNode,
    target: usize,
    new_id: usize,
    axis: SplitAxis,
) -> bool {
    match node {
        PaneNode::Leaf(id) if *id == target => {
            *node = PaneNode::Split {
                axis,
                ratio: 500,
                first: Box::new(PaneNode::Leaf(target)),
                second: Box::new(PaneNode::Leaf(new_id)),
            };
            true
        }
        PaneNode::Leaf(_) => false,
        PaneNode::Split { first, second, .. } => {
            split_pane(first, target, new_id, axis) || split_pane(second, target, new_id, axis)
        }
    }
}

pub(super) fn remove_pane(node: PaneNode, target: usize) -> Option<PaneNode> {
    match node {
        PaneNode::Leaf(id) => (id != target).then_some(PaneNode::Leaf(id)),
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let first = remove_pane(*first, target);
            let second = remove_pane(*second, target);
            match (first, second) {
                (Some(first), Some(second)) => Some(PaneNode::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            }
        }
    }
}

pub(super) fn pane_ids(node: &PaneNode) -> Vec<usize> {
    let mut ids = Vec::new();
    fn collect(node: &PaneNode, ids: &mut Vec<usize>) {
        match node {
            PaneNode::Leaf(id) => ids.push(*id),
            PaneNode::Split { first, second, .. } => {
                collect(first, ids);
                collect(second, ids);
            }
        }
    }
    collect(node, &mut ids);
    ids
}

pub(super) fn pane_rects(node: &PaneNode, rect: PaneRect) -> Vec<(usize, PaneRect)> {
    let mut rects = Vec::new();
    fn layout(node: &PaneNode, rect: PaneRect, rects: &mut Vec<(usize, PaneRect)>) {
        match node {
            PaneNode::Leaf(id) => rects.push((*id, rect)),
            PaneNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (a, b) = match axis {
                    SplitAxis::Vertical => {
                        let first_width = split_extent(rect.width, *ratio);
                        (
                            PaneRect {
                                width: first_width,
                                ..rect
                            },
                            PaneRect {
                                column: rect.column + first_width,
                                width: rect.width - first_width,
                                ..rect
                            },
                        )
                    }
                    SplitAxis::Horizontal => {
                        let first_height = split_extent(rect.height, *ratio);
                        (
                            PaneRect {
                                height: first_height,
                                ..rect
                            },
                            PaneRect {
                                row: rect.row + first_height,
                                height: rect.height - first_height,
                                ..rect
                            },
                        )
                    }
                };
                layout(first, a, rects);
                layout(second, b, rects);
            }
        }
    }
    layout(node, rect, &mut rects);
    rects
}

fn split_extent(total: u16, ratio: u16) -> u16 {
    let extent = u32::from(total) * u32::from(ratio) / 1_000;
    let minimum = total.min(3);
    (extent as u16).clamp(minimum, total.saturating_sub(minimum).max(minimum))
}

pub(super) fn resize_pane(node: &mut PaneNode, target: usize, direction: Direction) -> bool {
    const STEP: u16 = 50;
    const MIN_RATIO: u16 = 100;
    const MAX_RATIO: u16 = 900;

    let PaneNode::Split {
        axis,
        ratio,
        first,
        second,
    } = node
    else {
        return false;
    };
    let in_first = pane_ids(first).contains(&target);
    let in_second = pane_ids(second).contains(&target);
    if !in_first && !in_second {
        return false;
    }
    let child_resized = if in_first {
        resize_pane(first, target, direction)
    } else {
        resize_pane(second, target, direction)
    };
    if child_resized {
        return true;
    }
    let adjustment = match (*axis, direction, in_first, in_second) {
        (SplitAxis::Vertical, Direction::Right, true, _) => i16::try_from(STEP).unwrap(),
        (SplitAxis::Vertical, Direction::Left, _, true) => -i16::try_from(STEP).unwrap(),
        (SplitAxis::Horizontal, Direction::Down, true, _) => i16::try_from(STEP).unwrap(),
        (SplitAxis::Horizontal, Direction::Up, _, true) => -i16::try_from(STEP).unwrap(),
        _ => return false,
    };
    let updated = (i32::from(*ratio) + i32::from(adjustment))
        .clamp(i32::from(MIN_RATIO), i32::from(MAX_RATIO)) as u16;
    let changed = updated != *ratio;
    *ratio = updated;
    changed
}

pub(super) fn pane_pty_size(rect: PaneRect, framed: bool) -> (u16, u16) {
    if framed {
        (
            rect.width.saturating_sub(2).max(1),
            rect.height.saturating_sub(2).max(1),
        )
    } else {
        (rect.width.max(1), rect.height.max(1))
    }
}

pub(super) fn rect_in_direction(from: PaneRect, to: PaneRect, direction: Direction) -> bool {
    let from_center = (
        i32::from(from.column) * 2 + i32::from(from.width),
        i32::from(from.row) * 2 + i32::from(from.height),
    );
    let to_center = (
        i32::from(to.column) * 2 + i32::from(to.width),
        i32::from(to.row) * 2 + i32::from(to.height),
    );
    match direction {
        Direction::Left => to_center.0 < from_center.0,
        Direction::Right => to_center.0 > from_center.0,
        Direction::Up => to_center.1 < from_center.1,
        Direction::Down => to_center.1 > from_center.1,
    }
}

pub(super) fn directional_distance(
    from: PaneRect,
    to: PaneRect,
    direction: Direction,
) -> (i32, i32) {
    let fx = i32::from(from.column) * 2 + i32::from(from.width);
    let fy = i32::from(from.row) * 2 + i32::from(from.height);
    let tx = i32::from(to.column) * 2 + i32::from(to.width);
    let ty = i32::from(to.row) * 2 + i32::from(to.height);
    match direction {
        Direction::Left | Direction::Right => ((tx - fx).abs(), (ty - fy).abs()),
        Direction::Up | Direction::Down => ((ty - fy).abs(), (tx - fx).abs()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FloatingLayout {
    pub(super) column: u16,
    pub(super) row: u16,
    pub(super) width: u16,
    pub(super) height: u16,
}

impl FloatingLayout {
    pub(super) fn content_size(self) -> (u16, u16) {
        (
            self.width.saturating_sub(2).max(1),
            self.height.saturating_sub(2).max(1),
        )
    }
}

pub(super) fn floating_layout_for(terminal_size: (u16, u16), compact: bool) -> FloatingLayout {
    let (available_width, available_height) = content_size_for(terminal_size, compact);
    let width = available_width
        .saturating_mul(3)
        .checked_div(4)
        .unwrap_or(available_width)
        .max(12)
        .min(available_width);
    let height = available_height
        .saturating_mul(7)
        .checked_div(10)
        .unwrap_or(available_height)
        .max(5)
        .min(available_height);
    FloatingLayout {
        column: 2 + available_width.saturating_sub(width) / 2,
        row: 3 + available_height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn window_winsize_for(
    terminal_size: (u16, u16),
    terminal_pixels: (u16, u16),
    floating: bool,
    compact: bool,
) -> Winsize {
    if !floating {
        return content_winsize_for(terminal_size, terminal_pixels, compact);
    }
    let (columns, rows) = floating_layout_for(terminal_size, compact).content_size();
    let (outer_columns, outer_rows) = terminal_size;
    let cell_width = terminal_pixels.0.checked_div(outer_columns).unwrap_or(0);
    let cell_height = terminal_pixels.1.checked_div(outer_rows).unwrap_or(0);
    Winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: cell_width.saturating_mul(columns),
        ws_ypixel: cell_height.saturating_mul(rows),
    }
}

pub(super) fn content_winsize_for(
    terminal_size: (u16, u16),
    terminal_pixels: (u16, u16),
    compact: bool,
) -> Winsize {
    let (columns, rows) = content_size_for(terminal_size, compact);
    let (outer_columns, outer_rows) = terminal_size;
    let (outer_width, outer_height) = terminal_pixels;
    let cell_width = outer_width.checked_div(outer_columns).unwrap_or(0);
    let cell_height = outer_height.checked_div(outer_rows).unwrap_or(0);
    Winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: cell_width.saturating_mul(columns),
        ws_ypixel: cell_height.saturating_mul(rows),
    }
}
