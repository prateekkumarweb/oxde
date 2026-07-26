use axum::{
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{routes::apps, state::AppState};

#[derive(Debug, PartialEq, Eq)]
enum HostMatch {
    ControlPlane,
    App(String),
    Unrecognized,
}

fn classify_host(host: &str, base_domain: &str) -> HostMatch {
    let host = host.split_once(':').map_or(host, |(h, _)| h).to_lowercase();
    let base_domain = base_domain.to_lowercase();

    if host == base_domain {
        return HostMatch::ControlPlane;
    }

    match host.strip_suffix(&format!(".{base_domain}")) {
        Some(label) if !label.is_empty() => HostMatch::App(label.to_string()),
        _ => HostMatch::Unrecognized,
    }
}

/// HTTP/2 (negotiated over TLS via ALPN) carries the host in the
/// `:authority` pseudo-header, not a literal `Host` header - hyper
/// surfaces that via `request.uri()`, not `request.headers()`.
fn resolve_host(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().authority().map(ToString::to_string))
}

pub async fn dispatch_by_host(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(host) = resolve_host(&request) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match classify_host(&host, state.base_domain()) {
        HostMatch::ControlPlane => next.run(request).await,
        HostMatch::App(label) => apps::serve(&state, &label, request).await,
        HostMatch::Unrecognized => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_host_prefers_the_host_header() {
        let request = Request::builder()
            .uri("https://from-uri.example/")
            .header(header::HOST, "from-header.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            resolve_host(&request).as_deref(),
            Some("from-header.example")
        );
    }

    #[test]
    fn resolve_host_falls_back_to_uri_authority_without_a_host_header() {
        // Mirrors an HTTP/2 request, whose `:authority` pseudo-header hyper
        // surfaces in `request.uri()` rather than the header map.
        let request = Request::builder()
            .uri("https://from-uri.example/")
            .body(Body::empty())
            .unwrap();
        assert_eq!(resolve_host(&request).as_deref(), Some("from-uri.example"));
    }

    #[test]
    fn resolve_host_is_none_without_a_host_header_or_uri_authority() {
        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        assert_eq!(resolve_host(&request), None);
    }

    #[test]
    fn exact_base_domain_is_control_plane() {
        assert_eq!(
            classify_host("example.com", "example.com"),
            HostMatch::ControlPlane
        );
    }

    #[test]
    fn subdomain_is_an_app() {
        assert_eq!(
            classify_host("blog.example.com", "example.com"),
            HostMatch::App("blog".to_string())
        );
    }

    #[test]
    fn host_is_case_insensitive() {
        assert_eq!(
            classify_host("BLOG.EXAMPLE.COM", "example.com"),
            HostMatch::App("blog".to_string())
        );
    }

    #[test]
    fn port_is_stripped_before_matching() {
        assert_eq!(
            classify_host("blog.localhost:3000", "localhost"),
            HostMatch::App("blog".to_string())
        );
    }

    #[test]
    fn unrelated_host_is_unrecognized() {
        assert_eq!(
            classify_host("evil.com", "example.com"),
            HostMatch::Unrecognized
        );
    }

    #[test]
    fn bare_dot_prefix_is_unrecognized() {
        assert_eq!(
            classify_host(".example.com", "example.com"),
            HostMatch::Unrecognized
        );
    }

    #[test]
    fn multi_level_subdomain_is_passed_through_as_one_label() {
        // Rejecting dotted app names is `models::validate_slug`'s job, not this
        // function's - it only needs to strip the base-domain suffix.
        assert_eq!(
            classify_host("foo.bar.example.com", "example.com"),
            HostMatch::App("foo.bar".to_string())
        );
    }
}
