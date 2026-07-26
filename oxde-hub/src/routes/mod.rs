use axum::{
    Router,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, header},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use tower_http::trace::TraceLayer;

use crate::{
    auth::{self, ApiUser, CurrentUser},
    dashboard_assets,
    error::AppError,
    state::AppState,
};

pub mod api;
pub mod apps;
mod auth_routes;
mod host_routing;
mod mcp;
mod users;

pub fn build_router(state: AppState) -> Router {
    let public_api = Router::new().nest("/api", auth_routes::public_router());

    // Cookie or bearer token; `/api/users` below stays cookie-only.
    let bearer_or_cookie_api = Router::new()
        .nest("/api", api::router(&state))
        .route_layer(middleware::from_fn(enforce_same_origin))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_authenticated_bearer_or_cookie,
        ));

    let cookie_only_api = Router::new()
        .nest("/api", auth_routes::protected_router())
        .nest("/api/users", users::router())
        .route_layer(middleware::from_fn(enforce_same_origin))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_authenticated,
        ));

    let mut router = Router::new()
        .merge(public_api)
        .merge(bearer_or_cookie_api)
        .merge(cookie_only_api);
    if state.enable_mcp() {
        router = router.merge(mcp::router(&state));
    }

    router
        .route(
            "/",
            get(|| async { Redirect::to("/dashboard").into_response() }),
        )
        .route("/dashboard", get(dashboard_assets::serve))
        .route("/dashboard/", get(dashboard_assets::serve))
        .route("/dashboard/{*path}", get(dashboard_assets::serve))
        .with_state(state.clone())
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(
            state,
            host_routing::dispatch_by_host,
        ))
        .layer(TraceLayer::new_for_http())
}

/// Only wraps the control-plane router, not proxied app responses (a
/// deployed app's headers aren't ours to override). No `Strict-Transport-
/// Security`: `tls.mode` is a single global switch, and HSTS caching would
/// hard-break every app subdomain if it's flipped back to `"off"`.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

async fn security_headers(request: Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response
}

/// Gates every route it's layered over on "does this request carry a
/// valid session." Per-app and admin-only checks happen deeper in each
/// handler/middleware (see `api::enforce_app_access` and the
/// `require_admin` calls in `routes::users`).
async fn require_authenticated(
    _current_user: CurrentUser,
    request: Request,
    next: middleware::Next,
) -> Response {
    next.run(request).await
}

/// Same gate as `require_authenticated`, but via `ApiUser` so a valid API
/// bearer token satisfies it too, not just the session cookie.
async fn require_authenticated_bearer_or_cookie(
    _current_user: ApiUser,
    request: Request,
    next: middleware::Next,
) -> Response {
    next.run(request).await
}

/// Rejects state-changing requests that carry a session cookie but aren't
/// same-origin (see `auth::verify_same_origin`).
async fn enforce_same_origin(
    method: Method,
    headers: HeaderMap,
    request: Request,
    next: middleware::Next,
) -> Result<Response, AppError> {
    auth::verify_same_origin(&method, &headers)?;
    Ok(next.run(request).await)
}
