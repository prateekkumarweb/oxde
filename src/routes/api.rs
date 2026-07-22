use std::{collections::HashMap, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use ts_rs::TS;

use crate::{
    accounts::AccountRole,
    auth::ApiUser,
    authz, containers,
    deployment_logs::{self, LogKind, LogTarget},
    error::{AppError, AppResult},
    models::{self, AppPermission, AppSource, Deployment, EnvVar, GitDeployMode, GitSource},
    state::AppState,
    storage,
};

pub fn router(state: &AppState) -> Router<AppState> {
    let app_scoped = Router::new()
        .route(
            "/",
            get(get_app).delete(delete_app).patch(update_app_env_vars),
        )
        .route("/permissions", post(update_app_permissions_endpoint))
        .route(
            "/deployments",
            post(create_deployment)
                .layer(DefaultBodyLimit::max(usize_from_u64(
                    state.max_upload_bytes(),
                )))
                .get(list_deployments),
        )
        .route("/deployments/{id}", delete(delete_deployment))
        .route("/deployments/{id}/activate", post(activate_deployment))
        .route("/deployments/git", post(create_git_deployment_endpoint))
        .route("/deployments/{id}/logs", get(deployment_logs))
        .route("/deployments/{id}/stats", get(deployment_stats))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            enforce_app_access,
        ));

    Router::new()
        .route("/apps", get(list_apps).post(create_app))
        .nest("/apps/{name}", app_scoped)
}

/// Gates every `/apps/{name}/...` route on the requesting user's per-app
/// permission: `Admin` always passes, a `Member` needs a matching
/// `AppPermission` at `Read` (GET/HEAD) or `Write` (everything else).
/// Applied once here rather than threading `CurrentUser` through every
/// handler below.
async fn enforce_app_access(
    State(state): State<AppState>,
    Path(params): Path<HashMap<String, String>>,
    current_user: ApiUser,
    method: Method,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    let app_name = params
        .get("name")
        .ok_or_else(|| AppError::AppNotFound(String::new()))?;
    let app = storage::get_app(&state, app_name).await?;
    let required = if method == Method::GET || method == Method::HEAD {
        models::PermissionLevel::Read
    } else {
        models::PermissionLevel::Write
    };
    authz::check_app_permission(&current_user, &app, required)?;
    Ok(next.run(request).await)
}

/// `App` plus the currently active deployment id, derived at read time from
/// the `App` row's `active_deployment_id` column.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct AppView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at: jiff::Timestamp,
    pub(crate) updated_at: jiff::Timestamp,
    pub(crate) active_deployment_id: Option<String>,
    pub(crate) source: AppSource,
    pub(crate) env_vars: Vec<EnvVar>,
    pub(crate) permissions: Vec<AppPermission>,
}

pub async fn app_view(state: &AppState, app: crate::models::App) -> AppView {
    let active_deployment_id = storage::active_deployment_id(state, &app.name).await;
    AppView {
        id: app.id,
        name: app.name,
        created_at: app.created_at,
        updated_at: app.updated_at,
        active_deployment_id,
        source: app.source,
        env_vars: app.env_vars,
        permissions: app.permissions,
    }
}

pub fn usize_from_u64(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// `Deployment` plus derived, request-time-only fields: whether it's the
/// active one, and (for run-mode deployments) the backing container's
/// current state.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct DeploymentView {
    #[serde(flatten)]
    pub(crate) deployment: Deployment,
    pub(crate) is_active: bool,
    pub(crate) container_status: Option<ContainerStatus>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ContainerStatus {
    Running,
    Stopped,
    Unknown,
}

async fn deployment_view(
    state: &AppState,
    active_id: Option<&str>,
    deployment: Deployment,
) -> DeploymentView {
    let is_active = active_id == Some(deployment.id.as_str());
    let container_status = match &deployment.container_name {
        Some(container_name) => Some(
            match containers::is_running(state.docker(), container_name).await {
                Ok(true) => ContainerStatus::Running,
                Ok(false) => ContainerStatus::Stopped,
                Err(_) => ContainerStatus::Unknown,
            },
        ),
        None => None,
    };
    DeploymentView {
        deployment,
        is_active,
        container_status,
    }
}

#[derive(Deserialize)]
struct CreateAppRequest {
    name: String,
    #[serde(default)]
    source: AppSource,
    #[serde(default)]
    env_vars: Vec<EnvVar>,
}

#[derive(Deserialize)]
struct UpdateAppEnvVarsRequest {
    env_vars: Vec<EnvVar>,
}

#[derive(Deserialize)]
struct UpdateAppPermissionsRequest {
    permissions: Vec<AppPermission>,
}

async fn list_apps(
    State(state): State<AppState>,
    current_user: ApiUser,
) -> AppResult<Json<Vec<AppView>>> {
    let apps = storage::list_apps(&state).await?;
    let mut views = Vec::with_capacity(apps.len());
    for app in apps {
        if matches!(current_user.role, AccountRole::Admin)
            || app.has_permission(&current_user.username, models::PermissionLevel::Read)
        {
            views.push(app_view(&state, app).await);
        }
    }
    Ok(Json(views))
}

async fn create_app(
    State(state): State<AppState>,
    current_user: ApiUser,
    Json(body): Json<CreateAppRequest>,
) -> AppResult<(StatusCode, Json<AppView>)> {
    // An `Admin` doesn't need an explicit grant (always full access), but a
    // `Member` would otherwise be immediately locked out of what they made.
    let creator =
        matches!(current_user.role, AccountRole::Member).then_some(current_user.username.as_str());
    let app = storage::create_app(&state, &body.name, body.source, body.env_vars, creator).await?;
    Ok((StatusCode::CREATED, Json(app_view(&state, app).await)))
}

async fn get_app(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<AppView>> {
    let app = storage::get_app(&state, &name).await?;
    Ok(Json(app_view(&state, app).await))
}

async fn update_app_env_vars(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<UpdateAppEnvVarsRequest>,
) -> AppResult<Json<AppView>> {
    let app = storage::update_app_env_vars(&state, &name, body.env_vars).await?;
    Ok(Json(app_view(&state, app).await))
}

/// Anyone with `Write` on the app (not just `Admin`) can manage who else
/// has access - project-level collaborator management, not restricted to
/// the account-level `Admin` role.
async fn update_app_permissions_endpoint(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<UpdateAppPermissionsRequest>,
) -> AppResult<Json<AppView>> {
    let app = storage::update_app_permissions(&state, &name, body.permissions).await?;
    Ok(Json(app_view(&state, app).await))
}

async fn delete_app(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<StatusCode> {
    delete_app_with_containers(&state, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stops every deployment's container (normally just the active one, but
/// checked for all of them rather than assumed) before removing the app, so
/// deleting a run-mode app never leaves an orphaned container behind.
pub async fn delete_app_with_containers(state: &AppState, app_name: &str) -> AppResult<()> {
    for deployment in storage::list_deployments(state, app_name).await? {
        if let Some(container_name) = &deployment.container_name {
            containers::stop_and_remove(state.docker(), container_name).await?;
        }
    }
    storage::delete_app(state, app_name).await
}

async fn create_deployment(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<Deployment>)> {
    let deployment = upload_deployment(&state, &app_name, &mut multipart).await?;
    Ok((StatusCode::CREATED, Json(deployment)))
}

async fn create_git_deployment_endpoint(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
) -> AppResult<(StatusCode, Json<Deployment>)> {
    let deployment = start_git_deployment(&state, &app_name).await?;
    Ok((StatusCode::ACCEPTED, Json(deployment)))
}

/// Creates the `Pending` record synchronously, then hands the actual clone/
/// install/activate work to a detached task - the caller gets the id back
/// immediately and can attach to `.../logs` while it runs.
pub async fn start_git_deployment(state: &AppState, app_name: &str) -> AppResult<Deployment> {
    let (deployment, git_source) = storage::create_pending_git_deployment(state, app_name).await?;

    tokio::spawn(run_git_deployment(
        state.clone(),
        app_name.to_string(),
        deployment.id.clone(),
        git_source,
    ));

    Ok(deployment)
}

async fn run_git_deployment(
    state: AppState,
    app_name: String,
    deployment_id: String,
    git_source: GitSource,
) {
    if let Err(err) = execute_git_deployment(&state, &app_name, &deployment_id, &git_source).await {
        tracing::error!(error = %err, app_name, deployment_id, "git deployment failed");
        if let Err(fail_err) =
            storage::fail_git_deployment(&state, &app_name, &deployment_id, &err.to_string()).await
        {
            tracing::error!(error = %fail_err, app_name, deployment_id, "failed to record git deployment failure");
        }
    }
}

/// Status stays `Pending` through the whole clone/build/install/activate
/// sequence (only flipping to `Ready` at the very end) so install/build
/// command logs stay attached to this deployment for their whole run.
async fn execute_git_deployment(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
    git_source: &GitSource,
) -> AppResult<()> {
    let app = storage::get_app(state, app_name).await?;
    let staging = state.unique_tmp_path("git-deployment");
    let timeout = Duration::from_secs(state.git_fetch_timeout_secs());

    let clone_target = LogTarget {
        path: state.deployment_log_path(&app.id, deployment_id, LogKind::Clone),
        deployment_id: deployment_id.to_string(),
        kind: LogKind::Clone,
        registry: state.log_registry().clone(),
    };
    let clone_result = {
        let blocking_staging = staging.clone();
        let blocking_git_source = git_source.clone();
        tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || {
                storage::clone_repo(&blocking_staging, &blocking_git_source, Some(clone_target))
            }),
        )
        .await
    };
    let (checkout_dir, commit_sha) = match clone_result {
        Ok(Ok(Ok(result))) => result,
        Ok(Ok(Err(err))) => {
            std::fs::remove_dir_all(&staging).ok();
            return Err(err);
        }
        Ok(Err(join_err)) => {
            std::fs::remove_dir_all(&staging).ok();
            return Err(AppError::Io(std::io::Error::other(join_err.to_string())));
        }
        Err(_) => {
            std::fs::remove_dir_all(&staging).ok();
            return Err(AppError::Git("timed out waiting for git fetch".to_string()));
        }
    };

    if let GitDeployMode::Build(build) = &git_source.mode {
        let container_name = containers::container_name(app_name, deployment_id);
        let build_timeout = Duration::from_secs(state.build_timeout_secs());
        let build_target = LogTarget {
            path: state.deployment_log_path(&app.id, deployment_id, LogKind::Build),
            deployment_id: deployment_id.to_string(),
            kind: LogKind::Build,
            registry: state.log_registry().clone(),
        };
        if let Err(err) = containers::run_build_command(
            state.docker(),
            &container_name,
            containers::CommandExec {
                checkout_dir: &checkout_dir,
                image: build.image.image_tag(),
                command: &build.command,
                env_vars: &app.env_vars,
                timeout: build_timeout,
            },
            Some(build_target),
        )
        .await
        {
            std::fs::remove_dir_all(&staging).ok();
            return Err(err);
        }
    }

    if let Err(err) = storage::finish_git_deployment(
        state,
        &staging,
        &checkout_dir,
        app_name,
        deployment_id,
        git_source,
        commit_sha,
    )
    .await
    {
        std::fs::remove_dir_all(&staging).ok();
        return Err(err);
    }

    activate_with_containers(state, app_name, deployment_id).await?;
    storage::mark_git_deployment_ready(state, app_name, deployment_id).await
}

/// Activates a deployment. For a run-mode app this starts the new
/// container *before* touching anything else: if that fails, the
/// previously-active container is untouched and the app keeps serving from
/// it. Only once the new container is confirmed up does this stop the old
/// container and flip `active` - so a bad redeploy degrades to "the old
/// deployment keeps serving," never to "nothing is serving."
pub async fn activate_with_containers(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<()> {
    let app = storage::get_app(state, app_name).await?;

    if let Some(run_config) = app.run_config() {
        let deployment = storage::get_deployment(state, app_name, deployment_id).await?;
        let container_name = deployment.container_name.ok_or_else(|| {
            AppError::ContainerStartFailed("run-mode deployment has no container_name".to_string())
        })?;
        let checkout_dir = state.deployment_files_dir(&app.id, deployment_id);
        let install_target = run_config.install_command.is_some().then(|| LogTarget {
            path: state.deployment_log_path(&app.id, deployment_id, LogKind::Install),
            deployment_id: deployment_id.to_string(),
            kind: LogKind::Install,
            registry: state.log_registry().clone(),
        });
        containers::start(
            state.docker(),
            &container_name,
            &checkout_dir,
            run_config,
            &app.env_vars,
            Duration::from_secs(state.install_timeout_secs()),
            install_target,
        )
        .await?;

        containers::spawn_run_log_pump(
            state.docker(),
            &container_name,
            LogTarget {
                path: state.deployment_log_path(&app.id, deployment_id, LogKind::Run),
                deployment_id: deployment_id.to_string(),
                kind: LogKind::Run,
                registry: state.log_registry().clone(),
            },
        );

        if let Some(previous_id) = storage::active_deployment_id(state, app_name).await
            && previous_id != deployment_id
            && let Ok(previous) = storage::get_deployment(state, app_name, &previous_id).await
            && let Some(previous_container) = &previous.container_name
            && let Err(err) = containers::stop_and_remove(state.docker(), previous_container).await
        {
            tracing::warn!(error = %err, app_name, "failed to stop previous container during activate");
        }
    }

    storage::activate_deployment(state, app_name, deployment_id).await
}

/// Deleting a run-mode deployment must leave no container behind for it -
/// checked explicitly even though the one-container-per-app model means
/// this should only ever be true for the (already-blocked) active case.
/// The container is stopped only *after* `storage::delete_deployment`
/// succeeds, so its own "can't delete the active deployment" guard gets to
/// run before anything live is touched.
pub async fn delete_deployment_with_containers(
    state: &AppState,
    app_name: &str,
    deployment_id: &str,
) -> AppResult<()> {
    let container_name = storage::get_deployment(state, app_name, deployment_id)
        .await
        .ok()
        .and_then(|deployment| deployment.container_name);

    storage::delete_deployment(state, app_name, deployment_id).await?;

    if let Some(container_name) = container_name {
        containers::stop_and_remove(state.docker(), &container_name).await?;
    }
    Ok(())
}

pub async fn upload_deployment(
    state: &AppState,
    app_name: &str,
    multipart: &mut Multipart,
) -> AppResult<Deployment> {
    let zip_path = state.unique_tmp_path("upload").with_extension("zip");
    let upload = stream_upload_to_disk(state, &zip_path, multipart).await;

    let (original_filename, upload_size_bytes) = match upload {
        Ok(fields) => fields,
        Err(err) => {
            tokio::fs::remove_file(&zip_path).await.ok();
            return Err(err);
        }
    };

    let result = storage::create_deployment(
        state,
        app_name,
        &zip_path,
        original_filename,
        upload_size_bytes,
    )
    .await;

    tokio::fs::remove_file(&zip_path).await.ok();
    result
}

async fn list_deployments(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
) -> AppResult<Json<Vec<DeploymentView>>> {
    let active_id = storage::active_deployment_id(&state, &app_name).await;
    let mut views = Vec::new();
    for deployment in storage::list_deployments(&state, &app_name).await? {
        views.push(deployment_view(&state, active_id.as_deref(), deployment).await);
    }
    Ok(Json(views))
}

async fn activate_deployment(
    State(state): State<AppState>,
    Path((app_name, id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    activate_with_containers(&state, &app_name, &id).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    follow: bool,
    /// Explicit phase to serve. Omitted means auto-pick.
    phase: Option<LogKind>,
}

/// Serves `phase` if given, else whichever phase is active or furthest
/// along. `follow=true` only live-tails if a pump is actively producing
/// that phase; otherwise it's the same as `follow=false`.
async fn deployment_logs(
    State(state): State<AppState>,
    Path((app_name, id)): Path<(String, String)>,
    Query(query): Query<LogsQuery>,
) -> AppResult<impl IntoResponse> {
    storage::get_deployment(&state, &app_name, &id).await?;
    let app = storage::get_app(&state, &app_name).await?;
    let dir = state.deployment_dir(&app.id, &id);
    let active = state.log_registry().active(&id);

    let kind = match query.phase {
        Some(kind) => kind,
        None => match active {
            Some((kind, _)) => kind,
            None => deployment_logs::resolve_terminal_phase(&dir)
                .ok_or_else(|| AppError::NoContainer(id.clone()))?,
        },
    };
    let live_rx = active
        .filter(|(active_kind, _)| *active_kind == kind)
        .map(|(_, rx)| rx)
        .filter(|_| query.follow);

    let backlog = deployment_logs::read_backlog(&dir, kind)?;

    let content_type = if query.follow {
        "text/event-stream"
    } else {
        "text/plain; charset=utf-8"
    };
    let body = match live_rx {
        Some(rx) => Body::from_stream(
            futures_util::stream::once(async move { Ok::<_, AppError>(Bytes::from(backlog)) })
                .chain(deployment_logs::live_tail(rx)),
        ),
        None => Body::from(backlog),
    };
    Ok(([(header::CONTENT_TYPE, content_type)], body))
}

async fn deployment_stats(
    State(state): State<AppState>,
    Path((app_name, id)): Path<(String, String)>,
) -> AppResult<Json<Option<containers::ContainerStats>>> {
    let deployment = storage::get_deployment(&state, &app_name, &id).await?;
    let Some(container_name) = deployment.container_name else {
        return Ok(Json(None));
    };
    if !containers::is_running(state.docker(), &container_name).await? {
        return Ok(Json(None));
    }
    let container_stats = containers::stats(state.docker(), &container_name).await?;
    Ok(Json(Some(container_stats)))
}

async fn delete_deployment(
    State(state): State<AppState>,
    Path((app_name, id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    delete_deployment_with_containers(&state, &app_name, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stream_upload_to_disk(
    state: &AppState,
    zip_path: &std::path::Path,
    multipart: &mut Multipart,
) -> AppResult<(Option<String>, u64)> {
    while let Some(mut field) = multipart.next_field().await? {
        if field.name() != Some("file") {
            continue;
        }

        let original_filename = field.file_name().map(str::to_string);
        let mut out = tokio::fs::File::create(zip_path).await?;
        let mut total: u64 = 0;

        while let Some(chunk) = field.chunk().await? {
            total += chunk.len() as u64;
            if total > state.max_upload_bytes() {
                return Err(AppError::TooLarge);
            }
            out.write_all(&chunk).await?;
        }

        return Ok((original_filename, total));
    }

    Err(AppError::MissingUploadFile)
}
