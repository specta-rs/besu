//! The built frontend, as bytes.
//!
//! `build.rs` writes the table; this module turns it into an axum response and
//! keeps the SPA fallback in one place. The bundle is served from the binary, so
//! `besu studio` stays a single file with no asset directory to lose.

use axum::{
    body::Body,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// The page every non-asset path falls back to, so client-side routes such as
/// `/t/users` survive a reload.
pub(crate) fn index() -> Response {
    match ASSETS.iter().find(|(path, ..)| *path == "index.html") {
        Some((_, bytes, content_type)) => response(bytes, content_type),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "no index.html").into_response(),
    }
}

/// Serve one file from the bundle, or the SPA fallback for a client-side route.
///
/// A path under `/api/` is never rewritten to `index.html`: a mistyped endpoint
/// should be a visible 404, not a page of HTML that fails to parse as JSON.
pub(crate) fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "no such endpoint").into_response();
    }

    let path = if path.is_empty() { "index.html" } else { path };
    if let Some((_, bytes, content_type)) = ASSETS.iter().find(|(name, ..)| *name == path) {
        return response(bytes, content_type);
    }

    if path.contains('.') {
        return (StatusCode::NOT_FOUND, "no such asset").into_response();
    }

    index()
}

fn response(bytes: &'static [u8], content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            // Vite fingerprints every asset, so the hashed names are immutable;
            // `index.html` must not be, or a deploy would pin the old bundle.
            (header::CACHE_CONTROL, cache_control(content_type)),
        ],
        Body::from(bytes),
    )
        .into_response()
}

fn cache_control(content_type: &str) -> &'static str {
    if content_type.starts_with("text/html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}
