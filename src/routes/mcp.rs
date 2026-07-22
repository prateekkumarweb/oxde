use std::sync::Arc;

use axum::{Router, extract::Request, http::request::Parts, middleware, response::Response};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    accounts::AccountRole,
    auth::{ApiUser, CurrentUser},
    authz, containers, deployment_logs,
    models::{self, AppSource, EnvVar},
    routes::api,
    state::AppState,
    storage,
};

#[derive(Deserialize, JsonSchema)]
struct AppNameParams {
    app_name: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateAppParams {
    name: String,
    #[serde(default)]
    source: AppSource,
    #[serde(default)]
    env_vars: Vec<EnvVar>,
}

#[derive(Deserialize, JsonSchema)]
struct DeploymentParams {
    app_name: String,
    deployment_id: String,
}

/// `jiff::Timestamp` isn't `JsonSchema`, so plain text, not `rmcp::Json`.
fn json_text<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|err| err.to_string())
}

/// `rmcp` exposes the whole `Parts`, not its nested `.extensions` directly.
fn current_user_from(parts: &Parts) -> Result<CurrentUser, String> {
    parts
        .extensions
        .get::<CurrentUser>()
        .cloned()
        .ok_or_else(|| "not authenticated".to_string())
}

#[derive(Clone)]
pub struct OxdeMcpServer {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl OxdeMcpServer {
    fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl OxdeMcpServer {
    #[tool(description = "List apps the caller has access to")]
    async fn list_apps(&self, Extension(parts): Extension<Parts>) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let apps = storage::list_apps(&self.state)
            .await
            .map_err(|e| e.to_string())?;
        let mut views = Vec::with_capacity(apps.len());
        for app in apps {
            if matches!(current_user.role, AccountRole::Admin)
                || app.has_permission(&current_user.username, models::PermissionLevel::Read)
            {
                views.push(api::app_view(&self.state, app).await);
            }
        }
        json_text(&views)
    }

    #[tool(description = "Get one app by name")]
    async fn get_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppNameParams>,
    ) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Read)
            .map_err(|e| e.to_string())?;
        json_text(&api::app_view(&self.state, app).await)
    }

    #[tool(description = "Create a new app")]
    async fn create_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<CreateAppParams>,
    ) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let creator = matches!(current_user.role, AccountRole::Member)
            .then_some(current_user.username.as_str());
        let app = storage::create_app(
            &self.state,
            &params.name,
            params.source,
            params.env_vars,
            creator,
        )
        .await
        .map_err(|e| e.to_string())?;
        json_text(&api::app_view(&self.state, app).await)
    }

    #[tool(description = "Delete an app and all its deployments")]
    async fn delete_app(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppNameParams>,
    ) -> Result<(), String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Write)
            .map_err(|e| e.to_string())?;
        api::delete_app_with_containers(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())
    }

    #[tool(description = "List an app's deployments")]
    async fn list_deployments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppNameParams>,
    ) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Read)
            .map_err(|e| e.to_string())?;
        let active_id = storage::active_deployment_id(&self.state, &params.app_name).await;
        let mut views = Vec::new();
        for deployment in storage::list_deployments(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?
        {
            views.push(api::deployment_view(&self.state, active_id.as_deref(), deployment).await);
        }
        json_text(&views)
    }

    #[tool(description = "Trigger a git deployment for an app")]
    async fn deploy_from_git(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<AppNameParams>,
    ) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Write)
            .map_err(|e| e.to_string())?;
        let deployment = api::start_git_deployment(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        json_text(&deployment)
    }

    #[tool(description = "Activate a deployment, making it the one served live")]
    async fn activate_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeploymentParams>,
    ) -> Result<(), String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Write)
            .map_err(|e| e.to_string())?;
        api::activate_with_containers(&self.state, &params.app_name, &params.deployment_id)
            .await
            .map_err(|e| e.to_string())
    }

    #[tool(description = "Delete a non-active deployment")]
    async fn delete_deployment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeploymentParams>,
    ) -> Result<(), String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Write)
            .map_err(|e| e.to_string())?;
        api::delete_deployment_with_containers(&self.state, &params.app_name, &params.deployment_id)
            .await
            .map_err(|e| e.to_string())
    }

    #[tool(description = "Get a snapshot of a deployment's clone/install/build/run logs")]
    async fn get_deployment_logs(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeploymentParams>,
    ) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Read)
            .map_err(|e| e.to_string())?;
        storage::get_deployment(&self.state, &params.app_name, &params.deployment_id)
            .await
            .map_err(|e| e.to_string())?;

        let dir = self.state.deployment_dir(&app.id, &params.deployment_id);
        let kind = match self.state.log_registry().active(&params.deployment_id) {
            Some((kind, _)) => kind,
            None => deployment_logs::resolve_terminal_phase(&dir)
                .ok_or("no logs recorded for this deployment yet")?,
        };
        let backlog = deployment_logs::read_backlog(&dir, kind).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&backlog).into_owned())
    }

    #[tool(description = "Get live CPU/memory stats for a running run-mode deployment")]
    async fn get_deployment_stats(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(params): Parameters<DeploymentParams>,
    ) -> Result<String, String> {
        let current_user = current_user_from(&parts)?;
        let app = storage::get_app(&self.state, &params.app_name)
            .await
            .map_err(|e| e.to_string())?;
        authz::check_app_permission(&current_user, &app, models::PermissionLevel::Read)
            .map_err(|e| e.to_string())?;
        let deployment =
            storage::get_deployment(&self.state, &params.app_name, &params.deployment_id)
                .await
                .map_err(|e| e.to_string())?;
        let Some(container_name) = deployment.container_name else {
            return json_text(&Option::<containers::ContainerStats>::None);
        };
        if !containers::is_running(self.state.docker(), &container_name)
            .await
            .map_err(|e| e.to_string())?
        {
            return json_text(&Option::<containers::ContainerStats>::None);
        }
        let stats = containers::stats(self.state.docker(), &container_name)
            .await
            .map_err(|e| e.to_string())?;
        json_text(&Some(stats))
    }
}

#[tool_handler(router = self.tool_router, instructions = "Manage OxDe apps and deployments.")]
impl ServerHandler for OxdeMcpServer {}

/// Extends the default DNS-rebinding allowlist with `base_domain`, or `rmcp` rejects real requests.
fn allowed_hosts(base_domain: &str) -> Vec<String> {
    let mut hosts = StreamableHttpServerConfig::default().allowed_hosts;
    hosts.push(base_domain.to_string());
    hosts
}

/// Stashes the resolved `CurrentUser` where `current_user_from` reads it back.
async fn mcp_auth(current_user: ApiUser, mut request: Request, next: middleware::Next) -> Response {
    request.extensions_mut().insert(current_user.0);
    next.run(request).await
}

pub fn router(state: &AppState) -> Router<AppState> {
    let mcp_state = state.clone();
    let mut config = StreamableHttpServerConfig::default();
    config.stateful_mode = false;
    config.json_response = true;
    config.allowed_hosts = allowed_hosts(state.base_domain());

    let service = StreamableHttpService::new(
        move || Ok(OxdeMcpServer::new(mcp_state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state.clone(), mcp_auth))
}
