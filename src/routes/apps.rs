use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{models, state::AppState};

pub async fn serve(state: &AppState, app_name: &str, request: Request<Body>) -> Response {
    if models::validate_slug(app_name).is_err() {
        return StatusCode::NOT_FOUND.into_response();
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
