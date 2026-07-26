use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use jiff::Timestamp;
use oxde_db::models::User;

use crate::error::{AppError, AppResult};

const MIN_USERNAME_LEN: usize = 3;
const MIN_PASSWORD_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountRole {
    /// Full access to every app, plus user management.
    Admin,
    /// No access by default; see per-app `AppPermission` grants.
    Member,
}

impl AccountRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

impl std::str::FromStr for AccountRole {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            other => Err(AppError::InvalidRole(other.to_string())),
        }
    }
}

pub fn user_role(user: &User) -> AppResult<AccountRole> {
    user.role.parse()
}

/// Username format: must start with a letter, then any of `a-z0-9_`,
/// always lowercase, at least `MIN_USERNAME_LEN` chars - the `users` table
/// equivalent of `validate_slug`.
pub fn validate_username(username: &str) -> AppResult<()> {
    let valid = username.len() >= MIN_USERNAME_LEN
        && username
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
        && username
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');

    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidUsername(username.to_string()))
    }
}

pub fn validate_password(password: &str) -> AppResult<()> {
    if password.len() >= MIN_PASSWORD_LEN {
        Ok(())
    } else {
        Err(AppError::InvalidPassword(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )))
    }
}

/// OWASP's current minimum argon2id parameters (19 MiB, 2 iterations, 1
/// degree of parallelism)
pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::PasswordHash(err.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

pub fn now_epoch_secs() -> i64 {
    Timestamp::now().as_second()
}
