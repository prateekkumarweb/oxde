use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{models, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{app_name}", get(serve_app_root))
        .route("/{app_name}/", get(serve_app_root))
        .route("/{app_name}/{*path}", get(serve_app_path))
}

async fn serve_app_root(
    state: State<AppState>,
    Path(app_name): Path<String>,
    request: Request<Body>,
) -> Response {
    serve(state, &app_name, String::new(), request).await
}

async fn serve_app_path(
    state: State<AppState>,
    Path((app_name, path)): Path<(String, String)>,
    request: Request<Body>,
) -> Response {
    serve(state, &app_name, path, request).await
}

async fn serve(
    State(state): State<AppState>,
    app_name: &str,
    path: String,
    mut request: Request<Body>,
) -> Response {
    if models::validate_slug(app_name).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let active_files_dir = state.apps_dir().join(app_name).join("active").join("files");
    if !active_files_dir.is_dir() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // ServeDir resolves files using the request URI's path directly against
    // its own root, so rewrite the URI to just the part after
    // `/apps/<app_name>/` before handing the request off to it.
    let Ok(uri) = format!("/{path}").parse::<Uri>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    *request.uri_mut() = uri;

    match ServeDir::new(active_files_dir).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(err) => {
            tracing::error!(error = %err, app = app_name, "static file serving failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
