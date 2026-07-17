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
    #[error("invalid publish dir: {0}")]
    InvalidPublishDir(String),
    #[error("git fetch failed: {0}")]
    Git(String),
    #[error("upload too large")]
    TooLarge,
    #[error("missing 'file' field in upload")]
    MissingUploadFile,
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("template error: {0}")]
    Template(#[from] askama::Error),
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
            Self::AppAlreadyExists(_) | Self::DeleteActiveDeployment => StatusCode::CONFLICT,
            Self::InvalidName(_)
            | Self::InvalidRepoUrl(_)
            | Self::NotGitSourced(_)
            | Self::InvalidPublishDir(_)
            | Self::MissingUploadFile => StatusCode::BAD_REQUEST,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Git(_) => StatusCode::BAD_GATEWAY,
            Self::Zip(_) | Self::Template(_) | Self::Multipart(_) | Self::Io(_) | Self::Json(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        tracing::error!(error = %self, "request failed");
        (status, self.to_string()).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
