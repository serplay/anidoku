//! Pure parsers for AniZone's pages. Kept free of I/O so they are testable
//! against captured pages (see `fixtures/`).
//!
//! AniZone is a Livewire/Alpine app: each list page inlines its first page of
//! rows as `items: JSON.parse('…')` inside an `x-data` attribute, and the
//! player inlines its config as `vidstackPlayer(JSON.parse('…'))`. Both are
//! real JSON once the JS string escaping is undone, so nothing here depends on
//! the page's markup beyond those two anchors.

use crate::models::{AnimeSummary, StreamKind, SubtitleTrack, VideoSource};
use crate::provider::scrape::{between, js_string_literal, unescape_html};
use crate::{Error, Result};
use serde::Deserialize;
use serde_json::Value;

/// `title_list` key for the English title (AniZone's own language ids).
const TITLE_ENGLISH: &str = "1";

const ITEMS_ANCHOR: &str = "items: JSON.parse('";
const ITEMS_EMPTY: &str = "items: []";
const PLAYER_ANCHOR: &str = "vidstackPlayer(JSON.parse('";

#[derive(Deserialize)]
struct AnimeItem {
    slug: String,
    main_title: String,
    #[serde(default)]
    title_list: Value,
    #[serde(default)]
    cover: Option<String>,
    #[serde(default)]
    episode_count: Option<u32>,
    #[serde(default)]
    is_unsafe: bool,
}

#[derive(Deserialize)]
struct EpisodeItem {
    slug: String,
}

/// The inlined first page of a list. `Ok(None)` means the page rendered but
/// has no rows; an `Err` means neither anchor is present, i.e. the layout
/// changed and the scraper needs attention.
fn inline_items(html: &str, stage: &str) -> Result<Option<String>> {
    if let Some(at) = html.find(ITEMS_ANCHOR) {
        return js_string_literal(&html[at + ITEMS_ANCHOR.len()..], '\'')
            .map(Some)
            .ok_or_else(|| Error::Provider(format!("{stage}: unterminated inline items")));
    }
    if html.contains(ITEMS_EMPTY) {
        return Ok(None);
    }
    Err(Error::Provider(format!(
        "{stage}: page layout changed (no inline items)"
    )))
}

pub fn parse_search(html: &str) -> Result<Vec<AnimeSummary>> {
    let Some(json) = inline_items(html, "search")? else {
        return Ok(Vec::new());
    };
    let items: Vec<AnimeItem> = serde_json::from_str(&json)
        .map_err(|e| Error::Provider(format!("search: invalid items json: {e}")))?;
    Ok(items
        .into_iter()
        .filter(|i| !i.is_unsafe && !i.slug.is_empty())
        .map(|i| {
            let english = i
                .title_list
                .get(TITLE_ENGLISH)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty() && *t != i.main_title)
                .map(str::to_string);
            AnimeSummary {
                provider_id: i.slug,
                title: i.main_title,
                title_english: english,
                cover_url: i.cover,
                available_episodes: i.episode_count.unwrap_or(0),
                anilist_id: None,
            }
        })
        .collect())
}

/// One page of a show's episode list plus the cursor for the next, if any.
#[derive(Debug, PartialEq)]
pub struct EpisodePage {
    pub episodes: Vec<String>,
    pub next_cursor: Option<String>,
}

pub fn parse_episode_page(html: &str) -> Result<EpisodePage> {
    let episodes = match inline_items(html, "episodes")? {
        Some(json) => episode_slugs(&json, "episodes")?,
        None => Vec::new(),
    };
    // `nextCursor: '…'` sits right after the items; `nextCursor: null` (or
    // `hasMore: false`) means this page is the whole list.
    let has_more = html.contains("hasMore: true");
    let next_cursor = between(html, "nextCursor: '", "'")
        .filter(|c| has_more && !c.is_empty())
        .map(str::to_string);
    Ok(EpisodePage {
        episodes,
        next_cursor,
    })
}

fn episode_slugs(json: &str, stage: &str) -> Result<Vec<String>> {
    let items: Vec<EpisodeItem> = serde_json::from_str(json)
        .map_err(|e| Error::Provider(format!("{stage}: invalid items json: {e}")))?;
    Ok(items
        .into_iter()
        .map(|i| i.slug)
        .filter(|s| !s.is_empty())
        .collect())
}

/// What a Livewire `update` call needs from the page it continues: the CSRF
/// token and the component's signed state snapshot.
#[derive(Debug, PartialEq)]
pub struct LivewireContext {
    pub csrf: String,
    pub snapshot: String,
}

/// Pull the Livewire context for the episode-list component out of a show
/// page. A page carries one snapshot per component (navbar, mobile navbar,
/// the list); the list's is the one whose state names this show's slug.
pub fn parse_livewire_context(html: &str, show_slug: &str) -> Option<LivewireContext> {
    let csrf = between(html, "name=\"csrf-token\" content=\"", "\"")?.to_string();
    let needle = format!("\"slug\":\"{show_slug}\"");
    let snapshot = html
        .split("wire:snapshot=\"")
        .skip(1)
        .filter_map(|chunk| chunk.split('"').next())
        .map(unescape_html)
        .find(|snap| snap.contains(&needle))?;
    Some(LivewireContext { csrf, snapshot })
}

/// One `loadPage` response: the rows it dispatched, the next cursor, and the
/// component's new snapshot (each call must send the previous call's).
#[derive(Debug, PartialEq)]
pub struct LivewirePage {
    pub page: EpisodePage,
    pub snapshot: String,
}

pub fn parse_livewire_page(body: &str) -> Result<LivewirePage> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Provider(format!("episodes: invalid livewire json: {e}")))?;
    let component = v
        .pointer("/components/0")
        .ok_or_else(|| Error::Provider("episodes: livewire response has no component".into()))?;
    let snapshot = component
        .get("snapshot")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Provider("episodes: livewire response has no snapshot".into()))?
        .to_string();
    let params = component
        .pointer("/effects/dispatches")
        .and_then(Value::as_array)
        .and_then(|d| {
            d.iter()
                .find(|e| e.get("name").and_then(Value::as_str) == Some("items-loaded"))
        })
        .and_then(|e| e.get("params"))
        .ok_or_else(|| Error::Provider("episodes: livewire response has no items-loaded".into()))?;
    let episodes = params
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.get("slug").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let has_more = params
        .get("hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next_cursor = params
        .get("nextCursor")
        .and_then(Value::as_str)
        .filter(|c| has_more && !c.is_empty())
        .map(str::to_string);
    Ok(LivewirePage {
        page: EpisodePage {
            episodes,
            next_cursor,
        },
        snapshot,
    })
}

#[derive(Deserialize)]
struct Player {
    src: String,
    #[serde(default)]
    subtitles: Vec<PlayerSubtitle>,
}

#[derive(Deserialize)]
struct PlayerSubtitle {
    #[serde(default)]
    title: String,
    #[serde(default)]
    language: String,
    file: String,
    #[serde(default)]
    default: bool,
}

/// The episode page's player config as one playable source. AniZone serves a
/// single adaptive HLS master (every resolution and audio language inside it)
/// with soft subtitles, so there is exactly one source per episode.
pub fn parse_player(html: &str, referer: &str) -> Result<Vec<VideoSource>> {
    let at = html
        .find(PLAYER_ANCHOR)
        .ok_or_else(|| Error::Provider("sources: page layout changed (no player config)".into()))?;
    let json = js_string_literal(&html[at + PLAYER_ANCHOR.len()..], '\'')
        .ok_or_else(|| Error::Provider("sources: unterminated player config".into()))?;
    let player: Player = serde_json::from_str(&json)
        .map_err(|e| Error::Provider(format!("sources: invalid player json: {e}")))?;
    if !player.src.starts_with("http") {
        // An episode that is listed but has no video uploaded yet.
        return Ok(Vec::new());
    }

    let mut subtitles: Vec<SubtitleTrack> = player
        .subtitles
        .into_iter()
        .filter(|s| s.file.starts_with("http"))
        .map(|s| SubtitleTrack {
            label: if s.title.is_empty() {
                s.language.clone()
            } else {
                s.title
            },
            lang: if s.language.is_empty() {
                "und".to_string()
            } else {
                s.language
            },
            url: s.file,
            default: s.default,
        })
        .collect();
    // The stream has no burned-in subtitles, so exactly one track must be on
    // by default: the one the site marks, else the first English one.
    if subtitles.iter().filter(|s| s.default).count() != 1 {
        let pick = subtitles
            .iter()
            .position(|s| s.default)
            .or_else(|| subtitles.iter().position(|s| s.lang.starts_with("en")));
        for (i, s) in subtitles.iter_mut().enumerate() {
            s.default = Some(i) == pick;
        }
    }

    let kind = if player.src.contains(".m3u8") {
        StreamKind::Hls
    } else {
        StreamKind::Mp4
    };
    Ok(vec![VideoSource {
        source: String::new(),
        provider_name: "AniZone".to_string(),
        quality: "auto".to_string(),
        url: player.src,
        kind,
        referer: Some(referer.to_string()),
        subtitles,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("fixtures/search.html");
    const SEARCH_EMPTY: &str = include_str!("fixtures/search_empty.html");
    const ANIME: &str = include_str!("fixtures/anime.html");
    const EPISODE: &str = include_str!("fixtures/episode.html");
    const LIVEWIRE: &str = include_str!("fixtures/livewire_page.json");

    #[test]
    fn search_reads_the_inlined_rows() {
        let hits = parse_search(SEARCH).unwrap();
        assert_eq!(hits.len(), 2);
        let first = &hits[0];
        assert_eq!(first.provider_id, "mdkytdqp");
        assert_eq!(first.title, "Sousou no Frieren");
        // The English title is double-escaped JSON inside a JS string; the
        // site writes the apostrophe as a backtick.
        assert_eq!(
            first.title_english.as_deref(),
            Some("Frieren: Beyond Journey`s End")
        );
        assert_eq!(first.available_episodes, 28);
        assert!(first
            .cover_url
            .as_deref()
            .unwrap()
            .starts_with("https://anizone.to/images/anime/"));
        assert_eq!(first.anilist_id, None);
    }

    #[test]
    fn an_empty_search_is_empty_not_an_error() {
        assert!(parse_search(SEARCH_EMPTY).unwrap().is_empty());
    }

    #[test]
    fn a_page_without_either_anchor_is_a_layout_change() {
        // Distinguishing "no results" from "we can no longer read this page"
        // is what lets the health check tell an outage from an empty search.
        let err = parse_search("<html><body>Just a moment...</body></html>").unwrap_err();
        assert!(err.to_string().contains("layout changed"), "{err}");
        let err = parse_episode_page("<html></html>").unwrap_err();
        assert!(err.to_string().contains("layout changed"), "{err}");
    }

    #[test]
    fn adult_rows_are_dropped() {
        let html = r#"items: JSON.parse('[{"slug":"a","main_title":"Safe"},{"slug":"b","main_title":"Not","is_unsafe":true}]')"#;
        let hits = parse_search(html).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].provider_id, "a");
        // Optional fields default rather than failing the row.
        assert_eq!(hits[0].available_episodes, 0);
        assert_eq!(hits[0].title_english, None);
    }

    #[test]
    fn the_show_page_carries_the_first_page_and_a_cursor() {
        let page = parse_episode_page(ANIME).unwrap();
        assert_eq!(page.episodes.len(), 24);
        assert_eq!(page.episodes.first().map(String::as_str), Some("1"));
        assert_eq!(page.episodes.last().map(String::as_str), Some("24"));
        assert!(page.next_cursor.is_some(), "28 episodes need a second page");
    }

    #[test]
    fn a_short_show_has_no_cursor() {
        let html = r#"items: JSON.parse('[{"slug":"1"}]'),
        nextCursor: null,
        hasMore: false,"#;
        let page = parse_episode_page(html).unwrap();
        assert_eq!(page.episodes, ["1"]);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn livewire_context_picks_the_episode_list_component() {
        let ctx = parse_livewire_context(ANIME, "mdkytdqp").unwrap();
        assert!(!ctx.csrf.is_empty());
        // Unescaped back into JSON, and the right one of the page's snapshots.
        let snap: Value = serde_json::from_str(&ctx.snapshot).unwrap();
        assert_eq!(snap["data"]["slug"], "mdkytdqp");
        assert_eq!(snap["memo"]["name"], "pages.anime-detail");
        assert_eq!(parse_livewire_context(ANIME, "someothershow"), None);
    }

    #[test]
    fn a_livewire_page_yields_rows_cursor_and_the_next_snapshot() {
        let got = parse_livewire_page(LIVEWIRE).unwrap();
        assert_eq!(got.page.episodes, ["25", "26", "27", "28"]);
        assert_eq!(got.page.next_cursor, None, "this was the last page");
        assert!(got.snapshot.contains("pages.anime-detail"));
    }

    #[test]
    fn a_livewire_error_body_is_an_error_not_an_empty_page() {
        assert!(parse_livewire_page("<html>419 Page Expired</html>").is_err());
        assert!(parse_livewire_page(r#"{"components":[]}"#).is_err());
        assert!(parse_livewire_page(r#"{"components":[{"snapshot":"{}","effects":{}}]}"#).is_err());
    }

    #[test]
    fn the_player_config_becomes_one_hls_source_with_soft_subs() {
        let sources = parse_player(EPISODE, "https://anizone.to/").unwrap();
        assert_eq!(sources.len(), 1);
        let s = &sources[0];
        assert_eq!(s.kind, StreamKind::Hls);
        assert!(s.url.ends_with("/master.m3u8"), "{}", s.url);
        assert_eq!(s.referer.as_deref(), Some("https://anizone.to/"));
        assert!(s.subtitles.len() > 5);
        // Exactly one default, and it is the English track the site marks.
        let defaults: Vec<_> = s.subtitles.iter().filter(|t| t.default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].lang, "en");
        assert!(defaults[0].url.ends_with(".ass"));
    }

    #[test]
    fn english_is_defaulted_when_the_site_marks_nothing() {
        let html = r#"vidstackPlayer(JSON.parse('{"src":"https:\\\/\\\/c.x\\\/m.m3u8","subtitles":[{"title":"German","language":"de","file":"https:\\\/\\\/c.x\\\/de.ass"},{"title":"English","language":"en-US","file":"https:\\\/\\\/c.x\\\/en.ass"}]}'))"#;
        let sources = parse_player(html, "r").unwrap();
        let subs = &sources[0].subtitles;
        assert!(!subs[0].default);
        assert!(subs[1].default);
    }

    #[test]
    fn an_episode_with_no_video_yet_has_no_sources() {
        let html = r#"vidstackPlayer(JSON.parse('{"src":"","subtitles":[]}'))"#;
        assert!(parse_player(html, "r").unwrap().is_empty());
        let err = parse_player("<html>no player</html>", "r").unwrap_err();
        assert!(err.to_string().contains("layout changed"), "{err}");
    }
}
