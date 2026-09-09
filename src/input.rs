use std::time::Instant;

use super::{ENCODED_PREFIXES, ESCAPE_SEQUENCE_TIMEOUT, PREFIX};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MouseAction {
    ScrollUp,
    ScrollDown,
    SelectStart(MousePosition),
    SelectExtend(MousePosition),
    SelectEnd(MousePosition),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MousePosition {
    pub(super) column: u16,
    pub(super) row: u16,
}

#[derive(Default)]
pub(super) struct InputDecoder {
    pending: Vec<u8>,
    pending_since: Option<Instant>,
}

impl InputDecoder {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let decoded = self.decode_complete();
        if self.pending.is_empty() {
            self.pending_since = None;
        } else if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
        decoded
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

    pub(super) fn flush(&mut self) -> Vec<u8> {
        self.pending_since = None;
        std::mem::take(&mut self.pending)
    }

    fn decode_complete(&mut self) -> Vec<u8> {
        let mut decoded = Vec::with_capacity(self.pending.len());
        let mut consumed = 0;
        loop {
            let pending = &self.pending[consumed..];
            if pending.is_empty() {
                break;
            }
            if let Some(sequence) = ENCODED_PREFIXES
                .iter()
                .find(|sequence| pending.starts_with(sequence))
            {
                decoded.push(PREFIX);
                consumed += sequence.len();
                continue;
            }
            if ENCODED_PREFIXES
                .iter()
                .any(|sequence| sequence.starts_with(pending))
            {
                break;
            }
            if pending == [0x1b]
                || (pending.starts_with(b"\x1b[")
                    && !pending[2..].iter().any(|byte| (0x40..=0x7e).contains(byte)))
            {
                break;
            }
            decoded.push(pending[0]);
            consumed += 1;
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        decoded
    }
}

pub(super) struct DecodedKey {
    pub(super) name: String,
    pub(super) raw: Vec<u8>,
}

pub(super) fn decode_key(bytes: &[u8]) -> (DecodedKey, usize) {
    let byte = bytes[0];
    let single = |name: &str| DecodedKey {
        name: name.to_owned(),
        raw: vec![byte],
    };
    match byte {
        b'\r' | b'\n' => (single("enter"), 1),
        b'\t' => (single("tab"), 1),
        0x7f => (single("backspace"), 1),
        0x01..=0x1a => (single(&format!("ctrl {}", char::from(b'a' + byte - 1))), 1),
        0x1b if bytes.get(1) == Some(&b'[') => {
            let Some(final_offset) = bytes[2..]
                .iter()
                .position(|value| (0x40..=0x7e).contains(value))
            else {
                return (single("esc"), 1);
            };
            let consumed = final_offset + 3;
            let raw = bytes[..consumed].to_vec();
            let name = decode_csi_key_name(&raw).unwrap_or_else(|| "unbound-csi".to_owned());
            (DecodedKey { name, raw }, consumed)
        }
        0x1b if bytes.get(1).is_some_and(u8::is_ascii_graphic) => {
            let raw = bytes[..2].to_vec();
            let name = format!("alt {}", char::from(bytes[1]));
            (DecodedKey { name, raw }, 2)
        }
        0x1b => (single("esc"), 1),
        0x20..=0x7e => (single(&char::from(byte).to_string()), 1),
        _ => {
            let width = std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| text.chars().next())
                .map(char::len_utf8)
                .unwrap_or(1);
            let raw = bytes[..width.min(bytes.len())].to_vec();
            let name = String::from_utf8_lossy(&raw).into_owned();
            (DecodedKey { name, raw }, width.min(bytes.len()))
        }
    }
}

fn decode_csi_key_name(sequence: &[u8]) -> Option<String> {
    let final_byte = *sequence.last()?;
    let parameters = std::str::from_utf8(&sequence[2..sequence.len() - 1]).ok()?;
    match (parameters, final_byte) {
        ("", b'A') => Some("up".to_owned()),
        ("", b'B') => Some("down".to_owned()),
        ("", b'C') => Some("right".to_owned()),
        ("", b'D') => Some("left".to_owned()),
        ("5", b'~') => Some("pageup".to_owned()),
        ("6", b'~') => Some("pagedown".to_owned()),
        (_, b'u') => decode_kitty_key(parameters),
        (_, b'~') if parameters.starts_with("27;") => decode_xterm_modified_key(parameters),
        _ => None,
    }
}

fn decode_kitty_key(parameters: &str) -> Option<String> {
    let mut fields = parameters.split(';');
    let codepoint = fields.next()?.split(':').next()?.parse::<u32>().ok()?;
    let modifiers = fields
        .next()
        .and_then(|value| value.split(':').next())
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    let character = char::from_u32(codepoint)?;
    if modifiers & 4 != 0 && character.is_ascii_alphabetic() {
        Some(format!("ctrl {}", character.to_ascii_lowercase()))
    } else if modifiers & 2 != 0 {
        Some(format!("alt {character}"))
    } else {
        Some(character.to_string())
    }
}

fn decode_xterm_modified_key(parameters: &str) -> Option<String> {
    let mut fields = parameters.split(';');
    (fields.next()? == "27").then_some(())?;
    let modifiers = fields.next()?.parse::<u8>().ok()?.saturating_sub(1);
    let character = char::from_u32(fields.next()?.parse().ok()?)?;
    if modifiers & 4 != 0 && character.is_ascii_alphabetic() {
        Some(format!("ctrl {}", character.to_ascii_lowercase()))
    } else if modifiers & 2 != 0 {
        Some(format!("alt {character}"))
    } else {
        Some(character.to_string())
    }
}

pub(super) fn decode_sgr_mouse(bytes: &[u8]) -> Option<(MouseAction, usize)> {
    if !bytes.starts_with(b"\x1b[<") {
        return None;
    }
    let final_offset = bytes[3..]
        .iter()
        .position(|byte| *byte == b'M' || *byte == b'm')?;
    let final_index = final_offset + 3;
    let mut fields = bytes[3..final_index].split(|byte| *byte == b';');
    let parse_number = |digits: &[u8]| {
        digits.iter().try_fold(0_u16, |value, digit| {
            digit
                .is_ascii_digit()
                .then(|| value.saturating_mul(10) + u16::from(digit - b'0'))
        })
    };
    let button = parse_number(fields.next()?)?;
    let position = MousePosition {
        column: parse_number(fields.next()?)?,
        row: parse_number(fields.next()?)?,
    };
    if fields.next().is_some() {
        return None;
    }
    let released = bytes[final_index] == b'm';
    let action = match button {
        button if button & 64 != 0 && button & 3 == 0 => MouseAction::ScrollUp,
        button if button & 64 != 0 && button & 3 == 1 => MouseAction::ScrollDown,
        button if released && button & 3 == 0 => MouseAction::SelectEnd(position),
        button if button & 32 != 0 && button & 3 == 0 => MouseAction::SelectExtend(position),
        button if button & 32 == 0 && button & 3 == 0 => MouseAction::SelectStart(position),
        _ => MouseAction::Other,
    };
    Some((action, final_index + 1))
}
