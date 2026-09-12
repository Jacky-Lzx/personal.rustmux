use std::fmt::Write;

use crate::session::SessionInfo;
use crate::theme::{Rgb, Theme};

fn age(timestamp: u64, now: u64) -> String {
    if timestamp == 0 {
        return "—".to_owned();
    }
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

pub(super) fn format_sessions(
    sessions: &[SessionInfo],
    current: &str,
    now: u64,
    theme: &Theme,
    color: bool,
) -> String {
    if sessions.is_empty() {
        return "no sessions\n".to_owned();
    }
    let headers = ["SESSION", "LAYOUT", "STATUS", "CREATED", "LAST CONNECTED"];
    let rows: Vec<_> = sessions
        .iter()
        .map(|session| {
            let status = session.status();
            [
                if session.name == current {
                    format!("{} *", session.name)
                } else {
                    session.name.clone()
                },
                format!("{}t · {}p", session.tabs, session.panes),
                status.to_owned(),
                age(session.created_at, now),
                if session.name == current || session.connected {
                    "Now".to_owned()
                } else {
                    age(session.last_connected_at, now)
                },
            ]
        })
        .collect();
    let mut widths = headers.map(unicode_width::UnicodeWidthStr::width);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(unicode_width::UnicodeWidthStr::width(cell.as_str()));
        }
    }
    let mut output = String::new();
    let count = sessions.len();
    style(&mut output, theme.accent, color, true);
    let _ = writeln!(
        output,
        "{count} session{}",
        if count == 1 { "" } else { "s" }
    );
    reset(&mut output, color);
    output.push('\n');
    let write_row = |output: &mut String, row: &[&str], colors: &[Rgb], bold| {
        for (index, cell) in row.iter().enumerate() {
            style(output, colors[index], color, bold);
            output.push_str(cell);
            reset(output, color);
            if index + 1 < row.len() {
                output.push_str(
                    &" ".repeat(widths[index] - unicode_width::UnicodeWidthStr::width(*cell) + 2),
                );
            }
        }
        output.push('\n');
    };
    write_row(&mut output, &headers, &[theme.muted; 5], true);
    style(&mut output, theme.border, color, false);
    output.push_str(&"─".repeat(widths.iter().sum::<usize>() + 2 * (headers.len() - 1)));
    reset(&mut output, color);
    output.push('\n');
    for row in &rows {
        let state_color = match row[2].as_str() {
            "ATTACHED" => theme.orange,
            "SAVED" => theme.purple,
            _ => theme.secondary,
        };
        let cells = row.each_ref().map(String::as_str);
        write_row(
            &mut output,
            &cells,
            &[
                theme.teal,
                theme.foreground,
                state_color,
                theme.muted,
                theme.foreground,
            ],
            false,
        );
    }
    output
}

fn style(output: &mut String, (r, g, b): Rgb, color: bool, bold: bool) {
    if color {
        let _ = write!(output, "\x1b[{};38;2;{r};{g};{b}m", u8::from(bold));
    }
}
fn reset(output: &mut String, color: bool) {
    if color {
        output.push_str("\x1b[0m");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_preserves_spacing_and_distinguishes_saved_from_running() {
        let sessions = vec![
            SessionInfo {
                name: "main".into(),
                tabs: 2,
                panes: 3,
                connected: true,
                running: true,
                created_at: 59,
                last_connected_at: 59,
                saved: true,
            },
            SessionInfo {
                name: "a-long-saved-session".into(),
                tabs: 1,
                panes: 1,
                connected: false,
                running: false,
                created_at: 0,
                last_connected_at: 40,
                saved: true,
            },
        ];
        let output = format_sessions(&sessions, "main", 100, &Theme::default(), false);
        assert!(output.contains("2 sessions"));
        assert!(output.contains("ATTACHED"));
        assert!(output.contains("main *"));
        assert!(output.contains("SAVED"));
        assert!(output.contains("41s ago  Now"));
        assert!(output.contains("1m ago"));
        assert!(!output.contains('\x1b'));
        assert_eq!(
            format_sessions(&[], "", 100, &Theme::default(), false),
            "no sessions\n"
        );
    }
}
