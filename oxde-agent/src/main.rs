mod host_stats;

use std::path::PathBuf;

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
        let tx = tx.clone();
        tokio::spawn(async move {
            handle_request(&data_dir, request_id, payload, tx).await;
        });
    }

    Ok(())
}

async fn handle_request(
    data_dir: &std::path::Path,
    request_id: u64,
    payload: session_response::Payload,
    tx: tokio::sync::mpsc::Sender<SessionRequest>,
) {
    let response_payload = match payload {
        session_response::Payload::GetHostStats(_) => {
            let result = match host_stats::collect(data_dir).await {
                Ok(stats) => host_stats_result::Result::Ok(stats),
                Err(err) => host_stats_result::Result::Error(err.to_string()),
            };
            session_request::Payload::HostStatsResult(HostStatsResult {
                result: Some(result),
            })
        }
    };

    drop(
        tx.send(SessionRequest {
            request_id,
            payload: Some(response_payload),
        })
        .await,
    );
}
