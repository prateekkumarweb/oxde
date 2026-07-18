use std::time::Duration;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{
    containers,
    error::{AppError, AppResult},
    models::{AppSource, Deployment},
    state::AppState,
    storage,
};

pub fn router(max_upload_bytes: u64) -> Router<AppState> {
    Router::new()
        .route("/apps", get(list_apps).post(create_app))
        .route("/apps/{name}", get(get_app).delete(delete_app))
        .route(
            "/apps/{name}/deployments",
            post(create_deployment)
                .layer(DefaultBodyLimit::max(usize_from_u64(max_upload_bytes)))
                .get(list_deployments),
        )
        .route("/apps/{name}/deployments/{id}", delete(delete_deployment))
        .route(
            "/apps/{name}/deployments/{id}/activate",
            post(activate_deployment),
        )
        .route(
            "/apps/{name}/deployments/git",
            post(create_git_deployment_endpoint),
        )
}

/// `App` plus the currently active deployment id, derived at read time from
/// the `active` symlink rather than stored in `app.json`.
#[derive(Serialize)]
pub struct AppView {
    pub(crate) name: String,
    pub(crate) created_at: jiff::Timestamp,
    pub(crate) active_deployment_id: Option<String>,
    pub(crate) source: AppSource,
}

pub fn app_view(state: &AppState, app: crate::models::App) -> AppView {
    let active_deployment_id = storage::active_deployment_id(state, &app.name);
    AppView {
        name: app.name,
        created_at: app.created_at,
        active_deployment_id,
        source: app.source,
    }
}

pub fn usize_from_u64(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// `Deployment` plus derived, request-time-only fields: whether it's the
/// active one, and (for run-mode deployments) the backing container's
/// current state.
#[derive(Serialize)]
pub struct DeploymentView {
    #[serde(flatten)]
    pub(crate) deployment: Deployment,
    pub(crate) is_active: bool,
    pub(crate) container_status: Option<ContainerStatus>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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
}

async fn list_apps(State(state): State<AppState>) -> AppResult<Json<Vec<AppView>>> {
    let views = storage::list_apps(&state)?
        .into_iter()
        .map(|app| app_view(&state, app))
        .collect();
    Ok(Json(views))
}

async fn create_app(
    State(state): State<AppState>,
    Json(body): Json<CreateAppRequest>,
) -> AppResult<(StatusCode, Json<AppView>)> {
    let app = storage::create_app(&state, &body.name, body.source)?;
    Ok((StatusCode::CREATED, Json(app_view(&state, app))))
}

async fn get_app(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<AppView>> {
    let app = storage::get_app(&state, &name)?;
    Ok(Json(app_view(&state, app)))
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
    for deployment in storage::list_deployments(state, app_name)? {
        if let Some(container_name) = &deployment.container_name {
            containers::stop_and_remove(state.docker(), container_name).await?;
        }
    }
    storage::delete_app(state, app_name)
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
    let deployment = deploy_from_git(&state, &app_name).await?;
    Ok((StatusCode::CREATED, Json(deployment)))
}

/// Runs the (blocking, network-bound) git fetch on a blocking thread, bounded
/// by `git_fetch_timeout_secs` so a stalled fetch can't hang the request
/// forever, then activates the result. Shared by the API and dashboard
/// routes.
pub async fn deploy_from_git(state: &AppState, app_name: &str) -> AppResult<Deployment> {
    let blocking_state = state.clone();
    let blocking_app_name = app_name.to_string();
    let timeout = Duration::from_secs(state.git_fetch_timeout_secs());

    let join_handle = tokio::task::spawn_blocking(move || {
        storage::create_git_deployment(&blocking_state, &blocking_app_name)
    });

    let deployment = match tokio::time::timeout(timeout, join_handle).await {
        Ok(Ok(result)) => result?,
        Ok(Err(err)) => return Err(AppError::Io(std::io::Error::other(err.to_string()))),
        Err(_) => return Err(AppError::Git("timed out waiting for git fetch".to_string())),
    };

    activate_with_containers(state, app_name, &deployment.id).await?;
    Ok(deployment)
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
    let app = storage::get_app(state, app_name)?;

    if let Some(run_config) = app.run_config() {
        let deployment = storage::get_deployment(state, app_name, deployment_id)?;
        let container_name = deployment.container_name.ok_or_else(|| {
            AppError::ContainerStartFailed("run-mode deployment has no container_name".to_string())
        })?;
        let checkout_dir = state.deployment_files_dir(app_name, deployment_id);
        containers::start(state.docker(), &container_name, &checkout_dir, run_config).await?;

        if let Some(previous_id) = storage::active_deployment_id(state, app_name)
            && previous_id != deployment_id
            && let Ok(previous) = storage::get_deployment(state, app_name, &previous_id)
            && let Some(previous_container) = &previous.container_name
            && let Err(err) = containers::stop_and_remove(state.docker(), previous_container).await
        {
            tracing::warn!(error = %err, app_name, "failed to stop previous container during activate");
        }
    }

    storage::activate_deployment(state, app_name, deployment_id)
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
        .ok()
        .and_then(|deployment| deployment.container_name);

    storage::delete_deployment(state, app_name, deployment_id)?;

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

    // Extraction is CPU/disk-bound and the `zip` crate's API is synchronous,
    // so it runs on a blocking thread rather than tying up the async runtime.
    let blocking_state = state.clone();
    let blocking_zip_path = zip_path.clone();
    let app_name = app_name.to_string();
    let result = tokio::task::spawn_blocking(move || {
        storage::create_deployment(
            &blocking_state,
            &app_name,
            &blocking_zip_path,
            original_filename,
            upload_size_bytes,
        )
    })
    .await
    .map_err(|err| AppError::Io(std::io::Error::other(err.to_string())))?;

    tokio::fs::remove_file(&zip_path).await.ok();
    result
}

async fn list_deployments(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
) -> AppResult<Json<Vec<DeploymentView>>> {
    let active_id = storage::active_deployment_id(&state, &app_name);
    let mut views = Vec::new();
    for deployment in storage::list_deployments(&state, &app_name)? {
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
