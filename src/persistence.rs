use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::session::{SessionSnapshot, save_session_snapshot};

type SaveResult = Result<(), String>;
type SaveFn = Arc<dyn Fn(&str, &SessionSnapshot) -> SaveResult + Send + Sync>;

pub(super) struct SaveRequest {
    pub name: String,
    pub snapshot: Arc<SessionSnapshot>,
    pub notify: bool,
    pub replies: Vec<UnixStream>,
}

pub(super) struct SaveCompletion {
    pub snapshot: Arc<SessionSnapshot>,
    pub notify: bool,
    pub result: SaveResult,
}

struct ActiveSave {
    snapshot: Arc<SessionSnapshot>,
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
    pub fn latest_snapshot(&self) -> Option<&SessionSnapshot> {
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
            let result = save(&request.name, &request.snapshot);
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

    fn request(id: usize) -> SaveRequest {
        SaveRequest {
            name: "test".to_owned(),
            snapshot: Arc::new(SessionSnapshot::new(Vec::new(), None, id)),
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
        assert_eq!(writer.latest_snapshot().unwrap().active_pane, 3);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        let completed = writer.flush();
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].snapshot.active_pane, 1);
        assert_eq!(completed[1].snapshot.active_pane, 3);
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
