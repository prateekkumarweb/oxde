#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Validation failures for the model types below.
///
/// Kept separate from the main crate's `AppError` so this crate has no
/// dependency on axum/HTTP concerns - `berth::error::AppError` converts these
/// via `From`, preserving today's exact error variants/messages/status
/// codes.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ModelError {
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid repo url: {0}")]
    InvalidRepoUrl(String),
    #[error("invalid run config: {0}")]
    InvalidRunConfig(String),
    #[error("invalid build config: {0}")]
    InvalidBuildConfig(String),
    #[error("invalid env var key: {0}")]
    InvalidEnvVar(String),
}

pub type ModelResult<T> = Result<T, ModelError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub host_id: i64,
    #[serde(default)]
    pub source: AppSource,
    /// Injected into run-mode containers and install/build commands.
    /// Doesn't apply to static-mode apps, which run no commands at all.
    #[serde(default)]
    pub env_vars: Vec<EnvVar>,
    /// Per-`Member` access grants. `Admin` accounts ignore this entirely
    /// (always full access); a `Member` gets exactly what's listed here.
    #[serde(default)]
    pub permissions: Vec<AppPermission>,
}

impl App {
    /// Whether `username` (a `Member`, not `Admin` - callers check that
    /// separately) has at least `required` access to this app.
    #[must_use]
    pub fn has_permission(&self, username: &str, required: PermissionLevel) -> bool {
        self.permissions
            .iter()
            .any(|grant| grant.username == username && grant.level.satisfies(required))
    }

    #[must_use]
    pub const fn run_config(&self) -> Option<&RunConfig> {
        match &self.source {
            AppSource::Git(git_source) => match &git_source.mode {
                GitDeployMode::Run(run) => Some(run),
                GitDeployMode::Static { .. } | GitDeployMode::Build(_) => None,
            },
            AppSource::Upload => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppPermission {
    pub username: String,
    pub level: PermissionLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionLevel {
    /// Read app config, deployments, logs, stats.
    Read,
    /// Everything `Read` allows, plus deploy, activate, env vars, delete.
    Write,
}

impl PermissionLevel {
    /// `Write` satisfies a `Read` requirement; `Read` does not satisfy `Write`.
    const fn satisfies(self, required: Self) -> bool {
        matches!(
            (self, required),
            (Self::Write, _) | (Self::Read, Self::Read)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[ts(export)]
pub enum EnvVarValue {
    Plain(String),
    /// Ciphertext.
    Secret(String),
}

impl EnvVarValue {
    /// The raw string payload regardless of variant - callers that only
    /// need a `KEY=value` pair (e.g. injecting into a container) don't care
    /// whether it's plaintext or (already-decrypted) ciphertext.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain(value) | Self::Secret(value) => value,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct EnvVar {
    pub key: String,
    pub value: EnvVarValue,
}

/// Create/update wire type. `Secret(None)` means "keep the existing
/// encrypted value" (update only).
#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[ts(export)]
pub enum EnvVarInputValue {
    Plain(String),
    Secret(Option<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct EnvVarInput {
    pub key: String,
    pub value: EnvVarInputValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
#[ts(export)]
pub enum AppSource {
    #[default]
    Upload,
    Git(GitSource),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct GitSource {
    pub repo_url: String,
    pub branch: String,
    #[serde(default)]
    pub mode: GitDeployMode,
}

/// The three ways a git-sourced app can be served - exactly one at a time.
#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum GitDeployMode {
    Static {
        #[serde(default)]
        publish_dir: Option<String>,
    },
    Build(BuildConfig),
    Run(RunConfig),
}

impl Default for GitDeployMode {
    fn default() -> Self {
        Self::Static { publish_dir: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct BuildConfig {
    pub image: RunImage,
    pub command: String,
    pub output_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct RunConfig {
    pub image: RunImage,
    #[serde(default)]
    pub install_command: Option<String>,
    pub start_command: String,
    pub container_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RunImage {
    Node24,
    Python314,
}

impl RunImage {
    /// The curated catalog this maps to - deliberately not arbitrary
    /// images/Dockerfiles.
    #[must_use]
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
///
/// # Errors
///
/// Returns `ModelError::InvalidRepoUrl` if `repo_url` doesn't start with an
/// allowed scheme.
pub fn validate_repo_url(repo_url: &str) -> ModelResult<()> {
    let allowed = ["https://", "http://", "ssh://", "git://"];
    if allowed.iter().any(|prefix| repo_url.starts_with(prefix)) {
        Ok(())
    } else {
        Err(ModelError::InvalidRepoUrl(repo_url.to_string()))
    }
}

/// # Errors
///
/// Returns `ModelError::InvalidRunConfig` if `container_port` is `0` or
/// `start_command` is empty.
pub fn validate_run_config(run: &RunConfig) -> ModelResult<()> {
    if run.container_port == 0 {
        return Err(ModelError::InvalidRunConfig(
            "container port must be 1-65535".to_string(),
        ));
    }
    if run.start_command.trim().is_empty() {
        return Err(ModelError::InvalidRunConfig(
            "start command is required in run mode".to_string(),
        ));
    }
    Ok(())
}

fn valid_env_var_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// # Errors
///
/// Returns `ModelError::InvalidEnvVar` for the first key that isn't a valid
/// identifier (`[A-Za-z_][A-Za-z0-9_]*`), or the first key that repeats.
pub fn validate_env_var_inputs(env_vars: &[EnvVarInput]) -> ModelResult<()> {
    let mut seen = std::collections::HashSet::new();
    for env_var in env_vars {
        if !valid_env_var_key(&env_var.key) {
            return Err(ModelError::InvalidEnvVar(env_var.key.clone()));
        }
        if !seen.insert(env_var.key.as_str()) {
            return Err(ModelError::InvalidEnvVar(env_var.key.clone()));
        }
    }
    Ok(())
}

/// # Errors
///
/// Returns `ModelError::InvalidBuildConfig` if `command` or `output_dir` is
/// empty.
pub fn validate_build_config(build: &BuildConfig) -> ModelResult<()> {
    if build.command.trim().is_empty() {
        return Err(ModelError::InvalidBuildConfig(
            "build command is required".to_string(),
        ));
    }
    if build.output_dir.trim().is_empty() {
        return Err(ModelError::InvalidBuildConfig(
            "output dir is required".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Deployment {
    pub id: String,
    pub app: String,
    pub created_at: Timestamp,
    pub original_filename: Option<String>,
    pub upload_size_bytes: u64,
    #[serde(default)]
    pub git: Option<GitDeploymentInfo>,
    #[serde(default)]
    pub build_info: Option<BuildInfo>,
    /// Deterministic name (`berth-{app_name}-{deployment_id}`) of the
    /// container backing this deployment when it's run-mode; `None` for
    /// static/upload/build deployments.
    #[serde(default)]
    pub container_name: Option<String>,
    /// Defaults to `Ready` on deserialize so deployments written before this
    /// field existed (always synchronously finished) come back correctly.
    #[serde(default = "DeploymentStatus::default_ready")]
    pub status: DeploymentStatus,
}

/// Every deployment starts `Ready` except an in-flight git deploy, which
/// starts `Pending` and is only visible as a record (no `files/` yet) so a
/// client can attach to its logs before it finishes.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum DeploymentStatus {
    Pending,
    Ready,
    Failed { error: String },
}

impl DeploymentStatus {
    const fn default_ready() -> Self {
        Self::Ready
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitDeploymentInfo {
    pub commit_sha: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BuildInfo {
    pub image: RunImage,
    pub command: String,
}

/// Slugs double as directory names and `<name>.<base_domain>` subdomain
/// labels, so they're restricted to what's safe in both places.
///
/// # Errors
///
/// Returns `ModelError::InvalidName` if `name` is empty, over 63 characters,
/// starts/ends with `-`, or contains anything outside
/// `[a-z0-9-]`.
pub fn validate_slug(name: &str) -> ModelResult<()> {
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
        Err(ModelError::InvalidName(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Deployment, DeploymentStatus, EnvVarInput, EnvVarInputValue, GitDeployMode, GitSource,
        ModelError, RunConfig, RunImage, validate_env_var_inputs,
    };

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
            mode: GitDeployMode::Static {
                publish_dir: Some("dist".to_string()),
            },
        };
        let json = serde_json::to_string(&source).expect("serialize");
        let round_tripped: GitSource = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            round_tripped.mode,
            GitDeployMode::Static { publish_dir: Some(ref dir) } if dir == "dist"
        ));
    }

    #[test]
    fn git_source_round_trips_in_run_mode() {
        let source = GitSource {
            repo_url: "https://example.com/repo.git".to_string(),
            branch: "main".to_string(),
            mode: GitDeployMode::Run(RunConfig {
                image: RunImage::Node24,
                install_command: Some("npm install".to_string()),
                start_command: "npm start".to_string(),
                container_port: 3000,
            }),
        };
        let json = serde_json::to_string(&source).expect("serialize");
        let round_tripped: GitSource = serde_json::from_str(&json).expect("deserialize");
        let GitDeployMode::Run(run) = round_tripped.mode else {
            panic!("expected run mode");
        };
        assert_eq!(run.image, RunImage::Node24);
        assert_eq!(run.container_port, 3000);
        assert_eq!(run.install_command.as_deref(), Some("npm install"));
    }

    /// A `GitSource` serialized before `mode` existed (implicitly
    /// build-less/static with no `publish_dir`) must still deserialize.
    #[test]
    fn git_source_without_mode_field_deserializes() {
        let json = r#"{"repo_url":"https://example.com/repo.git","branch":"main"}"#;
        let source: GitSource = serde_json::from_str(json).expect("deserialize");
        assert!(matches!(
            source.mode,
            GitDeployMode::Static { publish_dir: None }
        ));
    }

    #[test]
    fn git_source_round_trips_in_build_mode() {
        let source = GitSource {
            repo_url: "https://example.com/repo.git".to_string(),
            branch: "main".to_string(),
            mode: GitDeployMode::Build(super::BuildConfig {
                image: RunImage::Node24,
                command: "npm run build".to_string(),
                output_dir: "dist".to_string(),
            }),
        };
        let json = serde_json::to_string(&source).expect("serialize");
        let round_tripped: GitSource = serde_json::from_str(&json).expect("deserialize");
        let GitDeployMode::Build(build) = round_tripped.mode else {
            panic!("expected build mode");
        };
        assert_eq!(build.image, RunImage::Node24);
        assert_eq!(build.output_dir, "dist");
    }

    /// A `Deployment` serialized before `container_name` existed must still
    /// deserialize, defaulting to `None`.
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
        assert!(matches!(deployment.status, DeploymentStatus::Ready));
    }

    #[test]
    fn deployment_status_round_trips() {
        for status in [
            DeploymentStatus::Pending,
            DeploymentStatus::Ready,
            DeploymentStatus::Failed {
                error: "boom".to_string(),
            },
        ] {
            let json = serde_json::to_string(&status).expect("serialize");
            let round_tripped: DeploymentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                serde_json::to_string(&round_tripped).expect("serialize"),
                json
            );
        }
    }

    #[test]
    fn validate_env_var_inputs_rejects_duplicate_keys() {
        let env_vars = vec![
            EnvVarInput {
                key: "API_KEY".to_string(),
                value: EnvVarInputValue::Plain("one".to_string()),
            },
            EnvVarInput {
                key: "API_KEY".to_string(),
                value: EnvVarInputValue::Secret(Some("two".to_string())),
            },
        ];
        let err = validate_env_var_inputs(&env_vars).expect_err("duplicate key must be rejected");
        assert!(matches!(err, ModelError::InvalidEnvVar(key) if key == "API_KEY"));
    }

    #[test]
    fn validate_env_var_inputs_accepts_unique_keys() {
        let env_vars = vec![
            EnvVarInput {
                key: "API_KEY".to_string(),
                value: EnvVarInputValue::Plain("one".to_string()),
            },
            EnvVarInput {
                key: "OTHER_KEY".to_string(),
                value: EnvVarInputValue::Secret(Some("two".to_string())),
            },
        ];
        validate_env_var_inputs(&env_vars).expect("unique keys must be accepted");
    }
}
