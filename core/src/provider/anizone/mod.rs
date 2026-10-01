//! AniZone source (anizone.to).
//!
//! A plain server-rendered site with no client-side crypto: search results and
//! episode lists are inlined in the page as JSON, and each episode page inlines
//! its player config — one adaptive HLS master plus soft subtitle tracks. That
//! makes it the opposite failure profile to allanime (nothing rotates), which
//! is exactly what a second source is for.
//!
//! Flow:
//!   search   -> GET /anime?search=<q>, parse the inlined rows
//!   episodes -> GET /anime/<slug>, then page the rest through Livewire
//!   sources  -> GET /anime/<slug>/<episode>, parse the player config
//!
//! Sub only: the HLS master does carry dub audio tracks for some shows, but
//! the player has no audio-track picker, so advertising dub would play
//! Japanese audio under a "dub" label.

mod parse;

use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::provider::scrape::TtlCache;
use crate::provider::{Capabilities, Provider};
use crate::{Error, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;

const BASE_URL: &str = "https://anizone.to";
/// Browser UA: the site sits behind Cloudflare, which is stricter with
/// library-default agents.
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";
/// Per-request ceiling, so one stalled connection can't hang a whole listing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The show page inlines 24 episodes; the rest arrive 24 at a time. This caps
/// a runaway cursor loop at ~2400 episodes, comfortably past the longest show.
const MAX_EPISODE_PAGES: usize = 100;
/// Opening a show lists its episodes and pressing play lists them again; keep
/// the (possibly many-request) list around for a while.
const EPISODES_TTL: Duration = Duration::from_secs(10 * 60);

pub struct AniZone {
    client: Client,
    base_url: String,
    episodes: TtlCache<Vec<String>>,
}

impl Default for AniZone {
    fn default() -> Self {
        Self::new()
    }
}

impl AniZone {
    /// Stable slug — persisted in namespaced ids and settings; never change it.
    pub const ID: &'static str = "anizone";
    pub const DISPLAY_NAME: &'static str = "AniZone";

    pub fn new() -> Self {
        Self::with_base_url(BASE_URL)
    }

    /// Point the scraper at another origin (a mirror, or a local test server).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            episodes: TtlCache::new(EPISODES_TTL),
        }
    }

    fn referer(&self) -> String {
        format!("{}/", self.base_url)
    }

    /// GET a page, returning its body and the cookies it set (Livewire calls
    /// must replay the session + XSRF cookies of the page they continue).
    async fn get_page(&self, url: &str, stage: &str) -> Result<(String, String)> {
        let resp = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| Error::Provider(format!("{stage}: {e}")))?;
        let cookies = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|c| c.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        Ok((resp.text().await?, cookies))
    }

    /// Fetch the episode pages after the first through Livewire's `loadPage`.
    ///
    /// Best-effort by design: the first 24 episodes are already in hand, so a
    /// failure here (expired token, changed component) returns what has been
    /// collected rather than failing the whole listing.
    async fn load_more_episodes(
        &self,
        show_id: &str,
        html: &str,
        cookies: &str,
        mut cursor: String,
        episodes: &mut Vec<String>,
    ) {
        let Some(ctx) = parse::parse_livewire_context(html, show_id) else {
            eprintln!("[anizone] episodes: no livewire context; list truncated");
            return;
        };
        let mut snapshot = ctx.snapshot;
        for _ in 0..MAX_EPISODE_PAGES {
            let body = json!({
                "_token": ctx.csrf,
                "components": [{
                    "snapshot": snapshot,
                    "updates": {},
                    "calls": [{ "path": "", "method": "loadPage", "params": [cursor] }],
                }],
            });
            let page = async {
                let text = self
                    .client
                    .post(format!("{}/livewire/update", self.base_url))
                    .header("X-Livewire", "")
                    .header("Referer", format!("{}/anime/{show_id}", self.base_url))
                    .header("Cookie", cookies)
                    .json(&body)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                parse::parse_livewire_page(&text)
            }
            .await;
            let page = match page {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[anizone] episodes: paging stopped early: {e}");
                    return;
                }
            };
            episodes.extend(page.page.episodes);
            snapshot = page.snapshot;
            match page.page.next_cursor {
                Some(next) => cursor = next,
                None => return,
            }
        }
    }
}

#[async_trait]
impl Provider for AniZone {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            dub: false,
            carries_anilist_id: false,
            subtitles: true,
        }
    }

    async fn search(&self, query: &str, mode: TranslationType) -> Result<Vec<AnimeSummary>> {
        if mode == TranslationType::Dub {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/anime?search={}",
            self.base_url,
            urlencoding::encode(query)
        );
        let (html, _) = self.get_page(&url, "search").await?;
        parse::parse_search(&html)
    }

    async fn episodes(&self, show_id: &str, mode: TranslationType) -> Result<Vec<String>> {
        if mode == TranslationType::Dub {
            return Ok(Vec::new());
        }
        if let Some(cached) = self.episodes.get(show_id) {
            return Ok(cached);
        }
        let url = format!("{}/anime/{}", self.base_url, urlencoding::encode(show_id));
        let (html, cookies) = self.get_page(&url, "episodes").await?;
        let first = parse::parse_episode_page(&html)?;
        let mut episodes = first.episodes;
        if let Some(cursor) = first.next_cursor {
            self.load_more_episodes(show_id, &html, &cookies, cursor, &mut episodes)
                .await;
        }
        episodes.dedup();
        self.episodes.put(show_id, episodes.clone());
        Ok(episodes)
    }

    async fn sources(
        &self,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<Vec<VideoSource>> {
        if mode == TranslationType::Dub {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/anime/{}/{}",
            self.base_url,
            urlencoding::encode(show_id),
            urlencoding::encode(episode)
        );
        let resp = self.client.get(&url).send().await?;
        // An episode this source doesn't have is a gap, not a failure: return
        // nothing so the dispatch layer falls across to another source.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let html = resp
            .error_for_status()
            .map_err(|e| Error::Provider(format!("sources: {e}")))?
            .text()
            .await?;
        parse::parse_player(&html, &self.referer())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::testutil::{serve, serve_with_cookie};

    const ANIME: &str = include_str!("fixtures/anime.html");
    const EPISODE: &str = include_str!("fixtures/episode.html");
    const LIVEWIRE: &str = include_str!("fixtures/livewire_page.json");
    const SEARCH: &str = include_str!("fixtures/search.html");

    #[tokio::test]
    async fn the_full_flow_runs_against_a_recorded_site() {
        let server = serve(|req| {
            if req.path.starts_with("/anime?search=") {
                (200, SEARCH.to_string())
            } else if req.path == "/anime/mdkytdqp" {
                (200, ANIME.to_string())
            } else if req.method == "POST" && req.path == "/livewire/update" {
                (200, LIVEWIRE.to_string())
            } else if req.path == "/anime/mdkytdqp/1" {
                (200, EPISODE.to_string())
            } else {
                (404, String::new())
            }
        })
        .await;
        let p = AniZone::with_base_url(&server.base);

        let hits = p.search("frieren", TranslationType::Sub).await.unwrap();
        assert_eq!(hits[0].provider_id, "mdkytdqp");

        // 24 inlined + 4 from the Livewire page.
        let eps = p.episodes("mdkytdqp", TranslationType::Sub).await.unwrap();
        assert_eq!(eps.len(), 28);
        assert_eq!(eps.last().map(String::as_str), Some("28"));

        let sources = p
            .sources("mdkytdqp", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(sources.len(), 1);
        // The referer follows the origin in use, not the baked-in one.
        assert_eq!(
            sources[0].referer.as_deref(),
            Some(format!("{}/", server.base).as_str())
        );

        // A missing episode is a gap (empty), not an error.
        let missing = p
            .sources("mdkytdqp", "999", TranslationType::Sub)
            .await
            .unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn the_livewire_call_replays_the_page_session() {
        let server = serve_with_cookie("sess=abc; Path=/; HttpOnly", |req| {
            if req.method == "POST" {
                // Reject the call unless it looks like the one Livewire
                // accepts: the page's cookie, its CSRF token, and the marker
                // header.
                let ok = req.header("cookie").is_some_and(|c| c.contains("sess=abc"))
                    && req.body.contains("\"_token\"")
                    && req.body.contains("loadPage")
                    && req.header("x-livewire").is_some();
                (if ok { 200 } else { 419 }, LIVEWIRE.to_string())
            } else {
                (200, ANIME.to_string())
            }
        })
        .await;
        let p = AniZone::with_base_url(&server.base);
        let eps = p.episodes("mdkytdqp", TranslationType::Sub).await.unwrap();
        assert_eq!(eps.len(), 28, "the paged request was rejected");
    }

    #[tokio::test]
    async fn a_failed_page_load_keeps_the_episodes_already_in_hand() {
        let server = serve(|req| {
            if req.method == "POST" {
                (419, "Page Expired".to_string())
            } else {
                (200, ANIME.to_string())
            }
        })
        .await;
        let p = AniZone::with_base_url(&server.base);
        let eps = p.episodes("mdkytdqp", TranslationType::Sub).await.unwrap();
        assert_eq!(eps.len(), 24);
    }

    #[tokio::test]
    async fn the_episode_list_is_cached_between_calls() {
        let server = serve(|req| {
            if req.method == "POST" {
                (200, LIVEWIRE.to_string())
            } else {
                (200, ANIME.to_string())
            }
        })
        .await;
        let p = AniZone::with_base_url(&server.base);
        p.episodes("mdkytdqp", TranslationType::Sub).await.unwrap();
        let after_first = server.hits();
        p.episodes("mdkytdqp", TranslationType::Sub).await.unwrap();
        assert_eq!(server.hits(), after_first, "second listing hit the network");
    }

    #[tokio::test]
    async fn a_down_site_is_an_error_and_dub_is_simply_empty() {
        let server = serve(|_| (503, "Service Unavailable".to_string())).await;
        let p = AniZone::with_base_url(&server.base);
        let err = p.search("x", TranslationType::Sub).await.unwrap_err();
        assert!(err.to_string().contains("search:"), "{err}");
        assert!(p.episodes("x", TranslationType::Sub).await.is_err());
        assert!(p.sources("x", "1", TranslationType::Sub).await.is_err());
        // Sub-only: dub requests never reach the network.
        let before = server.hits();
        assert!(p
            .search("x", TranslationType::Dub)
            .await
            .unwrap()
            .is_empty());
        assert!(p
            .sources("x", "1", TranslationType::Dub)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(server.hits(), before);
    }
}
