use toasty::Deferred;

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
    #[has_many]
    pub app_permissions: Deferred<Vec<AppPermission>>,
    #[has_many]
    pub api_tokens: Deferred<Vec<ApiToken>>,
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
    #[has_many]
    pub permissions: Deferred<Vec<AppPermission>>,
    #[has_many]
    pub deployments: Deferred<Vec<Deployment>>,
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
    #[belongs_to(key = app_id, references = id)]
    pub app: Deferred<App>,
    #[belongs_to(key = user_id, references = id)]
    pub user: Deferred<User>,
}

#[derive(Debug, Clone, toasty::Model)]
pub struct ApiToken {
    #[key]
    #[auto]
    pub id: i64,
    #[index]
    pub user_id: i64,
    pub name: String,
    /// Non-secret lookup key; the secret half is only ever stored hashed
    /// (`token_hash`), since a hash can't be looked up by equality.
    #[unique]
    pub token_id: String,
    pub token_hash: String,
    pub expires_at: i64,
    /// Revocation flips this rather than deleting the row.
    pub revoked: bool,
    pub created_at: i64,
    pub updated_at: i64,
    #[belongs_to(key = user_id, references = id)]
    pub user: Deferred<User>,
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
    #[belongs_to(key = app_id, references = id)]
    pub app: Deferred<App>,
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
