use std::collections::VecDeque;
use std::env;
use std::ffi::OsString;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use nix::pty::Winsize;

use super::ESCAPE_SEQUENCE_TIMEOUT;

const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const MAX_KITTY_COMMAND_BYTES: usize = 64 * 1024 * 1024;
const MAX_KITTY_DND_SEQUENCE_BYTES: usize = 16 * 1024;
const KITTY_DND_PREFIX: &[u8] = b"\x1b]72;";
const KITTY_CLIPBOARD_PREFIX: &[u8] = b"\x1b]5522;";
const KITTY_FILE_PREFIX: &[u8] = b"\x1b]5113;";
const STRING_TERMINATOR: &[u8] = b"\x1b\\";
const KITTY_KEYBOARD_FLAGS: u8 = 0b1_1111;
const MAX_MODE_STACK_DEPTH: usize = 32;
const MOCHA_TEXT: (u8, u8, u8) = (205, 214, 244);
const MOCHA_BASE: (u8, u8, u8) = (30, 30, 46);

pub(super) fn terminal_parser_size(columns: u16, rows: u16) -> (u16, u16) {
    // vt100's wrapping logic requires room for a double-width character and
    // for scrolling off the first row. Tiny outer terminals can otherwise
    // produce a one-cell parser that panics on ordinary output.
    (columns.max(2), rows.max(2))
}

#[derive(Default)]
pub(super) struct TerminalMetadata {
    pub(super) title: String,
    pub(super) current_directory: Option<PathBuf>,
}

impl vt100::Callbacks for TerminalMetadata {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title)
            .chars()
            .filter(|character| !character.is_control())
            .collect();
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let [b"7", uri] = params
            && let Some(path) = osc7_path(uri)
        {
            self.current_directory = Some(path);
        }
    }
}

pub(super) fn osc7_path(uri: &[u8]) -> Option<PathBuf> {
    let uri = uri.strip_prefix(b"file://")?;
    let path_start = uri.iter().position(|byte| *byte == b'/')?;
    let encoded = &uri[path_start..];
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            let high = hex_digit(*encoded.get(index + 1)?)?;
            let low = hex_digit(*encoded.get(index + 2)?)?;
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    (!decoded.contains(&0) && decoded.starts_with(b"/"))
        .then(|| PathBuf::from(OsString::from_vec(decoded)))
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct SemanticOutputCapture {
    state: TextCaptureState,
    capturing: bool,
    bell_received: bool,
    pub(super) semantic_boundaries: bool,
    current: Vec<u8>,
    last: Vec<u8>,
    pub(super) command_started_at: Option<Instant>,
}

#[derive(Default)]
enum TextCaptureState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
    String,
    StringEscape,
}

impl SemanticOutputCapture {
    pub(super) fn process(&mut self, bytes: &[u8]) -> Vec<Duration> {
        let mut completions = Vec::new();
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                TextCaptureState::Ground => match byte {
                    0x1b => TextCaptureState::Escape,
                    0x07 => {
                        self.bell_received = true;
                        TextCaptureState::Ground
                    }
                    b'\r' => TextCaptureState::Ground,
                    b'\n' | b'\t' if self.capturing => {
                        self.push(byte);
                        TextCaptureState::Ground
                    }
                    0x08 if self.capturing => {
                        self.current.pop();
                        TextCaptureState::Ground
                    }
                    0x20..=0x7e | 0x80..=0xff if self.capturing => {
                        self.push(byte);
                        TextCaptureState::Ground
                    }
                    _ => TextCaptureState::Ground,
                },
                TextCaptureState::Escape => match byte {
                    b'[' => TextCaptureState::Csi,
                    b']' => TextCaptureState::Osc(Vec::new()),
                    b'P' | b'X' | b'^' | b'_' => TextCaptureState::String,
                    _ => TextCaptureState::Ground,
                },
                TextCaptureState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        TextCaptureState::Ground
                    } else {
                        TextCaptureState::Csi
                    }
                }
                TextCaptureState::Osc(mut control) => match byte {
                    0x07 => {
                        if let Some(duration) = self.finish_osc(&control) {
                            completions.push(duration);
                        }
                        TextCaptureState::Ground
                    }
                    0x1b => TextCaptureState::OscEscape(control),
                    _ => {
                        if control.len() < 1024 {
                            control.push(byte);
                        }
                        TextCaptureState::Osc(control)
                    }
                },
                TextCaptureState::OscEscape(mut control) => {
                    if byte == b'\\' {
                        if let Some(duration) = self.finish_osc(&control) {
                            completions.push(duration);
                        }
                        TextCaptureState::Ground
                    } else {
                        if control.len() < 1024 {
                            control.extend_from_slice(&[0x1b, byte]);
                        }
                        TextCaptureState::Osc(control)
                    }
                }
                TextCaptureState::String => {
                    if byte == 0x1b {
                        TextCaptureState::StringEscape
                    } else {
                        TextCaptureState::String
                    }
                }
                TextCaptureState::StringEscape => {
                    if byte == b'\\' {
                        TextCaptureState::Ground
                    } else if byte == 0x1b {
                        TextCaptureState::StringEscape
                    } else {
                        TextCaptureState::String
                    }
                }
            };
        }
        completions
    }

    pub(super) fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell_received)
    }

    fn finish_osc(&mut self, control: &[u8]) -> Option<Duration> {
        let marker = control
            .strip_prefix(b"133;")
            .and_then(|value| value.first().copied());
        match marker {
            Some(b'C') => {
                self.current.clear();
                self.capturing = true;
                self.semantic_boundaries = true;
                self.command_started_at = Some(Instant::now());
                None
            }
            Some(b'D') if self.capturing => self.finish_command(),
            Some(b'A') if self.capturing => self.finish_command(),
            _ => None,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.current.len() < MAX_CAPTURE_BYTES {
            self.current.push(byte);
        }
    }

    fn finish_command(&mut self) -> Option<Duration> {
        while self.current.last().is_some_and(u8::is_ascii_whitespace) {
            self.current.pop();
        }
        self.last = std::mem::take(&mut self.current);
        self.capturing = false;
        self.semantic_boundaries = false;
        self.command_started_at
            .take()
            .map(|started| started.elapsed())
    }

    pub(super) fn last_output(&self) -> String {
        if self.capturing && !self.semantic_boundaries {
            fallback_command_output(&self.current)
        } else {
            String::from_utf8_lossy(&self.last).into_owned()
        }
    }

    pub(super) fn command_submitted(&mut self) {
        if self.capturing && self.semantic_boundaries {
            return;
        }
        if self.capturing && !self.semantic_boundaries {
            self.last = fallback_command_output(&self.current).into_bytes();
        }
        self.current.clear();
        self.capturing = true;
        self.semantic_boundaries = false;
        self.command_started_at = Some(Instant::now());
    }
}

pub(super) fn fallback_command_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().collect::<Vec<_>>();
    if !lines.is_empty() {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.pop();
    }
    lines.join("\n").trim_end().to_owned()
}

#[derive(Default)]
pub(super) struct KittyDndParser {
    pending: Vec<u8>,
    pending_since: Option<Instant>,
}

#[derive(Default)]
pub(super) struct KittyIpcParser {
    pending: Vec<u8>,
    pending_since: Option<Instant>,
}

#[derive(Default)]
pub(super) struct KittyIpcOutput {
    pub(super) terminal: Vec<u8>,
    pub(super) commands: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Hyperlink {
    parameters: String,
    uri: String,
}

#[derive(Default)]
enum HyperlinkSequenceState {
    #[default]
    Ground,
    Escape,
    Csi,
    String,
    StringEscape,
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
}

/// Mirrors OSC 8 state for the visible screen while vt100 handles the rest of
/// terminal emulation. Links are re-emitted by the compositor when panes move.
#[derive(Default)]
pub(super) struct HyperlinkTracker {
    state: HyperlinkSequenceState,
    active: Option<Hyperlink>,
    cells: Vec<Option<Hyperlink>>,
    rows: u16,
    columns: u16,
}

impl HyperlinkTracker {
    pub(super) fn resize(&mut self, rows: u16, columns: u16) {
        if self.rows == rows && self.columns == columns {
            return;
        }
        let mut resized = vec![None; usize::from(rows) * usize::from(columns)];
        for row in 0..self.rows.min(rows) {
            for column in 0..self.columns.min(columns) {
                resized[usize::from(row) * usize::from(columns) + usize::from(column)] = self
                    .cells
                    .get(usize::from(row) * usize::from(self.columns) + usize::from(column))
                    .cloned()
                    .flatten();
            }
        }
        self.rows = rows;
        self.columns = columns;
        self.cells = resized;
    }

    pub(super) fn process(&mut self, bytes: &[u8], terminal: &mut vt100::Parser<TerminalMetadata>) {
        let (rows, columns) = terminal.screen().size();
        self.resize(rows, columns);
        if matches!(self.state, HyperlinkSequenceState::Ground)
            && self.active.is_none()
            && self.cells.iter().all(Option::is_none)
            && !may_contain_osc8(bytes)
        {
            terminal.process(bytes);
            return;
        }
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            let ground = matches!(state, HyperlinkSequenceState::Ground);
            let before = terminal.screen().cursor_position();
            terminal.process(&[byte]);
            let after = terminal.screen().cursor_position();

            if ground && is_printable_terminal_byte(byte) {
                self.set_cell(before.0, before.1, self.active.clone());
                if after.0 == before.0 && after.1 > before.1 + 1 {
                    for column in before.1 + 1..after.1 {
                        self.set_cell(before.0, column, self.active.clone());
                    }
                }
            } else if ground
                && matches!(byte, b'\n' | 0x0b | 0x0c)
                && before.0 + 1 == self.rows
                && after.0 == before.0
            {
                self.scroll_up();
            }

            self.state = match state {
                HyperlinkSequenceState::Ground => match byte {
                    0x1b => HyperlinkSequenceState::Escape,
                    0x9d => HyperlinkSequenceState::Osc(Vec::new()),
                    _ => HyperlinkSequenceState::Ground,
                },
                HyperlinkSequenceState::Escape => match byte {
                    b']' => HyperlinkSequenceState::Osc(Vec::new()),
                    b'[' => HyperlinkSequenceState::Csi,
                    b'P' | b'_' | b'^' => HyperlinkSequenceState::String,
                    0x1b => HyperlinkSequenceState::Escape,
                    _ => HyperlinkSequenceState::Ground,
                },
                HyperlinkSequenceState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        HyperlinkSequenceState::Ground
                    } else if byte == 0x1b {
                        HyperlinkSequenceState::Escape
                    } else {
                        HyperlinkSequenceState::Csi
                    }
                }
                HyperlinkSequenceState::String => match byte {
                    0x1b => HyperlinkSequenceState::StringEscape,
                    0x9c => HyperlinkSequenceState::Ground,
                    _ => HyperlinkSequenceState::String,
                },
                HyperlinkSequenceState::StringEscape => {
                    if byte == b'\\' {
                        HyperlinkSequenceState::Ground
                    } else {
                        HyperlinkSequenceState::String
                    }
                }
                HyperlinkSequenceState::Osc(mut content) => match byte {
                    0x07 | 0x9c => {
                        self.apply_osc(&content);
                        HyperlinkSequenceState::Ground
                    }
                    0x1b => HyperlinkSequenceState::OscEscape(content),
                    _ if content.len() < MAX_KITTY_DND_SEQUENCE_BYTES => {
                        content.push(byte);
                        HyperlinkSequenceState::Osc(content)
                    }
                    _ => HyperlinkSequenceState::Ground,
                },
                HyperlinkSequenceState::OscEscape(mut content) => {
                    if byte == b'\\' {
                        self.apply_osc(&content);
                        HyperlinkSequenceState::Ground
                    } else {
                        if content.len() + 1 < MAX_KITTY_DND_SEQUENCE_BYTES {
                            content.extend_from_slice(&[0x1b, byte]);
                        }
                        HyperlinkSequenceState::Osc(content)
                    }
                }
            };
        }
    }

    pub(super) fn osc8_at(&self, row: u16, column: u16, pane_id: usize) -> Option<String> {
        let link = self
            .cells
            .get(usize::from(row) * usize::from(self.columns) + usize::from(column))?
            .as_ref()?;
        let mut parameters = link
            .parameters
            .split(':')
            .filter(|field| !field.is_empty() && !field.starts_with("id="))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut hasher = DefaultHasher::new();
        link.hash(&mut hasher);
        parameters.push(format!("id=rustmux-{pane_id}-{:x}", hasher.finish()));
        let parameters = parameters.join(":");
        Some(format!("\x1b]8;{parameters};{}\x1b\\", link.uri))
    }

    fn apply_osc(&mut self, content: &[u8]) {
        let Some(body) = content.strip_prefix(b"8;") else {
            return;
        };
        let Some(separator) = body.iter().position(|byte| *byte == b';') else {
            return;
        };
        let Ok(parameters) = std::str::from_utf8(&body[..separator]) else {
            return;
        };
        let Ok(uri) = std::str::from_utf8(&body[separator + 1..]) else {
            return;
        };
        self.active = (!uri.is_empty() && !uri.chars().any(char::is_control)).then(|| Hyperlink {
            parameters: parameters.to_owned(),
            uri: uri.to_owned(),
        });
    }

    fn set_cell(&mut self, row: u16, column: u16, link: Option<Hyperlink>) {
        if row < self.rows && column < self.columns {
            self.cells[usize::from(row) * usize::from(self.columns) + usize::from(column)] = link;
        }
    }

    fn scroll_up(&mut self) {
        if self.columns == 0 || self.rows == 0 {
            return;
        }
        self.cells.rotate_left(usize::from(self.columns));
        let start = self.cells.len() - usize::from(self.columns);
        self.cells[start..].fill(None);
    }
}

fn is_printable_terminal_byte(byte: u8) -> bool {
    matches!(byte, 0x20..=0x7e | 0xa0..=0xff)
}

fn may_contain_osc8(bytes: &[u8]) -> bool {
    [b"\x1b]8;".as_slice(), b"\x9d8;".as_slice()]
        .into_iter()
        .any(|prefix| {
            find_subslice(bytes, prefix).is_some() || partial_prefix_length(bytes, prefix) > 0
        })
}

impl KittyIpcParser {
    pub(super) fn process(&mut self, bytes: &[u8]) -> KittyIpcOutput {
        self.pending.extend_from_slice(bytes);
        let mut output = KittyIpcOutput::default();
        loop {
            let starts = [KITTY_CLIPBOARD_PREFIX, KITTY_FILE_PREFIX];
            let start = starts
                .iter()
                .filter_map(|prefix| find_subslice(&self.pending, prefix))
                .min();
            let Some(start) = start else {
                let retained = starts
                    .iter()
                    .map(|prefix| partial_prefix_length(&self.pending, prefix))
                    .max()
                    .unwrap_or(0);
                let visible = self.pending.len().saturating_sub(retained);
                output.terminal.extend(self.pending.drain(..visible));
                break;
            };
            output.terminal.extend(self.pending.drain(..start));
            let Some(end) = find_subslice(&self.pending, STRING_TERMINATOR) else {
                if self.pending.len() > MAX_KITTY_DND_SEQUENCE_BYTES {
                    output.terminal.push(self.pending.remove(0));
                    continue;
                }
                break;
            };
            let length = end + STRING_TERMINATOR.len();
            output.commands.push(self.pending.drain(..length).collect());
        }

        let starts = [KITTY_CLIPBOARD_PREFIX, KITTY_FILE_PREFIX];
        if self.pending.is_empty() || starts.iter().any(|prefix| self.pending.starts_with(prefix)) {
            self.pending_since = None;
        } else if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
        output
    }

    pub(super) fn flush_if_expired(&mut self) -> Vec<u8> {
        if self
            .pending_since
            .is_some_and(|since| since.elapsed() >= ESCAPE_SEQUENCE_TIMEOUT)
        {
            self.flush()
        } else {
            Vec::new()
        }
    }

    pub(super) fn flush_deadline(&self) -> Option<Instant> {
        self.pending_since
            .and_then(|since| since.checked_add(ESCAPE_SEQUENCE_TIMEOUT))
    }

    pub(super) fn flush(&mut self) -> Vec<u8> {
        self.pending_since = None;
        std::mem::take(&mut self.pending)
    }
}

pub(super) fn kitty_ipc_with_pane(command: &[u8], pane_id: usize) -> Option<Vec<u8>> {
    rewrite_kitty_ipc(
        command,
        Some(format!(
            "rm{pane_id}-{}",
            hex_encode(kitty_ipc_id(command).unwrap_or_default().as_bytes())
        )),
    )
}

pub(super) fn kitty_ipc_for_child(command: &[u8]) -> Option<(usize, Vec<u8>)> {
    let tagged = kitty_ipc_id(command)?;
    let rest = tagged.strip_prefix("rm")?;
    let (pane, original) = rest.split_once('-')?;
    let pane = pane.parse().ok()?;
    let original = String::from_utf8(hex_decode(original)?).ok()?;
    let rewritten = rewrite_kitty_ipc(command, (!original.is_empty()).then_some(original))?;
    Some((pane, rewritten))
}

pub(super) fn kitty_ipc_is_clipboard(command: &[u8]) -> bool {
    command.starts_with(KITTY_CLIPBOARD_PREFIX)
}

fn kitty_ipc_id(command: &[u8]) -> Option<&str> {
    let (prefix, separator) = if command.starts_with(KITTY_CLIPBOARD_PREFIX) {
        (KITTY_CLIPBOARD_PREFIX, ':')
    } else if command.starts_with(KITTY_FILE_PREFIX) {
        (KITTY_FILE_PREFIX, ';')
    } else {
        return None;
    };
    let body = command
        .strip_prefix(prefix)?
        .strip_suffix(STRING_TERMINATOR)?;
    let metadata = if command.starts_with(KITTY_CLIPBOARD_PREFIX) {
        body.split(|byte| *byte == b';').next()?
    } else {
        body
    };
    std::str::from_utf8(metadata)
        .ok()?
        .split(separator)
        .find_map(|field| field.strip_prefix("id="))
}

fn rewrite_kitty_ipc(command: &[u8], replacement_id: Option<String>) -> Option<Vec<u8>> {
    let (prefix, separator) = if command.starts_with(KITTY_CLIPBOARD_PREFIX) {
        (KITTY_CLIPBOARD_PREFIX, ':')
    } else if command.starts_with(KITTY_FILE_PREFIX) {
        (KITTY_FILE_PREFIX, ';')
    } else {
        return None;
    };
    let body = command
        .strip_prefix(prefix)?
        .strip_suffix(STRING_TERMINATOR)?;
    let metadata_end = if command.starts_with(KITTY_CLIPBOARD_PREFIX) {
        body.iter()
            .position(|byte| *byte == b';')
            .unwrap_or(body.len())
    } else {
        body.len()
    };
    let metadata = std::str::from_utf8(&body[..metadata_end]).ok()?;
    let mut replaced = false;
    let mut fields = Vec::new();
    for field in metadata.split(separator) {
        if field.starts_with("id=") {
            if let Some(id) = replacement_id.as_ref() {
                fields.push(format!("id={id}"));
            }
            replaced = true;
        } else {
            fields.push(field.to_owned());
        }
    }
    if !replaced && let Some(id) = replacement_id {
        fields.push(format!("id={id}"));
    }
    let mut result = prefix.to_vec();
    result.extend_from_slice(fields.join(&separator.to_string()).as_bytes());
    result.extend_from_slice(&body[metadata_end..]);
    result.extend_from_slice(STRING_TERMINATOR);
    Some(result)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum KittyDndEvent {
    Terminal(Vec<u8>),
    Command(Vec<u8>),
}

#[derive(Default)]
pub(super) struct KittyDndOutput {
    pub(super) events: Vec<KittyDndEvent>,
}

impl KittyDndOutput {
    fn push_terminal(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(KittyDndEvent::Terminal(previous)) = self.events.last_mut() {
            previous.extend(bytes);
        } else {
            self.events.push(KittyDndEvent::Terminal(bytes));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum KittyDndRegistration {
    Drag(bool),
    Drop(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct KittyDndCommand<'a> {
    pub(super) kind: Option<char>,
    pub(super) client_id: Option<u32>,
    pub(super) operation: Option<i32>,
    pub(super) x: Option<i32>,
    pub(super) y: Option<i32>,
    pub(super) more: bool,
    pub(super) payload: &'a [u8],
}

impl KittyDndParser {
    pub(super) fn process(&mut self, bytes: &[u8]) -> KittyDndOutput {
        self.pending.extend_from_slice(bytes);
        let mut output = KittyDndOutput::default();

        loop {
            let Some(start) = find_subslice(&self.pending, KITTY_DND_PREFIX) else {
                let retained = partial_prefix_length(&self.pending, KITTY_DND_PREFIX);
                let visible = self.pending.len().saturating_sub(retained);
                output.push_terminal(self.pending.drain(..visible).collect());
                break;
            };
            output.push_terminal(self.pending.drain(..start).collect());
            let Some(end) =
                find_subslice(&self.pending[KITTY_DND_PREFIX.len()..], STRING_TERMINATOR)
            else {
                if self.pending.len() > MAX_KITTY_DND_SEQUENCE_BYTES {
                    output.push_terminal(vec![self.pending.remove(0)]);
                    continue;
                }
                break;
            };
            let length = KITTY_DND_PREFIX.len() + end + STRING_TERMINATOR.len();
            output.events.push(KittyDndEvent::Command(
                self.pending.drain(..length).collect(),
            ));
        }

        if self.pending.is_empty() || self.pending.starts_with(KITTY_DND_PREFIX) {
            self.pending_since = None;
        } else if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }

        output
    }

    pub(super) fn flush_if_expired(&mut self) -> Vec<u8> {
        if self
            .pending_since
            .is_some_and(|since| since.elapsed() >= ESCAPE_SEQUENCE_TIMEOUT)
        {
            self.flush()
        } else {
            Vec::new()
        }
    }

    pub(super) fn flush_deadline(&self) -> Option<Instant> {
        self.pending_since
            .and_then(|since| since.checked_add(ESCAPE_SEQUENCE_TIMEOUT))
    }

    pub(super) fn flush(&mut self) -> Vec<u8> {
        self.pending_since = None;
        std::mem::take(&mut self.pending)
    }
}

pub(super) fn kitty_dnd_with_id(command: &[u8], id: u32) -> Option<Vec<u8>> {
    rewrite_kitty_dnd(
        command,
        |key, value| (key != "i").then(|| format!("{key}={value}")),
        Some(("i", id.to_string())),
    )
}

pub(super) fn kitty_dnd_id(command: &[u8]) -> Option<u32> {
    let (metadata, _) = kitty_dnd_parts(command)?;
    metadata.split(':').find_map(|field| {
        let (key, value) = field.split_once('=')?;
        (key == "i").then(|| value.parse().ok()).flatten()
    })
}

pub(super) fn kitty_dnd_command(command: &[u8]) -> Option<KittyDndCommand<'_>> {
    let (metadata, payload) = kitty_dnd_parts(command)?;
    Some(KittyDndCommand {
        kind: metadata_value(metadata, "t").and_then(|value| value.chars().next()),
        client_id: metadata_value(metadata, "i").and_then(|value| value.parse().ok()),
        operation: metadata_value(metadata, "o").and_then(|value| value.parse().ok()),
        x: metadata_value(metadata, "x").and_then(|value| value.parse().ok()),
        y: metadata_value(metadata, "y").and_then(|value| value.parse().ok()),
        more: metadata_value(metadata, "m") == Some("1"),
        payload: payload.unwrap_or_default(),
    })
}

pub(super) fn kitty_dnd_data_response(index: i32, data: &[u8]) -> Option<Vec<u8>> {
    (index > 0).then_some(())?;
    let mut response = Vec::with_capacity(data.len() + 64);
    for chunk in data.chunks(4096) {
        response.extend_from_slice(format!("\x1b]72;t=r:x={index}:m=1;").as_bytes());
        response.extend_from_slice(chunk);
        response.extend_from_slice(STRING_TERMINATOR);
    }
    response.extend_from_slice(format!("\x1b]72;t=r:x={index}:m=0;").as_bytes());
    response.extend_from_slice(STRING_TERMINATOR);
    Some(response)
}

pub(super) fn kitty_dnd_drag_start_position(command: &[u8]) -> Option<(i32, i32)> {
    let command = kitty_dnd_command(command)?;
    (command.kind == Some('o')).then_some(())?;
    let x = command.x?;
    let y = command.y?;
    (x >= 0 && y >= 0).then_some((x, y))
}

pub(super) fn kitty_dnd_registration(command: &[u8]) -> Option<KittyDndRegistration> {
    let (metadata, _) = kitty_dnd_parts(command)?;
    let kind = metadata_value(metadata, "t")?;
    match kind {
        "a" => Some(KittyDndRegistration::Drop(true)),
        "A" => Some(KittyDndRegistration::Drop(false)),
        "o" if metadata_value(metadata, "x") == Some("1") => Some(KittyDndRegistration::Drag(true)),
        "o" if metadata_value(metadata, "x") == Some("2") => {
            Some(KittyDndRegistration::Drag(false))
        }
        _ => None,
    }
}

pub(super) fn kitty_dnd_for_child(
    command: &[u8],
    origin: (i32, i32),
    size: (u16, u16),
    cell_pixels: (u16, u16),
) -> Option<Vec<u8>> {
    let (metadata, _) = kitty_dnd_parts(command)?;
    let kind = metadata_value(metadata, "t");
    let location = matches!(kind, Some("m" | "M" | "o"));
    let x = metadata_value(metadata, "x").and_then(|value| value.parse::<i32>().ok());
    let y = metadata_value(metadata, "y").and_then(|value| value.parse::<i32>().ok());
    let inside = location
        && x.zip(y).is_some_and(|(x, y)| {
            x >= origin.0
                && y >= origin.1
                && x < origin.0 + i32::from(size.0)
                && y < origin.1 + i32::from(size.1)
        });

    rewrite_kitty_dnd(
        command,
        |key, value| {
            if key == "i" {
                return None;
            }
            let value = if location && matches!(key, "x" | "y") {
                let coordinate = value.parse::<i32>().ok()?;
                if coordinate < 0 || !inside {
                    "-1".to_owned()
                } else if key == "x" {
                    (coordinate - origin.0).to_string()
                } else {
                    (coordinate - origin.1).to_string()
                }
            } else if location && inside && matches!(key, "X" | "Y") {
                let coordinate = value.parse::<i64>().ok()?;
                let offset = if key == "X" {
                    i64::from(origin.0) * i64::from(cell_pixels.0)
                } else {
                    i64::from(origin.1) * i64::from(cell_pixels.1)
                };
                coordinate.saturating_sub(offset).to_string()
            } else {
                value.to_owned()
            };
            Some(format!("{key}={value}"))
        },
        None,
    )
}

fn kitty_dnd_parts(command: &[u8]) -> Option<(&str, Option<&[u8]>)> {
    let body = command
        .strip_prefix(KITTY_DND_PREFIX)?
        .strip_suffix(STRING_TERMINATOR)?;
    let (metadata, payload) = body
        .iter()
        .position(|byte| *byte == b';')
        .map_or((body, None), |position| {
            (&body[..position], Some(&body[position + 1..]))
        });
    Some((std::str::from_utf8(metadata).ok()?, payload))
}

fn metadata_value<'a>(metadata: &'a str, wanted: &str) -> Option<&'a str> {
    metadata.split(':').find_map(|field| {
        let (key, value) = field.split_once('=')?;
        (key == wanted).then_some(value)
    })
}

fn rewrite_kitty_dnd(
    command: &[u8],
    mut rewrite: impl FnMut(&str, &str) -> Option<String>,
    additional: Option<(&str, String)>,
) -> Option<Vec<u8>> {
    let (metadata, payload) = kitty_dnd_parts(command)?;
    let mut fields = Vec::new();
    for field in metadata.split(':') {
        let (key, value) = field.split_once('=')?;
        if let Some(field) = rewrite(key, value) {
            fields.push(field);
        }
    }
    if let Some((key, value)) = additional {
        fields.push(format!("{key}={value}"));
    }
    let mut result = Vec::with_capacity(command.len() + 16);
    result.extend_from_slice(KITTY_DND_PREFIX);
    result.extend_from_slice(fields.join(":").as_bytes());
    if let Some(payload) = payload {
        result.push(b';');
        result.extend_from_slice(payload);
    }
    result.extend_from_slice(STRING_TERMINATOR);
    Some(result)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn partial_prefix_length(bytes: &[u8], prefix: &[u8]) -> usize {
    (1..prefix.len())
        .rev()
        .find(|length| bytes.ends_with(&prefix[..*length]))
        .unwrap_or(0)
}

#[derive(Default)]
pub(super) struct KittyGraphicsParser {
    state: KittyGraphicsState,
    utf8_continuations: u8,
}

#[derive(Default)]
enum KittyGraphicsState {
    #[default]
    Ground,
    Escape,
    ApcPrefix {
        command: Vec<u8>,
        c1: bool,
    },
    Graphics {
        command: Vec<u8>,
        escape: bool,
        c1: bool,
    },
    Discard {
        escape: bool,
        c1: bool,
    },
    PassthroughApc {
        escape: bool,
        c1: bool,
    },
}

#[derive(Default)]
pub(super) struct KittyGraphicsOutput {
    pub(super) commands: Vec<Vec<u8>>,
    pub(super) terminal: Vec<u8>,
}

impl KittyGraphicsParser {
    pub(super) fn process(&mut self, bytes: &[u8]) -> KittyGraphicsOutput {
        let mut output = KittyGraphicsOutput {
            commands: Vec::new(),
            terminal: Vec::with_capacity(bytes.len().min(4096)),
        };
        for &byte in bytes {
            if matches!(self.state, KittyGraphicsState::Ground) {
                if self.utf8_continuations > 0 {
                    if byte & 0xc0 == 0x80 {
                        output.terminal.push(byte);
                        self.utf8_continuations -= 1;
                        continue;
                    }
                    self.utf8_continuations = 0;
                }
                self.utf8_continuations = match byte {
                    0xc2..=0xdf => 1,
                    0xe0..=0xef => 2,
                    0xf0..=0xf4 => 3,
                    _ => 0,
                };
                if self.utf8_continuations > 0 {
                    output.terminal.push(byte);
                    continue;
                }
            }
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                KittyGraphicsState::Ground => match byte {
                    0x1b => KittyGraphicsState::Escape,
                    0x9f => KittyGraphicsState::ApcPrefix {
                        command: vec![0x9f],
                        c1: true,
                    },
                    _ => {
                        output.terminal.push(byte);
                        KittyGraphicsState::Ground
                    }
                },
                KittyGraphicsState::Escape => match byte {
                    b'_' => KittyGraphicsState::ApcPrefix {
                        command: vec![0x1b, b'_'],
                        c1: false,
                    },
                    0x1b => {
                        output.terminal.push(0x1b);
                        KittyGraphicsState::Escape
                    }
                    0x9f => {
                        output.terminal.push(0x1b);
                        KittyGraphicsState::ApcPrefix {
                            command: vec![0x9f],
                            c1: true,
                        }
                    }
                    _ => {
                        output.terminal.extend_from_slice(&[0x1b, byte]);
                        KittyGraphicsState::Ground
                    }
                },
                KittyGraphicsState::ApcPrefix { mut command, c1 } => {
                    if byte == b'G' {
                        command.push(byte);
                        KittyGraphicsState::Graphics {
                            command,
                            escape: false,
                            c1,
                        }
                    } else {
                        output.terminal.extend_from_slice(&command);
                        output.terminal.push(byte);
                        if c1 && byte == 0x9c {
                            KittyGraphicsState::Ground
                        } else {
                            KittyGraphicsState::PassthroughApc {
                                escape: byte == 0x1b,
                                c1,
                            }
                        }
                    }
                }
                KittyGraphicsState::Graphics {
                    mut command,
                    escape,
                    c1,
                } => {
                    command.push(byte);
                    let terminated = (c1 && byte == 0x9c) || (!c1 && escape && byte == b'\\');
                    if terminated {
                        output.commands.push(command);
                        KittyGraphicsState::Ground
                    } else if command.len() >= MAX_KITTY_COMMAND_BYTES {
                        KittyGraphicsState::Discard {
                            escape: byte == 0x1b,
                            c1,
                        }
                    } else {
                        KittyGraphicsState::Graphics {
                            command,
                            escape: byte == 0x1b,
                            c1,
                        }
                    }
                }
                KittyGraphicsState::Discard { escape, c1 } => {
                    let terminated = (c1 && byte == 0x9c) || (!c1 && escape && byte == b'\\');
                    if terminated {
                        KittyGraphicsState::Ground
                    } else {
                        KittyGraphicsState::Discard {
                            escape: byte == 0x1b,
                            c1,
                        }
                    }
                }
                KittyGraphicsState::PassthroughApc { escape, c1 } => {
                    output.terminal.push(byte);
                    let terminated = (c1 && byte == 0x9c) || (!c1 && escape && byte == b'\\');
                    if terminated {
                        KittyGraphicsState::Ground
                    } else {
                        KittyGraphicsState::PassthroughApc {
                            escape: byte == 0x1b,
                            c1,
                        }
                    }
                }
            };
        }
        output
    }
}

#[derive(Default)]
pub(super) struct CursorStyleTracker {
    state: CursorSequenceState,
    pub(super) style: u8,
}

#[derive(Default)]
enum CursorSequenceState {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
}

impl CursorStyleTracker {
    pub(super) fn process(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                CursorSequenceState::Ground => match byte {
                    0x1b => CursorSequenceState::Escape,
                    0x9b => CursorSequenceState::Csi(Vec::new()),
                    _ => CursorSequenceState::Ground,
                },
                CursorSequenceState::Escape => match byte {
                    b'[' => CursorSequenceState::Csi(Vec::new()),
                    0x1b => CursorSequenceState::Escape,
                    _ => CursorSequenceState::Ground,
                },
                CursorSequenceState::Csi(mut parameters) => {
                    if byte == 0x1b {
                        CursorSequenceState::Escape
                    } else if (0x40..=0x7e).contains(&byte) {
                        if byte == b'q' {
                            self.apply_decscusr(&parameters);
                        }
                        CursorSequenceState::Ground
                    } else if parameters.len() < 16 {
                        parameters.push(byte);
                        CursorSequenceState::Csi(parameters)
                    } else {
                        CursorSequenceState::Ground
                    }
                }
            };
        }
    }

    fn apply_decscusr(&mut self, parameters: &[u8]) {
        let Some(digits) = parameters.strip_suffix(b" ") else {
            return;
        };
        if !digits.iter().all(u8::is_ascii_digit) {
            return;
        }
        let style = digits.iter().fold(0_u16, |value, digit| {
            value
                .saturating_mul(10)
                .saturating_add(u16::from(digit - b'0'))
        });
        if style <= 6 {
            self.style = style as u8;
        }
    }
}

#[derive(Default)]
struct KeyboardMode {
    flags: u8,
    stack: VecDeque<u8>,
}

/// Virtualizes input-related terminal modes independently for every PTY.
#[derive(Default)]
pub(super) struct InputModeTracker {
    state: InputModeSequenceState,
    main: KeyboardMode,
    alternate: KeyboardMode,
    alternate_screen: bool,
    focus_reporting: bool,
    rich_clipboard_paste: bool,
}

#[derive(Default)]
enum InputModeSequenceState {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
}

impl InputModeTracker {
    pub(super) fn alternate_screen(&self) -> bool {
        self.alternate_screen
    }
    pub(super) fn keyboard_flags(&self) -> u8 {
        if self.alternate_screen {
            self.alternate.flags
        } else {
            self.main.flags
        }
    }

    pub(super) fn focus_reporting(&self) -> bool {
        self.focus_reporting
    }

    pub(super) fn rich_clipboard_paste(&self) -> bool {
        self.rich_clipboard_paste
    }

    pub(super) fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut responses = Vec::new();
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                InputModeSequenceState::Ground => match byte {
                    0x1b => InputModeSequenceState::Escape,
                    0x9b => InputModeSequenceState::Csi(Vec::new()),
                    _ => InputModeSequenceState::Ground,
                },
                InputModeSequenceState::Escape => match byte {
                    b'[' => InputModeSequenceState::Csi(Vec::new()),
                    0x1b => InputModeSequenceState::Escape,
                    _ => InputModeSequenceState::Ground,
                },
                InputModeSequenceState::Csi(mut parameters) => {
                    if byte == 0x1b {
                        InputModeSequenceState::Escape
                    } else if (0x40..=0x7e).contains(&byte) {
                        self.apply_csi(&parameters, byte, &mut responses);
                        InputModeSequenceState::Ground
                    } else if parameters.len() < 128 {
                        parameters.push(byte);
                        InputModeSequenceState::Csi(parameters)
                    } else {
                        InputModeSequenceState::Ground
                    }
                }
            };
        }
        responses
    }

    fn apply_csi(&mut self, parameters: &[u8], final_byte: u8, responses: &mut Vec<u8>) {
        if final_byte == b'p' && parameters == b"?5522$" {
            let status = if self.rich_clipboard_paste { 1 } else { 2 };
            let _ = write!(responses, "\x1b[?5522;{status}$y");
            return;
        }

        if final_byte == b'u' {
            match parameters.first().copied() {
                Some(b'?') if parameters == b"?" => {
                    let _ = write!(responses, "\x1b[?{}u", self.keyboard_flags());
                }
                Some(b'=') => {
                    let values = parse_csi_numbers(&parameters[1..]);
                    let requested =
                        values.first().copied().unwrap_or(0) as u8 & KITTY_KEYBOARD_FLAGS;
                    let mode = values.get(1).copied().unwrap_or(1);
                    let keyboard = self.keyboard_mode_mut();
                    match mode {
                        2 => keyboard.flags |= requested,
                        3 => keyboard.flags &= !requested,
                        _ => keyboard.flags = requested,
                    }
                }
                Some(b'>') => {
                    let requested = parse_csi_numbers(&parameters[1..])
                        .first()
                        .copied()
                        .unwrap_or(0) as u8
                        & KITTY_KEYBOARD_FLAGS;
                    let keyboard = self.keyboard_mode_mut();
                    if keyboard.stack.len() == MAX_MODE_STACK_DEPTH {
                        keyboard.stack.pop_front();
                    }
                    keyboard.stack.push_back(keyboard.flags);
                    keyboard.flags = requested;
                }
                Some(b'<') => {
                    let count = parse_csi_numbers(&parameters[1..])
                        .first()
                        .copied()
                        .unwrap_or(1)
                        .max(1);
                    let keyboard = self.keyboard_mode_mut();
                    for _ in 0..count {
                        keyboard.flags = keyboard.stack.pop_back().unwrap_or(0);
                    }
                }
                _ => {}
            }
            return;
        }

        if matches!(final_byte, b'h' | b'l') && parameters.starts_with(b"?") {
            let enabled = final_byte == b'h';
            for mode in parse_csi_numbers(&parameters[1..]) {
                match mode {
                    47 | 1047 | 1049 => self.alternate_screen = enabled,
                    1004 => self.focus_reporting = enabled,
                    5522 => self.rich_clipboard_paste = enabled,
                    _ => {}
                }
            }
        }
    }

    fn keyboard_mode_mut(&mut self) -> &mut KeyboardMode {
        if self.alternate_screen {
            &mut self.alternate
        } else {
            &mut self.main
        }
    }
}

#[derive(Clone)]
struct ColorSnapshot {
    foreground: (u8, u8, u8),
    background: (u8, u8, u8),
    cursor: Option<(u8, u8, u8)>,
    palette: [Option<(u8, u8, u8)>; 256],
}

impl Default for ColorSnapshot {
    fn default() -> Self {
        Self {
            foreground: MOCHA_TEXT,
            background: MOCHA_BASE,
            cursor: None,
            palette: [None; 256],
        }
    }
}

#[derive(Default)]
enum OscSequenceState {
    #[default]
    Ground,
    Escape,
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
}

#[derive(Default)]
pub(super) struct TerminalOscTracker {
    state: OscSequenceState,
    colors: ColorSnapshot,
    color_stack: Vec<ColorSnapshot>,
    pointer_main: Vec<String>,
    pointer_alternate: Vec<String>,
    alternate_screen: bool,
}

#[derive(Default)]
pub(super) struct TerminalOscOutput {
    pub(super) responses: Vec<u8>,
    pub(super) pointer_changed: bool,
}

impl TerminalOscTracker {
    pub(super) fn set_alternate_screen(&mut self, alternate: bool) {
        self.alternate_screen = alternate;
    }

    pub(super) fn foreground(&self) -> (u8, u8, u8) {
        self.colors.foreground
    }

    pub(super) fn background(&self) -> (u8, u8, u8) {
        self.colors.background
    }

    pub(super) fn cursor(&self) -> Option<(u8, u8, u8)> {
        self.colors.cursor
    }

    pub(super) fn pointer_shape(&self) -> &str {
        self.pointer_stack()
            .last()
            .map(String::as_str)
            .unwrap_or("default")
    }

    pub(super) fn process(&mut self, bytes: &[u8]) -> TerminalOscOutput {
        let mut output = TerminalOscOutput::default();
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                OscSequenceState::Ground => {
                    if byte == 0x1b {
                        OscSequenceState::Escape
                    } else {
                        OscSequenceState::Ground
                    }
                }
                OscSequenceState::Escape => match byte {
                    b']' => OscSequenceState::Osc(Vec::new()),
                    0x1b => OscSequenceState::Escape,
                    _ => OscSequenceState::Ground,
                },
                OscSequenceState::Osc(mut control) => match byte {
                    0x07 => {
                        self.apply_osc(&control, &mut output);
                        OscSequenceState::Ground
                    }
                    0x1b => OscSequenceState::OscEscape(control),
                    _ if control.len() < 64 * 1024 => {
                        control.push(byte);
                        OscSequenceState::Osc(control)
                    }
                    _ => OscSequenceState::Ground,
                },
                OscSequenceState::OscEscape(mut control) => {
                    if byte == b'\\' {
                        self.apply_osc(&control, &mut output);
                        OscSequenceState::Ground
                    } else {
                        if control.len() < 64 * 1024 {
                            control.extend_from_slice(&[0x1b, byte]);
                        }
                        OscSequenceState::Osc(control)
                    }
                }
            };
        }
        output
    }

    fn apply_osc(&mut self, control: &[u8], output: &mut TerminalOscOutput) {
        let Ok(control) = std::str::from_utf8(control) else {
            return;
        };
        let mut fields = control.split(';');
        let Some(code) = fields.next() else { return };
        match code {
            "4" => self.apply_palette(fields.collect(), output),
            "10" => self.apply_special_color("10", fields.next(), output),
            "11" => self.apply_special_color("11", fields.next(), output),
            "12" => self.apply_special_color("12", fields.next(), output),
            "104" => {
                let indices = fields
                    .filter_map(|value| value.parse::<usize>().ok())
                    .collect::<Vec<_>>();
                if indices.is_empty() {
                    self.colors.palette = [None; 256];
                } else {
                    for index in indices {
                        if index < 256 {
                            self.colors.palette[index] = None;
                        }
                    }
                }
            }
            "110" => self.colors.foreground = MOCHA_TEXT,
            "111" => self.colors.background = MOCHA_BASE,
            "112" => self.colors.cursor = None,
            "21" => self.apply_kitty_colors(fields, output),
            "30001" => {
                if self.color_stack.len() == MAX_MODE_STACK_DEPTH {
                    self.color_stack.remove(0);
                }
                self.color_stack.push(self.colors.clone());
            }
            "30101" => {
                if let Some(colors) = self.color_stack.pop() {
                    self.colors = colors;
                }
            }
            "22" => self.apply_pointer(fields.next().unwrap_or_default(), output),
            _ => {}
        }
    }

    fn apply_palette(&mut self, fields: Vec<&str>, output: &mut TerminalOscOutput) {
        for pair in fields.chunks_exact(2) {
            let Ok(index) = pair[0].parse::<usize>() else {
                continue;
            };
            if index >= 256 {
                continue;
            }
            if pair[1] == "?" {
                let color =
                    self.colors.palette[index].unwrap_or_else(|| default_palette(index as u8));
                append_osc_color_response(&mut output.responses, "4", Some(index), color);
            } else if let Some(color) = parse_color(pair[1]) {
                self.colors.palette[index] = Some(color);
            }
        }
    }

    fn apply_special_color(
        &mut self,
        code: &str,
        value: Option<&str>,
        output: &mut TerminalOscOutput,
    ) {
        let Some(value) = value else { return };
        let current = match code {
            "10" => Some(self.colors.foreground),
            "11" => Some(self.colors.background),
            _ => self.colors.cursor,
        };
        if value == "?" {
            if let Some(current) = current {
                append_osc_color_response(&mut output.responses, code, None, current);
            }
        } else if let Some(color) = parse_color(value) {
            match code {
                "10" => self.colors.foreground = color,
                "11" => self.colors.background = color,
                _ => self.colors.cursor = Some(color),
            }
        }
    }

    fn apply_kitty_colors<'a>(
        &mut self,
        fields: impl Iterator<Item = &'a str>,
        output: &mut TerminalOscOutput,
    ) {
        let mut response = Vec::new();
        for field in fields {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let current = match key {
                "foreground" => Some(self.colors.foreground),
                "background" => Some(self.colors.background),
                "cursor" => self.colors.cursor,
                _ => key.parse::<u8>().ok().map(|index| {
                    self.colors.palette[usize::from(index)]
                        .unwrap_or_else(|| default_palette(index))
                }),
            };
            if value == "?" {
                if let Some(color) = current {
                    response.push(format!("{key}={}", color_spec(color)));
                }
            } else if let Some(color) = parse_color(value) {
                match key {
                    "foreground" => self.colors.foreground = color,
                    "background" => self.colors.background = color,
                    "cursor" => self.colors.cursor = Some(color),
                    _ => {
                        if let Ok(index) = key.parse::<u8>() {
                            self.colors.palette[usize::from(index)] = Some(color);
                        }
                    }
                }
            }
        }
        if !response.is_empty() {
            let _ = write!(output.responses, "\x1b]21;{}\x1b\\", response.join(";"));
        }
    }

    fn apply_pointer(&mut self, value: &str, output: &mut TerminalOscOutput) {
        let stack = self.pointer_stack_mut();
        if let Some(query) = value.strip_prefix('?') {
            let reply = if query == "__current__" {
                stack.last().map(String::as_str).unwrap_or("0").to_owned()
            } else {
                query
                    .split(',')
                    .map(|shape| u8::from(valid_pointer_shape(shape)).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let _ = write!(output.responses, "\x1b]22;{reply}\x1b\\");
            return;
        }
        if let Some(shapes) = value.strip_prefix('>') {
            for shape in shapes.split(',').filter(|shape| valid_pointer_shape(shape)) {
                if stack.len() == MAX_MODE_STACK_DEPTH {
                    stack.remove(0);
                }
                stack.push(shape.to_owned());
            }
        } else if value.starts_with('<') {
            stack.pop();
        } else {
            stack.clear();
            let shape = value.strip_prefix('=').unwrap_or(value);
            if valid_pointer_shape(shape) {
                stack.push(shape.to_owned());
            }
        }
        output.pointer_changed = true;
    }

    fn pointer_stack(&self) -> &Vec<String> {
        if self.alternate_screen {
            &self.pointer_alternate
        } else {
            &self.pointer_main
        }
    }

    fn pointer_stack_mut(&mut self) -> &mut Vec<String> {
        if self.alternate_screen {
            &mut self.pointer_alternate
        } else {
            &mut self.pointer_main
        }
    }
}

fn parse_color(value: &str) -> Option<(u8, u8, u8)> {
    if let Some(hex) = value.strip_prefix('#').filter(|value| value.len() == 6) {
        return Some((
            u8::from_str_radix(&hex[..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..], 16).ok()?,
        ));
    }
    let rgb = value.strip_prefix("rgb:")?;
    let mut components = rgb.split('/');
    let component = |value: &str| -> Option<u8> {
        let parsed = u16::from_str_radix(value, 16).ok()?;
        let max = (1_u32 << (value.len() * 4).min(16)) - 1;
        Some(((u32::from(parsed) * 255 + max / 2) / max) as u8)
    };
    Some((
        component(components.next()?)?,
        component(components.next()?)?,
        component(components.next()?)?,
    ))
}

fn color_spec((red, green, blue): (u8, u8, u8)) -> String {
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        u16::from(red) * 257,
        u16::from(green) * 257,
        u16::from(blue) * 257
    )
}

fn append_osc_color_response(
    output: &mut Vec<u8>,
    code: &str,
    index: Option<usize>,
    color: (u8, u8, u8),
) {
    let spec = color_spec(color);
    if let Some(index) = index {
        let _ = write!(output, "\x1b]{code};{index};{spec}\x1b\\");
    } else {
        let _ = write!(output, "\x1b]{code};{spec}\x1b\\");
    }
}

fn default_palette(index: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (69, 71, 90),
        (243, 139, 168),
        (166, 227, 161),
        (249, 226, 175),
        (137, 180, 250),
        (245, 194, 231),
        (148, 226, 213),
        (166, 173, 200),
        (88, 91, 112),
        (243, 139, 168),
        (166, 227, 161),
        (249, 226, 175),
        (137, 180, 250),
        (245, 194, 231),
        (148, 226, 213),
        (205, 214, 244),
    ];
    match index {
        0..=15 => ANSI[usize::from(index)],
        16..=231 => {
            let value = index - 16;
            let component = |part: u8| if part == 0 { 0 } else { 55 + part * 40 };
            (
                component(value / 36),
                component((value / 6) % 6),
                component(value % 6),
            )
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn valid_pointer_shape(shape: &str) -> bool {
    matches!(
        shape,
        "alias"
            | "cell"
            | "copy"
            | "crosshair"
            | "default"
            | "e-resize"
            | "ew-resize"
            | "grab"
            | "grabbing"
            | "help"
            | "move"
            | "n-resize"
            | "ne-resize"
            | "nesw-resize"
            | "no-drop"
            | "not-allowed"
            | "ns-resize"
            | "nw-resize"
            | "nwse-resize"
            | "pointer"
            | "progress"
            | "s-resize"
            | "se-resize"
            | "sw-resize"
            | "text"
            | "vertical-text"
            | "w-resize"
            | "wait"
            | "zoom-in"
            | "zoom-out"
    )
}

fn parse_csi_numbers(parameters: &[u8]) -> Vec<u16> {
    parameters
        .split(|byte| *byte == b';')
        .map(|digits| {
            digits.iter().try_fold(0_u16, |value, digit| {
                digit.is_ascii_digit().then(|| {
                    value
                        .saturating_mul(10)
                        .saturating_add(u16::from(digit - b'0'))
                })
            })
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

pub(super) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(char::from(TABLE[((value >> 18) & 0x3f) as usize]));
        encoded.push(char::from(TABLE[((value >> 12) & 0x3f) as usize]));
        encoded.push(if chunk.len() > 1 {
            char::from(TABLE[((value >> 6) & 0x3f) as usize])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(TABLE[(value & 0x3f) as usize])
        } else {
            '='
        });
    }
    encoded
}

pub(super) fn kitty_notification(identifier: &str, title: &str, body: &str) -> Vec<u8> {
    let application = base64_encode(b"rustmux");
    let title = base64_encode(title.as_bytes());
    let body = base64_encode(body.as_bytes());
    format!(
        "\x1b]99;i={identifier}:d=0:f={application}:o=always;\x1b\\\
         \x1b]99;i={identifier}:d=0:e=1:p=title;{title}\x1b\\\
         \x1b]99;i={identifier}:d=0:e=1:p=body;{body}\x1b\\\
         \x1b]99;i={identifier};\x1b\\\x07"
    )
    .into_bytes()
}

pub(super) fn format_duration(duration: Duration) -> String {
    if duration >= Duration::from_secs(60) {
        let total_seconds = duration.as_secs();
        format!("{}m {}s", total_seconds / 60, total_seconds % 60)
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

pub(super) fn terminal_responses(
    output: &[u8],
    window: Winsize,
    terminal_identity: &str,
    cursor: (u16, u16),
    bracketed_paste: bool,
) -> Vec<u8> {
    const MODE_QUERIES: &[(&[u8], u16)] = &[
        (b"\x1b[?69$p", 69),
        (b"\x1b[?2004$p", 2004),
        (b"\x1b[?2026$p", 2026),
        (b"\x1b[?2027$p", 2027),
        (b"\x1b[?2031$p", 2031),
        (b"\x1b[?2048$p", 2048),
    ];
    let mut responses = Vec::new();
    for position in output
        .iter()
        .enumerate()
        .filter_map(|(position, byte)| (*byte == 0x1b).then_some(position))
    {
        let query = &output[position..];
        let mode = MODE_QUERIES
            .iter()
            .find_map(|(request, mode)| query.starts_with(request).then_some(*mode));
        if let Some(mode) = mode {
            let status = match mode {
                2004 if bracketed_paste => 1,
                2004 | 2026 => 2,
                _ => 0,
            };
            let _ = write!(responses, "\x1b[?{mode};{status}$y");
        } else if query.starts_with(b"\x1bP$qm\x1b\\") {
            responses.extend_from_slice(b"\x1bP1$r0m\x1b\\");
        } else if query.starts_with(b"\x1b[0c") || query.starts_with(b"\x1b[c") {
            responses.extend_from_slice(b"\x1b[?1;2c");
        } else if query.starts_with(b"\x1b[5n") {
            responses.extend_from_slice(b"\x1b[0n");
        } else if query.starts_with(b"\x1b[6n") {
            let _ = write!(responses, "\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1);
        } else if query.starts_with(b"\x1b[>0q") || query.starts_with(b"\x1b[>q") {
            let _ = write!(responses, "\x1bP>|{terminal_identity}\x1b\\");
        } else if window.ws_xpixel > 0 && window.ws_ypixel > 0 && query.starts_with(b"\x1b[14t") {
            let _ = write!(
                responses,
                "\x1b[4;{};{}t",
                window.ws_ypixel, window.ws_xpixel
            );
        } else if window.ws_xpixel > 0 && window.ws_ypixel > 0 && query.starts_with(b"\x1b[16t") {
            let cell_width = window.ws_xpixel / window.ws_col.max(1);
            let cell_height = window.ws_ypixel / window.ws_row.max(1);
            let _ = write!(responses, "\x1b[6;{cell_height};{cell_width}t");
        }
    }
    responses
}

pub(super) fn outer_terminal_identity() -> String {
    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();
    let term = env::var("TERM").unwrap_or_default();
    let fingerprint = format!("{term_program} {term}").to_ascii_lowercase();
    let name = if env::var_os("KITTY_WINDOW_ID").is_some() || fingerprint.contains("kitty") {
        "kitty"
    } else if fingerprint.contains("ghostty") {
        "ghostty"
    } else if fingerprint.contains("wezterm") {
        "wezterm"
    } else {
        "rustmux"
    };
    let version = env::var("TERM_PROGRAM_VERSION")
        .ok()
        .filter(|version| {
            !version.is_empty()
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    format!("{name} {version}")
}

pub(super) fn kitty_graphics_query_response(command: &[u8]) -> Option<Vec<u8>> {
    let control_start = if command.starts_with(b"\x1b_G") {
        3
    } else if command.starts_with(b"\x9fG") {
        2
    } else {
        return None;
    };
    let control_end = command[control_start..]
        .iter()
        .position(|&byte| byte == b';')?
        + control_start;
    let control = &command[control_start..control_end];
    if !control
        .split(|&byte| byte == b',')
        .any(|field| field == b"a=q")
    {
        return None;
    }

    let mut response = b"\x1b_G".to_vec();
    let mut first = true;
    for field in control.split(|&byte| byte == b',') {
        if field.starts_with(b"i=") || field.starts_with(b"I=") {
            if !first {
                response.push(b',');
            }
            response.extend_from_slice(field);
            first = false;
        }
    }
    response.extend_from_slice(b";OK\x1b\\");
    Some(response)
}

pub(super) fn kitty_graphics_uses_shared_memory(command: &[u8]) -> bool {
    let control_start = if command.starts_with(b"\x1b_G") {
        3
    } else if command.starts_with(b"\x9fG") {
        2
    } else {
        return false;
    };
    let control_end = command[control_start..]
        .iter()
        .position(|&byte| byte == b';')
        .map_or(command.len(), |end| control_start + end);
    command[control_start..control_end]
        .split(|&byte| byte == b',')
        .any(|field| field == b"t=s")
}
