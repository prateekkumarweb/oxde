use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub source: AppSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum AppSource {
    #[default]
    Upload,
    Git(GitSource),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSource {
    pub repo_url: String,
    pub branch: String,
    #[serde(default)]
    pub publish_dir: Option<String>,
}

/// Only `https://`/`http://`/`ssh://`/`git://` are accepted - a cheap
/// footgun guard, not a hard security boundary (this is admin-only input,
/// same trust level as an uploaded zip).
pub fn validate_repo_url(repo_url: &str) -> AppResult<()> {
    let allowed = ["https://", "http://", "ssh://", "git://"];
    if allowed.iter().any(|prefix| repo_url.starts_with(prefix)) {
        Ok(())
    } else {
        Err(AppError::InvalidRepoUrl(repo_url.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub id: String,
    pub app: String,
    pub created_at: Timestamp,
    pub original_filename: Option<String>,
    pub upload_size_bytes: u64,
    #[serde(default)]
    pub git: Option<GitDeploymentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDeploymentInfo {
    pub commit_sha: String,
    pub branch: String,
}

/// Slugs double as directory names and `<name>.<base_domain>` subdomain
/// labels, so they're restricted to what's safe in both places.
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
