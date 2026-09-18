//! Dispatch across sources: namespacing, parallel search, and failover.
//!
//! Scrapers deal in their own bare show ids and know nothing about each other.
//! Everything that turns that into a multi-source app lives here:
//!
//! - ids crossing this boundary are namespaced on the way out and stripped on
//!   the way in, so a scraper never sees a prefix and a caller never sees a
//!   bare id;
//! - `VideoSource.source` is stamped here rather than trusted from a scraper;
//! - a source that is slow, broken, or mid-rotation is skipped, never fatal —
//!   the whole point of having more than one.

use super::id::SourceId;
use super::rank::playability_rank;
use super::registry::Registry;
use crate::db::Database;
use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::{Error, Result};
use futures_util::future::join_all;
use std::sync::Arc;
use std::time::Duration;

/// How long any one source gets before the others are used without it. Long
/// enough for a cold bootstrap, short enough that one hung source doesn't hold
/// up a search.
pub const PER_SOURCE_TIMEOUT: Duration = Duration::from_secs(12);

/// Run `fut`, treating a timeout as a source-level failure rather than a hang.
async fn bounded<T>(fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    match tokio::time::timeout(PER_SOURCE_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(Error::Provider("source timed out".into())),
    }
}

/// Search every enabled source at once and merge the hits.
///
/// Results are namespaced and interleaved by source rank: the first source's
/// best hit, then the second's, and so on. That keeps the preferred source at
/// the top without burying a better match from a lower-ranked one, which a
/// naive concatenation would do for a show the first source barely has.
///
/// A source that errors or times out contributes nothing and does not fail the
/// search — with several sources, a partial result is still useful.
pub async fn search_all(
    registry: &Registry,
    db: &Database,
    query: &str,
    mode: TranslationType,
) -> Vec<AnimeSummary> {
    let sources = enabled_for(registry, db, mode);
    let hits = join_all(
        sources
            .iter()
            .map(|p| async move { bounded(p.search(query, mode)).await.unwrap_or_default() }),
    )
    .await;

    let mut per_source: Vec<Vec<AnimeSummary>> = hits
        .into_iter()
        .zip(sources.iter())
        .map(|(mut v, p)| {
            for s in &mut v {
                s.provider_id = SourceId::new(p.id(), &s.provider_id).to_string();
            }
            v
        })
        .collect();

    let mut out = Vec::new();
    for round in 0.. {
        let mut any = false;
        for list in &mut per_source {
            if round < list.len() {
                out.push(list[round].clone());
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    out
}

/// Episode list for a namespaced show id, from the source that owns it.
pub async fn episodes(
    registry: &Registry,
    show_id: &str,
    mode: TranslationType,
) -> Result<Vec<String>> {
    let id = SourceId::parse(show_id);
    let p = registry
        .get(&id.source)
        .ok_or_else(|| Error::Provider(format!("unknown source {:?}", id.source)))?;
    p.episodes(&id.show, mode).await
}

/// Resolve one episode to playable links, falling across sources.
///
/// The named source is tried first (it is what the user is looking at). If it
/// yields nothing — rotated, episode missing, host down — every *other* source
/// mapped to the same AniList show is tried in parallel and the results merged
/// into one ranked list. The watch page's existing per-URL fall-forward then
/// walks that list, so a broken source degrades into a slightly slower start
/// rather than an outage.
///
/// The original error is preserved when nothing anywhere resolves, so a
/// rotation still surfaces as a rotation and not as a bland "no sources".
pub async fn sources_for(
    registry: &Registry,
    db: &Database,
    show_id: &str,
    episode: &str,
    mode: TranslationType,
) -> Result<Vec<VideoSource>> {
    let id = SourceId::parse(show_id);
    let primary = registry
        .get(&id.source)
        .ok_or_else(|| Error::Provider(format!("unknown source {:?}", id.source)))?;

    let first = bounded(primary.sources(&id.show, episode, mode)).await;
    if let Ok(v) = &first {
        if !v.is_empty() {
            return Ok(finish(v.clone(), primary.id()));
        }
    }

    // Siblings: other sources already mapped to the same AniList show.
    let siblings: Vec<(String, String)> = db
        .anilist_id_for_provider(show_id)
        .ok()
        .flatten()
        .and_then(|anilist_id| db.sources_for_anilist(anilist_id).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(source, _)| source != &id.source)
        .collect();

    let enabled = enabled_for(registry, db, mode);
    let attempts: Vec<_> = siblings
        .iter()
        .filter_map(|(source, sid)| {
            enabled
                .iter()
                .find(|p| p.id() == source)
                .map(|p| (p.clone(), SourceId::parse(sid).show))
        })
        .collect();

    let results = join_all(
        attempts
            .iter()
            .map(|(p, show)| async move { bounded(p.sources(show, episode, mode)).await }),
    )
    .await;

    let mut merged = Vec::new();
    for (res, (p, _)) in results.into_iter().zip(attempts.iter()) {
        if let Ok(v) = res {
            merged.extend(finish(v, p.id()));
        }
    }
    if !merged.is_empty() {
        merged.sort_by(rank_then_quality);
        return Ok(merged);
    }

    // Nothing anywhere: report what the source the user asked for said.
    first.map(|v| finish(v, primary.id()))
}

/// Sources enabled for this request. A source that cannot do dub is skipped
/// for a dub request rather than asked and returning nothing.
fn enabled_for(
    registry: &Registry,
    db: &Database,
    mode: TranslationType,
) -> Vec<Arc<dyn super::Provider>> {
    registry
        .enabled_ordered(db)
        .into_iter()
        .filter(|p| mode != TranslationType::Dub || p.capabilities().dub)
        .collect()
}

/// Stamp the owning source and order by what will actually play.
fn finish(mut v: Vec<VideoSource>, source: &str) -> Vec<VideoSource> {
    for s in &mut v {
        s.source = source.to_string();
    }
    v.sort_by(rank_then_quality);
    v
}

fn rank_then_quality(a: &VideoSource, b: &VideoSource) -> std::cmp::Ordering {
    playability_rank(a).cmp(&playability_rank(b)).then_with(|| {
        let qa: i64 = a.quality.parse().unwrap_or(-1);
        let qb: i64 = b.quality.parse().unwrap_or(-1);
        qb.cmp(&qa)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{StreamKind, SubtitleTrack};
    use crate::provider::{Capabilities, Provider};
    use async_trait::async_trait;

    /// A scriptable stand-in: it can carry hits, be episode-specific, fail, or
    /// hang past the per-source timeout.
    struct Stub {
        id: &'static str,
        hits: Vec<&'static str>,
        /// Episodes this source can actually resolve.
        playable: Vec<&'static str>,
        dub: bool,
        fails: bool,
        hangs: bool,
    }

    impl Stub {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                hits: vec![],
                playable: vec!["1"],
                dub: true,
                fails: false,
                hangs: false,
            }
        }
        fn hits(mut self, h: &[&'static str]) -> Self {
            self.hits = h.to_vec();
            self
        }
        fn playable(mut self, e: &[&'static str]) -> Self {
            self.playable = e.to_vec();
            self
        }
        fn sub_only(mut self) -> Self {
            self.dub = false;
            self
        }
        fn failing(mut self) -> Self {
            self.fails = true;
            self
        }
        fn hanging(mut self) -> Self {
            self.hangs = true;
            self
        }
    }

    #[async_trait]
    impl Provider for Stub {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            self.id
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                dub: self.dub,
                carries_anilist_id: false,
                subtitles: true,
            }
        }
        async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
            if self.fails {
                return Err(Error::Provider("boom".into()));
            }
            if self.hangs {
                tokio::time::sleep(PER_SOURCE_TIMEOUT * 3).await;
            }
            Ok(self
                .hits
                .iter()
                .map(|h| AnimeSummary {
                    provider_id: (*h).to_string(),
                    title: (*h).to_string(),
                    title_english: None,
                    cover_url: None,
                    available_episodes: 1,
                    anilist_id: None,
                })
                .collect())
        }
        async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
            Ok(self.playable.iter().map(|e| e.to_string()).collect())
        }
        async fn sources(
            &self,
            _s: &str,
            episode: &str,
            _m: TranslationType,
        ) -> Result<Vec<VideoSource>> {
            if self.fails {
                return Err(Error::Provider("rotated".into()));
            }
            if !self.playable.contains(&episode) {
                return Ok(vec![]);
            }
            Ok(vec![VideoSource {
                // Deliberately wrong: the dispatch layer must overwrite it.
                source: "lying".into(),
                provider_name: self.id.into(),
                quality: "1080".into(),
                url: format!("https://{}/{episode}.m3u8", self.id),
                kind: StreamKind::Hls,
                referer: None,
                subtitles: Vec::<SubtitleTrack>::new(),
            }])
        }
    }

    fn reg(stubs: Vec<Stub>) -> Registry {
        Registry::new(
            stubs
                .into_iter()
                .map(|s| Arc::new(s) as Arc<dyn Provider>)
                .collect(),
        )
    }

    fn db() -> Database {
        Database::open_in_memory().unwrap()
    }

    /// Map one AniList show to several sources.
    fn map(db: &Database, anilist_id: i64, pairs: &[(&str, &str)]) {
        for (source, show) in pairs {
            let id = format!("{source}:{show}");
            db.cache_anime(&id, "T", None, None, None).unwrap();
            db.link_provider_anilist(&id, anilist_id).unwrap();
        }
    }

    #[tokio::test]
    async fn search_namespaces_ids_so_results_route_back_to_their_source() {
        let r = reg(vec![
            Stub::new("allanime").hits(&["a1"]),
            Stub::new("hianime").hits(&["h1"]),
        ]);
        let got = search_all(&r, &db(), "one piece", TranslationType::Sub).await;
        let ids: Vec<_> = got.iter().map(|s| s.provider_id.as_str()).collect();
        assert_eq!(ids, ["allanime:a1", "hianime:h1"]);
    }

    #[tokio::test]
    async fn search_interleaves_rather_than_concatenating() {
        // A naive concat would bury hianime's best hit behind allanime's worst.
        let r = reg(vec![
            Stub::new("allanime").hits(&["a1", "a2", "a3"]),
            Stub::new("hianime").hits(&["h1"]),
        ]);
        let got = search_all(&r, &db(), "q", TranslationType::Sub).await;
        let ids: Vec<_> = got.iter().map(|s| s.provider_id.as_str()).collect();
        assert_eq!(
            ids,
            ["allanime:a1", "hianime:h1", "allanime:a2", "allanime:a3"]
        );
    }

    #[tokio::test]
    async fn a_broken_source_does_not_fail_the_search() {
        let r = reg(vec![
            Stub::new("allanime").failing(),
            Stub::new("hianime").hits(&["h1"]),
        ]);
        let got = search_all(&r, &db(), "q", TranslationType::Sub).await;
        let ids: Vec<_> = got.iter().map(|s| s.provider_id.as_str()).collect();
        assert_eq!(ids, ["hianime:h1"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_source_is_dropped_instead_of_holding_up_the_search() {
        let r = reg(vec![
            Stub::new("allanime").hanging(),
            Stub::new("hianime").hits(&["h1"]),
        ]);
        let got = search_all(&r, &db(), "q", TranslationType::Sub).await;
        let ids: Vec<_> = got.iter().map(|s| s.provider_id.as_str()).collect();
        assert_eq!(ids, ["hianime:h1"]);
    }

    #[tokio::test]
    async fn a_sub_only_source_is_skipped_for_dub_requests() {
        let r = reg(vec![
            Stub::new("animepahe").sub_only().hits(&["p1"]),
            Stub::new("hianime").hits(&["h1"]),
        ]);
        let sub = search_all(&r, &db(), "q", TranslationType::Sub).await;
        assert_eq!(sub.len(), 2);
        let dub = search_all(&r, &db(), "q", TranslationType::Dub).await;
        let ids: Vec<_> = dub.iter().map(|s| s.provider_id.as_str()).collect();
        assert_eq!(ids, ["hianime:h1"]);
    }

    #[tokio::test]
    async fn the_named_source_is_used_when_it_works() {
        let r = reg(vec![Stub::new("allanime"), Stub::new("hianime")]);
        let d = db();
        map(&d, 21, &[("allanime", "a1"), ("hianime", "h1")]);
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "allanime");
    }

    #[tokio::test]
    async fn a_lying_scraper_cannot_mislabel_its_own_source() {
        let r = reg(vec![Stub::new("allanime")]);
        let d = db();
        map(&d, 21, &[("allanime", "a1")]);
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got[0].source, "allanime", "dispatch must stamp the source");
    }

    #[tokio::test]
    async fn a_rotated_source_falls_across_to_a_sibling() {
        let r = reg(vec![Stub::new("allanime").failing(), Stub::new("hianime")]);
        let d = db();
        map(&d, 21, &[("allanime", "a1"), ("hianime", "h1")]);
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "hianime");
    }

    #[tokio::test]
    async fn an_episode_one_source_lacks_falls_across_too() {
        // Not an outage — just a gap in one source's catalogue.
        let r = reg(vec![
            Stub::new("allanime").playable(&["1"]),
            Stub::new("hianime").playable(&["1", "2"]),
        ]);
        let d = db();
        map(&d, 21, &[("allanime", "a1"), ("hianime", "h1")]);
        let got = sources_for(&r, &d, "allanime:a1", "2", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got[0].source, "hianime");
    }

    #[tokio::test]
    async fn failover_merges_every_sibling_not_just_the_first() {
        let r = reg(vec![
            Stub::new("allanime").failing(),
            Stub::new("hianime"),
            Stub::new("animepahe"),
        ]);
        let d = db();
        map(
            &d,
            21,
            &[("allanime", "a1"), ("hianime", "h1"), ("animepahe", "p1")],
        );
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        let mut sources: Vec<_> = got.iter().map(|s| s.source.as_str()).collect();
        sources.sort();
        assert_eq!(sources, ["animepahe", "hianime"]);
    }

    #[tokio::test]
    async fn a_disabled_sibling_is_not_used_for_failover() {
        let r = reg(vec![Stub::new("allanime").failing(), Stub::new("hianime")]);
        let d = db();
        map(&d, 21, &[("allanime", "a1"), ("hianime", "h1")]);
        r.set_enabled(&d, "hianime", false).unwrap();
        assert!(
            sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn with_no_sibling_the_original_error_survives() {
        // A rotation must still read as a rotation, not as a bland empty list —
        // the outage UI keys off it.
        let r = reg(vec![Stub::new("allanime").failing()]);
        let d = db();
        map(&d, 21, &[("allanime", "a1")]);
        let err = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rotated"), "got {err}");
    }

    #[tokio::test]
    async fn an_unknown_source_prefix_is_a_clear_error() {
        let r = reg(vec![Stub::new("allanime")]);
        let err = sources_for(&r, &db(), "gogoanime:x", "1", TranslationType::Sub)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("gogoanime"), "got {err}");
    }

    #[tokio::test]
    async fn a_legacy_bare_id_still_resolves_through_allanime() {
        let r = reg(vec![Stub::new("allanime")]);
        let d = db();
        let got = sources_for(&r, &d, "ReooPAxPMsHM4KPMY", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got[0].source, "allanime");
    }

    #[tokio::test]
    async fn episodes_dispatch_to_the_owning_source() {
        let r = reg(vec![
            Stub::new("allanime").playable(&["1"]),
            Stub::new("hianime").playable(&["1", "2", "3"]),
        ]);
        let got = episodes(&r, "hianime:h1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got, ["1", "2", "3"]);
    }
}
