//! Live smoke test against allanime — hits the network, so it is #[ignore]d.
//! Run explicitly:
//!   cargo test -p anidoku-core --test allanime_live -- --ignored --nocapture
//!
//! Every failure message is shaped `<stage>: <detail>` because
//! .github/workflows/provider-health.yml greps for exactly that to fill the
//! outage issue's Reason field. A bare .expect("sources") yields a panic whose
//! first line is only a file:line, which is what made issue #6 unreadable.
use anidoku_core::models::TranslationType;
use anidoku_core::provider::allanime::AllAnime;
use anidoku_core::provider::Provider;

#[tokio::test]
#[ignore]
async fn live_sources_one_piece() {
    let p = AllAnime::new();
    let results = p
        .search("one piece", TranslationType::Sub)
        .await
        .unwrap_or_else(|e| panic!("search: {e}"));
    assert!(!results.is_empty(), "search: returned nothing");
    let show = &results[0];
    println!("show: {} ({})", show.title, show.provider_id);

    let eps = p
        .episodes(&show.provider_id, TranslationType::Sub)
        .await
        .unwrap_or_else(|e| panic!("episodes: {e}"));
    assert!(!eps.is_empty(), "episodes: none listed");

    let sources = p
        .sources(&show.provider_id, "1", TranslationType::Sub)
        .await
        .unwrap_or_else(|e| panic!("sources: {e}"));
    println!("got {} sources", sources.len());
    for s in sources.iter().take(6) {
        let url = &s.url[..s.url.len().min(70)];
        println!(
            "  [{}] {} {:?} -> {}",
            s.provider_name, s.quality, s.kind, url
        );
    }
    assert!(
        !sources.is_empty(),
        "sources: empty — provider still broken"
    );
}
