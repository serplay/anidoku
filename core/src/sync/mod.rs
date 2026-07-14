//! Local-first AniList sync: conflict resolution, provider→AniList matching,
//! and the pure planning logic for draining the outbound mutation queue.
//!
//! The async worker that actually performs pulls/pushes lives on the Tauri side
//! (it needs the token, the `AniListClient`, and the DB), but every decision it
//! makes — merge outcome, match choice, drain order, backoff timing — is a pure
//! function here so it can be exhaustively unit-tested.

pub mod conflict;
pub mod matching;

pub use conflict::{merge, MergeOutcome};
pub use matching::best_match;

/// Exponential backoff for a queued mutation that has failed `attempts` times.
/// 0 attempts → ready now. Base 5s, doubling, capped at 1 hour.
pub fn backoff_secs(attempts: i64) -> u64 {
    if attempts <= 0 {
        return 0;
    }
    const BASE: u64 = 5;
    const CAP: u64 = 3600;
    // 5, 10, 20, 40, ... saturating.
    BASE.saturating_mul(1u64 << (attempts - 1).min(20)).min(CAP)
}

/// A pending outbound mutation (one `sync_queue` row).
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedMutation {
    pub id: i64,
    pub anilist_id: i64,
    /// Serialized `anilist::SaveEntry`.
    pub mutation_json: String,
    pub attempts: i64,
    /// Earliest unix time this row may be retried (queued_at + backoff).
    pub next_attempt_at: i64,
}

/// Plan a drain pass at time `now`: keep only rows whose backoff has elapsed,
/// preserve FIFO (id ascending) order, then coalesce so at most one mutation
/// per `anilist_id` is sent — `SaveMediaListEntry` writes absolute values, so
/// only the newest queued mutation for a show matters. Superseded rows are
/// returned separately so the caller can delete them without a network call.
pub struct DrainPlan {
    /// Rows to actually send, FIFO order, one per anilist_id (the latest).
    pub to_send: Vec<QueuedMutation>,
    /// Older rows made redundant by a newer mutation for the same show; delete.
    pub superseded: Vec<QueuedMutation>,
}

pub fn plan_drain(mut items: Vec<QueuedMutation>, now: i64) -> DrainPlan {
    items.retain(|m| m.next_attempt_at <= now);
    items.sort_by_key(|m| m.id);

    // Walk newest→oldest so the first time we see an anilist_id it's the latest.
    let mut seen = std::collections::HashSet::new();
    let mut keep_ids = std::collections::HashSet::new();
    for m in items.iter().rev() {
        if seen.insert(m.anilist_id) {
            keep_ids.insert(m.id);
        }
    }

    let mut to_send = Vec::new();
    let mut superseded = Vec::new();
    for m in items {
        if keep_ids.contains(&m.id) {
            to_send.push(m);
        } else {
            superseded.push(m);
        }
    }
    DrainPlan {
        to_send,
        superseded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: i64, anilist_id: i64, attempts: i64, next_at: i64) -> QueuedMutation {
        QueuedMutation {
            id,
            anilist_id,
            mutation_json: format!("{{\"media_id\":{anilist_id},\"id\":{id}}}"),
            attempts,
            next_attempt_at: next_at,
        }
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_secs(0), 0);
        assert_eq!(backoff_secs(1), 5);
        assert_eq!(backoff_secs(2), 10);
        assert_eq!(backoff_secs(3), 20);
        assert_eq!(backoff_secs(4), 40);
        assert_eq!(backoff_secs(100), 3600); // capped, no overflow
    }

    #[test]
    fn plan_drain_preserves_fifo_order() {
        let items = vec![q(3, 30, 0, 0), q(1, 10, 0, 0), q(2, 20, 0, 0)];
        let plan = plan_drain(items, 100);
        let ids: Vec<i64> = plan.to_send.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert!(plan.superseded.is_empty());
    }

    #[test]
    fn plan_drain_skips_backed_off_rows() {
        let items = vec![q(1, 10, 3, 500), q(2, 20, 0, 50)];
        let plan = plan_drain(items, 100);
        // Row 1 not due until t=500; only row 2 is sent.
        let ids: Vec<i64> = plan.to_send.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn plan_drain_coalesces_to_latest_per_show() {
        // Three mutations for show 10, plus one for show 20. Only the newest
        // (highest id) for show 10 is sent; the earlier two are superseded.
        let items = vec![
            q(1, 10, 0, 0),
            q(2, 10, 0, 0),
            q(5, 10, 0, 0),
            q(3, 20, 0, 0),
        ];
        let plan = plan_drain(items, 100);
        let sent: Vec<i64> = plan.to_send.iter().map(|m| m.id).collect();
        assert_eq!(sent, vec![3, 5]); // FIFO order of survivors
        let dead: Vec<i64> = plan.superseded.iter().map(|m| m.id).collect();
        assert_eq!(dead, vec![1, 2]);
    }
}
