use oxde_proto::{
    AGENT_GRPC_PORT,
    hub::v1::{
        PingRequest, PingResponse,
        hub_service_server::{HubService, HubServiceServer},
    },
};
use tonic::{Request, Response, Status, transport::Server};

struct Hub;

#[tonic::async_trait]
impl HubService for Hub {
    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}

/// Spawned alongside the HTTP server so an agent can dial in and confirm
/// it's talking to a live hub.
pub async fn serve() -> anyhow::Result<()> {
    let addr = ([0, 0, 0, 0], AGENT_GRPC_PORT).into();
    tracing::info!("hub gRPC listener started on {addr}");
    Server::builder()
        .add_service(HubServiceServer::new(Hub))
        .serve(addr)
        .await?;
    Ok(())
}
