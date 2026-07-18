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
    pub publish_dir: Option<String>, // static mode; ignored when `run` is set
    #[serde(default)]
    pub run: Option<RunConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub image: RunImage,
    #[serde(default)]
    pub install_command: Option<String>,
    pub start_command: String,
    pub container_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunImage {
    Node24,
    Python314,
}

impl RunImage {
    /// The curated catalog this maps to - deliberately not arbitrary
    /// images/Dockerfiles.
    pub const fn image_tag(self) -> &'static str {
        match self {
            Self::Node24 => "docker.io/library/node:24",
            Self::Python314 => "docker.io/library/python:3.14",
        }
    }
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
    /// Deterministic name (`oxde-{app_name}-{deployment_id}`) of the
    /// container backing this deployment when it's run-mode; `None` for
    /// static/upload deployments.
    #[serde(default)]
    pub container_name: Option<String>,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{Deployment, GitSource, RunConfig, RunImage};

    #[test]
    fn run_image_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RunImage::Node24).expect("serialize"),
            "\"node24\""
        );
        assert_eq!(
            serde_json::to_string(&RunImage::Python314).expect("serialize"),
            "\"python314\""
        );
    }

    #[test]
    fn run_image_maps_to_curated_catalog() {
        assert_eq!(RunImage::Node24.image_tag(), "docker.io/library/node:24");
        assert_eq!(
            RunImage::Python314.image_tag(),
            "docker.io/library/python:3.14"
        );
    }

    #[test]
    fn git_source_round_trips_in_static_mode() {
        let source = GitSource {
            repo_url: "https://example.com/repo.git".to_string(),
            branch: "main".to_string(),
            publish_dir: Some("dist".to_string()),
            run: None,
        };
        let json = serde_json::to_string(&source).expect("serialize");
        let round_tripped: GitSource = serde_json::from_str(&json).expect("deserialize");
        assert!(round_tripped.run.is_none());
        assert_eq!(round_tripped.publish_dir.as_deref(), Some("dist"));
    }

    #[test]
    fn git_source_round_trips_in_run_mode() {
        let source = GitSource {
            repo_url: "https://example.com/repo.git".to_string(),
            branch: "main".to_string(),
            publish_dir: None,
            run: Some(RunConfig {
                image: RunImage::Node24,
                install_command: Some("npm install".to_string()),
                start_command: "npm start".to_string(),
                container_port: 3000,
            }),
        };
        let json = serde_json::to_string(&source).expect("serialize");
        let round_tripped: GitSource = serde_json::from_str(&json).expect("deserialize");
        let run = round_tripped.run.expect("run config");
        assert_eq!(run.image, RunImage::Node24);
        assert_eq!(run.container_port, 3000);
        assert_eq!(run.install_command.as_deref(), Some("npm install"));
    }

    /// A `GitSource` written before `run` existed (old `app.json` on disk)
    /// must still deserialize, defaulting `run` to `None`.
    #[test]
    fn git_source_without_run_field_deserializes() {
        let json = r#"{"repo_url":"https://example.com/repo.git","branch":"main"}"#;
        let source: GitSource = serde_json::from_str(json).expect("deserialize");
        assert!(source.run.is_none());
        assert!(source.publish_dir.is_none());
    }

    /// A `Deployment` written before `container_name` existed (old
    /// `deployment.json` on disk) must still deserialize, defaulting to `None`.
    #[test]
    fn deployment_without_container_name_field_deserializes() {
        let json = r#"{
            "id": "1-0",
            "app": "blog",
            "created_at": "2024-01-01T00:00:00Z",
            "original_filename": null,
            "upload_size_bytes": 0
        }"#;
        let deployment: Deployment = serde_json::from_str(json).expect("deserialize");
        assert!(deployment.container_name.is_none());
        assert!(deployment.git.is_none());
    }
}
