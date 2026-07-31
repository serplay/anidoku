//! Live smoke test against allanime — hits the network, so it is #[ignore]d.
//! Run explicitly:
//!   cargo test -p anidoku-core --test allanime_live -- --ignored --nocapture
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
        .expect("search");
    assert!(!results.is_empty(), "search returned nothing");
    let show = &results[0];
    println!("show: {} ({})", show.title, show.provider_id);

    let eps = p
        .episodes(&show.provider_id, TranslationType::Sub)
        .await
        .expect("episodes");
    assert!(!eps.is_empty(), "no episodes");

    let sources = p
        .sources(&show.provider_id, "1", TranslationType::Sub)
        .await
        .expect("sources");
    println!("got {} sources", sources.len());
    for s in sources.iter().take(6) {
        let url = &s.url[..s.url.len().min(70)];
        println!(
            "  [{}] {} {:?} -> {}",
            s.provider_name, s.quality, s.kind, url
        );
    }
    assert!(!sources.is_empty(), "sources empty — provider still broken");
}
