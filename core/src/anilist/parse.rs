//! Pure JSON shredding for AniList GraphQL responses. No network I/O, so every
//! function here is unit-tested against captured fixtures.

use crate::models::{
    AiringInfo, CatalogMedia, CatalogPage, HomeMedia, HomeSections, MediaInfo, MediaListStatus,
    MediaOverview, MediaTag, RemoteListEntry, Viewer,
};
use crate::{Error, Result};
use serde_json::Value;

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn parse_viewer(v: &Value) -> Result<Viewer> {
    let viewer = v
        .pointer("/data/Viewer")
        .ok_or_else(|| Error::AniList("viewer: missing data.Viewer".into()))?;
    let id = viewer
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::AniList("viewer: missing id".into()))?;
    let name = viewer
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let avatar_url = viewer
        .get("avatar")
        .and_then(|a| str_field(a, "large").or_else(|| str_field(a, "medium")));
    Ok(Viewer {
        id,
        name,
        avatar_url,
    })
}

/// Shared media-object shredder used by search, by-id, and list entries.
pub fn parse_media_obj(m: &Value) -> MediaInfo {
    let title = m.get("title");
    let synonyms = m
        .get("synonyms")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    MediaInfo {
        anilist_id: m.get("id").and_then(Value::as_i64).unwrap_or(0),
        title_romaji: title.and_then(|t| str_field(t, "romaji")),
        title_english: title.and_then(|t| str_field(t, "english")),
        title_native: title.and_then(|t| str_field(t, "native")),
        synonyms,
        cover_url: m.get("coverImage").and_then(|c| str_field(c, "large")),
        episode_count: m.get("episodes").and_then(Value::as_i64),
        format: str_field(m, "format"),
    }
}

/// Shred one media object of the overview query (description + meta).
pub fn parse_media_overview(m: &Value) -> MediaOverview {
    let genres = m
        .get("genres")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    MediaOverview {
        anilist_id: m.get("id").and_then(Value::as_i64).unwrap_or(0),
        description: str_field(m, "description"),
        genres,
        average_score: m.get("averageScore").and_then(Value::as_i64),
        season_year: m.get("seasonYear").and_then(Value::as_i64),
    }
}

pub fn parse_media_search(v: &Value) -> Result<Vec<MediaInfo>> {
    let arr = v
        .pointer("/data/Page/media")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::AniList("search: missing data.Page.media".into()))?;
    Ok(arr.iter().map(parse_media_obj).collect())
}

/// Flatten `MediaListCollection.lists[].entries[]` into remote entries.
pub fn parse_media_list_collection(v: &Value) -> Result<Vec<RemoteListEntry>> {
    let lists = v
        .pointer("/data/MediaListCollection/lists")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::AniList("list: missing data.MediaListCollection.lists".into()))?;

    let mut out = Vec::new();
    for list in lists {
        let Some(entries) = list.get("entries").and_then(Value::as_array) else {
            continue;
        };
        for e in entries {
            let media = e.get("media");
            let Some(anilist_id) = media.and_then(|m| m.get("id")).and_then(Value::as_i64) else {
                continue;
            };
            let Some(status) = e
                .get("status")
                .and_then(Value::as_str)
                .and_then(MediaListStatus::parse)
            else {
                continue;
            };
            let progress = e.get("progress").and_then(Value::as_i64).unwrap_or(0);
            let score = e
                .get("score")
                .and_then(Value::as_f64)
                .filter(|s| *s > 0.0);
            let updated_at = e.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let title = media.and_then(|m| m.get("title"));
            out.push(RemoteListEntry {
                anilist_id,
                status,
                progress,
                score,
                updated_at,
                title_romaji: title.and_then(|t| str_field(t, "romaji")),
                title_english: title.and_then(|t| str_field(t, "english")),
                cover_url: media
                    .and_then(|m| m.get("coverImage"))
                    .and_then(|c| str_field(c, "large")),
                episode_count: media.and_then(|m| m.get("episodes")).and_then(Value::as_i64),
            });
        }
    }
    Ok(out)
}

/// Shred one home-row media object (`Media` with `nextAiringEpisode`).
pub fn parse_home_media(m: &Value) -> HomeMedia {
    let title = m.get("title");
    let airing = m.get("nextAiringEpisode").filter(|a| !a.is_null());
    HomeMedia {
        anilist_id: m.get("id").and_then(Value::as_i64).unwrap_or(0),
        title_romaji: title.and_then(|t| str_field(t, "romaji")),
        title_english: title.and_then(|t| str_field(t, "english")),
        cover_url: m.get("coverImage").and_then(|c| str_field(c, "large")),
        episode_count: m.get("episodes").and_then(Value::as_i64),
        format: str_field(m, "format"),
        status: str_field(m, "status"),
        next_episode: airing.and_then(|a| a.get("episode")).and_then(Value::as_i64),
        airing_at: airing.and_then(|a| a.get("airingAt")).and_then(Value::as_i64),
        season_year: m.get("seasonYear").and_then(Value::as_i64),
        average_score: m.get("averageScore").and_then(Value::as_i64),
    }
}

/// Shred one catalog-search media object into `CatalogMedia`.
pub fn parse_catalog_media(m: &Value) -> CatalogMedia {
    let title = m.get("title");
    let airing = m.get("nextAiringEpisode").filter(|a| !a.is_null());
    let genres = m
        .get("genres")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    CatalogMedia {
        anilist_id: m.get("id").and_then(Value::as_i64).unwrap_or(0),
        title_romaji: title.and_then(|t| str_field(t, "romaji")),
        title_english: title.and_then(|t| str_field(t, "english")),
        cover_url: m.get("coverImage").and_then(|c| str_field(c, "large")),
        format: str_field(m, "format"),
        episode_count: m.get("episodes").and_then(Value::as_i64),
        average_score: m.get("averageScore").and_then(Value::as_i64),
        season_year: m.get("seasonYear").and_then(Value::as_i64),
        status: str_field(m, "status"),
        genres,
        next_episode: airing.and_then(|a| a.get("episode")).and_then(Value::as_i64),
        is_adult: m.get("isAdult").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Shred a `Page { pageInfo media }` catalog-search response.
pub fn parse_catalog_search(v: &Value) -> Result<CatalogPage> {
    let page = v
        .pointer("/data/Page")
        .ok_or_else(|| Error::AniList("search: missing data.Page".into()))?;
    let media = page
        .get("media")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::AniList("search: missing data.Page.media".into()))?;
    let info = page.get("pageInfo");
    Ok(CatalogPage {
        media: media
            .iter()
            .filter(|m| !m.is_null())
            .map(parse_catalog_media)
            .collect(),
        has_next_page: info
            .and_then(|p| p.get("hasNextPage"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        current_page: info
            .and_then(|p| p.get("currentPage"))
            .and_then(Value::as_i64)
            .unwrap_or(1),
    })
}

/// Shred `MediaTagCollection` into tag rows for the search filter picker.
pub fn parse_media_tags(v: &Value) -> Result<Vec<MediaTag>> {
    let arr = v
        .pointer("/data/MediaTagCollection")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::AniList("tags: missing data.MediaTagCollection".into()))?;
    Ok(arr
        .iter()
        .filter_map(|t| {
            Some(MediaTag {
                name: str_field(t, "name")?,
                category: str_field(t, "category"),
                is_adult: t.get("isAdult").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

fn parse_home_page(v: &Value, alias: &str) -> Vec<HomeMedia> {
    v.pointer(&format!("/data/{alias}/media"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|m| !m.is_null()).map(parse_home_media).collect())
        .unwrap_or_default()
}

/// Shred the three aliased Pages of the home query into `HomeSections`.
pub fn parse_home_sections(v: &Value) -> Result<HomeSections> {
    if v.pointer("/data").is_none() {
        return Err(Error::AniList("home: missing data".into()));
    }
    Ok(HomeSections {
        trending: parse_home_page(v, "trending"),
        season: parse_home_page(v, "season"),
        next_season: parse_home_page(v, "next"),
    })
}

/// Shred the batched `Page.media[]` airing query into `AiringInfo` rows.
pub fn parse_airing(v: &Value) -> Result<Vec<AiringInfo>> {
    let arr = v
        .pointer("/data/Page/media")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::AniList("airing: missing data.Page.media".into()))?;
    Ok(arr
        .iter()
        .filter(|m| !m.is_null())
        .filter_map(|m| {
            let anilist_id = m.get("id").and_then(Value::as_i64)?;
            let airing = m.get("nextAiringEpisode").filter(|a| !a.is_null());
            Some(AiringInfo {
                anilist_id,
                media_status: str_field(m, "status"),
                next_episode: airing.and_then(|a| a.get("episode")).and_then(Value::as_i64),
                airing_at: airing.and_then(|a| a.get("airingAt")).and_then(Value::as_i64),
            })
        })
        .collect())
}

/// Return the server-side `updatedAt` from a `SaveMediaListEntry` response.
pub fn parse_save_response(v: &Value) -> Result<i64> {
    v.pointer("/data/SaveMediaListEntry/updatedAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::AniList("save: missing SaveMediaListEntry.updatedAt".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn viewer_parses_name_and_avatar() {
        let v = json!({"data":{"Viewer":{"id":42,"name":"kuba","avatar":{"large":"http://a/x.png","medium":"http://a/m.png"}}}});
        let viewer = parse_viewer(&v).unwrap();
        assert_eq!(viewer.id, 42);
        assert_eq!(viewer.name, "kuba");
        assert_eq!(viewer.avatar_url.as_deref(), Some("http://a/x.png"));
    }

    #[test]
    fn viewer_missing_errors() {
        assert!(parse_viewer(&json!({"data":{}})).is_err());
    }

    #[test]
    fn media_search_extracts_all_fields() {
        let v = json!({"data":{"Page":{"media":[
            {"id":154587,"episodes":28,"format":"TV",
             "title":{"romaji":"Sousou no Frieren","english":"Frieren","native":"葬送のフリーレン"},
             "synonyms":["Frieren at the Funeral"],
             "coverImage":{"large":"http://c/154587.jpg"}},
            {"id":1,"episodes":null,"format":null,
             "title":{"romaji":"X","english":null,"native":null},"synonyms":[],"coverImage":{}}
        ]}}});
        let r = parse_media_search(&v).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].anilist_id, 154587);
        assert_eq!(r[0].episode_count, Some(28));
        assert_eq!(r[0].title_english.as_deref(), Some("Frieren"));
        assert_eq!(r[0].synonyms, vec!["Frieren at the Funeral"]);
        assert_eq!(r[0].cover_url.as_deref(), Some("http://c/154587.jpg"));
        assert_eq!(r[1].episode_count, None);
        assert_eq!(r[1].title_english, None);
        assert_eq!(r[1].cover_url, None);
    }

    #[test]
    fn list_collection_flattens_sublists() {
        let v = json!({"data":{"MediaListCollection":{"lists":[
            {"entries":[
                {"status":"CURRENT","progress":5,"score":8.5,"updatedAt":1700,
                 "media":{"id":100,"episodes":12,"format":"TV",
                   "title":{"romaji":"A","english":"A-en"},"coverImage":{"large":"http://c/a.jpg"}}},
                {"status":"COMPLETED","progress":12,"score":0,"updatedAt":1600,
                 "media":{"id":200,"episodes":12,"title":{"romaji":"B"},"coverImage":{}}}
            ]},
            {"entries":[
                {"status":"PLANNING","progress":0,"updatedAt":1500,
                 "media":{"id":300,"title":{"romaji":"C"}}}
            ]}
        ]}}});
        let r = parse_media_list_collection(&v).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].anilist_id, 100);
        assert_eq!(r[0].status, MediaListStatus::Current);
        assert_eq!(r[0].progress, 5);
        assert_eq!(r[0].score, Some(8.5));
        assert_eq!(r[0].updated_at, 1700);
        assert_eq!(r[0].episode_count, Some(12));
        // score 0 → None
        assert_eq!(r[1].score, None);
        assert_eq!(r[2].status, MediaListStatus::Planning);
        assert_eq!(r[2].anilist_id, 300);
    }

    #[test]
    fn save_response_returns_updated_at() {
        let v = json!({"data":{"SaveMediaListEntry":{"id":9,"status":"CURRENT","progress":3,"updatedAt":1712345678}}});
        assert_eq!(parse_save_response(&v).unwrap(), 1712345678);
    }

    #[test]
    fn home_sections_parse_all_three_rows() {
        // Shape captured live from graphql.anilist.co (2026-07).
        let v = json!({"data":{
            "trending":{"media":[
                {"id":177699,"title":{"romaji":"Koukaku Kidoutai","english":"THE GHOST IN THE SHELL"},
                 "coverImage":{"large":"http://c/177699.png"},"episodes":null,"format":"TV",
                 "status":"RELEASING","nextAiringEpisode":{"episode":3,"airingAt":1784642400}}
            ]},
            "season":{"media":[
                {"id":21,"title":{"romaji":"ONE PIECE","english":"ONE PIECE"},
                 "coverImage":{"large":"http://c/21.jpg"},"episodes":null,"format":"TV",
                 "status":"RELEASING","nextAiringEpisode":{"episode":1170,"airingAt":1784470560}}
            ]},
            "next":{"media":[
                {"id":900,"title":{"romaji":"Future Show","english":null},
                 "coverImage":{"large":"http://c/900.jpg"},"episodes":12,"format":"TV",
                 "status":"NOT_YET_RELEASED","nextAiringEpisode":null}
            ]}
        }});
        let s = parse_home_sections(&v).unwrap();
        assert_eq!(s.trending.len(), 1);
        assert_eq!(s.trending[0].anilist_id, 177699);
        assert_eq!(s.trending[0].next_episode, Some(3));
        assert_eq!(s.trending[0].airing_at, Some(1784642400));
        assert_eq!(s.season[0].anilist_id, 21);
        assert_eq!(s.next_season[0].anilist_id, 900);
        // NOT_YET_RELEASED show with null airing → no caption fields.
        assert_eq!(s.next_season[0].next_episode, None);
        assert_eq!(s.next_season[0].status.as_deref(), Some("NOT_YET_RELEASED"));
    }

    #[test]
    fn home_sections_tolerates_missing_pages() {
        let v = json!({"data":{"trending":{"media":[]}}});
        let s = parse_home_sections(&v).unwrap();
        assert!(s.trending.is_empty() && s.season.is_empty() && s.next_season.is_empty());
        assert!(parse_home_sections(&json!({"errors":[]})).is_err());
    }

    #[test]
    fn home_media_parses_year_and_score() {
        let m = json!({"id":154587,"title":{"romaji":"Sousou no Frieren","english":"Frieren"},
            "coverImage":{"large":"http://c/1.jpg"},"episodes":28,"format":"TV","status":"FINISHED",
            "seasonYear":2023,"averageScore":91,"nextAiringEpisode":null});
        let h = parse_home_media(&m);
        assert_eq!(h.season_year, Some(2023));
        assert_eq!(h.average_score, Some(91));
    }

    #[test]
    fn catalog_search_parses_page_and_meta() {
        // Shape captured live from graphql.anilist.co (2026-07): a finished show,
        // a not-yet-released one (null episodes/score/airing), and a releasing one.
        let v = json!({"data":{"Page":{
            "pageInfo":{"currentPage":1,"hasNextPage":true},
            "media":[
                {"id":154587,"title":{"romaji":"Sousou no Frieren","english":"Frieren"},
                 "coverImage":{"large":"http://c/1.jpg"},"format":"TV","episodes":28,
                 "averageScore":91,"seasonYear":2023,"status":"FINISHED",
                 "genres":["Adventure","Drama","Fantasy"],"nextAiringEpisode":null,"isAdult":false},
                {"id":189046,"title":{"romaji":"Re:Zero 4th Season","english":null},
                 "coverImage":{"large":"http://c/2.jpg"},"format":"TV","episodes":19,
                 "averageScore":90,"seasonYear":2026,"status":"RELEASING",
                 "genres":["Drama","Fantasy","Psychological"],"nextAiringEpisode":{"episode":12},"isAdult":false},
                {"id":113417,"title":{"romaji":"Overflow","english":null},
                 "coverImage":{},"format":"ONA","episodes":null,"averageScore":null,
                 "seasonYear":null,"status":"NOT_YET_RELEASED","genres":[],"nextAiringEpisode":null,"isAdult":true}
            ]}}});
        let p = parse_catalog_search(&v).unwrap();
        assert!(p.has_next_page);
        assert_eq!(p.current_page, 1);
        assert_eq!(p.media.len(), 3);
        assert_eq!(p.media[0].anilist_id, 154587);
        assert_eq!(p.media[0].average_score, Some(91));
        assert_eq!(p.media[0].season_year, Some(2023));
        assert_eq!(p.media[0].genres, vec!["Adventure", "Drama", "Fantasy"]);
        assert!(!p.media[0].is_adult);
        // Releasing show: nextAiringEpisode drives the aired/total chip (aired = 12-1).
        assert_eq!(p.media[1].next_episode, Some(12));
        assert_eq!(p.media[1].episode_count, Some(19));
        // Null-heavy adult ONA parses cleanly.
        assert_eq!(p.media[2].cover_url, None);
        assert_eq!(p.media[2].episode_count, None);
        assert!(p.media[2].is_adult);
        assert!(p.media[2].genres.is_empty());
    }

    #[test]
    fn media_overview_parses_description_and_meta() {
        // Shape as returned by graphql.anilist.co: description carries simple
        // HTML (<br>, <i>) and entities; genres is a plain string array.
        let m = json!({"id":154587,
            "description":"After the party of heroes defeated the Demon King&hellip;<br><br><i>(Source: Crunchyroll)</i>",
            "genres":["Adventure","Drama","Fantasy"],"averageScore":91,"seasonYear":2023});
        let o = parse_media_overview(&m);
        assert_eq!(o.anilist_id, 154587);
        assert!(o.description.unwrap().starts_with("After the party"));
        assert_eq!(o.genres, vec!["Adventure", "Drama", "Fantasy"]);
        assert_eq!(o.average_score, Some(91));
        assert_eq!(o.season_year, Some(2023));
        // Null-heavy media parses cleanly.
        let o = parse_media_overview(&json!({"id":1,"description":null,"genres":null}));
        assert_eq!(o.description, None);
        assert!(o.genres.is_empty());
        assert_eq!(o.average_score, None);
    }

    #[test]
    fn catalog_search_missing_page_errors() {
        assert!(parse_catalog_search(&json!({"errors":[]})).is_err());
    }

    #[test]
    fn media_tags_parse_name_category_adult() {
        let v = json!({"data":{"MediaTagCollection":[
            {"name":"4-koma","category":"Technical","isAdult":false},
            {"name":"Ahegao","category":"Sexual Content","isAdult":true},
            {"name":null,"category":"Bad","isAdult":false}
        ]}});
        let t = parse_media_tags(&v).unwrap();
        // The null-named row is dropped.
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "4-koma");
        assert_eq!(t[0].category.as_deref(), Some("Technical"));
        assert!(!t[0].is_adult);
        assert!(t[1].is_adult);
    }

    #[test]
    fn airing_parses_releasing_and_finished() {
        // Captured live: FINISHED → null nextAiringEpisode; RELEASING → set.
        let v = json!({"data":{"Page":{"media":[
            {"id":1,"status":"FINISHED","episodes":26,"nextAiringEpisode":null},
            {"id":21,"status":"RELEASING","episodes":null,
             "nextAiringEpisode":{"episode":1170,"airingAt":1784470560}}
        ]}}});
        let a = parse_airing(&v).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].anilist_id, 1);
        assert_eq!(a[0].media_status.as_deref(), Some("FINISHED"));
        assert_eq!(a[0].next_episode, None);
        assert_eq!(a[1].anilist_id, 21);
        assert_eq!(a[1].next_episode, Some(1170));
        assert_eq!(a[1].airing_at, Some(1784470560));
    }

    #[test]
    fn graphql_errors_have_no_data() {
        // The client surfaces `errors`; the parser must still fail cleanly if
        // asked to shred a payload with no data.
        let v = json!({"errors":[{"message":"Invalid token"}]});
        assert!(parse_viewer(&v).is_err());
        assert!(parse_media_search(&v).is_err());
    }
}
