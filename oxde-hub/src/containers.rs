use std::{path::Path, time::Duration};

use bytes::Bytes;
use oxde_models::{EnvVar, RunConfig};
use oxde_proto::hub::v1::{
    AgentErrorKind, CommandOutput, ContainerStatsRequest, GetContainerIpRequest,
    IsContainerRunningRequest, RunBuildCommandRequest, StartRunContainerRequest,
    StopAndRemoveContainerRequest, StreamContainerLogsRequest, command_output, command_result,
    container_stats_result, get_container_ip_result, is_container_running_result, session_request,
    session_response,
};
use serde::Serialize;
use tokio::sync::mpsc;
use ts_rs::TS;

use crate::{
    agent_link::AgentLink,
    deployment_logs::{LogPump, LogTarget},
    error::{AppError, AppResult},
};

/// Deterministic, so startup reconciliation can look containers up by name.
/// The only place this name is computed - agent RPCs take it as a field
/// rather than recomputing it.
pub fn container_name(app_id: &str, deployment_id: &str) -> String {
    format!("oxde-{app_id}-{deployment_id}")
}

fn agent_error(err: oxde_proto::hub::v1::AgentError) -> AppError {
    match AgentErrorKind::try_from(err.kind).unwrap_or(AgentErrorKind::Unspecified) {
        AgentErrorKind::StartFailed => AppError::ContainerStartFailed(err.message),
        AgentErrorKind::CommandFailed => AppError::CommandFailed(err.message),
        AgentErrorKind::Unavailable | AgentErrorKind::Unspecified => {
            AppError::ContainerUnavailable(err.message)
        }
    }
}

fn command_result_to_app_result(result: oxde_proto::hub::v1::CommandResult) -> AppResult<()> {
    match result.result {
        Some(command_result::Result::Ok(_)) => Ok(()),
        Some(command_result::Result::Error(err)) => Err(agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty command result".to_string(),
        )),
    }
}

/// Drains a streamed `CommandOutput` reply - log chunks (fed to `pump`, if
/// given) then one final result. `extract` picks the expected outer
/// `SessionRequest::Payload` variant; a mismatch ends the stream like an
/// early close.
async fn drain_command_output(
    agent_link: &AgentLink,
    request_id: u64,
    mut rx: mpsc::Receiver<session_request::Payload>,
    extract: impl Fn(session_request::Payload) -> Option<CommandOutput>,
    mut pump: Option<LogPump>,
) -> AppResult<()> {
    let result = loop {
        let Some(payload) = rx.recv().await else {
            break None;
        };
        let Some(output) = extract(payload) else {
            break None;
        };
        match output.output {
            Some(command_output::Output::Log(chunk)) => {
                if let Some(pump) = &mut pump {
                    pump.push(Bytes::from(chunk.data));
                }
            }
            Some(command_output::Output::Result(result)) => break Some(result),
            None => break None,
        }
    };
    agent_link.end_stream(request_id).await;

    result.map_or_else(
        || {
            Err(AppError::AgentError(
                "agent closed the stream without a final result".to_string(),
            ))
        },
        command_result_to_app_result,
    )
}

/// Idempotent - see the agent's `RunContainerConfig` doc comment for why.
/// Streams install output live into `install_log_target`, if given.
pub async fn start(
    agent_link: &AgentLink,
    deployment_id: &str,
    container_name: &str,
    run_config: &RunConfig,
    env_vars: &[EnvVar],
    install_timeout: Duration,
    install_log_target: Option<LogTarget>,
) -> AppResult<()> {
    let payload = session_response::Payload::StartRunContainer(StartRunContainerRequest {
        deployment_id: deployment_id.to_string(),
        container_name: container_name.to_string(),
        run_config_json: serde_json::to_string(run_config)?,
        env_vars_json: serde_json::to_string(env_vars)?,
        install_timeout_secs: install_timeout.as_secs(),
    });
    let (request_id, rx) = agent_link.call_streaming_reply(payload).await?;
    let pump = install_log_target.and_then(LogPump::try_new);
    drain_command_output(
        agent_link,
        request_id,
        rx,
        |payload| match payload {
            session_request::Payload::StartRunContainerResult(output) => Some(output),
            _ => None,
        },
        pump,
    )
    .await
}

/// Bundles a build command's config to stay under clippy's arg-count lint.
pub struct BuildCommandConfig<'a> {
    pub image: &'a str,
    pub command: &'a str,
    pub env_vars: &'a [EnvVar],
    pub timeout: Duration,
}

/// Streams build output live into `build_log_target`, if given.
pub async fn run_build_command(
    agent_link: &AgentLink,
    container_name: &str,
    checkout_dir: &Path,
    config: BuildCommandConfig<'_>,
    build_log_target: Option<LogTarget>,
) -> AppResult<()> {
    let payload = session_response::Payload::RunBuildCommand(RunBuildCommandRequest {
        container_name: container_name.to_string(),
        checkout_dir: checkout_dir.display().to_string(),
        image: config.image.to_string(),
        command: config.command.to_string(),
        env_vars_json: serde_json::to_string(config.env_vars)?,
        timeout_secs: config.timeout.as_secs(),
    });
    let (request_id, rx) = agent_link.call_streaming_reply(payload).await?;
    let pump = build_log_target.and_then(LogPump::try_new);
    drain_command_output(
        agent_link,
        request_id,
        rx,
        |payload| match payload {
            session_request::Payload::RunBuildCommandResult(output) => Some(output),
            _ => None,
        },
        pump,
    )
    .await
}

/// Missing (already gone) counts as success. `is_install` targets
/// `container_name`'s install-phase container instead - only the agent
/// knows that suffix.
pub async fn stop_and_remove(
    agent_link: &AgentLink,
    container_name: &str,
    is_install: bool,
) -> AppResult<()> {
    let payload =
        session_response::Payload::StopAndRemoveContainer(StopAndRemoveContainerRequest {
            container_name: container_name.to_string(),
            is_install,
        });
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::StopAndRemoveContainerResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to StopAndRemoveContainer with the wrong payload type".to_string(),
        ));
    };
    command_result_to_app_result(result)
}

pub async fn is_running(agent_link: &AgentLink, container_name: &str) -> AppResult<bool> {
    let payload = session_response::Payload::IsContainerRunning(IsContainerRunningRequest {
        container_name: container_name.to_string(),
    });
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::IsContainerRunningResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to IsContainerRunning with the wrong payload type".to_string(),
        ));
    };
    match result.result {
        Some(is_container_running_result::Result::Ok(running)) => Ok(running),
        Some(is_container_running_result::Result::Error(err)) => Err(agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty IsContainerRunning result".to_string(),
        )),
    }
}

#[derive(Serialize, Clone, Copy, TS)]
#[ts(export)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
}

impl From<oxde_proto::hub::v1::ContainerStats> for ContainerStats {
    fn from(stats: oxde_proto::hub::v1::ContainerStats) -> Self {
        Self {
            cpu_percent: stats.cpu_percent,
            memory_usage_bytes: stats.memory_usage_bytes,
            memory_limit_bytes: stats.memory_limit_bytes,
        }
    }
}

pub async fn stats(agent_link: &AgentLink, container_name: &str) -> AppResult<ContainerStats> {
    let payload = session_response::Payload::ContainerStats(ContainerStatsRequest {
        container_name: container_name.to_string(),
    });
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::ContainerStatsResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to ContainerStats with the wrong payload type".to_string(),
        ));
    };
    match result.result {
        Some(container_stats_result::Result::Ok(stats)) => Ok(stats.into()),
        Some(container_stats_result::Result::Error(err)) => Err(agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty ContainerStats result".to_string(),
        )),
    }
}

pub async fn container_ip(
    agent_link: &AgentLink,
    container_name: &str,
) -> AppResult<Option<String>> {
    let payload = session_response::Payload::GetContainerIp(GetContainerIpRequest {
        container_name: container_name.to_string(),
    });
    let reply = agent_link.call_chunked(vec![payload]).await?;
    let session_request::Payload::GetContainerIpResult(result) = reply else {
        return Err(AppError::AgentError(
            "agent replied to GetContainerIp with the wrong payload type".to_string(),
        ));
    };
    match result.result {
        Some(get_container_ip_result::Result::Ok(ip)) => Ok(ip.ip),
        Some(get_container_ip_result::Result::Error(err)) => Err(agent_error(err)),
        None => Err(AppError::AgentError(
            "agent sent an empty GetContainerIp result".to_string(),
        )),
    }
}

/// Spawns a detached pump appending `container_name`'s logs to `target`'s
/// file until the container stops or is removed - a redeploy's
/// `stop_and_remove` on the old container ends this for free. A no-op if
/// this deployment already has a pump.
pub fn spawn_run_log_pump(agent_link: &AgentLink, container_name: &str, target: LogTarget) {
    let agent_link = agent_link.clone();
    let container_name = container_name.to_string();
    tokio::spawn(async move {
        let payload = session_response::Payload::StreamContainerLogs(StreamContainerLogsRequest {
            container_name,
            follow: true,
        });
        let (request_id, mut rx) = match agent_link.call_streaming_reply(payload).await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(error = %err, "failed to start run-log stream");
                return;
            }
        };
        let Some(mut pump) = LogPump::try_new(target) else {
            agent_link.end_stream(request_id).await;
            return;
        };
        while let Some(payload) = rx.recv().await {
            let session_request::Payload::StreamContainerLogsChunk(chunk) = payload else {
                break;
            };
            let is_final = chunk.is_final;
            pump.push(Bytes::from(chunk.data));
            if is_final {
                break;
            }
        }
        agent_link.end_stream(request_id).await;
    });
}
