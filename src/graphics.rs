//! Retained Kitty uploads and Unicode-placeholder placements for client reattachment.
use std::collections::BTreeMap;

// Bound retained protocol data independently of the PTY parser's command limit.
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;
const MAX_RETAINED_COMMANDS: usize = 16 * 1024;

#[derive(Default)]
pub(super) struct GraphicsCache {
    images: BTreeMap<u32, Image>,
    uploading: Option<u32>,
    bytes: usize,
    commands: usize,
}

#[derive(Default)]
struct Image {
    upload: Vec<Vec<u8>>,
    complete: bool,
    placements: BTreeMap<u32, Vec<u8>>,
}

impl GraphicsCache {
    pub(super) fn record(&mut self, command: &[u8]) {
        let Some(body) = command
            .strip_prefix(b"\x1b_G")
            .or_else(|| command.strip_prefix(b"\x9fG"))
        else {
            return;
        };
        let body = body
            .strip_suffix(b"\x1b\\")
            .or_else(|| body.strip_suffix(b"\x9c"))
            .unwrap_or(body);
        let control = body.split(|b| *b == b';').next().unwrap_or_default();
        let field = |key: u8| {
            control
                .split(|b| *b == b',')
                .find_map(|f| (f.len() >= 2 && f[0] == key && f[1] == b'=').then(|| &f[2..]))
        };
        let number =
            |key| field(key).and_then(|v| std::str::from_utf8(v).ok()?.parse::<u32>().ok());
        let action = field(b'a').unwrap_or(b"t");
        let id = number(b'i');
        let placement = number(b'p');
        if action == b"d" {
            let deletion = field(b'd').unwrap_or(b"a");
            match deletion {
                b"a" | b"A" => {
                    for image in self.images.values_mut() {
                        image.placements.clear();
                    }
                    if deletion == b"A" {
                        self.images.clear();
                        self.uploading = None;
                    }
                }
                b"i" | b"I" => {
                    if let Some(id) = id {
                        if let Some(image) = self.images.get_mut(&id) {
                            if let Some(p) = placement {
                                image.placements.remove(&p);
                            } else {
                                image.placements.clear();
                            }
                            if deletion == b"I" && image.placements.is_empty() {
                                self.images.remove(&id);
                            }
                        }
                        if !self.images.contains_key(&id) && self.uploading == Some(id) {
                            self.uploading = None;
                        }
                    }
                }
                _ => {}
            }
            self.recount();
            return;
        }
        if action == b"p" {
            // Physical placements depend on cursor/scroll state. Only virtual
            // placements can safely be restored alongside the saved text grid.
            if field(b'U') == Some(b"1")
                && let Some(image) = id.and_then(|id| self.images.get_mut(&id))
            {
                image
                    .placements
                    .insert(placement.unwrap_or(0), quiet(command));
                self.recount();
            }
        } else if action == b"t" {
            let continuing = self.uploading.take();
            let Some(id) = continuing.or(id).filter(|id| *id != 0) else {
                return;
            };
            if continuing.is_none() {
                self.images.remove(&id);
                // Persistent files (Snacks) and direct data are reusable. Shared
                // memory and temporary files are consumed by the outer terminal.
                if !matches!(field(b't'), None | Some(b"d") | Some(b"f")) {
                    self.recount();
                    return;
                }
                self.images.insert(id, Image::default());
                self.recount();
            }
            if let Some(image) = self.images.get_mut(&id) {
                let command = quiet(command);
                self.bytes += command.len();
                self.commands += 1;
                image.upload.push(command);
                image.complete = field(b'm') != Some(b"1");
                if !image.complete {
                    self.uploading = Some(id);
                }
            }
        }
        // Never retain an unbounded stream, including tiny placement commands.
        if self.bytes > MAX_RETAINED_BYTES || self.commands > MAX_RETAINED_COMMANDS {
            *self = Self::default();
        }
    }

    fn recount(&mut self) {
        self.bytes = 0;
        self.commands = 0;
        for command in self
            .images
            .values()
            .flat_map(|image| image.upload.iter().chain(image.placements.values()))
        {
            self.bytes += command.len();
            self.commands += 1;
        }
    }

    pub(super) fn replay(&self) -> Vec<Vec<u8>> {
        self.images
            .values()
            .filter(|image| image.complete)
            .chain(self.images.values().filter(|image| !image.complete))
            .flat_map(|image| {
                image
                    .upload
                    .iter()
                    .chain(image.placements.values())
                    .cloned()
            })
            .collect()
    }
}

fn quiet(command: &[u8]) -> Vec<u8> {
    let start = if command.starts_with(b"\x1b_G") { 3 } else { 2 };
    let end = command[start..]
        .iter()
        .position(|b| *b == b';')
        .map(|end| start + end)
        .unwrap_or_else(|| command.len() - if command.ends_with(b"\x1b\\") { 2 } else { 1 });
    let mut result = b"\x1b_Gq=2".to_vec();
    for field in command[start..end].split(|b| *b == b',') {
        if !field.is_empty() && !field.starts_with(b"q=") {
            result.push(b',');
            result.extend_from_slice(field);
        }
    }
    result.extend_from_slice(
        command[end..]
            .strip_suffix(b"\x9c")
            .unwrap_or(&command[end..]),
    );
    if command.ends_with(b"\x9c") {
        result.extend_from_slice(b"\x1b\\");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snacks_file_and_latest_virtual_placement_survive_reattach() {
        let mut cache = GraphicsCache::default();
        cache.record(b"\x1b_Gt=f,i=42,f=100,q=1;L3RtcC9pbWcucG5n\x1b\\");
        cache.record(b"\x1b_Ga=p,U=1,i=42,p=11,c=10,r=5;\x1b\\");
        cache.record(b"\x1b_Ga=p,U=1,i=42,p=11,c=20,r=8;\x1b\\");
        let replay = cache.replay();
        assert_eq!(replay.len(), 2);
        assert_eq!(
            replay[0],
            b"\x1b_Gq=2,t=f,i=42,f=100;L3RtcC9pbWcucG5n\x1b\\"
        );
        assert_eq!(replay[1], b"\x1b_Gq=2,a=p,U=1,i=42,p=11,c=20,r=8;\x1b\\");
        assert_eq!(cache.replay(), replay);
        cache.record(b"\x1b_Ga=d,d=i,i=42,p=11\x1b\\");
        assert_eq!(cache.replay(), vec![replay[0].clone()]);
        cache.record(b"\x1b_Ga=d,d=I,i=42\x1b\\");
        assert!(cache.replay().is_empty());
    }

    #[test]
    fn direct_chunks_resume_after_disconnect_without_losing_upload_prefix() {
        let mut cache = GraphicsCache::default();
        cache.record(b"\x1b_Gi=42,t=d,f=100,m=1;YWJj\x1b\\");
        let prefix = cache.replay();
        assert_eq!(prefix.len(), 1);
        cache.record(b"\x1b_Gm=0;ZA==\x1b\\");
        let replay = cache.replay();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0], prefix[0]);
        assert!(replay[1].ends_with(b"m=0;ZA==\x1b\\"));
        cache.record(b"\x1b_Ga=d,d=A;\x1b\\");
        assert!(cache.replay().is_empty());
    }

    #[test]
    fn consumed_transports_and_queries_are_not_replayed() {
        let mut cache = GraphicsCache::default();
        cache.record(b"\x1b_Ga=q,i=1;\x1b\\");
        cache.record(b"\x1b_Gt=s,i=2;L2ltZw==\x1b\\");
        cache.record(b"\x1b_Gt=t,i=3;L2ltZw==\x1b\\");
        assert!(cache.replay().is_empty());
        cache.record(b"\x9fGi=4;YWJj\x9c");
        assert_eq!(cache.replay(), vec![b"\x1b_Gq=2,i=4;YWJj\x1b\\".to_vec()]);
    }
}
