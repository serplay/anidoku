//! `stream://` custom-scheme handler.
//!
//! The webview player fetches media through URLs shaped like:
//!   `stream://localhost/?url=<percent-encoded>&referer=<percent-encoded>`
//! We refetch upstream with the referer header (which the webview cannot set
//! itself) and stream the bytes back. HLS playlists are rewritten so their
//! child segments/variants come back through this same scheme.

use anidoku_core::proxy::{self, ProxyClient};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::UriSchemeResponder;

pub fn handle(
    proxy: Arc<ProxyClient>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let uri = request.uri().to_string();
    tauri::async_runtime::spawn(async move {
        let response = build_response(&proxy, &uri).await;
        responder.respond(response);
    });
}

/// Build the proxied URL a child resource should be fetched through.
pub fn make_stream_url(upstream: &str, referer: Option<&str>) -> String {
    let mut s = format!("stream://localhost/?url={}", urlencoding::encode(upstream));
    if let Some(r) = referer {
        s.push_str(&format!("&referer={}", urlencoding::encode(r)));
    }
    s
}

fn parse_query(uri: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = uri.split('?').nth(1) {
        // Drop any fragment.
        let q = q.split('#').next().unwrap_or(q);
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if let Ok(decoded) = urlencoding::decode(v) {
                    map.insert(k.to_string(), decoded.into_owned());
                }
            }
        }
    }
    map
}

async fn build_response(
    proxy: &ProxyClient,
    uri: &str,
) -> tauri::http::Response<Vec<u8>> {
    let params = parse_query(uri);
    let Some(url) = params.get("url") else {
        return error_response(400, "missing url parameter");
    };
    let referer = params.get("referer").map(String::as_str);

    let fetched = match proxy.fetch(url, referer).await {
        Ok(f) => f,
        Err(e) => return error_response(502, &format!("upstream fetch failed: {e}")),
    };

    if proxy::is_hls_playlist(url, &fetched.content_type) {
        let playlist = String::from_utf8_lossy(&fetched.bytes);
        let rewritten = proxy::rewrite_playlist(&playlist, url, |abs| {
            make_stream_url(abs, referer)
        });
        return ok_response("application/vnd.apple.mpegurl", rewritten.into_bytes());
    }

    ok_response(&fetched.content_type, fetched.bytes)
}

fn ok_response(content_type: &str, body: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(200)
        .header("Content-Type", content_type)
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap()
}

fn error_response(status: u16, msg: &str) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(msg.as_bytes().to_vec())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_and_parse_roundtrip() {
        let url = "https://cdn.example/hls/master.m3u8?token=a&b=c";
        let s = make_stream_url(url, Some("https://youtu-chan.com"));
        let params = parse_query(&s);
        assert_eq!(params.get("url").unwrap(), url);
        assert_eq!(params.get("referer").unwrap(), "https://youtu-chan.com");
    }

    #[test]
    fn parse_query_without_referer() {
        let s = make_stream_url("https://x/y.ts", None);
        let params = parse_query(&s);
        assert_eq!(params.get("url").unwrap(), "https://x/y.ts");
        assert!(params.get("referer").is_none());
    }
}
