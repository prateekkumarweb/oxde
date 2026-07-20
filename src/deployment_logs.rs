use std::{
    collections::HashMap,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use ts_rs::TS;

use crate::error::AppResult;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LogKind {
    Clone,
    Install,
    Build,
    Run,
}

impl LogKind {
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Clone => "clone.log",
            Self::Install => "install.log",
            Self::Build => "build.log",
            Self::Run => "run.log",
        }
    }
}

const LOG_BROADCAST_CAPACITY: usize = 256;
/// Byte cap, not a line count - cheaper to check than scanning for lines.
const RUN_LOG_ROTATE_BYTES: u64 = 1024 * 1024;

struct ActivePump {
    kind: LogKind,
    tx: broadcast::Sender<Bytes>,
}

/// Deployments with an active log pump - lets the HTTP handler offer live
/// tail, and ensures only one pump writes to a deployment's log at a time.
#[derive(Clone)]
pub struct LogRegistry(Arc<Mutex<HashMap<String, ActivePump>>>);

impl LogRegistry {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, ActivePump>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Subscribes under the same lock as the presence check, so the caller
    /// never misses a pump's final bytes to a race with it finishing.
    pub fn active(&self, deployment_id: &str) -> Option<(LogKind, broadcast::Receiver<Bytes>)> {
        self.lock()
            .get(deployment_id)
            .map(|pump| (pump.kind, pump.tx.subscribe()))
    }

    /// `None` if a pump is already registered - callers treat that as "don't
    /// start a second one," not an error.
    pub fn try_register(
        &self,
        deployment_id: &str,
        kind: LogKind,
    ) -> Option<broadcast::Sender<Bytes>> {
        use std::collections::hash_map::Entry;
        match self.lock().entry(deployment_id.to_string()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(entry) => {
                let (tx, _rx) = broadcast::channel(LOG_BROADCAST_CAPACITY);
                entry.insert(ActivePump {
                    kind,
                    tx: tx.clone(),
                });
                Some(tx)
            }
        }
    }

    pub fn deregister(&self, deployment_id: &str) {
        self.lock().remove(deployment_id);
    }
}

impl Default for LogRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a pump writes/broadcasts for one deployment + phase.
#[derive(Clone)]
pub struct LogTarget {
    pub path: PathBuf,
    pub deployment_id: String,
    pub kind: LogKind,
    pub registry: LogRegistry,
}

/// Drains `source` into `target`'s file and broadcast channel until `source`
/// ends, then deregisters. A no-op if a pump is already registered for this
/// deployment. A write failure only drops persistence for that chunk (not
/// the live broadcast) - a watching client shouldn't lose output over a
/// transient disk error.
pub async fn run_pump(target: LogTarget, mut source: impl Stream<Item = AppResult<Bytes>> + Unpin) {
    let Some(tx) = target
        .registry
        .try_register(&target.deployment_id, target.kind)
    else {
        return;
    };

    let mut file = match open_for_kind(target.kind, &target.path) {
        Ok(file) => Some(file),
        Err(err) => {
            tracing::warn!(error = %err, path = %target.path.display(), "failed to open log file");
            None
        }
    };
    let mut bytes_written = file
        .as_ref()
        .and_then(|file| file.metadata().ok())
        .map_or(0, |metadata| metadata.len());

    while let Some(chunk) = source.next().await {
        let Ok(chunk) = chunk else { break };

        if file.is_some() && target.kind == LogKind::Run && bytes_written >= RUN_LOG_ROTATE_BYTES {
            file = rotate_and_reopen(&target.path)
                .inspect_err(|err| {
                    tracing::warn!(error = %err, path = %target.path.display(), "failed to rotate run.log");
                })
                .ok();
            bytes_written = 0;
        }

        if let Some(current) = &mut file {
            match current.write_all(&chunk) {
                Ok(()) => bytes_written += chunk.len() as u64,
                Err(err) => {
                    tracing::warn!(error = %err, path = %target.path.display(), "failed to persist log chunk");
                }
            }
        }

        let _ = tx.send(chunk); // Err just means no live subscribers right now.
    }

    target.registry.deregister(&target.deployment_id);
}

fn open_for_kind(kind: LogKind, path: &Path) -> AppResult<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true);
    if kind == LogKind::Run {
        options.append(true);
    } else {
        options.truncate(true);
    }
    Ok(options.open(path)?)
}

/// Renames `run.log` to `run.log.1` (clobbering any old `.1`) and reopens.
fn rotate_and_reopen(run_log_path: &Path) -> AppResult<std::fs::File> {
    let rotated = run_log_path.with_extension("log.1");
    std::fs::rename(run_log_path, &rotated)?;
    open_for_kind(LogKind::Run, run_log_path)
}

/// Live-tail byte stream. A lagged subscriber just skips ahead rather than
/// erroring - the backlog already covers everything up to subscribe time.
pub fn live_tail(rx: broadcast::Receiver<Bytes>) -> impl Stream<Item = AppResult<Bytes>> {
    futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(bytes) => return Some((Ok(bytes), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

/// Backlog for `kind`. `Run` concatenates `run.log.1` then `run.log`
/// (oldest first); a missing file just reads as empty.
pub fn read_backlog(deployment_dir: &Path, kind: LogKind) -> AppResult<Vec<u8>> {
    let mut buf = Vec::new();
    if kind == LogKind::Run {
        read_into(&deployment_dir.join("run.log.1"), &mut buf)?;
    }
    read_into(&deployment_dir.join(kind.file_name()), &mut buf)?;
    Ok(buf)
}

fn read_into(path: &Path, buf: &mut Vec<u8>) -> AppResult<()> {
    match std::fs::read(path) {
        Ok(bytes) => {
            buf.extend_from_slice(&bytes);
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Furthest-progressed phase with a file on disk - used when nothing's
/// actively pumping right now.
pub fn resolve_terminal_phase(deployment_dir: &Path) -> Option<LogKind> {
    [
        LogKind::Run,
        LogKind::Build,
        LogKind::Install,
        LogKind::Clone,
    ]
    .into_iter()
    .find(|kind| deployment_dir.join(kind.file_name()).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-deployment-logs-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn chunk_stream(chunks: Vec<Bytes>) -> impl Stream<Item = AppResult<Bytes>> + Unpin {
        futures_util::stream::iter(chunks.into_iter().map(Ok))
    }

    #[tokio::test]
    async fn pump_writes_chunks_and_deregisters_on_stream_end() {
        let dir = tempdir("basic");
        let registry = LogRegistry::new();
        let target = LogTarget {
            path: dir.join("install.log"),
            deployment_id: "dep1".to_string(),
            kind: LogKind::Install,
            registry: registry.clone(),
        };
        run_pump(
            target,
            chunk_stream(vec![
                Bytes::from_static(b"hello "),
                Bytes::from_static(b"world"),
            ]),
        )
        .await;

        assert_eq!(
            std::fs::read(dir.join("install.log")).unwrap(),
            b"hello world"
        );
        assert!(registry.active("dep1").is_none());
    }

    #[tokio::test]
    async fn second_pump_for_same_deployment_is_a_no_op() {
        let dir = tempdir("dup");
        let registry = LogRegistry::new();
        let tx = registry
            .try_register("dep1", LogKind::Install)
            .expect("first register");

        let target = LogTarget {
            path: dir.join("install.log"),
            deployment_id: "dep1".to_string(),
            kind: LogKind::Install,
            registry: registry.clone(),
        };
        run_pump(target, chunk_stream(vec![Bytes::from_static(b"ignored")])).await;

        // The second pump returned immediately without touching the file.
        assert!(std::fs::read(dir.join("install.log")).is_err());
        // The first (still-registered) pump is untouched.
        assert!(registry.active("dep1").is_some());
        drop(tx);
    }

    #[tokio::test]
    async fn run_log_rotates_at_cap() {
        let dir = tempdir("rotate");
        let registry = LogRegistry::new();
        let target = LogTarget {
            path: dir.join("run.log"),
            deployment_id: "dep1".to_string(),
            kind: LogKind::Run,
            registry,
        };

        let first = Bytes::from(vec![b'a'; RUN_LOG_ROTATE_BYTES as usize]);
        let second = Bytes::from_static(b"after-rotation");
        run_pump(target, chunk_stream(vec![first, second])).await;

        assert_eq!(
            std::fs::read(dir.join("run.log.1")).unwrap().len(),
            RUN_LOG_ROTATE_BYTES as usize
        );
        assert_eq!(
            std::fs::read(dir.join("run.log")).unwrap(),
            b"after-rotation"
        );
    }

    #[test]
    fn read_backlog_concatenates_rotated_run_logs_oldest_first() {
        let dir = tempdir("backlog");
        std::fs::write(dir.join("run.log.1"), b"old-").unwrap();
        std::fs::write(dir.join("run.log"), b"new").unwrap();

        assert_eq!(read_backlog(&dir, LogKind::Run).unwrap(), b"old-new");
    }

    #[test]
    fn read_backlog_missing_file_is_empty_not_error() {
        let dir = tempdir("missing");
        assert_eq!(
            read_backlog(&dir, LogKind::Install).unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn resolve_terminal_phase_prefers_furthest_progressed() {
        let dir = tempdir("phase");
        std::fs::write(dir.join("clone.log"), b"").unwrap();
        std::fs::write(dir.join("install.log"), b"").unwrap();
        assert_eq!(resolve_terminal_phase(&dir), Some(LogKind::Install));
    }

    #[test]
    fn resolve_terminal_phase_none_when_nothing_on_disk() {
        let dir = tempdir("phase-empty");
        assert_eq!(resolve_terminal_phase(&dir), None);
    }
}
