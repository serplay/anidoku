//! AniDoku desktop shell: exposes the core provider engine and local store to
//! the Svelte UI over Tauri IPC, and registers the `stream://` proxy protocol
//! that lets the webview player fetch referer-gated media.

mod commands;
mod stream;

use anidoku_core::db::Database;
use anidoku_core::provider::allanime::AllAnime;
use anidoku_core::proxy::ProxyClient;
use std::sync::Arc;

/// Shared application state, injected into every command.
pub struct AppState {
    pub provider: AllAnime,
    pub db: Database,
    pub proxy: Arc<ProxyClient>,
}

impl AppState {
    fn new() -> Self {
        let db_path = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("AniDoku")
            .join("anidoku.db");
        let db = Database::open(&db_path).expect("open database");
        AppState {
            provider: AllAnime::new(),
            db,
            proxy: Arc::new(ProxyClient::new()),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running AniDoku");
}
