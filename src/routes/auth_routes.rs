use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
};
use oxde_db::models::User;
use serde::Deserialize;

use super::users::UserView;
use crate::{
    accounts,
    auth::{self, CurrentUser, Session},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
}

pub fn protected_router() -> Router<AppState> {
    Router::new().route("/me", get(me))
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let mut db = state.db().clone();
    let user = User::all()
        .filter(User::fields().username().eq(&body.username))
        .first()
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?
        .ok_or(AppError::InvalidCredentials)?;

    if !accounts::verify_password(&body.password, &user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }

    let token = auth::generate_session_token();
    state.sessions().pin().insert(
        token.clone(),
        Session {
            username: user.username.clone(),
            created_at: accounts::now_epoch_secs(),
        },
    );

    let (header_name, header_value) =
        auth::session_cookie_header(&token, auth::SESSION_MAX_AGE_SECS);
    Ok((
        [(header_name, header_value)],
        Json(UserView {
            username: user.username,
            role: user.role,
        }),
    ))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(token) = auth::cookie_value(&headers, auth::SESSION_COOKIE) {
        state.sessions().pin().remove(&token);
    }
    let (header_name, header_value) = auth::clear_session_cookie_header();
    [(header_name, header_value)]
}

async fn me(current_user: CurrentUser) -> Json<UserView> {
    Json(UserView {
        username: current_user.username,
        role: current_user.role.as_str().to_string(),
    })
}
