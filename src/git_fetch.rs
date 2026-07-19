use std::{
    num::NonZeroU32,
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
};

use crate::error::{AppError, AppResult};

/// Shallow (depth 1), single-branch clone + checkout of `repo_url` at
/// `branch` into `dest`. Returns the checked-out commit's SHA.
pub fn clone_shallow(repo_url: &str, branch: &str, dest: &Path) -> AppResult<String> {
    let should_interrupt = AtomicBool::new(false);

    let mut prepare = gix::prepare_clone(repo_url, dest)
        .map_err(|err| AppError::Git(err.to_string()))?
        .with_ref_name(Some(branch))
        .map_err(|err| AppError::Git(err.to_string()))?
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(NonZeroU32::MIN));

    let (mut checkout, _) = prepare
        .fetch_then_checkout(gix::progress::Discard, &should_interrupt)
        .map_err(|err| AppError::Git(err.to_string()))?;
    let (repo, _) = checkout
        .main_worktree(gix::progress::Discard, &should_interrupt)
        .map_err(|err| AppError::Git(err.to_string()))?;

    let head_id = repo
        .head_id()
        .map_err(|err| AppError::Git(err.to_string()))?;
    Ok(head_id.to_string())
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
