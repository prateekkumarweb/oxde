use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{
    containers,
    models::{self, AppSource},
    reverse_proxy,
    state::AppState,
    storage,
};

pub async fn serve(state: &AppState, app_name: &str, request: Request<Body>) -> Response {
    if models::validate_slug(app_name).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(deployment_id) = storage::active_deployment_id(state, app_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(deployment) = storage::get_deployment(state, app_name, &deployment_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if let Some(container_name) = &deployment.container_name {
        return serve_run_mode(state, app_name, container_name, request).await;
    }

    let active_files_dir = state.apps_dir().join(app_name).join("active").join("files");
    if !active_files_dir.is_dir() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match ServeDir::new(active_files_dir).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(err) => {
            tracing::error!(error = %err, app = app_name, "static file serving failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn serve_run_mode(
    state: &AppState,
    app_name: &str,
    container_name: &str,
    request: Request<Body>,
) -> Response {
    let Ok(app) = storage::get_app(state, app_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let AppSource::Git(git_source) = app.source else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let Some(run_config) = git_source.run else {
        return StatusCode::BAD_GATEWAY.into_response();
    };

    match containers::container_ip(state.docker(), container_name).await {
        Ok(Some(ip)) => {
            reverse_proxy::proxy(
                state.proxy_client(),
                &ip,
                run_config.container_port,
                request,
            )
            .await
        }
        Ok(None) => StatusCode::BAD_GATEWAY.into_response(),
        Err(err) => {
            tracing::error!(error = %err, app = app_name, "container lookup failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}
