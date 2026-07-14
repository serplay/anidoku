//! Streaming proxy support.
//!
//! Webview media elements (and hls.js) cannot set a custom `Referer`/`Origin`
//! on their media fetches, but allanime's CDNs require them. The desktop shell
//! therefore registers a custom URI scheme that routes every media request
//! through here: we refetch upstream with the correct headers, and — for HLS
//! playlists — rewrite child URIs so segments and sub-playlists come back
//! through the same scheme.
//!
//! This module holds the reusable pieces: the fetch client and the (pure,
//! unit-tested) playlist rewriter. Wiring to a concrete scheme lives in the
//! Tauri shell.

use crate::Result;
use reqwest::Client;

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:150.0) Gecko/20100101 Firefox/150.0";

pub struct ProxyClient {
    client: Client,
}

pub struct FetchedResource {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

impl Default for ProxyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                // Do not let reqwest transparently gzip/decompress: the media
                // server proxies raw bytes and relays upstream Content-Length /
                // Content-Range, which transparent decompression would falsify.
                .no_gzip()
                .build()
                .expect("reqwest client"),
        }
    }

    /// Fetch `url` fully with the given `referer`, returning body bytes and the
    /// upstream content type (defaulting to octet-stream). Used for small
    /// resources we must have in full (HLS playlists to rewrite).
    pub async fn fetch(&self, url: &str, referer: Option<&str>) -> Result<FetchedResource> {
        let mut req = self.client.get(url);
        if let Some(r) = referer {
            req = req.header("Referer", r).header("Origin", r);
        }
        let resp = req.send().await?.error_for_status()?;
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = resp.bytes().await?.to_vec();
        Ok(FetchedResource {
            bytes,
            content_type,
        })
    }

    /// Issue a GET with the referer injected and the client's `Range` header
    /// (if any) forwarded verbatim, returning the raw upstream response so the
    /// caller can relay its status (`200`/`206`), range headers, and stream the
    /// body. This is the passthrough path for MP4 and HLS segments, where
    /// preserving `Range` -> `206 Partial Content` is what macOS AVFoundation
    /// requires to play at all.
    pub async fn get_ranged(
        &self,
        url: &str,
        referer: Option<&str>,
        range: Option<&str>,
    ) -> Result<reqwest::Response> {
        let mut req = self.client.get(url);
        if let Some(r) = referer {
            req = req.header(reqwest::header::REFERER, r).header(reqwest::header::ORIGIN, r);
        }
        if let Some(rg) = range {
            req = req.header(reqwest::header::RANGE, rg);
        }
        Ok(req.send().await?)
    }
}

/// Is this URL / content type an HLS playlist that needs rewriting?
pub fn is_hls_playlist(url: &str, content_type: &str) -> bool {
    url.contains(".m3u8")
        || content_type.contains("mpegurl")
        || content_type.contains("vnd.apple.mpegurl")
}

/// Rewrite every URI line and `URI="..."` attribute in an m3u8 playlist so it
/// is fetched back through the proxy.
///
/// `make_proxy_url` receives an absolute upstream URL (relative URIs are
/// resolved against `base_url`) and returns the proxied URL to substitute.
pub fn rewrite_playlist<F>(playlist: &str, base_url: &str, mut make_proxy_url: F) -> String
where
    F: FnMut(&str) -> String,
{
    let mut out = String::with_capacity(playlist.len() + playlist.len() / 4);
    for line in playlist.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(c) => (c, "\n"),
            None => (line, ""),
        };
        let trimmed = content.trim_end_matches('\r');
        let cr = if content.len() != trimmed.len() { "\r" } else { "" };

        if trimmed.is_empty() {
            out.push_str(line);
            continue;
        }

        if trimmed.starts_with('#') {
            // Rewrite any URI="..." attribute (EXT-X-KEY, EXT-X-MEDIA, MAP...).
            out.push_str(&rewrite_uri_attr(trimmed, base_url, &mut make_proxy_url));
            out.push_str(cr);
            out.push_str(newline);
        } else {
            // A bare URI line: a segment or a variant playlist.
            let abs = resolve_url(base_url, trimmed);
            out.push_str(&make_proxy_url(&abs));
            out.push_str(cr);
            out.push_str(newline);
        }
    }
    out
}

fn rewrite_uri_attr<F>(tag: &str, base_url: &str, make_proxy_url: &mut F) -> String
where
    F: FnMut(&str) -> String,
{
    let Some(start) = tag.find("URI=\"") else {
        return tag.to_string();
    };
    let val_start = start + 5;
    let Some(rel_end) = tag[val_start..].find('"') else {
        return tag.to_string();
    };
    let end = val_start + rel_end;
    let uri = &tag[val_start..end];
    let abs = resolve_url(base_url, uri);
    let proxied = make_proxy_url(&abs);
    format!("{}{}{}", &tag[..val_start], proxied, &tag[end..])
}

/// Resolve a possibly-relative URL against a base playlist URL. Handles
/// absolute URLs, root-relative paths, and plain relative paths. Query and
/// fragment on the base are dropped for the directory computation.
pub fn resolve_url(base_url: &str, target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }

    // Split scheme://authority from the path.
    let (scheme_authority, base_path) = match base_url.find("://") {
        Some(i) => {
            let after = &base_url[i + 3..];
            match after.find('/') {
                Some(j) => (&base_url[..i + 3 + j], &after[j..]),
                None => (base_url, "/"),
            }
        }
        None => return target.to_string(),
    };
    // Strip query/fragment from base path.
    let base_path = base_path
        .split(['?', '#'])
        .next()
        .unwrap_or(base_path);

    if let Some(rooted) = target.strip_prefix('/') {
        return format!("{scheme_authority}/{rooted}");
    }

    // Directory of the base path (everything up to and including last '/').
    let dir = match base_path.rfind('/') {
        Some(k) => &base_path[..=k],
        None => "/",
    };
    format!("{scheme_authority}{dir}{target}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_absolute_passthrough() {
        assert_eq!(
            resolve_url("https://a/b/c.m3u8", "https://x/y.ts"),
            "https://x/y.ts"
        );
    }

    #[test]
    fn resolve_relative_and_rooted() {
        assert_eq!(
            resolve_url("https://cdn.example/hls/master.m3u8", "seg0.ts"),
            "https://cdn.example/hls/seg0.ts"
        );
        assert_eq!(
            resolve_url("https://cdn.example/hls/master.m3u8?token=1", "480/index.m3u8"),
            "https://cdn.example/hls/480/index.m3u8"
        );
        assert_eq!(
            resolve_url("https://cdn.example/hls/master.m3u8", "/abs/seg.ts"),
            "https://cdn.example/abs/seg.ts"
        );
    }

    #[test]
    fn rewrites_segment_and_variant_lines() {
        let playlist = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n480/index.m3u8\n#EXTINF:6.0,\nseg0.ts\n";
        let out = rewrite_playlist(playlist, "https://cdn/hls/master.m3u8", |abs| {
            format!("stream://proxy?u={abs}")
        });
        assert!(out.contains("stream://proxy?u=https://cdn/hls/480/index.m3u8"));
        assert!(out.contains("stream://proxy?u=https://cdn/hls/seg0.ts"));
        // Tag lines that are not URIs stay intact.
        assert!(out.contains("#EXT-X-STREAM-INF:BANDWIDTH=1000"));
    }

    #[test]
    fn rewrites_uri_attribute_in_tags() {
        let playlist = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x1\n#EXTINF:6,\nseg.ts\n";
        let out = rewrite_playlist(playlist, "https://cdn/h/p.m3u8", |abs| format!("P[{abs}]"));
        assert!(out.contains("URI=\"P[https://cdn/h/key.bin]\""));
        assert!(out.contains("P[https://cdn/h/seg.ts]"));
    }

    #[test]
    fn preserves_crlf_and_blank_lines() {
        let playlist = "#EXTM3U\r\n\r\nseg.ts\r\n";
        let out = rewrite_playlist(playlist, "https://c/p.m3u8", |_| "X".into());
        assert!(out.contains("\r\n\r\n"));
        assert!(out.contains("X\r\n"));
    }

    #[test]
    fn detects_hls() {
        assert!(is_hls_playlist("https://x/master.m3u8", "text/plain"));
        assert!(is_hls_playlist("https://x/p", "application/vnd.apple.mpegurl"));
        assert!(!is_hls_playlist("https://x/seg.ts", "video/mp2t"));
    }
}
