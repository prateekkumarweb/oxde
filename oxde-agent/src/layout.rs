use std::path::{Path, PathBuf};

/// Flat by `deployment_id` alone - the agent only ever owns content for
/// run-mode deployments, and `deployment_id` (a `UUIDv7`) is already
/// globally unique, so there's no need to nest under an app-level
/// directory the way the hub's own (log-only) tree does.
pub fn deployments_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("deployments")
}

pub fn deployment_dir(data_dir: &Path, deployment_id: &str) -> PathBuf {
    deployments_dir(data_dir).join(deployment_id)
}

pub fn deployment_files_dir(data_dir: &Path, deployment_id: &str) -> PathBuf {
    deployment_dir(data_dir, deployment_id).join("files")
}

pub fn tmp_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tmp")
}

/// Scratch path for extracting an uploaded zip before it's renamed into
/// place - scoped by `deployment_id`, which is already unique, so no
/// further uniqueness scheme is needed.
pub fn upload_staging_dir(data_dir: &Path, deployment_id: &str) -> PathBuf {
    tmp_dir(data_dir).join(format!("upload-{deployment_id}"))
}
