use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bollard::Docker;

use crate::{deployment_logs::LogRegistry, reverse_proxy::ProxyClient};

/// How long a resolved container IP is trusted before `container_ip` is
/// asked again - bounds how long routing can stay wrong after a container
/// crashes and Podman's restart policy respawns it with a new IP.
const CONTAINER_IP_TTL: Duration = Duration::from_secs(10);

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
    base_domain: String,
    git_fetch_timeout_secs: u64,
    install_timeout_secs: u64,
    build_timeout_secs: u64,
    docker: Docker,
    proxy_client: ProxyClient,
    container_ips: Mutex<HashMap<String, (String, Instant)>>,
    db: toasty::Db,
    sessions: Mutex<HashMap<String, crate::auth::Session>>,
    log_registry: LogRegistry,
}

/// Scalar config `AppState::new` bundles a plain constructor's worth of
/// values into a struct rather than exceeding clippy's argument-count lint.
pub struct AppStateLimits {
    pub max_upload_bytes: u64,
    pub max_uncompressed_bytes: u64,
    pub base_domain: String,
    pub git_fetch_timeout_secs: u64,
    pub install_timeout_secs: u64,
    pub build_timeout_secs: u64,
}

impl AppState {
    pub fn new(
        data_dir: PathBuf,
        limits: AppStateLimits,
        docker: Docker,
        proxy_client: ProxyClient,
        db: toasty::Db,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                data_dir,
                write_lock: Mutex::new(()),
                id_seq: AtomicU64::new(0),
                max_upload_bytes: limits.max_upload_bytes,
                max_uncompressed_bytes: limits.max_uncompressed_bytes,
                base_domain: limits.base_domain,
                git_fetch_timeout_secs: limits.git_fetch_timeout_secs,
                install_timeout_secs: limits.install_timeout_secs,
                build_timeout_secs: limits.build_timeout_secs,
                docker,
                proxy_client,
                container_ips: Mutex::new(HashMap::new()),
                db,
                sessions: Mutex::new(HashMap::new()),
                log_registry: LogRegistry::new(),
            }),
        }
    }

    pub fn log_registry(&self) -> &LogRegistry {
        &self.inner.log_registry
    }

    pub fn db(&self) -> &toasty::Db {
        &self.inner.db
    }

    pub fn sessions(&self) -> &Mutex<HashMap<String, crate::auth::Session>> {
        &self.inner.sessions
    }

    pub fn docker(&self) -> &Docker {
        &self.inner.docker
    }

    pub fn proxy_client(&self) -> &ProxyClient {
        &self.inner.proxy_client
    }

    pub fn cached_container_ip(&self, container_name: &str) -> Option<String> {
        let cache = self
            .inner
            .container_ips
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.get(container_name).and_then(|(ip, cached_at)| {
            (cached_at.elapsed() < CONTAINER_IP_TTL).then(|| ip.clone())
        })
    }

    pub fn cache_container_ip(&self, container_name: &str, ip: String) {
        let mut cache = self
            .inner
            .container_ips
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.insert(container_name.to_string(), (ip, Instant::now()));
    }

    pub fn max_upload_bytes(&self) -> u64 {
        self.inner.max_upload_bytes
    }

    pub fn max_uncompressed_bytes(&self) -> u64 {
        self.inner.max_uncompressed_bytes
    }

    pub fn base_domain(&self) -> &str {
        &self.inner.base_domain
    }

    pub fn git_fetch_timeout_secs(&self) -> u64 {
        self.inner.git_fetch_timeout_secs
    }

    pub fn install_timeout_secs(&self) -> u64 {
        self.inner.install_timeout_secs
    }

    pub fn build_timeout_secs(&self) -> u64 {
        self.inner.build_timeout_secs
    }

    pub fn apps_dir(&self) -> PathBuf {
        self.inner.data_dir.join("apps")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.inner.data_dir.join("tmp")
    }

    pub fn deployment_dir(&self, app_name: &str, deployment_id: &str) -> PathBuf {
        self.apps_dir()
            .join(app_name)
            .join("deployments")
            .join(deployment_id)
    }

    pub fn deployment_files_dir(&self, app_name: &str, deployment_id: &str) -> PathBuf {
        self.deployment_dir(app_name, deployment_id).join("files")
    }

    pub fn deployment_log_path(
        &self,
        app_name: &str,
        deployment_id: &str,
        kind: crate::deployment_logs::LogKind,
    ) -> PathBuf {
        self.deployment_dir(app_name, deployment_id)
            .join(kind.file_name())
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
