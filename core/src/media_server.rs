//! Local HTTP media server (127.0.0.1:<random port>).
//!
//! Why this exists instead of a custom `stream://` scheme: on macOS the
//! `<video>` element (progressive MP4 and native HLS in WKWebView) is driven by
//! AVFoundation, which fetches media *out of band* and opens every resource
//! with a byte-range probe (`Range: bytes=0-1`). A custom `WKURLSchemeHandler`
//! that answers those probes with a `200` full body makes playback stall
//! forever (MP4) or hard-error (HLS). A real loopback HTTP server behaves like
//! any origin server: it forwards the `Range` header upstream and relays the
//! `206 Partial Content` + `Content-Range`, which is what AVFoundation needs.
//!
//! The server does three jobs:
//!   1. Inject the `Referer`/`Origin` header the webview cannot set (CDN-gated).
//!   2. Rewrite HLS playlists so child segments/variants route back through it.
//!   3. Pass byte ranges through for MP4 / segments; satisfy ranges itself for
//!      the playlists it generates in memory.
//!
//! URL shape: `http://127.0.0.1:<port>/media?url=<enc>&referer=<enc>`.

use crate::proxy::{self, ProxyClient};
use crate::range::parse_range;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;

/// A running media server. Dropping this does not stop the server (it lives on
/// the async runtime for the app's lifetime); keep the `base` to build URLs.
#[derive(Debug, Clone)]
pub struct MediaHandle {
    pub port: u16,
    /// e.g. `http://127.0.0.1:52123`
    pub base: String,
}

struct ServerState {
    proxy: Arc<ProxyClient>,
    base: String,
}

/// Bind a loopback listener on an ephemeral port and start serving. Must be
/// called from within a Tokio runtime (Tauri's `async_runtime` is Tokio).
pub async fn spawn(proxy: Arc<ProxyClient>) -> std::io::Result<MediaHandle> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");

    let state = Arc::new(ServerState {
        proxy,
        base: base.clone(),
    });
    let app = Router::new()
        .route("/media", get(handle_media))
        .with_state(state);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("media server exited: {e}");
        }
    });

    Ok(MediaHandle { port, base })
}

/// Build the local URL through which a given upstream resource should be
/// fetched (used both by the UI for the top-level source and by the playlist
/// rewriter for child segments/variants).
pub fn make_media_url(base: &str, upstream: &str, referer: Option<&str>) -> String {
    let mut s = format!("{base}/media?url={}", urlencoding::encode(upstream));
    if let Some(r) = referer {
        s.push_str(&format!("&referer={}", urlencoding::encode(r)));
    }
    s
}

async fn handle_media(
    State(st): State<Arc<ServerState>>,
    Query(params): Query<HashMap<String, String>>,
    req_headers: HeaderMap,
) -> Response {
    let Some(url) = params.get("url") else {
        return text(StatusCode::BAD_REQUEST, "missing url parameter");
    };
    let referer = params.get("referer").map(String::as_str);
    let range = req_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // Playlists must be fetched in full and rewritten; a partial (`206`) body
    // would corrupt the rewrite, so we never forward Range upstream for them
    // and instead satisfy the probe against the rewritten bytes ourselves.
    if url.contains(".m3u8") {
        return serve_playlist(&st, url, referer, range.as_deref()).await;
    }

    // Everything else (MP4, HLS segments, key files, subtitle files): stream
    // through with Range forwarded so we relay upstream 206 / Content-Range.
    let resp = match st.proxy.get_ranged(url, referer, range.as_deref()).await {
        Ok(r) => r,
        Err(e) => return text(StatusCode::BAD_GATEWAY, &format!("upstream fetch failed: {e}")),
    };

    // Safety net: a playlist whose URL lacks `.m3u8` but is served with an HLS
    // content type still needs rewriting (only when we got a full 200 body).
    let upstream_ct = header_str(resp.headers(), header::CONTENT_TYPE)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if resp.status() == StatusCode::OK && proxy::is_hls_playlist(url, &upstream_ct) {
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return text(StatusCode::BAD_GATEWAY, &format!("read failed: {e}")),
        };
        let playlist = String::from_utf8_lossy(&bytes);
        let rewritten = proxy::rewrite_playlist(&playlist, url, |abs| {
            make_media_url(&st.base, abs, referer)
        });
        return serve_bytes(
            rewritten.into_bytes(),
            "application/vnd.apple.mpegurl",
            range.as_deref(),
        );
    }

    // Content-type hint: some CDNs (fast4speed) serve real MP4 as
    // `application/octet-stream`, which WebKit's <video> refuses to play. When
    // the UI passes a `ct` hint and upstream gave only a generic type, relay
    // the hint so the player recognises the media. Genuine `video/*` upstream
    // types (and `text/html` embed pages) are left untouched.
    let response_ct = match params.get("ct") {
        Some(hint) if is_generic_ct(&upstream_ct) => hint.clone(),
        _ => upstream_ct,
    };
    relay_streaming(resp, &response_ct)
}

async fn serve_playlist(
    st: &ServerState,
    url: &str,
    referer: Option<&str>,
    range: Option<&str>,
) -> Response {
    let fetched = match st.proxy.fetch(url, referer).await {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_GATEWAY, &format!("upstream fetch failed: {e}")),
    };
    let playlist = String::from_utf8_lossy(&fetched.bytes);
    let rewritten =
        proxy::rewrite_playlist(&playlist, url, |abs| make_media_url(&st.base, abs, referer));
    serve_bytes(
        rewritten.into_bytes(),
        "application/vnd.apple.mpegurl",
        range,
    )
}

/// Relay a streaming upstream response, preserving status (200/206), content
/// type, and the range-related headers, while advertising range support.
fn relay_streaming(resp: reqwest::Response, content_type: &str) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::OK);
    let content_length = header_str(resp.headers(), header::CONTENT_LENGTH);
    let content_range = header_str(resp.headers(), header::CONTENT_RANGE);

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if let Some(len) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, len);
    }
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    builder
        .body(Body::from_stream(resp.bytes_stream()))
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "response build failed"))
}

/// Serve an in-memory body with full single-range support (used for the HLS
/// playlists we generate). Answers AVFoundation's `bytes=0-1` probe with a
/// proper `206`.
fn serve_bytes(body: Vec<u8>, content_type: &str, range: Option<&str>) -> Response {
    let total = body.len() as u64;

    match range.map(|r| parse_range(r, total)) {
        Some(Ok(Some(br))) => {
            let slice = body[br.start as usize..=br.end as usize].to_vec();
            Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_RANGE, br.content_range())
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, slice.len())
                .header(header::CACHE_CONTROL, "no-store")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(Body::from(slice))
                .unwrap()
        }
        Some(Err(())) => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Body::empty())
            .unwrap(),
        _ => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, total)
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Body::from(body))
            .unwrap(),
    }
}

/// A content type carrying no useful codec info for the player to sniff.
fn is_generic_ct(ct: &str) -> bool {
    let ct = ct.trim().to_ascii_lowercase();
    ct.is_empty()
        || ct.starts_with("application/octet-stream")
        || ct.starts_with("binary/octet-stream")
}

fn header_str(headers: &reqwest::header::HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn text(status: StatusCode, msg: &str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Body::from(msg.to_owned()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_media_url_encodes_and_roundtrips() {
        let u = make_media_url(
            "http://127.0.0.1:5000",
            "https://cdn.example/hls/m.m3u8?token=a&b=c",
            Some("https://ref.example"),
        );
        assert!(u.starts_with("http://127.0.0.1:5000/media?url="));
        // The upstream query separators must be encoded, not leak into ours.
        assert!(u.contains("token%3Da"));
        assert!(u.contains("&referer=https%3A%2F%2Fref.example"));
    }

    #[test]
    fn make_media_url_without_referer() {
        let u = make_media_url("http://127.0.0.1:1", "https://x/y.ts", None);
        assert!(!u.contains("referer="));
    }
}
