use axum::{
    Router, middleware,
    response::{IntoResponse, Redirect},
    routing::get,
};
use tower_http::{trace::TraceLayer, validate_request::ValidateRequestHeaderLayer};

use crate::{auth::BasicAuth, dashboard_assets, state::AppState};

pub mod api;
pub mod apps;
mod host_routing;

pub fn build_router(state: AppState, admin_username: &str, admin_password: &str) -> Router {
    let api = Router::new()
        .nest("/api", api::router(state.max_upload_bytes()))
        .route_layer(ValidateRequestHeaderLayer::custom(BasicAuth::new(
            admin_username,
            admin_password,
        )));

    Router::new()
        .merge(api)
        .route(
            "/",
            get(|| async { Redirect::to("/dashboard").into_response() }),
        )
        .route("/dashboard", get(dashboard_assets::serve))
        .route("/dashboard/{*path}", get(dashboard_assets::serve))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state,
            host_routing::dispatch_by_host,
        ))
        .layer(TraceLayer::new_for_http())
}
