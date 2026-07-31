//! Manual smoke test against the live AniList public GraphQL API (no auth).
//!
//!   cargo run -p anidoku-core --example live_anilist -- "frieren"
//!
//! Exercises the search + media-by-id parsing against the real endpoint. OAuth,
//! Viewer, MediaListCollection and SaveMediaListEntry need a token and can only
//! be code-reviewed / unit-tested.

use anidoku_core::anilist::{current_and_next_from_unix, AniListClient};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let query = std::env::args().nth(1).unwrap_or_else(|| "frieren".into());
    let c = AniListClient::new();

    // Home sections (Trending / This Season / Next Season) — public, no auth.
    let ((cur, cy), (nxt, ny)) = current_and_next_from_unix(now());
    match c.home_sections(cur, cy, nxt, ny, 6).await {
        Ok(h) => {
            println!(
                "home: trending={} season={} ({} {cy}) next={} ({} {ny})",
                h.trending.len(),
                h.season.len(),
                cur.as_str(),
                h.next_season.len(),
                nxt.as_str()
            );
            for m in h.trending.iter().take(3) {
                println!(
                    "  trending [{}] {} — status {:?}, next ep {:?} @ {:?}",
                    m.anilist_id,
                    m.title_english
                        .as_deref()
                        .or(m.title_romaji.as_deref())
                        .unwrap_or("?"),
                    m.status,
                    m.next_episode,
                    m.airing_at
                );
            }
            // Batched airing lookup over the trending ids.
            let ids: Vec<i64> = h.trending.iter().map(|m| m.anilist_id).collect();
            match c.airing_for(&ids).await {
                Ok(a) => println!("airing_for({} ids): {} rows", ids.len(), a.len()),
                Err(e) => eprintln!("airing_for failed: {e}"),
            }
        }
        Err(e) => eprintln!("home_sections failed: {e}"),
    }

    match c.search_media(&query).await {
        Ok(results) => {
            println!("search '{query}': {} results", results.len());
            for m in results.iter().take(5) {
                println!(
                    "  [{}] {} / {:?} — {:?} eps, format {:?}",
                    m.anilist_id,
                    m.title_romaji.as_deref().unwrap_or("?"),
                    m.title_english,
                    m.episode_count,
                    m.format
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
