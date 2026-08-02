use std::path::PathBuf;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use oxde_models::RunConfig;
use oxde_proto::hub::v1::{
    Chunk, HttpHeader, RelayHttpRequest, RelayHttpResponse, relay_http_request,
    relay_http_response, session_request, session_response,
};
use tokio::sync::mpsc;
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::{agent_link::AgentLink, state::AppState, storage};

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

/// Relays `request` to the app's container over the same `Session`
/// connection every other agent command uses, instead of the hub dialing
/// the container's IP directly (only reachable when that IP happens to be
/// routable from the hub, which isn't true for a real separate host).
async fn serve_run_mode(
    agent_link: &AgentLink,
    container_name: &str,
    run_config: &RunConfig,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let head = RelayHttpRequest {
        part: Some(relay_http_request::Part::Head(
            oxde_proto::hub::v1::RelayHttpRequestHead {
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
            },
        )),
    };

    let Ok((request_id, mut rx)) = agent_link
        .call_bidi_streamed(outgoing_stream(head, body))
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

    let status = u16::try_from(head.status)
        .ok()
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::BAD_GATEWAY);
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

/// The head, then every body chunk read from `body`, then a trailing
/// `is_final` marker - `call_bidi_streamed` drains this in the background
/// while the response is already streaming back.
fn outgoing_stream(
    head: RelayHttpRequest,
    body: Body,
) -> impl Stream<Item = session_response::Payload> + Send + 'static {
    let chunks = body.into_data_stream().filter_map(|result| async move {
        result.ok().map(|bytes| {
            session_response::Payload::RelayHttpRequest(RelayHttpRequest {
                part: Some(relay_http_request::Part::BodyChunk(Chunk {
                    data: bytes.to_vec(),
                    is_final: false,
                })),
            })
        })
    });
    let final_chunk = futures_util::stream::once(async {
        session_response::Payload::RelayHttpRequest(RelayHttpRequest {
            part: Some(relay_http_request::Part::BodyChunk(Chunk {
                data: Vec::new(),
                is_final: true,
            })),
        })
    });
    futures_util::stream::once(async move { session_response::Payload::RelayHttpRequest(head) })
        .chain(chunks)
        .chain(final_chunk)
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
    async fn outgoing_stream_sends_head_then_chunks_then_a_final_marker() {
        let body = Body::from("hello world");
        let payloads: Vec<_> = outgoing_stream(head_payload(), body).collect().await;

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
}
