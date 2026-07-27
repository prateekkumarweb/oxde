#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub const AGENT_GRPC_PORT: u16 = 50051;

pub mod hub {
    #[allow(clippy::pedantic, clippy::nursery)]
    pub mod v1 {
        tonic::include_proto!("oxde.hub.v1");
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use tonic::{
        Request, Response, Status, Streaming,
        codegen::tokio_stream::Stream,
        transport::{Server, server::TcpIncoming},
    };

    use super::hub::v1::{
        PingRequest, PingResponse, SessionRequest, SessionResponse,
        hub_service_client::HubServiceClient,
        hub_service_server::{HubService, HubServiceServer},
    };

    struct TestHub;

    #[tonic::async_trait]
    impl HubService for TestHub {
        async fn ping(
            &self,
            _request: Request<PingRequest>,
        ) -> Result<Response<PingResponse>, Status> {
            Ok(Response::new(PingResponse {
                version: "test".to_string(),
            }))
        }

        type SessionStream = Pin<Box<dyn Stream<Item = Result<SessionResponse, Status>> + Send>>;

        async fn session(
            &self,
            _request: Request<Streaming<SessionRequest>>,
        ) -> Result<Response<Self::SessionStream>, Status> {
            Err(Status::unimplemented("not exercised by this test"))
        }
    }

    #[tokio::test]
    async fn ping_round_trips_over_a_real_connection() {
        let incoming = TcpIncoming::bind("127.0.0.1:0".parse().expect("addr")).expect("bind");
        let addr = incoming.local_addr().expect("local_addr");

        tokio::spawn(async move {
            Server::builder()
                .add_service(HubServiceServer::new(TestHub))
                .serve_with_incoming(incoming)
                .await
                .expect("serve");
        });

        let mut client = HubServiceClient::connect(format!("http://{addr}"))
            .await
            .expect("connect");
        let response = client.ping(PingRequest {}).await.expect("ping");
        assert_eq!(response.into_inner().version, "test");
    }
}
