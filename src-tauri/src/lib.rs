//! AniDoku desktop shell: exposes the core provider engine and local store to
//! the Svelte UI over Tauri IPC, and registers the `stream://` proxy protocol
//! that lets the webview player fetch referer-gated media.

mod commands;
mod stream;

use anidoku_core::db::Database;
use anidoku_core::media_server;
use anidoku_core::provider::allanime::AllAnime;
use anidoku_core::proxy::ProxyClient;
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
}

impl AppState {
    fn new() -> Self {
        let db_path = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("AniDoku")
            .join("anidoku.db");
        let db = Database::open(&db_path).expect("open database");
        let proxy = Arc::new(ProxyClient::new());

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
        .invoke_handler(tauri::generate_handler![
            commands::search_anime,
            commands::get_episodes,
            commands::get_sources,
            commands::get_watch_state,
            commands::set_watch_state,
            commands::list_watch_states,
            commands::convert_subtitles,
            commands::media_base,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AniDoku");
}
