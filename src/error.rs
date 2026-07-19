use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

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
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::AppNotFound(_) | Self::DeploymentNotFound(_) => StatusCode::NOT_FOUND,
            Self::AppAlreadyExists(_)
            | Self::DeleteActiveDeployment
            | Self::DeploymentInProgress(_) => StatusCode::CONFLICT,
            Self::InvalidName(_)
            | Self::InvalidRepoUrl(_)
            | Self::NotGitSourced(_)
            | Self::InvalidPublishDir(_)
            | Self::InvalidRunConfig(_)
            | Self::InvalidBuildConfig(_)
            | Self::NoContainer(_)
            | Self::MissingUploadFile => StatusCode::BAD_REQUEST,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Git(_) | Self::ContainerStartFailed(_) | Self::ContainerUnavailable(_) => {
                StatusCode::BAD_GATEWAY
            }
            Self::Zip(_) | Self::Multipart(_) | Self::Io(_) | Self::Json(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        tracing::error!(error = %self, "request failed");
        (status, self.to_string()).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
