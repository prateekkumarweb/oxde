use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    data_dir: PathBuf,
    write_lock: Mutex<()>,
    id_seq: AtomicU64,
    max_upload_bytes: u64,
    max_uncompressed_bytes: u64,
}

impl AppState {
    pub fn new(data_dir: PathBuf, max_upload_bytes: u64, max_uncompressed_bytes: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                data_dir,
                write_lock: Mutex::new(()),
                id_seq: AtomicU64::new(0),
                max_upload_bytes,
                max_uncompressed_bytes,
            }),
        }
    }

    pub fn max_upload_bytes(&self) -> u64 {
        self.inner.max_upload_bytes
    }

    pub fn max_uncompressed_bytes(&self) -> u64 {
        self.inner.max_uncompressed_bytes
    }

    pub fn apps_dir(&self) -> PathBuf {
        self.inner.data_dir.join("apps")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.inner.data_dir.join("tmp")
    }

    /// Serializes activate/delete so they can't race each other into leaving
    /// `active` dangling.
    pub fn write_lock(&self) -> MutexGuard<'_, ()> {
        self.inner
            .write_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn next_seq(&self) -> u64 {
        self.inner.id_seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn unique_tmp_path(&self, prefix: &str) -> PathBuf {
        let ts = jiff::Timestamp::now().as_nanosecond();
        let seq = self.next_seq();
        self.tmp_dir().join(format!("{prefix}-{ts}-{seq}"))
    }
}
