//! Pure JSON shredding for AniList GraphQL responses. No network I/O, so every
//! function here is unit-tested against captured fixtures.

use crate::models::{MediaInfo, MediaListStatus, RemoteListEntry, Viewer};
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
    fn graphql_errors_have_no_data() {
        // The client surfaces `errors`; the parser must still fail cleanly if
        // asked to shred a payload with no data.
        let v = json!({"errors":[{"message":"Invalid token"}]});
        assert!(parse_viewer(&v).is_err());
        assert!(parse_media_search(&v).is_err());
    }
}
