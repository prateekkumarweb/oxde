use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("app not found: {0}")]
    AppNotFound(String),
    #[error("deployment not found: {0}")]
    DeploymentNotFound(String),
    #[error("app already exists: {0}")]
    AppAlreadyExists(String),
    #[error("cannot delete the active deployment")]
    DeleteActiveDeployment,
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid repo url: {0}")]
    InvalidRepoUrl(String),
    #[error("app {0} is not git-sourced")]
    NotGitSourced(String),
    #[error("app {0} already has a deployment in progress")]
    DeploymentInProgress(String),
    #[error("invalid publish dir: {0}")]
    InvalidPublishDir(String),
    #[error("invalid run config: {0}")]
    InvalidRunConfig(String),
    #[error("invalid build config: {0}")]
    InvalidBuildConfig(String),
    #[error("invalid env var key: {0}")]
    InvalidEnvVar(String),
    #[error("invalid username: {0}")]
    InvalidUsername(String),
    #[error("invalid role: {0}")]
    InvalidRole(String),
    #[error("{0}")]
    InvalidPassword(String),
    #[error("user not found: {0}")]
    UserNotFound(String),
    #[error("user already exists: {0}")]
    UserAlreadyExists(String),
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("password hashing failed: {0}")]
    PasswordHash(String),
    #[error("not authenticated")]
    Unauthenticated,
    #[error("{0}")]
    Forbidden(String),
    #[error("database error: {0}")]
    Db(#[from] toasty::Error),
    #[error("git fetch failed: {0}")]
    Git(String),
    #[error("container failed to start: {0}")]
    ContainerStartFailed(String),
    #[error("container backend unavailable: {0}")]
    ContainerUnavailable(String),
    #[error("deployment {0} has no container")]
    NoContainer(String),
    #[error("upload too large")]
    TooLarge,
    #[error("missing 'file' field in upload")]
    MissingUploadFile,
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Multipart(#[from] axum::extract::multipart::MultipartError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Timestamp(#[from] jiff::Error),
    #[error("corrupt database row: {0}")]
    CorruptData(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::AppNotFound(_) | Self::DeploymentNotFound(_) | Self::UserNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            Self::AppAlreadyExists(_)
            | Self::DeleteActiveDeployment
            | Self::DeploymentInProgress(_)
            | Self::UserAlreadyExists(_) => StatusCode::CONFLICT,
            Self::InvalidName(_)
            | Self::InvalidRepoUrl(_)
            | Self::NotGitSourced(_)
            | Self::InvalidPublishDir(_)
            | Self::InvalidRunConfig(_)
            | Self::InvalidBuildConfig(_)
            | Self::InvalidEnvVar(_)
            | Self::InvalidUsername(_)
            | Self::InvalidRole(_)
            | Self::InvalidPassword(_)
            | Self::NoContainer(_)
            | Self::MissingUploadFile => StatusCode::BAD_REQUEST,
            Self::InvalidCredentials | Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Git(_) | Self::ContainerStartFailed(_) | Self::ContainerUnavailable(_) => {
                StatusCode::BAD_GATEWAY
            }
            Self::Zip(_)
            | Self::Multipart(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::PasswordHash(_)
            | Self::Timestamp(_)
            | Self::CorruptData(_)
            | Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // 4xx are routine (bad input, no session yet, no permission) - only
        // an actual server fault (5xx) is worth an ERROR-level log.
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        } else {
            tracing::debug!(error = %self, "request failed");
        }
        (
            status,
            Json(ErrorBody {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
