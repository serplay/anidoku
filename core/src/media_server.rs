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
//! URL shapes:
//!   `http://127.0.0.1:<port>/media?url=<enc>&referer=<enc>`  (streaming proxy)
//!   `http://127.0.0.1:<port>/dl/<anime>/<ep>/<file>`         (downloaded files)
//!
//! The `/dl/` route serves the offline downloads directory with full Range
//! support (AVFoundation probes local playback exactly like remote). Localized
//! playlists reference segments by bare relative filename, which resolve
//! naturally against the `/dl/...` path — no rewriting needed when serving.

use crate::proxy::{self, ProxyClient};
use crate::range::parse_range;
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncSeekExt;
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
    /// Root of the offline downloads tree served under `/dl/`, when enabled.
    downloads_root: Option<PathBuf>,
}

/// Bind a loopback listener on an ephemeral port and start serving. Must be
/// called from within a Tokio runtime (Tauri's `async_runtime` is Tokio).
/// `downloads_root`, when given, is served under `/dl/<relative path>` with
/// local-file Range support for offline playback.
pub async fn spawn(
    proxy: Arc<ProxyClient>,
    downloads_root: Option<PathBuf>,
) -> std::io::Result<MediaHandle> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");

    let state = Arc::new(ServerState {
        proxy,
        base: base.clone(),
        downloads_root,
    });
    let app = Router::new()
        .route("/media", get(handle_media))
        .route("/dl/{*path}", get(handle_download))
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

// ---------------------------------------------------------------------------
// Offline downloads: /dl/<relative path>
// ---------------------------------------------------------------------------

async fn handle_download(
    State(st): State<Arc<ServerState>>,
    AxumPath(path): AxumPath<String>,
    req_headers: HeaderMap,
) -> Response {
    let Some(root) = &st.downloads_root else {
        return text(StatusCode::NOT_FOUND, "downloads not enabled");
    };
    let Some(abs) = safe_join(root, &path) else {
        return text(StatusCode::FORBIDDEN, "invalid path");
    };
    let range = req_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());
    serve_file(&abs, range).await
}

/// Join a client-supplied relative path onto the downloads root, rejecting
/// absolute paths and any traversal (`..`) components.
fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for comp in rel.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." || comp.contains('\\') || comp.contains(':') {
            return None;
        }
        out.push(comp);
    }
    // Must still be under the root (belt and braces).
    if out.starts_with(root) && out != *root {
        Some(out)
    } else {
        None
    }
}

/// Serve a local file with single-range support: `206` + `Content-Range` for
/// range requests (AVFoundation's probe), `200` with `Accept-Ranges` otherwise.
async fn serve_file(path: &Path, range: Option<&str>) -> Response {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() => m,
        _ => return text(StatusCode::NOT_FOUND, "file not found"),
    };
    let total = meta.len();
    let content_type = local_content_type(path);

    let br = match range.map(|r| parse_range(r, total)) {
        Some(Ok(Some(br))) => Some(br),
        Some(Err(())) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(Body::empty())
                .unwrap();
        }
        _ => None,
    };

    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => return text(StatusCode::INTERNAL_SERVER_ERROR, &format!("open failed: {e}")),
    };

    let (status, start, len) = match br {
        Some(br) => (StatusCode::PARTIAL_CONTENT, br.start, br.len()),
        None => (StatusCode::OK, 0, total),
    };
    if start > 0 {
        if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await {
            return text(StatusCode::INTERNAL_SERVER_ERROR, &format!("seek failed: {e}"));
        }
    }
    let stream = tokio_util::io::ReaderStream::new(tokio::io::AsyncReadExt::take(file, len));

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if let Some(br) = br {
        builder = builder.header(header::CONTENT_RANGE, br.content_range());
    }
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "response build failed"))
}

/// Content type for a downloaded file, by extension.
fn local_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("m4s") => "video/iso.segment",
        Some("ts") => "video/mp2t",
        Some("aac") => "audio/aac",
        Some("m3u8") => "application/vnd.apple.mpegurl",
        Some("vtt") => "text/vtt",
        Some("json") => "application/json",
        _ => "application/octet-stream",
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

    #[test]
    fn safe_join_blocks_traversal() {
        let root = Path::new("/data/downloads");
        assert_eq!(
            safe_join(root, "show/1/video.mp4"),
            Some(PathBuf::from("/data/downloads/show/1/video.mp4"))
        );
        // Empty and "." components collapse.
        assert_eq!(
            safe_join(root, "show//1/./index.m3u8"),
            Some(PathBuf::from("/data/downloads/show/1/index.m3u8"))
        );
        assert_eq!(safe_join(root, "../etc/passwd"), None);
        assert_eq!(safe_join(root, "show/../../etc/passwd"), None);
        assert_eq!(safe_join(root, "show\\..\\x"), None);
        assert_eq!(safe_join(root, ""), None); // the root itself is not a file
    }

    #[test]
    fn local_content_types() {
        assert_eq!(local_content_type(Path::new("a/video.mp4")), "video/mp4");
        assert_eq!(local_content_type(Path::new("a/seg_00001.ts")), "video/mp2t");
        assert_eq!(
            local_content_type(Path::new("a/index.m3u8")),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(local_content_type(Path::new("a/sub_00_en.vtt")), "text/vtt");
        assert_eq!(
            local_content_type(Path::new("a/key_00.bin")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn serve_file_honors_ranges() {
        let dir = std::env::temp_dir().join(format!("anidoku-serve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("video.mp4");
        std::fs::write(&path, (0u8..=99).collect::<Vec<u8>>()).unwrap();

        // Full body.
        let resp = serve_file(&path, None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body.len(), 100);

        // AVFoundation probe.
        let resp = serve_file(&path, Some("bytes=0-1")).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 0-1/100"
        );
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], &[0, 1]);

        // Mid-file range returns the right bytes.
        let resp = serve_file(&path, Some("bytes=50-59")).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], &(50u8..=59).collect::<Vec<u8>>()[..]);

        // Unsatisfiable -> 416.
        let resp = serve_file(&path, Some("bytes=500-")).await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        // Missing file -> 404.
        let resp = serve_file(&dir.join("nope.mp4"), None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
