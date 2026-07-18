use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Multipart, Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;

use super::api::{
    AppView, activate_with_containers, app_view, delete_app_with_containers,
    delete_deployment_with_containers, deploy_from_git, upload_deployment, usize_from_u64,
};
use crate::{
    containers,
    error::{AppError, AppResult},
    models::{AppSource, GitSource, RunConfig, RunImage},
    state::AppState,
    storage,
};

pub fn router(max_upload_bytes: u64) -> Router<AppState> {
    Router::new()
        .route("/", get(apps_list_page))
        .route("/apps", post(create_app_action))
        .route("/apps/{name}", get(app_detail_page))
        .route("/apps/{name}/delete", post(delete_app_action))
        .route(
            "/apps/{name}/deployments",
            post(upload_deployment_action)
                .layer(DefaultBodyLimit::max(usize_from_u64(max_upload_bytes))),
        )
        .route("/apps/{name}/deployments/git", post(deploy_git_action))
        .route(
            "/apps/{name}/deployments/{id}/activate",
            post(activate_deployment_action),
        )
        .route(
            "/apps/{name}/deployments/{id}/delete",
            post(delete_deployment_action),
        )
}

pub(super) fn render(template: impl Template) -> AppResult<Response> {
    let html = template.render()?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response())
}

#[derive(Template)]
#[template(path = "apps_list.html")]
struct AppsListTemplate {
    apps: Vec<AppView>,
}

async fn apps_list_page(State(state): State<AppState>) -> AppResult<Response> {
    let apps = storage::list_apps(&state)?
        .into_iter()
        .map(|app| app_view(&state, app))
        .collect();
    render(AppsListTemplate { apps })
}

/// `axum::Form` (urlencoded) doesn't map onto a tagged enum the way JSON
/// does, so this stays flat and gets assembled into an `AppSource` by hand.
#[derive(Deserialize)]
struct CreateAppForm {
    name: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    repo_url: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    publish_dir: String,
    #[serde(default)]
    run_enabled: String,
    #[serde(default)]
    run_image: String,
    #[serde(default)]
    install_command: String,
    #[serde(default)]
    start_command: String,
    #[serde(default)]
    container_port: String,
}

fn run_config_from_form(form: &CreateAppForm) -> AppResult<Option<RunConfig>> {
    if form.run_enabled != "on" {
        return Ok(None);
    }
    let image = match form.run_image.as_str() {
        "python314" => RunImage::Python314,
        _ => RunImage::Node24,
    };
    let invalid_port = || AppError::InvalidRunConfig("container port must be 1-65535".to_string());
    let container_port = form
        .container_port
        .trim()
        .parse::<u16>()
        .map_err(|_| invalid_port())?;
    if container_port == 0 {
        return Err(invalid_port());
    }
    if form.start_command.trim().is_empty() {
        return Err(AppError::InvalidRunConfig(
            "start command is required in run mode".to_string(),
        ));
    }
    Ok(Some(RunConfig {
        image,
        install_command: (!form.install_command.trim().is_empty())
            .then(|| form.install_command.trim().to_string()),
        start_command: form.start_command.trim().to_string(),
        container_port,
    }))
}

async fn create_app_action(
    State(state): State<AppState>,
    Form(form): Form<CreateAppForm>,
) -> AppResult<Redirect> {
    let source = if form.source == "git" {
        let run = run_config_from_form(&form)?;
        let branch = if form.branch.trim().is_empty() {
            "main".to_string()
        } else {
            form.branch.clone()
        };
        AppSource::Git(GitSource {
            repo_url: form.repo_url.clone(),
            branch,
            publish_dir: (!form.publish_dir.is_empty()).then_some(form.publish_dir.clone()),
            run,
        })
    } else {
        AppSource::Upload
    };
    storage::create_app(&state, &form.name, source)?;
    Ok(Redirect::to("/dashboard"))
}

struct DeploymentRow {
    id: String,
    created_at: jiff::Timestamp,
    upload_size_bytes: u64,
    is_active: bool,
    commit_sha: Option<String>,
}

#[derive(Template)]
#[template(path = "app_detail.html")]
struct AppDetailTemplate {
    app_name: String,
    app_host: String,
    git_source: Option<GitSource>,
    run_config: Option<RunConfig>,
    container_status: Option<String>,
    deployments: Vec<DeploymentRow>,
}

async fn container_status_label(state: &AppState, app_name: &str, deployment_id: &str) -> String {
    let Ok(deployment) = storage::get_deployment(state, app_name, deployment_id) else {
        return "unknown".to_string();
    };
    let Some(container_name) = &deployment.container_name else {
        return "not started".to_string();
    };
    match containers::is_running(state.docker(), container_name).await {
        Ok(true) => "running".to_string(),
        Ok(false) => "stopped".to_string(),
        Err(_) => "unknown".to_string(),
    }
}

async fn app_detail_page(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let app = storage::get_app(&state, &app_name)?;
    let run_config = app.run_config().cloned();
    let git_source = match app.source {
        AppSource::Git(git_source) => Some(git_source),
        AppSource::Upload => None,
    };
    let active_id = storage::active_deployment_id(&state, &app_name);
    let container_status = match (&run_config, &active_id) {
        (Some(_), Some(id)) => Some(container_status_label(&state, &app_name, id).await),
        _ => None,
    };
    let deployments = storage::list_deployments(&state, &app_name)?
        .into_iter()
        .map(|d| DeploymentRow {
            is_active: active_id.as_deref() == Some(d.id.as_str()),
            id: d.id,
            created_at: d.created_at,
            upload_size_bytes: d.upload_size_bytes,
            commit_sha: d.git.map(|git| git.commit_sha),
        })
        .collect();
    // Dashboard's own Host header already carries the right port for the link.
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| state.base_domain());
    let app_host = format!("{app_name}.{host}");
    render(AppDetailTemplate {
        app_name,
        app_host,
        git_source,
        run_config,
        container_status,
        deployments,
    })
}

async fn delete_app_action(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
) -> AppResult<Redirect> {
    delete_app_with_containers(&state, &app_name).await?;
    Ok(Redirect::to("/dashboard"))
}

async fn upload_deployment_action(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Redirect> {
    upload_deployment(&state, &app_name, &mut multipart).await?;
    Ok(Redirect::to(&format!("/dashboard/apps/{app_name}")))
}

async fn deploy_git_action(
    State(state): State<AppState>,
    Path(app_name): Path<String>,
) -> AppResult<Redirect> {
    deploy_from_git(&state, &app_name).await?;
    Ok(Redirect::to(&format!("/dashboard/apps/{app_name}")))
}

async fn activate_deployment_action(
    State(state): State<AppState>,
    Path((app_name, id)): Path<(String, String)>,
) -> AppResult<Redirect> {
    activate_with_containers(&state, &app_name, &id).await?;
    Ok(Redirect::to(&format!("/dashboard/apps/{app_name}")))
}

async fn delete_deployment_action(
    State(state): State<AppState>,
    Path((app_name, id)): Path<(String, String)>,
) -> AppResult<Redirect> {
    delete_deployment_with_containers(&state, &app_name, &id).await?;
    Ok(Redirect::to(&format!("/dashboard/apps/{app_name}")))
}
