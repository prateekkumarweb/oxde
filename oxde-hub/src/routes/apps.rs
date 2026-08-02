use std::path::PathBuf;

use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{CONNECTION, UPGRADE},
        request,
    },
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use hyper_util::rt::TokioIo;
use oxde_models::RunConfig;
use oxde_proto::hub::v1::{
    Chunk, HttpHeader, RelayHttpRequest, RelayHttpRequestHead, RelayHttpResponse,
    RelayHttpResponseHead, relay_http_request, relay_http_response, session_request,
    session_response,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use tokio_stream::wrappers::ReceiverStream;
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{agent_link::AgentLink, state::AppState, storage};

/// Bound on the channel feeding a relayed request's outgoing chunks - the
/// agent is expected to keep pace with the hub draining it, not buffer
/// unboundedly ahead.
const OUTGOING_CHANNEL_CAPACITY: usize = 16;

/// A raw byte pump reads and writes this much at a time once a connection
/// has switched protocols (e.g. a websocket).
const UPGRADE_BUFFER_SIZE: usize = 8192;

enum ServeTarget {
    NotFound,
    Static(PathBuf),
    Run {
        container_name: String,
        run_config: RunConfig,
        host_id: i64,
    },
}

pub async fn serve(state: &AppState, app_name: &str, request: Request<Body>) -> Response {
    if oxde_models::validate_slug(app_name).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let target = resolve_serve_target(state, app_name).await;

    match target {
        ServeTarget::NotFound => StatusCode::NOT_FOUND.into_response(),
        ServeTarget::Static(files_dir) => match ServeDir::new(files_dir).oneshot(request).await {
            Ok(response) => response.into_response(),
            Err(err) => {
                tracing::error!(error = %err, app = app_name, "static file serving failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
        ServeTarget::Run {
            container_name,
            run_config,
            host_id,
        } => {
            let agent_link = state.agent_link_for(host_id);
            serve_run_mode(&agent_link, &container_name, &run_config, request).await
        }
    }
}

async fn resolve_serve_target(state: &AppState, app_name: &str) -> ServeTarget {
    let Ok(app) = storage::get_app(state, app_name).await else {
        return ServeTarget::NotFound;
    };

    let Some(deployment_id) = storage::active_deployment_id(state, &app.id).await else {
        return ServeTarget::NotFound;
    };
    let Ok(deployment) = storage::get_deployment(state, &app.id, &deployment_id).await else {
        return ServeTarget::NotFound;
    };

    if let Some(container_name) = deployment.container_name {
        let Some(run_config) = app.run_config().cloned() else {
            return ServeTarget::NotFound;
        };
        return ServeTarget::Run {
            container_name,
            run_config,
            host_id: app.host_id,
        };
    }

    let active_files_dir = state.deployment_files_dir(&app.id, &deployment_id);
    if !active_files_dir.is_dir() {
        return ServeTarget::NotFound;
    }
    ServeTarget::Static(active_files_dir)
}

fn is_upgrade_request(headers: &axum::http::HeaderMap) -> bool {
    let has_upgrade_header = headers.contains_key(UPGRADE);
    let connection_says_upgrade = headers
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
    has_upgrade_header && connection_says_upgrade
}

fn request_head_payload(
    parts: &request::Parts,
    container_name: &str,
    run_config: &RunConfig,
) -> RelayHttpRequest {
    RelayHttpRequest {
        part: Some(relay_http_request::Part::Head(RelayHttpRequestHead {
            method: parts.method.to_string(),
            path: parts
                .uri
                .path_and_query()
                .map_or_else(|| "/".to_string(), ToString::to_string),
            headers: parts
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value.to_str().ok().map(|value| HttpHeader {
                        name: name.to_string(),
                        value: value.to_string(),
                    })
                })
                .collect(),
            container_name: container_name.to_string(),
            container_port: u32::from(run_config.container_port),
        })),
    }
}

fn status_from(status: u32) -> StatusCode {
    u16::try_from(status)
        .ok()
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::BAD_GATEWAY)
}

const fn body_chunk_payload(data: Vec<u8>, is_final: bool) -> session_response::Payload {
    session_response::Payload::RelayHttpRequest(RelayHttpRequest {
        part: Some(relay_http_request::Part::BodyChunk(Chunk {
            data,
            is_final,
        })),
    })
}

/// Sends `head` on `out_tx`, then - for an ordinary request - every body
/// chunk read from `body` followed by a trailing `is_final` marker. An
/// upgrade-eligible request never carries a body, so `out_tx` is left open
/// instead: `pump_upgraded_connection` reuses it if the agent's container
/// actually agrees to switch protocols.
fn spawn_request_feeder(
    out_tx: mpsc::Sender<session_response::Payload>,
    head: RelayHttpRequest,
    body: Body,
    is_upgrade: bool,
) {
    tokio::spawn(async move {
        if out_tx
            .send(session_response::Payload::RelayHttpRequest(head))
            .await
            .is_err()
            || is_upgrade
        {
            return;
        }

        let mut chunks = std::pin::pin!(body.into_data_stream());
        while let Some(result) = chunks.next().await {
            let Some(bytes) = result.ok() else { continue };
            if out_tx
                .send(body_chunk_payload(bytes.to_vec(), false))
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = out_tx.send(body_chunk_payload(Vec::new(), true)).await;
    });
}

/// Relays `request` to the app's container over the same `Session`
/// connection every other agent command uses, instead of the hub dialing
/// the container's IP directly (only reachable when that IP happens to be
/// routable from the hub, which isn't true for a real separate host).
async fn serve_run_mode(
    agent_link: &AgentLink,
    container_name: &str,
    run_config: &RunConfig,
    mut request: Request<Body>,
) -> Response {
    let is_upgrade = is_upgrade_request(request.headers());
    let on_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut request));

    let (parts, body) = request.into_parts();
    let head = request_head_payload(&parts, container_name, run_config);

    let (out_tx, out_rx) = mpsc::channel(OUTGOING_CHANNEL_CAPACITY);
    spawn_request_feeder(out_tx.clone(), head, body, is_upgrade);

    let Ok((request_id, mut rx)) = agent_link
        .call_bidi_streamed(ReceiverStream::new(out_rx))
        .await
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };

    let Some(session_request::Payload::RelayHttpResponse(RelayHttpResponse {
        part: Some(relay_http_response::Part::Head(head)),
    })) = rx.recv().await
    else {
        agent_link.end_stream(request_id).await;
        return StatusCode::BAD_GATEWAY.into_response();
    };

    let status = status_from(head.status);

    if let Some(on_upgrade) = on_upgrade
        && status == StatusCode::SWITCHING_PROTOCOLS
    {
        return upgrade_response(
            &head,
            agent_link.clone(),
            request_id,
            rx,
            out_tx,
            on_upgrade,
        );
    }

    let mut builder = Response::builder().status(status);
    for header in &head.headers {
        builder = builder.header(&header.name, &header.value);
    }

    let agent_link = agent_link.clone();
    let body = Body::from_stream(response_body_stream(rx, agent_link, request_id));
    match builder.body(body) {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(error = %err, "failed to build the relayed response");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Builds the `101` response and hands the connection off to
/// `pump_upgraded_connection` - the response is returned immediately so
/// axum can flush it; the byte pump only starts once that's done and the
/// browser's connection has actually switched protocols.
fn upgrade_response(
    head: &RelayHttpResponseHead,
    agent_link: AgentLink,
    request_id: u64,
    rx: mpsc::Receiver<session_request::Payload>,
    out_tx: mpsc::Sender<session_response::Payload>,
    on_upgrade: hyper::upgrade::OnUpgrade,
) -> Response {
    let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for header in &head.headers {
        builder = builder.header(&header.name, &header.value);
    }
    let response = match builder.body(Body::empty()) {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(error = %err, "failed to build the upgrade response");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    tokio::spawn(pump_upgraded_connection(
        agent_link, request_id, rx, out_tx, on_upgrade,
    ));
    response
}

/// Runs once the browser's connection has switched protocols - from here
/// on both directions are raw bytes, not HTTP framing, relayed until
/// either side closes.
async fn pump_upgraded_connection(
    agent_link: AgentLink,
    request_id: u64,
    mut rx: mpsc::Receiver<session_request::Payload>,
    out_tx: mpsc::Sender<session_response::Payload>,
    on_upgrade: hyper::upgrade::OnUpgrade,
) {
    let upgraded = match on_upgrade.await {
        Ok(upgraded) => upgraded,
        Err(err) => {
            tracing::error!(error = ?err, "failed to take over the upgraded browser connection");
            agent_link.end_stream(request_id).await;
            return;
        }
    };
    let (mut reader, mut writer) = tokio::io::split(TokioIo::new(upgraded));

    let read_browser = async {
        let mut buf = [0u8; UPGRADE_BUFFER_SIZE];
        loop {
            let read = match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            if out_tx
                .send(body_chunk_payload(buf[..read].to_vec(), false))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = out_tx.send(body_chunk_payload(Vec::new(), true)).await;
    };

    // Shutting the write half down (not just stopping) once the agent's
    // side ends is what lets the browser's own read loop see EOF and
    // notice, instead of blocking forever on a half-open connection.
    let write_browser = async {
        while let Some(session_request::Payload::RelayHttpResponse(RelayHttpResponse {
            part: Some(relay_http_response::Part::BodyChunk(chunk)),
        })) = rx.recv().await
        {
            if !chunk.data.is_empty() && writer.write_all(&chunk.data).await.is_err() {
                break;
            }
            if chunk.is_final {
                break;
            }
        }
        let _ = writer.shutdown().await;
    };

    tokio::join!(read_browser, write_browser);
    agent_link.end_stream(request_id).await;
}

/// Yields each response body chunk until the one marked `is_final` (or the
/// channel closes early); `end_stream` runs either way, including if the
/// client drops the response before it finishes.
fn response_body_stream(
    rx: mpsc::Receiver<session_request::Payload>,
    agent_link: AgentLink,
    request_id: u64,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    futures_util::stream::unfold(
        (
            rx,
            Some(EndStreamOnDrop {
                agent_link,
                request_id,
            }),
        ),
        |(mut rx, guard)| async move {
            let Some(session_request::Payload::RelayHttpResponse(RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(chunk)),
            })) = rx.recv().await
            else {
                return None;
            };
            if chunk.is_final {
                return None;
            }
            Some((Ok(Bytes::from(chunk.data)), (rx, guard)))
        },
    )
}

/// `AgentLink::end_stream` normally runs once a caller sees the reply
/// marking a stream finished - dropped early (the client disconnecting
/// mid-response) skips that, so this runs it on drop either way.
struct EndStreamOnDrop {
    agent_link: AgentLink,
    request_id: u64,
}

impl Drop for EndStreamOnDrop {
    fn drop(&mut self) {
        let agent_link = self.agent_link.clone();
        let request_id = self.request_id;
        tokio::spawn(async move {
            agent_link.end_stream(request_id).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_payload() -> RelayHttpRequest {
        RelayHttpRequest {
            part: Some(relay_http_request::Part::Head(
                oxde_proto::hub::v1::RelayHttpRequestHead::default(),
            )),
        }
    }

    #[tokio::test]
    async fn request_feeder_sends_head_then_chunks_then_a_final_marker() {
        let body = Body::from("hello world");
        let (out_tx, out_rx) = mpsc::channel(OUTGOING_CHANNEL_CAPACITY);
        spawn_request_feeder(out_tx, head_payload(), body, false);
        let payloads: Vec<_> = ReceiverStream::new(out_rx).collect().await;

        let session_response::Payload::RelayHttpRequest(RelayHttpRequest {
            part: Some(relay_http_request::Part::Head(_)),
        }) = &payloads[0]
        else {
            panic!("first payload must be the head");
        };

        let mut received = Vec::new();
        for payload in &payloads[1..payloads.len() - 1] {
            let session_response::Payload::RelayHttpRequest(RelayHttpRequest {
                part: Some(relay_http_request::Part::BodyChunk(chunk)),
            }) = payload
            else {
                panic!("expected a body chunk");
            };
            assert!(!chunk.is_final);
            received.extend_from_slice(&chunk.data);
        }
        assert_eq!(received, b"hello world");

        let session_response::Payload::RelayHttpRequest(RelayHttpRequest {
            part: Some(relay_http_request::Part::BodyChunk(last)),
        }) = payloads.last().expect("at least the final marker")
        else {
            panic!("last payload must be a body chunk");
        };
        assert!(last.is_final);
        assert!(last.data.is_empty());
    }

    #[tokio::test]
    async fn request_feeder_sends_only_the_head_for_an_upgrade_request() {
        let body = Body::empty();
        let (out_tx, mut out_rx) = mpsc::channel(OUTGOING_CHANNEL_CAPACITY);
        spawn_request_feeder(out_tx.clone(), head_payload(), body, true);
        drop(out_tx);

        let session_response::Payload::RelayHttpRequest(RelayHttpRequest {
            part: Some(relay_http_request::Part::Head(_)),
        }) = out_rx.recv().await.expect("head payload")
        else {
            panic!("expected the head payload");
        };
        assert!(
            out_rx.recv().await.is_none(),
            "no body chunk should follow an upgrade request's head"
        );
    }

    #[tokio::test]
    async fn response_body_stream_ends_at_the_final_chunk() {
        let (tx, rx) = mpsc::channel(16);
        tx.send(session_request::Payload::RelayHttpResponse(
            RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(Chunk {
                    data: b"hello ".to_vec(),
                    is_final: false,
                })),
            },
        ))
        .await
        .expect("send chunk");
        tx.send(session_request::Payload::RelayHttpResponse(
            RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(Chunk {
                    data: b"world".to_vec(),
                    is_final: false,
                })),
            },
        ))
        .await
        .expect("send chunk");
        tx.send(session_request::Payload::RelayHttpResponse(
            RelayHttpResponse {
                part: Some(relay_http_response::Part::BodyChunk(Chunk {
                    data: Vec::new(),
                    is_final: true,
                })),
            },
        ))
        .await
        .expect("send final chunk");

        let stream = response_body_stream(rx, AgentLink::new(), 1);
        let chunks: Vec<Bytes> = stream
            .map(|result| result.expect("no io error"))
            .collect()
            .await;
        let received: Vec<u8> = chunks.into_iter().flat_map(Vec::from).collect();
        assert_eq!(received, b"hello world");
    }

    #[test]
    fn is_upgrade_request_needs_both_headers() {
        let mut headers = axum::http::HeaderMap::new();
        assert!(!is_upgrade_request(&headers));

        headers.insert(UPGRADE, "websocket".parse().unwrap());
        assert!(!is_upgrade_request(&headers), "missing Connection header");

        headers.insert(CONNECTION, "keep-alive".parse().unwrap());
        assert!(
            !is_upgrade_request(&headers),
            "Connection header without the upgrade token"
        );

        headers.insert(CONNECTION, "Keep-Alive, Upgrade".parse().unwrap());
        assert!(is_upgrade_request(&headers));
    }
}
