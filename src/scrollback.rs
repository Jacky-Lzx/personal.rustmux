use crate::render::{CellStyle, write_cell_style};

// Encode only SGR styles and text, rather than replaying terminal control output.
pub(super) fn colored_row(screen: &vt100::Screen, row: u16) -> String {
    let plain = CellStyle::plain();
    let end = (0..screen.size().1).rfind(|column| {
        screen.cell(row, *column).is_some_and(|cell| {
            cell.has_contents() || cell.is_wide_continuation() || CellStyle::from(cell) != plain
        })
    });
    let Some(end) = end else {
        return String::new();
    };
    let mut output = Vec::new();
    let mut current = plain;
    for column in 0..=end {
        let Some(cell) = screen.cell(row, column) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let style = CellStyle::from(cell);
        if style != current {
            write_cell_style(&mut output, style);
            current = style;
        }
        if cell.has_contents() {
            output.extend_from_slice(cell.contents().as_bytes());
        } else {
            output.push(b' ');
        }
    }
    if current != plain {
        output.extend_from_slice(b"\x1b[0m");
    }
    String::from_utf8(output).expect("terminal cells and SGR styles are UTF-8")
}

pub(super) fn sanitize_saved_line(line: &str, colored: bool) -> String {
    let mut result = String::with_capacity(line.len());
    let mut remaining = line;
    while !remaining.is_empty() {
        if colored && let Some(parameters) = remaining.strip_prefix("\x1b[") {
            let length = parameters
                .bytes()
                .take_while(|byte| byte.is_ascii_digit() || *byte == b';')
                .count();
            if parameters.as_bytes().get(length) == Some(&b'm') {
                result.push_str(&remaining[..length + 3]);
                remaining = &remaining[length + 3..];
                continue;
            }
        }
        let ch = remaining.chars().next().unwrap();
        if !ch.is_control() {
            result.push(ch);
        }
        remaining = &remaining[ch.len_utf8()..];
    }
    result
}
