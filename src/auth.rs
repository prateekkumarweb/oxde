use std::net::IpAddr;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use base64::Engine;
use oxde_db::models::User;

use crate::{
    accounts::{self, AccountRole},
    api_tokens,
    error::AppError,
    state::AppState,
    storage,
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

/// Failed logins allowed from one IP within [`LOGIN_LOCKOUT_WINDOW_SECS`]
/// before it's locked out - see [`check_login_lockout`].
pub const LOGIN_MAX_FAILURES: u32 = 10;

pub const LOGIN_LOCKOUT_WINDOW_SECS: i64 = 15 * 60;

#[derive(Debug, Clone)]
pub struct LoginAttempts {
    count: u32,
    window_start: i64,
}

/// Rejects a login attempt from `ip` if it's already failed
/// [`LOGIN_MAX_FAILURES`] times within the current window - including a
/// correct password: once locked out, nothing from that IP succeeds until
/// the window rolls over, or an attacker who eventually guesses right would
/// still get in, just slower. Per-IP rather than per-username so an
/// attacker can't lock the real admin out by deliberately failing their
/// (often-guessable) username.
pub fn check_login_lockout(state: &AppState, ip: IpAddr) -> Result<(), AppError> {
    let now = accounts::now_epoch_secs();
    let Some(attempts) = state.login_attempts().pin().get(&ip).cloned() else {
        return Ok(());
    };
    let elapsed = now - attempts.window_start;
    if elapsed > LOGIN_LOCKOUT_WINDOW_SECS || attempts.count < LOGIN_MAX_FAILURES {
        return Ok(());
    }
    let retry_after_secs = LOGIN_LOCKOUT_WINDOW_SECS - elapsed;
    let retry_after_mins = (retry_after_secs + 59) / 60;
    Err(AppError::TooManyLoginAttempts(retry_after_mins))
}

/// Counts one more failed login from `ip` toward [`check_login_lockout`].
pub fn record_failed_login(state: &AppState, ip: IpAddr) {
    let now = accounts::now_epoch_secs();
    state.login_attempts().pin().update_or_insert_with(
        ip,
        |attempts| {
            if now - attempts.window_start > LOGIN_LOCKOUT_WINDOW_SECS {
                LoginAttempts {
                    count: 1,
                    window_start: now,
                }
            } else {
                LoginAttempts {
                    count: attempts.count + 1,
                    window_start: attempts.window_start,
                }
            }
        },
        || LoginAttempts {
            count: 1,
            window_start: now,
        },
    );
}

/// Clears `ip`'s failure count on a successful login.
pub fn clear_login_attempts(state: &AppState, ip: IpAddr) {
    state.login_attempts().pin().remove(&ip);
}

/// Random 32-byte token, base64-encoded - opaque, unguessable, and doesn't
/// need to be looked up against a hash (unlike the password itself), since
/// losing it only grants what the session already grants.
pub fn generate_session_token() -> String {
    let bytes = rand::random::<[u8; 32]>();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The authenticated user for the current request, resolved from the
/// session cookie only - never a bearer token, so a token can never mint
/// or revoke other tokens (see [`ApiUser`] for routes that accept both).
/// Re-reads the user's role from the database on every request (not
/// cached in the session) so a role change or deletion takes effect
/// immediately rather than only on next login.
#[derive(Clone)]
pub struct CurrentUser {
    pub id: i64,
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
        let username = resolve_cookie_username(&state, &parts.headers)?;
        load_current_user(&state, &username).await
    }
}

fn resolve_cookie_username(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<String, AppError> {
    let token = cookie_value(headers, SESSION_COOKIE).ok_or(AppError::Unauthenticated)?;

    let sessions = state.sessions().pin();
    let session = sessions.get(&token).ok_or(AppError::Unauthenticated)?;
    let expired = accounts::now_epoch_secs() - session.created_at > SESSION_MAX_AGE_SECS;
    let username = session.username.clone();

    if expired {
        sessions.remove(&token);
        return Err(AppError::Unauthenticated);
    }
    Ok(username)
}

async fn load_current_user(state: &AppState, username: &str) -> Result<CurrentUser, AppError> {
    let mut db = state.db().clone();
    let user = User::all()
        .filter(User::fields().username().eq(username))
        .first()
        .exec(&mut db)
        .await
        .map_err(AppError::Db)?
        .ok_or(AppError::Unauthenticated)?;

    let role = accounts::user_role(&user)?;
    Ok(CurrentUser {
        id: user.id,
        username: user.username,
        role,
    })
}

impl CurrentUser {
    pub fn require_admin(&self) -> Result<(), AppError> {
        match self.role {
            AccountRole::Admin => Ok(()),
            AccountRole::Member => Err(AppError::Forbidden("admin access required".to_string())),
        }
    }
}

/// Like [`CurrentUser`], but also accepts an API bearer token, checked
/// before falling back to the cookie. `Deref`s to `CurrentUser`.
pub struct ApiUser(pub CurrentUser);

impl std::ops::Deref for ApiUser {
    type Target = CurrentUser;

    fn deref(&self) -> &CurrentUser {
        &self.0
    }
}

impl<S> FromRequestParts<S> for ApiUser
where
    AppState: axum::extract::FromRef<S>,
    S: Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        if let Some(user) = load_user_from_bearer(&app_state, &parts.headers).await? {
            return Ok(Self(user));
        }

        let username = resolve_cookie_username(&app_state, &parts.headers)?;
        Ok(Self(load_current_user(&app_state, &username).await?))
    }
}

/// Like [`ApiUser`], but bearer-token only, no cookie fallback - for routes
/// whose clients are never a browser (e.g. MCP), so there's no session
/// cookie to protect from CSRF in the first place.
pub struct BearerUser(pub CurrentUser);

impl std::ops::Deref for BearerUser {
    type Target = CurrentUser;

    fn deref(&self) -> &CurrentUser {
        &self.0
    }
}

impl<S> FromRequestParts<S> for BearerUser
where
    AppState: axum::extract::FromRef<S>,
    S: Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        let user = load_user_from_bearer(&app_state, &parts.headers)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        Ok(Self(user))
    }
}

async fn load_user_from_bearer(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<Option<CurrentUser>, AppError> {
    let Some(bearer) = bearer_token(headers) else {
        return Ok(None);
    };
    let (token_id, secret) =
        api_tokens::parse_bearer_value(&bearer).ok_or(AppError::Unauthenticated)?;
    let user = storage::find_user_by_api_token(state, token_id, secret)
        .await?
        .ok_or(AppError::Unauthenticated)?;
    let role = accounts::user_role(&user)?;
    Ok(Some(CurrentUser {
        id: user.id,
        username: user.username,
        role,
    }))
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    value.strip_prefix("Bearer ").map(str::to_string)
}

/// CSRF check via the browser-set, JS-unforgeable `Sec-Fetch-Site` header -
/// needed because every deployed app is same-*site* as the dashboard
/// (subdomain-per-app), so `SameSite=Lax` alone doesn't block it, but no
/// app is same-*origin*. A missing header is treated as a rejection too.
pub fn verify_same_origin(
    method: &axum::http::Method,
    headers: &axum::http::HeaderMap,
) -> Result<(), AppError> {
    use axum::http::Method;
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(());
    }
    if bearer_token(headers).is_some() {
        return Ok(());
    }
    if cookie_value(headers, SESSION_COOKIE).is_none() {
        return Ok(());
    }

    let site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    if site == Some("same-origin") {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "cross-origin request rejected".to_string(),
        ))
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
    state
        .sessions()
        .pin()
        .retain(|_, session| session.username != username);
}

/// Like [`revoke_sessions_for`], but keeps `keep_token`'s session alive.
pub fn revoke_other_sessions_for(state: &AppState, username: &str, keep_token: &str) {
    state
        .sessions()
        .pin()
        .retain(|token, session| session.username != username || token == keep_token);
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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use axum::http::{HeaderMap, HeaderValue, Method};

    use super::*;
    use crate::state::AppStateLimits;

    /// A fresh `AppState` over its own tempdir, so tests never share state.
    async fn test_state(label: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-auth-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("create test data dir");
        let db = oxde_db::connect(&dir)
            .await
            .expect("connect test accounts database");
        oxde_db::apply_migrations(&db)
            .await
            .expect("apply test accounts database migrations");
        AppState::new(
            dir,
            AppStateLimits {
                max_upload_bytes: 10_000,
                max_uncompressed_bytes: 10_000,
                base_domain: "localhost".to_string(),
                git_fetch_timeout_secs: 60,
                install_timeout_secs: 300,
                build_timeout_secs: 300,
                api_token_max_expiry_days: 30,
                enable_mcp: false,
            },
            // These tests never touch containers - just needs to construct.
            bollard::Docker::connect_with_http(
                "http://localhost:0",
                5,
                bollard::API_DEFAULT_VERSION,
            )
            .expect("construct unused docker client"),
            crate::reverse_proxy::new_client(),
            db,
        )
    }

    fn test_ip(label: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, label))
    }

    #[tokio::test]
    async fn login_lockout_allows_attempts_under_the_threshold() {
        let state = test_state("lockout-under-threshold").await;
        let ip = test_ip(1);
        for _ in 0..LOGIN_MAX_FAILURES - 1 {
            record_failed_login(&state, ip);
        }
        assert!(check_login_lockout(&state, ip).is_ok());
    }

    #[tokio::test]
    async fn login_lockout_rejects_once_threshold_is_reached() {
        let state = test_state("lockout-at-threshold").await;
        let ip = test_ip(2);
        for _ in 0..LOGIN_MAX_FAILURES {
            record_failed_login(&state, ip);
        }
        assert!(matches!(
            check_login_lockout(&state, ip),
            Err(AppError::TooManyLoginAttempts(_))
        ));
    }

    #[tokio::test]
    async fn login_lockout_is_per_ip() {
        let state = test_state("lockout-per-ip").await;
        let attacker = test_ip(3);
        let admin = test_ip(4);
        for _ in 0..LOGIN_MAX_FAILURES {
            record_failed_login(&state, attacker);
        }
        assert!(check_login_lockout(&state, attacker).is_err());
        assert!(check_login_lockout(&state, admin).is_ok());
    }

    #[tokio::test]
    async fn clearing_login_attempts_lifts_the_lockout() {
        let state = test_state("lockout-cleared").await;
        let ip = test_ip(5);
        for _ in 0..LOGIN_MAX_FAILURES {
            record_failed_login(&state, ip);
        }
        assert!(check_login_lockout(&state, ip).is_err());
        clear_login_attempts(&state, ip);
        assert!(check_login_lockout(&state, ip).is_ok());
    }

    fn headers_with_cookie(name: &str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!("{name}={value}")).expect("valid cookie header"),
        );
        headers
    }

    fn headers_with_session_and_sec_fetch_site(sec_fetch_site: &str) -> HeaderMap {
        let mut headers = headers_with_cookie(SESSION_COOKIE, "sess-token");
        headers.insert(
            "sec-fetch-site",
            HeaderValue::from_str(sec_fetch_site).expect("valid header value"),
        );
        headers
    }

    #[test]
    fn get_requests_bypass_the_check() {
        let result = verify_same_origin(&Method::GET, &HeaderMap::new());
        assert!(result.is_ok());
    }

    #[test]
    fn bearer_token_requests_bypass_the_check() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sometoken"),
        );
        let result = verify_same_origin(&Method::POST, &headers);
        assert!(result.is_ok());
    }

    #[test]
    fn requests_without_a_session_cookie_bypass_the_check() {
        let result = verify_same_origin(&Method::POST, &HeaderMap::new());
        assert!(result.is_ok());
    }

    #[test]
    fn session_cookie_with_same_origin_sec_fetch_site_is_accepted() {
        let headers = headers_with_session_and_sec_fetch_site("same-origin");
        let result = verify_same_origin(&Method::POST, &headers);
        assert!(result.is_ok());
    }

    #[test]
    fn session_cookie_with_same_site_sec_fetch_site_is_rejected() {
        let headers = headers_with_session_and_sec_fetch_site("same-site");
        let result = verify_same_origin(&Method::POST, &headers);
        assert!(matches!(result, Err(AppError::Forbidden(_))));
    }

    #[test]
    fn session_cookie_without_sec_fetch_site_header_is_rejected() {
        let headers = headers_with_cookie(SESSION_COOKIE, "sess-token");
        let result = verify_same_origin(&Method::POST, &headers);
        assert!(matches!(result, Err(AppError::Forbidden(_))));
    }
}
