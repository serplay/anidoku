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

impl StreamKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamKind::Hls => "hls",
            StreamKind::Mp4 => "mp4",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hls" => Some(StreamKind::Hls),
            "mp4" => Some(StreamKind::Mp4),
            _ => None,
        }
    }
}

/// Lifecycle state of a download job (the `downloads.state` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Queued,
    Downloading,
    Paused,
    Done,
    Failed,
}

impl DownloadState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadState::Queued => "queued",
            DownloadState::Downloading => "downloading",
            DownloadState::Paused => "paused",
            DownloadState::Done => "done",
            DownloadState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => DownloadState::Queued,
            "downloading" => DownloadState::Downloading,
            "paused" => DownloadState::Paused,
            "done" => DownloadState::Done,
            "failed" => DownloadState::Failed,
            _ => return None,
        })
    }
}

/// One `downloads` row: a persistent, resumable download job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRow {
    pub id: i64,
    pub anime_id: String,
    pub episode_number: String,
    pub state: DownloadState,
    /// Requested quality preference ("best", "1080", ...).
    pub quality: Option<String>,
    pub dub: bool,
    /// Resolved stream kind, known once the download has started.
    pub kind: Option<StreamKind>,
    pub bytes_total: Option<i64>,
    pub bytes_done: i64,
    /// HLS resume checkpoint: number of contiguous segments fully written.
    pub segments_done: i64,
    pub segments_total: Option<i64>,
    /// Episode directory, relative to the downloads root ("<anime>/<ep>").
    pub dir_path: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Show title joined from the anime cache (listing queries only).
    #[serde(default)]
    pub title: Option<String>,
}

/// Storage accounting for the download manager: completed bytes per show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimeStorage {
    pub anime_id: String,
    pub title: Option<String>,
    pub cover_url: Option<String>,
    pub episodes: i64,
    pub bytes: i64,
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

/// Detail-page overview fetched by AniList id: the synopsis plus the meta the
/// hero doesn't get from the provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOverview {
    pub anilist_id: i64,
    /// AniList synopsis. May contain simple HTML (`<br>`, `<i>`) and entities;
    /// the UI sanitizes before rendering.
    pub description: Option<String>,
    pub genres: Vec<String>,
    pub average_score: Option<i64>,
    pub season_year: Option<i64>,
}

/// A media card for the home page rows (Trending / This Season / Next Season).
/// Serialized straight into `home_cache` as JSON and returned to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HomeMedia {
    pub anilist_id: i64,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    pub episode_count: Option<i64>,
    pub format: Option<String>,
    /// AniList media status ("RELEASING", "NOT_YET_RELEASED", "FINISHED", ...).
    pub status: Option<String>,
    /// Next airing episode number, when the show is currently releasing.
    pub next_episode: Option<i64>,
    /// Unix seconds the next episode airs at (drives the "Ep N in Xd" caption).
    pub airing_at: Option<i64>,
    /// Release year (startDate/seasonYear) for the card meta chip.
    #[serde(default)]
    pub season_year: Option<i64>,
    /// Weighted mean score 0–100 for the card meta chip.
    #[serde(default)]
    pub average_score: Option<i64>,
}

/// A search result from the AniList catalog (the reworked /search). Carries
/// everything AnimeCard's meta chips need plus `is_adult` for content gating.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CatalogMedia {
    pub anilist_id: i64,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    pub format: Option<String>,
    pub episode_count: Option<i64>,
    pub average_score: Option<i64>,
    pub season_year: Option<i64>,
    /// AniList media status ("RELEASING", "FINISHED", ...).
    pub status: Option<String>,
    pub genres: Vec<String>,
    /// Next airing episode number, when the show is currently releasing.
    pub next_episode: Option<i64>,
    pub is_adult: bool,
}

/// One page of catalog search results plus pagination bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CatalogPage {
    pub media: Vec<CatalogMedia>,
    pub has_next_page: bool,
    pub current_page: i64,
}

/// One AniList media tag (from `MediaTagCollection`), cached with a long TTL for
/// the search filter's type-to-filter tag picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaTag {
    pub name: String,
    pub category: Option<String>,
    pub is_adult: bool,
}

/// The three AniList-sourced home sections, one batched fetch.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HomeSections {
    pub trending: Vec<HomeMedia>,
    pub season: Vec<HomeMedia>,
    pub next_season: Vec<HomeMedia>,
}

/// One Continue-Watching card: a local CURRENT/REPEATING entry joined with the
/// media cache and provider mapping, plus the next unwatched episode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinueWatchingItem {
    pub anilist_id: i64,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    pub episode_count: Option<i64>,
    /// Watched-episode count (list progress).
    pub progress: i64,
    /// The episode number to resume on (progress + 1), as a string for the
    /// provider watch route. `None` when the show is already fully watched.
    pub next_episode: Option<String>,
    /// Provider id when a mapping exists (enables a direct deep-link).
    pub provider_id: Option<String>,
    /// Most recent watch activity (for ordering), unix seconds.
    pub last_watched_at: i64,
}

/// Remote airing snapshot for one media id, from the batched `media(id_in:)`
/// `nextAiringEpisode` query. Pure input to the airing-refresh planner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiringInfo {
    pub anilist_id: i64,
    /// AniList media status ("RELEASING", "FINISHED", "CANCELLED", ...).
    pub media_status: Option<String>,
    pub next_episode: Option<i64>,
    pub airing_at: Option<i64>,
}

/// A persisted `airing` row: the tracker's local view of when a show's next
/// episode airs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiringRow {
    pub anilist_id: i64,
    pub next_episode: Option<i64>,
    pub airing_at: Option<i64>,
    pub media_status: Option<String>,
    pub refreshed_at: i64,
}

/// A `notifications` row for the inbox, joined with media metadata for display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    pub id: i64,
    pub anilist_id: i64,
    pub episode: i64,
    pub airing_at: Option<i64>,
    /// Notification kind ("episode" for a released episode).
    pub kind: String,
    pub created_at: i64,
    pub read: bool,
    pub title_romaji: Option<String>,
    pub title_english: Option<String>,
    pub cover_url: Option<String>,
    /// Provider id when a mapping exists (enables a direct deep-link).
    pub provider_id: Option<String>,
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
