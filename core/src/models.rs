use serde::{Deserialize, Serialize};

/// Sub or dub stream variant (allanime "translationType").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TranslationType {
    #[default]
    Sub,
    Dub,
}

impl TranslationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TranslationType::Sub => "sub",
            TranslationType::Dub => "dub",
        }
    }
}

/// A search hit from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimeSummary {
    /// Provider-scoped id (allanime `_id`).
    pub provider_id: String,
    pub title: String,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    /// Episodes available for the requested translation type.
    pub available_episodes: u32,
    /// AniList media id carried by the provider, when it advertises one.
    /// allanime's Show object exposes `aniListId` (a string) which is an exact,
    /// free mapping — we surface it so M2's matcher can skip a search round-trip.
    #[serde(default)]
    pub anilist_id: Option<i64>,
}

/// One playable episode of a show. allanime episode "numbers" are strings
/// ("1", "5.5", "13.5"), so we keep them as strings end to end.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub anime_id: String,
    pub number: String,
}

/// The kind of stream a source points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Hls,
    Mp4,
}

/// A single resolved, directly-playable video link.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSource {
    /// Human name of the upstream host ("wixmp", "sharepoint", ...).
    pub provider_name: String,
    /// Quality label, e.g. "1080", "720", "hls-multi".
    pub quality: String,
    pub url: String,
    pub kind: StreamKind,
    /// Referer header required to fetch this URL, if any.
    pub referer: Option<String>,
    /// Subtitle tracks advertised alongside this source.
    pub subtitles: Vec<SubtitleTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub label: String,
    pub lang: String,
    pub url: String,
}

/// AniList media-list status. Serialized as AniList's SCREAMING enum values so
/// the same strings round-trip through the DB `list_entries.status` CHECK and
/// the GraphQL `SaveMediaListEntry` mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaListStatus {
    #[serde(rename = "CURRENT")]
    Current,
    #[serde(rename = "PLANNING")]
    Planning,
    #[serde(rename = "COMPLETED")]
    Completed,
    #[serde(rename = "DROPPED")]
    Dropped,
    #[serde(rename = "PAUSED")]
    Paused,
    #[serde(rename = "REPEATING")]
    Repeating,
}

impl MediaListStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaListStatus::Current => "CURRENT",
            MediaListStatus::Planning => "PLANNING",
            MediaListStatus::Completed => "COMPLETED",
            MediaListStatus::Dropped => "DROPPED",
            MediaListStatus::Paused => "PAUSED",
            MediaListStatus::Repeating => "REPEATING",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "CURRENT" => MediaListStatus::Current,
            "PLANNING" => MediaListStatus::Planning,
            "COMPLETED" => MediaListStatus::Completed,
            "DROPPED" => MediaListStatus::Dropped,
            "PAUSED" => MediaListStatus::Paused,
            "REPEATING" => MediaListStatus::Repeating,
            _ => return None,
        })
    }
}

/// A local `list_entries` row: the user's tracking state for one AniList show,
/// plus sync bookkeeping (dirty flag, local/remote timestamps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListEntry {
    pub anilist_id: i64,
    pub status: MediaListStatus,
    pub progress: i64,
    pub score: Option<f64>,
    pub local_updated_at: i64,
    pub remote_updated_at: Option<i64>,
    pub dirty: bool,
}

/// A library row for the UI: a `ListEntry` joined with whatever media metadata
/// we know (from AniList's list pull and/or the provider mapping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryItem {
    pub anilist_id: i64,
    pub status: MediaListStatus,
    pub progress: i64,
    pub score: Option<f64>,
    pub dirty: bool,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    pub episode_count: Option<i64>,
    /// Provider id, when this AniList show is mapped to a scraped show.
    pub provider_id: Option<String>,
}

/// The authenticated AniList user (Viewer query).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewer {
    pub id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
}

/// One entry as returned by `MediaListCollection` — the remote snapshot the
/// conflict resolver merges against local state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteListEntry {
    pub anilist_id: i64,
    pub status: MediaListStatus,
    pub progress: i64,
    pub score: Option<f64>,
    /// AniList `updatedAt` (unix seconds).
    pub updated_at: i64,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    pub episode_count: Option<i64>,
}

/// A minimal AniList media record (search / match resolution).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub anilist_id: i64,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub title_native: Option<String>,
    pub synonyms: Vec<String>,
    pub cover_url: Option<String>,
    pub episode_count: Option<i64>,
    pub format: Option<String>,
}

/// Resume-point row (watch_state table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchState {
    pub anime_id: String,
    pub episode_number: String,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub updated_at: i64,
}
