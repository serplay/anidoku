//! AniDoku desktop shell: exposes the core provider engine and local store to
//! the Svelte UI over Tauri IPC, and registers the `stream://` proxy protocol
//! that lets the webview player fetch referer-gated media.

mod airing;
#[cfg(target_os = "android")]
mod android;
mod auth;
mod commands;
mod stream;
mod sync;

use anidoku_core::anilist::AniListClient;
use anidoku_core::db::Database;
use anidoku_core::downloads::{DownloadEvent, DownloadManager};
use anidoku_core::media_server;
use anidoku_core::provider::allanime::AllAnime;
use anidoku_core::proxy::ProxyClient;
use auth::AuthStore;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Emitter;

/// Shared application state, injected into every command.
pub struct AppState {
    pub provider: Arc<AllAnime>,
    pub db: Arc<Database>,
    pub proxy: Arc<ProxyClient>,
    /// Base URL of the loopback media server, e.g. `http://127.0.0.1:52123`.
    /// The UI fetches this once and routes all playback (MP4/HLS/subtitles)
    /// through it so Range/Referer are handled correctly. Downloaded episodes
    /// are served under `<media_base>/dl/<anime>/<ep>/...`.
    pub media_base: String,
    /// AniList GraphQL client (M2 sync).
    pub anilist: AniListClient,
    /// OAuth token + client-id storage (app-data file, 0600).
    pub auth: AuthStore,
    /// Offline download engine (M3).
    pub downloads: Arc<DownloadManager>,
    /// Root of the downloads tree (app-data `downloads/`).
    pub downloads_root: PathBuf,
}

/// Platform data root. On Android `dirs::data_dir()` is `None` and the
/// `temp_dir()` fallback lands in the app's **cache** dir, which the OS may
/// clear under storage pressure — wiping the DB and every download. Persist
/// next door in `files/` instead (TMPDIR is `<app root>/cache`).
#[cfg(target_os = "android")]
fn base_data_dir() -> PathBuf {
    let tmp = std::env::temp_dir();
    let dir = match tmp.parent() {
        Some(app_root) => app_root.join("files"),
        None => tmp.clone(),
    };
    // One-time migration for installs that wrote into cache/ before this fix.
    let old = tmp.join("AniDoku");
    if old.is_dir() && !dir.join("AniDoku").exists() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::rename(&old, dir.join("AniDoku"));
    }
    dir
}

#[cfg(not(target_os = "android"))]
fn base_data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir)
}

impl AppState {
    fn new(download_events: tokio::sync::mpsc::UnboundedSender<DownloadEvent>) -> Self {
        let data_dir = base_data_dir().join("AniDoku");
        let db_path = data_dir.join("anidoku.db");
        let db = Arc::new(Database::open(&db_path).expect("open database"));
        let proxy = Arc::new(ProxyClient::new());
        let auth = AuthStore::load(&data_dir);
        let provider = Arc::new(AllAnime::new());
        let downloads_root = data_dir.join("downloads");
        std::fs::create_dir_all(&downloads_root).expect("create downloads dir");

        // Start the loopback media server before the webview loads. Tauri's
        // async runtime is Tokio, so we can block on the bind here. It also
        // serves the downloads tree under /dl/ for offline playback.
        let media = tauri::async_runtime::block_on(media_server::spawn(
            proxy.clone(),
            Some(downloads_root.clone()),
        ))
        .expect("start media server");
        eprintln!("media server listening on {}", media.base);

        let downloads = DownloadManager::new(
            db.clone(),
            proxy.clone(),
            provider.clone(),
            downloads_root.clone(),
            download_events,
        );

        AppState {
            provider,
            db,
            proxy,
            media_base: media.base,
            anilist: AniListClient::new(),
            auth,
            downloads,
            downloads_root,
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (dl_tx, mut dl_rx) = tokio::sync::mpsc::unbounded_channel::<DownloadEvent>();
    let state = AppState::new(dl_tx);
    let proxy = state.proxy.clone();
    let downloads = state.downloads.clone();
    #[cfg(target_os = "android")]
    let dl_db = state.db.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        // `stream://` now serves cover images only (referer-gated thumbnails);
        // all playback media goes through the loopback HTTP media server.
        .register_asynchronous_uri_scheme_protocol("stream", move |_ctx, request, responder| {
            stream::handle(proxy.clone(), request, responder);
        })
        .setup(move |app| {
            // Background AniList sync worker: startup pull + periodic drain/pull.
            // No-ops while logged out, so it is always safe to spawn.
            sync::spawn_worker(app.handle().clone());

            // Airing tracker: startup refresh (catches airings missed while
            // closed), 60s due-check ticker, 6h re-fetch. Public GraphQL, so
            // it works logged-out too (tracked set is just empty then).
            airing::spawn_worker(app.handle().clone());

            // Download engine: recover in-flight rows, start the scheduler,
            // and forward engine events to the UI (same pattern as sync.rs).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                downloads.start();
                // Android: hold a foreground service while rows are queued or
                // downloading, so the OS doesn't kill the engine off-screen.
                #[cfg(target_os = "android")]
                let mut fg_active = false;
                while let Some(ev) = dl_rx.recv().await {
                    match ev {
                        DownloadEvent::Progress(p) => {
                            let _ = handle.emit("download:progress", p);
                        }
                        DownloadEvent::State(s) => {
                            let _ = handle.emit("download:state", s);
                            #[cfg(target_os = "android")]
                            {
                                let active = dl_db.count_active_downloads().unwrap_or(0) > 0;
                                if active != fg_active {
                                    fg_active = active;
                                    android::set_download_service_active(&handle, active);
                                }
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::search_anime,
            commands::get_episodes,
            commands::get_sources,
            commands::get_watch_state,
            commands::set_watch_state,
            commands::list_watch_states,
            commands::convert_subtitles,
            commands::media_base,
            commands::get_settings,
            commands::set_client_id,
            commands::anilist_status,
            commands::anilist_login,
            commands::anilist_logout,
            commands::anilist_sync_now,
            commands::get_library,
            commands::set_list_entry,
            commands::get_anime_list_state,
            commands::search_anilist,
            commands::get_media_overview,
            commands::search_catalog,
            commands::get_media_tags,
            commands::set_anime_mapping,
            commands::enqueue_downloads,
            commands::list_downloads,
            commands::downloads_for_anime,
            commands::pause_download,
            commands::resume_download,
            commands::cancel_download,
            commands::delete_anime_downloads,
            commands::delete_completed_downloads,
            commands::download_storage,
            commands::get_offline_info,
            commands::get_home_cached,
            commands::refresh_home,
            commands::get_continue_watching,
            commands::resolve_provider_for_anilist,
            commands::check_availability,
            commands::get_notifications,
            commands::get_upcoming,
            commands::unread_notifications,
            commands::mark_notifications_read,
            commands::clear_notifications,
            commands::airing_refresh_now,
            commands::get_notify_planning,
            commands::set_notify_planning,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AniDoku")
}
