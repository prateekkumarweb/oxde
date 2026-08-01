use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, patch},
};
use oxde_db::models::Host as DbHost;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    auth::CurrentUser,
    error::{AppError, AppResult},
    host_stats::{self, HostStats},
    state::AppState,
    storage,
};

#[derive(Serialize, TS)]
#[ts(export)]
pub struct HostView {
    pub id: i64,
    pub name: String,
    pub revoked: bool,
    pub connected: bool,
    pub ip: Option<String>,
    pub last_connected_ip: Option<String>,
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

#[derive(Deserialize)]
struct UpdateHostIpRequest {
    ip: Option<String>,
}

fn host_view(state: &AppState, host: DbHost) -> HostView {
    HostView {
        connected: state.agent_registry().is_connected(host.id),
        id: host.id,
        name: host.name,
        revoked: host.revoked,
        ip: host.ip,
        last_connected_ip: host.last_connected_ip,
        last_seen_at: host.last_seen_at,
        created_at: host.created_at,
        updated_at: host.updated_at,
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_hosts).post(create_host))
        .route("/{id}", delete(revoke_host))
        .route("/{id}/ip", patch(update_host_ip))
        .route("/{id}/stats", get(host_stats_endpoint))
}

/// Any authenticated user, not just admins - a `Member` needs this list to
/// pick a `host_id`. Creating/revoking a host stays admin-only below.
async fn list_hosts(
    State(state): State<AppState>,
    _current_user: CurrentUser,
) -> AppResult<Json<Vec<HostView>>> {
    let hosts = storage::list_hosts(&state).await?;
    Ok(Json(
        hosts
            .into_iter()
            .map(|host| host_view(&state, host))
            .collect(),
    ))
}

async fn create_host(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Json(body): Json<CreateHostRequest>,
) -> AppResult<(StatusCode, Json<CreateHostResponse>)> {
    current_user.require_admin()?;
    let (row, plaintext_token) = storage::create_host(&state, body.name.trim()).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateHostResponse {
            host: host_view(&state, row),
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

async fn update_host_ip(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateHostIpRequest>,
) -> AppResult<Json<HostView>> {
    current_user.require_admin()?;
    let ip = body.ip.filter(|ip| !ip.trim().is_empty());
    storage::update_host_ip(&state, id, ip).await?;
    let host = storage::list_hosts(&state)
        .await?
        .into_iter()
        .find(|host| host.id == id)
        .ok_or(AppError::HostNotFound)?;
    Ok(Json(host_view(&state, host)))
}

/// Any authenticated user, matching `list_hosts` - the underlying agent
/// call is scoped to one host, not admin-only host-wide data.
async fn host_stats_endpoint(
    State(state): State<AppState>,
    _current_user: CurrentUser,
    Path(id): Path<i64>,
) -> AppResult<Json<HostStats>> {
    let host_stats = host_stats::collect(&state.agent_link_for(id)).await?;
    Ok(Json(host_stats))
}
