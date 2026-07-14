//! allanime provider: a native port of ani-cli 4.14.1's scraping flow.
//!
//! Flow (matching ani-cli):
//!   search  -> POST GraphQL, parse shows.edges
//!   episodes-> POST GraphQL, parse availableEpisodesDetail
//!   sources -> GET persisted GraphQL (POST fallback) for sourceUrls, which
//!              may be wrapped in an encrypted `tobeparsed` blob; deobfuscate
//!              each sourceUrl into a /clock.json path, fetch it with the
//!              allanime referer, parse the links.

mod constants;
mod decrypt;
mod parse;

use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::provider::Provider;
use crate::{Error, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

pub use constants::*;

pub struct AllAnime {
    client: Client,
    key: [u8; 32],
}

impl Default for AllAnime {
    fn default() -> Self {
        Self::new()
    }
}

impl AllAnime {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("reqwest client");
        Self {
            client,
            key: decrypt::derive_key(DECRYPT_PASSPHRASE),
        }
    }

    /// GET the persisted episode-sources query. This is ani-cli's primary
    /// path; the server rejects the equivalent ad-hoc POST for many shows.
    async fn get_episode_persisted(
        &self,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<String> {
        let variables = format!(
            r#"{{"showId":"{show_id}","translationType":"{}","episodeString":"{episode}"}}"#,
            mode.as_str()
        );
        let extensions = format!(
            r#"{{"persistedQuery":{{"version":1,"sha256Hash":"{EPISODE_QUERY_HASH}"}}}}"#
        );
        let resp = self
            .client
            .get(API_URL)
            .header("Referer", REFERER)
            .header("Origin", REFERER)
            .query(&[("variables", variables.as_str()), ("extensions", &extensions)])
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    async fn post_gql(&self, variables: Value, query: &str) -> Result<String> {
        let body = json!({ "variables": variables, "query": query });
        let resp = self
            .client
            .post(API_URL)
            .header("Referer", REFERER)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    /// Fetch a deobfuscated `/clock.json?...` embed path against BASE_HOST.
    async fn fetch_clock(&self, path: &str) -> Result<String> {
        let url = format!("https://{BASE_HOST}{path}");
        let resp = self
            .client
            .get(&url)
            .header("Referer", REFERER)
            .header("Origin", REFERER)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }

    /// The sources GraphQL response may inline the sourceUrls JSON or wrap it
    /// in an encrypted `tobeparsed` blob. Return the plaintext JSON either way.
    fn unwrap_sources_response(&self, body: &str) -> Result<String> {
        let v: Value = serde_json::from_str(body)
            .map_err(|e| Error::Provider(format!("sources: invalid json envelope: {e}")))?;
        if let Some(tbp) = find_tobeparsed(&v) {
            decrypt::decrypt_tobeparsed(tbp, &self.key)
        } else {
            Ok(body.to_string())
        }
    }
}

/// Best-effort playability ranking for ordering sources (0 = best).
///
/// A webview `<video>` can only play a direct media file (MP4/HLS), not an
/// embed *page*. allanime mixes both kinds into one list; ani-cli sidesteps
/// this by only handling a known subset. We can't extract embed pages here, so
/// we at least float the directly-playable sources to the top:
///   0 — looks like a direct media file / known direct CDN
///   1 — unknown (could be either)
///   2 — looks like an HTML embed page (ok.ru, mp4upload, /e/…, …)
fn playability_rank(s: &crate::models::VideoSource) -> u8 {
    let url = s.url.to_ascii_lowercase();
    let path = url.split(['?', '#']).next().unwrap_or(&url);

    let is_media_ext = path.ends_with(".m3u8")
        || path.ends_with(".mp4")
        || path.ends_with(".m4v")
        || path.ends_with(".mkv")
        || path.ends_with(".webm");
    let is_direct_cdn = url.contains("fast4speed")
        || url.contains("wixmp")
        || (url.contains("sharepoint.com") && url.contains("download.aspx"));
    if matches!(s.kind, crate::models::StreamKind::Hls) || is_media_ext || is_direct_cdn {
        return 0;
    }

    let is_embed_page = path.ends_with(".html")
        || url.contains("/e/")
        || url.contains("/embed")
        || url.contains("videoembed")
        || url.contains("ok.ru")
        || url.contains("mp4upload")
        || url.contains("vidnest")
        // Fragment-routed single-page embeds (e.g. allanime.uns.bio/#abc123).
        || url.contains("uns.bio");
    if is_embed_page {
        return 2;
    }
    1
}

/// Recursively search for a `tobeparsed` string field anywhere in the value.
fn find_tobeparsed(v: &Value) -> Option<&str> {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("tobeparsed") {
                return Some(s);
            }
            map.values().find_map(find_tobeparsed)
        }
        Value::Array(arr) => arr.iter().find_map(find_tobeparsed),
        _ => None,
    }
}

#[async_trait]
impl Provider for AllAnime {
    fn name(&self) -> &'static str {
        "allanime"
    }

    async fn search(&self, query: &str, mode: TranslationType) -> Result<Vec<AnimeSummary>> {
        let variables = json!({
            "search": { "allowAdult": false, "allowUnknown": false, "query": query },
            "limit": 40,
            "page": 1,
            "translationType": mode.as_str(),
            "countryOrigin": "ALL"
        });
        let body = self.post_gql(variables, SEARCH_GQL).await?;
        parse::parse_search(&body)
    }

    async fn episodes(&self, show_id: &str, mode: TranslationType) -> Result<Vec<String>> {
        let variables = json!({ "showId": show_id });
        let body = self.post_gql(variables, EPISODES_LIST_GQL).await?;
        parse::parse_episodes(&body, mode.as_str())
    }

    async fn sources(
        &self,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<Vec<VideoSource>> {
        // Primary: persisted GET (returns an encrypted `tobeparsed` blob).
        // Fallback: ad-hoc POST, matching ani-cli's two-step approach.
        let body = self.get_episode_persisted(show_id, episode, mode).await?;
        let refs = match self
            .unwrap_sources_response(&body)
            .and_then(|j| parse::parse_source_refs(&j))
        {
            Ok(refs) if !refs.is_empty() => refs,
            _ => {
                let variables = json!({
                    "showId": show_id,
                    "translationType": mode.as_str(),
                    "episodeString": episode
                });
                let body = self.post_gql(variables, EPISODE_EMBED_GQL).await?;
                let json = self.unwrap_sources_response(&body)?;
                parse::parse_source_refs(&json)?
            }
        };

        // Resolve each embed reference into concrete links, skipping refs
        // that fail rather than aborting the whole set.
        let mut tasks = Vec::new();
        for r in refs {
            tasks.push(self.resolve_ref(r));
        }
        let mut sources = Vec::new();
        for res in futures_join(tasks).await {
            if let Ok(mut links) = res {
                sources.append(&mut links);
            }
        }

        // Order so the default (first) source is one that actually plays in a
        // webview <video>. Many allanime "sources" are HTML embed pages we
        // cannot drop straight into a media element; sort those last. Within a
        // playability tier, prefer higher numeric resolution.
        sources.sort_by(|a, b| {
            playability_rank(a).cmp(&playability_rank(b)).then_with(|| {
                let qa: i64 = a.quality.parse().unwrap_or(-1);
                let qb: i64 = b.quality.parse().unwrap_or(-1);
                qb.cmp(&qa)
            })
        });
        Ok(sources)
    }
}

impl AllAnime {
    async fn resolve_ref(&self, r: parse::SourceRef) -> Result<Vec<VideoSource>> {
        // Direct-download hosts (ani-cli's fast4speed/Yt case) are already
        // playable and need no clock indirection.
        if r.url.starts_with("http://") || r.url.starts_with("https://") {
            let kind = if r.url.contains(".m3u8") {
                crate::models::StreamKind::Hls
            } else {
                crate::models::StreamKind::Mp4
            };
            return Ok(vec![VideoSource {
                provider_name: r.name,
                quality: "auto".into(),
                url: r.url,
                kind,
                referer: Some(REFERER.to_string()),
                subtitles: Vec::new(),
            }]);
        }

        let path = decrypt::deobfuscate_source_url(&r.url)?;
        if !path.starts_with('/') {
            return Ok(Vec::new());
        }
        let clock_body = self.fetch_clock(&path).await?;
        Ok(parse::parse_clock_links(&clock_body, &r.name, Some(REFERER)))
    }
}

/// Minimal join over a Vec of futures without pulling in the `futures` crate.
async fn futures_join<F, T>(tasks: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T>,
{
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(t.await);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_tobeparsed_nested() {
        let v: Value = serde_json::from_str(
            r#"{"data":{"episode":{"tobeparsed":"AAAA","other":1}}}"#,
        )
        .unwrap();
        assert_eq!(find_tobeparsed(&v), Some("AAAA"));
    }

    fn src(url: &str, kind: crate::models::StreamKind) -> crate::models::VideoSource {
        crate::models::VideoSource {
            provider_name: "t".into(),
            quality: "auto".into(),
            url: url.into(),
            kind,
            referer: None,
            subtitles: Vec::new(),
        }
    }

    #[test]
    fn playability_rank_orders_direct_media_before_embeds() {
        use crate::models::StreamKind::*;
        // Direct media / known CDNs -> 0
        assert_eq!(playability_rank(&src("https://cdn/x.mp4", Mp4)), 0);
        assert_eq!(playability_rank(&src("https://cdn/x.m3u8", Hls)), 0);
        assert_eq!(
            playability_rank(&src("https://tools.fast4speed.rsvp/media/1?Authorization=z", Mp4)),
            0
        );
        assert_eq!(
            playability_rank(&src("https://x.sharepoint.com/_layouts/15/download.aspx?id=1", Mp4)),
            0
        );
        // HTML embed pages -> 2
        assert_eq!(playability_rank(&src("https://ok.ru/videoembed/123", Mp4)), 2);
        assert_eq!(playability_rank(&src("https://mp4upload.com/embed-a.html", Mp4)), 2);
        assert_eq!(playability_rank(&src("https://vidnest.io/e/abc", Mp4)), 2);
        assert_eq!(playability_rank(&src("https://allanime.uns.bio/#abc", Mp4)), 2);
        // Unknown -> 1
        assert_eq!(playability_rank(&src("https://weird.host/thing", Mp4)), 1);
    }

    #[test]
    fn unwrap_sources_passes_through_plain() {
        let a = AllAnime::new();
        let plain = r#"{"data":{"episode":{"sourceUrls":[]}}}"#;
        assert_eq!(a.unwrap_sources_response(plain).unwrap(), plain);
    }
}
