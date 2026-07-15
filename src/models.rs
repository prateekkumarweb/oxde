use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub id: String,
    pub app: String,
    pub created_at: Timestamp,
    pub original_filename: Option<String>,
    pub upload_size_bytes: u64,
}

/// Slugs double as directory names and `/apps/<name>/...` URL segments, so
/// they're restricted to what's safe in both places.
pub fn validate_slug(name: &str) -> AppResult<()> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');

    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidName(name.to_string()))
    }
}
