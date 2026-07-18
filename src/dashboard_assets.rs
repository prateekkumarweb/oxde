use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

/// The built `oxde-ui` SPA. `rust-embed` serves these straight off disk at
/// runtime in debug builds instead of compiling them in, so `vp build` in
/// `oxde-ui/` is enough to pick up frontend changes without a Rust rebuild;
/// release builds embed the files into the binary.
#[derive(RustEmbed)]
#[folder = "oxde-ui/dist"]
struct DashboardAssets;

/// Serves the embedded SPA under `/dashboard`. A path matching a real built
/// file is served as-is; anything else falls back to `index.html` so the
/// client-side router resolves it - matching `base: "/dashboard/"` and
/// `basepath: "/dashboard"` in the frontend's own config.
pub async fn serve(uri: Uri) -> Response {
    let path = uri
        .path()
        .strip_prefix("/dashboard/")
        .or_else(|| uri.path().strip_prefix("/dashboard"))
        .unwrap_or("");

    let Some(asset) = DashboardAssets::get(path).or_else(|| DashboardAssets::get("index.html"))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };

    (
        [(header::CONTENT_TYPE, asset.metadata.mimetype())],
        Body::from(asset.data),
    )
        .into_response()
}
