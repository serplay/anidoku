//! Pure JSON parsing for allanime responses. Kept free of network I/O so it
//! can be unit-tested against captured fixtures.

use crate::models::{AnimeSummary, StreamKind, SubtitleTrack, VideoSource};
use crate::{Error, Result};
use serde_json::Value;

/// Parse the `shows.edges` array of a search response into summaries.
pub fn parse_search(body: &str) -> Result<Vec<AnimeSummary>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Provider(format!("search: invalid json: {e}")))?;
    let edges = v
        .pointer("/data/shows/edges")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Provider("search: missing data.shows.edges".into()))?;

    let mut out = Vec::with_capacity(edges.len());
    for e in edges {
        let Some(id) = e.get("_id").and_then(Value::as_str) else {
            continue;
        };
        let title = e
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        let title_english = e
            .get("englishName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let cover_url = e
            .get("thumbnail")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // availableEpisodes is an object { sub, dub, raw }; grab the max seen.
        let available_episodes = e
            .get("availableEpisodes")
            .and_then(|ae| {
                ae.get("sub")
                    .or_else(|| ae.get("dub"))
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0) as u32;

        // allanime exposes `aniListId` on the Show, usually a string like
        // "154587" (occasionally a number, occasionally null/empty). It is an
        // exact AniList media-id mapping, so keep it when parseable.
        let anilist_id = e.get("aniListId").and_then(|v| match v {
            Value::String(s) => s.trim().parse::<i64>().ok(),
            Value::Number(n) => n.as_i64(),
            _ => None,
        });

        out.push(AnimeSummary {
            provider_id: id.to_string(),
            title,
            title_english,
            cover_url,
            available_episodes,
            anilist_id,
        });
    }
    Ok(out)
}

/// Parse `availableEpisodesDetail.{sub,dub}` into an ascending list of
/// episode-number strings.
pub fn parse_episodes(body: &str, mode: &str) -> Result<Vec<String>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| Error::Provider(format!("episodes: invalid json: {e}")))?;
    let arr = v
        .pointer(&format!("/data/show/availableEpisodesDetail/{mode}"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Provider("episodes: missing availableEpisodesDetail".into()))?;

    let mut eps: Vec<String> = arr
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect();
    eps.sort_by(|a, b| {
        let pa: f64 = a.parse().unwrap_or(f64::MAX);
        let pb: f64 = b.parse().unwrap_or(f64::MAX);
        pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(eps)
}

/// One provider embed reference from the (decrypted) episode-sources response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub name: String,
    pub url: String,
}

/// Extract `{ sourceName, sourceUrl }` pairs from the decrypted sources JSON.
///
/// Distinguishes two failure shapes that used to collapse into one error:
///  - **Legitimately source-less episode** — the `episode` object is present but
///    its `sourceUrls` is null/absent/`[]`. Some shows can be searched and have
///    episodes listed but simply have no playable hosts. This returns
///    `Ok(vec![])` so the UI shows a clean "no sources" state instead of a scary
///    error, and the caller does not treat it as provider breakage.
///  - **Query/crypto failure** — the response carries a GraphQL `errors` array
///    (e.g. `AA_CRYPTO_STALE`, `PersistedQueryNotFound`) or no `episode` object
///    at all. This returns `Err` so the caller's self-heal (config refresh +
///    retry) fires and the user sees a real problem, never silent emptiness.
pub fn parse_source_refs(decrypted_json: &str) -> Result<Vec<SourceRef>> {
    let v: Value = serde_json::from_str(decrypted_json)
        .map_err(|e| Error::Provider(format!("sources: invalid json: {e}")))?;

    // A GraphQL/crypto-level rejection comes back as `{"errors":[{message,..}]}`
    // (often with no `data`). Surface it so self-heal fires — do not mistake it
    // for a source-less episode.
    if let Some(errs) = v.get("errors").and_then(Value::as_array) {
        if !errs.is_empty() {
            let msg = errs
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            let msg = if msg.is_empty() {
                "unspecified api error".to_string()
            } else {
                msg
            };
            return Err(Error::Provider(format!("sources: api error: {msg}")));
        }
    }

    // The persisted-query payload decrypts to `{"episode":{"sourceUrls":..}}`;
    // the POST fallback nests it under `data`. Accept either shape. A missing /
    // null `episode` means the query itself did not resolve (breakage), not an
    // empty episode — error so self-heal fires.
    let Some(episode) = v
        .pointer("/data/episode")
        .or_else(|| v.pointer("/episode"))
        .filter(|e| !e.is_null())
    else {
        return Err(Error::Provider(
            "sources: missing episode in response".into(),
        ));
    };

    // Episode present but no `sourceUrls` array (null / absent / empty) => this
    // episode legitimately has no playable sources.
    let Some(arr) = episode.get("sourceUrls").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for s in arr {
        let (Some(url), Some(name)) = (
            s.get("sourceUrl").and_then(Value::as_str),
            s.get("sourceName").and_then(Value::as_str),
        ) else {
            continue;
        };
        out.push(SourceRef {
            name: name.to_string(),
            url: url.to_string(),
        });
    }
    Ok(out)
}

/// Parse a `/clock.json`-style embed response body into playable links.
///
/// The response is `{ "links": [ { "link": "...", "resolutionStr": "1080",
/// "hls": bool?, "subtitles": [...] }, ... ] }`. Mirrors ani-cli's
/// `get_links` extraction but keeps subtitle tracks (ani-cli discards them).
pub fn parse_clock_links(
    body: &str,
    provider_name: &str,
    referer: Option<&str>,
) -> Vec<VideoSource> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(links) = v.get("links").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for l in links {
        let Some(url) = l.get("link").and_then(Value::as_str) else {
            continue;
        };
        let quality = l
            .get("resolutionStr")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .to_string();
        let is_hls =
            l.get("hls").and_then(Value::as_bool).unwrap_or(false) || url.contains(".m3u8");
        let kind = if is_hls {
            StreamKind::Hls
        } else {
            StreamKind::Mp4
        };

        let subtitles = l
            .get("subtitles")
            .and_then(Value::as_array)
            .map(|subs| {
                subs.iter()
                    .filter_map(|s| {
                        let url = s.get("src").or_else(|| s.get("url"))?.as_str()?;
                        let lang = s
                            .get("lang")
                            .or_else(|| s.get("label"))
                            .and_then(Value::as_str)
                            .unwrap_or("und");
                        Some(SubtitleTrack {
                            label: s
                                .get("label")
                                .and_then(Value::as_str)
                                .unwrap_or(lang)
                                .to_string(),
                            lang: lang.to_string(),
                            url: url.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        out.push(VideoSource {
            provider_name: provider_name.to_string(),
            quality,
            url: url.to_string(),
            kind,
            referer: referer.map(str::to_string),
            subtitles,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_extracts_fields() {
        let body = r#"{"data":{"shows":{"edges":[
            {"_id":"abc","name":"Frieren","englishName":"Frieren: Beyond","aniListId":"154587","thumbnail":"http://x/c.jpg","availableEpisodes":{"sub":28,"dub":12,"raw":0},"__typename":"Show"},
            {"_id":"def","name":"No English","aniListId":"","availableEpisodes":{"sub":5},"__typename":"Show"}
        ]}}}"#;
        let r = parse_search(body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].provider_id, "abc");
        assert_eq!(r[0].title, "Frieren");
        assert_eq!(r[0].title_english.as_deref(), Some("Frieren: Beyond"));
        assert_eq!(r[0].cover_url.as_deref(), Some("http://x/c.jpg"));
        assert_eq!(r[0].available_episodes, 28);
        assert_eq!(r[0].anilist_id, Some(154587));
        assert_eq!(r[1].title_english, None);
        assert_eq!(r[1].cover_url, None);
        assert_eq!(r[1].available_episodes, 5);
        assert_eq!(r[1].anilist_id, None);
    }

    #[test]
    fn parse_episodes_sorts_numerically_with_fractions() {
        let body = r#"{"data":{"show":{"_id":"x","availableEpisodesDetail":{"sub":["10","2","1","5.5","5"]}}}}"#;
        let eps = parse_episodes(body, "sub").unwrap();
        assert_eq!(eps, vec!["1", "2", "5", "5.5", "10"]);
    }

    #[test]
    fn parse_episodes_missing_mode_errors() {
        let body = r#"{"data":{"show":{"_id":"x","availableEpisodesDetail":{"sub":["1"]}}}}"#;
        assert!(parse_episodes(body, "dub").is_err());
    }

    #[test]
    fn parse_source_refs_extracts_pairs() {
        let body = r#"{"data":{"episode":{"episodeString":"1","sourceUrls":[
            {"sourceUrl":"--1748abcd","sourceName":"Default","priority":9},
            {"sourceUrl":"https://tools.fast4speed.rsvp/x","sourceName":"Yt"}
        ]}}}"#;
        let refs = parse_source_refs(body).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "Default");
        assert_eq!(refs[0].url, "--1748abcd");
        assert_eq!(refs[1].name, "Yt");
    }

    #[test]
    fn parse_source_refs_sourceless_episode_is_empty_not_error() {
        // Episode present but no playable hosts: null, absent, and [] all mean
        // "no sources" — Ok(empty), so the UI shows a clean message rather than
        // erroring on play.
        for body in [
            r#"{"data":{"episode":{"episodeString":"1","sourceUrls":null}}}"#,
            r#"{"data":{"episode":{"episodeString":"1"}}}"#,
            r#"{"data":{"episode":{"episodeString":"1","sourceUrls":[]}}}"#,
            r#"{"episode":{"sourceUrls":null}}"#,
        ] {
            let refs = parse_source_refs(body).expect("source-less episode must not error");
            assert!(refs.is_empty(), "expected empty for {body}");
        }
    }

    #[test]
    fn parse_source_refs_api_error_and_missing_episode_still_error() {
        // A crypto/GraphQL rejection or a missing episode object is real
        // breakage — must Err so self-heal (config refresh + retry) fires.
        let stale = r#"{"errors":[{"message":"AA_CRYPTO_STALE"}]}"#;
        let err = parse_source_refs(stale).unwrap_err().to_string();
        assert!(err.contains("AA_CRYPTO_STALE"), "got: {err}");

        assert!(parse_source_refs(r#"{"data":{"episode":null}}"#).is_err());
        assert!(parse_source_refs(r#"{"data":{}}"#).is_err());
        assert!(parse_source_refs(r#"{"randomshape":1}"#).is_err());
    }

    #[test]
    fn parse_clock_links_hls_and_mp4_with_subs() {
        let body = r#"{"links":[
            {"link":"https://cdn/master.m3u8","resolutionStr":"auto","hls":true,
             "subtitles":[{"src":"https://cdn/en.vtt","lang":"en","label":"English"}]},
            {"link":"https://cdn/720.mp4","resolutionStr":"720"}
        ]}"#;
        let out = parse_clock_links(body, "wixmp", Some("https://ref"));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, StreamKind::Hls);
        assert_eq!(out[0].referer.as_deref(), Some("https://ref"));
        assert_eq!(out[0].subtitles.len(), 1);
        assert_eq!(out[0].subtitles[0].lang, "en");
        assert_eq!(out[1].kind, StreamKind::Mp4);
        assert_eq!(out[1].quality, "720");
    }

    #[test]
    fn parse_clock_links_detects_hls_by_extension() {
        let body = r#"{"links":[{"link":"https://cdn/x.m3u8","resolutionStr":"1080"}]}"#;
        let out = parse_clock_links(body, "p", None);
        assert_eq!(out[0].kind, StreamKind::Hls);
    }

    #[test]
    fn parse_clock_links_empty_on_garbage() {
        assert!(parse_clock_links("not json", "p", None).is_empty());
        assert!(parse_clock_links("{}", "p", None).is_empty());
    }
}
