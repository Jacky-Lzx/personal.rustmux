use std::env;
use std::ffi::OsString;
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
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

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
pub(super) struct KittyDndOutput {
    pub(super) terminal: Vec<u8>,
    pub(super) commands: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum KittyDndRegistration {
    Drag(bool),
    Drop(bool),
}

impl KittyDndParser {
    pub(super) fn process(&mut self, bytes: &[u8]) -> KittyDndOutput {
        self.pending.extend_from_slice(bytes);
        let mut output = KittyDndOutput::default();

        loop {
            let Some(start) = find_subslice(&self.pending, KITTY_DND_PREFIX) else {
                let retained = partial_prefix_length(&self.pending, KITTY_DND_PREFIX);
                let visible = self.pending.len().saturating_sub(retained);
                output.terminal.extend(self.pending.drain(..visible));
                break;
            };
            output.terminal.extend(self.pending.drain(..start));
            let Some(end) =
                find_subslice(&self.pending[KITTY_DND_PREFIX.len()..], STRING_TERMINATOR)
            else {
                if self.pending.len() > MAX_KITTY_DND_SEQUENCE_BYTES {
                    output.terminal.push(self.pending.remove(0));
                    continue;
                }
                break;
            };
            let length = KITTY_DND_PREFIX.len() + end + STRING_TERMINATOR.len();
            output.commands.push(self.pending.drain(..length).collect());
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
        } else if query.starts_with(b"\x1b[?u") {
            responses.extend_from_slice(b"\x1b[?0u");
        } else if query.starts_with(b"\x1b[5n") {
            responses.extend_from_slice(b"\x1b[0n");
        } else if query.starts_with(b"\x1b[6n") {
            let _ = write!(responses, "\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1);
        } else if query.starts_with(b"\x1b[>0q") || query.starts_with(b"\x1b[>q") {
            let _ = write!(responses, "\x1bP>|{terminal_identity}\x1b\\");
        } else if query.starts_with(b"\x1b]11;?\x1b\\") || query.starts_with(b"\x1b]11;?\x07") {
            responses.extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x1b\\");
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
