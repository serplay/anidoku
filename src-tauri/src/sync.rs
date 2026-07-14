//! Tauri-side AniList sync worker: the async half of the local-first engine
//! whose decisions all come from `anidoku_core::sync` pure functions.
//!
//! - `push_local`  — after a local edit: enqueue a SaveMediaListEntry and kick a drain.
//! - `drain`       — send queued mutations (coalesced, backed-off) to AniList.
//! - `pull`        — pull the remote list, merge per the conflict resolver.
//! - `spawn_worker`— periodic pull + drain loop; also runs once at startup.
//!
//! Everything degrades to local-only when logged out / offline / token expired:
//! a missing-or-expired token short-circuits network work, and an `Unauthorized`
//! from AniList emits `sync:auth-expired` (a re-login prompt) rather than an
//! error storm.

use crate::AppState;
use anidoku_core::anilist::SaveEntry;
use anidoku_core::models::{ListEntry, MediaListStatus};
use anidoku_core::sync::{backoff_secs, merge, plan_drain, QueuedMutation};
use anidoku_core::Error;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn save_entry_of(e: &ListEntry) -> SaveEntry {
    SaveEntry {
        media_id: e.anilist_id,
        status: e.status,
        progress: e.progress,
        score: e.score,
    }
}

#[derive(Clone, Serialize)]
pub struct RemoteOverwrite {
    pub anilist_id: i64,
    pub title: Option<String>,
}

/// Payload for `sync:pushed` / `sync:queued`, so the UI can toast the outcome
/// of a local edit instead of syncing silently.
#[derive(Clone, Serialize)]
pub struct PushOutcome {
    pub anilist_id: i64,
    pub progress: i64,
    pub status: String,
    pub completed: bool,
}

/// Enqueue a mutation for an already-persisted local entry and trigger a drain.
pub fn push_local(state: &AppState, app: &AppHandle, entry: &ListEntry) {
    let save = save_entry_of(entry);
    if let Ok(json) = serde_json::to_string(&save) {
        let _ = state.db.enqueue_mutation(entry.anilist_id, &json);
    }
    let outcome = PushOutcome {
        anilist_id: entry.anilist_id,
        progress: entry.progress,
        status: entry.status.as_str().to_string(),
        completed: entry.status == MediaListStatus::Completed,
    };
    // Drain now (no-ops when offline/logged out), then report whether the
    // mutation actually left the queue so the UI can say "synced" vs "queued".
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        drain(&app).await;
        let state = app.state::<AppState>();
        let still_queued = state
            .db
            .queued_mutations()
            .map(|q| q.iter().any(|m| m.anilist_id == outcome.anilist_id))
            .unwrap_or(true);
        let _ = app.emit(
            if still_queued { "sync:queued" } else { "sync:pushed" },
            outcome,
        );
    });
}

/// Drain the outbound queue. Best-effort: stops early on auth/rate-limit, marks
/// individual failures with exponential backoff.
pub async fn drain(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(token) = state.auth.valid_token() else {
        if state.auth.is_expired() {
            let _ = app.emit("sync:auth-expired", ());
        }
        return;
    };

    let items: Vec<QueuedMutation> = match state.db.queued_mutations() {
        Ok(i) => i,
        Err(_) => return,
    };
    if items.is_empty() {
        return;
    }
    let plan = plan_drain(items, now());
    for dead in plan.superseded {
        let _ = state.db.delete_mutation(dead.id);
    }

    for m in plan.to_send {
        let Ok(save) = serde_json::from_str::<SaveEntry>(&m.mutation_json) else {
            // Corrupt row: drop it rather than retry forever.
            let _ = state.db.delete_mutation(m.id);
            continue;
        };
        match state.anilist.save_media_list_entry(&token, &save).await {
            Ok(updated_at) => {
                let _ = state.db.delete_mutation(m.id);
                // Clear dirty only if the local row still matches what we sent
                // (a newer edit would have enqueued its own mutation).
                if let Ok(Some(cur)) = state.db.get_list_entry(save.media_id) {
                    if cur.progress == save.progress && cur.status == save.status {
                        let merged = ListEntry {
                            remote_updated_at: Some(updated_at),
                            dirty: false,
                            ..cur
                        };
                        let _ = state.db.put_list_entry(&merged);
                    }
                }
            }
            Err(Error::Unauthorized) => {
                let _ = app.emit("sync:auth-expired", ());
                return;
            }
            Err(Error::RateLimited(_)) => {
                // Back off this whole pass; the periodic worker retries later.
                let _ = state
                    .db
                    .fail_mutation(m.id, now() + backoff_secs(m.attempts + 1) as i64);
                return;
            }
            Err(e) => {
                eprintln!(
                    "sync: SaveMediaListEntry failed for anilist_id {} (attempt {}): {e}",
                    m.anilist_id,
                    m.attempts + 1
                );
                let _ = state
                    .db
                    .fail_mutation(m.id, now() + backoff_secs(m.attempts + 1) as i64);
            }
        }
    }
    let _ = app.emit("sync:updated", ());
}

/// Pull the remote list and merge it into local state per the conflict resolver.
/// Returns Ok(()) or a short error string (for the login command to surface).
pub async fn pull(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some(token) = state.auth.valid_token() else {
        if state.auth.is_expired() {
            let _ = app.emit("sync:auth-expired", ());
        }
        return Err("not logged in".into());
    };

    // Refresh the viewer (also confirms the token) and cache it.
    let viewer = match state.anilist.viewer(&token).await {
        Ok(v) => {
            state.auth.save_viewer(v.clone());
            v
        }
        Err(Error::Unauthorized) => {
            let _ = app.emit("sync:auth-expired", ());
            return Err("auth expired".into());
        }
        Err(e) => return Err(e.to_string()),
    };

    let remote = match state.anilist.media_list_collection(&token, viewer.id).await {
        Ok(r) => r,
        Err(Error::Unauthorized) => {
            let _ = app.emit("sync:auth-expired", ());
            return Err("auth expired".into());
        }
        Err(e) => return Err(e.to_string()),
    };

    for r in &remote {
        let _ = state.db.upsert_media(
            r.anilist_id,
            r.title_romaji.as_deref(),
            r.title_english.as_deref(),
            r.cover_url.as_deref(),
            r.episode_count,
            None,
        );
        let ep_count = r
            .episode_count
            .or_else(|| state.db.media_episode_count(r.anilist_id).ok().flatten());
        let local = state.db.get_list_entry(r.anilist_id).ok().flatten();
        let outcome = merge(local.as_ref(), r, ep_count);
        if outcome.changed_local {
            let _ = state.db.put_list_entry(&outcome.merged);
        }
        if outcome.needs_push {
            if let Ok(json) = serde_json::to_string(&save_entry_of(&outcome.merged)) {
                let _ = state.db.enqueue_mutation(r.anilist_id, &json);
            }
        }
        if outcome.remote_overwrote_local {
            let _ = app.emit(
                "sync:remote-overwrote",
                RemoteOverwrite {
                    anilist_id: r.anilist_id,
                    title: r.title_english.clone().or_else(|| r.title_romaji.clone()),
                },
            );
        }
    }

    let _ = app.emit("sync:updated", ());
    drain(app).await;
    Ok(())
}

/// Spawn the background worker: one pull+drain at startup, then a periodic
/// drain every 60s and a full pull every 15 min. All calls no-op when logged
/// out, so this is safe to run unconditionally.
pub fn spawn_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Give the webview a moment to register event listeners.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = pull(&app).await;

        let mut ticks: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            ticks += 1;
            drain(&app).await;
            if ticks % 15 == 0 {
                let _ = pull(&app).await;
            }
        }
    });
}
