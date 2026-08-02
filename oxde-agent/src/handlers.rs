use std::{future::Future, path::Path, time::Duration};

use bollard::Docker;
use bytes::Bytes;
use futures_util::StreamExt;
use oxde_models::{EnvVar, RunConfig};
use oxde_proto::hub::v1::{
    AgentError, AgentErrorKind, Chunk, CommandOutput, CommandResult,
    ContainerStats as ProtoContainerStats, ContainerStatsRequest, ContainerStatsResult,
    DeploymentIdList, Empty, HostStatsResult, IsContainerRunningRequest, IsContainerRunningResult,
    ListDeploymentDirsResult, RunBuildCommandRequest, SessionRequest, StartRunContainerRequest,
    StopAndRemoveContainerRequest, StreamContainerLogsRequest, UploadZipAndExtractResult,
    command_output, command_result, container_stats_result, host_stats_result,
    is_container_running_result, list_deployment_dirs_result, session_request,
    upload_zip_and_extract_result,
};
use tokio::sync::mpsc;

use crate::{
    containers::{self, StartError},
    fs_ops, layout,
};

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
    data_dir: &Path,
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
    let files_dir = layout::deployment_files_dir(data_dir, &req.deployment_id);

    stream_command(tx, request_id, wrap, |log_tx| {
        containers::start(
            docker,
            &req.container_name,
            &files_dir,
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

pub async fn get_host_stats(data_dir: &Path) -> HostStatsResult {
    let result = match crate::host_stats::collect(data_dir).await {
        Ok(stats) => host_stats_result::Result::Ok(stats),
        Err(err) => host_stats_result::Result::Error(err.to_string()),
    };
    HostStatsResult {
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

pub fn create_deployment_dir(data_dir: &Path, deployment_id: &str) -> CommandResult {
    match fs_ops::create_deployment_dir(data_dir, deployment_id) {
        Ok(()) => command_result_ok(),
        Err(msg) => command_result_err(AgentErrorKind::Unavailable, msg),
    }
}

pub fn delete_deployment_dir(data_dir: &Path, deployment_id: &str) -> CommandResult {
    match fs_ops::delete_deployment_dir(data_dir, deployment_id) {
        Ok(()) => command_result_ok(),
        Err(msg) => command_result_err(AgentErrorKind::Unavailable, msg),
    }
}

pub fn list_deployment_dirs(data_dir: &Path) -> ListDeploymentDirsResult {
    let result = match fs_ops::list_deployment_dirs(data_dir) {
        Ok(ids) => list_deployment_dirs_result::Result::Ok(DeploymentIdList {
            deployment_ids: ids,
        }),
        Err(msg) => list_deployment_dirs_result::Result::Error(agent_error(
            AgentErrorKind::Unavailable,
            msg,
        )),
    };
    ListDeploymentDirsResult {
        result: Some(result),
    }
}

/// Receives a chunked zip upload (chunks routed in by `main.rs`) and
/// extracts it into place before the final result is sent, matching every
/// other create path's fs-before-DB-row invariant.
pub async fn upload_zip_and_extract(
    data_dir: &Path,
    deployment_id: String,
    max_uncompressed_bytes: u64,
    mut chunk_rx: mpsc::Receiver<Chunk>,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) {
    let zip_path = layout::tmp_dir(data_dir).join(format!("upload-{deployment_id}.zip"));
    let result = match receive_zip(&zip_path, &mut chunk_rx).await {
        Ok(()) => {
            let data_dir = data_dir.to_path_buf();
            let deployment_id = deployment_id.clone();
            let zip_path = zip_path.clone();
            tokio::task::spawn_blocking(move || {
                fs_ops::extract_and_place(
                    &data_dir,
                    &deployment_id,
                    &zip_path,
                    max_uncompressed_bytes,
                )
            })
            .await
            .map_err(|err| err.to_string())
            .and_then(|inner| inner)
        }
        Err(err) => Err(err),
    };
    std::fs::remove_file(&zip_path).ok();

    let payload = match result {
        Ok(size) => upload_zip_and_extract_result::Result::ContentSizeBytes(size),
        Err(msg) => upload_zip_and_extract_result::Result::Error(agent_error(
            AgentErrorKind::CommandFailed,
            msg,
        )),
    };
    send(
        tx,
        request_id,
        session_request::Payload::UploadZipAndExtractResult(UploadZipAndExtractResult {
            result: Some(payload),
        }),
    )
    .await;
}

async fn receive_zip(zip_path: &Path, chunk_rx: &mut mpsc::Receiver<Chunk>) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(zip_path)
        .await
        .map_err(|err| err.to_string())?;
    while let Some(chunk) = chunk_rx.recv().await {
        file.write_all(&chunk.data)
            .await
            .map_err(|err| err.to_string())?;
        if chunk.is_final {
            return Ok(());
        }
    }
    Err("hub closed the upload stream before sending a final chunk".to_string())
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, time::SystemTime};

    use super::*;

    fn test_data_dir(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "oxde-agent-test-handlers-{label}-{}-{nanos}",
            std::process::id(),
        ));
        // main.rs creates tmp/ at agent startup in production.
        std::fs::create_dir_all(layout::tmp_dir(&dir)).expect("create tmp dir");
        dir
    }

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).expect("start_file");
            std::io::Write::write_all(&mut writer, contents).expect("write contents");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[tokio::test]
    async fn upload_zip_and_extract_assembles_chunks_and_extracts_them() {
        let data_dir = test_data_dir("ok");
        fs_ops::create_deployment_dir(&data_dir, "dep-1").expect("create deployment dir");

        let zip_bytes = build_zip(&[("index.html", b"hello")]);
        let (chunk_tx, chunk_rx) = mpsc::channel(4);
        chunk_tx
            .send(Chunk {
                data: zip_bytes[..3].to_vec(),
                is_final: false,
            })
            .await
            .expect("send first chunk");
        chunk_tx
            .send(Chunk {
                data: zip_bytes[3..].to_vec(),
                is_final: true,
            })
            .await
            .expect("send final chunk");
        drop(chunk_tx);

        let (tx, mut rx) = mpsc::channel(1);
        upload_zip_and_extract(&data_dir, "dep-1".to_string(), 10_000, chunk_rx, 7, &tx).await;

        let reply = rx.recv().await.expect("a reply was sent");
        let session_request::Payload::UploadZipAndExtractResult(result) =
            reply.payload.expect("payload")
        else {
            panic!("expected an UploadZipAndExtractResult");
        };
        assert_eq!(
            result.result,
            Some(upload_zip_and_extract_result::Result::ContentSizeBytes(5))
        );
        assert_eq!(
            std::fs::read_to_string(
                layout::deployment_files_dir(&data_dir, "dep-1").join("index.html")
            )
            .expect("read index.html"),
            "hello"
        );
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[tokio::test]
    async fn upload_zip_and_extract_reports_an_error_if_the_stream_ends_early() {
        let data_dir = test_data_dir("early-close");
        fs_ops::create_deployment_dir(&data_dir, "dep-1").expect("create deployment dir");

        let (chunk_tx, chunk_rx) = mpsc::channel(4);
        chunk_tx
            .send(Chunk {
                data: b"not a full zip".to_vec(),
                is_final: false,
            })
            .await
            .expect("send chunk");
        drop(chunk_tx);

        let (tx, mut rx) = mpsc::channel(1);
        upload_zip_and_extract(&data_dir, "dep-1".to_string(), 10_000, chunk_rx, 7, &tx).await;

        let reply = rx.recv().await.expect("a reply was sent");
        let session_request::Payload::UploadZipAndExtractResult(result) =
            reply.payload.expect("payload")
        else {
            panic!("expected an UploadZipAndExtractResult");
        };
        assert!(
            matches!(
                result.result,
                Some(upload_zip_and_extract_result::Result::Error(_))
            ),
            "a stream that never sends a final chunk must be reported as an error"
        );
        std::fs::remove_dir_all(&data_dir).ok();
    }
}
