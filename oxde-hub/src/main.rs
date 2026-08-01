#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod accounts;
mod agent_fs;
mod agent_link;
mod agent_tls;
mod api_tokens;
mod auth;
mod authz;
mod containers;
mod dashboard_assets;
mod deployment_logs;
mod error;
mod git_fetch;
mod grpc;
mod host_stats;
mod reconcile;
mod reverse_proxy;
mod routes;
mod state;
mod storage;
mod tls;
mod zip_extract;

use std::net::SocketAddr;

use anyhow::Context;
use oxde_config::TlsConfig;
use oxde_db::models::User;

use crate::{
    accounts::AccountRole,
    state::{AppState, AppStateLimits},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install the rustls crypto provider"))?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = oxde_config::load_oxde_config().context("failed to load configuration")?;

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

    let agent_tls = agent_tls::load_or_generate(&data_dir)
        .context("failed to load or generate the hub<->agent TLS certificate")?;
    tracing::info!(
        fingerprint = agent_tls.fingerprint_hex,
        "hub<->agent gRPC certificate fingerprint - set hub_tls_fingerprint in oxde-agent.toml on \
         an agent connecting from a different host to pin it"
    );

    let db = oxde_db::connect(&data_dir)
        .await
        .context("failed to open database")?;
    oxde_db::apply_migrations(&db)
        .await
        .context("failed to apply pending database migrations")?;

    let state = AppState::new(
        data_dir,
        AppStateLimits {
            max_upload_bytes: config.max_upload_bytes,
            max_uncompressed_bytes: config.max_uncompressed_bytes,
            base_domain: config.base_domain.clone(),
            git_fetch_timeout_secs: config.git_fetch_timeout_secs,
            install_timeout_secs: config.install_timeout_secs,
            build_timeout_secs: config.build_timeout_secs,
            api_token_max_expiry_days: config.api_token_max_expiry_days,
            enable_mcp: config.enable_mcp,
        },
        reverse_proxy::new_client(),
        db,
        agent_link::AgentRegistry::new(),
    );

    bootstrap_admin(&state, &config.admin_username, &config.admin_password)
        .await
        .context("failed to bootstrap admin user")?;

    std::fs::create_dir_all(state.apps_dir())
        .context("failed to create apps dir under data dir")?;
    storage::sweep_tmp_dir(&state).context("failed to sweep tmp directory on startup")?;
    storage::sweep_orphaned_dirs(&state)
        .await
        .context("failed to sweep orphaned app/deployment directories on startup")?;

    spawn_grpc_and_reconciliation(&state, agent_tls.identity);
    auth::spawn_login_attempts_sweeper(state.clone());

    let app = routes::build_router(state);

    match &config.tls {
        TlsConfig::Off => {
            let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.http_port)).await?;
            let addr = listener.local_addr()?;
            tracing::info!("OxDe server started, listening on http://{addr}");
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;
        }
        TlsConfig::Manual {
            cert_path,
            key_path,
        } => {
            let tls_config = tls::build_rustls_config(cert_path, key_path).await?;
            tls::spawn_reload_watcher(tls_config.clone(), cert_path.clone(), key_path.clone());

            let addr = SocketAddr::from(([0, 0, 0, 0], config.https_port));
            tracing::info!("OxDe server started, listening on https://{addr}");
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .with_context(|| {
                    format!(
                        "failed to bind HTTPS port {} - if this is a permission error, grant \
                         CAP_NET_BIND_SERVICE or run as root",
                        config.https_port
                    )
                })?;
        }
    }
    Ok(())
}

/// Spawns the gRPC listener and, driven off its connection signal, the
/// agent-dependent reconciliation loop (see `reconcile::on_agent_connected`).
fn spawn_grpc_and_reconciliation(state: &AppState, agent_tls_identity: tonic::transport::Identity) {
    let (agent_connected_tx, mut agent_connected_rx) = tokio::sync::mpsc::channel(1);
    let grpc_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) = grpc::serve(grpc_state, agent_connected_tx, agent_tls_identity).await {
            tracing::error!(error = ?err, "hub gRPC listener stopped");
        }
    });
    let reconcile_state = state.clone();
    tokio::spawn(async move {
        while agent_connected_rx.recv().await.is_some() {
            reconcile::on_agent_connected(&reconcile_state).await;
        }
    });
}

/// Re-evaluated on every startup, not just once: if no `Admin` currently
/// exists in `users`, one is created from `oxde.toml`'s
/// `admin_username`/`admin_password`. Once at least one `Admin` exists,
/// those config values are ignored entirely - but the check is "does an
/// admin exist right now," not a one-time flag, so if every `Admin` were
/// ever deleted, the config file becomes the recovery path again on the
/// next restart rather than a permanent lockout.
async fn bootstrap_admin(
    state: &AppState,
    admin_username: &str,
    admin_password: &str,
) -> anyhow::Result<()> {
    let mut db = state.db().clone();
    let admin_exists = User::all()
        .filter(User::fields().role().eq(AccountRole::Admin.as_str()))
        .first()
        .exec(&mut db)
        .await?
        .is_some();
    if admin_exists {
        return Ok(());
    }

    accounts::validate_username(admin_username)
        .with_context(|| format!("invalid admin_username in config: {admin_username}"))?;
    accounts::validate_password(admin_password)
        .map_err(|err| anyhow::anyhow!(err.to_string()))
        .context("invalid admin_password in config")?;
    let password_hash =
        accounts::hash_password(admin_password).map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let now = accounts::now_epoch_secs();

    User::create()
        .username(admin_username)
        .password_hash(password_hash)
        .role(AccountRole::Admin.as_str())
        .created_at(now)
        .updated_at(now)
        .exec(&mut db)
        .await?;

    tracing::info!(
        username = admin_username,
        "bootstrapped admin user from config"
    );
    Ok(())
}
