use std::fmt::Write;

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

/// The built `oxde-ui` SPA. `rust-embed` serves these straight off disk at
/// runtime in debug builds instead of compiling them in, so `vp build` in
/// `oxde-ui/` is enough to pick up frontend changes without a Rust rebuild;
/// release builds embed the files into the binary.
#[derive(RustEmbed)]
#[folder = "../oxde-ui/dist"]
struct DashboardAssets;

fn hex_encode(bytes: [u8; 32]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Serves the embedded SPA under `/dashboard`, falling back to `index.html`
/// for client-side routing unless the miss is under `assets/` (a stale
/// hashed-build reference, so 404 instead of silently returning HTML).
///
/// `assets/*` is content-hashed, so it's cached forever; everything else is
/// unhashed and revalidated via `ETag`/`If-None-Match` on every load.
pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri
        .path()
        .strip_prefix("/dashboard/")
        .or_else(|| uri.path().strip_prefix("/dashboard"))
        .unwrap_or("");

    let is_hashed_asset = path.starts_with("assets/");
    let asset = DashboardAssets::get(path).or_else(|| {
        if is_hashed_asset {
            None
        } else {
            DashboardAssets::get("index.html")
        }
    });

    let Some(asset) = asset else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let etag = format!("\"{}\"", hex_encode(asset.metadata.sha256_hash()));
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|value| value.as_bytes() == etag.as_bytes())
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }

    let cache_control = if is_hashed_asset {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    (
        [
            (header::CONTENT_TYPE, asset.metadata.mimetype().to_string()),
            (header::CACHE_CONTROL, cache_control.to_string()),
            (header::ETAG, etag),
        ],
        Body::from(asset.data),
    )
        .into_response()
}
