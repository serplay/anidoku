//! Tauri IPC commands. Thin wrappers that translate between the UI and the
//! core crate; all real logic lives in `anidoku-core`.

use crate::AppState;
use anidoku_core::models::{AnimeSummary, TranslationType, VideoSource, WatchState};
use anidoku_core::provider::Provider;
use tauri::State;

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
    state: State<'_, AppState>,
    anime_id: String,
    episode: String,
    position_secs: f64,
    duration_secs: Option<f64>,
) -> CmdResult<()> {
    state
        .db
        .set_watch_state(&anime_id, &episode, position_secs, duration_secs)
        .map_err(map_err)
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
