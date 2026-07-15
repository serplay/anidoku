//! Tauri IPC commands. Thin wrappers that translate between the UI and the
//! core crate; all real logic lives in `anidoku-core`.

use crate::{auth, AppState};
use anidoku_core::airing::{is_fresh, HOME_TTL_SECS};
use anidoku_core::anilist::current_and_next_from_unix;
use anidoku_core::downloads as downloads_core;
use anidoku_core::models::{
    AnimeStorage, AnimeSummary, ContinueWatchingItem, DownloadRow, DownloadState,
    HomeSections, LibraryItem, ListEntry, MediaInfo, MediaListStatus, Notification, StreamKind,
    TranslationType, VideoSource, Viewer, WatchState,
};
use anidoku_core::provider::Provider;
use anidoku_core::sync::{best_match, best_provider_match};
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

/// Search results plus the proxied stream-scheme prefix the UI needs.
type CmdResult<T> = Result<T, String>;

fn map_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub async fn search_anime(
    state: State<'_, AppState>,
    query: String,
    dub: bool,
) -> CmdResult<Vec<AnimeSummary>> {
    let mode = if dub { TranslationType::Dub } else { TranslationType::Sub };
    let results = state.provider.search(&query, mode).await.map_err(map_err)?;
    // Warm the metadata cache so detail pages can render offline.
    for r in &results {
        let _ = state.db.cache_anime(
            &r.provider_id,
            &r.title,
            r.title_english.as_deref(),
            r.cover_url.as_deref(),
            Some(r.available_episodes),
        );
    }
    Ok(results)
}

#[tauri::command]
pub async fn get_episodes(
    state: State<'_, AppState>,
    show_id: String,
    dub: bool,
) -> CmdResult<Vec<String>> {
    let mode = if dub { TranslationType::Dub } else { TranslationType::Sub };
    state.provider.episodes(&show_id, mode).await.map_err(map_err)
}

#[tauri::command]
pub async fn get_sources(
    state: State<'_, AppState>,
    show_id: String,
    episode: String,
    dub: bool,
) -> CmdResult<Vec<VideoSource>> {
    let mode = if dub { TranslationType::Dub } else { TranslationType::Sub };
    state
        .provider
        .sources(&show_id, &episode, mode)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub fn get_watch_state(
    state: State<'_, AppState>,
    anime_id: String,
    episode: String,
) -> CmdResult<Option<WatchState>> {
    state.db.get_watch_state(&anime_id, &episode).map_err(map_err)
}

#[tauri::command]
pub fn set_watch_state(
    app: AppHandle,
    state: State<'_, AppState>,
    anime_id: String,
    episode: String,
    position_secs: f64,
    duration_secs: Option<f64>,
) -> CmdResult<()> {
    state
        .db
        .set_watch_state(&anime_id, &episode, position_secs, duration_secs)
        .map_err(map_err)?;

    // Auto-progress: crossing 85% of an episode marks it watched. If the show
    // is mapped to AniList, bump progress locally (auto-adding to CURRENT when
    // unlisted, promoting to COMPLETED on the last episode) and enqueue a push.
    // An unmapped show is surfaced to the UI instead of no-oping silently.
    if let Some(dur) = duration_secs {
        if dur > 0.0 && position_secs / dur >= 0.85 {
            match (
                state.db.anilist_id_for_provider(&anime_id).ok().flatten(),
                episode_to_progress(&episode),
            ) {
                (Some(anilist_id), Some(progress)) => {
                    if let Ok((entry, changed)) = state.db.mark_watched(anilist_id, progress) {
                        if changed {
                            crate::sync::push_local(&state, &app, &entry);
                        }
                    }
                }
                (None, Some(_)) => {
                    let _ = app.emit("sync:unmapped", &anime_id);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Map an allanime episode string ("1", "13", "5.5") to an integer progress
/// value. Fractional specials floor down; anything < 1 is ignored.
fn episode_to_progress(ep: &str) -> Option<i64> {
    ep.parse::<f64>().ok().map(|f| f.floor() as i64).filter(|n| *n >= 1)
}

#[tauri::command]
pub fn list_watch_states(
    state: State<'_, AppState>,
    anime_id: String,
) -> CmdResult<Vec<WatchState>> {
    state.db.watch_states_for_anime(&anime_id).map_err(map_err)
}

/// Convert an external subtitle file's contents to WebVTT.
#[tauri::command]
pub fn convert_subtitles(content: String, format_hint: String) -> CmdResult<String> {
    anidoku_core::subs::to_vtt(&content, &format_hint).map_err(map_err)
}

/// Base URL of the loopback media server the UI should route playback through,
/// e.g. `http://127.0.0.1:52123`.
#[tauri::command]
pub fn media_base(state: State<'_, AppState>) -> CmdResult<String> {
    Ok(state.media_base.clone())
}

// ---------------------------------------------------------------------------
// AniList: settings, auth, library, sync
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Settings {
    pub client_id: Option<String>,
    /// The exact redirect URL the user must register on their AniList client.
    pub redirect_url: String,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    Ok(Settings {
        client_id: state.auth.client_id(),
        redirect_url: auth::REDIRECT_URL.to_string(),
    })
}

#[tauri::command]
pub fn set_client_id(state: State<'_, AppState>, client_id: Option<String>) -> CmdResult<()> {
    state.auth.set_client_id(client_id);
    Ok(())
}

#[derive(Serialize)]
pub struct AuthStatus {
    pub viewer: Option<Viewer>,
    pub logged_in: bool,
    pub expired: bool,
    pub has_client_id: bool,
}

#[tauri::command]
pub fn anilist_status(state: State<'_, AppState>) -> CmdResult<AuthStatus> {
    Ok(AuthStatus {
        viewer: state.auth.viewer(),
        logged_in: state.auth.valid_token().is_some(),
        expired: state.auth.is_expired(),
        has_client_id: state.auth.client_id().is_some(),
    })
}

/// Begin the OAuth implicit-grant login: start the loopback capture, open the
/// system browser to AniList, wait for the token, then fetch + cache the Viewer
/// and kick off an initial pull. Only public/no-auth work happens in the app
/// webview — the login itself is in the user's real browser.
#[tauri::command]
pub async fn anilist_login(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Viewer> {
    let client_id = state
        .auth
        .client_id()
        .ok_or_else(|| "Set your AniList client ID in Settings first.".to_string())?;

    // Start the loopback capture before opening the browser so the port is
    // bound and listening by the time AniList redirects back.
    let capture = tauri::async_runtime::spawn(auth::run_loopback_capture(Duration::from_secs(300)));

    let url = auth::authorize_url(&client_id);
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("could not open browser: {e}"))?;

    let captured = capture
        .await
        .map_err(|e| format!("login task failed: {e}"))??;
    state
        .auth
        .save_token(captured.access_token, captured.expires_at);

    let token = state
        .auth
        .valid_token()
        .ok_or_else(|| "token missing after login".to_string())?;
    let viewer = state.anilist.viewer(&token).await.map_err(map_err)?;
    state.auth.save_viewer(viewer.clone());

    // Initial pull in the background so the command returns promptly.
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = crate::sync::pull(&app2).await;
    });
    Ok(viewer)
}

#[tauri::command]
pub fn anilist_logout(state: State<'_, AppState>) -> CmdResult<()> {
    state.auth.logout();
    // Drop pending pushes; they belong to the account that just logged out.
    let _ = state.db.clear_sync_queue();
    Ok(())
}

/// Force a full pull now (Library "refresh" / re-login recovery).
#[tauri::command]
pub async fn anilist_sync_now(app: AppHandle) -> CmdResult<()> {
    crate::sync::pull(&app).await
}

#[tauri::command]
pub fn get_library(state: State<'_, AppState>) -> CmdResult<Vec<LibraryItem>> {
    state.db.library().map_err(map_err)
}

/// Apply a user edit to a list entry (status/progress/score): write locally,
/// mark dirty, and enqueue a push. Works offline (queued for later).
#[tauri::command]
pub fn set_list_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    anilist_id: i64,
    status: String,
    progress: i64,
    score: Option<f64>,
) -> CmdResult<ListEntry> {
    let status = MediaListStatus::parse(&status)
        .ok_or_else(|| format!("invalid status: {status}"))?;
    let entry = state
        .db
        .set_list_entry_local(anilist_id, status, progress, score)
        .map_err(map_err)?;
    crate::sync::push_local(&state, &app, &entry);
    Ok(entry)
}

#[derive(Serialize)]
pub struct AnimeListState {
    pub anilist_id: Option<i64>,
    pub entry: Option<ListEntry>,
    pub episode_count: Option<i64>,
}

/// The list status/progress for a provider show, resolving (and persisting) its
/// AniList mapping if not already known. `anilist_hint` is the provider-carried
/// aniListId (exact, free) when available; otherwise we fall back to an AniList
/// title search + the matching heuristic.
#[tauri::command]
pub async fn get_anime_list_state(
    state: State<'_, AppState>,
    provider_id: String,
    title: String,
    episodes: Option<u32>,
    anilist_hint: Option<i64>,
) -> CmdResult<AnimeListState> {
    let anilist_id =
        resolve_mapping(&state, &provider_id, &title, episodes, anilist_hint).await;
    let entry = anilist_id.and_then(|id| state.db.get_list_entry(id).ok().flatten());
    let episode_count = anilist_id.and_then(|id| state.db.media_episode_count(id).ok().flatten());
    Ok(AnimeListState {
        anilist_id,
        entry,
        episode_count,
    })
}

/// Manual AniList search for the "wrong match?" affordance on the detail page.
#[tauri::command]
pub async fn search_anilist(state: State<'_, AppState>, query: String) -> CmdResult<Vec<MediaInfo>> {
    state.anilist.search_media(&query).await.map_err(map_err)
}

/// Manually pin a provider show to a specific AniList id (overrides matching).
#[tauri::command]
pub async fn set_anime_mapping(
    state: State<'_, AppState>,
    provider_id: String,
    anilist_id: i64,
) -> CmdResult<()> {
    // Clear any prior mapping on this provider row, then link + cache media.
    state
        .db
        .link_provider_anilist(&provider_id, anilist_id)
        .map_err(map_err)?;
    if let Ok(Some(m)) = state.anilist.media_by_id(anilist_id).await {
        let _ = state.db.upsert_media(
            m.anilist_id,
            m.title_romaji.as_deref(),
            m.title_english.as_deref(),
            m.cover_url.as_deref(),
            m.episode_count,
            m.format.as_deref(),
        );
    }
    Ok(())
}

/// Resolve a provider show to an AniList id, persisting the mapping + media
/// metadata. Order: existing mapping → provider aniListId hint → title search.
async fn resolve_mapping(
    state: &AppState,
    provider_id: &str,
    title: &str,
    episodes: Option<u32>,
    anilist_hint: Option<i64>,
) -> Option<i64> {
    if let Ok(Some(id)) = state.db.anilist_id_for_provider(provider_id) {
        return Some(id);
    }
    // Exact, free mapping carried by allanime.
    if let Some(hint) = anilist_hint {
        let _ = state.db.link_provider_anilist(provider_id, hint);
        if let Ok(Some(m)) = state.anilist.media_by_id(hint).await {
            let _ = state.db.upsert_media(
                m.anilist_id,
                m.title_romaji.as_deref(),
                m.title_english.as_deref(),
                m.cover_url.as_deref(),
                m.episode_count,
                m.format.as_deref(),
            );
        }
        return Some(hint);
    }
    // Fallback: AniList title search + heuristic.
    let candidates = state.anilist.search_media(title).await.ok()?;
    let m = best_match(title, episodes, &candidates)?;
    let _ = state.db.link_provider_anilist(provider_id, m.anilist_id);
    let _ = state.db.upsert_media(
        m.anilist_id,
        m.title_romaji.as_deref(),
        m.title_english.as_deref(),
        m.cover_url.as_deref(),
        m.episode_count,
        m.format.as_deref(),
    );
    Some(m.anilist_id)
}

// ---------------------------------------------------------------------------
// Downloads (M3)
// ---------------------------------------------------------------------------

/// Enqueue one or more episodes of a show (single / range / all). Returns how
/// many were actually enqueued (episodes already downloaded/queued skip).
#[tauri::command]
pub fn enqueue_downloads(
    state: State<'_, AppState>,
    anime_id: String,
    episodes: Vec<String>,
    quality: Option<String>,
    dub: bool,
) -> CmdResult<u32> {
    let mut n = 0;
    for ep in &episodes {
        if state
            .downloads
            .enqueue(&anime_id, ep, quality.as_deref(), dub)
            .map_err(map_err)?
            .is_some()
        {
            n += 1;
        }
    }
    Ok(n)
}

#[tauri::command]
pub fn list_downloads(state: State<'_, AppState>) -> CmdResult<Vec<DownloadRow>> {
    state.db.list_downloads().map_err(map_err)
}

#[tauri::command]
pub fn downloads_for_anime(
    state: State<'_, AppState>,
    anime_id: String,
) -> CmdResult<Vec<DownloadRow>> {
    state.db.downloads_for_anime(&anime_id).map_err(map_err)
}

#[tauri::command]
pub fn pause_download(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.downloads.pause(id).map_err(map_err)
}

#[tauri::command]
pub fn resume_download(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.downloads.resume(id).map_err(map_err)
}

/// Cancel an active/queued download, or delete a completed/failed one.
/// Removes the row and any files on disk.
#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.downloads.remove(id).map_err(map_err)
}

/// Delete all completed downloads of one show. Returns how many were removed.
#[tauri::command]
pub fn delete_anime_downloads(state: State<'_, AppState>, anime_id: String) -> CmdResult<usize> {
    state.downloads.remove_anime_completed(&anime_id).map_err(map_err)
}

/// Bulk cleanup: delete every completed download. Returns how many.
#[tauri::command]
pub fn delete_completed_downloads(state: State<'_, AppState>) -> CmdResult<usize> {
    state.downloads.remove_all_completed().map_err(map_err)
}

#[derive(Serialize)]
pub struct DownloadStorage {
    pub per_anime: Vec<AnimeStorage>,
    pub total_bytes: i64,
}

#[tauri::command]
pub fn download_storage(state: State<'_, AppState>) -> CmdResult<DownloadStorage> {
    Ok(DownloadStorage {
        per_anime: state.db.download_storage().map_err(map_err)?,
        total_bytes: state.db.download_total_bytes().map_err(map_err)?,
    })
}

/// Offline playback info for one episode: present only when a completed
/// download (with its manifest) exists. The UI builds player URLs as
/// `<media_base>/dl/<dir>/<file>`.
#[derive(Serialize)]
pub struct OfflineInfo {
    pub dir: String,
    pub kind: StreamKind,
    pub quality: String,
    pub video: String,
    pub subtitles: Vec<downloads_core::ManifestSub>,
}

#[tauri::command]
pub fn get_offline_info(
    state: State<'_, AppState>,
    anime_id: String,
    episode: String,
) -> CmdResult<Option<OfflineInfo>> {
    let Some(row) = state
        .db
        .download_for_episode(&anime_id, &episode)
        .map_err(map_err)?
    else {
        return Ok(None);
    };
    if row.state != DownloadState::Done {
        return Ok(None);
    }
    let Some(dir) = row.dir_path else { return Ok(None) };
    let Some(manifest) = downloads_core::read_manifest(&state.downloads_root, &dir) else {
        return Ok(None);
    };
    Ok(Some(OfflineInfo {
        dir,
        kind: manifest.kind,
        quality: manifest.quality,
        video: manifest.video,
        subtitles: manifest.subtitles,
    }))
}

// ---------------------------------------------------------------------------
// Home page + airing tracker & notification inbox (M3.5)
// ---------------------------------------------------------------------------

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The three AniList home rows plus cache bookkeeping so the UI can render
/// instantly from cache and decide whether to refresh in the background.
#[derive(Serialize)]
pub struct HomePayload {
    pub sections: HomeSections,
    pub fetched_at: i64,
    /// False when past the 6h TTL — the UI should kick a background refresh.
    pub fresh: bool,
}

const HOME_CACHE_SECTION: &str = "home"; // one blob for all three rows

/// Cached home rows (instant, offline-tolerant). `None` when never fetched.
#[tauri::command]
pub fn get_home_cached(state: State<'_, AppState>) -> CmdResult<Option<HomePayload>> {
    let Some((json, fetched_at)) = state.db.get_home_cache(HOME_CACHE_SECTION).map_err(map_err)?
    else {
        return Ok(None);
    };
    let sections: HomeSections = serde_json::from_str(&json).map_err(map_err)?;
    Ok(Some(HomePayload {
        sections,
        fetched_at,
        fresh: is_fresh(fetched_at, now(), HOME_TTL_SECS),
    }))
}

/// Fetch the home rows live (one batched public GraphQL request), cache, and
/// return them. Also warms `media_cache` so home cards resolve titles/covers
/// elsewhere (inbox, library) without extra requests.
#[tauri::command]
pub async fn refresh_home(state: State<'_, AppState>) -> CmdResult<HomePayload> {
    let ((season, year), (next_season, next_year)) = current_and_next_from_unix(now());
    let sections = state
        .anilist
        .home_sections(season, year, next_season, next_year, 16)
        .await
        .map_err(map_err)?;
    for m in sections
        .trending
        .iter()
        .chain(sections.season.iter())
        .chain(sections.next_season.iter())
    {
        let _ = state.db.upsert_media(
            m.anilist_id,
            m.title_romaji.as_deref(),
            m.title_english.as_deref(),
            m.cover_url.as_deref(),
            m.episode_count,
            m.format.as_deref(),
        );
    }
    let json = serde_json::to_string(&sections).map_err(map_err)?;
    state
        .db
        .put_home_cache(HOME_CACHE_SECTION, &json)
        .map_err(map_err)?;
    Ok(HomePayload {
        sections,
        fetched_at: now(),
        fresh: true,
    })
}

/// Continue Watching row: local-only join of CURRENT/REPEATING entries with
/// media metadata, provider mapping, and last watch activity. Works offline.
#[tauri::command]
pub fn get_continue_watching(state: State<'_, AppState>) -> CmdResult<Vec<ContinueWatchingItem>> {
    state.db.continue_watching().map_err(map_err)
}

/// Resolve an AniList id to a provider show for deep-linking (home card /
/// inbox click). Order: existing mapping → provider title-search matched by
/// the provider-carried aniListId (sync matching, reversed). Returns `None`
/// when unresolvable so the UI can fall back to `/search?q=title`.
#[tauri::command]
pub async fn resolve_provider_for_anilist(
    state: State<'_, AppState>,
    anilist_id: i64,
    title: String,
    episodes: Option<u32>,
) -> CmdResult<Option<AnimeSummary>> {
    // 1. Existing mapping: build a summary from the local anime cache.
    if let Ok(Some(provider_id)) = state.db.provider_id_for_anilist(anilist_id) {
        let cached = state.db.get_cached_anime(&provider_id).ok().flatten();
        let (title_romaji, title_english, cover_url) =
            cached.unwrap_or((title.clone(), None, None));
        return Ok(Some(AnimeSummary {
            provider_id,
            title: title_romaji,
            title_english,
            cover_url,
            available_episodes: episodes.unwrap_or(0),
            anilist_id: Some(anilist_id),
        }));
    }

    // 2. Provider title search, matched by carried aniListId (else title
    //    similarity). Persist the discovered mapping for next time.
    let results = state
        .provider
        .search(&title, TranslationType::Sub)
        .await
        .map_err(map_err)?;
    for r in &results {
        let _ = state.db.cache_anime(
            &r.provider_id,
            &r.title,
            r.title_english.as_deref(),
            r.cover_url.as_deref(),
            Some(r.available_episodes),
        );
    }
    let Some(m) = best_provider_match(anilist_id, &title, episodes, &results) else {
        return Ok(None);
    };
    let _ = state.db.link_provider_anilist(&m.provider_id, anilist_id);
    Ok(Some(m.clone()))
}

/// Inbox: fired episode notifications, newest first.
#[tauri::command]
pub fn get_notifications(state: State<'_, AppState>) -> CmdResult<Vec<Notification>> {
    state.db.notifications().map_err(map_err)
}

/// Inbox: upcoming airings (tracked shows with a known next episode), soonest
/// first. `kind` is "upcoming" and `read` is always true.
#[tauri::command]
pub fn get_upcoming(state: State<'_, AppState>) -> CmdResult<Vec<Notification>> {
    state.db.upcoming().map_err(map_err)
}

#[tauri::command]
pub fn unread_notifications(state: State<'_, AppState>) -> CmdResult<i64> {
    state.db.unread_notification_count().map_err(map_err)
}

#[tauri::command]
pub fn mark_notifications_read(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.db.mark_all_notifications_read().map_err(map_err)?;
    let _ = app.emit("notify:read", ());
    Ok(())
}

#[tauri::command]
pub fn clear_notifications(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.db.clear_notifications().map_err(map_err)?;
    let _ = app.emit("notify:read", ());
    Ok(())
}

/// Force an airing refresh now (Settings toggle change / inbox pull-to-refresh).
#[tauri::command]
pub async fn airing_refresh_now(app: AppHandle) -> CmdResult<()> {
    crate::airing::refresh(&app).await;
    Ok(())
}

/// Whether PLANNING entries are also tracked for airing notifications.
#[tauri::command]
pub fn get_notify_planning(state: State<'_, AppState>) -> CmdResult<bool> {
    state
        .db
        .get_bool_setting(crate::airing::NOTIFY_PLANNING_KEY, false)
        .map_err(map_err)
}

/// Persist the PLANNING toggle and re-run the airing refresh so the tracked
/// set updates immediately.
#[tauri::command]
pub async fn set_notify_planning(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<()> {
    state
        .db
        .set_bool_setting(crate::airing::NOTIFY_PLANNING_KEY, enabled)
        .map_err(map_err)?;
    crate::airing::refresh(&app).await;
    Ok(())
}
