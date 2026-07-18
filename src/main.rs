#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod auth;
mod config;
mod containers;
mod error;
mod git_fetch;
mod models;
mod reverse_proxy;
mod routes;
mod state;
mod storage;
mod zip_extract;

use anyhow::Context;

use crate::{config::Config, models::AppSource, state::AppState};

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
        config.max_upload_bytes,
        config.max_uncompressed_bytes,
        config.base_domain.clone(),
        config.git_fetch_timeout_secs,
        docker,
        reverse_proxy::new_client(),
    );

    std::fs::create_dir_all(state.apps_dir())
        .context("failed to create apps dir under data dir")?;
    storage::sweep_tmp_dir(&state).context("failed to sweep tmp directory on startup")?;
    containers::ensure_network(state.docker())
        .await
        .context("failed to ensure the run-mode container network exists")?;
    reconcile_run_mode_containers(&state)
        .await
        .context("failed to reconcile run-mode containers on startup")?;

    let app = routes::build_router(state, &config.admin_username, &config.admin_password);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!(addr = ?listener.local_addr()?, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Podman containers survive an `OxDe` restart (the restart policy doesn't
/// depend on this process), so recovery here means starting only the
/// run-mode apps whose active deployment isn't already running - routing
/// itself needs no attaching, since it looks up a container's IP fresh on
/// every request rather than caching it.
async fn reconcile_run_mode_containers(state: &AppState) -> anyhow::Result<()> {
    for app in storage::list_apps(state).context("failed to list apps")? {
        let AppSource::Git(git_source) = &app.source else {
            continue;
        };
        let Some(run_config) = &git_source.run else {
            continue;
        };
        let Some(deployment_id) = storage::active_deployment_id(state, &app.name) else {
            continue;
        };
        let deployment = storage::get_deployment(state, &app.name, &deployment_id)
            .with_context(|| format!("failed to read active deployment for {}", app.name))?;
        let Some(container_name) = &deployment.container_name else {
            continue;
        };

        if containers::is_running(state.docker(), container_name)
            .await
            .with_context(|| format!("failed to check container status for {}", app.name))?
        {
            continue;
        }

        let checkout_dir = state
            .apps_dir()
            .join(&app.name)
            .join("deployments")
            .join(&deployment_id)
            .join("files");
        tracing::info!(app = app.name, "starting run-mode container on startup");
        containers::start(state.docker(), container_name, &checkout_dir, run_config)
            .await
            .with_context(|| format!("failed to start container for {}", app.name))?;
    }
    Ok(())
}
