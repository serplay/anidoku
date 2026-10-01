//! Pure parsers for AnimeGG's pages. Kept free of I/O so they are testable
//! against captured pages (see `fixtures/`).
//!
//! The site is classic server-rendered HTML, so these anchor on the handful of
//! class names and attributes that carry meaning (`class="mse"` for a search
//! hit, `ul.newmanga` for the episode list, `data-version` for a video tab)
//! and ignore everything else.

use crate::models::{AnimeSummary, StreamKind, VideoSource};
use crate::provider::scrape::{attr, between, episode_order, unescape_html};
use crate::{Error, Result};

/// Present on every search page, with or without results — its absence means
/// we were served something else (a challenge page, a redesign).
const SEARCH_MARKER: &str = "class=\"moose page\"";
const SERIES_HREF: &str = "<a href=\"/series/";

pub fn parse_search(html: &str) -> Result<Vec<AnimeSummary>> {
    let mut out = Vec::new();
    for block in html.split(SERIES_HREF).skip(1) {
        let Some(tag_end) = block.find('>') else {
            continue;
        };
        // Only result cards: the nav and footer link to /series/ too.
        if !block[..tag_end].contains("class=\"mse\"") {
            continue;
        }
        let Some(slug) = block.split('"').next().filter(|s| !s.is_empty()) else {
            continue;
        };
        let card = block.split("</a>").next().unwrap_or(block);
        let Some(title) = between(card, "<h2>", "</h2>").map(clean) else {
            continue;
        };
        let episodes = between(card, "Episodes:", "<")
            .and_then(|n| n.trim().parse::<u32>().ok())
            .unwrap_or(0);
        // "Alt Titles : 葬送のフリーレン; Frieren: Beyond Journey's End" — take
        // the first Latin-script one as the English title.
        let english = between(card, "Alt Titles", "<")
            .map(|alts| alts.trim_start_matches([' ', ':']).to_string())
            .and_then(|alts| {
                alts.split(';')
                    .map(clean)
                    .find(|t| is_latin(t) && *t != title)
            });
        let cover = card
            .find("<img")
            .and_then(|i| attr(&card[i..], "src"))
            .filter(|src| src.starts_with("http"))
            .map(str::to_string);
        out.push(AnimeSummary {
            provider_id: slug.to_string(),
            title,
            title_english: english,
            cover_url: cover,
            available_episodes: episodes,
            anilist_id: None,
        });
    }
    if out.is_empty() && !html.contains(SEARCH_MARKER) {
        return Err(Error::Provider(
            "search: page layout changed (no results container)".into(),
        ));
    }
    Ok(out)
}

fn clean(s: &str) -> String {
    unescape_html(s.trim())
}

/// Mostly ASCII letters, i.e. not a Japanese/Chinese/Korean native title.
fn is_latin(s: &str) -> bool {
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    letters > 0 && s.chars().filter(char::is_ascii_alphabetic).count() * 2 > letters
}

/// One row of a show's episode list.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeEntry {
    /// Episode label as the site numbers it ("1", "12.5").
    pub number: String,
    /// Site-relative URL of the episode page ("/one-piece-episode-1").
    pub path: String,
    pub sub: bool,
    pub dub: bool,
}

/// The show page's episode list, in ascending episode order (the site lists
/// newest first).
pub fn parse_series(html: &str) -> Result<Vec<EpisodeEntry>> {
    let list = between(html, "<ul class=\"newmanga\">", "</ul>")
        .ok_or_else(|| Error::Provider("episodes: page layout changed (no episode list)".into()))?;
    let mut out: Vec<EpisodeEntry> = Vec::new();
    for row in list.split("<li>").skip(1) {
        let Some(path) = between(row, "<a href=\"", "\"") else {
            continue;
        };
        // "/<show>-episode-<n>": the label is whatever follows the last marker.
        let Some((_, number)) = path.rsplit_once("-episode-") else {
            continue;
        };
        if number.is_empty() || out.iter().any(|e| e.number == number) {
            continue;
        }
        out.push(EpisodeEntry {
            number: number.to_string(),
            path: path.to_string(),
            sub: row.contains("btn-subbed"),
            dub: row.contains("btn-dubbed"),
        });
    }
    out.sort_by(|a, b| episode_order(&a.number, &b.number));
    Ok(out)
}

/// One video tab on an episode page: an embed id for one mirror in one
/// version ("subbed" / "dubbed" / "raw").
#[derive(Debug, Clone, PartialEq)]
pub struct VideoTab {
    pub embed_id: String,
    pub mirror: String,
    pub version: String,
}

pub fn parse_episode_tabs(html: &str) -> Result<Vec<VideoTab>> {
    let tabs = between(html, "<ul id=\"videos\"", "</ul>")
        .ok_or_else(|| Error::Provider("sources: page layout changed (no video tabs)".into()))?;
    Ok(tabs
        .split("<a ")
        .skip(1)
        .filter_map(|chunk| {
            let tag = chunk.split('>').next()?;
            let embed_id = attr(tag, "data-id")?;
            if embed_id.is_empty() || !embed_id.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some(VideoTab {
                embed_id: embed_id.to_string(),
                mirror: attr(tag, "data-mirror").unwrap_or("AnimeGG").to_string(),
                version: attr(tag, "data-version")?.to_string(),
            })
        })
        .collect())
}

/// The embed player's `videoSources = [{file: "…", label: "720p", …}, …]` as
/// playable sources. `base_url` resolves the site-relative `file` paths;
/// `referer` is what the media host checks.
pub fn parse_embed(html: &str, base_url: &str, referer: &str, mirror: &str) -> Vec<VideoSource> {
    let Some(array) = between(html, "videoSources = [", "];") else {
        return Vec::new();
    };
    array
        .split('{')
        .skip(1)
        .filter_map(|object| {
            let file = js_field(object, "file")?;
            if file.is_empty() {
                return None;
            }
            let url = if file.starts_with("http") {
                file.to_string()
            } else if let Some(rest) = file.strip_prefix("//") {
                format!("https://{rest}")
            } else {
                format!("{base_url}/{}", file.trim_start_matches('/'))
            };
            let quality = js_field(object, "label")
                .map(|l| l.trim_end_matches(['p', 'P']).to_string())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "auto".to_string());
            let kind = if url.contains(".m3u8") {
                StreamKind::Hls
            } else {
                StreamKind::Mp4
            };
            Some(VideoSource {
                source: String::new(),
                provider_name: mirror.to_string(),
                quality,
                url,
                kind,
                referer: Some(referer.to_string()),
                subtitles: Vec::new(),
            })
        })
        .collect()
}

/// Value of `name: "…"` in a JS object literal body (unquoted keys).
fn js_field<'a>(object: &'a str, name: &str) -> Option<&'a str> {
    let mut search = object;
    loop {
        let at = search.find(name)?;
        let after = search[at + name.len()..].trim_start();
        let whole = at == 0
            || !search[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if whole {
            if let Some(value) = after.strip_prefix(':') {
                let value = value.trim_start();
                let quote = value.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let body = &value[1..];
                    return body.find(quote).map(|end| &body[..end]);
                }
            }
        }
        search = &search[at + name.len()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("fixtures/search.html");
    const SEARCH_EMPTY: &str = include_str!("fixtures/search_empty.html");
    const SERIES: &str = include_str!("fixtures/series.html");
    const EPISODE: &str = include_str!("fixtures/episode.html");
    const EMBED: &str = include_str!("fixtures/embed.html");

    #[test]
    fn search_reads_the_result_cards() {
        let hits = parse_search(SEARCH).unwrap();
        assert_eq!(hits.len(), 3);
        let first = &hits[0];
        assert_eq!(first.provider_id, "sousou-no-frieren");
        assert_eq!(first.title, "Sousou no Frieren");
        // The native-script alt title is skipped in favour of the Latin one.
        assert_eq!(
            first.title_english.as_deref(),
            Some("Frieren: Beyond Journey's End")
        );
        assert_eq!(first.available_episodes, 25);
        assert!(first.cover_url.as_deref().unwrap().starts_with("https://"));
        // A card with no alt titles still parses.
        assert_eq!(hits[2].provider_id, "frieren-beyond-journeys-end-season-2");
        assert_eq!(hits[2].title_english, None);
    }

    #[test]
    fn an_empty_search_is_empty_not_an_error() {
        assert!(parse_search(SEARCH_EMPTY).unwrap().is_empty());
    }

    #[test]
    fn a_page_that_is_not_the_search_page_is_a_layout_change() {
        let err = parse_search("<html><body>Just a moment...</body></html>").unwrap_err();
        assert!(err.to_string().contains("layout changed"), "{err}");
    }

    #[test]
    fn series_links_outside_result_cards_are_ignored() {
        let html = r#"<a href="/series/nav-link">Nav</a><div class="moose page"></div>"#;
        assert!(parse_search(html).unwrap().is_empty());
    }

    #[test]
    fn the_episode_list_is_ascending_and_knows_sub_from_dub() {
        let eps = parse_series(SERIES).unwrap();
        // The recorded show really is missing 13-15: a catalogue gap, which is
        // why episodes are listed rather than assumed to be 1..=N.
        assert_eq!(eps.len(), 25);
        assert_eq!(eps[0].number, "1");
        assert_eq!(eps[0].path, "/sousou-no-frieren-episode-1");
        assert_eq!(eps[24].number, "28");
        assert!(!eps.iter().any(|e| e.number == "14"));
        // Episode 21 is listed but has no video in either version.
        let subbed: Vec<_> = eps.iter().filter(|e| e.sub).collect();
        assert_eq!(subbed.len(), 24);
        assert!(!eps.iter().any(|e| e.number == "21" && (e.sub || e.dub)));
        // Only the first nine are dubbed.
        let dubbed: Vec<_> = eps.iter().filter(|e| e.dub).map(|e| &e.number).collect();
        assert_eq!(dubbed, ["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
    }

    #[test]
    fn episode_rows_sort_numerically_and_dedupe() {
        let html = r#"<ul class="newmanga">
            <li><div><a href="/x-episode-10" class="anm_det_pop">x</a><span class="btn-xs btn-subbed">S</span></div></li>
            <li><div><a href="/x-episode-9.5" class="anm_det_pop">x</a><span class="btn-xs btn-dubbed">D</span></div></li>
            <li><div><a href="/x-episode-2" class="anm_det_pop">x</a></div></li>
            <li><div><a href="/x-episode-2" class="anm_det_pop">dup</a></div></li>
            <li><div><a href="/not-an-ep" class="anm_det_pop">x</a></div></li>
        </ul>"#;
        let eps = parse_series(html).unwrap();
        let numbers: Vec<_> = eps.iter().map(|e| e.number.as_str()).collect();
        assert_eq!(numbers, ["2", "9.5", "10"]);
        assert!(eps[1].dub && !eps[1].sub);
        assert!(parse_series("<html>nothing</html>").is_err());
    }

    #[test]
    fn episode_tabs_carry_one_embed_per_version() {
        let tabs = parse_episode_tabs(EPISODE).unwrap();
        assert_eq!(
            tabs,
            [
                VideoTab {
                    embed_id: "131519".into(),
                    mirror: "Animegg".into(),
                    version: "subbed".into(),
                },
                VideoTab {
                    embed_id: "131769".into(),
                    mirror: "Animegg".into(),
                    version: "dubbed".into(),
                },
            ]
        );
        assert!(parse_episode_tabs("<html>nope</html>").is_err());
    }

    #[test]
    fn the_embed_lists_every_rendition_as_a_direct_mp4() {
        let sources = parse_embed(
            EMBED,
            "https://www.animegg.org",
            "https://www.animegg.org/",
            "Animegg",
        );
        let qualities: Vec<_> = sources.iter().map(|s| s.quality.as_str()).collect();
        assert_eq!(qualities, ["360", "480", "720", "1080"]);
        for s in &sources {
            assert_eq!(s.kind, StreamKind::Mp4);
            assert!(
                s.url.starts_with("https://www.animegg.org/play/"),
                "{}",
                s.url
            );
            assert_eq!(s.referer.as_deref(), Some("https://www.animegg.org/"));
            // Direct media, so it ranks as playable rather than as an embed.
            assert_eq!(crate::provider::playability_rank(s), 0);
        }
    }

    #[test]
    fn an_embed_without_sources_is_empty() {
        assert!(parse_embed("<html>removed</html>", "b", "r", "m").is_empty());
        assert!(parse_embed("var videoSources = [];", "b", "r", "m").is_empty());
        // `profile:` must not be mistaken for `file:`.
        let html = r#"videoSources = [{profile: "x", label: "720p"}];"#;
        assert!(parse_embed(html, "b", "r", "m").is_empty());
    }
}
