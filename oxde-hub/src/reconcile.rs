use oxde_models::{App, DeploymentStatus};

use crate::{containers, deployment_logs, error::AppResult, state::AppState, storage};

/// Every agent-dependent reconciliation step, run each time the agent's
/// `Session` connects (see `grpc.rs`) rather than once at a fixed point in
/// hub startup - handles both a hub that starts before its agent and an
/// agent that restarts later, without a human intervening. Each step below
/// is independently idempotent, so re-running all of them on a reconnect
/// that didn't need it is safe, just a bit of wasted work.
pub async fn on_agent_connected(state: &AppState) {
    fail_pending_deployments(state).await;
    reconcile_run_mode_containers(state).await;
    storage::sweep_agent_orphaned_deployment_dirs(state).await;
}

/// A `Pending` deployment never got to `Ready`/`Failed` before the hub or
/// agent stopped. Its container may or may not have survived, so rather
/// than guess, it's marked `Failed` and any lingering install container is
/// force-removed.
async fn fail_pending_deployments(state: &AppState) {
    let apps = match storage::list_apps(state).await {
        Ok(apps) => apps,
        Err(err) => {
            tracing::error!(error = %err, "failed to list apps for pending-deployment reconciliation");
            return;
        }
    };

    for app in apps {
        let deployments = match storage::list_deployments(state, &app.id).await {
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
                "marking interrupted deployment as failed"
            );

            if let Some(container_name) = &deployment.container_name
                && let Err(err) =
                    containers::stop_and_remove(&state.agent_link(), container_name, true).await
            {
                tracing::error!(
                    error = %err,
                    app = app.name,
                    deployment = deployment.id,
                    "failed to remove install container during reconciliation"
                );
            }

            if let Err(err) = storage::fail_git_deployment(
                state,
                &app.id,
                &deployment.id,
                "interrupted by server restart",
            )
            .await
            {
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

/// Podman containers survive a hub/agent restart (the restart policy
/// doesn't depend on either process), so recovery here means starting any
/// run-mode app whose container isn't already running - `containers::start`
/// is idempotent, so this is safe to call unconditionally. One app's
/// reconciliation failure is logged and skipped rather than aborting the
/// rest.
async fn reconcile_run_mode_containers(state: &AppState) {
    let apps = match storage::list_apps(state).await {
        Ok(apps) => apps,
        Err(err) => {
            tracing::error!(error = %err, "failed to list apps for run-mode reconciliation");
            return;
        }
    };

    for app in apps {
        if let Err(err) = reconcile_app(state, &app).await {
            tracing::error!(
                error = %err,
                app = app.name,
                "failed to reconcile run-mode container"
            );
        }
    }
}

async fn reconcile_app(state: &AppState, app: &App) -> AppResult<()> {
    let Some(run_config) = app.run_config() else {
        return Ok(());
    };
    let Some(deployment_id) = storage::active_deployment_id(state, &app.id).await else {
        return Ok(());
    };
    let deployment = storage::get_deployment(state, &app.id, &deployment_id).await?;
    let Some(container_name) = &deployment.container_name else {
        return Ok(());
    };

    tracing::info!(app = app.name, "starting run-mode container");
    containers::start(
        &state.agent_link(),
        &deployment_id,
        container_name,
        run_config,
        &app.env_vars,
        std::time::Duration::from_secs(state.install_timeout_secs()),
        None, // install already ran on a previous startup
    )
    .await?;

    // `spawn_run_log_pump` is itself a no-op if a pump for this deployment
    // is already registered (see `LogPump::try_new`), so calling it again
    // on a reconnect that didn't need it just opens and immediately closes
    // a redundant stream rather than duplicating output.
    containers::spawn_run_log_pump(
        &state.agent_link(),
        container_name,
        deployment_logs::LogTarget {
            path: state.deployment_log_path(&app.id, &deployment_id, deployment_logs::LogKind::Run),
            deployment_id: deployment_id.clone(),
            kind: deployment_logs::LogKind::Run,
            registry: state.log_registry().clone(),
        },
    );
    Ok(())
}
