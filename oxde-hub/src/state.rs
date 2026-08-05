use std::{
    net::IpAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use papaya::HashMap as ConcurrentHashMap;
use tokio::sync::{
    Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard, OwnedSemaphorePermit, Semaphore,
};

use crate::{
    agent_link::{AgentLink, AgentRegistry},
    auth::LoginAttempts,
    deployment_logs::LogRegistry,
    error::{AppError, AppResult},
};

/// Git fetches are CPU-, memory-, disk-, and network-intensive. Keeping this
/// small is especially important on the single-board computers `OxDe` targets.
const MAX_CONCURRENT_GIT_FETCHES: usize = 2;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

impl AppState {
    pub fn new(
        data_dir: PathBuf,
        limits: AppStateLimits,
        db: toasty::Db,
        agent_registry: AgentRegistry,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                data_dir,
                write_lock: AsyncMutex::new(()),
                git_fetches: Arc::new(Semaphore::new(MAX_CONCURRENT_GIT_FETCHES)),
                id_seq: AtomicU64::new(0),
                max_upload_bytes: limits.max_upload_bytes,
                max_uncompressed_bytes: limits.max_uncompressed_bytes,
                base_domain: limits.base_domain,
                git_fetch_timeout_secs: limits.git_fetch_timeout_secs,
                install_timeout_secs: limits.install_timeout_secs,
                build_timeout_secs: limits.build_timeout_secs,
                api_token_max_expiry_days: limits.api_token_max_expiry_days,
                enable_mcp: limits.enable_mcp,
                db,
                sessions: ConcurrentHashMap::new(),
                login_attempts: ConcurrentHashMap::new(),
                log_registry: LogRegistry::new(),
                agent_registry,
            }),
        }
    }

    /// An app's specific host's link - see `AgentRegistry::for_host`.
    pub fn agent_link_for(&self, host_id: i64) -> AgentLink {
        self.inner.agent_registry.for_host(host_id)
    }

    pub fn agent_registry(&self) -> &AgentRegistry {
        &self.inner.agent_registry
    }

    pub fn log_registry(&self) -> &LogRegistry {
        &self.inner.log_registry
    }

    pub fn db(&self) -> &toasty::Db {
        &self.inner.db
    }

    pub fn sessions(&self) -> &ConcurrentHashMap<String, crate::auth::Session> {
        &self.inner.sessions
    }

    pub fn login_attempts(&self) -> &ConcurrentHashMap<IpAddr, LoginAttempts> {
        &self.inner.login_attempts
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

    pub fn api_token_max_expiry_days(&self) -> i64 {
        self.inner.api_token_max_expiry_days
    }

    pub fn enable_mcp(&self) -> bool {
        self.inner.enable_mcp
    }

    pub fn apps_dir(&self) -> PathBuf {
        self.inner.data_dir.join("apps")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.inner.data_dir.join("tmp")
    }

    pub fn deployment_dir(&self, app_id: &str, deployment_id: &str) -> PathBuf {
        self.apps_dir()
            .join(app_id)
            .join("deployments")
            .join(deployment_id)
    }

    pub fn deployment_files_dir(&self, app_id: &str, deployment_id: &str) -> PathBuf {
        self.deployment_dir(app_id, deployment_id).join("files")
    }

    pub fn deployment_log_path(
        &self,
        app_id: &str,
        deployment_id: &str,
        kind: crate::deployment_logs::LogKind,
    ) -> PathBuf {
        self.deployment_dir(app_id, deployment_id)
            .join(kind.file_name())
    }

    /// Serializes mutating operations that touch both `files/` and the
    /// database so they can't race each other into an inconsistent state.
    pub async fn write_lock(&self) -> AsyncMutexGuard<'_, ()> {
        self.inner.write_lock.lock().await
    }

    /// Reserves one of the bounded git-fetch slots. The caller keeps the
    /// permit inside the blocking clone task, so timed-out fetches continue
    /// to count until their cooperative cancellation has actually finished.
    pub async fn acquire_git_fetch_permit(&self) -> AppResult<OwnedSemaphorePermit> {
        self.inner
            .git_fetches
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::Git("git fetch limiter closed".to_string()))
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

struct Inner {
    data_dir: PathBuf,
    write_lock: AsyncMutex<()>,
    git_fetches: Arc<Semaphore>,
    id_seq: AtomicU64,
    max_upload_bytes: u64,
    max_uncompressed_bytes: u64,
    base_domain: String,
    git_fetch_timeout_secs: u64,
    install_timeout_secs: u64,
    build_timeout_secs: u64,
    api_token_max_expiry_days: i64,
    enable_mcp: bool,
    db: toasty::Db,
    sessions: ConcurrentHashMap<String, crate::auth::Session>,
    login_attempts: ConcurrentHashMap<IpAddr, LoginAttempts>,
    log_registry: LogRegistry,
    agent_registry: AgentRegistry,
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
    pub api_token_max_expiry_days: i64,
    pub enable_mcp: bool,
}
