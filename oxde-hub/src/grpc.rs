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

use crate::{api_tokens, state::AppState, storage};

struct Hub {
    state: AppState,
    /// Signaled once per `Session` connection, after the agent is
    /// registered so it's already callable - lets `main.rs` re-run
    /// agent-dependent reconciliation without `grpc.rs` needing to know
    /// what that means.
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
        let host = authenticate(&self.state, &request).await?;
        let peer_ip = request.remote_addr().map(|addr| addr.ip().to_string());
        let registry = self.state.agent_registry().clone();
        let Some(agent_link) = registry.connect(host.id) else {
            return Err(Status::already_exists(
                "this host already has an active session",
            ));
        };

        let mut incoming = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        agent_link.set_outbound(tx).await;
        // A full connected_tx channel just means a reconciliation run is
        // already pending or in progress - nothing new to signal.
        let _ = self.connected_tx.try_send(());

        if let Err(err) = storage::touch_host_last_seen(&self.state, host.id, peer_ip).await {
            tracing::warn!(error = ?err, host_id = host.id, "failed to record host last_seen_at");
        }

        tokio::spawn(async move {
            while let Ok(Some(message)) = incoming.message().await {
                agent_link.resolve(message).await;
            }
            registry.disconnect(host.id);
        });

        let stream = ReceiverStream::new(rx).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Verifies the `authorization: Bearer <token>` metadata against a `Host`
/// row, the same scheme `find_user_by_api_token` uses for API tokens.
async fn authenticate(
    state: &AppState,
    request: &Request<Streaming<SessionRequest>>,
) -> Result<oxde_db::models::Host, Status> {
    let header = request
        .metadata()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Status::unauthenticated("missing authorization metadata"))?;
    let bearer = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| Status::unauthenticated("malformed authorization metadata"))?;
    let (token_id, secret) = api_tokens::parse_bearer_value(bearer)
        .ok_or_else(|| Status::unauthenticated("malformed agent token"))?;

    storage::find_host_by_token(state, token_id, secret)
        .await
        .map_err(|err| {
            tracing::error!(error = ?err, "failed to look up host by token");
            Status::internal("failed to authenticate agent")
        })?
        .ok_or_else(|| Status::unauthenticated("unknown or revoked agent token"))
}

/// Spawned alongside the HTTP server so an agent can dial in and confirm
/// it's talking to a live hub. `connected_tx` is signaled on every
/// connection (see `Hub::session`). `identity` is the self-signed cert
/// the agent pins by fingerprint (see `agent_tls.rs`).
pub async fn serve(
    state: AppState,
    connected_tx: mpsc::Sender<()>,
    identity: Identity,
) -> anyhow::Result<()> {
    let addr = ([0, 0, 0, 0], AGENT_GRPC_PORT).into();
    tracing::info!("hub gRPC listener started on {addr}");
    Server::builder()
        .tls_config(ServerTlsConfig::new().identity(identity))?
        .add_service(HubServiceServer::new(Hub {
            state,
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
    use tokio_stream::wrappers::ReceiverStream;
    use tonic::transport::{Certificate, Channel, ClientTlsConfig, server::TcpIncoming};

    use super::*;
    use crate::{
        agent_link::AgentRegistry,
        state::{AppState, AppStateLimits},
        storage,
    };

    const TEST_TLS_SUBJECT: &str = "oxde-agent-link-test";

    /// A fresh `AppState` over its own tempdir, so tests never share state.
    async fn test_state(label: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "oxde-test-grpc-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(dir.join("apps")).expect("create apps dir");
        std::fs::create_dir_all(dir.join("tmp")).expect("create tmp dir");
        let db = oxde_db::connect(&dir).await.expect("connect test database");
        oxde_db::apply_migrations(&db)
            .await
            .expect("apply test database migrations");
        AppState::new(
            dir,
            AppStateLimits {
                max_upload_bytes: 10_000,
                max_uncompressed_bytes: 10_000,
                base_domain: "localhost".to_string(),
                git_fetch_timeout_secs: 60,
                install_timeout_secs: 300,
                build_timeout_secs: 300,
                api_token_max_expiry_days: 30,
                enable_mcp: false,
            },
            crate::reverse_proxy::new_client(),
            db,
            AgentRegistry::new(),
        )
    }

    /// Starts a real, TLS-enabled hub gRPC server with a `Host` row a fake
    /// agent can authenticate as. Returns the `AppState` (fetch the
    /// connected `AgentLink` via `.agent_registry().for_host(host_id)`), the
    /// dial address, the cert to trust, the host's id, and its plaintext
    /// token.
    async fn spawn_test_hub() -> (AppState, String, Certificate, i64, String) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let state = test_state("hub").await;
        let (host, token) = storage::create_host(&state, "test-agent")
            .await
            .expect("create_host");
        let incoming = TcpIncoming::bind("127.0.0.1:0".parse().expect("addr")).expect("bind");
        let addr = incoming.local_addr().expect("local_addr");

        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec![TEST_TLS_SUBJECT.to_string()])
                .expect("generate test cert");
        let ca_cert = Certificate::from_pem(cert.pem());
        let identity = Identity::from_pem(cert.pem(), signing_key.serialize_pem());

        let server_state = state.clone();
        let (connected_tx, _) = mpsc::channel(1);
        tokio::spawn(async move {
            Server::builder()
                .tls_config(ServerTlsConfig::new().identity(identity))
                .expect("tls_config")
                .add_service(HubServiceServer::new(Hub {
                    state: server_state,
                    connected_tx,
                }))
                .serve_with_incoming(incoming)
                .await
                .expect("serve");
        });

        (state, format!("https://{addr}"), ca_cert, host.id, token)
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

    /// Attaches `token` as `authorization: Bearer <token>` metadata, the
    /// same way the real agent authenticates on `Session`.
    fn session_request(
        outbound: ReceiverStream<SessionRequest>,
        token: &str,
    ) -> Request<ReceiverStream<SessionRequest>> {
        let mut request = Request::new(outbound);
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}")
                .parse()
                .expect("valid header value"),
        );
        request
    }

    /// Proves `call_chunked`'s "hub sends N, agent replies once" half: a
    /// fake agent accumulates `EchoUpload` chunks until one is marked
    /// final, then replies with the total byte count received.
    #[tokio::test]
    async fn call_chunked_delivers_every_chunk_before_the_single_reply() {
        let (state, hub_addr, ca_cert, host_id, token) = spawn_test_hub().await;
        let (ready_tx, ready_rx) = oneshot::channel();

        tokio::spawn(async move {
            let mut client = connect_test_client(hub_addr, ca_cert).await;
            let (tx, rx) = mpsc::channel(16);
            let mut inbound = client
                .session(session_request(ReceiverStream::new(rx), &token))
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

        let agent_link = state.agent_registry().for_host(host_id);
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
        let (state, hub_addr, ca_cert, host_id, token) = spawn_test_hub().await;
        let (ready_tx, ready_rx) = oneshot::channel();

        tokio::spawn(async move {
            let mut client = connect_test_client(hub_addr, ca_cert).await;
            let (tx, rx) = mpsc::channel(16);
            let mut inbound = client
                .session(session_request(ReceiverStream::new(rx), &token))
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

        let agent_link = state.agent_registry().for_host(host_id);
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

    #[tokio::test]
    async fn session_rejects_an_invalid_token() {
        let (_state, hub_addr, ca_cert, _host_id, _token) = spawn_test_hub().await;
        let mut client = connect_test_client(hub_addr, ca_cert).await;
        let (_tx, rx) = mpsc::channel(16);

        let err = client
            .session(session_request(ReceiverStream::new(rx), "wrong-token"))
            .await
            .expect_err("an invalid token must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// Proves the duplicate-connect decision: a second `Session` for the
    /// same already-connected host is rejected, not silently allowed to
    /// take over.
    #[tokio::test]
    async fn session_rejects_a_second_connection_for_an_already_connected_host() {
        let (_state, hub_addr, ca_cert, _host_id, token) = spawn_test_hub().await;

        let mut first_client = connect_test_client(hub_addr.clone(), ca_cert.clone()).await;
        let (_first_tx, first_rx) = mpsc::channel(16);
        // Resolves only once `Hub::session` has returned its stream, so
        // the host is already registered before the second connect races it.
        let _first_session = first_client
            .session(session_request(ReceiverStream::new(first_rx), &token))
            .await
            .expect("open first session");

        let mut second_client = connect_test_client(hub_addr, ca_cert).await;
        let (_second_tx, second_rx) = mpsc::channel(16);
        let err = second_client
            .session(session_request(ReceiverStream::new(second_rx), &token))
            .await
            .expect_err("a second connection for the same host must be rejected");
        assert_eq!(err.code(), tonic::Code::AlreadyExists);
    }
}
