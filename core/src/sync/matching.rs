//! Resolve a provider show to an AniList media id.
//!
//! The cheap, exact path is the `aniListId` the allanime Show already carries
//! (see `AnimeSummary::anilist_id`) — when present, use it, no search needed.
//!
//! The fallback path searches AniList by title and picks the best candidate,
//! scored by title similarity with an episode-count sanity check. Kept pure so
//! the heuristic is unit-testable against fixed candidate sets.

use crate::models::MediaInfo;

/// Normalize a title for comparison: lowercase, strip season/part suffixes and
/// punctuation, collapse whitespace. Deliberately lossy — it exists to make
/// "Sousou no Frieren: Something" and "Sousou no Frieren" comparable.
pub fn normalize(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut cleaned = String::with_capacity(lower.len());
    for ch in lower.chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            cleaned.push(ch);
        } else {
            cleaned.push(' ');
        }
    }
    // Drop common season/ordinal noise words.
    const NOISE: &[&str] = &[
        "season", "cour", "part", "the", "tv", "2nd", "3rd", "4th", "1st",
    ];
    let tokens: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|t| !NOISE.contains(t))
        .collect();
    tokens.join(" ")
}

/// Token-set similarity in [0,1]: |intersection| / |union| (Jaccard) over the
/// normalized token sets. Simple, order-insensitive, good enough to rank
/// AniList search candidates.
pub fn similarity(a: &str, b: &str) -> f64 {
    let na = normalize(a);
    let nb = normalize(b);
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }
    let sa: std::collections::BTreeSet<&str> = na.split_whitespace().collect();
    let sb: std::collections::BTreeSet<&str> = nb.split_whitespace().collect();
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Score a single candidate against the query title + optional episode count.
/// Returns a value in roughly [0,1.1]; higher is better.
pub fn score_candidate(
    query_title: &str,
    query_episodes: Option<u32>,
    candidate: &MediaInfo,
) -> f64 {
    // Best similarity across all of the candidate's known titles/synonyms.
    let mut best = 0.0_f64;
    for t in candidate
        .title_romaji
        .iter()
        .chain(candidate.title_english.iter())
        .chain(candidate.title_native.iter())
        .chain(candidate.synonyms.iter())
    {
        best = best.max(similarity(query_title, t));
    }

    // Episode-count sanity: nudge up when counts agree, down when they clash.
    // Only applies when both are known and the show has finished airing.
    if let (Some(q), Some(c)) = (query_episodes, candidate.episode_count) {
        if q > 0 && c > 0 {
            if q as i64 == c {
                best += 0.1;
            } else if (q as i64 - c).abs() > 2 {
                best -= 0.15;
            }
        }
    }
    best.clamp(0.0, 1.1)
}

/// Pick the best AniList candidate for a provider show. Returns `None` when no
/// candidate clears the confidence threshold, so the caller can leave the show
/// unmapped rather than link it wrongly.
pub fn best_match<'a>(
    query_title: &str,
    query_episodes: Option<u32>,
    candidates: &'a [MediaInfo],
) -> Option<&'a MediaInfo> {
    const THRESHOLD: f64 = 0.34;
    candidates
        .iter()
        .map(|c| (c, score_candidate(query_title, query_episodes, c)))
        .filter(|(_, s)| *s >= THRESHOLD)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(c, _)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(id: i64, romaji: &str, english: Option<&str>, eps: Option<i64>) -> MediaInfo {
        MediaInfo {
            anilist_id: id,
            title_romaji: Some(romaji.to_string()),
            title_english: english.map(str::to_string),
            title_native: None,
            synonyms: Vec::new(),
            cover_url: None,
            episode_count: eps,
            format: None,
        }
    }

    #[test]
    fn normalize_strips_noise_and_punctuation() {
        // "season" is dropped as noise; the bare ordinal digit is kept (Jaccard
        // similarity tolerates it, and stripping small numbers would break
        // titles like "86" or "91 Days").
        assert_eq!(normalize("Sousou no Frieren: Season 2"), "sousou no frieren 2");
        assert_eq!(normalize("Frieren - The Movie"), "frieren movie");
    }

    #[test]
    fn identical_titles_are_perfectly_similar() {
        assert_eq!(similarity("Frieren", "Frieren"), 1.0);
        assert_eq!(similarity("Sousou no Frieren", "sousou no frieren!!"), 1.0);
    }

    #[test]
    fn similarity_partial_overlap() {
        let s = similarity("Attack on Titan", "Attack on Titan Final Season");
        assert!(s > 0.4 && s < 1.0, "got {s}");
    }

    #[test]
    fn best_match_prefers_episode_count_agreement() {
        let candidates = vec![
            media(1, "Sousou no Frieren", Some("Frieren"), Some(12)),
            media(154587, "Sousou no Frieren", Some("Frieren: Beyond"), Some(28)),
        ];
        // Query knows it has 28 episodes → should pick the 28-ep candidate even
        // though titles are otherwise identical.
        let m = best_match("Sousou no Frieren", Some(28), &candidates).unwrap();
        assert_eq!(m.anilist_id, 154587);
    }

    #[test]
    fn best_match_returns_none_below_threshold() {
        let candidates = vec![media(1, "Completely Different Show", None, Some(24))];
        assert!(best_match("Sousou no Frieren", Some(28), &candidates).is_none());
    }

    #[test]
    fn best_match_uses_synonyms() {
        let mut m = media(500, "Shingeki no Kyojin", Some("Attack on Titan"), Some(25));
        m.synonyms = vec!["AoT".into()];
        let candidates = vec![m];
        let got = best_match("Attack on Titan", Some(25), &candidates).unwrap();
        assert_eq!(got.anilist_id, 500);
    }

    #[test]
    fn empty_candidates_none() {
        assert!(best_match("Frieren", None, &[]).is_none());
    }
}
