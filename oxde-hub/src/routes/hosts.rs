use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
};
use oxde_db::models::Host as DbHost;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    auth::CurrentUser,
    error::{AppError, AppResult},
    state::AppState,
    storage,
};

#[derive(Serialize, TS)]
#[ts(export)]
pub struct HostView {
    pub id: i64,
    pub name: String,
    pub revoked: bool,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Returned only from `create_host` - `plaintext_token` is shown exactly
/// once and never recoverable afterward (only its hash is stored).
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CreateHostResponse {
    pub host: HostView,
    pub plaintext_token: String,
}

#[derive(Deserialize)]
struct CreateHostRequest {
    name: String,
}

fn host_view(host: DbHost) -> HostView {
    HostView {
        id: host.id,
        name: host.name,
        revoked: host.revoked,
        last_seen_at: host.last_seen_at,
        created_at: host.created_at,
        updated_at: host.updated_at,
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_hosts).post(create_host))
        .route("/{id}", delete(revoke_host))
}

async fn list_hosts(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> AppResult<Json<Vec<HostView>>> {
    current_user.require_admin()?;
    let hosts = storage::list_hosts(&state).await?;
    Ok(Json(hosts.into_iter().map(host_view).collect()))
}

async fn create_host(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateHostRequest>,
) -> AppResult<(StatusCode, Json<CreateHostResponse>)> {
    current_user.require_admin()?;
    if body.name.trim().is_empty() {
        return Err(AppError::InvalidName(body.name));
    }
    let (row, plaintext_token) = storage::create_host(&state, body.name.trim()).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateHostResponse {
            host: host_view(row),
            plaintext_token,
        }),
    ))
}

async fn revoke_host(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    current_user.require_admin()?;
    storage::revoke_host(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
