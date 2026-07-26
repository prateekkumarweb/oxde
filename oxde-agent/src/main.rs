use oxde_proto::{AGENT_GRPC_PORT, hub::v1::hub_service_client::HubServiceClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let hub_addr = format!("http://127.0.0.1:{AGENT_GRPC_PORT}");
    tracing::info!(hub_addr, "dialing hub");

    let mut client = HubServiceClient::connect(hub_addr).await?;
    let response = client
        .ping(oxde_proto::hub::v1::PingRequest {})
        .await?
        .into_inner();
    tracing::info!(hub_version = response.version, "hub answered ping");

    Ok(())
}
