#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod containers;
mod fs_ops;
mod handlers;
mod host_stats;
mod hub_tls;
mod layout;
mod zip_extract;

use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use anyhow::Context;
use bollard::Docker;
use oxde_config::AgentConfig;
use oxde_proto::hub::v1::{
    Chunk, HostStatsResult, PingRequest, SessionRequest, host_stats_result,
    hub_service_client::HubServiceClient, session_request, session_response,
};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{ClientTlsConfig, Endpoint};

/// In-flight chunked uploads, keyed by `request_id` - every
/// `UploadZipAndExtract` message sharing a `request_id` is a chunk of the
/// same transfer and gets routed here instead of spawning a fresh handler
/// per message.
type InFlightUploads = Arc<Mutex<HashMap<u64, mpsc::Sender<Chunk>>>>;

const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

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

    let config = oxde_config::load_agent_config().context("failed to load configuration")?;

    std::fs::create_dir_all(&config.data_dir)?;
    let data_dir = config.data_dir.canonicalize()?;
    std::fs::create_dir_all(layout::tmp_dir(&data_dir))?;

    let docker = containers::connect().context("failed to build Podman client")?;
    containers::ensure_network(&docker)
        .await
        .context("failed to ensure the run-mode container network exists")?;

    let hub_addr = format!("https://{}", config.hub_addr);

    // Every attempt after the first is a reconnect: a session that ends
    // (hub restart, network blip) is retried with backoff rather than
    // exiting the process. The delay resets once a session actually opens,
    // so a brief blip doesn't leave the agent waiting out a long backoff.
    let mut reconnect_delay = INITIAL_RECONNECT_DELAY;
    loop {
        if let Err(err) =
            run_session(&config, &hub_addr, &data_dir, &docker, &mut reconnect_delay).await
        {
            tracing::warn!(error = ?err, retry_in_secs = reconnect_delay.as_secs(), "hub session ended");
        }
        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
    }
}

/// One connect-and-serve attempt: dials the hub, opens a `Session`, and
/// dispatches requests until the stream ends or errors. `reconnect_delay`
/// is reset to the initial delay as soon as the session opens, so only
/// attempts that never even connect grow the backoff.
async fn run_session(
    config: &AgentConfig,
    hub_addr: &str,
    data_dir: &Path,
    docker: &Docker,
    reconnect_delay: &mut Duration,
) -> anyhow::Result<()> {
    tracing::info!(hub_addr, "dialing hub");

    let pinned_fingerprint =
        hub_tls::expected_fingerprint(data_dir, config.hub_tls_fingerprint.clone());
    let is_first_connect = pinned_fingerprint.is_none();
    let verifier = Arc::new(hub_tls::FingerprintVerifier::new(pinned_fingerprint));
    let channel = Endpoint::from_shared(hub_addr.to_string())?
        .tls_config_with_verifier(ClientTlsConfig::new(), verifier.clone())?
        .connect()
        .await?;
    if is_first_connect && let Some(fingerprint) = verifier.captured_fingerprint() {
        hub_tls::persist_fingerprint(data_dir, &fingerprint)
            .context("failed to persist the hub's TLS certificate fingerprint")?;
        tracing::warn!(
            fingerprint,
            "no pinned hub certificate fingerprint - trusted the one presented on this first \
             connect and pinned it for future connects"
        );
    }

    let mut client = HubServiceClient::new(channel);
    let response = client.ping(PingRequest {}).await?.into_inner();
    tracing::info!(hub_version = response.version, "hub answered ping");

    let (tx, rx) = mpsc::channel(16);
    let outbound = ReceiverStream::new(rx);
    let mut session_request = tonic::Request::new(outbound);
    session_request.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", config.agent_token)
            .parse()
            .context("agent_token isn't a valid gRPC metadata value")?,
    );
    let mut inbound = client.session(session_request).await?.into_inner();

    *reconnect_delay = INITIAL_RECONNECT_DELAY;
    tracing::info!("session opened, waiting for hub requests");

    let in_flight_uploads: InFlightUploads = Arc::new(Mutex::new(HashMap::new()));
    while let Some(message) = inbound.message().await? {
        let Some(payload) = message.payload else {
            continue;
        };
        let request_id = message.request_id;

        if let session_response::Payload::UploadZipAndExtract(req) = payload {
            route_upload_chunk(&in_flight_uploads, data_dir, request_id, req, tx.clone()).await;
            continue;
        }

        let data_dir = data_dir.to_path_buf();
        let docker = docker.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            handle_request(&data_dir, &docker, request_id, payload, tx).await;
        });
    }

    Ok(())
}

/// The first chunk for a `request_id` starts the upload's handler task and
/// registers a channel for it; every later chunk is forwarded into that
/// same channel instead of starting a second one.
async fn route_upload_chunk(
    in_flight: &InFlightUploads,
    data_dir: &std::path::Path,
    request_id: u64,
    req: oxde_proto::hub::v1::UploadZipAndExtractRequest,
    tx: mpsc::Sender<SessionRequest>,
) {
    let mut map = in_flight.lock().await;
    if let Some(sender) = map.get(&request_id) {
        let sender = sender.clone();
        drop(map);
        if let Some(chunk) = req.chunk {
            drop(sender.send(chunk).await);
        }
        return;
    }

    let (chunk_tx, chunk_rx) = mpsc::channel(16);
    map.insert(request_id, chunk_tx.clone());
    drop(map);
    if let Some(chunk) = req.chunk {
        drop(chunk_tx.send(chunk).await);
    }

    let data_dir = data_dir.to_path_buf();
    let in_flight = in_flight.clone();
    tokio::spawn(async move {
        handlers::upload_zip_and_extract(
            &data_dir,
            req.deployment_id,
            req.max_uncompressed_bytes,
            chunk_rx,
            request_id,
            &tx,
        )
        .await;
        in_flight.lock().await.remove(&request_id);
    });
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
            handlers::start_run_container(docker, data_dir, req, request_id, &tx).await;
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
        session_response::Payload::CreateDeploymentDir(req) => {
            let result = handlers::create_deployment_dir(data_dir, &req.deployment_id);
            send(
                &tx,
                request_id,
                session_request::Payload::CreateDeploymentDirResult(result),
            )
            .await;
        }
        session_response::Payload::DeleteDeploymentDir(req) => {
            let result = handlers::delete_deployment_dir(data_dir, &req.deployment_id);
            send(
                &tx,
                request_id,
                session_request::Payload::DeleteDeploymentDirResult(result),
            )
            .await;
        }
        session_response::Payload::ListDeploymentDirs(_) => {
            let result = handlers::list_deployment_dirs(data_dir);
            send(
                &tx,
                request_id,
                session_request::Payload::ListDeploymentDirsResult(result),
            )
            .await;
        }
        // Routed to `route_upload_chunk` in the main loop before reaching
        // this dispatch - never seen here.
        session_response::Payload::UploadZipAndExtract(_) => {}
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
