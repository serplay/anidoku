//! Manual end-to-end smoke test against the live allanime API.
//!
//!   cargo run -p anidoku-core --example live_search -- "frieren"
//!
//! Network-dependent; not part of `cargo test`. Exercises the full provider
//! flow: search -> episodes -> source resolution.

use anidoku_core::models::TranslationType;
use anidoku_core::provider::{allanime::AllAnime, Provider};

#[tokio::main]
async fn main() {
    let query = std::env::args().nth(1).unwrap_or_else(|| "frieren".into());
    let p = AllAnime::new();

    let results = match p.search(&query, TranslationType::Sub).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("search failed: {e}");
            std::process::exit(1);
        }
    };
    println!("search '{query}': {} results", results.len());
    let Some(show) = results.first() else { return };
    println!("  -> {} ({})", show.title, show.provider_id);

    let eps = p
        .episodes(&show.provider_id, TranslationType::Sub)
        .await
        .expect("episodes");
    println!(
        "episodes: {} (first={:?}, last={:?})",
        eps.len(),
        eps.first(),
        eps.last()
    );

    let Some(ep) = eps.first() else { return };
    match p.sources(&show.provider_id, ep, TranslationType::Sub).await {
        Ok(sources) => {
            println!("sources for ep {ep}: {}", sources.len());
            for s in &sources {
                println!(
                    "  [{}] quality={} kind={:?} subs={}",
                    s.provider_name,
                    s.quality,
                    s.kind,
                    s.subtitles.len()
                );
            }
        }
        Err(e) => eprintln!("sources failed: {e}"),
    }
}
