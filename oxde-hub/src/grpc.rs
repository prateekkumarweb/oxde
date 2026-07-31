use std::pin::Pin;

use oxde_proto::{
    AGENT_GRPC_PORT,
    hub::v1::{
        PingRequest, PingResponse, SessionRequest, SessionResponse,
        hub_service_server::{HubService, HubServiceServer},
    },
};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt, wrappers::ReceiverStream};
use tonic::{
    Request, Response, Status, Streaming,
    transport::{Identity, Server, ServerTlsConfig},
};

use crate::agent_link::AgentLink;

struct Hub {
    agent_link: AgentLink,
    /// Signaled once per `Session` connection, after `set_outbound` so the
    /// agent is already callable - lets `main.rs` re-run agent-dependent
    /// reconciliation without `grpc.rs` needing to know what that means.
    connected_tx: mpsc::Sender<()>,
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
        // A full connected_tx channel just means a reconciliation run is
        // already pending or in progress - nothing new to signal.
        let _ = self.connected_tx.try_send(());

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
/// it's talking to a live hub. `connected_tx` is signaled on every
/// connection (see `Hub::session`). `identity` is the self-signed cert
/// the agent pins by fingerprint (see `agent_tls.rs`).
pub async fn serve(
    agent_link: AgentLink,
    connected_tx: mpsc::Sender<()>,
    identity: Identity,
) -> anyhow::Result<()> {
    let addr = ([0, 0, 0, 0], AGENT_GRPC_PORT).into();
    tracing::info!("hub gRPC listener started on {addr}");
    Server::builder()
        .tls_config(ServerTlsConfig::new().identity(identity))?
        .add_service(HubServiceServer::new(Hub {
            agent_link,
            connected_tx,
        }))
        .serve(addr)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use oxde_proto::hub::v1::{
        Chunk, EchoStreamRequest, EchoUploadResult, hub_service_client::HubServiceClient,
        session_request, session_response,
    };
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use tokio::sync::{mpsc, oneshot};
    use tonic::transport::{Certificate, Channel, ClientTlsConfig, server::TcpIncoming};

    use super::*;

    const TEST_TLS_SUBJECT: &str = "oxde-agent-link-test";

    /// Starts a real, TLS-enabled hub gRPC server on an ephemeral port
    /// over its own fresh `AgentLink`, returning that link (for the test
    /// to drive as the hub side), the address to dial, and the self-signed
    /// cert a fake agent needs to trust to connect.
    fn spawn_test_hub() -> (AgentLink, String, Certificate) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let agent_link = AgentLink::new();
        let incoming = TcpIncoming::bind("127.0.0.1:0".parse().expect("addr")).expect("bind");
        let addr = incoming.local_addr().expect("local_addr");

        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec![TEST_TLS_SUBJECT.to_string()])
                .expect("generate test cert");
        let ca_cert = Certificate::from_pem(cert.pem());
        let identity = Identity::from_pem(cert.pem(), signing_key.serialize_pem());

        let server_link = agent_link.clone();
        let (connected_tx, _) = mpsc::channel(1);
        tokio::spawn(async move {
            Server::builder()
                .tls_config(ServerTlsConfig::new().identity(identity))
                .expect("tls_config")
                .add_service(HubServiceServer::new(Hub {
                    agent_link: server_link,
                    connected_tx,
                }))
                .serve_with_incoming(incoming)
                .await
                .expect("serve");
        });

        (agent_link, format!("https://{addr}"), ca_cert)
    }

    /// Trusts exactly the cert `spawn_test_hub` generated - fingerprint
    /// pinning itself is oxde-agent's concern, not tested here.
    async fn connect_test_client(
        hub_addr: String,
        ca_cert: Certificate,
    ) -> HubServiceClient<Channel> {
        let channel = Channel::from_shared(hub_addr)
            .expect("uri")
            .tls_config(
                ClientTlsConfig::new()
                    .ca_certificate(ca_cert)
                    .domain_name(TEST_TLS_SUBJECT),
            )
            .expect("tls_config")
            .connect()
            .await
            .expect("connect");
        HubServiceClient::new(channel)
    }

    /// Proves `call_chunked`'s "hub sends N, agent replies once" half: a
    /// fake agent accumulates `EchoUpload` chunks until one is marked
    /// final, then replies with the total byte count received.
    #[tokio::test]
    async fn call_chunked_delivers_every_chunk_before_the_single_reply() {
        let (agent_link, hub_addr, ca_cert) = spawn_test_hub();
        let (ready_tx, ready_rx) = oneshot::channel();

        tokio::spawn(async move {
            let mut client = connect_test_client(hub_addr, ca_cert).await;
            let (tx, rx) = mpsc::channel(16);
            let mut inbound = client
                .session(tokio_stream::wrappers::ReceiverStream::new(rx))
                .await
                .expect("open session")
                .into_inner();
            let _ = ready_tx.send(());

            let mut received = Vec::new();
            while let Some(message) = inbound.message().await.expect("recv") {
                let Some(session_response::Payload::EchoUpload(chunk)) = message.payload else {
                    panic!("expected an EchoUpload chunk");
                };
                received.extend_from_slice(&chunk.data);
                if chunk.is_final {
                    tx.send(SessionRequest {
                        request_id: message.request_id,
                        payload: Some(session_request::Payload::EchoUploadResult(
                            EchoUploadResult {
                                bytes_received: received.len() as u64,
                            },
                        )),
                    })
                    .await
                    .expect("send reply");
                    // Keeps `tx`/`client` alive rather than dropping them
                    // (which would cancel the request stream) right after
                    // the send call returns - `send` completing only means
                    // the reply was queued, not that it's been flushed to
                    // the wire yet.
                    break;
                }
            }
            std::future::pending::<()>().await;
        });
        ready_rx.await.expect("fake agent ready");

        let result = agent_link
            .call_chunked(vec![
                session_response::Payload::EchoUpload(Chunk {
                    data: b"hello ".to_vec(),
                    is_final: false,
                }),
                session_response::Payload::EchoUpload(Chunk {
                    data: b"world".to_vec(),
                    is_final: true,
                }),
            ])
            .await
            .expect("call_chunked");

        let session_request::Payload::EchoUploadResult(result) = result else {
            panic!("expected an EchoUploadResult");
        };
        assert_eq!(result.bytes_received, "hello world".len() as u64);
    }

    /// Proves `call_streaming_reply`'s "hub sends one, agent replies with
    /// N" half: a fake agent answers a single `EchoStreamRequest` with a
    /// run of `EchoStreamChunk` replies, the last marked final.
    #[tokio::test]
    async fn call_streaming_reply_delivers_every_chunk_the_agent_sends() {
        let (agent_link, hub_addr, ca_cert) = spawn_test_hub();
        let (ready_tx, ready_rx) = oneshot::channel();

        tokio::spawn(async move {
            let mut client = connect_test_client(hub_addr, ca_cert).await;
            let (tx, rx) = mpsc::channel(16);
            let mut inbound = client
                .session(tokio_stream::wrappers::ReceiverStream::new(rx))
                .await
                .expect("open session")
                .into_inner();
            let _ = ready_tx.send(());

            let message = inbound
                .message()
                .await
                .expect("recv")
                .expect("some message");
            let Some(session_response::Payload::EchoStreamRequest(request)) = message.payload
            else {
                panic!("expected an EchoStreamRequest");
            };
            for i in 0..request.chunk_count {
                tx.send(SessionRequest {
                    request_id: message.request_id,
                    payload: Some(session_request::Payload::EchoStreamChunk(Chunk {
                        data: vec![u8::try_from(i).expect("chunk_count fits in a byte")],
                        is_final: i + 1 == request.chunk_count,
                    })),
                })
                .await
                .expect("send chunk");
            }
            // See the matching comment in the `call_chunked` test above -
            // dropping `tx`/`client` immediately after the last send would
            // race the send actually reaching the wire.
            std::future::pending::<()>().await;
        });
        ready_rx.await.expect("fake agent ready");

        let (request_id, mut rx) = agent_link
            .call_streaming_reply(session_response::Payload::EchoStreamRequest(
                EchoStreamRequest { chunk_count: 3 },
            ))
            .await
            .expect("call_streaming_reply");

        let mut received = Vec::new();
        loop {
            let payload = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("no timeout")
                .expect("channel open");
            let session_request::Payload::EchoStreamChunk(chunk) = payload else {
                panic!("expected an EchoStreamChunk");
            };
            received.push(chunk.data[0]);
            if chunk.is_final {
                break;
            }
        }
        agent_link.end_stream(request_id).await;

        assert_eq!(received, vec![0, 1, 2]);
    }
}
