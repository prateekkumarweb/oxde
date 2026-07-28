#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod containers;
mod handlers;
mod host_stats;

use std::path::PathBuf;

use anyhow::Context;
use bollard::Docker;
use oxde_proto::{
    AGENT_GRPC_PORT,
    hub::v1::{
        HostStatsResult, PingRequest, SessionRequest, host_stats_result,
        hub_service_client::HubServiceClient, session_request, session_response,
    },
};
use tokio_stream::wrappers::ReceiverStream;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let data_dir = PathBuf::from(
        std::env::var("OXDE_AGENT_DATA_DIR").unwrap_or_else(|_| "agent-data".to_string()),
    );
    std::fs::create_dir_all(&data_dir)?;
    let data_dir = data_dir.canonicalize()?;

    let docker = containers::connect().context("failed to build Podman client")?;
    containers::ensure_network(&docker)
        .await
        .context("failed to ensure the run-mode container network exists")?;

    let hub_addr = format!("http://127.0.0.1:{AGENT_GRPC_PORT}");
    tracing::info!(hub_addr, "dialing hub");

    let mut client = HubServiceClient::connect(hub_addr).await?;
    let response = client.ping(PingRequest {}).await?.into_inner();
    tracing::info!(hub_version = response.version, "hub answered ping");

    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let outbound = ReceiverStream::new(rx);
    let mut inbound = client.session(outbound).await?.into_inner();

    tracing::info!("session opened, waiting for hub requests");
    while let Some(message) = inbound.message().await? {
        let Some(payload) = message.payload else {
            continue;
        };
        let request_id = message.request_id;
        let data_dir = data_dir.clone();
        let docker = docker.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            handle_request(&data_dir, &docker, request_id, payload, tx).await;
        });
    }

    Ok(())
}

async fn handle_request(
    data_dir: &std::path::Path,
    docker: &Docker,
    request_id: u64,
    payload: session_response::Payload,
    tx: tokio::sync::mpsc::Sender<SessionRequest>,
) {
    match payload {
        session_response::Payload::GetHostStats(_) => {
            let result = match host_stats::collect(data_dir).await {
                Ok(stats) => host_stats_result::Result::Ok(stats),
                Err(err) => host_stats_result::Result::Error(err.to_string()),
            };
            send(
                &tx,
                request_id,
                session_request::Payload::HostStatsResult(HostStatsResult {
                    result: Some(result),
                }),
            )
            .await;
        }
        // These three stream their own replies (log chunks, then a final
        // result) directly, rather than returning one value for this
        // function to wrap and send.
        session_response::Payload::StartRunContainer(req) => {
            handlers::start_run_container(docker, req, request_id, &tx).await;
        }
        session_response::Payload::RunBuildCommand(req) => {
            handlers::run_build_command(docker, req, request_id, &tx).await;
        }
        session_response::Payload::StreamContainerLogs(req) => {
            handlers::stream_container_logs(docker, req, request_id, &tx).await;
        }
        session_response::Payload::StopAndRemoveContainer(req) => {
            let result = handlers::stop_and_remove_container(docker, req).await;
            send(
                &tx,
                request_id,
                session_request::Payload::StopAndRemoveContainerResult(result),
            )
            .await;
        }
        session_response::Payload::IsContainerRunning(req) => {
            let result = handlers::is_container_running(docker, req).await;
            send(
                &tx,
                request_id,
                session_request::Payload::IsContainerRunningResult(result),
            )
            .await;
        }
        session_response::Payload::ContainerStats(req) => {
            let result = handlers::container_stats(docker, req).await;
            send(
                &tx,
                request_id,
                session_request::Payload::ContainerStatsResult(result),
            )
            .await;
        }
        session_response::Payload::GetContainerIp(req) => {
            let result = handlers::get_container_ip(docker, req).await;
            send(
                &tx,
                request_id,
                session_request::Payload::GetContainerIpResult(result),
            )
            .await;
        }
        // Exercised only by oxde-hub's own AgentLink tests, which play the
        // agent role directly against the proto types - the real agent
        // never needs to answer these.
        session_response::Payload::EchoUpload(_)
        | session_response::Payload::EchoStreamRequest(_) => {
            tracing::warn!(
                request_id,
                "agent received an echo-only request it can't answer"
            );
        }
    }
}

async fn send(
    tx: &tokio::sync::mpsc::Sender<SessionRequest>,
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
