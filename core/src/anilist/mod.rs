//! AniList GraphQL client (https://graphql.anilist.co).
//!
//! Covers the four operations M2 needs:
//!   - `Viewer`               — who is logged in (name + avatar)
//!   - `MediaListCollection`  — pull the user's full list for conflict merge
//!   - `SaveMediaListEntry`   — push status/progress/score
//!   - `Media` search / by-id — resolve a provider show to an AniList id
//!
//! All network I/O lives on `AniListClient`; the response shredding lives in
//! free `parse_*` functions so they can be unit-tested against captured JSON.
//!
//! Rate limiting: AniList allows ~90 req/min and advertises
//! `X-RateLimit-Remaining`; on a 429 it sends `Retry-After` (seconds). We
//! honour a single automatic retry on 429, and otherwise surface
//! `Error::RateLimited` so the caller (sync worker) can back off.

mod parse;
pub mod season;

pub use parse::{
    parse_airing, parse_home_sections, parse_media_list_collection, parse_media_search,
    parse_save_response, parse_viewer,
};
pub use season::{current_and_next_from_unix, Season};

use crate::models::{AiringInfo, HomeSections, MediaInfo, MediaListStatus, RemoteListEntry, Viewer};
use crate::{Error, Result};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

pub const GRAPHQL_URL: &str = "https://graphql.anilist.co";
pub const AUTHORIZE_URL: &str = "https://anilist.co/api/v2/oauth/authorize";

const VIEWER_QUERY: &str = "query { Viewer { id name avatar { large medium } } }";

const MEDIA_LIST_QUERY: &str = "\
query ($userId: Int) { \
  MediaListCollection(userId: $userId, type: ANIME) { \
    lists { entries { \
      status progress score(format: POINT_10_DECIMAL) updatedAt \
      media { id episodes format \
        title { romaji english } coverImage { large } } \
    } } \
  } \
}";

const SAVE_MUTATION: &str = "\
mutation ($mediaId: Int, $status: MediaListStatus, $progress: Int, $score: Float) { \
  SaveMediaListEntry(mediaId: $mediaId, status: $status, progress: $progress, score: $score) { \
    id status progress score updatedAt \
  } \
}";

const SEARCH_QUERY: &str = "\
query ($search: String) { \
  Page(perPage: 10) { media(search: $search, type: ANIME) { \
    id episodes format \
    title { romaji english native } synonyms coverImage { large } \
  } } \
}";

const MEDIA_BY_ID_QUERY: &str = "\
query ($id: Int) { Media(id: $id, type: ANIME) { \
  id episodes format title { romaji english native } synonyms coverImage { large } \
} }";

/// Home rows: Trending (releasing), Popular This Season, Popular Next Season —
/// one request, three aliased Pages, verified live against graphql.anilist.co.
/// `nextAiringEpisode` powers the "Ep N in Xd" caption on airing shows.
const HOME_QUERY: &str = "\
query ($season: MediaSeason, $seasonYear: Int, $nextSeason: MediaSeason, $nextYear: Int, $perPage: Int) { \
  trending: Page(perPage: $perPage) { media(sort: TRENDING_DESC, type: ANIME, status: RELEASING) { \
    id title { romaji english } coverImage { large } episodes format status \
    nextAiringEpisode { episode airingAt } } } \
  season: Page(perPage: $perPage) { media(season: $season, seasonYear: $seasonYear, sort: POPULARITY_DESC, type: ANIME) { \
    id title { romaji english } coverImage { large } episodes format status \
    nextAiringEpisode { episode airingAt } } } \
  next: Page(perPage: $perPage) { media(season: $nextSeason, seasonYear: $nextYear, sort: POPULARITY_DESC, type: ANIME) { \
    id title { romaji english } coverImage { large } episodes format status \
    nextAiringEpisode { episode airingAt } } } \
}";

/// Batched airing lookup for the tracker: one request, up to 50 ids/page.
const AIRING_QUERY: &str = "\
query ($ids: [Int]) { Page(perPage: 50) { media(id_in: $ids, type: ANIME) { \
  id status episodes nextAiringEpisode { episode airingAt } } } }";

/// Payload for a `SaveMediaListEntry` mutation. Serialized into `sync_queue`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SaveEntry {
    pub media_id: i64,
    pub status: MediaListStatus,
    pub progress: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// Variables for `SaveMediaListEntry`. A missing score must be OMITTED, not
/// sent as `null` — AniList's validation rejects `"score": null` with
/// "The score must be a number." (an unprovided nullable GraphQL variable
/// makes the argument behave as if it were not passed at all).
fn save_variables(e: &SaveEntry) -> Value {
    let mut vars = json!({
        "mediaId": e.media_id,
        "status": e.status.as_str(),
        "progress": e.progress,
    });
    if let Some(s) = e.score {
        vars["score"] = json!(s);
    }
    vars
}

pub struct AniListClient {
    client: Client,
}

impl Default for AniListClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AniListClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .build()
            .expect("reqwest client for anilist");
        Self { client }
    }

    /// POST a GraphQL operation. Handles auth (401 → `Unauthorized`), one
    /// automatic retry on 429, and GraphQL-level `errors` arrays.
    async fn post(&self, token: Option<&str>, query: &str, variables: Value) -> Result<Value> {
        let mut attempt = 0;
        loop {
            let mut req = self
                .client
                .post(GRAPHQL_URL)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json");
            if let Some(t) = token {
                req = req.bearer_auth(t);
            }
            let resp = req
                .json(&json!({ "query": query, "variables": variables }))
                .send()
                .await?;

            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Err(Error::Unauthorized);
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(60);
                if attempt == 0 {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_secs(retry.min(60))).await;
                    continue;
                }
                return Err(Error::RateLimited(retry));
            }

            let body = resp.text().await?;
            let v: Value = serde_json::from_str(&body)
                .map_err(|e| Error::AniList(format!("invalid json: {e}")))?;

            // GraphQL transports errors in a top-level `errors` array even on 200.
            if let Some(errs) = v.get("errors").and_then(Value::as_array) {
                if !errs.is_empty() {
                    // A 400 with an auth message means the token went bad.
                    let msg = errs
                        .iter()
                        .filter_map(|e| e.get("message").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("; ");
                    if status == StatusCode::BAD_REQUEST
                        && msg.to_lowercase().contains("invalid token")
                    {
                        return Err(Error::Unauthorized);
                    }
                    return Err(Error::AniList(msg));
                }
            }
            if !status.is_success() {
                return Err(Error::AniList(format!("http {status}")));
            }
            return Ok(v);
        }
    }

    pub async fn viewer(&self, token: &str) -> Result<Viewer> {
        let v = self.post(Some(token), VIEWER_QUERY, json!({})).await?;
        parse_viewer(&v)
    }

    /// Pull the user's whole anime list, flattened across AniList's per-status
    /// sub-lists into one vec.
    pub async fn media_list_collection(
        &self,
        token: &str,
        user_id: i64,
    ) -> Result<Vec<RemoteListEntry>> {
        let v = self
            .post(Some(token), MEDIA_LIST_QUERY, json!({ "userId": user_id }))
            .await?;
        parse_media_list_collection(&v)
    }

    /// Push one entry. Returns AniList's post-save `updatedAt` (unix seconds).
    pub async fn save_media_list_entry(&self, token: &str, e: &SaveEntry) -> Result<i64> {
        let v = self
            .post(Some(token), SAVE_MUTATION, save_variables(e))
            .await?;
        parse_save_response(&v)
    }

    /// Public (no-auth) title search for match resolution.
    pub async fn search_media(&self, query: &str) -> Result<Vec<MediaInfo>> {
        let v = self
            .post(None, SEARCH_QUERY, json!({ "search": query }))
            .await?;
        parse_media_search(&v)
    }

    /// Fetch the three home sections in one public (no-auth) request. Compute
    /// the season/year pair from the current time with `season::current_and_next_from_unix`.
    pub async fn home_sections(
        &self,
        season: Season,
        season_year: i64,
        next_season: Season,
        next_year: i64,
        per_page: i64,
    ) -> Result<HomeSections> {
        let v = self
            .post(
                None,
                HOME_QUERY,
                json!({
                    "season": season.as_str(),
                    "seasonYear": season_year,
                    "nextSeason": next_season.as_str(),
                    "nextYear": next_year,
                    "perPage": per_page,
                }),
            )
            .await?;
        parse_home_sections(&v)
    }

    /// Batched `nextAiringEpisode` lookup for the tracker (≤ 50 ids per call).
    pub async fn airing_for(&self, ids: &[i64]) -> Result<Vec<AiringInfo>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let v = self.post(None, AIRING_QUERY, json!({ "ids": ids })).await?;
        parse_airing(&v)
    }

    pub async fn media_by_id(&self, id: i64) -> Result<Option<MediaInfo>> {
        let v = self.post(None, MEDIA_BY_ID_QUERY, json!({ "id": id })).await?;
        let media = v.pointer("/data/Media");
        match media {
            Some(m) if !m.is_null() => Ok(Some(parse::parse_media_obj(m))),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_variables_omits_missing_score() {
        // Regression: AniList rejects "score": null with a 400 validation
        // error ("The score must be a number"), so None must mean absent.
        let vars = save_variables(&SaveEntry {
            media_id: 189046,
            status: MediaListStatus::Current,
            progress: 11,
            score: None,
        });
        assert!(vars.get("score").is_none());
        assert_eq!(vars["mediaId"], 189046);
        assert_eq!(vars["status"], "CURRENT");
        assert_eq!(vars["progress"], 11);
    }

    #[test]
    fn save_variables_includes_present_score() {
        let vars = save_variables(&SaveEntry {
            media_id: 1,
            status: MediaListStatus::Completed,
            progress: 12,
            score: Some(8.5),
        });
        assert_eq!(vars["score"], 8.5);
    }
}
