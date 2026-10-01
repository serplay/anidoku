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
//!   the whole point of having more than one;
//! - when the source a show was opened from fails, the same show is found on
//!   the others (by link, or by a strict title match) and played from there.

use super::id::SourceId;
use super::rank::playability_rank;
use super::registry::Registry;
use crate::db::Database;
use crate::models::{AnimeSummary, TranslationType, VideoSource};
use crate::sync::matching::{best_provider_match, confident_provider_match};
use crate::{Error, Result};
use futures_util::future::join_all;
use std::sync::Arc;
use std::time::Duration;

/// How long any one source gets before the others are used without it. Long
/// enough for a cold bootstrap, short enough that one hung source doesn't hold
/// up a search.
pub const PER_SOURCE_TIMEOUT: Duration = Duration::from_secs(12);

/// Ceiling for listing a show's episodes. Longer than [`PER_SOURCE_TIMEOUT`]
/// because some sources page through a long-running show request by request.
pub const EPISODES_TIMEOUT: Duration = Duration::from_secs(45);

/// Run `fut`, treating a timeout as a source-level failure rather than a hang.
async fn bounded<T>(fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    bounded_by(PER_SOURCE_TIMEOUT, fut).await
}

async fn bounded_by<T>(
    limit: Duration,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(limit, fut).await {
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

/// Episode list for a namespaced show id.
///
/// Comes from the source that owns the id; if that source is down (or simply
/// has nothing in this translation), the list comes from another source that
/// has the same show, so a show page still opens during an outage.
pub async fn episodes(
    registry: &Registry,
    db: &Database,
    show_id: &str,
    mode: TranslationType,
) -> Result<Vec<String>> {
    let id = SourceId::parse(show_id);
    let primary = registry
        .get(&id.source)
        .ok_or_else(|| Error::Provider(format!("unknown source {:?}", id.source)))?;

    let first = bounded_by(EPISODES_TIMEOUT, primary.episodes(&id.show, mode)).await;
    if matches!(&first, Ok(v) if !v.is_empty()) {
        return first;
    }
    for (p, show) in siblings(registry, db, &id, mode).await {
        if let Ok(v) = bounded_by(EPISODES_TIMEOUT, p.episodes(&show, mode)).await {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    first
}

/// Resolve one episode to playable links, falling across sources.
///
/// The named source is tried first (it is what the user is looking at). If it
/// yields nothing — rotated, episode missing, host down — every *other* source
/// that has the same show is tried in parallel and the results merged into one
/// ranked list. The watch page's existing per-URL fall-forward then walks that
/// list, so a broken source degrades into a slightly slower start rather than
/// an outage.
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

    let attempts = siblings(registry, db, &id, mode).await;
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

/// Every other enabled source that has the same show as `id`, with its own
/// id for it.
///
/// Sources already linked to the show's AniList entry are known. The rest are
/// *discovered*: searched by the show's cached titles and accepted only on a
/// confident, same-season match. Without this, failover would only ever work
/// for shows the user happened to open while every source was healthy —
/// exactly not the case that matters.
async fn siblings(
    registry: &Registry,
    db: &Database,
    id: &SourceId,
    mode: TranslationType,
) -> Vec<(Arc<dyn super::Provider>, String)> {
    let key = id.to_string();
    let anilist_id = db.anilist_id_for_provider(&key).ok().flatten();
    let linked: Vec<(String, String)> = anilist_id
        .and_then(|a| db.sources_for_anilist(a).ok())
        .unwrap_or_default();

    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for p in enabled_for(registry, db, mode) {
        if p.id() == id.source {
            continue;
        }
        match linked.iter().find(|(source, _)| source == p.id()) {
            Some((_, sid)) => known.push((p, SourceId::parse(sid).show)),
            None if !registry.recently_missed(&key, p.id()) => unknown.push(p),
            None => {}
        }
    }
    if unknown.is_empty() {
        return known;
    }

    // Titles to search by: whatever the owning source called the show.
    let Some((romaji, english, _)) = db.get_cached_anime(&key).ok().flatten() else {
        return known;
    };
    let titles: Vec<&str> = std::iter::once(romaji.as_str())
        .chain(english.as_deref())
        .filter(|t| !t.trim().is_empty())
        .collect();
    let episodes = anilist_id
        .and_then(|a| db.media_episode_count(a).ok().flatten())
        .and_then(|n| u32::try_from(n).ok());

    let found = join_all(unknown.iter().map(|p| {
        let titles = &titles;
        async move {
            for query in titles {
                let hits = bounded(p.search(query, mode)).await.unwrap_or_default();
                if let Some(m) = confident_provider_match(titles, episodes, &hits) {
                    return Some(m.clone());
                }
            }
            None
        }
    }))
    .await;

    for (p, hit) in unknown.into_iter().zip(found) {
        let Some(hit) = hit else {
            registry.note_miss(&key, p.id());
            continue;
        };
        // Persist, so the next play (and the Settings/picker UI) sees the
        // sibling without searching again.
        let sibling = SourceId::new(p.id(), &hit.provider_id).to_string();
        let _ = db.cache_anime(
            &sibling,
            &hit.title,
            hit.title_english.as_deref(),
            hit.cover_url.as_deref(),
            Some(hit.available_episodes),
        );
        if let Some(anilist_id) = anilist_id {
            let _ = db.link_provider_anilist(&sibling, anilist_id);
        }
        known.push((p, hit.provider_id));
    }
    known
}

/// Link every source's version of an AniList show from one aggregated search,
/// and return the one to open.
///
/// The show to open is picked as before (a carried AniList id is exact;
/// otherwise the best title match). What this adds is linking the *other*
/// sources' matches too, so when the opened source later breaks, failover
/// already knows where else the show lives. Those extra links use the strict
/// same-season matcher: they are acted on without the user ever seeing them.
pub fn link_matches(
    db: &Database,
    anilist_id: i64,
    titles: &[&str],
    episodes: Option<u32>,
    results: &[AnimeSummary],
) -> Option<AnimeSummary> {
    let title = titles.first().copied().unwrap_or_default();
    let exact = |pool: &[AnimeSummary]| -> Option<AnimeSummary> {
        pool.iter()
            .find(|c| c.anilist_id == Some(anilist_id))
            .cloned()
    };
    let primary = exact(results)
        .or_else(|| confident_provider_match(titles, episodes, results).cloned())
        .or_else(|| best_provider_match(anilist_id, title, episodes, results).cloned())?;
    // Linked first, so it becomes the preferred source for the show.
    let _ = db.link_provider_anilist(&primary.provider_id, anilist_id);

    let primary_source = SourceId::parse(&primary.provider_id).source;
    let mut seen = vec![primary_source];
    for r in results {
        let source = SourceId::parse(&r.provider_id).source;
        if seen.contains(&source) {
            continue;
        }
        let pool: Vec<AnimeSummary> = results
            .iter()
            .filter(|c| SourceId::parse(&c.provider_id).source == source)
            .cloned()
            .collect();
        if let Some(m) =
            exact(&pool).or_else(|| confident_provider_match(titles, episodes, &pool).cloned())
        {
            let _ = db.link_provider_anilist(&m.provider_id, anilist_id);
        }
        seen.push(source);
    }
    Some(primary)
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
        /// Searches served, to prove a miss is not re-asked.
        searches: std::sync::atomic::AtomicUsize,
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
                searches: std::sync::atomic::AtomicUsize::new(0),
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
            self.searches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
            if self.fails {
                return Err(Error::Provider("down".into()));
            }
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
        let got = episodes(&r, &db(), "hianime:h1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got, ["1", "2", "3"]);
    }

    // ---- failover for shows no other source was ever linked to ----

    /// The show as the user opened it: cached under its owning source, with
    /// the titles a search would have stored. Deliberately NOT linked to any
    /// other source.
    fn opened(db: &Database, show_id: &str, romaji: &str, english: Option<&str>) {
        db.cache_anime(show_id, romaji, english, None, None)
            .unwrap();
    }

    #[tokio::test]
    async fn an_unlinked_show_is_found_on_another_source_by_title() {
        // The outage case that matters: allanime is down and the show was only
        // ever opened from allanime.
        let r = reg(vec![
            Stub::new("allanime").failing(),
            Stub::new("anizone").hits(&["Unrelated Show", "Sousou no Frieren"]),
        ]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "anizone");
    }

    #[tokio::test]
    async fn discovery_tries_the_english_title_when_the_romaji_finds_nothing() {
        // A source that indexes the show only under its English name.
        struct EnglishOnly;
        #[async_trait]
        impl Provider for EnglishOnly {
            fn id(&self) -> &'static str {
                "animegg"
            }
            fn display_name(&self) -> &'static str {
                "animegg"
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    dub: true,
                    carries_anilist_id: false,
                    subtitles: false,
                }
            }
            async fn search(&self, q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
                Ok(if q.contains("Beyond") {
                    vec![AnimeSummary {
                        provider_id: "frieren".into(),
                        title: "Frieren: Beyond Journey's End".into(),
                        title_english: None,
                        cover_url: None,
                        available_episodes: 28,
                        anilist_id: None,
                    }]
                } else {
                    vec![]
                })
            }
            async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
                Ok(vec!["1".into()])
            }
            async fn sources(
                &self,
                show: &str,
                _e: &str,
                _m: TranslationType,
            ) -> Result<Vec<VideoSource>> {
                Ok(vec![VideoSource {
                    source: String::new(),
                    provider_name: "gg".into(),
                    quality: "720".into(),
                    url: format!("https://gg/{show}.mp4"),
                    kind: StreamKind::Mp4,
                    referer: None,
                    subtitles: vec![],
                }])
            }
        }
        let r = Registry::new(vec![
            Arc::new(Stub::new("allanime").failing()) as Arc<dyn Provider>,
            Arc::new(EnglishOnly),
        ]);
        let d = db();
        opened(
            &d,
            "allanime:a1",
            "Sousou no Frieren",
            Some("Frieren: Beyond Journey's End"),
        );
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got[0].url, "https://gg/frieren.mp4");
    }

    #[tokio::test]
    async fn discovery_refuses_a_different_season() {
        // Wrong video silently playing is worse than an honest failure.
        let r = reg(vec![
            Stub::new("allanime").failing(),
            Stub::new("anizone").hits(&["Sousou no Frieren"]),
        ]);
        let d = db();
        opened(&d, "allanime:a2", "Sousou no Frieren 2nd Season", None);
        let err = sources_for(&r, &d, "allanime:a2", "1", TranslationType::Sub)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rotated"), "got {err}");
    }

    #[tokio::test]
    async fn a_discovered_sibling_is_linked_so_it_is_not_searched_for_again() {
        let anizone = Arc::new(Stub::new("anizone").hits(&["Sousou no Frieren"]));
        let r = Registry::new(vec![
            Arc::new(Stub::new("allanime").failing()) as Arc<dyn Provider>,
            anizone.clone(),
        ]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        d.link_provider_anilist("allanime:a1", 154587).unwrap();

        for _ in 0..3 {
            sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
                .await
                .unwrap();
        }
        assert_eq!(
            anizone.searches.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        // And the link is real: the show now lists both sources.
        let linked = d.sources_for_anilist(154587).unwrap();
        assert_eq!(
            linked,
            [
                ("allanime".to_string(), "allanime:a1".to_string()),
                (
                    "anizone".to_string(),
                    "anizone:Sousou no Frieren".to_string()
                ),
            ]
        );
        // allanime stays the preferred source: failover is not a re-pin.
        assert_eq!(
            d.provider_id_for_anilist(154587).unwrap().as_deref(),
            Some("allanime:a1")
        );
    }

    #[tokio::test]
    async fn a_source_without_the_show_is_not_re_searched_on_every_episode() {
        let anizone = Arc::new(Stub::new("anizone").hits(&["Something Else"]));
        let r = Registry::new(vec![
            Arc::new(Stub::new("allanime").failing()) as Arc<dyn Provider>,
            anizone.clone(),
        ]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        for ep in ["1", "2", "3"] {
            assert!(sources_for(&r, &d, "allanime:a1", ep, TranslationType::Sub)
                .await
                .is_err());
        }
        assert_eq!(
            anizone.searches.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn a_healthy_source_never_triggers_discovery() {
        let anizone = Arc::new(Stub::new("anizone").hits(&["Sousou no Frieren"]));
        let r = Registry::new(vec![
            Arc::new(Stub::new("allanime")) as Arc<dyn Provider>,
            anizone.clone(),
        ]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        episodes(&r, &d, "allanime:a1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(
            anizone.searches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn every_other_source_being_down_is_still_just_the_original_error() {
        let r = reg(vec![
            Stub::new("allanime").failing(),
            Stub::new("anizone").failing(),
            Stub::new("animegg").failing(),
        ]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        let err = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rotated"), "got {err}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_primary_fails_over_instead_of_hanging_playback() {
        struct HungSources;
        #[async_trait]
        impl Provider for HungSources {
            fn id(&self) -> &'static str {
                "allanime"
            }
            fn display_name(&self) -> &'static str {
                "allanime"
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    dub: true,
                    carries_anilist_id: true,
                    subtitles: true,
                }
            }
            async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
                Ok(vec![])
            }
            async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
                std::future::pending().await
            }
            async fn sources(
                &self,
                _s: &str,
                _e: &str,
                _m: TranslationType,
            ) -> Result<Vec<VideoSource>> {
                std::future::pending().await
            }
        }
        let r = Registry::new(vec![
            Arc::new(HungSources) as Arc<dyn Provider>,
            Arc::new(Stub::new("anizone").playable(&["1", "2"])),
        ]);
        let d = db();
        map(&d, 21, &[("allanime", "a1"), ("anizone", "z1")]);
        let got = sources_for(&r, &d, "allanime:a1", "1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got[0].source, "anizone");
        let eps = episodes(&r, &d, "allanime:a1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(eps, ["1", "2"]);
    }

    #[tokio::test]
    async fn the_episode_list_falls_across_when_its_source_is_down() {
        // Without this the show page is a dead end before play is ever pressed.
        let r = reg(vec![
            Stub::new("allanime").failing(),
            Stub::new("anizone")
                .hits(&["Sousou no Frieren"])
                .playable(&["1", "2", "3"]),
        ]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        let got = episodes(&r, &d, "allanime:a1", TranslationType::Sub)
            .await
            .unwrap();
        assert_eq!(got, ["1", "2", "3"]);
    }

    #[tokio::test]
    async fn a_dead_show_page_with_no_alternative_reports_the_real_error() {
        let r = reg(vec![Stub::new("allanime").failing()]);
        let d = db();
        opened(&d, "allanime:a1", "Sousou no Frieren", None);
        let err = episodes(&r, &d, "allanime:a1", TranslationType::Sub)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("down"), "got {err}");
    }

    #[tokio::test]
    async fn a_sub_only_source_hands_dub_requests_to_one_that_has_it() {
        // Opened from a sub-only source, then the user flips to dub.
        struct NoDub;
        #[async_trait]
        impl Provider for NoDub {
            fn id(&self) -> &'static str {
                "anizone"
            }
            fn display_name(&self) -> &'static str {
                "anizone"
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    dub: false,
                    carries_anilist_id: false,
                    subtitles: true,
                }
            }
            async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
                Ok(vec![])
            }
            async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
                Ok(vec![])
            }
            async fn sources(
                &self,
                _s: &str,
                _e: &str,
                _m: TranslationType,
            ) -> Result<Vec<VideoSource>> {
                Ok(vec![])
            }
        }
        let r = Registry::new(vec![
            Arc::new(NoDub) as Arc<dyn Provider>,
            Arc::new(Stub::new("animegg").hits(&["Sousou no Frieren"])),
        ]);
        let d = db();
        opened(&d, "anizone:z1", "Sousou no Frieren", None);
        let eps = episodes(&r, &d, "anizone:z1", TranslationType::Dub)
            .await
            .unwrap();
        assert_eq!(eps, ["1"]);
        let got = sources_for(&r, &d, "anizone:z1", "1", TranslationType::Dub)
            .await
            .unwrap();
        assert_eq!(got[0].source, "animegg");
    }

    // ---- link_matches: one aggregated search links every source ----

    fn hit(id: &str, title: &str, eps: u32, anilist_id: Option<i64>) -> AnimeSummary {
        AnimeSummary {
            provider_id: id.to_string(),
            title: title.to_string(),
            title_english: None,
            cover_url: None,
            available_episodes: eps,
            anilist_id,
        }
    }

    #[test]
    fn link_matches_links_each_sources_version_of_the_show() {
        let d = db();
        let results = vec![
            hit("allanime:a1", "Sousou no Frieren", 28, Some(154587)),
            hit("anizone:z9", "Sousou no Frieren (2026)", 10, None),
            hit("animegg:sousou-no-frieren", "Sousou no Frieren", 25, None),
            hit("anizone:z1", "Sousou no Frieren", 28, None),
        ];
        for r in &results {
            d.cache_anime(&r.provider_id, &r.title, None, None, None)
                .unwrap();
        }
        let opened = link_matches(&d, 154587, &["Sousou no Frieren"], Some(28), &results).unwrap();
        // The carried AniList id is exact, so allanime is what opens...
        assert_eq!(opened.provider_id, "allanime:a1");
        // ...and the other two are linked behind it, each to the right show.
        let mut linked = d.sources_for_anilist(154587).unwrap();
        linked.sort();
        assert_eq!(
            linked,
            [
                ("allanime".to_string(), "allanime:a1".to_string()),
                (
                    "animegg".to_string(),
                    "animegg:sousou-no-frieren".to_string()
                ),
                ("anizone".to_string(), "anizone:z1".to_string()),
            ]
        );
        assert_eq!(
            d.provider_id_for_anilist(154587).unwrap().as_deref(),
            Some("allanime:a1")
        );
    }

    #[test]
    fn link_matches_opens_another_source_when_the_usual_one_is_absent() {
        // allanime contributed nothing to the search (down): the show must
        // still open, from whoever has it.
        let d = db();
        let results = vec![
            hit("anizone:z1", "Sousou no Frieren", 28, None),
            hit("animegg:sousou-no-frieren", "Sousou no Frieren", 25, None),
        ];
        for r in &results {
            d.cache_anime(&r.provider_id, &r.title, None, None, None)
                .unwrap();
        }
        let opened = link_matches(&d, 154587, &["Sousou no Frieren"], Some(28), &results).unwrap();
        assert_eq!(opened.provider_id, "anizone:z1");
        assert_eq!(d.sources_for_anilist(154587).unwrap().len(), 2);
    }

    #[test]
    fn link_matches_does_not_link_a_lookalike_from_a_secondary_source() {
        let d = db();
        let results = vec![
            hit("allanime:a1", "Attack on Titan", 25, Some(16498)),
            hit("anizone:jh", "Attack on Titan: Junior High", 12, None),
        ];
        for r in &results {
            d.cache_anime(&r.provider_id, &r.title, None, None, None)
                .unwrap();
        }
        link_matches(&d, 16498, &["Attack on Titan"], Some(25), &results).unwrap();
        assert_eq!(
            d.sources_for_anilist(16498).unwrap(),
            [("allanime".to_string(), "allanime:a1".to_string())]
        );
        assert!(link_matches(&d, 1, &["Nothing Like It"], None, &results).is_none());
    }
}
