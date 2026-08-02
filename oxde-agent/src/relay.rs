use std::{collections::HashMap, convert::Infallible, sync::Arc};

use bollard::Docker;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, StreamBody, combinators::BoxBody};
use hyper::{Request, Response, Uri, body::Frame, upgrade::OnUpgrade};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo},
};
use oxde_proto::hub::v1::{
    AgentError, AgentErrorKind, Chunk, HttpHeader, RelayHttpRequest, RelayHttpRequestHead,
    RelayHttpResponse, RelayHttpResponseHead, SessionRequest, relay_http_request,
    relay_http_response, session_request,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, mpsc},
};

use crate::containers;

/// A raw byte pump reads and writes this much at a time once a connection
/// has switched protocols (e.g. a websocket).
const UPGRADE_BUFFER_SIZE: usize = 8192;

pub type RelayClient = Client<HttpConnector, BoxBody<Bytes, Infallible>>;

/// In-flight relayed requests, keyed by `request_id` - the first
/// `RelayHttpRequest::head` starts the handler task and registers a
/// channel here; every later `body_chunk` for the same `request_id` routes
/// into it, the same pattern `route_upload_chunk` uses for uploads.
pub type InFlightRelays = Arc<Mutex<HashMap<u64, mpsc::Sender<Chunk>>>>;

pub fn new_client() -> RelayClient {
    Client::builder(TokioExecutor::new()).build(HttpConnector::new())
}

pub async fn route_relay_message(
    in_flight: &InFlightRelays,
    docker: &Docker,
    client: &RelayClient,
    request_id: u64,
    message: RelayHttpRequest,
    tx: mpsc::Sender<SessionRequest>,
) {
    match message.part {
        Some(relay_http_request::Part::Head(head)) => {
            let (body_tx, body_rx) = mpsc::channel(16);
            in_flight.lock().await.insert(request_id, body_tx);

            let in_flight = in_flight.clone();
            let docker = docker.clone();
            let client = client.clone();
            tokio::spawn(async move {
                serve_relayed_request(&docker, &client, request_id, head, body_rx, &tx).await;
                in_flight.lock().await.remove(&request_id);
            });
        }
        Some(relay_http_request::Part::BodyChunk(chunk)) => {
            let sender = in_flight.lock().await.get(&request_id).cloned();
            if let Some(sender) = sender {
                drop(sender.send(chunk).await);
            }
        }
        None => {}
    }
}

async fn serve_relayed_request(
    docker: &Docker,
    client: &RelayClient,
    request_id: u64,
    head: RelayHttpRequestHead,
    body_rx: mpsc::Receiver<Chunk>,
    tx: &mpsc::Sender<SessionRequest>,
) {
    if let Err(err) = proxy(docker, client, &head, body_rx, request_id, tx).await {
        send(
            tx,
            request_id,
            session_request::Payload::RelayHttpResponse(RelayHttpResponse {
                part: Some(relay_http_response::Part::Error(err)),
            }),
        )
        .await;
    }
}

/// Yields each chunk the hub sends, ending after the one marked
/// `is_final` (or if the channel closes early).
fn request_body(rx: mpsc::Receiver<Chunk>) -> BoxBody<Bytes, Infallible> {
    let stream = futures_util::stream::unfold((rx, false), |(mut rx, done)| async move {
        if done {
            return None;
        }
        let chunk = rx.recv().await?;
        let frame: Result<Frame<Bytes>, Infallible> = Ok(Frame::data(Bytes::from(chunk.data)));
        Some((frame, (rx, chunk.is_final)))
    });
    StreamBody::new(stream).boxed()
}

fn is_upgrade_request(headers: &[HttpHeader]) -> bool {
    let has_upgrade_header = headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("upgrade"));
    let connection_says_upgrade = headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("connection")
            && header
                .value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    });
    has_upgrade_header && connection_says_upgrade
}

fn response_head_payload<B>(response: &Response<B>) -> RelayHttpResponse {
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|value| HttpHeader {
                name: name.to_string(),
                value: value.to_string(),
            })
        })
        .collect();
    RelayHttpResponse {
        part: Some(relay_http_response::Part::Head(RelayHttpResponseHead {
            status: u32::from(response.status().as_u16()),
            headers,
        })),
    }
}

async fn proxy(
    docker: &Docker,
    client: &RelayClient,
    head: &RelayHttpRequestHead,
    body_rx: mpsc::Receiver<Chunk>,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) -> Result<(), AgentError> {
    let ip = containers::container_ip(docker, &head.container_name)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unavailable(format!("container {} has no IP yet", head.container_name)))?;
    proxy_to_ip(client, &ip, head, body_rx, request_id, tx).await
}

/// The HTTP half of `proxy`, split out so it's testable against a plain
/// local server instead of a real container.
async fn proxy_to_ip(
    client: &RelayClient,
    ip: &str,
    head: &RelayHttpRequestHead,
    body_rx: mpsc::Receiver<Chunk>,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) -> Result<(), AgentError> {
    let uri: Uri = format!("http://{ip}:{}{}", head.container_port, head.path)
        .parse()
        .map_err(|err: http::uri::InvalidUri| unavailable(err.to_string()))?;
    let method = head
        .method
        .parse::<hyper::Method>()
        .map_err(|err| unavailable(err.to_string()))?;

    // An upgrade handshake (e.g. a websocket) never carries a body, so
    // `body_rx` is kept instead of wired into the request - `proxy_upgrade`
    // relays raw bytes through it if the container actually agrees to
    // switch protocols.
    if is_upgrade_request(&head.headers) {
        return proxy_upgrade(client, uri, method, head, body_rx, request_id, tx).await;
    }

    let mut builder = Request::builder().method(method).uri(uri);
    for header in &head.headers {
        builder = builder.header(&header.name, &header.value);
    }
    let request = builder
        .body(request_body(body_rx))
        .map_err(|err| unavailable(err.to_string()))?;

    let response = client
        .request(request)
        .await
        .map_err(|err| unavailable(err.to_string()))?;
    send(
        tx,
        request_id,
        session_request::Payload::RelayHttpResponse(response_head_payload(&response)),
    )
    .await;
    stream_response_body(response, tx, request_id).await;
    Ok(())
}

async fn proxy_upgrade(
    client: &RelayClient,
    uri: Uri,
    method: hyper::Method,
    head: &RelayHttpRequestHead,
    body_rx: mpsc::Receiver<Chunk>,
    request_id: u64,
    tx: &mpsc::Sender<SessionRequest>,
) -> Result<(), AgentError> {
    let mut builder = Request::builder().method(method).uri(uri);
    for header in &head.headers {
        builder = builder.header(&header.name, &header.value);
    }
    let request = builder
        .body(Empty::new().boxed())
        .map_err(|err| unavailable(err.to_string()))?;

    let mut response = client
        .request(request)
        .await
        .map_err(|err| unavailable(err.to_string()))?;

    if response.status() != hyper::StatusCode::SWITCHING_PROTOCOLS {
        send(
            tx,
            request_id,
            session_request::Payload::RelayHttpResponse(response_head_payload(&response)),
        )
        .await;
        stream_response_body(response, tx, request_id).await;
        return Ok(());
    }

    let on_upgrade = hyper::upgrade::on(&mut response);
    send(
        tx,
        request_id,
        session_request::Payload::RelayHttpResponse(response_head_payload(&response)),
    )
    .await;
    pump_upgraded_connection(on_upgrade, body_rx, tx, request_id).await
}

async fn stream_response_body(
    response: Response<hyper::body::Incoming>,
    tx: &mpsc::Sender<SessionRequest>,
    request_id: u64,
) {
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else { break };
        let Some(data) = frame.data_ref() else {
            continue;
        };
        send(
            tx,
            request_id,
            session_request::Payload::RelayHttpResponse(RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(Chunk {
                    data: data.to_vec(),
                    is_final: false,
                })),
            }),
        )
        .await;
    }
    send(
        tx,
        request_id,
        session_request::Payload::RelayHttpResponse(RelayHttpResponse {
            part: Some(relay_http_response::Part::BodyChunk(Chunk {
                data: Vec::new(),
                is_final: true,
            })),
        }),
    )
    .await;
}

/// Runs once the container's connection has switched protocols - from here
/// on both directions are raw bytes, not HTTP framing, relayed until
/// either side closes.
async fn pump_upgraded_connection(
    on_upgrade: OnUpgrade,
    mut body_rx: mpsc::Receiver<Chunk>,
    tx: &mpsc::Sender<SessionRequest>,
    request_id: u64,
) -> Result<(), AgentError> {
    let upgraded = on_upgrade
        .await
        .map_err(|err| unavailable(err.to_string()))?;
    let (mut reader, mut writer) = tokio::io::split(TokioIo::new(upgraded));

    let read_container = async {
        let mut buf = [0u8; UPGRADE_BUFFER_SIZE];
        loop {
            let read = match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            send(
                tx,
                request_id,
                session_request::Payload::RelayHttpResponse(RelayHttpResponse {
                    part: Some(relay_http_response::Part::BodyChunk(Chunk {
                        data: buf[..read].to_vec(),
                        is_final: false,
                    })),
                }),
            )
            .await;
        }
        send(
            tx,
            request_id,
            session_request::Payload::RelayHttpResponse(RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(Chunk {
                    data: Vec::new(),
                    is_final: true,
                })),
            }),
        )
        .await;
    };

    // Shutting the write half down (not just stopping) once the hub's side
    // ends is what lets the container's own read loop see EOF and notice,
    // instead of blocking forever on a half-open connection.
    let write_container = async {
        while let Some(chunk) = body_rx.recv().await {
            let is_final = chunk.is_final;
            if !chunk.data.is_empty() && writer.write_all(&chunk.data).await.is_err() {
                break;
            }
            if is_final {
                break;
            }
        }
        let _ = writer.shutdown().await;
    };

    tokio::join!(read_container, write_container);
    Ok(())
}

fn unavailable(message: impl Into<String>) -> AgentError {
    AgentError {
        kind: AgentErrorKind::Unavailable as i32,
        message: message.into(),
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

#[cfg(test)]
mod tests {
    use http_body_util::{BodyExt, Full};
    use hyper::{Response, body::Incoming, service::service_fn};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    use super::*;

    async fn spawn_hello_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let service = service_fn(|_req: Request<Incoming>| async {
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"world"))))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        port
    }

    async fn spawn_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let service = service_fn(|req: Request<Incoming>| async move {
                        let body = req.collect().await.expect("read body").to_bytes();
                        Ok::<_, Infallible>(Response::new(Full::new(body)))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        port
    }

    /// Answers every request with a real `101` upgrade handshake, then
    /// echoes whatever raw bytes arrive on the upgraded connection back to
    /// the sender - lets `proxy_to_ip`'s upgrade path be tested against an
    /// actual hyper server performing the same handshake a container would.
    async fn spawn_upgrade_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let service = service_fn(|mut req: Request<Incoming>| async move {
                        let on_upgrade = hyper::upgrade::on(&mut req);
                        tokio::spawn(async move {
                            let Ok(upgraded) = on_upgrade.await else {
                                return;
                            };
                            let mut io = TokioIo::new(upgraded);
                            let mut buf = [0u8; 1024];
                            loop {
                                let read = match io.read(&mut buf).await {
                                    Ok(0) | Err(_) => break,
                                    Ok(read) => read,
                                };
                                if io.write_all(&buf[..read]).await.is_err() {
                                    break;
                                }
                            }
                        });
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(101)
                                .header("connection", "upgrade")
                                .header("upgrade", "websocket")
                                .body(Empty::<Bytes>::new().boxed())
                                .expect("build 101 response"),
                        )
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .with_upgrades()
                        .await;
                });
            }
        });
        port
    }

    fn upgrade_request_head(port: u16) -> RelayHttpRequestHead {
        RelayHttpRequestHead {
            method: "GET".to_string(),
            path: "/".to_string(),
            headers: vec![
                HttpHeader {
                    name: "connection".to_string(),
                    value: "upgrade".to_string(),
                },
                HttpHeader {
                    name: "upgrade".to_string(),
                    value: "websocket".to_string(),
                },
            ],
            container_name: String::new(),
            container_port: u32::from(port),
        }
    }

    fn request_head(port: u16) -> RelayHttpRequestHead {
        RelayHttpRequestHead {
            method: "GET".to_string(),
            path: "/".to_string(),
            headers: Vec::new(),
            container_name: String::new(),
            container_port: u32::from(port),
        }
    }

    fn body_of(chunks: Vec<Chunk>) -> mpsc::Receiver<Chunk> {
        let (tx, rx) = mpsc::channel(chunks.len().max(1));
        tokio::spawn(async move {
            for chunk in chunks {
                let _ = tx.send(chunk).await;
            }
        });
        rx
    }

    /// Drains every `RelayHttpResponse` sent for `request_id` into
    /// `(status, body)`, stopping at the final body chunk.
    async fn collect_response(
        rx: &mut mpsc::Receiver<SessionRequest>,
        request_id: u64,
    ) -> (u32, Vec<u8>) {
        let mut status = 0;
        let mut body = Vec::new();
        while let Some(message) = rx.recv().await {
            assert_eq!(message.request_id, request_id);
            let Some(session_request::Payload::RelayHttpResponse(response)) = message.payload
            else {
                panic!("expected a RelayHttpResponse");
            };
            match response.part {
                Some(relay_http_response::Part::Head(head)) => status = head.status,
                Some(relay_http_response::Part::BodyChunk(chunk)) => {
                    body.extend(chunk.data);
                    if chunk.is_final {
                        break;
                    }
                }
                Some(relay_http_response::Part::Error(err)) => panic!("unexpected error: {err:?}"),
                None => {}
            }
        }
        (status, body)
    }

    #[tokio::test]
    async fn proxies_a_simple_response() {
        let port = spawn_hello_server().await;
        let client = new_client();
        let (tx, mut rx) = mpsc::channel(16);

        proxy_to_ip(
            &client,
            "127.0.0.1",
            &request_head(port),
            body_of(vec![Chunk {
                data: Vec::new(),
                is_final: true,
            }]),
            1,
            &tx,
        )
        .await
        .expect("proxy_to_ip");

        let (status, body) = collect_response(&mut rx, 1).await;
        assert_eq!(status, 200);
        assert_eq!(body, b"world");
    }

    #[tokio::test]
    async fn streams_the_request_body_to_the_target() {
        let port = spawn_echo_server().await;
        let client = new_client();
        let (tx, mut rx) = mpsc::channel(16);

        let mut head = request_head(port);
        head.method = "POST".to_string();
        proxy_to_ip(
            &client,
            "127.0.0.1",
            &head,
            body_of(vec![
                Chunk {
                    data: b"hello ".to_vec(),
                    is_final: false,
                },
                Chunk {
                    data: b"world".to_vec(),
                    is_final: true,
                },
            ]),
            2,
            &tx,
        )
        .await
        .expect("proxy_to_ip");

        let (status, body) = collect_response(&mut rx, 2).await;
        assert_eq!(status, 200);
        assert_eq!(body, b"hello world");
    }

    #[tokio::test]
    async fn relays_raw_bytes_both_directions_after_an_upgrade() {
        let port = spawn_upgrade_echo_server().await;
        let client = new_client();
        let (tx, mut rx) = mpsc::channel(16);
        let (body_tx, body_rx) = mpsc::channel(16);

        let head = upgrade_request_head(port);
        let proxy_task =
            tokio::spawn(
                async move { proxy_to_ip(&client, "127.0.0.1", &head, body_rx, 4, &tx).await },
            );

        let message = rx.recv().await.expect("101 response head");
        let Some(session_request::Payload::RelayHttpResponse(RelayHttpResponse {
            part: Some(relay_http_response::Part::Head(head)),
        })) = message.payload
        else {
            panic!("expected the response head");
        };
        assert_eq!(head.status, 101);

        body_tx
            .send(Chunk {
                data: b"ping".to_vec(),
                is_final: false,
            })
            .await
            .expect("send chunk");
        let echoed = loop {
            let message = rx.recv().await.expect("echoed chunk");
            let Some(session_request::Payload::RelayHttpResponse(RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(chunk)),
            })) = message.payload
            else {
                panic!("expected a body chunk");
            };
            if !chunk.data.is_empty() {
                break chunk.data;
            }
        };
        assert_eq!(echoed, b"ping");

        body_tx
            .send(Chunk {
                data: Vec::new(),
                is_final: true,
            })
            .await
            .expect("send final chunk");
        proxy_task
            .await
            .expect("proxy_to_ip task")
            .expect("proxy_to_ip");
    }

    #[test]
    fn is_upgrade_request_needs_both_headers() {
        assert!(!is_upgrade_request(&[]));

        let upgrade_only = [HttpHeader {
            name: "Upgrade".to_string(),
            value: "websocket".to_string(),
        }];
        assert!(!is_upgrade_request(&upgrade_only));

        let both = [
            HttpHeader {
                name: "Connection".to_string(),
                value: "keep-alive, Upgrade".to_string(),
            },
            HttpHeader {
                name: "Upgrade".to_string(),
                value: "websocket".to_string(),
            },
        ];
        assert!(is_upgrade_request(&both));
    }

    #[tokio::test]
    async fn unreachable_target_is_reported_as_an_agent_error() {
        let client = new_client();
        let (tx, _rx) = mpsc::channel(16);

        let err = proxy_to_ip(
            &client,
            "127.0.0.1",
            &request_head(1),
            body_of(vec![Chunk {
                data: Vec::new(),
                is_final: true,
            }]),
            3,
            &tx,
        )
        .await
        .expect_err("unreachable target must be reported");
        assert_eq!(err.kind, AgentErrorKind::Unavailable as i32);
    }
}
