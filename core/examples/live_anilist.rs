//! Manual smoke test against the live AniList public GraphQL API (no auth).
//!
//!   cargo run -p anidoku-core --example live_anilist -- "frieren"
//!
//! Exercises the search + media-by-id parsing against the real endpoint. OAuth,
//! Viewer, MediaListCollection and SaveMediaListEntry need a token and can only
//! be code-reviewed / unit-tested.

use anidoku_core::anilist::AniListClient;

#[tokio::main]
async fn main() {
    let query = std::env::args().nth(1).unwrap_or_else(|| "frieren".into());
    let c = AniListClient::new();

    match c.search_media(&query).await {
        Ok(results) => {
            println!("search '{query}': {} results", results.len());
            for m in results.iter().take(5) {
                println!(
                    "  [{}] {} / {:?} — {:?} eps, format {:?}",
                    m.anilist_id, m.title_romaji.as_deref().unwrap_or("?"),
                    m.title_english, m.episode_count, m.format
                );
            }
            if let Some(first) = results.first() {
                match c.media_by_id(first.anilist_id).await {
                    Ok(Some(m)) => println!(
                        "media_by_id({}): {}",
                        first.anilist_id,
                        m.title_romaji.as_deref().unwrap_or("?")
                    ),
                    Ok(None) => println!("media_by_id({}): none", first.anilist_id),
                    Err(e) => eprintln!("media_by_id failed: {e}"),
                }
            }
        }
        Err(e) => eprintln!("search failed: {e}"),
    }
}
