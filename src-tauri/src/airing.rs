//! Airing tracker worker: keeps the `airing` table current for tracked shows
//! and fires episode-released notifications into the inbox.
//!
//! Same shape as `sync.rs`: all decisions (what to track/untrack, what is due)
//! are pure functions in `anidoku_core::airing`; this module owns the async
//! plumbing — the batched AniList fetch, DB writes, `notify:new` events, and
//! the OS notification via tauri-plugin-notification.
//!
//! Cadence:
//! - `refresh` on startup, after each list pull (sync.rs calls it), and every 6h.
//! - a 60s ticker runs `fire_due`, which moves `airing_at <= now` rows into
//!   `notifications` (de-duped on (anilist_id, episode) by the DB) and advances
//!   or drops the airing row. Airings missed while the app was closed are
//!   caught by the first tick after startup.

use crate::AppState;
use anidoku_core::airing::{due_notifications, plan_airing_refresh};
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// Settings key: also track PLANNING entries (default off).
pub const NOTIFY_PLANNING_KEY: &str = "notify_planning";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Payload for `notify:new` — one fired episode notification.
#[derive(Clone, Serialize)]
pub struct NotifyNew {
    pub anilist_id: i64,
    pub episode: i64,
    pub title: Option<String>,
    pub unread: i64,
}

/// Refresh the airing table for all tracked list entries: batched
/// `media(id_in:) { nextAiringEpisode }` (public, no auth), 50 ids per call.
/// Tracks releasing shows, untracks FINISHED/CANCELLED/no-next-episode, and
/// drops rows for shows that left the tracked statuses.
pub async fn refresh(app: &AppHandle) {
    // Fire stored rows that came due since the last tick BEFORE overwriting
    // them with live data: an episode that aired while the app was closed is
    // gone from AniList's nextAiringEpisode (it already points at the episode
    // after), so the stored row is the only evidence it aired. This is the
    // "missed while closed" catch-up — it must precede the upserts.
    fire_due(app).await;

    let state = app.state::<AppState>();
    let include_planning = state
        .db
        .get_bool_setting(NOTIFY_PLANNING_KEY, false)
        .unwrap_or(false);
    let ids = match state.db.tracked_anilist_ids(include_planning) {
        Ok(ids) => ids,
        Err(_) => return,
    };

    // Shows that stopped being tracked (left Watching, toggle turned off, ...)
    // are removed even when `ids` is empty.
    if let Ok(existing) = state.db.all_airing() {
        for row in existing {
            if !ids.contains(&row.anilist_id) {
                let _ = state.db.delete_airing(row.anilist_id);
            }
        }
    }
    if ids.is_empty() {
        return;
    }

    for chunk in ids.chunks(50) {
        let fetched = match state.anilist.airing_for(chunk).await {
            Ok(f) => f,
            Err(e) => {
                eprintln!("airing: refresh fetch failed: {e}");
                return; // network problem — the 6h/next-pull retry covers it
            }
        };
        let plan = plan_airing_refresh(&fetched, now());
        for row in &plan.upserts {
            let _ = state.db.upsert_airing(row);
        }
        for id in &plan.untrack {
            let _ = state.db.delete_airing(*id);
        }
    }

    // A refresh can surface an airing that already elapsed (e.g. list pull for
    // a show whose episode aired an hour ago) — fire immediately, don't wait
    // for the next tick.
    fire_due(app).await;
    let _ = app.emit("airing:updated", ());
}

/// Move due airings (airing_at <= now) into `notifications`, notify the UI and
/// the OS, and advance the airing row (next tick's refresh will fetch the real
/// next episode; meanwhile we clear airing_at so the row can't re-fire).
pub async fn fire_due(app: &AppHandle) {
    let state = app.state::<AppState>();
    let rows = match state.db.all_airing() {
        Ok(r) => r,
        Err(_) => return,
    };
    let due = due_notifications(&rows, now());
    if due.is_empty() {
        return;
    }

    let mut fired_any = false;
    for d in due {
        // De-dupe on (anilist_id, episode): INSERT OR IGNORE returns false when
        // this episode already notified (e.g. row seen by startup catch-up AND
        // a later refresh).
        let inserted = state
            .db
            .insert_notification(d.anilist_id, d.episode, Some(d.airing_at), "episode")
            .unwrap_or(false);
        // Either way the airing row must not keep re-firing: blank its airing
        // fields until the next refresh replaces them with the real next ep.
        let _ = state.db.upsert_airing(&anidoku_core::models::AiringRow {
            anilist_id: d.anilist_id,
            next_episode: None,
            airing_at: None,
            media_status: rows
                .iter()
                .find(|r| r.anilist_id == d.anilist_id)
                .and_then(|r| r.media_status.clone()),
            refreshed_at: now(),
        });
        if !inserted {
            continue;
        }
        fired_any = true;

        let title = title_of(&state, d.anilist_id);
        let unread = state.db.unread_notification_count().unwrap_or(0);
        let _ = app.emit(
            "notify:new",
            NotifyNew {
                anilist_id: d.anilist_id,
                episode: d.episode,
                title: title.clone(),
                unread,
            },
        );

        // OS notification (best-effort; macOS may require the user to allow
        // notifications for the app the first time).
        let body = format!(
            "Episode {} of {} is out",
            d.episode,
            title.unwrap_or_else(|| format!("AniList #{}", d.anilist_id))
        );
        if let Err(e) = app
            .notification()
            .builder()
            .title("New episode")
            .body(&body)
            .show()
        {
            eprintln!("airing: OS notification failed (permission?): {e}");
        }
    }

    if fired_any {
        // Ask AniList for the next airing of the shows we just fired, so the
        // Upcoming list immediately shows the following episode.
        let _ = app.emit("airing:updated", ());
    }
}

fn title_of(state: &AppState, anilist_id: i64) -> Option<String> {
    state
        .db
        .notifications()
        .ok()?
        .iter()
        .find(|n| n.anilist_id == anilist_id)
        .and_then(|n| n.title_english.clone().or_else(|| n.title_romaji.clone()))
}

/// Spawn the tracker: one refresh at startup (which also catches airings missed
/// while the app was closed), then a 60s due-check ticker and a 6h refresh.
pub fn spawn_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Give the webview a moment to register event listeners (same grace
        // period as the sync worker).
        tokio::time::sleep(Duration::from_secs(3)).await;
        refresh(&app).await;

        let mut ticks: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            ticks += 1;
            fire_due(&app).await;
            if ticks.is_multiple_of(360) {
                refresh(&app).await; // every 6h
            }
        }
    });
}
