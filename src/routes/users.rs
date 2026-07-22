use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, patch},
};
use oxde_db::models::{ApiToken as DbApiToken, User};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    accounts::{self, AccountRole},
    auth::{self, CurrentUser},
    error::{AppError, AppResult},
    state::AppState,
    storage,
};

#[derive(Serialize, TS)]
#[ts(export)]
pub struct UserView {
    pub username: String,
    pub role: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_users).post(create_user))
        .route("/me/password", patch(change_own_password))
        .route("/me/tokens", get(list_own_tokens).post(create_own_token))
        .route("/me/tokens/{id}", delete(revoke_own_token))
        .route("/{username}", patch(update_user).delete(delete_user))
}

#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    role: String,
}

async fn list_users(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> AppResult<Json<Vec<UserView>>> {
    current_user.require_admin()?;
    let mut db = state.db().clone();
    let users = User::all().exec(&mut db).await.map_err(AppError::Db)?;
    Ok(Json(
        users
            .into_iter()
            .map(|user| UserView {
                username: user.username,
                role: user.role,
            })
            .collect(),
    ))
}

async fn create_user(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateUserRequest>,
) -> AppResult<(StatusCode, Json<UserView>)> {
    current_user.require_admin()?;
    accounts::validate_username(&body.username)?;
    accounts::validate_password(&body.password)?;
    let role: AccountRole = body.role.parse()?;

    let mut db = state.db().clone();
    let existing = User::all()
        .filter(User::fields().username().eq(&body.username))
        .first()
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?;
    if existing.is_some() {
        return Err(AppError::UserAlreadyExists(body.username));
    }

    let password_hash = accounts::hash_password(&body.password)?;
    let now = accounts::now_epoch_secs();
    let user = User::create()
        .username(&body.username)
        .password_hash(password_hash)
        .role(role.as_str())
        .created_at(now)
        .updated_at(now)
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?;

    Ok((
        StatusCode::CREATED,
        Json(UserView {
            username: user.username,
            role: user.role,
        }),
    ))
}

#[derive(Deserialize)]
struct UpdateUserRequest {
    role: Option<String>,
    password: Option<String>,
}

/// Admin-only role change and/or password reset (no old password needed) -
/// the "I forgot my password, ask an admin" recovery path.
async fn update_user(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(username): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> AppResult<Json<UserView>> {
    current_user.require_admin()?;

    let mut db = state.db().clone();
    let mut user = User::all()
        .filter(User::fields().username().eq(&username))
        .first()
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?
        .ok_or_else(|| AppError::UserNotFound(username.clone()))?;

    let mut update = user.update();
    if let Some(role) = &body.role {
        let role: AccountRole = role.parse()?;
        if username == current_user.username && role != AccountRole::Admin {
            // Same self-lockout footgun delete_user blocks below - demoting
            // yourself can leave zero admins reachable until a restart.
            return Err(AppError::Forbidden(
                "you can't change your own role".to_string(),
            ));
        }
        update = update.role(role.as_str());
    }
    if let Some(password) = &body.password {
        accounts::validate_password(password)?;
        update = update.password_hash(accounts::hash_password(password)?);
    }
    update = update.updated_at(accounts::now_epoch_secs());
    // `exec` reloads `user` in place from the applied update rather than
    // returning a separate value.
    update.exec(&mut db).await.map_err(AppError::Db)?;

    if body.role.is_some() || body.password.is_some() {
        auth::revoke_sessions_for(&state, &username);
    }

    Ok(Json(UserView {
        username: user.username,
        role: user.role,
    }))
}

async fn delete_user(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(username): Path<String>,
) -> AppResult<StatusCode> {
    current_user.require_admin()?;
    if username == current_user.username {
        // Self-lockout footgun with no recovery short of editing oxde.toml
        // and restarting - block it outright.
        return Err(AppError::Forbidden(
            "you can't delete your own account".to_string(),
        ));
    }

    // TODO: this leaves `username` in any app's `permissions` list (still
    // JSON-file-backed) as a dangling grant, which a later user recreated
    // under the same name would silently inherit. Handle it once app
    // permissions move into the DB alongside `users`.
    let mut db = state.db().clone();
    User::all()
        .filter(User::fields().username().eq(&username))
        .delete()
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?;

    auth::revoke_sessions_for(&state, &username);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// Self-service password change - requires the current password, no
/// `Admin` rights needed since proving you know the old password is itself
/// the authorization.
async fn change_own_password(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    let mut db = state.db().clone();
    let mut user = User::all()
        .filter(User::fields().username().eq(&current_user.username))
        .first()
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?
        .ok_or(AppError::Unauthenticated)?;

    if !accounts::verify_password(&body.current_password, &user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }
    accounts::validate_password(&body.new_password)?;

    user.update()
        .password_hash(accounts::hash_password(&body.new_password)?)
        .updated_at(accounts::now_epoch_secs())
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?;

    auth::revoke_sessions_for(&state, &current_user.username);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct ApiTokenView {
    pub id: i64,
    pub name: String,
    pub expires_at: i64,
    pub revoked: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Returned only from `create_own_token` - `plaintext_token` is shown
/// exactly once and never recoverable afterward (only its hash is stored).
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CreateApiTokenResponse {
    pub token: ApiTokenView,
    pub plaintext_token: String,
}

#[derive(Deserialize)]
struct CreateApiTokenRequest {
    name: String,
    /// Epoch seconds.
    expires_at: i64,
}

fn api_token_view(token: DbApiToken) -> ApiTokenView {
    ApiTokenView {
        id: token.id,
        name: token.name,
        expires_at: token.expires_at,
        revoked: token.revoked,
        created_at: token.created_at,
        updated_at: token.updated_at,
    }
}

/// `CurrentUser`, not `ApiUser`: a token must never be usable to create,
/// list, or revoke other tokens.
async fn list_own_tokens(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> AppResult<Json<Vec<ApiTokenView>>> {
    let tokens = storage::list_api_tokens(&state, current_user.id).await?;
    Ok(Json(tokens.into_iter().map(api_token_view).collect()))
}

async fn create_own_token(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateApiTokenRequest>,
) -> AppResult<(StatusCode, Json<CreateApiTokenResponse>)> {
    if body.name.trim().is_empty() {
        return Err(AppError::InvalidName(body.name));
    }
    let (row, plaintext_token) =
        storage::create_api_token(&state, current_user.id, body.name.trim(), body.expires_at)
            .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateApiTokenResponse {
            token: api_token_view(row),
            plaintext_token,
        }),
    ))
}

async fn revoke_own_token(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    storage::revoke_api_token(&state, current_user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
