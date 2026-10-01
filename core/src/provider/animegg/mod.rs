//! AnimeGG source (animegg.org).
//!
//! Server-rendered HTML with self-hosted, hard-subbed MP4s in several
//! resolutions, and both sub and dub. Nothing is signed or rotated, so its
//! failure modes (markup change, host down) are independent of allanime's.
//!
//! Flow:
//!   search   -> GET /search/?q=<q>, parse the result cards
//!   episodes -> GET /series/<slug>, parse the episode list
//!   sources  -> GET the episode page, pick the sub/dub tab, GET /embed/<id>,
//!               parse its `videoSources`
//!
//! The media URLs (`/play/<id>/video.mp4`) redirect to the CDN and are gated
//! on a site Referer, which every source therefore carries.

mod parse;

use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::provider::scrape::TtlCache;
use crate::provider::{Capabilities, Provider};
use crate::{Error, Result};
use async_trait::async_trait;
use parse::EpisodeEntry;
use reqwest::Client;
use std::time::Duration;

const BASE_URL: &str = "https://www.animegg.org";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";
/// Per-request ceiling, so one stalled connection can't hang a whole listing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// `sources` needs the show page to find an episode's URL; opening a show has
/// usually just fetched it.
const SERIES_TTL: Duration = Duration::from_secs(10 * 60);

pub struct AnimeGg {
    client: Client,
    base_url: String,
    series: TtlCache<Vec<EpisodeEntry>>,
}

impl Default for AnimeGg {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimeGg {
    /// Stable slug — persisted in namespaced ids and settings; never change it.
    pub const ID: &'static str = "animegg";
    pub const DISPLAY_NAME: &'static str = "AnimeGG";

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
            series: TtlCache::new(SERIES_TTL),
        }
    }

    fn referer(&self) -> String {
        format!("{}/", self.base_url)
    }

    async fn get(&self, path: &str, stage: &str) -> Result<String> {
        let resp = self
            .client
            .get(format!("{}{path}", self.base_url))
            .header("Referer", self.referer())
            .send()
            .await?
            .error_for_status()
            .map_err(|e| Error::Provider(format!("{stage}: {e}")))?;
        Ok(resp.text().await?)
    }

    /// The show's episode rows, from cache or the show page.
    async fn series(&self, show_id: &str, stage: &str) -> Result<Vec<EpisodeEntry>> {
        if let Some(cached) = self.series.get(show_id) {
            return Ok(cached);
        }
        let html = self
            .get(&format!("/series/{}", urlencoding::encode(show_id)), stage)
            .await?;
        let entries = parse::parse_series(&html)?;
        self.series.put(show_id, entries.clone());
        Ok(entries)
    }
}

/// The site's name for a translation type, as used in `data-version`.
fn version(mode: TranslationType) -> &'static str {
    match mode {
        TranslationType::Sub => "subbed",
        TranslationType::Dub => "dubbed",
    }
}

#[async_trait]
impl Provider for AnimeGg {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn display_name(&self) -> &'static str {
        Self::DISPLAY_NAME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            dub: true,
            carries_anilist_id: false,
            // Subtitles are burned into the video; there are no tracks.
            subtitles: false,
        }
    }

    async fn search(&self, query: &str, _mode: TranslationType) -> Result<Vec<AnimeSummary>> {
        let html = self
            .get(
                &format!("/search/?q={}", urlencoding::encode(query)),
                "search",
            )
            .await?;
        parse::parse_search(&html)
    }

    async fn episodes(&self, show_id: &str, mode: TranslationType) -> Result<Vec<String>> {
        Ok(self
            .series(show_id, "episodes")
            .await?
            .into_iter()
            .filter(|e| match mode {
                TranslationType::Sub => e.sub,
                TranslationType::Dub => e.dub,
            })
            .map(|e| e.number)
            .collect())
    }

    async fn sources(
        &self,
        show_id: &str,
        episode: &str,
        mode: TranslationType,
    ) -> Result<Vec<VideoSource>> {
        // The episode URL's prefix is not always the show slug, so it comes
        // from the show page rather than being guessed. An episode the list
        // doesn't have is a gap: empty, so dispatch falls across sources.
        let entries = self.series(show_id, "sources").await?;
        let Some(entry) = entries.iter().find(|e| e.number == episode) else {
            return Ok(Vec::new());
        };
        let page = self.get(&entry.path, "sources").await?;
        let tabs = parse::parse_episode_tabs(&page)?;

        let mut out = Vec::new();
        for tab in tabs.iter().filter(|t| t.version == version(mode)) {
            // One mirror failing must not hide the others.
            let Ok(embed) = self
                .get(&format!("/embed/{}", tab.embed_id), "sources")
                .await
            else {
                continue;
            };
            out.extend(parse::parse_embed(
                &embed,
                &self.base_url,
                &self.referer(),
                &tab.mirror,
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::testutil::serve;

    const SEARCH: &str = include_str!("fixtures/search.html");
    const SERIES: &str = include_str!("fixtures/series.html");
    const EPISODE: &str = include_str!("fixtures/episode.html");
    const EMBED: &str = include_str!("fixtures/embed.html");

    async fn recorded_site() -> crate::provider::testutil::TestServer {
        serve(|req| {
            // Every page fetch must look like in-site navigation.
            if req.header("referer").is_none() {
                return (403, String::new());
            }
            match req.path.as_str() {
                p if p.starts_with("/search/?q=") => (200, SEARCH.to_string()),
                "/series/sousou-no-frieren" => (200, SERIES.to_string()),
                "/sousou-no-frieren-episode-1" => (200, EPISODE.to_string()),
                "/embed/131519" => (200, EMBED.to_string()),
                // The dub embed is "down": the sub path must be unaffected.
                "/embed/131769" => (500, String::new()),
                _ => (404, String::new()),
            }
        })
        .await
    }

    #[tokio::test]
    async fn the_full_flow_runs_against_a_recorded_site() {
        let server = recorded_site().await;
        let p = AnimeGg::with_base_url(&server.base);

        let hits = p.search("frieren", TranslationType::Sub).await.unwrap();
        assert_eq!(hits[0].provider_id, "sousou-no-frieren");

        let eps = p
            .episodes("sousou-no-frieren", TranslationType::Sub)
            .await
            .unwrap();
        // 25 listed, one of them with no subbed video.
        assert_eq!(eps.len(), 24);
        assert_eq!(eps[0], "1");

        let sources = p
            .sources("sousou-no-frieren", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(sources.len(), 4);
        // Media URLs resolve against the origin in use.
        assert!(sources[0]
            .url
            .starts_with(&format!("{}/play/", server.base)));
        assert_eq!(
            sources[0].referer.as_deref(),
            Some(format!("{}/", server.base).as_str())
        );
    }

    #[tokio::test]
    async fn dub_lists_only_dubbed_episodes() {
        let server = recorded_site().await;
        let p = AnimeGg::with_base_url(&server.base);
        let sub = p
            .episodes("sousou-no-frieren", TranslationType::Sub)
            .await
            .unwrap();
        let dub = p
            .episodes("sousou-no-frieren", TranslationType::Dub)
            .await
            .unwrap();
        assert_eq!(dub.len(), 9);
        assert!(dub.len() < sub.len());
        // One show page serves both listings and the later sources call.
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn a_dead_mirror_or_missing_episode_is_empty_not_an_error() {
        let server = recorded_site().await;
        let p = AnimeGg::with_base_url(&server.base);
        // The dub embed 500s.
        let dub = p
            .sources("sousou-no-frieren", "1", TranslationType::Dub)
            .await
            .unwrap();
        assert!(dub.is_empty());
        // Episode not in the list at all.
        let missing = p
            .sources("sousou-no-frieren", "999", TranslationType::Sub)
            .await
            .unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn a_down_site_is_an_error_naming_the_stage() {
        let server = serve(|_| (503, "Service Unavailable".to_string())).await;
        let p = AnimeGg::with_base_url(&server.base);
        let err = p.search("x", TranslationType::Sub).await.unwrap_err();
        assert!(err.to_string().contains("search:"), "{err}");
        let err = p.episodes("x", TranslationType::Sub).await.unwrap_err();
        assert!(err.to_string().contains("episodes:"), "{err}");
        let err = p.sources("x", "1", TranslationType::Sub).await.unwrap_err();
        assert!(err.to_string().contains("sources:"), "{err}");
    }
}
