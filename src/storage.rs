use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    path::Path,
    str::FromStr,
};

use jiff::Timestamp;
use oxde_db::models::{
    ApiToken as DbApiToken, App as DbApp, AppPermission as DbAppPermission,
    Deployment as DbDeployment, DeploymentState, PermissionLevel as DbPermissionLevel,
    User as DbUser,
};
use toasty::Db;
use uuid::Uuid;

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

/// Deletes any `apps/<id>` directory with no matching `App` row, and any
/// `apps/<id>/deployments/<id>` directory with no matching `Deployment`
/// row - cleans up orphans left by a crash between staging a directory into
/// place and committing its DB row (or the reverse ordering on delete).
pub async fn sweep_orphaned_dirs(state: &AppState) -> AppResult<()> {
    let mut db = state.db().clone();
    let valid_app_ids: HashSet<Uuid> = DbApp::all()
        .exec(&mut db)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();

    let entries = match std::fs::read_dir(state.apps_dir()) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(AppError::Io(err)),
    };

    let mut removed = 0u32;
    for entry in entries {
        let entry = entry?;
        let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(app_id) = Uuid::parse_str(&dir_name) else {
            continue;
        };

        if valid_app_ids.contains(&app_id) {
            removed += sweep_orphaned_deployment_dirs(&mut db, state, &dir_name, app_id).await?;
        } else {
            tracing::warn!(app_id = %app_id, "removing orphaned app directory with no matching App row");
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    tracing::info!(removed, "orphan sweep complete");
    Ok(())
}

async fn sweep_orphaned_deployment_dirs(
    db: &mut Db,
    state: &AppState,
    app_dir_name: &str,
    app_id: Uuid,
) -> AppResult<u32> {
    let valid_deployment_ids: HashSet<Uuid> = DbDeployment::all()
        .filter(DbDeployment::fields().app_id().eq(app_id))
        .exec(db)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();

    let deployments_dir = state.apps_dir().join(app_dir_name).join("deployments");
    let entries = match std::fs::read_dir(&deployments_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(AppError::Io(err)),
    };

    let mut removed = 0u32;
    for entry in entries {
        let entry = entry?;
        let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(deployment_id) = Uuid::parse_str(&dir_name) else {
            continue;
        };
        if !valid_deployment_ids.contains(&deployment_id) {
            tracing::warn!(
                app_id = %app_id,
                deployment_id = %deployment_id,
                "removing orphaned deployment directory with no matching Deployment row"
            );
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

async fn find_app_row(db: &mut Db, name: &str) -> AppResult<DbApp> {
    DbApp::all()
        .filter(DbApp::fields().name().eq(name))
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| AppError::AppNotFound(name.to_string()))
}

async fn find_deployment_row(
    db: &mut Db,
    app_id: Uuid,
    deployment_id: &str,
) -> AppResult<DbDeployment> {
    let deployment_id = Uuid::parse_str(deployment_id)
        .map_err(|_| AppError::DeploymentNotFound(deployment_id.to_string()))?;
    DbDeployment::all()
        .filter(
            DbDeployment::fields()
                .id()
                .eq(deployment_id)
                .and(DbDeployment::fields().app_id().eq(app_id)),
        )
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| AppError::DeploymentNotFound(deployment_id.to_string()))
}

async fn find_user_id(db: &mut Db, username: &str) -> AppResult<i64> {
    DbUser::all()
        .filter(DbUser::fields().username().eq(username))
        .first()
        .exec(db)
        .await?
        .map(|user| user.id)
        .ok_or_else(|| AppError::UserNotFound(username.to_string()))
}

pub async fn create_api_token(
    state: &AppState,
    user_id: i64,
    name: &str,
    expires_at: i64,
) -> AppResult<(DbApiToken, String)> {
    let now = Timestamp::now().as_second();
    crate::api_tokens::validate_expiry(now, expires_at, state.api_token_max_expiry_days())?;

    let (token_id, secret, token_hash) = crate::api_tokens::generate()?;
    let mut db = state.db().clone();
    let row = DbApiToken::create()
        .user_id(user_id)
        .name(name)
        .token_id(&token_id)
        .token_hash(token_hash)
        .expires_at(expires_at)
        .revoked(false)
        .created_at(now)
        .updated_at(now)
        .exec(&mut db)
        .await?;

    let plaintext = crate::api_tokens::format_token(&token_id, &secret);
    Ok((row, plaintext))
}

pub async fn list_api_tokens(state: &AppState, user_id: i64) -> AppResult<Vec<DbApiToken>> {
    let mut db = state.db().clone();
    Ok(DbApiToken::all()
        .filter(DbApiToken::fields().user_id().eq(user_id))
        .exec(&mut db)
        .await?)
}

/// Scoped to `user_id` as well as `id` - never lets a user revoke another
/// user's token by guessing an id.
pub async fn revoke_api_token(state: &AppState, user_id: i64, token_row_id: i64) -> AppResult<()> {
    let mut db = state.db().clone();
    let mut token = DbApiToken::all()
        .filter(DbApiToken::fields().id().eq(token_row_id))
        .filter(DbApiToken::fields().user_id().eq(user_id))
        .first()
        .exec(&mut db)
        .await?
        .ok_or(AppError::TokenNotFound)?;

    token
        .update()
        .revoked(true)
        .updated_at(Timestamp::now().as_second())
        .exec(&mut db)
        .await?;
    Ok(())
}

/// Used only by `auth::ApiUser`'s bearer path.
pub async fn find_user_by_api_token(
    state: &AppState,
    token_id: &str,
    secret: &str,
) -> AppResult<Option<DbUser>> {
    let mut db = state.db().clone();
    let Some(token) = DbApiToken::all()
        .filter(DbApiToken::fields().token_id().eq(token_id))
        .first()
        .exec(&mut db)
        .await?
    else {
        return Ok(None);
    };

    if token.revoked || token.expires_at <= Timestamp::now().as_second() {
        return Ok(None);
    }
    if !crate::api_tokens::verify_secret(secret, &token.token_hash) {
        return Ok(None);
    }

    Ok(Some(token.user().exec(&mut db).await?))
}

/// A user deleted after being granted access leaves an orphaned grant with
/// no matching `User` row - skipped rather than failing the whole app read.
async fn load_permissions(db: &mut Db, app_row: &DbApp) -> AppResult<Vec<models::AppPermission>> {
    let rows = app_row.permissions().exec(db).await?;

    let mut user_ids: Vec<i64> = rows.iter().map(|row| row.user_id).collect();
    user_ids.sort_unstable();
    user_ids.dedup();
    let usernames: HashMap<i64, String> = DbUser::all()
        .filter(DbUser::fields().id().in_list(user_ids))
        .exec(db)
        .await?
        .into_iter()
        .map(|user| (user.id, user.username))
        .collect();

    let mut permissions = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(username) = usernames.get(&row.user_id) else {
            continue;
        };
        let level = DbPermissionLevel::from_str(&row.level).map_err(AppError::CorruptData)?;
        permissions.push(models::AppPermission {
            username: username.clone(),
            level: match level {
                DbPermissionLevel::Read => models::PermissionLevel::Read,
                DbPermissionLevel::Write => models::PermissionLevel::Write,
            },
        });
    }
    Ok(permissions)
}

fn app_from_row(row: DbApp, permissions: Vec<models::AppPermission>) -> AppResult<App> {
    Ok(App {
        id: row.id.to_string(),
        name: row.name,
        created_at: Timestamp::from_second(row.created_at)?,
        updated_at: Timestamp::from_second(row.updated_at)?,
        source: serde_json::from_str(&row.source_json)?,
        env_vars: serde_json::from_str(&row.env_vars_json)?,
        permissions,
    })
}

async fn app_from_row_with_permissions(db: &mut Db, row: DbApp) -> AppResult<App> {
    let permissions = load_permissions(db, &row).await?;
    app_from_row(row, permissions)
}

fn deployment_from_row(row: DbDeployment, app_name: &str) -> AppResult<Deployment> {
    let state_tag = DeploymentState::from_str(&row.status).map_err(AppError::CorruptData)?;
    let status = match state_tag {
        DeploymentState::Pending => DeploymentStatus::Pending,
        DeploymentState::Ready => DeploymentStatus::Ready,
        DeploymentState::Failed => DeploymentStatus::Failed {
            error: row.failure_error.unwrap_or_default(),
        },
    };
    let git = row
        .git_info_json
        .map(|json| serde_json::from_str::<GitDeploymentInfo>(&json))
        .transpose()?;
    let build_info = row
        .build_info_json
        .map(|json| serde_json::from_str::<BuildInfo>(&json))
        .transpose()?;

    Ok(Deployment {
        id: row.id.to_string(),
        app: app_name.to_string(),
        created_at: Timestamp::from_second(row.created_at)?,
        original_filename: row.original_filename,
        upload_size_bytes: u64::try_from(row.upload_size_bytes).unwrap_or(0),
        git,
        build_info,
        container_name: row.container_name,
        status,
    })
}

/// `creator`, if given, is granted `Write` access in the same transaction as
/// the app insert - used so a `Member` who creates an app is never left
/// locked out of what they just made, without a separate non-atomic write.
pub async fn create_app(
    state: &AppState,
    name: &str,
    source: AppSource,
    env_vars: Vec<models::EnvVar>,
    creator: Option<&str>,
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

    let mut db = state.db().clone();
    let guard = state.write_lock().await;
    let result = async {
        if DbApp::all()
            .filter(DbApp::fields().name().eq(name))
            .first()
            .exec(&mut db)
            .await?
            .is_some()
        {
            return Err(AppError::AppAlreadyExists(name.to_string()));
        }
        let creator_id = match creator {
            Some(username) => Some(find_user_id(&mut db, username).await?),
            None => None,
        };

        let id = Uuid::now_v7();
        let staging = state.unique_tmp_path("create-app");
        std::fs::create_dir(&staging)?;
        std::fs::create_dir(staging.join("deployments"))?;
        std::fs::rename(&staging, state.apps_dir().join(id.to_string())).map_err(|err| {
            std::fs::remove_dir_all(&staging).ok();
            AppError::Io(err)
        })?;

        let now = Timestamp::now().as_second();
        let mut tx = db.transaction().await?;
        let row = DbApp::create()
            .id(id)
            .name(name)
            .source_json(serde_json::to_string(&source)?)
            .env_vars_json(serde_json::to_string(&env_vars)?)
            .created_at(now)
            .updated_at(now)
            .exec(&mut tx)
            .await?;
        if let Some(user_id) = creator_id {
            DbAppPermission::create()
                .app_id(row.id)
                .user_id(user_id)
                .level(DbPermissionLevel::Write.as_str())
                .created_at(now)
                .updated_at(now)
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;

        let permissions = load_permissions(&mut db, &row).await?;
        app_from_row(row, permissions)
    }
    .await;
    drop(guard);
    result
}

pub async fn list_apps(state: &AppState) -> AppResult<Vec<App>> {
    let mut db = state.db().clone();
    let mut rows = DbApp::all().exec(&mut db).await?;
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let mut apps = Vec::with_capacity(rows.len());
    for row in rows {
        apps.push(app_from_row_with_permissions(&mut db, row).await?);
    }
    Ok(apps)
}

pub async fn get_app(state: &AppState, name: &str) -> AppResult<App> {
    let mut db = state.db().clone();
    let row = find_app_row(&mut db, name).await?;
    app_from_row_with_permissions(&mut db, row).await
}

/// Replaces the full env var list (not a merge by key). Doesn't touch any
/// running container - new values take effect on the next deploy/start.
pub async fn update_app_env_vars(
    state: &AppState,
    name: &str,
    env_vars: Vec<models::EnvVar>,
) -> AppResult<App> {
    models::validate_env_vars(&env_vars)?;
    let mut db = state.db().clone();
    let mut row = find_app_row(&mut db, name).await?;

    let mut update = row.update();
    update = update
        .env_vars_json(serde_json::to_string(&env_vars)?)
        .updated_at(Timestamp::now().as_second());
    update.exec(&mut db).await?;

    app_from_row_with_permissions(&mut db, row).await
}

/// Replaces the full permissions list (not a merge) - the same
/// replace-wholesale pattern as `update_app_env_vars`.
pub async fn update_app_permissions(
    state: &AppState,
    name: &str,
    permissions: Vec<models::AppPermission>,
) -> AppResult<App> {
    let mut db = state.db().clone();
    let app_row = find_app_row(&mut db, name).await?;

    let mut resolved = Vec::with_capacity(permissions.len());
    for permission in &permissions {
        let user_id = find_user_id(&mut db, &permission.username).await?;
        resolved.push((user_id, permission.level));
    }

    let now = Timestamp::now().as_second();
    let mut tx = db.transaction().await?;
    DbAppPermission::all()
        .filter(DbAppPermission::fields().app_id().eq(app_row.id))
        .delete()
        .exec(&mut tx)
        .await?;
    for (user_id, level) in resolved {
        let db_level = match level {
            models::PermissionLevel::Read => DbPermissionLevel::Read,
            models::PermissionLevel::Write => DbPermissionLevel::Write,
        };
        DbAppPermission::create()
            .app_id(app_row.id)
            .user_id(user_id)
            .level(db_level.as_str())
            .created_at(now)
            .updated_at(now)
            .exec(&mut tx)
            .await?;
    }
    tx.commit().await?;

    app_from_row_with_permissions(&mut db, app_row).await
}

pub async fn delete_app(state: &AppState, name: &str) -> AppResult<()> {
    let mut db = state.db().clone();
    let guard = state.write_lock().await;
    let result: AppResult<()> = async {
        let app_row = find_app_row(&mut db, name).await?;

        let mut tx = db.transaction().await?;
        DbDeployment::all()
            .filter(DbDeployment::fields().app_id().eq(app_row.id))
            .delete()
            .exec(&mut tx)
            .await?;
        DbAppPermission::all()
            .filter(DbAppPermission::fields().app_id().eq(app_row.id))
            .delete()
            .exec(&mut tx)
            .await?;
        DbApp::all()
            .filter(DbApp::fields().id().eq(app_row.id))
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;

        let staging = state.unique_tmp_path("deleted");
        std::fs::rename(state.apps_dir().join(app_row.id.to_string()), &staging)?;
        std::fs::remove_dir_all(&staging)?;
        Ok(())
    }
    .await;
    drop(guard);
    result
}

fn stage_deployment_files(
    staging: &Path,
    zip_path: &Path,
    max_uncompressed_bytes: u64,
) -> AppResult<()> {
    std::fs::create_dir(staging)?;
    let files_dir = staging.join("files");
    std::fs::create_dir(&files_dir)?;
    let zip_file = std::fs::File::open(zip_path)?;
    crate::zip_extract::unpack_zip(zip_file, &files_dir, max_uncompressed_bytes)?;
    Ok(())
}

pub async fn create_deployment(
    state: &AppState,
    app_name: &str,
    zip_path: &Path,
    original_filename: Option<String>,
    upload_size_bytes: u64,
) -> AppResult<Deployment> {
    let mut db = state.db().clone();

    // Staging is independent of any app's directory tree, so it doesn't
    // need `write_lock` - only the app-existence check, rename into place,
    // and DB insert below do (see `activate_deployment`'s doc comment for
    // why: without the lock, a concurrent `delete_app` could tear down the
    // app's directory out from under this rename, or leave this insert
    // referencing an app that's already gone).
    let id = Uuid::now_v7();
    let staging = state.unique_tmp_path("deployment");
    let blocking_staging = staging.clone();
    let blocking_zip_path = zip_path.to_path_buf();
    let max_uncompressed_bytes = state.max_uncompressed_bytes();
    tokio::task::spawn_blocking(move || {
        stage_deployment_files(
            &blocking_staging,
            &blocking_zip_path,
            max_uncompressed_bytes,
        )
    })
    .await
    .map_err(|err| AppError::Io(std::io::Error::other(err.to_string())))?
    .inspect_err(|_| {
        std::fs::remove_dir_all(&staging).ok();
    })?;

    let guard = state.write_lock().await;
    let result: AppResult<Deployment> = async {
        let app_row = find_app_row(&mut db, app_name).await.inspect_err(|_| {
            std::fs::remove_dir_all(&staging).ok();
        })?;

        let target = state.deployment_dir(&app_row.id.to_string(), &id.to_string());
        std::fs::rename(&staging, &target).map_err(|err| {
            std::fs::remove_dir_all(&staging).ok();
            AppError::Io(err)
        })?;

        let row = DbDeployment::create()
            .id(id)
            .app_id(app_row.id)
            .created_at(Timestamp::now().as_second())
            .original_filename(original_filename)
            .upload_size_bytes(i64::try_from(upload_size_bytes).unwrap_or(i64::MAX))
            .status(DeploymentState::Ready.as_str())
            .exec(&mut db)
            .await
            .inspect_err(|_| {
                std::fs::remove_dir_all(&target).ok();
            })?;

        deployment_from_row(row, app_name)
    }
    .await;
    drop(guard);
    let deployment = result?;

    activate_deployment(state, app_name, &deployment.id).await?;
    Ok(deployment)
}

/// Creates the `Pending` record synchronously (no `files/` yet) so a caller
/// can attach to its logs before the rest finishes.
pub async fn create_pending_git_deployment(
    state: &AppState,
    app_name: &str,
) -> AppResult<(Deployment, GitSource)> {
    let mut db = state.db().clone();
    let guard = state.write_lock().await;
    let result = async {
        let app_row = find_app_row(&mut db, app_name).await?;
        let source: AppSource = serde_json::from_str(&app_row.source_json)?;
        let AppSource::Git(git_source) = source else {
            return Err(AppError::NotGitSourced(app_name.to_string()));
        };

        let already_pending = DbDeployment::all()
            .filter(DbDeployment::fields().app_id().eq(app_row.id))
            .exec(&mut db)
            .await?
            .iter()
            .any(|row| row.status == DeploymentState::Pending.as_str());
        if already_pending {
            return Err(AppError::DeploymentInProgress(app_name.to_string()));
        }

        let id = Uuid::now_v7();
        let container_name = matches!(git_source.mode, GitDeployMode::Run(_))
            .then(|| containers::container_name(app_name, &id.to_string()));

        let deployment_dir = state.deployment_dir(&app_row.id.to_string(), &id.to_string());
        std::fs::create_dir(&deployment_dir)?;

        let row = DbDeployment::create()
            .id(id)
            .app_id(app_row.id)
            .created_at(Timestamp::now().as_second())
            .upload_size_bytes(0)
            .container_name(container_name)
            .status(DeploymentState::Pending.as_str())
            .exec(&mut db)
            .await?;

        let deployment = deployment_from_row(row, app_name)?;
        Ok((deployment, git_source))
    }
    .await;
    drop(guard);
    result
}

/// Clones the checkout but does *not* move it into `staging/files` yet - a
/// build deploy needs the raw checkout still in place to bind-mount into
/// the build container before `finish_git_deployment` can resolve its
/// output dir.
pub fn clone_repo(
    staging: &Path,
    git_source: &GitSource,
    log_target: Option<crate::deployment_logs::LogTarget>,
) -> AppResult<(std::path::PathBuf, String)> {
    std::fs::create_dir(staging)?;
    let checkout_dir = staging.join("_checkout");
    let commit_sha = git_fetch::clone_shallow(
        &git_source.repo_url,
        &git_source.branch,
        &checkout_dir,
        log_target,
    )?;
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
pub async fn finish_git_deployment(
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

    let mut db = state.db().clone();
    let app_row = find_app_row(&mut db, app_name).await?;
    let deployment_dir = state.deployment_dir(&app_row.id.to_string(), deployment_id);
    std::fs::rename(staging.join("files"), deployment_dir.join("files"))?;

    let mut deployment_row = find_deployment_row(&mut db, app_row.id, deployment_id).await?;
    let git_info_json = serde_json::to_string(&GitDeploymentInfo {
        commit_sha,
        branch: git_source.branch.clone(),
    })?;
    let build_info_json = match &git_source.mode {
        GitDeployMode::Build(build) => Some(serde_json::to_string(&BuildInfo {
            image: build.image,
            command: build.command.clone(),
        })?),
        GitDeployMode::Static { .. } | GitDeployMode::Run(_) => None,
    };

    let mut update = deployment_row.update();
    update = update
        .git_info_json(Some(git_info_json))
        .build_info_json(build_info_json)
        .upload_size_bytes(i64::try_from(content_size).unwrap_or(i64::MAX));
    update.exec(&mut db).await?;

    std::fs::remove_dir_all(staging).ok();
    Ok(())
}

pub async fn mark_git_deployment_ready(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<()> {
    set_deployment_status(state, app_name, deployment_id, DeploymentStatus::Ready).await
}

pub async fn fail_git_deployment(
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
    .await
}

async fn set_deployment_status(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
    status: DeploymentStatus,
) -> AppResult<()> {
    let mut db = state.db().clone();
    let app_row = find_app_row(&mut db, app_name).await?;
    let mut deployment_row = find_deployment_row(&mut db, app_row.id, deployment_id).await?;

    let (state_tag, failure_error) = match status {
        DeploymentStatus::Pending => (DeploymentState::Pending, None),
        DeploymentStatus::Ready => (DeploymentState::Ready, None),
        DeploymentStatus::Failed { error } => (DeploymentState::Failed, Some(error)),
    };

    let mut update = deployment_row.update();
    update = update
        .status(state_tag.as_str())
        .failure_error(failure_error);
    update.exec(&mut db).await?;
    Ok(())
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

pub async fn list_deployments(state: &AppState, app_name: &str) -> AppResult<Vec<Deployment>> {
    let mut db = state.db().clone();
    let app_row = find_app_row(&mut db, app_name).await?;
    let mut rows = DbDeployment::all()
        .filter(DbDeployment::fields().app_id().eq(app_row.id))
        .exec(&mut db)
        .await?;
    rows.sort_by_key(|row| row.id);
    rows.into_iter()
        .map(|row| deployment_from_row(row, app_name))
        .collect()
}

pub async fn get_deployment(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<Deployment> {
    let mut db = state.db().clone();
    let app_row = find_app_row(&mut db, app_name).await?;
    let row = find_deployment_row(&mut db, app_row.id, deployment_id).await?;
    deployment_from_row(row, app_name)
}

/// The active deployment id, read from the `App` row's `active_deployment_id`
/// column rather than derived from anything on disk.
pub async fn active_deployment_id(state: &AppState, app_name: &str) -> Option<String> {
    let mut db = state.db().clone();
    let row = find_app_row(&mut db, app_name).await.ok()?;
    row.active_deployment_id.map(|id| id.to_string())
}

/// Holds `write_lock` so this can't race `delete_deployment`'s "is this the
/// active deployment" check - without it, a delete could read the *old*
/// active id, lose the race to a concurrent activate, and remove the
/// deployment that just became active, leaving `active_deployment_id`
/// dangling.
pub async fn activate_deployment(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<()> {
    let mut db = state.db().clone();
    let guard = state.write_lock().await;
    let result: AppResult<()> = async {
        let mut app_row = find_app_row(&mut db, app_name).await?;
        let deployment_row = find_deployment_row(&mut db, app_row.id, deployment_id).await?;

        let mut update = app_row.update();
        update = update
            .active_deployment_id(Some(deployment_row.id))
            .updated_at(Timestamp::now().as_second());
        update.exec(&mut db).await?;
        Ok(())
    }
    .await;
    drop(guard);
    result
}

pub async fn delete_deployment(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<()> {
    let mut db = state.db().clone();
    let guard = state.write_lock().await;
    let result: AppResult<Uuid> = async {
        let app_row = find_app_row(&mut db, app_name).await?;
        if app_row
            .active_deployment_id
            .map(|id| id.to_string())
            .as_deref()
            == Some(deployment_id)
        {
            return Err(AppError::DeleteActiveDeployment);
        }
        let deployment_row = find_deployment_row(&mut db, app_row.id, deployment_id).await?;

        DbDeployment::all()
            .filter(DbDeployment::fields().id().eq(deployment_row.id))
            .delete()
            .exec(&mut db)
            .await?;

        Ok(app_row.id)
    }
    .await;
    drop(guard);
    let app_id = result?;

    let deployments_dir = state.deployment_dir(&app_id.to_string(), deployment_id);
    let staging = state.unique_tmp_path("deleted-deployment");
    std::fs::rename(&deployments_dir, &staging).map_err(|err| match err.kind() {
        ErrorKind::NotFound => AppError::DeploymentNotFound(deployment_id.to_string()),
        _ => AppError::Io(err),
    })?;
    std::fs::remove_dir_all(&staging)?;
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
    async fn test_state(label: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-storage-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(dir.join("apps")).expect("create apps dir");
        std::fs::create_dir_all(dir.join("tmp")).expect("create tmp dir");
        let db = oxde_db::connect(&dir)
            .await
            .expect("connect test accounts database");
        oxde_db::apply_migrations(&db)
            .await
            .expect("apply test accounts database migrations");
        AppState::new(
            dir,
            AppStateLimits {
                max_upload_bytes: 10_000,
                max_uncompressed_bytes: 10_000,
                base_domain: "localhost".to_string(),
                git_fetch_timeout_secs: 60,
                install_timeout_secs: 300,
                build_timeout_secs: 300,
                api_token_max_expiry_days: 30,
                enable_mcp: false,
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

    #[tokio::test]
    async fn create_list_get_app_round_trip() {
        let state = test_state("round-trip").await;

        let created = create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");
        assert_eq!(created.name, "blog");

        let fetched = get_app(&state, "blog").await.expect("get_app");
        assert_eq!(fetched.name, "blog");

        let listed = list_apps(&state).await.expect("list_apps");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "blog");
    }

    #[tokio::test]
    async fn duplicate_create_is_rejected_and_leaves_tmp_clean() {
        let state = test_state("duplicate-create").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("first create_app");

        let err = create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
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

    #[tokio::test]
    async fn delete_app_removes_it() {
        let state = test_state("delete-app").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");

        delete_app(&state, "blog").await.expect("delete_app");

        let err = get_app(&state, "blog")
            .await
            .expect_err("app should be gone");
        assert!(matches!(err, AppError::AppNotFound(_)));
    }

    #[tokio::test]
    async fn delete_app_cascades_to_its_deployments() {
        let state = test_state("delete-app-cascade").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");

        let zip = state.tmp_dir().join("v1.zip");
        std::fs::write(&zip, tiny_zip(b"v1")).expect("write zip");
        let deployment = create_deployment(&state, "blog", &zip, None, 2)
            .await
            .expect("create deployment");
        let deployment_uuid = uuid::Uuid::parse_str(&deployment.id).expect("parse deployment id");

        delete_app(&state, "blog").await.expect("delete_app");

        let mut db = state.db().clone();
        let remaining = oxde_db::models::Deployment::all()
            .filter(
                oxde_db::models::Deployment::fields()
                    .id()
                    .eq(deployment_uuid),
            )
            .exec(&mut db)
            .await
            .expect("query deployments");
        assert!(
            remaining.is_empty(),
            "deleting an app must delete its Deployment rows too, not just the App row"
        );
    }

    #[tokio::test]
    async fn delete_app_on_missing_app_is_not_found() {
        let state = test_state("delete-missing-app").await;
        let err = delete_app(&state, "nope")
            .await
            .expect_err("deleting a missing app must fail");
        assert!(matches!(err, AppError::AppNotFound(_)));
    }

    #[tokio::test]
    async fn deployment_lifecycle_activate_and_delete() {
        let state = test_state("deployment-lifecycle").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");

        let zip_v1 = state.tmp_dir().join("v1.zip");
        std::fs::write(&zip_v1, tiny_zip(b"v1")).expect("write v1 zip");
        let v1 = create_deployment(&state, "blog", &zip_v1, None, 2)
            .await
            .expect("create v1");

        let zip_v2 = state.tmp_dir().join("v2.zip");
        std::fs::write(&zip_v2, tiny_zip(b"v2")).expect("write v2 zip");
        let v2 = create_deployment(&state, "blog", &zip_v2, None, 2)
            .await
            .expect("create v2");

        // Uploading auto-activates, so the newest deployment should be live.
        assert_eq!(
            super::active_deployment_id(&state, "blog").await,
            Some(v2.id.clone())
        );

        let deployments = list_deployments(&state, "blog")
            .await
            .expect("list_deployments");
        assert_eq!(deployments.len(), 2);

        // Rolling back to v1 must actually flip the active pointer.
        activate_deployment(&state, "blog", &v1.id)
            .await
            .expect("activate v1");
        assert_eq!(
            super::active_deployment_id(&state, "blog").await,
            Some(v1.id.clone())
        );

        // The active deployment can never be deleted directly...
        let err = delete_deployment(&state, "blog", &v1.id)
            .await
            .expect_err("deleting active must fail");
        assert!(matches!(err, AppError::DeleteActiveDeployment));

        // ...but a non-active one can be, and it disappears from the listing.
        delete_deployment(&state, "blog", &v2.id)
            .await
            .expect("delete v2");
        let remaining = list_deployments(&state, "blog")
            .await
            .expect("list_deployments after delete");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, v1.id);
    }

    #[tokio::test]
    async fn sweep_tmp_dir_finishes_an_interrupted_delete() {
        let state = test_state("sweep-recovery").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");

        let zip = state.tmp_dir().join("v1.zip");
        std::fs::write(&zip, tiny_zip(b"v1")).expect("write zip");
        let deployment = create_deployment(&state, "blog", &zip, None, 2)
            .await
            .expect("create deployment");

        let app = get_app(&state, "blog").await.expect("get_app");

        // Simulate a crash between delete_deployment's DB-row delete and its
        // remove_dir_all: delete the row and do the rename ourselves, then stop.
        let mut db = state.db().clone();
        let deployment_uuid = uuid::Uuid::parse_str(&deployment.id).expect("parse deployment id");
        oxde_db::models::Deployment::all()
            .filter(
                oxde_db::models::Deployment::fields()
                    .id()
                    .eq(deployment_uuid),
            )
            .delete()
            .exec(&mut db)
            .await
            .expect("delete deployment row");
        let deployment_dir = state.deployment_dir(&app.id, &deployment.id);
        let orphan = state.tmp_dir().join("orphaned-partial-delete");
        std::fs::rename(&deployment_dir, &orphan).expect("simulate interrupted delete");

        assert!(
            list_deployments(&state, "blog")
                .await
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

    #[tokio::test]
    async fn sweep_orphaned_dirs_removes_dirs_with_no_matching_db_row() {
        let state = test_state("sweep-orphans").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");
        let app = get_app(&state, "blog").await.expect("get_app");

        let zip = state.tmp_dir().join("v1.zip");
        std::fs::write(&zip, tiny_zip(b"v1")).expect("write zip");
        let deployment = create_deployment(&state, "blog", &zip, None, 2)
            .await
            .expect("create deployment");

        // An orphaned deployment directory - no matching `Deployment` row -
        // under a real app.
        let orphan_deployment_id = uuid::Uuid::now_v7();
        std::fs::create_dir_all(state.deployment_dir(&app.id, &orphan_deployment_id.to_string()))
            .expect("create orphan deployment dir");

        // An orphaned app directory - no matching `App` row at all.
        let orphan_app_id = uuid::Uuid::now_v7();
        std::fs::create_dir_all(state.apps_dir().join(orphan_app_id.to_string()))
            .expect("create orphan app dir");

        super::sweep_orphaned_dirs(&state)
            .await
            .expect("sweep_orphaned_dirs");

        assert!(
            !state.apps_dir().join(orphan_app_id.to_string()).exists(),
            "orphaned app directory must be removed"
        );
        assert!(
            !state
                .deployment_dir(&app.id, &orphan_deployment_id.to_string())
                .exists(),
            "orphaned deployment directory must be removed"
        );
        assert!(
            state.apps_dir().join(&app.id).exists(),
            "app with a matching row must be untouched"
        );
        assert!(
            state.deployment_dir(&app.id, &deployment.id).exists(),
            "deployment with a matching row must be untouched"
        );
    }

    #[tokio::test]
    async fn create_git_deployment_on_upload_app_is_rejected() {
        let state = test_state("git-not-sourced").await;
        create_app(&state, "blog", AppSource::Upload, Vec::new(), None)
            .await
            .expect("create_app");

        let err = create_pending_git_deployment(&state, "blog")
            .await
            .expect_err("upload app must be rejected");
        assert!(matches!(err, AppError::NotGitSourced(_)));
    }

    #[tokio::test]
    async fn create_pending_git_deployment_is_rejected_while_one_is_already_pending() {
        let state = test_state("git-deploy-in-progress").await;
        create_app(
            &state,
            "site",
            AppSource::Git(GitSource {
                repo_url: "https://example.com/repo.git".to_string(),
                branch: "main".to_string(),
                mode: GitDeployMode::default(),
            }),
            Vec::new(),
            None,
        )
        .await
        .expect("create_app");

        let (first, _) = create_pending_git_deployment(&state, "site")
            .await
            .expect("first pending deploy");
        assert!(matches!(first.status, DeploymentStatus::Pending));

        let err = create_pending_git_deployment(&state, "site")
            .await
            .expect_err("a second deploy while one is pending must be rejected");
        assert!(matches!(err, AppError::DeploymentInProgress(_)));
    }
}
