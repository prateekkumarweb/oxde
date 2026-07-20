use std::{io::ErrorKind, path::Path};

use jiff::Timestamp;

use crate::{
    containers,
    error::{AppError, AppResult},
    git_fetch,
    models::{
        self, App, AppSource, BuildInfo, Deployment, DeploymentStatus, GitDeployMode,
        GitDeploymentInfo, GitSource,
    },
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

pub fn create_app(
    state: &AppState,
    name: &str,
    source: AppSource,
    env_vars: Vec<models::EnvVar>,
) -> AppResult<App> {
    models::validate_slug(name)?;
    models::validate_env_vars(&env_vars)?;
    if let AppSource::Git(ref git_source) = source {
        models::validate_repo_url(&git_source.repo_url)?;
        match &git_source.mode {
            GitDeployMode::Run(run) => models::validate_run_config(run)?,
            GitDeployMode::Build(build) => models::validate_build_config(build)?,
            GitDeployMode::Static { .. } => {}
        }
    }

    let staging = state.unique_tmp_path("create-app");
    std::fs::create_dir(&staging)?;
    std::fs::create_dir(staging.join("deployments"))?;

    let app = App {
        name: name.to_string(),
        created_at: Timestamp::now(),
        source,
        env_vars,
        permissions: Vec::new(),
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

/// Replaces the full env var list (not a merge by key). Doesn't touch any
/// running container - new values take effect on the next deploy/start.
pub fn update_app_env_vars(
    state: &AppState,
    name: &str,
    env_vars: Vec<models::EnvVar>,
) -> AppResult<App> {
    models::validate_env_vars(&env_vars)?;
    let path = state.apps_dir().join(name).join("app.json");
    if !path.is_file() {
        return Err(AppError::AppNotFound(name.to_string()));
    }
    let guard = state.write_lock();
    let mut app: App = read_json(&path)?;
    app.env_vars = env_vars;
    write_json(&path, &app)?;
    drop(guard);
    Ok(app)
}

/// Grants `username` `level` access to `app_name` - used to give a
/// `Member` who creates an app `Write` access to what they just made.
pub fn add_app_permission(
    state: &AppState,
    app_name: &str,
    username: &str,
    level: models::PermissionLevel,
) -> AppResult<()> {
    let path = state.apps_dir().join(app_name).join("app.json");
    if !path.is_file() {
        return Err(AppError::AppNotFound(app_name.to_string()));
    }
    let guard = state.write_lock();
    let mut app: App = read_json(&path)?;
    app.permissions.push(models::AppPermission {
        username: username.to_string(),
        level,
    });
    write_json(&path, &app)?;
    drop(guard);
    Ok(())
}

/// Replaces the full permissions list (not a merge) - the same
/// replace-wholesale pattern as `update_app_env_vars`.
pub fn update_app_permissions(
    state: &AppState,
    name: &str,
    permissions: Vec<models::AppPermission>,
) -> AppResult<App> {
    let path = state.apps_dir().join(name).join("app.json");
    if !path.is_file() {
        return Err(AppError::AppNotFound(name.to_string()));
    }
    let guard = state.write_lock();
    let mut app: App = read_json(&path)?;
    app.permissions = permissions;
    write_json(&path, &app)?;
    drop(guard);
    Ok(app)
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
        build_info: None,
        container_name: None,
        status: DeploymentStatus::Ready,
    };
    write_json(&staging.join("deployment.json"), &deployment)?;
    Ok(deployment)
}

/// Writes `deployment.json` directly rather than via the stage-then-rename
/// pattern used elsewhere here - the partial (`Pending`, no `files/`) state
/// is the point, so a caller can attach to its logs before the rest finishes.
pub fn create_pending_git_deployment(
    state: &AppState,
    app_name: &str,
) -> AppResult<(Deployment, GitSource)> {
    let app = get_app(state, app_name)?;
    let AppSource::Git(git_source) = app.source else {
        return Err(AppError::NotGitSourced(app_name.to_string()));
    };

    let guard = state.write_lock();
    let already_pending = list_deployments(state, app_name)?
        .iter()
        .any(|deployment| matches!(deployment.status, DeploymentStatus::Pending));
    if already_pending {
        drop(guard);
        return Err(AppError::DeploymentInProgress(app_name.to_string()));
    }

    let now = Timestamp::now();
    let id = format!("{}-{}", now.as_millisecond(), state.next_seq());
    let container_name = matches!(git_source.mode, GitDeployMode::Run(_))
        .then(|| containers::container_name(app_name, &id));

    let deployment = Deployment {
        id: id.clone(),
        app: app_name.to_string(),
        created_at: now,
        original_filename: None,
        upload_size_bytes: 0,
        git: None,
        build_info: None,
        container_name,
        status: DeploymentStatus::Pending,
    };

    let deployment_dir = state
        .apps_dir()
        .join(app_name)
        .join("deployments")
        .join(&id);
    std::fs::create_dir(&deployment_dir)?;
    write_json(&deployment_dir.join("deployment.json"), &deployment)?;
    drop(guard);

    Ok((deployment, git_source))
}

/// Clones the checkout but does *not* move it into `staging/files` yet - a
/// build deploy needs the raw checkout still in place to bind-mount into
/// the build container before `finish_git_deployment` can resolve its
/// output dir.
pub fn clone_repo(
    staging: &Path,
    git_source: &GitSource,
) -> AppResult<(std::path::PathBuf, String)> {
    std::fs::create_dir(staging)?;
    let checkout_dir = staging.join("_checkout");
    let commit_sha =
        git_fetch::clone_shallow(&git_source.repo_url, &git_source.branch, &checkout_dir)?;
    std::fs::remove_dir_all(checkout_dir.join(".git"))?;
    Ok((checkout_dir, commit_sha))
}

/// Resolves the servable content root - the whole checkout for `Run`, the
/// build's `output_dir` for `Build` (only valid once the build has run),
/// `publish_dir` for `Static` - then moves it into place and records the
/// git/build info. Leaves status as `Pending`; the caller flips it to
/// `Ready` (`mark_git_deployment_ready`) only after activation, so
/// install/build-command logs stay attached to this deployment the whole
/// time.
pub fn finish_git_deployment(
    state: &AppState,
    staging: &Path,
    checkout_dir: &Path,
    app_name: &str,
    deployment_id: &str,
    git_source: &GitSource,
    commit_sha: String,
) -> AppResult<()> {
    match &git_source.mode {
        GitDeployMode::Run(_) => {
            std::fs::rename(checkout_dir, staging.join("files"))?;
        }
        GitDeployMode::Static { publish_dir } => {
            let content_root =
                git_fetch::resolve_publish_dir(checkout_dir, publish_dir.as_deref())?;
            std::fs::rename(&content_root, staging.join("files"))?;
            std::fs::remove_dir_all(checkout_dir).ok();
        }
        GitDeployMode::Build(build) => {
            let content_root =
                git_fetch::resolve_publish_dir(checkout_dir, Some(&build.output_dir))?;
            std::fs::rename(&content_root, staging.join("files"))?;
            std::fs::remove_dir_all(checkout_dir).ok();
        }
    }

    let content_size = dir_size_bytes(&staging.join("files"))?;
    let deployment_dir = state
        .apps_dir()
        .join(app_name)
        .join("deployments")
        .join(deployment_id);
    std::fs::rename(staging.join("files"), deployment_dir.join("files"))?;

    let mut deployment: Deployment = read_json(&deployment_dir.join("deployment.json"))?;
    deployment.git = Some(GitDeploymentInfo {
        commit_sha,
        branch: git_source.branch.clone(),
    });
    deployment.build_info = match &git_source.mode {
        GitDeployMode::Build(build) => Some(BuildInfo {
            image: build.image,
            command: build.command.clone(),
        }),
        GitDeployMode::Static { .. } | GitDeployMode::Run(_) => None,
    };
    deployment.upload_size_bytes = content_size;
    write_json(&deployment_dir.join("deployment.json"), &deployment)?;
    std::fs::remove_dir_all(staging).ok();
    Ok(())
}

pub fn mark_git_deployment_ready(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<()> {
    set_deployment_status(state, app_name, deployment_id, DeploymentStatus::Ready)
}

pub fn fail_git_deployment(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
    error: &str,
) -> AppResult<()> {
    set_deployment_status(
        state,
        app_name,
        deployment_id,
        DeploymentStatus::Failed {
            error: error.to_string(),
        },
    )
}

fn set_deployment_status(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
    status: DeploymentStatus,
) -> AppResult<()> {
    let deployment_dir = state
        .apps_dir()
        .join(app_name)
        .join("deployments")
        .join(deployment_id);
    let mut deployment: Deployment = read_json(&deployment_dir.join("deployment.json"))?;
    deployment.status = status;
    write_json(&deployment_dir.join("deployment.json"), &deployment)
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
mod tests {
    use std::io::Cursor;

    use super::{
        activate_deployment, create_app, create_deployment, create_pending_git_deployment,
        delete_app, delete_deployment, get_app, list_apps, list_deployments,
    };
    use crate::{
        error::AppError,
        models::{AppSource, DeploymentStatus, GitDeployMode, GitSource},
        state::{AppState, AppStateLimits},
    };

    /// A fresh `AppState` over its own tempdir, so tests never share state.
    fn test_state(label: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-storage-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(dir.join("apps")).expect("create apps dir");
        std::fs::create_dir_all(dir.join("tmp")).expect("create tmp dir");
        let db = tokio::runtime::Runtime::new()
            .expect("build test runtime")
            .block_on(async {
                let db = oxde_db::connect(&dir)
                    .await
                    .expect("connect test accounts database");
                oxde_db::apply_migrations(&db)
                    .await
                    .expect("apply test accounts database migrations");
                db
            });
        AppState::new(
            dir,
            AppStateLimits {
                max_upload_bytes: 10_000,
                max_uncompressed_bytes: 10_000,
                base_domain: "localhost".to_string(),
                git_fetch_timeout_secs: 60,
                install_timeout_secs: 300,
                build_timeout_secs: 300,
            },
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
            db,
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

        let created =
            create_app(&state, "blog", AppSource::Upload, Vec::new()).expect("create_app");
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
        create_app(&state, "blog", AppSource::Upload, Vec::new()).expect("first create_app");

        let err = create_app(&state, "blog", AppSource::Upload, Vec::new())
            .expect_err("duplicate create must fail");
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
        create_app(&state, "blog", AppSource::Upload, Vec::new()).expect("create_app");

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
        create_app(&state, "blog", AppSource::Upload, Vec::new()).expect("create_app");

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
        create_app(&state, "blog", AppSource::Upload, Vec::new()).expect("create_app");

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
        create_app(&state, "blog", AppSource::Upload, Vec::new()).expect("create_app");

        let err =
            create_pending_git_deployment(&state, "blog").expect_err("upload app must be rejected");
        assert!(matches!(err, AppError::NotGitSourced(_)));
    }

    #[test]
    fn create_pending_git_deployment_is_rejected_while_one_is_already_pending() {
        let state = test_state("git-deploy-in-progress");
        create_app(
            &state,
            "site",
            AppSource::Git(GitSource {
                repo_url: "https://example.com/repo.git".to_string(),
                branch: "main".to_string(),
                mode: GitDeployMode::default(),
            }),
            Vec::new(),
        )
        .expect("create_app");

        let (first, _) =
            create_pending_git_deployment(&state, "site").expect("first pending deploy");
        assert!(matches!(first.status, DeploymentStatus::Pending));

        let err = create_pending_git_deployment(&state, "site")
            .expect_err("a second deploy while one is pending must be rejected");
        assert!(matches!(err, AppError::DeploymentInProgress(_)));
    }
}
