//! Live smoke tests for the non-allanime sources — they hit the network, so
//! they are #[ignore]d. Run explicitly:
//!   cargo test -p anidoku-core --test sources_live -- --ignored --nocapture
//!   cargo test -p anidoku-core --test sources_live anizone -- --ignored
//!
//! Each test walks the whole chain a viewer does (search → episodes → sources
//! → first bytes of the media through the same proxy client the player uses),
//! because a source whose links resolve but won't actually fetch is as broken
//! as one that returns nothing.
//!
//! Failure messages are shaped `<stage>: <detail>`, which is what
//! .github/workflows/provider-health.yml greps for.
use anidoku_core::models::{StreamKind, TranslationType, VideoSource};
use anidoku_core::provider::animegg::AnimeGg;
use anidoku_core::provider::anizone::AniZone;
use anidoku_core::provider::Provider;
use anidoku_core::proxy::ProxyClient;

/// A long-finished show every general catalogue carries.
const QUERY: &str = "frieren";

async fn walk(p: &dyn Provider, mode: TranslationType) -> Vec<VideoSource> {
    let results = p
        .search(QUERY, mode)
        .await
        .unwrap_or_else(|e| panic!("search: {e}"));
    assert!(!results.is_empty(), "search: returned nothing");
    let show = &results[0];
    println!("[{}] show: {} ({})", p.id(), show.title, show.provider_id);

    let eps = p
        .episodes(&show.provider_id, mode)
        .await
        .unwrap_or_else(|e| panic!("episodes: {e}"));
    assert!(!eps.is_empty(), "episodes: none listed");
    println!("[{}] {} episodes, first {}", p.id(), eps.len(), eps[0]);

    let sources = p
        .sources(&show.provider_id, &eps[0], mode)
        .await
        .unwrap_or_else(|e| panic!("sources: {e}"));
    assert!(!sources.is_empty(), "sources: empty");
    for s in &sources {
        println!(
            "[{}]   [{}] {} {:?} -> {}",
            p.id(),
            s.provider_name,
            s.quality,
            s.kind,
            &s.url[..s.url.len().min(80)]
        );
    }
    sources
}

/// Fetch the head of a source the way the media server does and check it is
/// what it claims to be.
async fn assert_playable(s: &VideoSource) {
    let proxy = ProxyClient::new();
    match s.kind {
        StreamKind::Hls => {
            let got = proxy
                .fetch(&s.url, s.referer.as_deref())
                .await
                .unwrap_or_else(|e| panic!("media: playlist fetch failed: {e}"));
            let body = String::from_utf8_lossy(&got.bytes);
            assert!(
                body.trim_start().starts_with("#EXTM3U"),
                "media: not an HLS playlist: {}",
                &body[..body.len().min(80)]
            );
        }
        StreamKind::Mp4 => {
            let resp = proxy
                .get_ranged(&s.url, s.referer.as_deref(), Some("bytes=0-1023"))
                .await
                .unwrap_or_else(|e| panic!("media: fetch failed: {e}"));
            let status = resp.status();
            assert!(status.is_success(), "media: http {status}");
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            assert!(
                ct.starts_with("video/") || ct == "application/octet-stream",
                "media: unexpected content type {ct:?}"
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn live_anizone() {
    let sources = walk(&AniZone::new(), TranslationType::Sub).await;
    assert_playable(&sources[0]).await;
    let subs = &sources[0].subtitles;
    assert!(!subs.is_empty(), "sources: no subtitle tracks");
    assert!(
        subs.iter().any(|t| t.default),
        "sources: no default subtitle track"
    );
    // The subtitle the player would turn on must convert to real cues.
    let track = subs.iter().find(|t| t.default).unwrap();
    let got = ProxyClient::new()
        .fetch(&track.url, sources[0].referer.as_deref())
        .await
        .unwrap_or_else(|e| panic!("media: subtitle fetch failed: {e}"));
    let ext = anidoku_core::subs::url_extension(&track.url).unwrap_or_default();
    let vtt = anidoku_core::subs::to_vtt(&String::from_utf8_lossy(&got.bytes), &ext)
        .unwrap_or_else(|e| panic!("media: subtitle convert failed: {e}"));
    let cues = vtt.matches(" --> ").count();
    println!("[anizone] default subtitle {} -> {cues} cues", track.label);
    assert!(cues > 50, "media: subtitle converted to only {cues} cues");
}

#[tokio::test]
#[ignore]
async fn live_animegg() {
    let sources = walk(&AnimeGg::new(), TranslationType::Sub).await;
    assert_playable(&sources[0]).await;
}

#[tokio::test]
#[ignore]
async fn live_animegg_dub() {
    let sources = walk(&AnimeGg::new(), TranslationType::Dub).await;
    assert_playable(&sources[0]).await;
}

/// The scenario the extra sources exist for, against the real sites: a show
/// that was only ever opened from allanime, with allanime dead. Nothing links
/// it to another source, so this exercises real title discovery — the part a
/// stubbed test cannot vouch for.
#[tokio::test]
#[ignore]
async fn live_failover_when_allanime_is_down() {
    use anidoku_core::db::Database;
    use anidoku_core::models::AnimeSummary;
    use anidoku_core::provider::{aggregate, Capabilities, Registry};
    use anidoku_core::{Error, Result};
    use std::sync::Arc;

    struct DeadAllAnime;
    #[async_trait::async_trait]
    impl Provider for DeadAllAnime {
        fn id(&self) -> &'static str {
            "allanime"
        }
        fn display_name(&self) -> &'static str {
            "AllAnime"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                dub: true,
                carries_anilist_id: true,
                subtitles: true,
            }
        }
        async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
            Err(Error::Provider("search: down".into()))
        }
        async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
            Err(Error::Provider("episodes: down".into()))
        }
        async fn sources(
            &self,
            _s: &str,
            _e: &str,
            _m: TranslationType,
        ) -> Result<Vec<VideoSource>> {
            Err(Error::Provider(
                "PROVIDER_ROTATED: sources: bootstrap rejected".into(),
            ))
        }
    }

    let registry = Registry::new(vec![
        Arc::new(DeadAllAnime) as Arc<dyn Provider>,
        Arc::new(AniZone::new()),
        Arc::new(AnimeGg::new()),
    ]);
    let db = Database::open_in_memory().unwrap();
    // What a pre-outage search would have left behind: allanime's row only.
    db.cache_anime(
        "allanime:ReooPAxPMsHM4KPMY",
        "Sousou no Frieren",
        Some("Frieren: Beyond Journey's End"),
        None,
        Some(28),
    )
    .unwrap();
    db.link_provider_anilist("allanime:ReooPAxPMsHM4KPMY", 154587)
        .unwrap();

    let eps = aggregate::episodes(
        &registry,
        &db,
        "allanime:ReooPAxPMsHM4KPMY",
        TranslationType::Sub,
    )
    .await
    .unwrap_or_else(|e| panic!("episodes: failover found no episode list: {e}"));
    println!("[failover] {} episodes", eps.len());
    assert!(eps.len() >= 20, "episodes: only {} via failover", eps.len());

    let sources = aggregate::sources_for(
        &registry,
        &db,
        "allanime:ReooPAxPMsHM4KPMY",
        "1",
        TranslationType::Sub,
    )
    .await
    .unwrap_or_else(|e| panic!("sources: failover found nothing: {e}"));
    let mut from: Vec<&str> = sources.iter().map(|s| s.source.as_str()).collect();
    from.dedup();
    println!("[failover] {} sources from {from:?}", sources.len());
    assert!(
        from.contains(&"anizone") && from.contains(&"animegg"),
        "sources: expected both fallbacks, got {from:?}"
    );
    assert_playable(&sources[0]).await;

    // Both were linked to the show, each to the first season — not the sequel
    // or the mini-anime that the same searches also return.
    let linked = db.sources_for_anilist(154587).unwrap();
    println!("[failover] linked: {linked:?}");
    assert!(linked.contains(&("anizone".to_string(), "anizone:mdkytdqp".to_string())));
    assert!(linked.contains(&(
        "animegg".to_string(),
        "animegg:sousou-no-frieren".to_string()
    )));
}
