use std::{
    io::Write as _,
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
    use super::resolve_publish_dir;
    use crate::error::AppError;

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
