#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod auth;
mod config;
mod containers;
mod dashboard_assets;
mod error;
mod git_fetch;
mod models;
mod reverse_proxy;
mod routes;
mod state;
mod storage;
mod zip_extract;

use anyhow::Context;

use crate::{
    config::Config,
    error::AppResult,
    models::{App, DeploymentStatus},
    state::{AppState, AppStateLimits},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::load().context("failed to load configuration")?;
    let docker = containers::connect().context("failed to build Podman client")?;

    // Must be absolute: it's used as a bind-mount source for run-mode
    // containers, which Podman resolves against its own process, not
    // OxDe's - a relative `data_dir` (e.g. the default `./data`) would
    // resolve to the wrong place there even though plain `std::fs` calls
    // tolerate it fine.
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("failed to create data dir at {}", config.data_dir.display()))?;
    let data_dir = config.data_dir.canonicalize().with_context(|| {
        format!(
            "failed to resolve data dir at {}",
            config.data_dir.display()
        )
    })?;

    let state = AppState::new(
        data_dir,
        AppStateLimits {
            max_upload_bytes: config.max_upload_bytes,
            max_uncompressed_bytes: config.max_uncompressed_bytes,
            base_domain: config.base_domain.clone(),
            git_fetch_timeout_secs: config.git_fetch_timeout_secs,
            install_timeout_secs: config.install_timeout_secs,
        },
        docker,
        reverse_proxy::new_client(),
    );

    std::fs::create_dir_all(state.apps_dir())
        .context("failed to create apps dir under data dir")?;
    storage::sweep_tmp_dir(&state).context("failed to sweep tmp directory on startup")?;
    containers::ensure_network(state.docker())
        .await
        .context("failed to ensure the run-mode container network exists")?;
    fail_pending_deployments(&state).await;
    reconcile_run_mode_containers(&state).await;

    let app = routes::build_router(state, &config.admin_username, &config.admin_password);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!(addr = ?listener.local_addr()?, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// A deployment left `Pending` was mid-clone/install/activate when the
/// server stopped - its container may or may not have survived the
/// restart, so rather than guess, it's marked `Failed` and any lingering
/// install container is force-removed.
async fn fail_pending_deployments(state: &AppState) {
    let apps = match storage::list_apps(state) {
        Ok(apps) => apps,
        Err(err) => {
            tracing::error!(error = %err, "failed to list apps for pending-deployment reconciliation");
            return;
        }
    };

    for app in apps {
        let deployments = match storage::list_deployments(state, &app.name) {
            Ok(deployments) => deployments,
            Err(err) => {
                tracing::error!(error = %err, app = app.name, "failed to list deployments for pending-deployment reconciliation");
                continue;
            }
        };

        for deployment in deployments {
            if !matches!(deployment.status, DeploymentStatus::Pending) {
                continue;
            }

            tracing::warn!(
                app = app.name,
                deployment = deployment.id,
                "marking deployment interrupted by server restart as failed"
            );

            if let Some(container_name) = &deployment.container_name {
                let install_name = containers::install_container_name(container_name);
                if let Err(err) = containers::stop_and_remove(state.docker(), &install_name).await {
                    tracing::error!(
                        error = %err,
                        app = app.name,
                        deployment = deployment.id,
                        "failed to remove install container during reconciliation"
                    );
                }
            }

            if let Err(err) = storage::fail_git_deployment(
                state,
                &app.name,
                &deployment.id,
                "interrupted by server restart",
            ) {
                tracing::error!(
                    error = %err,
                    app = app.name,
                    deployment = deployment.id,
                    "failed to mark interrupted deployment as failed"
                );
            }
        }
    }
}

/// Podman containers survive an `OxDe` restart (the restart policy doesn't
/// depend on this process), so recovery here means starting any run-mode
/// app whose container isn't already running - `containers::start` is
/// idempotent, so this is safe to call unconditionally. One app's
/// reconciliation failure is logged and skipped rather than aborting
/// startup, so a single broken run-mode app can't take down unrelated apps.
async fn reconcile_run_mode_containers(state: &AppState) {
    let apps = match storage::list_apps(state) {
        Ok(apps) => apps,
        Err(err) => {
            tracing::error!(error = %err, "failed to list apps for startup reconciliation");
            return;
        }
    };

    for app in apps {
        if let Err(err) = reconcile_app(state, &app).await {
            tracing::error!(
                error = %err,
                app = app.name,
                "failed to reconcile run-mode container on startup"
            );
        }
    }
}

async fn reconcile_app(state: &AppState, app: &App) -> AppResult<()> {
    let Some(run_config) = app.run_config() else {
        return Ok(());
    };
    let Some(deployment_id) = storage::active_deployment_id(state, &app.name) else {
        return Ok(());
    };
    let deployment = storage::get_deployment(state, &app.name, &deployment_id)?;
    let Some(container_name) = &deployment.container_name else {
        return Ok(());
    };

    let checkout_dir = state.deployment_files_dir(&app.name, &deployment_id);
    tracing::info!(app = app.name, "starting run-mode container on startup");
    containers::start(
        state.docker(),
        container_name,
        &checkout_dir,
        run_config,
        std::time::Duration::from_secs(state.install_timeout_secs()),
    )
    .await
}
