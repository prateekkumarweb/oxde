use std::{future::Future, path::Path, time::Duration};

use bollard::Docker;
use bytes::Bytes;
use futures_util::StreamExt;
use oxde_models::{EnvVar, RunConfig};
use oxde_proto::hub::v1::{
    AgentError, AgentErrorKind, Chunk, CommandOutput, CommandResult, ContainerIp,
    ContainerStats as ProtoContainerStats, ContainerStatsRequest, ContainerStatsResult, Empty,
    GetContainerIpRequest, GetContainerIpResult, IsContainerRunningRequest,
    IsContainerRunningResult, RunBuildCommandRequest, SessionRequest, StartRunContainerRequest,
    StopAndRemoveContainerRequest, StreamContainerLogsRequest, command_output, command_result,
    container_stats_result, get_container_ip_result, is_container_running_result, session_request,
};
use tokio::sync::mpsc;

use crate::containers::{self, StartError};

const fn agent_error(kind: AgentErrorKind, message: String) -> AgentError {
    AgentError {
        kind: kind as i32,
        message,
    }
}

const fn command_result_ok() -> CommandResult {
    CommandResult {
        result: Some(command_result::Result::Ok(Empty {})),
    }
}

const fn command_result_err(kind: AgentErrorKind, message: String) -> CommandResult {
    CommandResult {
        result: Some(command_result::Result::Error(agent_error(kind, message))),
    }
}

fn start_error_to_command_result(err: StartError) -> CommandResult {
    match err {
        StartError::Unavailable(msg) => command_result_err(AgentErrorKind::Unavailable, msg),
        StartError::StartFailed(msg) => command_result_err(AgentErrorKind::StartFailed, msg),
        StartError::CommandFailed(msg) => command_result_err(AgentErrorKind::CommandFailed, msg),
    }
}

async fn send(
    tx: &mpsc::Sender<SessionRequest>,
    request_id: u64,
    payload: session_request::Payload,
) {
    let _ = tx
        .send(SessionRequest {
            request_id,
            payload: Some(payload),
        })
        .await;
}

async fn send_final(
    tx: &mpsc::Sender<SessionRequest>,
    request_id: u64,
    wrap: fn(CommandOutput) -> session_request::Payload,
    result: CommandResult,
) {
    let output = CommandOutput {
        output: Some(command_output::Output::Result(result)),
    };
    send(tx, request_id, wrap(output)).await;
}

/// Drives `run`, forwarding everything it writes to its log sink on to the
/// hub as `CommandOutput::Log` chunks, then sends one final
/// `CommandOutput::Result`.
async fn stream_command<F>(
    tx: &mpsc::Sender<SessionRequest>,
    request_id: u64,
    wrap: fn(CommandOutput) -> session_request::Payload,
    run: impl FnOnce(mpsc::Sender<Bytes>) -> F,
) where
    F: Future<Output = Result<(), StartError>>,
{
    let (log_tx, mut log_rx) = mpsc::channel::<Bytes>(64);
    let forward_tx = tx.clone();
    let forward_handle = tokio::spawn(async move {
        while let Some(bytes) = log_rx.recv().await {
            let output = CommandOutput {
                output: Some(command_output::Output::Log(Chunk {
                    data: bytes.to_vec(),
                    is_final: false,
                })),
            };
            if forward_tx
                .send(SessionRequest {
                    request_id,
                    payload: Some(wrap(output)),
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let result = run(log_tx).await;
    drop(forward_handle.await);

    let final_result = match result {
        Ok(()) => command_result_ok(),
        Err(err) => start_error_to_command_result(err),
    };
    send_final(tx, request_id, wrap, final_result).await;
}

/// Runs install (if any) then creates/starts the run-mode container,
/// streaming install output live to the hub.
pub async fn start_run_container(
    docker: &Docker,
    req: StartRunContainerRequest,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) {
    let wrap = session_request::Payload::StartRunContainerResult;
    let run_config: RunConfig = match serde_json::from_str(&req.run_config_json) {
        Ok(config) => config,
        Err(err) => {
            send_final(
                tx,
                request_id,
                wrap,
                command_result_err(AgentErrorKind::StartFailed, err.to_string()),
            )
            .await;
            return;
        }
    };
    let env_vars: Vec<EnvVar> = match serde_json::from_str(&req.env_vars_json) {
        Ok(vars) => vars,
        Err(err) => {
            send_final(
                tx,
                request_id,
                wrap,
                command_result_err(AgentErrorKind::StartFailed, err.to_string()),
            )
            .await;
            return;
        }
    };
    let install_timeout = Duration::from_secs(req.install_timeout_secs);

    stream_command(tx, request_id, wrap, |log_tx| {
        containers::start(
            docker,
            &req.container_name,
            Path::new(&req.deployment_files_dir),
            containers::RunContainerConfig {
                image: run_config.image.image_tag(),
                install_command: run_config.install_command.as_deref(),
                start_command: &run_config.start_command,
                env_vars: &env_vars,
                install_timeout,
            },
            Some(log_tx),
        )
    })
    .await;
}

/// Runs a git build-mode deploy's build command to completion, streaming
/// its output live to the hub.
pub async fn run_build_command(
    docker: &Docker,
    req: RunBuildCommandRequest,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) {
    let wrap = session_request::Payload::RunBuildCommandResult;
    let env_vars: Vec<EnvVar> = match serde_json::from_str(&req.env_vars_json) {
        Ok(vars) => vars,
        Err(err) => {
            send_final(
                tx,
                request_id,
                wrap,
                command_result_err(AgentErrorKind::StartFailed, err.to_string()),
            )
            .await;
            return;
        }
    };
    let timeout = Duration::from_secs(req.timeout_secs);

    stream_command(tx, request_id, wrap, |log_tx| {
        containers::run_build_command(
            docker,
            &req.container_name,
            Path::new(&req.checkout_dir),
            containers::CommandExec {
                image: &req.image,
                command: &req.command,
                env_vars: &env_vars,
                timeout,
                log_sink: Some(log_tx),
            },
        )
    })
    .await;
}

pub async fn stop_and_remove_container(
    docker: &Docker,
    req: StopAndRemoveContainerRequest,
) -> CommandResult {
    let name = if req.is_install {
        containers::install_container_name(&req.container_name)
    } else {
        req.container_name
    };
    match containers::stop_and_remove(docker, &name).await {
        Ok(()) => command_result_ok(),
        Err(msg) => command_result_err(AgentErrorKind::Unavailable, msg),
    }
}

pub async fn is_container_running(
    docker: &Docker,
    req: IsContainerRunningRequest,
) -> IsContainerRunningResult {
    let result = match containers::is_running(docker, &req.container_name).await {
        Ok(running) => is_container_running_result::Result::Ok(running),
        Err(msg) => is_container_running_result::Result::Error(agent_error(
            AgentErrorKind::Unavailable,
            msg,
        )),
    };
    IsContainerRunningResult {
        result: Some(result),
    }
}

pub async fn container_stats(docker: &Docker, req: ContainerStatsRequest) -> ContainerStatsResult {
    let result = match containers::stats(docker, &req.container_name).await {
        Ok(stats) => container_stats_result::Result::Ok(ProtoContainerStats {
            cpu_percent: stats.cpu_percent,
            memory_usage_bytes: stats.memory_usage_bytes,
            memory_limit_bytes: stats.memory_limit_bytes,
        }),
        Err(msg) => {
            container_stats_result::Result::Error(agent_error(AgentErrorKind::Unavailable, msg))
        }
    };
    ContainerStatsResult {
        result: Some(result),
    }
}

pub async fn get_container_ip(docker: &Docker, req: GetContainerIpRequest) -> GetContainerIpResult {
    let result = match containers::container_ip(docker, &req.container_name).await {
        Ok(ip) => get_container_ip_result::Result::Ok(ContainerIp { ip }),
        Err(msg) => {
            get_container_ip_result::Result::Error(agent_error(AgentErrorKind::Unavailable, msg))
        }
    };
    GetContainerIpResult {
        result: Some(result),
    }
}

/// Tails `req.container_name`'s logs to the hub, ending with an
/// `is_final = true` chunk once the stream ends.
pub async fn stream_container_logs(
    docker: &Docker,
    req: StreamContainerLogsRequest,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) {
    let mut source = containers::logs(docker, &req.container_name, req.follow);
    while let Some(chunk) = source.next().await {
        let Ok(chunk) = chunk else { break };
        send(
            tx,
            request_id,
            session_request::Payload::StreamContainerLogsChunk(Chunk {
                data: chunk.to_vec(),
                is_final: false,
            }),
        )
        .await;
    }
    send(
        tx,
        request_id,
        session_request::Payload::StreamContainerLogsChunk(Chunk {
            data: Vec::new(),
            is_final: true,
        }),
    )
    .await;
}
