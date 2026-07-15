//! Pure planning logic for the airing tracker + notification inbox.
//!
//! The async worker (fetch batched `nextAiringEpisode`, write rows, fire OS
//! notifications) lives on the Tauri side; every decision it makes — which
//! shows to keep tracking, which to drop, and which airings are now due — is a
//! pure function here so it can be exhaustively unit-tested, including the
//! awkward cases (episode missed while the app was closed, de-dupe).

use crate::models::{AiringInfo, AiringRow};

/// Home-cache freshness: true when `fetched_at` is within `ttl_secs` of `now`.
/// Used for the 6h `home_cache` TTL and any other cached-render decision.
pub fn is_fresh(fetched_at: i64, now: i64, ttl_secs: i64) -> bool {
    now >= fetched_at && now - fetched_at < ttl_secs
}

/// The default home-cache TTL: 6 hours.
pub const HOME_TTL_SECS: i64 = 6 * 60 * 60;

/// A media status that means the show is done and should stop being tracked.
fn is_terminal_status(status: Option<&str>) -> bool {
    matches!(status, Some("FINISHED") | Some("CANCELLED"))
}

/// Outcome of diffing a batch of fetched airing info against the tracker.
#[derive(Debug, Default, PartialEq)]
pub struct AiringRefresh {
    /// Rows to upsert into `airing` (still-airing shows with a next episode).
    pub upserts: Vec<AiringRow>,
    /// AniList ids to untrack: `nextAiringEpisode == null` or a terminal status.
    /// The final episode still notifies (it was written on a prior refresh); we
    /// just stop tracking once there is nothing more to air.
    pub untrack: Vec<i64>,
}

/// Plan an airing refresh from freshly fetched `nextAiringEpisode` info.
///
/// - A show with a `next_episode` + `airing_at` and a non-terminal status is
///   upserted so the ticker can fire it when it comes due.
/// - A show whose `nextAiringEpisode` is null, or whose media has FINISHED /
///   CANCELLED, is untracked.
pub fn plan_airing_refresh(fetched: &[AiringInfo], now: i64) -> AiringRefresh {
    let mut out = AiringRefresh::default();
    for info in fetched {
        let terminal = is_terminal_status(info.media_status.as_deref());
        match (info.next_episode, info.airing_at, terminal) {
            (Some(ep), Some(at), false) => out.upserts.push(AiringRow {
                anilist_id: info.anilist_id,
                next_episode: Some(ep),
                airing_at: Some(at),
                media_status: info.media_status.clone(),
                refreshed_at: now,
            }),
            // No next episode, or the show is done: stop tracking.
            _ => out.untrack.push(info.anilist_id),
        }
    }
    out
}

/// A notification that should be fired: the show's next episode has aired.
#[derive(Debug, Clone, PartialEq)]
pub struct DueNotification {
    pub anilist_id: i64,
    pub episode: i64,
    pub airing_at: i64,
}

/// The airing rows whose next episode has aired by `now` (`airing_at <= now`).
///
/// This is the single source of "what fired": it runs on startup (so an episode
/// that aired while the app was closed is caught the moment it reopens) and on
/// every 60s tick. De-duplication is enforced at the DB layer via a UNIQUE
/// `(anilist_id, episode)` constraint + `INSERT OR IGNORE`, so re-running this
/// over the same still-persisted row is harmless.
pub fn due_notifications(rows: &[AiringRow], now: i64) -> Vec<DueNotification> {
    rows.iter()
        .filter_map(|r| match (r.next_episode, r.airing_at) {
            (Some(ep), Some(at)) if at <= now => Some(DueNotification {
                anilist_id: r.anilist_id,
                episode: ep,
                airing_at: at,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: i64, status: &str, ep: Option<i64>, at: Option<i64>) -> AiringInfo {
        AiringInfo {
            anilist_id: id,
            media_status: Some(status.to_string()),
            next_episode: ep,
            airing_at: at,
        }
    }

    fn row(id: i64, ep: Option<i64>, at: Option<i64>) -> AiringRow {
        AiringRow {
            anilist_id: id,
            next_episode: ep,
            airing_at: at,
            media_status: Some("RELEASING".into()),
            refreshed_at: 0,
        }
    }

    #[test]
    fn is_fresh_respects_ttl() {
        assert!(is_fresh(1000, 1000, HOME_TTL_SECS)); // exactly now
        assert!(is_fresh(1000, 1000 + HOME_TTL_SECS - 1, HOME_TTL_SECS));
        assert!(!is_fresh(1000, 1000 + HOME_TTL_SECS, HOME_TTL_SECS)); // boundary = stale
        assert!(!is_fresh(1000, 1000 + HOME_TTL_SECS + 1, HOME_TTL_SECS));
        // A clock that went backwards reads as stale, not fresh-forever.
        assert!(!is_fresh(1000, 500, HOME_TTL_SECS));
    }

    #[test]
    fn refresh_tracks_releasing_and_untracks_finished() {
        let fetched = vec![
            info(1, "RELEASING", Some(3), Some(5000)), // track
            info(2, "FINISHED", None, None),           // untrack (done)
            info(3, "RELEASING", None, None),          // untrack (no next ep)
            info(4, "CANCELLED", Some(9), Some(9000)), // untrack (terminal)
        ];
        let plan = plan_airing_refresh(&fetched, 100);
        assert_eq!(plan.upserts.len(), 1);
        assert_eq!(plan.upserts[0].anilist_id, 1);
        assert_eq!(plan.upserts[0].next_episode, Some(3));
        assert_eq!(plan.upserts[0].refreshed_at, 100);
        let mut untrack = plan.untrack.clone();
        untrack.sort();
        assert_eq!(untrack, vec![2, 3, 4]);
    }

    #[test]
    fn due_notifications_fires_past_airings_only() {
        let rows = vec![
            row(1, Some(3), Some(500)),  // due (aired)
            row(2, Some(1), Some(2000)), // future
            row(3, Some(5), None),       // no airing time
        ];
        let due = due_notifications(&rows, 1000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].anilist_id, 1);
        assert_eq!(due[0].episode, 3);
    }

    #[test]
    fn due_notifications_catches_episode_missed_while_closed() {
        // App was closed for days; the stored row's airing_at is well in the
        // past. On the next tick it must still fire (once — DB de-dupes).
        let rows = vec![row(42, Some(7), Some(1_000))];
        let due = due_notifications(&rows, 1_000_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].episode, 7);
        // Re-running yields the same due row; de-dupe is the DB's job.
        let again = due_notifications(&rows, 1_000_001);
        assert_eq!(again, due);
    }

    #[test]
    fn due_at_exact_airing_time_is_due() {
        let rows = vec![row(1, Some(2), Some(1000))];
        assert_eq!(due_notifications(&rows, 1000).len(), 1);
        assert_eq!(due_notifications(&rows, 999).len(), 0);
    }
}
