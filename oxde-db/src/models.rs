#[derive(Debug, Clone, toasty::Model)]
pub struct User {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub username: String,
    pub password_hash: String,
    /// `"admin"` or `"member"`, see `accounts::AccountRole` in the `oxde`
    /// crate. Stored as `String` rather than a native enum column -
    /// simplest thing the ORM is known to support well.
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, toasty::Model)]
pub struct App {
    #[key]
    #[auto]
    pub id: uuid::Uuid,
    #[unique]
    pub name: String,
    /// Serialized `AppSource` (the `oxde` crate's type).
    pub source_json: String,
    /// Serialized `Vec<EnvVar>` (the `oxde` crate's type) - always
    /// read/written as a whole list, never filtered by individual key.
    pub env_vars_json: String,
    pub active_deployment_id: Option<uuid::Uuid>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, toasty::Model)]
pub struct AppPermission {
    #[key]
    #[auto]
    pub id: i64,
    #[index]
    pub app_id: uuid::Uuid,
    #[index]
    pub user_id: i64,
    pub level: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    Read,
    Write,
}

impl PermissionLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

impl std::str::FromStr for PermissionLevel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            other => Err(format!("invalid permission level: {other}")),
        }
    }
}

#[derive(Debug, Clone, toasty::Model)]
pub struct Deployment {
    #[key]
    #[auto]
    pub id: uuid::Uuid,
    #[index]
    pub app_id: uuid::Uuid,
    pub created_at: i64,
    pub original_filename: Option<String>,
    pub upload_size_bytes: i64,
    pub git_info_json: Option<String>,
    pub build_info_json: Option<String>,
    pub container_name: Option<String>,
    pub status: String,
    pub failure_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentState {
    Pending,
    Ready,
    Failed,
}

impl DeploymentState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for DeploymentState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            other => Err(format!("invalid deployment state: {other}")),
        }
    }
}
