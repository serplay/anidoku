//! Tauri IPC commands. Thin wrappers that translate between the UI and the
//! core crate; all real logic lives in `anidoku-core`.

use crate::{auth, AppState};
use anidoku_core::airing::{is_fresh, HOME_TTL_SECS};
use anidoku_core::anilist::{current_and_next_from_unix, CatalogSearch};
use anidoku_core::downloads as downloads_core;
use anidoku_core::models::{
    AnimeStorage, AnimeSummary, CatalogPage, ContinueWatchingItem, DownloadRow, DownloadState,
    HomeSections, LibraryItem, ListEntry, MediaInfo, MediaListStatus, MediaOverview, MediaTag,
    Notification, StreamKind, TranslationType, VideoSource, Viewer, WatchState,
};
use anidoku_core::provider::{aggregate, SourceStatus};
use anidoku_core::sync::best_match;
use serde::Serialize;
use std::time::Duration;
#[cfg(target_os = "ios")]
use tauri::Manager as _;
use tauri::{AppHandle, Emitter, State};
#[cfg(not(target_os = "ios"))]
use tauri_plugin_opener::OpenerExt;

/// Navigate the given webview from the main thread — wry on iOS requires
/// UIKit calls to happen there.
#[cfg(target_os = "ios")]
fn navigate_on_main_thread(webview: &tauri::WebviewWindow, url: tauri::Url) -> CmdResult<()> {
    let w = webview.clone();
    webview
        .run_on_main_thread(move || {
            let _ = w.navigate(url);
        })
        .map_err(map_err)
}

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
    let mode = if dub {
        TranslationType::Dub
    } else {
        TranslationType::Sub
    };
    // Aggregated across every enabled source; a broken source is skipped
    // rather than failing the search.
    let results = aggregate::search_all(&state.sources, &state.db, &query, mode).await;
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
    let mode = if dub {
        TranslationType::Dub
    } else {
        TranslationType::Sub
    };
    aggregate::episodes(&state.sources, &state.db, &show_id, mode)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn get_sources(
    state: State<'_, AppState>,
    show_id: String,
    episode: String,
    dub: bool,
) -> CmdResult<Vec<VideoSource>> {
    let mode = if dub {
        TranslationType::Dub
    } else {
        TranslationType::Sub
    };
    aggregate::sources_for(&state.sources, &state.db, &show_id, &episode, mode)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub fn get_watch_state(
    state: State<'_, AppState>,
    anime_id: String,
    episode: String,
) -> CmdResult<Option<WatchState>> {
    state
        .db
        .get_watch_state(&anime_id, &episode)
        .map_err(map_err)
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
    ep.parse::<f64>()
        .ok()
        .map(|f| f.floor() as i64)
        .filter(|n| *n >= 1)
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
    /// One row per registered source: build id, config origin, and whether the
    /// user has it enabled.
    pub sources: Vec<SourceSetting>,
}

/// A source as Settings shows it.
#[derive(Serialize)]
pub struct SourceSetting {
    #[serde(flatten)]
    pub status: SourceStatus,
    pub enabled: bool,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    let enabled = state.sources.enabled_ordered(&state.db);
    // Ordered as the user sees them: enabled sources in failover order, then
    // the disabled ones.
    let mut rows: Vec<SourceSetting> = enabled
        .iter()
        .map(|p| SourceSetting {
            status: p.status(),
            enabled: true,
        })
        .collect();
    for p in state.sources.all() {
        if !enabled.iter().any(|e| e.id() == p.id()) {
            rows.push(SourceSetting {
                status: p.status(),
                enabled: false,
            });
        }
    }
    Ok(Settings {
        client_id: state.auth.client_id(),
        redirect_url: auth::REDIRECT_URL.to_string(),
        sources: rows,
    })
}

/// Reorder the failover chain (ids best-first).
#[tauri::command]
pub fn set_source_order(state: State<'_, AppState>, order: Vec<String>) -> CmdResult<()> {
    state.sources.set_order(&state.db, &order).map_err(map_err)
}

/// Enable or disable one source.
#[tauri::command]
pub fn set_source_enabled(
    state: State<'_, AppState>,
    source: String,
    enabled: bool,
) -> CmdResult<()> {
    state
        .sources
        .set_enabled(&state.db, &source, enabled)
        .map_err(map_err)
}

/// Pin which source a show plays from (the watch page's manual pick).
///
/// Takes the namespaced show id the user is actually looking at rather than an
/// AniList id, because the watch page has the former and not the latter. A
/// show with no AniList mapping yet simply has no preference to store.
#[tauri::command]
pub fn set_preferred_source(
    state: State<'_, AppState>,
    show_id: String,
    source: String,
) -> CmdResult<()> {
    let Some(anilist_id) = state
        .db
        .anilist_id_for_provider(&show_id)
        .map_err(map_err)?
    else {
        return Ok(());
    };
    state
        .db
        .set_preferred_source(anilist_id, &source)
        .map_err(map_err)
}

#[derive(Serialize)]
pub struct ProviderRefresh {
    /// True when a newer remote config was fetched and applied for any source.
    pub changed: bool,
    pub build_id: String,
    pub config_source: String,
}

/// Force-fetch remote config (the "Check for fix" button when a source has
/// rotated and the user doesn't want to wait for the next automatic self-heal).
/// With no `source`, every registered source is refreshed.
#[tauri::command]
pub async fn refresh_provider_config(
    state: State<'_, AppState>,
    source: Option<String>,
) -> CmdResult<ProviderRefresh> {
    let targets: Vec<_> = match &source {
        Some(id) => state.sources.get(id).into_iter().collect(),
        None => state.sources.all().to_vec(),
    };
    let mut changed = false;
    for p in &targets {
        changed |= p.refresh_config(true).await;
    }
    let status = targets
        .first()
        .map(|p| p.status())
        .unwrap_or_else(|| SourceStatus::r#static("", ""));
    Ok(ProviderRefresh {
        changed,
        build_id: status.build_id,
        config_source: status.config_source.to_string(),
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
///
/// iOS is the exception: an external browser backgrounds the app and iOS
/// suspends it within seconds, killing the loopback listener before AniList
/// can redirect back. There the auth page is shown in the app's own webview
/// (the app stays foreground, the listener stays alive) and the webview is
/// navigated back to the app when the capture finishes. That reload drops
/// this command's reply; the UI recovers the signed-in state from
/// `anilist_status` on boot.
#[tauri::command]
pub async fn anilist_login(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Viewer> {
    let client_id = state
        .auth
        .client_id()
        .ok_or_else(|| "Set your AniList client ID in Settings first.".to_string())?;

    // Supersede any prior in-flight capture: a re-tapped Sign in (common when
    // the iOS webview hop flakes) must abort the previous attempt so it releases
    // the fixed loopback port before this one binds it. Without this the second
    // login fails with "cannot bind loopback port" while the first still listens.
    if let Some(prev) = state.oauth_abort.lock().unwrap().take() {
        prev.abort();
    }

    // Start the loopback capture before opening the browser so the port is
    // bound and listening by the time AniList redirects back. Spawn on tokio
    // directly so we can hold an abort handle for the single-flight guard.
    let capture = tokio::spawn(auth::run_loopback_capture(Duration::from_secs(300)));
    *state.oauth_abort.lock().unwrap() = Some(capture.abort_handle());

    let url = auth::authorize_url(&client_id);

    #[cfg(target_os = "ios")]
    let return_url = {
        let webview = app
            .get_webview_window("main")
            .ok_or_else(|| "main window missing".to_string())?;
        let return_url = webview.url().map_err(map_err)?;
        let auth_url = tauri::Url::parse(&url).map_err(map_err)?;
        navigate_on_main_thread(&webview, auth_url)?;
        return_url
    };

    #[cfg(not(target_os = "ios"))]
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("could not open browser: {e}"))?;

    let captured = capture
        .await
        .map_err(|e| format!("login task failed: {e}"))?;

    // Leave the auth page whether login succeeded or not.
    #[cfg(target_os = "ios")]
    if let Some(webview) = app.get_webview_window("main") {
        let _ = navigate_on_main_thread(&webview, return_url);
    }

    let captured = captured?;
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
    let status =
        MediaListStatus::parse(&status).ok_or_else(|| format!("invalid status: {status}"))?;
    let entry = state
        .db
        .set_list_entry_local(anilist_id, status, progress, score)
        .map_err(map_err)?;
    crate::sync::push_local(&state, &app, &entry);
    // An entry created in-app may have no media_cache row yet, which leaves
    // "AniList #id" placeholders on home/library cards. Warm it best-effort.
    warm_media_cache(&state, anilist_id);
    Ok(entry)
}

/// Backfill `media_cache` (title/cover/…) for an AniList id in the background
/// when it's missing. Best-effort: failures just leave the placeholder until
/// the next sync pull warms the row.
fn warm_media_cache(state: &AppState, anilist_id: i64) {
    if !state.db.media_title_missing(anilist_id).unwrap_or(false) {
        return;
    }
    let anilist = state.anilist.clone();
    let db = state.db.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(Some(m)) = anilist.media_by_id(anilist_id).await {
            let _ = db.upsert_media(
                anilist_id,
                m.title_romaji.as_deref(),
                m.title_english.as_deref(),
                m.cover_url.as_deref(),
                m.episode_count,
                m.format.as_deref(),
            );
        }
    });
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
    let anilist_id = resolve_mapping(&state, &provider_id, &title, episodes, anilist_hint).await;
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
pub async fn search_anilist(
    state: State<'_, AppState>,
    query: String,
) -> CmdResult<Vec<MediaInfo>> {
    state.anilist.search_media(&query).await.map_err(map_err)
}

/// Synopsis + meta for the detail page, fetched once the AniList mapping is
/// known. Public query, no auth needed.
#[tauri::command]
pub async fn get_media_overview(
    state: State<'_, AppState>,
    anilist_id: i64,
) -> CmdResult<Option<MediaOverview>> {
    state
        .anilist
        .media_overview(anilist_id)
        .await
        .map_err(map_err)
}

/// AniList catalog search for the reworked /search page. The provider handoff
/// happens later at click time via `resolve_provider_for_anilist`.
#[tauri::command]
pub async fn search_catalog(
    state: State<'_, AppState>,
    filters: CatalogSearch,
) -> CmdResult<CatalogPage> {
    let mut f = filters;
    if f.per_page <= 0 {
        f.per_page = 30;
    }
    if f.page <= 0 {
        f.page = 1;
    }
    state.anilist.search_catalog(&f).await.map_err(map_err)
}

const TAGS_CACHE_KEY: &str = "media_tags";
const TAGS_FETCHED_KEY: &str = "media_tags_fetched_at";
/// Tag list rarely changes; refresh at most weekly.
const TAGS_TTL_SECS: i64 = 7 * 24 * 3600;

/// The AniList media-tag list for the search filter picker. Served from the DB
/// cache (long TTL); on a miss it fetches live and caches. When the cache is
/// present but stale, the cached list is returned immediately and a background
/// refresh is kicked off (kind to the rate limit).
#[tauri::command]
pub async fn get_media_tags(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<Vec<MediaTag>> {
    let cached = state.db.get_setting(TAGS_CACHE_KEY).map_err(map_err)?;
    let fetched_at = state
        .db
        .get_setting(TAGS_FETCHED_KEY)
        .map_err(map_err)?
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    if let Some(json) = cached {
        if let Ok(tags) = serde_json::from_str::<Vec<MediaTag>>(&json) {
            if !is_fresh(fetched_at, now(), TAGS_TTL_SECS) {
                // Stale: refresh in the background, serve the cached list now.
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = refresh_media_tags(&app2).await;
                });
            }
            return Ok(tags);
        }
    }
    // Cache miss (or corrupt): fetch live now.
    refresh_media_tags(&app).await
}

/// Fetch the tag list live and cache it. Shared by the on-miss path and the
/// background staleness refresh.
async fn refresh_media_tags(app: &AppHandle) -> CmdResult<Vec<MediaTag>> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let tags = state.anilist.media_tags().await.map_err(map_err)?;
    if let Ok(json) = serde_json::to_string(&tags) {
        let _ = state.db.set_setting(TAGS_CACHE_KEY, &json);
        let _ = state.db.set_setting(TAGS_FETCHED_KEY, &now().to_string());
    }
    Ok(tags)
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
    state
        .downloads
        .remove_anime_completed(&anime_id)
        .map_err(map_err)
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
    let Some(dir) = row.dir_path else {
        return Ok(None);
    };
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
    let Some((json, fetched_at)) = state
        .db
        .get_home_cache(HOME_CACHE_SECTION)
        .map_err(map_err)?
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
    let rows = state.db.continue_watching().map_err(map_err)?;
    // Self-heal rows whose media_cache never got warmed (entries added before
    // the set_list_entry backfill existed): fetch metadata in the background
    // so the next render shows the real title/cover instead of "AniList #id".
    for r in rows
        .iter()
        .filter(|r| r.title_romaji.is_none() && r.title_english.is_none())
    {
        warm_media_cache(&state, r.anilist_id);
    }
    Ok(rows)
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
    let results =
        aggregate::search_all(&state.sources, &state.db, &title, TranslationType::Sub).await;
    for r in &results {
        let _ = state.db.cache_anime(
            &r.provider_id,
            &r.title,
            r.title_english.as_deref(),
            r.cover_url.as_deref(),
            Some(r.available_episodes),
        );
    }
    // Links every source that has the show (not just the one that opens), so
    // failover has somewhere to go if this source later breaks.
    Ok(aggregate::link_matches(
        &state.db,
        anilist_id,
        &[&title],
        episodes,
        &results,
    ))
}

/// Re-check a negative availability result after this many seconds (7 days).
/// Positive results never expire — a stored provider mapping already implies
/// the show is streamable.
const AVAILABILITY_NEG_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// Whether an AniList title resolves to a streamable provider show, using the
/// same reverse-resolution as `resolve_provider_for_anilist` but WITHOUT
/// navigating. The outcome is cached: positives permanently (a mapping implies
/// available), negatives with a 7-day TTL so repeat searches don't re-hammer the
/// provider. Used to de-emphasise catalog entries the provider doesn't have.
#[tauri::command]
pub async fn check_availability(
    state: State<'_, AppState>,
    anilist_id: i64,
    title: String,
    episodes: Option<u32>,
) -> CmdResult<bool> {
    // 1. An existing provider mapping means it's streamable — permanent yes.
    if let Ok(Some(_)) = state.db.provider_id_for_anilist(anilist_id) {
        mark_available(&state, anilist_id, true);
        return Ok(true);
    }

    // 2. Serve a cached outcome: positives always, negatives within their TTL.
    if let Ok(Some((available, checked_at))) = state.db.get_availability(anilist_id) {
        if available || now() - checked_at < AVAILABILITY_NEG_TTL_SECS {
            return Ok(available);
        }
    }

    // 3. Reverse-resolve against the provider (no navigation). Persist the
    //    discovered mapping so a later click is instant, then cache the outcome.
    let results =
        aggregate::search_all(&state.sources, &state.db, &title, TranslationType::Sub).await;
    for r in &results {
        let _ = state.db.cache_anime(
            &r.provider_id,
            &r.title,
            r.title_english.as_deref(),
            r.cover_url.as_deref(),
            Some(r.available_episodes),
        );
    }
    let available =
        aggregate::link_matches(&state.db, anilist_id, &[&title], episodes, &results).is_some();
    mark_available(&state, anilist_id, available);
    Ok(available)
}

/// Record an availability outcome for every enabled source.
///
/// The check itself is aggregated (it searches all sources at once), so the
/// answer applies to all of them: writing one row per source keeps the cache
/// keyed the way the schema expects while preserving "any source has it" reads.
fn mark_available(state: &State<'_, AppState>, anilist_id: i64, available: bool) {
    for p in state.sources.enabled_ordered(&state.db) {
        let _ = state.db.set_availability(anilist_id, p.id(), available);
    }
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
