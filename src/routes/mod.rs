use axum::{
    Router, middleware,
    response::{IntoResponse, Redirect},
    routing::get,
};
use tower_http::trace::TraceLayer;

use crate::{auth::CurrentUser, dashboard_assets, state::AppState};

pub mod api;
pub mod apps;
mod auth_routes;
mod host_routing;
mod users;

pub fn build_router(state: AppState) -> Router {
    let public_api = Router::new().nest("/api", auth_routes::public_router());

    let protected_api = Router::new()
        .nest("/api", api::router(&state))
        .nest("/api", auth_routes::protected_router())
        .nest("/api/users", users::router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_authenticated,
        ));

    Router::new()
        .merge(public_api)
        .merge(protected_api)
        .route(
            "/",
            get(|| async { Redirect::to("/dashboard").into_response() }),
        )
        .route("/dashboard", get(dashboard_assets::serve))
        .route("/dashboard/", get(dashboard_assets::serve))
        .route("/dashboard/{*path}", get(dashboard_assets::serve))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state,
            host_routing::dispatch_by_host,
        ))
        .layer(TraceLayer::new_for_http())
}

/// Gates every route it's layered over on "does this request carry a
/// valid session." Per-app and admin-only checks happen deeper in each
/// handler/middleware (see `api::enforce_app_access` and the
/// `require_admin` calls in `routes::users`).
async fn require_authenticated(
    _current_user: CurrentUser,
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    next.run(request).await
}
