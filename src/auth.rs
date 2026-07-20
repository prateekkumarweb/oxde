use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use base64::Engine as _;
use oxde_db::models::User;

use crate::{
    accounts::{self, AccountRole},
    error::AppError,
    state::AppState,
};

pub const SESSION_COOKIE: &str = "oxde_session";

/// 30 days - also the cookie's `Max-Age`; a session past this age is
/// treated as expired and evicted on next use even if the process never
/// restarted (see [`CurrentUser::from_request_parts`]).
pub const SESSION_MAX_AGE_SECS: i64 = 60 * 60 * 24 * 30;

#[derive(Debug, Clone)]
pub struct Session {
    pub username: String,
    pub created_at: i64,
}

/// Random 32-byte token, base64-encoded - opaque, unguessable, and doesn't
/// need to be looked up against a hash (unlike the password itself), since
/// losing it only grants what the session already grants.
pub fn generate_session_token() -> String {
    let bytes = rand::random::<[u8; 32]>();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The authenticated user for the current request, resolved from the
/// session cookie. Re-reads the user's role from the database on every
/// request (not cached in the session) so a role change or deletion takes
/// effect immediately rather than only on next login.
pub struct CurrentUser {
    pub username: String,
    pub role: AccountRole,
}

impl<S> FromRequestParts<S> for CurrentUser
where
    AppState: axum::extract::FromRef<S>,
    S: Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);

        let token =
            cookie_value(&parts.headers, SESSION_COOKIE).ok_or(AppError::Unauthenticated)?;

        let username = {
            let mut sessions = state
                .sessions()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session = sessions.get(&token).ok_or(AppError::Unauthenticated)?;
            if accounts::now_epoch_secs() - session.created_at > SESSION_MAX_AGE_SECS {
                sessions.remove(&token);
                drop(sessions);
                return Err(AppError::Unauthenticated);
            }
            session.username.clone()
        };

        let mut db = state.db().clone();
        let user = User::all()
            .filter(User::fields().username().eq(&username))
            .first()
            .exec(&mut db)
            .await
            .map_err(AppError::Db)?
            .ok_or(AppError::Unauthenticated)?;

        let role = accounts::user_role(&user)?;
        Ok(Self {
            username: user.username,
            role,
        })
    }
}

impl CurrentUser {
    pub fn require_admin(&self) -> Result<(), AppError> {
        match self.role {
            AccountRole::Admin => Ok(()),
            AccountRole::Member => Err(AppError::Forbidden("admin access required".to_string())),
        }
    }
}

pub fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

/// Invalidates every existing session for `username` - called on role
/// change, password change/reset, and account deletion so a stale session
/// can't keep working past that point.
pub fn revoke_sessions_for(state: &AppState, username: &str) {
    let mut sessions = state
        .sessions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sessions.retain(|_, session| session.username != username);
}

/// `httpOnly`/`Secure`/`SameSite=Lax`, scoped to the whole domain (not just
/// `/dashboard` or `/api`) so it's sent on every `OxDe` request. Not scoped to
/// `base_domain` specifically since the session is only ever meaningful on
/// the host actually serving the dashboard/API.
pub fn session_cookie_header(token: &str, max_age_secs: i64) -> (axum::http::HeaderName, String) {
    (
        axum::http::header::SET_COOKIE,
        format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_secs}"
        ),
    )
}

pub fn clear_session_cookie_header() -> (axum::http::HeaderName, String) {
    (
        axum::http::header::SET_COOKIE,
        format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0"),
    )
}
