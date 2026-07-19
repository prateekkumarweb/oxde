use std::time::Duration;

use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};

pub type ProxyClient = Client<HttpConnector, Body>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A connect timeout is set explicitly - an unreachable container (or
/// network) must fail the request promptly rather than hang it forever, the
/// default with no timeout configured.
pub fn new_client() -> ProxyClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
    Client::builder(TokioExecutor::new()).build(connector)
}

pub async fn proxy(
    client: &ProxyClient,
    target_ip: &str,
    target_port: u16,
    mut request: Request<Body>,
) -> Response {
    let path_and_query = request.uri().path_and_query().map_or("/", |pq| pq.as_str());
    let Ok(target_uri) = format!("http://{target_ip}:{target_port}{path_and_query}").parse::<Uri>()
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    *request.uri_mut() = target_uri;

    match client.request(request).await {
        Ok(response) => response.map(Body::new),
        Err(err) => {
            tracing::error!(error = %err, target_ip, target_port, "reverse proxy request failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use tokio::net::TcpListener;

    use super::{Body, Request, StatusCode, new_client, proxy};

    async fn spawn_test_server(router: Router) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve");
        });
        port
    }

    #[tokio::test]
    async fn proxies_a_simple_response() {
        let router = Router::new().route("/hello", get(|| async { "world" }));
        let port = spawn_test_server(router).await;
        let client = new_client();

        let request = Request::builder()
            .uri("/hello")
            .body(Body::empty())
            .expect("build request");
        let response = proxy(&client, "127.0.0.1", port, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"world");
    }

    #[tokio::test]
    async fn streams_a_large_body_intact() {
        let large = vec![b'x'; 8 * 1024 * 1024];
        let router = Router::new().route(
            "/big",
            get(move || {
                let large = large.clone();
                async move { large }
            }),
        );
        let port = spawn_test_server(router).await;
        let client = new_client();

        let request = Request::builder()
            .uri("/big")
            .body(Body::empty())
            .expect("build request");
        let response = proxy(&client, "127.0.0.1", port, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        assert_eq!(body.len(), 8 * 1024 * 1024);
    }

    #[tokio::test]
    async fn unreachable_target_is_bad_gateway() {
        let client = new_client();
        let request = Request::builder()
            .uri("/")
            .body(Body::empty())
            .expect("build request");
        let response = proxy(&client, "127.0.0.1", 1, request).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}
