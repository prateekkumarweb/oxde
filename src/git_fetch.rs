use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs},
    num::NonZeroU32,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use gix::progress::{Id, MessageLevel, StepShared, Unit};
use tokio::sync::broadcast;

use crate::{
    deployment_logs::{LogRegistry, LogTarget},
    error::{AppError, AppResult},
};

/// Shallow (depth 1), single-branch clone + checkout of `repo_url` at
/// `branch` into `dest`. Returns the checked-out commit's SHA. `log_target`,
/// if given, receives clone progress as it happens.
pub fn clone_shallow(
    repo_url: &str,
    branch: &str,
    dest: &Path,
    log_target: Option<LogTarget>,
) -> AppResult<String> {
    ensure_host_is_public(repo_url)?;

    let should_interrupt = AtomicBool::new(false);
    let progress = CloneProgress::start(log_target);

    let mut prepare = gix::prepare_clone(repo_url, dest)
        .map_err(|err| AppError::Git(err.to_string()))?
        .with_ref_name(Some(branch))
        .map_err(|err| AppError::Git(err.to_string()))?
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(NonZeroU32::MIN));

    let (mut checkout, _) = prepare
        .fetch_then_checkout(progress.clone(), &should_interrupt)
        .map_err(|err| AppError::Git(err.to_string()))?;
    let (repo, _) = checkout
        .main_worktree(progress, &should_interrupt)
        .map_err(|err| AppError::Git(err.to_string()))?;

    let head_id = repo
        .head_id()
        .map_err(|err| AppError::Git(err.to_string()))?;
    Ok(head_id.to_string())
}

/// Rejects `repo_url` unless every address its host resolves to is
/// publicly routable - blocks git deploys from reaching internal services
/// or cloud metadata endpoints (e.g. `169.254.169.254`) via SSRF.
fn ensure_host_is_public(repo_url: &str) -> AppResult<()> {
    let url = gix::url::parse(repo_url).map_err(|err| AppError::Git(err.to_string()))?;
    let Some(host) = url.host() else {
        return Err(AppError::Git(format!(
            "repo URL {repo_url} has no host to validate"
        )));
    };
    let port = url.port_or_default().unwrap_or(0);

    let mut resolved_any = false;
    for addr in (host, port)
        .to_socket_addrs()
        .map_err(|err| AppError::Git(format!("could not resolve host {host}: {err}")))?
    {
        resolved_any = true;
        if !is_public_ip(addr.ip()) {
            return Err(AppError::Git(format!(
                "repo host {host} resolves to a non-public address"
            )));
        }
    }
    if !resolved_any {
        return Err(AppError::Git(format!(
            "repo host {host} did not resolve to any address"
        )));
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0 // "this network", 0.0.0.0/8
        || (octets[0] == 100 && (64..=127).contains(&octets[1])) // CGNAT, 100.64.0.0/10
        || (octets[0] == 198 && (18..=19).contains(&octets[1])) // benchmarking, 198.18.0.0/15
        || octets[0] >= 240) // reserved, 240.0.0.0/4
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    !(ip.is_loopback()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local())
}

/// Deregisters once every `CloneProgress` sharing this sink is dropped.
struct DeregisterOnDrop {
    registry: LogRegistry,
    deployment_id: String,
}

impl Drop for DeregisterOnDrop {
    fn drop(&mut self) {
        self.registry.deregister(&self.deployment_id);
    }
}

/// The shared log destination - every `CloneProgress` spawned from one root
/// writes to the same `clone.log`.
struct LogSink {
    file: Option<Mutex<std::fs::File>>,
    tx: Option<broadcast::Sender<Bytes>>,
    _dereg: Option<DeregisterOnDrop>,
}

/// Min gap between progress lines from one instance - gix calls `inc_by`
/// far more often than is useful to log.
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(500);

/// Bridges gix's progress reporting to `clone.log`. Each `add_child` gets
/// its own name/counter/throttle, sharing only the underlying sink, so
/// unrelated phases (objects, deltas, checkout, ...) don't share counts.
#[derive(Clone)]
struct CloneProgress {
    sink: Arc<LogSink>,
    name: String,
    max: Option<gix::progress::Step>,
    step: StepShared,
    last_emit: Arc<Mutex<Instant>>,
}

impl CloneProgress {
    fn start(log_target: Option<LogTarget>) -> Self {
        let Some(target) = log_target else {
            return Self::disabled();
        };
        let Some(tx) = target
            .registry
            .try_register(&target.deployment_id, target.kind)
        else {
            return Self::disabled();
        };

        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target.path)
            .inspect_err(|err| {
                tracing::warn!(error = %err, path = %target.path.display(), "failed to open clone.log");
            })
            .ok()
            .map(Mutex::new);

        Self::with_sink(Arc::new(LogSink {
            file,
            tx: Some(tx),
            _dereg: Some(DeregisterOnDrop {
                registry: target.registry,
                deployment_id: target.deployment_id,
            }),
        }))
    }

    fn disabled() -> Self {
        Self::with_sink(Arc::new(LogSink {
            file: None,
            tx: None,
            _dereg: None,
        }))
    }

    fn with_sink(sink: Arc<LogSink>) -> Self {
        Self {
            sink,
            name: String::new(),
            max: None,
            step: Arc::new(AtomicUsize::new(0)),
            last_emit: Arc::new(Mutex::new(
                Instant::now()
                    .checked_sub(PROGRESS_EMIT_INTERVAL)
                    .unwrap_or_else(Instant::now),
            )),
        }
    }

    fn spawn_child(&self, name: String) -> Self {
        Self {
            name,
            ..Self::with_sink(self.sink.clone())
        }
    }

    fn write_line(&self, message: &str) {
        if let Some(file) = &self.sink.file {
            let _ = writeln!(
                file.lock().unwrap_or_else(PoisonError::into_inner),
                "{message}"
            );
        }
        if let Some(tx) = &self.sink.tx {
            let _ = tx.send(Bytes::from(format!("{message}\n")));
        }
    }

    /// Skips unnamed progress (the root) - a bare number isn't useful.
    fn maybe_emit(&self, step: gix::progress::Step) {
        if self.name.is_empty() {
            return;
        }
        {
            let mut last = self
                .last_emit
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if last.elapsed() < PROGRESS_EMIT_INTERVAL {
                return;
            }
            *last = Instant::now();
        }
        let line = match self.max {
            Some(max) if max > 0 => {
                format!(
                    "{}: {step}/{max} ({}%)",
                    self.name,
                    step.saturating_mul(100) / max
                )
            }
            _ => format!("{}: {step}", self.name),
        };
        self.write_line(&line);
    }
}

impl gix::Count for CloneProgress {
    fn set(&self, step: gix::progress::Step) {
        self.step.store(step, Ordering::Relaxed);
        self.maybe_emit(step);
    }

    fn step(&self) -> gix::progress::Step {
        self.step.load(Ordering::Relaxed)
    }

    fn inc_by(&self, step: gix::progress::Step) {
        let new_step = self.step.fetch_add(step, Ordering::Relaxed) + step;
        self.maybe_emit(new_step);
    }

    fn counter(&self) -> StepShared {
        self.step.clone()
    }
}

impl gix::Progress for CloneProgress {
    fn init(&mut self, max: Option<gix::progress::Step>, _unit: Option<Unit>) {
        self.max = max;
    }

    fn set_max(&mut self, max: Option<gix::progress::Step>) -> Option<gix::progress::Step> {
        std::mem::replace(&mut self.max, max)
    }

    fn set_name(&mut self, name: String) {
        self.name = name;
    }

    fn name(&self) -> Option<String> {
        (!self.name.is_empty()).then(|| self.name.clone())
    }

    fn id(&self) -> Id {
        gix::progress::UNKNOWN
    }

    fn message(&self, _level: MessageLevel, message: String) {
        self.write_line(&message);
    }
}

impl gix::NestedProgress for CloneProgress {
    type SubProgress = Self;

    fn add_child(&mut self, name: impl Into<String>) -> Self {
        self.spawn_child(name.into())
    }

    fn add_child_with_id(&mut self, name: impl Into<String>, _id: Id) -> Self {
        self.spawn_child(name.into())
    }
}

/// Resolves `publish_dir` (a path relative to `checkout_dir`, or `None` for
/// the repo root) to an absolute path, rejecting anything that would escape
/// `checkout_dir` - same zip-slip-style guard as `zip_extract`'s
/// `enclosed_name()` check, needed here because there's no crate helper for
/// a plain relative path.
pub fn resolve_publish_dir(checkout_dir: &Path, publish_dir: Option<&str>) -> AppResult<PathBuf> {
    let Some(publish_dir) = publish_dir else {
        return Ok(checkout_dir.to_path_buf());
    };

    let relative = Path::new(publish_dir);
    if relative.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(AppError::InvalidPublishDir(publish_dir.to_string()));
    }

    let resolved = checkout_dir.join(relative);
    if !resolved.is_dir() {
        return Err(AppError::InvalidPublishDir(publish_dir.to_string()));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::{ensure_host_is_public, is_public_ipv4, is_public_ipv6, resolve_publish_dir};
    use crate::error::AppError;

    #[test]
    fn ipv4_private_and_reserved_ranges_are_rejected() {
        assert!(!is_public_ipv4("10.0.0.1".parse().unwrap()));
        assert!(!is_public_ipv4("172.16.0.1".parse().unwrap()));
        assert!(!is_public_ipv4("192.168.1.1".parse().unwrap()));
        assert!(!is_public_ipv4("127.0.0.1".parse().unwrap()));
        assert!(!is_public_ipv4("169.254.169.254".parse().unwrap())); // cloud metadata
        assert!(!is_public_ipv4("100.64.0.1".parse().unwrap())); // CGNAT
        assert!(!is_public_ipv4("0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn ipv4_public_address_is_accepted() {
        assert!(is_public_ipv4("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn ipv6_private_and_reserved_ranges_are_rejected() {
        assert!(!is_public_ipv6("::1".parse().unwrap())); // loopback
        assert!(!is_public_ipv6("fc00::1".parse().unwrap())); // unique local
        assert!(!is_public_ipv6("fe80::1".parse().unwrap())); // link local
        assert!(!is_public_ipv6("::ffff:10.0.0.1".parse().unwrap())); // IPv4-mapped private
    }

    #[test]
    fn ipv6_public_address_is_accepted() {
        assert!(is_public_ipv6("2001:4860:4860::8888".parse().unwrap()));
    }

    #[test]
    fn ensure_host_is_public_rejects_ip_literal_targets() {
        for url in [
            "http://127.0.0.1/repo.git",
            "http://169.254.169.254/latest/meta-data",
            "https://192.168.1.1/repo.git",
        ] {
            assert!(
                ensure_host_is_public(url).is_err(),
                "{url} should have been rejected"
            );
        }
    }

    #[test]
    fn ensure_host_is_public_accepts_public_ip_literal() {
        ensure_host_is_public("https://8.8.8.8/repo.git").expect("public IP should be allowed");
    }

    fn tempdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-git-fetch-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    #[test]
    fn none_resolves_to_checkout_root() {
        let checkout_dir = tempdir("root");
        let resolved = resolve_publish_dir(&checkout_dir, None).expect("resolve");
        assert_eq!(resolved, checkout_dir);
    }

    #[test]
    fn valid_nested_subdir_resolves() {
        let checkout_dir = tempdir("nested");
        std::fs::create_dir_all(checkout_dir.join("dist/site")).expect("create nested dir");

        let resolved = resolve_publish_dir(&checkout_dir, Some("dist/site")).expect("resolve");
        assert_eq!(resolved, checkout_dir.join("dist/site"));
    }

    #[test]
    fn parent_dir_escape_is_rejected() {
        let checkout_dir = tempdir("escape");
        let err = resolve_publish_dir(&checkout_dir, Some("../escape"))
            .expect_err("parent-dir escape must be rejected");
        assert!(matches!(err, AppError::InvalidPublishDir(_)));
    }

    #[test]
    fn absolute_path_is_rejected() {
        let checkout_dir = tempdir("absolute");
        let err = resolve_publish_dir(&checkout_dir, Some("/etc/passwd"))
            .expect_err("absolute path must be rejected");
        assert!(matches!(err, AppError::InvalidPublishDir(_)));
    }

    #[test]
    fn missing_subdir_is_rejected() {
        let checkout_dir = tempdir("missing");
        let err = resolve_publish_dir(&checkout_dir, Some("does-not-exist"))
            .expect_err("nonexistent publish dir must be rejected");
        assert!(matches!(err, AppError::InvalidPublishDir(_)));
    }
}
