use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};

use crate::error::{AppError, AppResult};

/// `.` as the delimiter since it doesn't appear in base64url output.
pub const PREFIX: &str = "oxde_";

const SELECTOR_BYTES: usize = 16;
const SECRET_BYTES: usize = 32;

pub const fn validate_expiry(now: i64, expires_at: i64, max_expiry_days: i64) -> AppResult<()> {
    let max_expires_at = now + max_expiry_days * 86_400;
    if expires_at > now && expires_at <= max_expires_at {
        Ok(())
    } else {
        Err(AppError::InvalidTokenExpiry(expires_at))
    }
}

/// `(token_id, secret_plaintext, token_hash)` - only `secret_plaintext` is
/// never persisted.
pub fn generate() -> AppResult<(String, String, String)> {
    let token_id = random_urlsafe(SELECTOR_BYTES);
    let secret = random_urlsafe(SECRET_BYTES);
    let hash = hash_secret(&secret)?;
    Ok((token_id, secret, hash))
}

fn random_urlsafe(len: usize) -> String {
    use base64::Engine as _;
    let bytes: Vec<u8> = (0..len).map(|_| rand::random::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_secret(secret: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::PasswordHash(err.to_string()))
}

pub fn verify_secret(secret: &str, hash: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(secret.as_bytes(), &parsed_hash)
        .is_ok()
}

/// Returns `None` (not an `AppError`) on any malformed shape - a malformed
/// header should reject the same way an unknown/expired/wrong token does,
/// not distinguishably.
pub fn parse_bearer_value(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix(PREFIX)?;
    rest.split_once('.')
}

pub fn format_token(token_id: &str, secret: &str) -> String {
    format!("{PREFIX}{token_id}.{secret}")
}
