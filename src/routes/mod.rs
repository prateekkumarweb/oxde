use askama::Template;
use axum::{Router, middleware, response::Response, routing::get};
use tower_http::{
    services::ServeDir, trace::TraceLayer, validate_request::ValidateRequestHeaderLayer,
};

use crate::{auth::BasicAuth, error::AppResult, state::AppState};

pub mod api;
pub mod apps;
pub mod dashboard;
mod host_routing;

pub fn build_router(state: AppState, admin_username: &str, admin_password: &str) -> Router {
    let protected = Router::new()
        .nest("/dashboard", dashboard::router(state.max_upload_bytes()))
        .nest("/api", api::router(state.max_upload_bytes()))
        .route_layer(ValidateRequestHeaderLayer::custom(BasicAuth::new(
            admin_username,
            admin_password,
        )));

    Router::new()
        .merge(protected)
        .route("/", get(index))
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state,
            host_routing::dispatch_by_host,
        ))
        .layer(TraceLayer::new_for_http())
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

async fn index() -> AppResult<Response> {
    dashboard::render(IndexTemplate)
}
