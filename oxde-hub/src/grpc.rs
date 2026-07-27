use std::pin::Pin;

use oxde_proto::{
    AGENT_GRPC_PORT,
    hub::v1::{
        PingRequest, PingResponse, SessionRequest, SessionResponse,
        hub_service_server::{HubService, HubServiceServer},
    },
};
use tokio_stream::{Stream, StreamExt, wrappers::ReceiverStream};
use tonic::{Request, Response, Status, Streaming, transport::Server};

use crate::agent_link::AgentLink;

struct Hub {
    agent_link: AgentLink,
}

#[tonic::async_trait]
impl HubService for Hub {
    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    type SessionStream = Pin<Box<dyn Stream<Item = Result<SessionResponse, Status>> + Send>>;

    async fn session(
        &self,
        request: Request<Streaming<SessionRequest>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let mut incoming = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        self.agent_link.set_outbound(tx).await;

        let agent_link = self.agent_link.clone();
        tokio::spawn(async move {
            while let Ok(Some(message)) = incoming.message().await {
                agent_link.resolve(message).await;
            }
            agent_link.clear_outbound().await;
        });

        let stream = ReceiverStream::new(rx).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Spawned alongside the HTTP server so an agent can dial in and confirm
/// it's talking to a live hub.
pub async fn serve(agent_link: AgentLink) -> anyhow::Result<()> {
    let addr = ([0, 0, 0, 0], AGENT_GRPC_PORT).into();
    tracing::info!("hub gRPC listener started on {addr}");
    Server::builder()
        .add_service(HubServiceServer::new(Hub { agent_link }))
        .serve(addr)
        .await?;
    Ok(())
}
