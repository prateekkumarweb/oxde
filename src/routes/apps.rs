use std::path::PathBuf;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{containers, models, models::RunConfig, reverse_proxy, state::AppState, storage};

enum ServeTarget {
    NotFound,
    Static(PathBuf),
    Run {
        container_name: String,
        run_config: RunConfig,
    },
}

pub async fn serve(state: &AppState, app_name: &str, request: Request<Body>) -> Response {
    if models::validate_slug(app_name).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let target = resolve_serve_target(state, app_name).await;

    match target {
        ServeTarget::NotFound => StatusCode::NOT_FOUND.into_response(),
        ServeTarget::Static(files_dir) => match ServeDir::new(files_dir).oneshot(request).await {
            Ok(response) => response.into_response(),
            Err(err) => {
                tracing::error!(error = %err, app = app_name, "static file serving failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
        ServeTarget::Run {
            container_name,
            run_config,
        } => serve_run_mode(state, app_name, &container_name, &run_config, request).await,
    }
}

async fn resolve_serve_target(state: &AppState, app_name: &str) -> ServeTarget {
    let Some(deployment_id) = storage::active_deployment_id(state, app_name).await else {
        return ServeTarget::NotFound;
    };
    let Ok(deployment) = storage::get_deployment(state, app_name, &deployment_id).await else {
        return ServeTarget::NotFound;
    };

    let Ok(app) = storage::get_app(state, app_name).await else {
        return ServeTarget::NotFound;
    };

    if let Some(container_name) = deployment.container_name {
        let Some(run_config) = app.run_config().cloned() else {
            return ServeTarget::NotFound;
        };
        return ServeTarget::Run {
            container_name,
            run_config,
        };
    }

    let active_files_dir = state.deployment_files_dir(&app.id, &deployment_id);
    if !active_files_dir.is_dir() {
        return ServeTarget::NotFound;
    }
    ServeTarget::Static(active_files_dir)
}

async fn serve_run_mode(
    state: &AppState,
    app_name: &str,
    container_name: &str,
    run_config: &RunConfig,
    request: Request<Body>,
) -> Response {
    let ip = match state.cached_container_ip(container_name) {
        Some(ip) => Some(ip),
        None => match containers::container_ip(state.docker(), container_name).await {
            Ok(Some(ip)) => {
                state.cache_container_ip(container_name, ip.clone());
                Some(ip)
            }
            Ok(None) => None,
            Err(err) => {
                tracing::error!(error = %err, app = app_name, "container lookup failed");
                None
            }
        },
    };

    match ip {
        Some(ip) => {
            reverse_proxy::proxy(
                state.proxy_client(),
                &ip,
                run_config.container_port,
                request,
            )
            .await
        }
        None => StatusCode::BAD_GATEWAY.into_response(),
    }
}
