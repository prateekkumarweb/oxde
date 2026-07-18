use std::{io::ErrorKind, path::Path};

use jiff::Timestamp;

use crate::{
    containers,
    error::{AppError, AppResult},
    git_fetch,
    models::{self, App, AppSource, Deployment, GitDeploymentInfo, GitSource},
    state::AppState,
};

/// Nothing under `tmp/` is ever referenced from `apps/`, so wiping it on
/// startup is always safe and finishes any create/delete a crash interrupted.
pub fn sweep_tmp_dir(state: &AppState) -> std::io::Result<()> {
    let tmp_dir = state.tmp_dir();
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }
    std::fs::create_dir_all(&tmp_dir)
}

pub fn create_app(state: &AppState, name: &str, source: AppSource) -> AppResult<App> {
    models::validate_slug(name)?;
    if let AppSource::Git(ref git_source) = source {
        models::validate_repo_url(&git_source.repo_url)?;
    }

    let staging = state.unique_tmp_path("create-app");
    std::fs::create_dir(&staging)?;
    std::fs::create_dir(staging.join("deployments"))?;

    let app = App {
        name: name.to_string(),
        created_at: Timestamp::now(),
        source,
    };
    write_json(&staging.join("app.json"), &app)?;

    // `rename` doubles as the uniqueness check: it fails if the target
    // already exists.
    std::fs::rename(&staging, state.apps_dir().join(name)).map_err(|err| {
        std::fs::remove_dir_all(&staging).ok();
        match err.kind() {
            ErrorKind::AlreadyExists | ErrorKind::DirectoryNotEmpty => {
                AppError::AppAlreadyExists(name.to_string())
            }
            _ => AppError::Io(err),
        }
    })?;

    Ok(app)
}

pub fn list_apps(state: &AppState) -> AppResult<Vec<App>> {
    let entries = match std::fs::read_dir(state.apps_dir()) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(AppError::Io(err)),
    };

    let mut apps: Vec<App> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let app_json = entry.path().join("app.json");
        if app_json.is_file() {
            apps.push(read_json(&app_json)?);
        }
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(apps)
}

pub fn get_app(state: &AppState, name: &str) -> AppResult<App> {
    let path = state.apps_dir().join(name).join("app.json");
    if !path.is_file() {
        return Err(AppError::AppNotFound(name.to_string()));
    }
    read_json(&path)
}

pub fn delete_app(state: &AppState, name: &str) -> AppResult<()> {
    let staging = state.unique_tmp_path("deleted");

    std::fs::rename(state.apps_dir().join(name), &staging).map_err(|err| match err.kind() {
        ErrorKind::NotFound => AppError::AppNotFound(name.to_string()),
        _ => AppError::Io(err),
    })?;

    std::fs::remove_dir_all(&staging)?;
    Ok(())
}

pub fn create_deployment(
    state: &AppState,
    app_name: &str,
    zip_path: &Path,
    original_filename: Option<String>,
    upload_size_bytes: u64,
) -> AppResult<Deployment> {
    let app_dir = state.apps_dir().join(app_name);
    if !app_dir.is_dir() {
        return Err(AppError::AppNotFound(app_name.to_string()));
    }

    let staging = state.unique_tmp_path("deployment");
    let deployment = match stage_deployment(
        state,
        &staging,
        app_name,
        zip_path,
        original_filename,
        upload_size_bytes,
    ) {
        Ok(deployment) => deployment,
        Err(err) => {
            std::fs::remove_dir_all(&staging).ok();
            return Err(err);
        }
    };

    let target = app_dir.join("deployments").join(&deployment.id);
    std::fs::rename(&staging, &target).map_err(|err| {
        std::fs::remove_dir_all(&staging).ok();
        AppError::Io(err)
    })?;

    activate_deployment(state, app_name, &deployment.id)?;
    Ok(deployment)
}

fn stage_deployment(
    state: &AppState,
    staging: &Path,
    app_name: &str,
    zip_path: &Path,
    original_filename: Option<String>,
    upload_size_bytes: u64,
) -> AppResult<Deployment> {
    std::fs::create_dir(staging)?;
    let files_dir = staging.join("files");
    std::fs::create_dir(&files_dir)?;

    let zip_file = std::fs::File::open(zip_path)?;
    crate::zip_extract::unpack_zip(zip_file, &files_dir, state.max_uncompressed_bytes())?;

    let now = Timestamp::now();
    let deployment = Deployment {
        id: format!("{}-{}", now.as_millisecond(), state.next_seq()),
        app: app_name.to_string(),
        created_at: now,
        original_filename,
        upload_size_bytes,
        git: None,
        container_name: None,
    };
    write_json(&staging.join("deployment.json"), &deployment)?;
    Ok(deployment)
}

/// Fetches the app's configured branch and checks it out as a new
/// deployment, reusing `create_deployment`'s stage-then-atomic-rename shape.
/// Unlike `create_deployment`, this does **not** auto-activate: a run-mode
/// deployment's activation also has to start a container, which needs an
/// async runtime, so the caller (`routes::api::deploy_from_git`) activates
/// explicitly afterward.
pub fn create_git_deployment(state: &AppState, app_name: &str) -> AppResult<Deployment> {
    let app = get_app(state, app_name)?;
    let AppSource::Git(git_source) = app.source else {
        return Err(AppError::NotGitSourced(app_name.to_string()));
    };

    let app_dir = state.apps_dir().join(app_name);
    let staging = state.unique_tmp_path("git-deployment");
    let deployment = match stage_git_deployment(state, &staging, app_name, &git_source) {
        Ok(deployment) => deployment,
        Err(err) => {
            std::fs::remove_dir_all(&staging).ok();
            return Err(err);
        }
    };

    let target = app_dir.join("deployments").join(&deployment.id);
    std::fs::rename(&staging, &target).map_err(|err| {
        std::fs::remove_dir_all(&staging).ok();
        AppError::Io(err)
    })?;

    Ok(deployment)
}

fn stage_git_deployment(
    state: &AppState,
    staging: &Path,
    app_name: &str,
    git_source: &GitSource,
) -> AppResult<Deployment> {
    std::fs::create_dir(staging)?;
    let checkout_dir = staging.join("_checkout");
    let commit_sha =
        git_fetch::clone_shallow(&git_source.repo_url, &git_source.branch, &checkout_dir)?;
    // Must happen before resolving publish_dir/moving the checkout: if
    // publish_dir is the repo root (or this is a run-mode deployment, which
    // uses the whole checkout), .git would otherwise end up inside files/.
    std::fs::remove_dir_all(checkout_dir.join(".git"))?;

    let now = Timestamp::now();
    let id = format!("{}-{}", now.as_millisecond(), state.next_seq());

    // Run mode ignores `publish_dir` and uses the whole checkout, since the
    // container needs the full repo (package.json, etc.), not a served
    // subtree.
    let container_name = if git_source.run.is_some() {
        std::fs::rename(&checkout_dir, staging.join("files"))?;
        Some(containers::container_name(app_name, &id))
    } else {
        let content_root =
            git_fetch::resolve_publish_dir(&checkout_dir, git_source.publish_dir.as_deref())?;
        std::fs::rename(&content_root, staging.join("files"))?;
        std::fs::remove_dir_all(&checkout_dir).ok();
        None
    };

    let content_size = dir_size_bytes(&staging.join("files"))?;
    let deployment = Deployment {
        id,
        app: app_name.to_string(),
        created_at: now,
        original_filename: None,
        upload_size_bytes: content_size,
        git: Some(GitDeploymentInfo {
            commit_sha,
            branch: git_source.branch.clone(),
        }),
        container_name,
    };
    write_json(&staging.join("deployment.json"), &deployment)?;
    Ok(deployment)
}

fn dir_size_bytes(dir: &Path) -> AppResult<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        total += if file_type.is_dir() {
            dir_size_bytes(&entry.path())?
        } else {
            entry.metadata()?.len()
        };
    }
    Ok(total)
}

pub fn list_deployments(state: &AppState, app_name: &str) -> AppResult<Vec<Deployment>> {
    let deployments_dir = state.apps_dir().join(app_name).join("deployments");
    let entries = match std::fs::read_dir(&deployments_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(AppError::AppNotFound(app_name.to_string()));
        }
        Err(err) => return Err(AppError::Io(err)),
    };

    let mut deployments: Vec<Deployment> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let deployment_json = entry.path().join("deployment.json");
        if deployment_json.is_file() {
            deployments.push(read_json(&deployment_json)?);
        }
    }
    deployments.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(deployments)
}

pub fn get_deployment(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<Deployment> {
    let path = state
        .apps_dir()
        .join(app_name)
        .join("deployments")
        .join(deployment_id)
        .join("deployment.json");
    if !path.is_file() {
        return Err(AppError::DeploymentNotFound(deployment_id.to_string()));
    }
    read_json(&path)
}

/// The active deployment id, derived by reading the `active` symlink rather
/// than stored anywhere - so there's exactly one source of truth for "live".
pub fn active_deployment_id(state: &AppState, app_name: &str) -> Option<String> {
    let target = std::fs::read_link(state.apps_dir().join(app_name).join("active")).ok()?;
    target.file_name()?.to_str().map(str::to_string)
}

pub fn activate_deployment(state: &AppState, app_name: &str, deployment_id: &str) -> AppResult<()> {
    let app_dir = state.apps_dir().join(app_name);
    let deployment_dir = app_dir.join("deployments").join(deployment_id);
    if !deployment_dir.is_dir() {
        return Err(AppError::DeploymentNotFound(deployment_id.to_string()));
    }

    let guard = state.write_lock();
    let tmp_link = state.unique_tmp_path("active-link");
    std::os::unix::fs::symlink(Path::new("deployments").join(deployment_id), &tmp_link)?;
    std::fs::rename(&tmp_link, app_dir.join("active"))?;
    drop(guard);
    Ok(())
}

pub fn delete_deployment(state: &AppState, app_name: &str, deployment_id: &str) -> AppResult<()> {
    let deployments_dir = state.apps_dir().join(app_name).join("deployments");
    let staging = state.unique_tmp_path("deleted-deployment");

    let guard = state.write_lock();
    if active_deployment_id(state, app_name).as_deref() == Some(deployment_id) {
        return Err(AppError::DeleteActiveDeployment);
    }
    std::fs::rename(deployments_dir.join(deployment_id), &staging).map_err(|err| {
        match err.kind() {
            ErrorKind::NotFound => AppError::DeploymentNotFound(deployment_id.to_string()),
            _ => AppError::Io(err),
        }
    })?;
    drop(guard);

    std::fs::remove_dir_all(&staging)?;
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    let contents = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&contents)?)
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let contents = serde_json::to_string_pretty(value)?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io::Cursor;

    use super::{
        activate_deployment, create_app, create_deployment, create_git_deployment, delete_app,
        delete_deployment, get_app, list_apps, list_deployments,
    };
    use crate::{error::AppError, models::AppSource, state::AppState};

    /// A fresh `AppState` over its own tempdir, so tests never share state.
    fn test_state(label: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-storage-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(dir.join("apps")).expect("create apps dir");
        std::fs::create_dir_all(dir.join("tmp")).expect("create tmp dir");
        AppState::new(
            dir,
            10_000,
            10_000,
            "localhost".to_string(),
            60,
            // None of these tests exercise container behavior, so this
            // just needs to construct - `connect_with_http` doesn't touch
            // the filesystem/network the way a Unix-socket connect does
            // (which errors immediately if no Docker/Podman is installed),
            // so this succeeds without a real container runtime present.
            bollard::Docker::connect_with_http(
                "http://localhost:0",
                5,
                bollard::API_DEFAULT_VERSION,
            )
            .expect("build docker client"),
            crate::reverse_proxy::new_client(),
        )
    }

    fn tiny_zip(content: &[u8]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("index.html", zip::write::SimpleFileOptions::default())
            .expect("start_file");
        std::io::Write::write_all(&mut writer, content).expect("write contents");
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn create_list_get_app_round_trip() {
        let state = test_state("round-trip");

        let created = create_app(&state, "blog", AppSource::Upload).expect("create_app");
        assert_eq!(created.name, "blog");

        let fetched = get_app(&state, "blog").expect("get_app");
        assert_eq!(fetched.name, "blog");

        let listed = list_apps(&state).expect("list_apps");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "blog");
    }

    #[test]
    fn duplicate_create_is_rejected_and_leaves_tmp_clean() {
        let state = test_state("duplicate-create");
        create_app(&state, "blog", AppSource::Upload).expect("first create_app");

        let err =
            create_app(&state, "blog", AppSource::Upload).expect_err("duplicate create must fail");
        assert!(matches!(err, AppError::AppAlreadyExists(_)));

        let leftovers: Vec<_> = std::fs::read_dir(state.tmp_dir())
            .expect("read tmp dir")
            .collect();
        assert!(
            leftovers.is_empty(),
            "a failed create must not leave a staging dir behind in tmp/"
        );
    }

    #[test]
    fn delete_app_removes_it() {
        let state = test_state("delete-app");
        create_app(&state, "blog", AppSource::Upload).expect("create_app");

        delete_app(&state, "blog").expect("delete_app");

        let err = get_app(&state, "blog").expect_err("app should be gone");
        assert!(matches!(err, AppError::AppNotFound(_)));
    }

    #[test]
    fn delete_app_on_missing_app_is_not_found() {
        let state = test_state("delete-missing-app");
        let err = delete_app(&state, "nope").expect_err("deleting a missing app must fail");
        assert!(matches!(err, AppError::AppNotFound(_)));
    }

    #[test]
    fn deployment_lifecycle_activate_and_delete() {
        let state = test_state("deployment-lifecycle");
        create_app(&state, "blog", AppSource::Upload).expect("create_app");

        let zip_v1 = state.tmp_dir().join("v1.zip");
        std::fs::write(&zip_v1, tiny_zip(b"v1")).expect("write v1 zip");
        let v1 = create_deployment(&state, "blog", &zip_v1, None, 2).expect("create v1");

        let zip_v2 = state.tmp_dir().join("v2.zip");
        std::fs::write(&zip_v2, tiny_zip(b"v2")).expect("write v2 zip");
        let v2 = create_deployment(&state, "blog", &zip_v2, None, 2).expect("create v2");

        // Uploading auto-activates, so the newest deployment should be live.
        assert_eq!(
            super::active_deployment_id(&state, "blog"),
            Some(v2.id.clone())
        );

        let deployments = list_deployments(&state, "blog").expect("list_deployments");
        assert_eq!(deployments.len(), 2);

        // Rolling back to v1 must actually flip the active pointer.
        activate_deployment(&state, "blog", &v1.id).expect("activate v1");
        assert_eq!(
            super::active_deployment_id(&state, "blog"),
            Some(v1.id.clone())
        );

        // The active deployment can never be deleted directly...
        let err = delete_deployment(&state, "blog", &v1.id).expect_err("deleting active must fail");
        assert!(matches!(err, AppError::DeleteActiveDeployment));

        // ...but a non-active one can be, and it disappears from the listing.
        delete_deployment(&state, "blog", &v2.id).expect("delete v2");
        let remaining = list_deployments(&state, "blog").expect("list_deployments after delete");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, v1.id);
    }

    #[test]
    fn sweep_tmp_dir_finishes_an_interrupted_delete() {
        let state = test_state("sweep-recovery");
        create_app(&state, "blog", AppSource::Upload).expect("create_app");

        let zip = state.tmp_dir().join("v1.zip");
        std::fs::write(&zip, tiny_zip(b"v1")).expect("write zip");
        let deployment =
            create_deployment(&state, "blog", &zip, None, 2).expect("create deployment");

        // Simulate a crash between delete_deployment's rename-out-of-apps/
        // step and its remove_dir_all: do the rename ourselves and stop.
        let deployment_dir = state
            .apps_dir()
            .join("blog")
            .join("deployments")
            .join(&deployment.id);
        let orphan = state.tmp_dir().join("orphaned-partial-delete");
        std::fs::rename(&deployment_dir, &orphan).expect("simulate interrupted delete");

        assert!(
            list_deployments(&state, "blog")
                .expect("list_deployments")
                .is_empty(),
            "deployment must already be invisible before the sweep runs"
        );

        super::sweep_tmp_dir(&state).expect("sweep_tmp_dir");

        let leftovers: Vec<_> = std::fs::read_dir(state.tmp_dir())
            .expect("read tmp dir")
            .collect();
        assert!(
            leftovers.is_empty(),
            "startup sweep must finish an interrupted delete"
        );
    }

    #[test]
    fn create_git_deployment_on_upload_app_is_rejected() {
        let state = test_state("git-not-sourced");
        create_app(&state, "blog", AppSource::Upload).expect("create_app");

        let err = create_git_deployment(&state, "blog").expect_err("upload app must be rejected");
        assert!(matches!(err, AppError::NotGitSourced(_)));
    }
}
