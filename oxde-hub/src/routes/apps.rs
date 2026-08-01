use std::path::PathBuf;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use oxde_models::RunConfig;
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{containers, reverse_proxy, state::AppState, storage};

enum ServeTarget {
    NotFound,
    Static(PathBuf),
    Run {
        container_name: String,
        run_config: RunConfig,
        host_id: i64,
    },
}

pub async fn serve(state: &AppState, app_name: &str, request: Request<Body>) -> Response {
    if oxde_models::validate_slug(app_name).is_err() {
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
            host_id,
        } => {
            serve_run_mode(
                state,
                app_name,
                &container_name,
                &run_config,
                host_id,
                request,
            )
            .await
        }
    }
}

async fn resolve_serve_target(state: &AppState, app_name: &str) -> ServeTarget {
    let Ok(app) = storage::get_app(state, app_name).await else {
        return ServeTarget::NotFound;
    };

    let Some(deployment_id) = storage::active_deployment_id(state, &app.id).await else {
        return ServeTarget::NotFound;
    };
    let Ok(deployment) = storage::get_deployment(state, &app.id, &deployment_id).await else {
        return ServeTarget::NotFound;
    };

    if let Some(container_name) = deployment.container_name {
        let Some(run_config) = app.run_config().cloned() else {
            return ServeTarget::NotFound;
        };
        return ServeTarget::Run {
            container_name,
            run_config,
            host_id: app.host_id,
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
    host_id: i64,
    request: Request<Body>,
) -> Response {
    let cached = state.cached_container_ip(container_name);
    let from_cache = cached.is_some();
    let ip = match cached {
        Some(ip) => Some(ip),
        None => resolve_and_cache_container_ip(state, app_name, container_name, host_id).await,
    };

    let Some(ip) = ip else {
        return StatusCode::BAD_GATEWAY.into_response();
    };

    match reverse_proxy::proxy(
        state.proxy_client(),
        &ip,
        run_config.container_port,
        request,
    )
    .await
    {
        Ok(response) => response,
        // Cached IP is stale (container recreated on redeploy) - evict so
        // the next request re-resolves instead of waiting out the TTL.
        Err(()) if from_cache => {
            state.evict_container_ip(container_name);
            StatusCode::BAD_GATEWAY.into_response()
        }
        Err(()) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn resolve_and_cache_container_ip(
    state: &AppState,
    app_name: &str,
    container_name: &str,
    host_id: i64,
) -> Option<String> {
    match containers::container_ip(&state.agent_link_for(host_id), container_name).await {
        Ok(Some(ip)) => {
            state.cache_container_ip(container_name, ip.clone());
            Some(ip)
        }
        Ok(None) => None,
        Err(err) => {
            tracing::error!(error = %err, app = app_name, "container lookup failed");
            None
        }
    }
}
