//! AniDoku desktop shell: exposes the core provider engine and local store to
//! the Svelte UI over Tauri IPC, and registers the `stream://` proxy protocol
//! that lets the webview player fetch referer-gated media.

mod auth;
mod commands;
mod stream;
mod sync;

use anidoku_core::anilist::AniListClient;
use anidoku_core::db::Database;
use anidoku_core::media_server;
use anidoku_core::provider::allanime::AllAnime;
use anidoku_core::proxy::ProxyClient;
use auth::AuthStore;
use std::sync::Arc;

/// Shared application state, injected into every command.
pub struct AppState {
    pub provider: AllAnime,
    pub db: Database,
    pub proxy: Arc<ProxyClient>,
    /// Base URL of the loopback media server, e.g. `http://127.0.0.1:52123`.
    /// The UI fetches this once and routes all playback (MP4/HLS/subtitles)
    /// through it so Range/Referer are handled correctly.
    pub media_base: String,
    /// AniList GraphQL client (M2 sync).
    pub anilist: AniListClient,
    /// OAuth token + client-id storage (app-data file, 0600).
    pub auth: AuthStore,
}

impl AppState {
    fn new() -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("AniDoku");
        let db_path = data_dir.join("anidoku.db");
        let db = Database::open(&db_path).expect("open database");
        let proxy = Arc::new(ProxyClient::new());
        let auth = AuthStore::load(&data_dir);

        // Start the loopback media server before the webview loads. Tauri's
        // async runtime is Tokio, so we can block on the bind here.
        let media = tauri::async_runtime::block_on(media_server::spawn(proxy.clone()))
            .expect("start media server");
        eprintln!("media server listening on {}", media.base);

        AppState {
            provider: AllAnime::new(),
            db,
            proxy,
            media_base: media.base,
            anilist: AniListClient::new(),
            auth,
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::new();
    let proxy = state.proxy.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        // `stream://` now serves cover images only (referer-gated thumbnails);
        // all playback media goes through the loopback HTTP media server.
        .register_asynchronous_uri_scheme_protocol("stream", move |_ctx, request, responder| {
            stream::handle(proxy.clone(), request, responder);
        })
        .setup(|app| {
            // Background AniList sync worker: startup pull + periodic drain/pull.
            // No-ops while logged out, so it is always safe to spawn.
            sync::spawn_worker(app.handle().clone());
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
            commands::set_anime_mapping,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AniDoku");
}
