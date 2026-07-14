//! Conflict resolution for local-first list sync.
//!
//! Implements ARCHITECTURE.md §3 exactly:
//!   1. progress = max(local, remote) — watching is monotonic, never regress.
//!   2. status   = last-writer-wins by timestamp (local_updated_at vs remote
//!      updatedAt), with one guard: if merged progress == episode_count the
//!      status promotes to COMPLETED regardless of the winner.
//!   3. local dirty + remote unchanged since last pull → push local (no merge).
//!   4. silent auto-merge; caller shows a toast only when a remote value
//!      actually overwrote a local one.

use crate::models::{ListEntry, MediaListStatus, RemoteListEntry};

/// Outcome of merging a remote entry into local state.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeOutcome {
    pub merged: ListEntry,
    /// The merge changed the local row (needs a DB write).
    pub changed_local: bool,
    /// A remote value overwrote what the user had locally — surface a toast.
    pub remote_overwrote_local: bool,
    /// After merge the local row still differs from remote and must be pushed
    /// (i.e. it stays dirty and should be enqueued).
    pub needs_push: bool,
}

/// Merge one remote entry into an optional local entry.
///
/// `episode_count` is the show's total episodes (for the COMPLETED promotion),
/// if known.
pub fn merge(
    local: Option<&ListEntry>,
    remote: &RemoteListEntry,
    episode_count: Option<i64>,
) -> MergeOutcome {
    let Some(local) = local else {
        // No local row: adopt remote wholesale. Nothing to push.
        return MergeOutcome {
            merged: ListEntry {
                anilist_id: remote.anilist_id,
                status: promote(remote.status, remote.progress, episode_count),
                progress: remote.progress,
                score: remote.score,
                local_updated_at: remote.updated_at,
                remote_updated_at: Some(remote.updated_at),
                dirty: false,
            },
            changed_local: true,
            remote_overwrote_local: false,
            needs_push: false,
        };
    };

    // Case 3: local is dirty and remote has not changed since our last pull.
    // Keep local as-is and push it; no conflict, no remote overwrite.
    let remote_unchanged = local.remote_updated_at == Some(remote.updated_at);
    if local.dirty && remote_unchanged {
        let merged = ListEntry {
            remote_updated_at: Some(remote.updated_at),
            ..local.clone()
        };
        return MergeOutcome {
            changed_local: merged != *local,
            merged,
            remote_overwrote_local: false,
            needs_push: true,
        };
    }

    // 1. Progress is monotonic.
    let progress = local.progress.max(remote.progress);

    // 2. Status: last-writer-wins by timestamp.
    let remote_wins_status = remote.updated_at >= local.local_updated_at;
    let base_status = if remote_wins_status {
        remote.status
    } else {
        local.status
    };
    // COMPLETED promotion guard.
    let status = promote(base_status, progress, episode_count);

    // Score follows the status winner (whoever most recently touched the entry).
    let score = if remote_wins_status {
        remote.score.or(local.score)
    } else {
        local.score.or(remote.score)
    };

    // Did a remote value overwrite something the user had locally?
    let remote_overwrote_local = (remote_wins_status && status != local.status)
        || (remote.progress > local.progress && local.dirty);

    // Does local still differ from remote after merge? Then it must be pushed.
    let needs_push = progress != remote.progress
        || status != remote.status
        || (score.is_some() && score != remote.score);

    let merged = ListEntry {
        anilist_id: local.anilist_id,
        status,
        progress,
        score,
        // Local timestamp advances only if the merged result differs from what
        // we last pushed; keep it stable otherwise so LWW stays meaningful.
        local_updated_at: local.local_updated_at.max(remote.updated_at),
        remote_updated_at: Some(remote.updated_at),
        dirty: needs_push,
    };

    MergeOutcome {
        changed_local: merged != *local,
        merged,
        remote_overwrote_local,
        needs_push,
    }
}

/// COMPLETED promotion: if merged progress reaches the episode count, the entry
/// is complete regardless of which side won the status race.
fn promote(status: MediaListStatus, progress: i64, episode_count: Option<i64>) -> MediaListStatus {
    match episode_count {
        Some(count) if count > 0 && progress >= count => MediaListStatus::Completed,
        _ => status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(status: MediaListStatus, progress: i64, local_ts: i64, remote_ts: Option<i64>, dirty: bool) -> ListEntry {
        ListEntry {
            anilist_id: 1,
            status,
            progress,
            score: None,
            local_updated_at: local_ts,
            remote_updated_at: remote_ts,
            dirty,
        }
    }

    fn remote(status: MediaListStatus, progress: i64, updated_at: i64) -> RemoteListEntry {
        RemoteListEntry {
            anilist_id: 1,
            status,
            progress,
            score: None,
            updated_at,
            title_romaji: None,
            title_english: None,
            cover_url: None,
            episode_count: None,
        }
    }

    #[test]
    fn no_local_adopts_remote() {
        let r = remote(MediaListStatus::Current, 5, 1000);
        let o = merge(None, &r, Some(12));
        assert_eq!(o.merged.progress, 5);
        assert_eq!(o.merged.status, MediaListStatus::Current);
        assert!(!o.merged.dirty);
        assert!(!o.needs_push);
        assert!(!o.remote_overwrote_local);
        assert!(o.changed_local);
    }

    #[test]
    fn no_local_adopts_remote_with_completion_promotion() {
        // Remote says CURRENT but progress hit the episode count.
        let r = remote(MediaListStatus::Current, 12, 1000);
        let o = merge(None, &r, Some(12));
        assert_eq!(o.merged.status, MediaListStatus::Completed);
    }

    #[test]
    fn progress_takes_max_never_regresses() {
        // Local ahead (E7), remote behind (E5) → keep 7, must push.
        let l = local(MediaListStatus::Current, 7, 2000, Some(1000), false);
        let r = remote(MediaListStatus::Current, 5, 1500);
        let o = merge(Some(&l), &r, Some(12));
        assert_eq!(o.merged.progress, 7);
        assert!(o.needs_push);
        assert!(!o.remote_overwrote_local);
    }

    #[test]
    fn remote_ahead_overwrites_and_no_push() {
        // Remote ahead (E7) vs local (E5) → adopt 7. Since local was dirty,
        // that's a remote-overwrote-local toast.
        let l = local(MediaListStatus::Current, 5, 2000, Some(1000), true);
        let r = remote(MediaListStatus::Current, 7, 2500);
        let o = merge(Some(&l), &r, Some(12));
        assert_eq!(o.merged.progress, 7);
        assert!(o.remote_overwrote_local);
        assert!(!o.needs_push);
        assert!(!o.merged.dirty);
    }

    #[test]
    fn status_last_writer_wins_remote_newer() {
        // Remote newer → its DROPPED status wins over local CURRENT.
        let l = local(MediaListStatus::Current, 5, 1000, Some(900), false);
        let r = remote(MediaListStatus::Dropped, 5, 2000);
        let o = merge(Some(&l), &r, Some(12));
        assert_eq!(o.merged.status, MediaListStatus::Dropped);
        assert!(o.remote_overwrote_local);
    }

    #[test]
    fn status_last_writer_wins_local_newer() {
        // Local newer → its PAUSED status wins; must push.
        let l = local(MediaListStatus::Paused, 5, 3000, Some(900), true);
        let r = remote(MediaListStatus::Current, 5, 2000);
        let o = merge(Some(&l), &r, Some(12));
        assert_eq!(o.merged.status, MediaListStatus::Paused);
        assert!(o.needs_push);
        assert!(!o.remote_overwrote_local);
    }

    #[test]
    fn completed_promotion_overrides_status_winner() {
        // Remote newer says CURRENT, but merged progress == count → COMPLETED.
        let l = local(MediaListStatus::Current, 12, 1000, Some(900), false);
        let r = remote(MediaListStatus::Current, 11, 2000);
        let o = merge(Some(&l), &r, Some(12));
        assert_eq!(o.merged.progress, 12);
        assert_eq!(o.merged.status, MediaListStatus::Completed);
    }

    #[test]
    fn local_dirty_remote_unchanged_pushes_local() {
        // Case 3: local dirty, remote updatedAt == our last-pulled remote ts.
        let l = local(MediaListStatus::Current, 8, 3000, Some(2000), true);
        let r = remote(MediaListStatus::Current, 5, 2000); // unchanged since pull
        let o = merge(Some(&l), &r, Some(12));
        assert_eq!(o.merged.progress, 8);
        assert_eq!(o.merged.status, MediaListStatus::Current);
        assert!(o.needs_push);
        assert!(!o.remote_overwrote_local);
        assert!(o.merged.dirty);
    }

    #[test]
    fn no_change_when_in_sync() {
        let l = local(MediaListStatus::Current, 5, 1000, Some(1000), false);
        let r = remote(MediaListStatus::Current, 5, 1000);
        let o = merge(Some(&l), &r, Some(12));
        assert!(!o.changed_local);
        assert!(!o.needs_push);
        assert!(!o.remote_overwrote_local);
    }
}
