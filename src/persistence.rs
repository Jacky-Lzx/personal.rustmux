use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::session::{SessionSnapshot, save_session_snapshot};

// A cache belongs to the pane, so moving a pane preserves it and closing a
// pane releases it. Invalidation never walks or drops the cached history.
#[derive(Default)]
pub(super) struct HistoryCache {
    dirty: bool,
    captured: Option<Arc<HistoryCapture>>,
}

impl HistoryCache {
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn capture(
        &mut self,
        screen: &vt100::Screen,
        limit: usize,
        colored: bool,
    ) -> Arc<HistoryCapture> {
        if self.dirty
            || self
                .captured
                .as_ref()
                .is_none_or(|capture| capture.limit != limit || capture.colored != colored)
        {
            self.captured = Some(Arc::new(HistoryCapture {
                screen: Mutex::new(Some(screen.clone())),
                lines: OnceLock::new(),
                limit,
                colored,
            }));
            self.dirty = false;
        }
        self.captured.as_ref().unwrap().clone()
    }
}

pub(super) struct HistoryCapture {
    screen: Mutex<Option<vt100::Screen>>,
    lines: OnceLock<Vec<String>>,
    limit: usize,
    colored: bool,
}

impl HistoryCapture {
    // Called only by the save worker (or benchmarks/tests), never to compare
    // snapshots on the event loop. Release the frozen screen after formatting.
    fn lines(&self) -> &Vec<String> {
        self.lines.get_or_init(|| {
            let screen = self.screen.lock().unwrap().take().unwrap();
            crate::app::snapshot_owned_scrollback(screen, self.limit, self.colored)
        })
    }
}

pub(super) struct PreparedSnapshot {
    pub layout: SessionSnapshot,
    // None denotes the floating pane; other entries use stable live pane IDs.
    histories: Vec<(Option<usize>, Arc<HistoryCapture>)>,
}

impl PreparedSnapshot {
    pub fn new(layout: SessionSnapshot) -> Self {
        Self {
            layout,
            histories: Vec::new(),
        }
    }

    pub fn add_history(&mut self, pane: Option<usize>, history: Arc<HistoryCapture>) {
        self.histories.push((pane, history));
    }

    pub fn materialize(&self) -> SessionSnapshot {
        let mut snapshot = self.layout.clone();
        for (id, history) in &self.histories {
            let destination = match id {
                Some(id) => {
                    &mut snapshot
                        .tabs
                        .iter_mut()
                        .flat_map(|tab| &mut tab.panes)
                        .find(|pane| pane.id == *id)
                        .expect("captured pane exists")
                        .scrollback
                }
                None => {
                    &mut snapshot
                        .floating
                        .as_mut()
                        .expect("captured floating pane exists")
                        .scrollback
                }
            };
            *destination = Some(history.lines().clone());
        }
        snapshot
    }
}

impl PartialEq for PreparedSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.layout == other.layout
            && self.histories.len() == other.histories.len()
            && self.histories.iter().zip(&other.histories).all(
                |((id, history), (other_id, other_history))| {
                    id == other_id && Arc::ptr_eq(history, other_history)
                },
            )
    }
}

type SaveResult = Result<(), String>;
type SaveFn = Arc<dyn Fn(&str, &SessionSnapshot) -> SaveResult + Send + Sync>;

pub(super) struct SaveRequest {
    pub name: String,
    pub snapshot: Arc<PreparedSnapshot>,
    pub notify: bool,
    pub replies: Vec<UnixStream>,
}

pub(super) struct SaveCompletion {
    pub snapshot: Arc<PreparedSnapshot>,
    pub notify: bool,
    pub result: SaveResult,
}

struct ActiveSave {
    snapshot: Arc<PreparedSnapshot>,
    notify: bool,
    thread: JoinHandle<SaveResult>,
}

pub(super) struct SnapshotWriter {
    active: Option<ActiveSave>,
    pending: Option<SaveRequest>,
    save: SaveFn,
}

impl Default for SnapshotWriter {
    fn default() -> Self {
        Self {
            active: None,
            pending: None,
            save: Arc::new(|name, snapshot| {
                save_session_snapshot(name, snapshot).map_err(|error| error.to_string())
            }),
        }
    }
}

impl SnapshotWriter {
    pub fn latest_snapshot(&self) -> Option<&PreparedSnapshot> {
        self.pending
            .as_ref()
            .map(|job| job.snapshot.as_ref())
            .or_else(|| self.active.as_ref().map(|job| job.snapshot.as_ref()))
    }

    pub fn submit(&mut self, mut request: SaveRequest) -> SaveResult {
        if self.active.is_none() {
            self.start(request);
        } else {
            // Keep at most one pending snapshot and a bounded set of RPC waiters.
            if let Some(pending) = &self.pending
                && pending.replies.len() + request.replies.len() > 64
            {
                return Err("too many pending save requests; try again later".to_owned());
            }
            if let Some(pending) = self.pending.take() {
                request.notify |= pending.notify;
                request.replies.extend(pending.replies);
            }
            self.pending = Some(request);
        }
        Ok(())
    }

    fn start(&mut self, request: SaveRequest) {
        let snapshot = request.snapshot.clone();
        let notify = request.notify;
        let save = self.save.clone();
        // Only immutable snapshot data crosses the thread boundary. PTYs and
        // terminal parsers remain owned by the event loop.
        let thread = thread::spawn(move || {
            let result = save(&request.name, &request.snapshot.materialize());
            let response = crate::control::Response {
                ok: result.is_ok(),
                output: result.as_ref().err().cloned().unwrap_or_default(),
            };
            if let Ok(encoded) = toml::to_string(&response) {
                for mut stream in request.replies {
                    let _ = crate::control::write_frame(&mut stream, &encoded);
                }
            }
            result
        });
        self.active = Some(ActiveSave {
            snapshot,
            notify,
            thread,
        });
    }

    pub fn poll(&mut self) -> Option<SaveCompletion> {
        if !self.active.as_ref()?.thread.is_finished() {
            return None;
        }
        self.finish()
    }

    fn finish(&mut self) -> Option<SaveCompletion> {
        let active = self.active.take()?;
        let result = active
            .thread
            .join()
            .unwrap_or_else(|_| Err("session save worker panicked".to_owned()));
        if let Some(pending) = self.pending.take() {
            self.start(pending);
        }
        Some(SaveCompletion {
            snapshot: active.snapshot,
            notify: active.notify,
            result,
        })
    }

    pub fn flush(&mut self) -> Vec<SaveCompletion> {
        let mut completed = Vec::new();
        while let Some(result) = self.finish() {
            completed.push(result);
        }
        completed
    }
}

impl Drop for SnapshotWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    #[test]
    fn history_capture_is_lazy_reusable_and_immutable_after_output() {
        let mut terminal = vt100::Parser::new(3, 40, 20);
        terminal.process(b"old\r\nkeep one\r\nkeep two\r\nkeep three");
        let mut cache = HistoryCache::default();
        let first = cache.capture(terminal.screen(), 3, false);
        assert!(first.lines.get().is_none());
        // Browsing history is not a change to the saved primary buffer.
        terminal.screen_mut().set_scrollback(1);
        assert!(Arc::ptr_eq(
            &first,
            &cache.capture(terminal.screen(), 3, false)
        ));
        terminal.screen_mut().set_scrollback(0);
        terminal.process(b"\r\nnew output");
        cache.invalidate();
        let second = cache.capture(terminal.screen(), 3, false);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.lines(), &["keep one", "keep two", "keep three"]);
        assert_eq!(second.lines(), &["keep two", "keep three", "new output"]);
        assert!(first.screen.lock().unwrap().is_none());
        assert!(second.screen.lock().unwrap().is_none());
    }

    #[test]
    fn history_capture_respects_format_limit_and_primary_screen() {
        let mut terminal = vt100::Parser::new(3, 40, 20);
        terminal.process(b"old\r\n\x1b[31mred\x1b[0m\r\nlast");
        terminal.process(b"\x1b[?1049h\x1b[2Jtemporary TUI");
        let before = terminal.screen().state_formatted();
        let mut cache = HistoryCache::default();
        let plain = cache.capture(terminal.screen(), 3, false);
        let colored = cache.capture(terminal.screen(), 3, true);
        let limited = cache.capture(terminal.screen(), 1, true);
        assert_eq!(plain.lines(), &["old", "red", "last"]);
        let mut restored = vt100::Parser::new(3, 40, 0);
        restored.process(colored.lines()[1].as_bytes());
        assert_eq!(restored.screen().contents(), "red");
        assert_eq!(
            restored.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(limited.lines(), &["last"]);
        assert!(!Arc::ptr_eq(&plain, &colored));
        assert!(!Arc::ptr_eq(&colored, &limited));
        assert_eq!(terminal.screen().state_formatted(), before);
    }

    #[test]
    fn unchanged_panes_are_reused_without_formatting_during_comparison() {
        let terminal = vt100::Parser::new(3, 40, 20);
        let mut left = HistoryCache::default();
        let mut right = HistoryCache::default();
        let mut before = PreparedSnapshot::new(SessionSnapshot::new(Vec::new(), None, 1));
        before.add_history(Some(1), left.capture(terminal.screen(), 10, false));
        before.add_history(None, right.capture(terminal.screen(), 10, false));
        let mut after = PreparedSnapshot::new(before.layout.clone());
        after.add_history(Some(1), left.capture(terminal.screen(), 10, false));
        after.add_history(None, right.capture(terminal.screen(), 10, false));
        assert!(before == after);
        left.invalidate();
        after.histories[0].1 = left.capture(terminal.screen(), 10, false);
        assert!(before != after);
        assert!(Arc::ptr_eq(&before.histories[1].1, &after.histories[1].1));
        assert!(
            before
                .histories
                .iter()
                .chain(&after.histories)
                .all(|(_, h)| h.lines.get().is_none())
        );
        after.histories[0].1 = before.histories[0].1.clone();
        after.layout.active_pane = 2;
        assert!(before != after);
    }

    #[test]
    fn worker_materializes_frozen_history_and_reports_write_failure() {
        use crate::session::{ScrollbackFormat, SnapshotFloating};
        let mut terminal = vt100::Parser::new(3, 40, 20);
        terminal.process(b"captured output");
        let mut cache = HistoryCache::default();
        let history = cache.capture(terminal.screen(), 10, false);
        let mut snapshot = PreparedSnapshot::new(SessionSnapshot::new(
            Vec::new(),
            Some(SnapshotFloating {
                scrollback_format: ScrollbackFormat::Plain,
                scrollback: None,
                cwd: None,
                visible: true,
                return_to: None,
            }),
            1,
        ));
        snapshot.add_history(None, history.clone());
        let snapshot = Arc::new(snapshot);
        terminal.process(b" changed later");
        cache.invalidate();
        assert!(history.lines.get().is_none());
        let caller = thread::current().id();
        let mut writer = SnapshotWriter {
            active: None,
            pending: None,
            save: Arc::new(move |_, snapshot| {
                assert_ne!(thread::current().id(), caller);
                assert_eq!(
                    snapshot
                        .floating
                        .as_ref()
                        .unwrap()
                        .scrollback
                        .as_ref()
                        .unwrap(),
                    &["captured output"]
                );
                Err("disk full".to_owned())
            }),
        };
        for _ in 0..2 {
            writer
                .submit(SaveRequest {
                    name: "test".to_owned(),
                    snapshot: snapshot.clone(),
                    notify: false,
                    replies: Vec::new(),
                })
                .unwrap();
            assert_eq!(writer.flush()[0].result, Err("disk full".to_owned()));
        }
        assert!(history.lines.get().is_some());
        assert!(history.screen.lock().unwrap().is_none());
    }

    fn request(id: usize) -> SaveRequest {
        SaveRequest {
            name: "test".to_owned(),
            snapshot: Arc::new(PreparedSnapshot::new(SessionSnapshot::new(
                Vec::new(),
                None,
                id,
            ))),
            notify: false,
            replies: Vec::new(),
        }
    }

    #[test]
    fn slow_save_does_not_block_submission_and_pending_snapshots_are_coalesced() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let mut writer = SnapshotWriter {
            save: Arc::new(move |_, snapshot| {
                started_tx.send(snapshot.active_pane).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(())
            }),
            active: None,
            pending: None,
        };
        writer.submit(request(1)).unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert!(writer.poll().is_none());
        let (mut client, reply) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut pending = request(2);
        pending.notify = true;
        pending.replies.push(reply);
        writer.submit(pending).unwrap();
        writer.submit(request(3)).unwrap();
        assert_eq!(writer.latest_snapshot().unwrap().layout.active_pane, 3);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        let completed = writer.flush();
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].snapshot.layout.active_pane, 1);
        assert_eq!(completed[1].snapshot.layout.active_pane, 3);
        assert!(completed[1].notify);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
        let response: crate::control::Response =
            toml::from_str(&crate::control::read_frame(&mut client).unwrap()).unwrap();
        assert!(response.ok);
        assert!(writer.latest_snapshot().is_none());
    }

    #[test]
    fn failed_save_reports_the_error_and_a_later_save_can_succeed() {
        let mut writer = SnapshotWriter {
            save: Arc::new(|_, snapshot| {
                if snapshot.active_pane == 1 {
                    Err("disk full".to_owned())
                } else {
                    Ok(())
                }
            }),
            active: None,
            pending: None,
        };
        let (mut client, reply) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut failing = request(1);
        failing.replies.push(reply);
        writer.submit(failing).unwrap();
        assert_eq!(writer.flush()[0].result, Err("disk full".to_owned()));
        let response: crate::control::Response =
            toml::from_str(&crate::control::read_frame(&mut client).unwrap()).unwrap();
        assert!(!response.ok);
        assert_eq!(response.output, "disk full");
        writer.submit(request(2)).unwrap();
        assert_eq!(writer.flush()[0].result, Ok(()));
    }
}
