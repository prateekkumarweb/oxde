#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod auth;
mod config;
mod error;
mod git_fetch;
mod models;
mod routes;
mod state;
mod storage;
mod zip_extract;

use anyhow::Context;

use crate::{config::Config, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::load().context("failed to load configuration")?;
    let state = AppState::new(
        config.data_dir.clone(),
        config.max_upload_bytes,
        config.max_uncompressed_bytes,
        config.base_domain.clone(),
        config.git_fetch_timeout_secs,
    );

    std::fs::create_dir_all(state.apps_dir()).with_context(|| {
        format!(
            "failed to create apps dir under {}",
            config.data_dir.display()
        )
    })?;
    storage::sweep_tmp_dir(&state).context("failed to sweep tmp directory on startup")?;

    let app = routes::build_router(state, &config.admin_username, &config.admin_password);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!(addr = ?listener.local_addr()?, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}
