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

pub async fn dispatch_by_host(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let Some(host) = host else {
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
