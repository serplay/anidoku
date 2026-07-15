mod downloads;
mod migrations;

use crate::models::{
    AiringRow, ContinueWatchingItem, LibraryItem, ListEntry, MediaListStatus, Notification,
    WatchState,
};
use crate::sync::QueuedMutation;
use crate::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

/// The single local store. All access is serialized through a mutex; SQLite
/// with WAL is fast enough for this app's write rates (watch-state ticks,
/// download progress) and it keeps the API trivially Send + Sync.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ---- watch_state (resume points) ----

    pub fn get_watch_state(&self, anime_id: &str, episode_number: &str) -> Result<Option<WatchState>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT anime_id, episode_number, position_secs, duration_secs, updated_at
                 FROM watch_state WHERE anime_id = ?1 AND episode_number = ?2",
                params![anime_id, episode_number],
                |r| {
                    Ok(WatchState {
                        anime_id: r.get(0)?,
                        episode_number: r.get(1)?,
                        position_secs: r.get(2)?,
                        duration_secs: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn set_watch_state(
        &self,
        anime_id: &str,
        episode_number: &str,
        position_secs: f64,
        duration_secs: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO watch_state (anime_id, episode_number, position_secs, duration_secs, updated_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())
             ON CONFLICT(anime_id, episode_number) DO UPDATE SET
               position_secs = excluded.position_secs,
               duration_secs = COALESCE(excluded.duration_secs, watch_state.duration_secs),
               updated_at = excluded.updated_at",
            params![anime_id, episode_number, position_secs, duration_secs],
        )?;
        Ok(())
    }

    /// All resume points for one show, keyed by episode number (detail page).
    pub fn watch_states_for_anime(&self, anime_id: &str) -> Result<Vec<WatchState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT anime_id, episode_number, position_secs, duration_secs, updated_at
             FROM watch_state WHERE anime_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![anime_id], |r| {
                Ok(WatchState {
                    anime_id: r.get(0)?,
                    episode_number: r.get(1)?,
                    position_secs: r.get(2)?,
                    duration_secs: r.get(3)?,
                    updated_at: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- anime metadata cache ----

    pub fn cache_anime(
        &self,
        provider_id: &str,
        title_romaji: &str,
        title_english: Option<&str>,
        cover_url: Option<&str>,
        episode_count: Option<u32>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO anime (provider_id, title_romaji, title_english, cover_url, episode_count, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())
             ON CONFLICT(provider_id) DO UPDATE SET
               title_romaji = excluded.title_romaji,
               title_english = COALESCE(excluded.title_english, anime.title_english),
               cover_url = COALESCE(excluded.cover_url, anime.cover_url),
               episode_count = COALESCE(excluded.episode_count, anime.episode_count),
               cached_at = excluded.cached_at",
            params![provider_id, title_romaji, title_english, cover_url, episode_count],
        )?;
        Ok(())
    }

    pub fn get_cached_anime(&self, provider_id: &str) -> Result<Option<(String, Option<String>, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT title_romaji, title_english, cover_url FROM anime WHERE provider_id = ?1",
                params![provider_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row)
    }

    // ---- provider <-> anilist mapping ----

    /// Best-effort link of a provider show to an AniList id. Ignores the UNIQUE
    /// violation that arises if the same AniList id is already mapped elsewhere
    /// (we never want to abort a search/caching pass over this).
    pub fn link_provider_anilist(&self, provider_id: &str, anilist_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let res = conn.execute(
            "UPDATE anime SET anilist_id = ?2 WHERE provider_id = ?1 AND anilist_id IS NULL",
            params![provider_id, anilist_id],
        );
        match res {
            Ok(_) => Ok(()),
            // 2067 = SQLITE_CONSTRAINT_UNIQUE. Leave the row unmapped rather
            // than error out.
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.extended_code == 2067 => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn anilist_id_for_provider(&self, provider_id: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT anilist_id FROM anime WHERE provider_id = ?1",
                params![provider_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?;
        Ok(row.flatten())
    }

    pub fn provider_id_for_anilist(&self, anilist_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT provider_id FROM anime WHERE anilist_id = ?1",
                params![anilist_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    // ---- availability (streamable-source cache, keyed by anilist_id) ----

    /// Cached availability outcome as `(available, checked_at)`, or `None` when
    /// the title has never been checked.
    pub fn get_availability(&self, anilist_id: i64) -> Result<Option<(bool, i64)>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT available, checked_at FROM availability WHERE anilist_id = ?1",
                params![anilist_id],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    /// Record an availability outcome, stamped now.
    pub fn set_availability(&self, anilist_id: i64, available: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO availability (anilist_id, available, checked_at)
             VALUES (?1, ?2, unixepoch())
             ON CONFLICT(anilist_id) DO UPDATE SET
               available  = excluded.available,
               checked_at = excluded.checked_at",
            params![anilist_id, available as i64],
        )?;
        Ok(())
    }

    // ---- media_cache (AniList metadata, keyed by anilist_id) ----

    pub fn upsert_media(
        &self,
        anilist_id: i64,
        title_romaji: Option<&str>,
        title_english: Option<&str>,
        cover_url: Option<&str>,
        episode_count: Option<i64>,
        format: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO media_cache (anilist_id, title_romaji, title_english, cover_url, episode_count, format, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
             ON CONFLICT(anilist_id) DO UPDATE SET
               title_romaji  = COALESCE(excluded.title_romaji, media_cache.title_romaji),
               title_english = COALESCE(excluded.title_english, media_cache.title_english),
               cover_url     = COALESCE(excluded.cover_url, media_cache.cover_url),
               episode_count = COALESCE(excluded.episode_count, media_cache.episode_count),
               format        = COALESCE(excluded.format, media_cache.format),
               cached_at     = excluded.cached_at",
            params![anilist_id, title_romaji, title_english, cover_url, episode_count, format],
        )?;
        Ok(())
    }

    /// True when `media_cache` has no title for this id — a card rendered from
    /// it would fall back to an "AniList #id" placeholder.
    pub fn media_title_missing(&self, anilist_id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let known: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM media_cache WHERE anilist_id = ?1
                 AND (title_romaji IS NOT NULL OR title_english IS NOT NULL)",
                params![anilist_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(known.is_none())
    }

    pub fn media_episode_count(&self, anilist_id: i64) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT episode_count FROM media_cache WHERE anilist_id = ?1",
                params![anilist_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?;
        Ok(row.flatten())
    }

    // ---- list_entries (local tracking state) ----

    pub fn get_list_entry(&self, anilist_id: i64) -> Result<Option<ListEntry>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT anilist_id, status, progress, score, local_updated_at, remote_updated_at, dirty
                 FROM list_entries WHERE anilist_id = ?1",
                params![anilist_id],
                row_to_list_entry,
            )
            .optional()?;
        Ok(row)
    }

    /// Write a fully-formed entry (used by the pull/merge path). Overwrites all
    /// columns including the sync bookkeeping.
    pub fn put_list_entry(&self, e: &ListEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO list_entries
               (anilist_id, status, progress, score, local_updated_at, remote_updated_at, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(anilist_id) DO UPDATE SET
               status = excluded.status,
               progress = excluded.progress,
               score = excluded.score,
               local_updated_at = excluded.local_updated_at,
               remote_updated_at = excluded.remote_updated_at,
               dirty = excluded.dirty",
            params![
                e.anilist_id,
                e.status.as_str(),
                e.progress,
                e.score,
                e.local_updated_at,
                e.remote_updated_at,
                e.dirty as i64,
            ],
        )?;
        Ok(())
    }

    /// Apply a local user edit: set status/progress/score, mark dirty, and bump
    /// `local_updated_at` to now. Explicit edits may DECREASE progress (undoing
    /// a mis-click); monotonicity is only enforced on the automatic paths
    /// (`ensure_current` / `mark_watched`), which guard before calling this.
    pub fn set_list_entry_local(
        &self,
        anilist_id: i64,
        status: MediaListStatus,
        progress: i64,
        score: Option<f64>,
    ) -> Result<ListEntry> {
        let existing = self.get_list_entry(anilist_id)?;
        let progress = progress.max(0);
        let remote_updated_at = existing.as_ref().and_then(|e| e.remote_updated_at);
        let entry = ListEntry {
            anilist_id,
            status,
            progress,
            score: score.or_else(|| existing.as_ref().and_then(|e| e.score)),
            local_updated_at: now(),
            remote_updated_at,
            dirty: true,
        };
        self.put_list_entry(&entry)?;
        Ok(entry)
    }

    /// Ensure a show is on the CURRENT list at at-least `progress`. Used by the
    /// auto-add path (user starts watching an unlisted show). Returns the entry
    /// and whether it was newly created, so the caller can enqueue a mutation.
    pub fn ensure_current(&self, anilist_id: i64, progress: i64) -> Result<(ListEntry, bool)> {
        match self.get_list_entry(anilist_id)? {
            Some(e) => {
                // Already listed. Only advance progress (monotonic); keep status.
                if progress > e.progress {
                    let updated = self.set_list_entry_local(anilist_id, e.status, progress, e.score)?;
                    Ok((updated, true))
                } else {
                    Ok((e, false))
                }
            }
            None => {
                let e =
                    self.set_list_entry_local(anilist_id, MediaListStatus::Current, progress, None)?;
                Ok((e, true))
            }
        }
    }

    /// Record that episode `progress` was watched: ensure the show is listed
    /// (auto-adding to CURRENT), advance progress monotonically, and promote
    /// the status to COMPLETED as soon as progress reaches the known episode
    /// count. Returns the entry and whether anything changed (so the caller
    /// can enqueue a mutation).
    pub fn mark_watched(&self, anilist_id: i64, progress: i64) -> Result<(ListEntry, bool)> {
        let (entry, changed) = self.ensure_current(anilist_id, progress)?;
        let total = self.media_episode_count(anilist_id)?.unwrap_or(0);
        if total > 0 && entry.progress >= total && entry.status != MediaListStatus::Completed {
            let promoted = self.set_list_entry_local(
                anilist_id,
                MediaListStatus::Completed,
                entry.progress,
                entry.score,
            )?;
            return Ok((promoted, true));
        }
        Ok((entry, changed))
    }

    /// The library view: every list entry joined with whatever media metadata
    /// and provider mapping we have.
    pub fn library(&self) -> Result<Vec<LibraryItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT le.anilist_id, le.status, le.progress, le.score, le.dirty,
                    mc.title_romaji, mc.title_english, mc.cover_url, mc.episode_count,
                    a.provider_id
             FROM list_entries le
             LEFT JOIN media_cache mc ON mc.anilist_id = le.anilist_id
             LEFT JOIN anime a ON a.anilist_id = le.anilist_id
             ORDER BY le.local_updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let status: String = r.get(1)?;
                Ok(LibraryItem {
                    anilist_id: r.get(0)?,
                    status: MediaListStatus::parse(&status).unwrap_or(MediaListStatus::Current),
                    progress: r.get(2)?,
                    score: r.get(3)?,
                    dirty: r.get::<_, i64>(4)? != 0,
                    title_romaji: r.get(5)?,
                    title_english: r.get(6)?,
                    cover_url: r.get(7)?,
                    episode_count: r.get(8)?,
                    provider_id: r.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- sync_queue (outbound mutation queue) ----

    pub fn enqueue_mutation(&self, anilist_id: i64, mutation_json: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_queue (anilist_id, mutation_json, queued_at, attempts, next_attempt_at)
             VALUES (?1, ?2, unixepoch(), 0, 0)",
            params![anilist_id, mutation_json],
        )?;
        Ok(())
    }

    pub fn queued_mutations(&self) -> Result<Vec<QueuedMutation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, anilist_id, mutation_json, attempts, next_attempt_at
             FROM sync_queue ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(QueuedMutation {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    mutation_json: r.get(2)?,
                    attempts: r.get(3)?,
                    next_attempt_at: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn queue_len(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT count(*) FROM sync_queue", [], |r| r.get(0))?)
    }

    pub fn delete_mutation(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Record a failed attempt: bump the counter and schedule the next retry.
    pub fn fail_mutation(&self, id: i64, next_attempt_at: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue SET attempts = attempts + 1, next_attempt_at = ?2 WHERE id = ?1",
            params![id, next_attempt_at],
        )?;
        Ok(())
    }

    /// Clear the outbound queue (used on logout).
    pub fn clear_sync_queue(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_queue", [])?;
        Ok(())
    }

    // ---- app_settings (generic kv for UI/behaviour toggles) ----

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// A boolean setting with a default (stored as "1"/"0").
    pub fn get_bool_setting(&self, key: &str, default: bool) -> Result<bool> {
        Ok(self
            .get_setting(key)?
            .map(|v| v == "1" || v == "true")
            .unwrap_or(default))
    }

    pub fn set_bool_setting(&self, key: &str, value: bool) -> Result<()> {
        self.set_setting(key, if value { "1" } else { "0" })
    }

    // ---- home_cache (AniList home rows, 6h TTL) ----

    /// The cached JSON + fetch time for a section, if present.
    pub fn get_home_cache(&self, section: &str) -> Result<Option<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT json, fetched_at FROM home_cache WHERE section = ?1",
                params![section],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    pub fn put_home_cache(&self, section: &str, json: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO home_cache (section, json, fetched_at) VALUES (?1, ?2, unixepoch())
             ON CONFLICT(section) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at",
            params![section, json],
        )?;
        Ok(())
    }

    // ---- continue watching (local CURRENT/REPEATING + watch_state) ----

    /// CURRENT/REPEATING list entries joined with media + provider mapping, plus
    /// the most-recent watch activity for ordering. Fully local (offline-capable).
    /// The next-unwatched episode is `progress + 1`, computed by the caller.
    pub fn continue_watching(&self) -> Result<Vec<ContinueWatchingItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT le.anilist_id, le.progress,
                    mc.title_romaji, mc.title_english, mc.cover_url, mc.episode_count,
                    a.provider_id,
                    COALESCE((SELECT MAX(ws.updated_at) FROM watch_state ws
                              WHERE ws.anime_id = a.provider_id), le.local_updated_at)
             FROM list_entries le
             LEFT JOIN media_cache mc ON mc.anilist_id = le.anilist_id
             LEFT JOIN anime a ON a.anilist_id = le.anilist_id
             WHERE le.status IN ('CURRENT','REPEATING')
             ORDER BY 8 DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let episode_count: Option<i64> = r.get(5)?;
                let progress: i64 = r.get(1)?;
                // Next unwatched episode = progress + 1, unless already complete.
                let next_episode = match episode_count {
                    Some(total) if total > 0 && progress >= total => None,
                    _ => Some((progress + 1).to_string()),
                };
                Ok(ContinueWatchingItem {
                    anilist_id: r.get(0)?,
                    progress,
                    title_romaji: r.get(2)?,
                    title_english: r.get(3)?,
                    cover_url: r.get(4)?,
                    episode_count,
                    provider_id: r.get(6)?,
                    next_episode,
                    last_watched_at: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- airing tracker ----

    /// AniList ids of shows to track: CURRENT/REPEATING always, plus PLANNING
    /// when `include_planning` is set (the Settings toggle).
    pub fn tracked_anilist_ids(&self, include_planning: bool) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let sql = if include_planning {
            "SELECT anilist_id FROM list_entries WHERE status IN ('CURRENT','REPEATING','PLANNING')"
        } else {
            "SELECT anilist_id FROM list_entries WHERE status IN ('CURRENT','REPEATING')"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_airing(&self, row: &AiringRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO airing (anilist_id, next_episode, airing_at, media_status, refreshed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(anilist_id) DO UPDATE SET
               next_episode = excluded.next_episode,
               airing_at = excluded.airing_at,
               media_status = excluded.media_status,
               refreshed_at = excluded.refreshed_at",
            params![
                row.anilist_id,
                row.next_episode,
                row.airing_at,
                row.media_status,
                row.refreshed_at
            ],
        )?;
        Ok(())
    }

    pub fn delete_airing(&self, anilist_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM airing WHERE anilist_id = ?1", params![anilist_id])?;
        Ok(())
    }

    pub fn all_airing(&self) -> Result<Vec<AiringRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT anilist_id, next_episode, airing_at, media_status, refreshed_at
             FROM airing ORDER BY airing_at ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AiringRow {
                    anilist_id: r.get(0)?,
                    next_episode: r.get(1)?,
                    airing_at: r.get(2)?,
                    media_status: r.get(3)?,
                    refreshed_at: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Upcoming airings for the inbox: every airing row with a known next
    /// episode, joined with media + provider metadata, soonest first. Reuses
    /// the `Notification` shape (id = anilist_id; kind = "upcoming").
    pub fn upcoming(&self) -> Result<Vec<Notification>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ai.anilist_id, ai.next_episode, ai.airing_at, ai.refreshed_at,
                    mc.title_romaji, mc.title_english, mc.cover_url, a.provider_id
             FROM airing ai
             LEFT JOIN media_cache mc ON mc.anilist_id = ai.anilist_id
             LEFT JOIN anime a ON a.anilist_id = ai.anilist_id
             WHERE ai.next_episode IS NOT NULL AND ai.airing_at IS NOT NULL
             ORDER BY ai.airing_at ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Notification {
                    id: r.get(0)?, // anilist_id doubles as a stable key
                    anilist_id: r.get(0)?,
                    episode: r.get(1)?,
                    airing_at: r.get(2)?,
                    kind: "upcoming".into(),
                    created_at: r.get(3)?,
                    read: true,
                    title_romaji: r.get(4)?,
                    title_english: r.get(5)?,
                    cover_url: r.get(6)?,
                    provider_id: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- notifications (inbox) ----

    /// Insert a fired notification. Returns `true` if it was new (de-duped on
    /// the UNIQUE `(anilist_id, episode)` constraint via INSERT OR IGNORE).
    pub fn insert_notification(
        &self,
        anilist_id: i64,
        episode: i64,
        airing_at: Option<i64>,
        kind: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT OR IGNORE INTO notifications (anilist_id, episode, airing_at, kind, created_at, read)
             VALUES (?1, ?2, ?3, ?4, unixepoch(), 0)",
            params![anilist_id, episode, airing_at, kind],
        )?;
        Ok(n > 0)
    }

    /// All notifications, newest first, joined with media + provider metadata.
    pub fn notifications(&self) -> Result<Vec<Notification>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.anilist_id, n.episode, n.airing_at, n.kind, n.created_at, n.read,
                    mc.title_romaji, mc.title_english, mc.cover_url, a.provider_id
             FROM notifications n
             LEFT JOIN media_cache mc ON mc.anilist_id = n.anilist_id
             LEFT JOIN anime a ON a.anilist_id = n.anilist_id
             ORDER BY n.created_at DESC, n.id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Notification {
                    id: r.get(0)?,
                    anilist_id: r.get(1)?,
                    episode: r.get(2)?,
                    airing_at: r.get(3)?,
                    kind: r.get(4)?,
                    created_at: r.get(5)?,
                    read: r.get::<_, i64>(6)? != 0,
                    title_romaji: r.get(7)?,
                    title_english: r.get(8)?,
                    cover_url: r.get(9)?,
                    provider_id: r.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn unread_notification_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT count(*) FROM notifications WHERE read = 0", [], |r| r.get(0))?)
    }

    pub fn mark_all_notifications_read(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE notifications SET read = 1 WHERE read = 0", [])?;
        Ok(())
    }

    pub fn clear_notifications(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM notifications", [])?;
        Ok(())
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row_to_list_entry(r: &rusqlite::Row) -> rusqlite::Result<ListEntry> {
    let status: String = r.get(1)?;
    Ok(ListEntry {
        anilist_id: r.get(0)?,
        status: MediaListStatus::parse(&status).unwrap_or(MediaListStatus::Current),
        progress: r.get(2)?,
        score: r.get(3)?,
        local_updated_at: r.get(4)?,
        remote_updated_at: r.get(5)?,
        dirty: r.get::<_, i64>(6)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let db = Database::open_in_memory().unwrap();
        // Re-running migrate on the same connection must be a no-op.
        let conn = db.conn.lock().unwrap();
        migrations::migrate(&conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
                 ('anime','episodes','downloads','list_entries','sync_queue','watch_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 6);
    }

    #[test]
    fn watch_state_roundtrip_and_upsert() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_watch_state("show1", "1").unwrap().is_none());

        db.set_watch_state("show1", "1", 42.5, Some(1440.0)).unwrap();
        let ws = db.get_watch_state("show1", "1").unwrap().unwrap();
        assert_eq!(ws.position_secs, 42.5);
        assert_eq!(ws.duration_secs, Some(1440.0));

        // Update keeps duration when not provided.
        db.set_watch_state("show1", "1", 100.0, None).unwrap();
        let ws = db.get_watch_state("show1", "1").unwrap().unwrap();
        assert_eq!(ws.position_secs, 100.0);
        assert_eq!(ws.duration_secs, Some(1440.0));

        assert_eq!(db.watch_states_for_anime("show1").unwrap().len(), 1);
    }

    #[test]
    fn list_entry_local_edit_marks_dirty_and_allows_decrement() {
        let db = Database::open_in_memory().unwrap();
        let e = db
            .set_list_entry_local(154587, MediaListStatus::Current, 5, Some(8.0))
            .unwrap();
        assert!(e.dirty);
        assert_eq!(e.progress, 5);
        // Explicit user edits may decrease progress (undo a mis-click)…
        let e = db
            .set_list_entry_local(154587, MediaListStatus::Current, 3, None)
            .unwrap();
        assert_eq!(e.progress, 3);
        assert_eq!(e.score, Some(8.0)); // preserved
        // …but never below zero.
        let e = db
            .set_list_entry_local(154587, MediaListStatus::Current, -2, None)
            .unwrap();
        assert_eq!(e.progress, 0);
        // The automatic path stays monotonic: re-watching an episode at or
        // below current progress is a no-op, above it advances.
        db.set_list_entry_local(154587, MediaListStatus::Current, 3, None).unwrap();
        let (_e, changed) = db.ensure_current(154587, 2).unwrap();
        assert!(!changed);
        let (e, changed) = db.ensure_current(154587, 4).unwrap();
        assert!(changed);
        assert_eq!(e.progress, 4);
    }

    #[test]
    fn ensure_current_adds_then_advances() {
        let db = Database::open_in_memory().unwrap();
        let (e, created) = db.ensure_current(100, 1).unwrap();
        assert!(created);
        assert_eq!(e.status, MediaListStatus::Current);
        assert_eq!(e.progress, 1);
        // Same or lower progress → no change.
        let (_e, changed) = db.ensure_current(100, 1).unwrap();
        assert!(!changed);
        // Higher progress advances.
        let (e, changed) = db.ensure_current(100, 4).unwrap();
        assert!(changed);
        assert_eq!(e.progress, 4);
    }

    #[test]
    fn mark_watched_promotes_to_completed_on_last_episode() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_media(100, Some("Show"), None, None, Some(12), Some("TV")).unwrap();
        // Mid-season: stays CURRENT.
        let (e, changed) = db.mark_watched(100, 11).unwrap();
        assert!(changed);
        assert_eq!(e.status, MediaListStatus::Current);
        // Final episode: promoted in the same local write.
        let (e, changed) = db.mark_watched(100, 12).unwrap();
        assert!(changed);
        assert_eq!(e.status, MediaListStatus::Completed);
        assert_eq!(e.progress, 12);
        assert!(e.dirty);
        // Idempotent: re-watching the finale changes nothing.
        let (_e, changed) = db.mark_watched(100, 12).unwrap();
        assert!(!changed);
    }

    #[test]
    fn mark_watched_promotes_even_when_progress_already_at_count() {
        let db = Database::open_in_memory().unwrap();
        // Entry reached the count before the episode total became known.
        db.set_list_entry_local(100, MediaListStatus::Current, 12, None).unwrap();
        db.upsert_media(100, Some("Show"), None, None, Some(12), Some("TV")).unwrap();
        let (e, changed) = db.mark_watched(100, 12).unwrap();
        assert!(changed);
        assert_eq!(e.status, MediaListStatus::Completed);
    }

    #[test]
    fn mark_watched_without_known_count_stays_current() {
        let db = Database::open_in_memory().unwrap();
        let (e, changed) = db.mark_watched(200, 3).unwrap();
        assert!(changed);
        assert_eq!(e.status, MediaListStatus::Current);
        assert_eq!(e.progress, 3);
    }

    #[test]
    fn library_join_pulls_media_and_provider() {
        let db = Database::open_in_memory().unwrap();
        db.cache_anime("prov1", "Frieren", None, None, Some(28)).unwrap();
        db.link_provider_anilist("prov1", 154587).unwrap();
        db.upsert_media(154587, Some("Sousou no Frieren"), Some("Frieren"), Some("http://c.jpg"), Some(28), Some("TV"))
            .unwrap();
        db.set_list_entry_local(154587, MediaListStatus::Current, 5, None).unwrap();
        // An entry with no media/provider metadata still shows up.
        db.set_list_entry_local(999, MediaListStatus::Planning, 0, None).unwrap();

        let lib = db.library().unwrap();
        assert_eq!(lib.len(), 2);
        let frieren = lib.iter().find(|i| i.anilist_id == 154587).unwrap();
        assert_eq!(frieren.title_english.as_deref(), Some("Frieren"));
        assert_eq!(frieren.episode_count, Some(28));
        assert_eq!(frieren.provider_id.as_deref(), Some("prov1"));
        let bare = lib.iter().find(|i| i.anilist_id == 999).unwrap();
        assert!(bare.title_english.is_none());
        assert!(bare.provider_id.is_none());
    }

    #[test]
    fn sync_queue_roundtrip_and_backoff() {
        let db = Database::open_in_memory().unwrap();
        db.enqueue_mutation(1, "{\"a\":1}").unwrap();
        db.enqueue_mutation(2, "{\"a\":2}").unwrap();
        assert_eq!(db.queue_len().unwrap(), 2);
        let items = db.queued_mutations().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].anilist_id, 1);
        assert_eq!(items[0].attempts, 0);

        db.fail_mutation(items[0].id, 12345).unwrap();
        let items = db.queued_mutations().unwrap();
        assert_eq!(items[0].attempts, 1);
        assert_eq!(items[0].next_attempt_at, 12345);

        db.delete_mutation(items[0].id).unwrap();
        assert_eq!(db.queue_len().unwrap(), 1);
        db.clear_sync_queue().unwrap();
        assert_eq!(db.queue_len().unwrap(), 0);
    }

    #[test]
    fn link_provider_anilist_ignores_unique_conflict() {
        let db = Database::open_in_memory().unwrap();
        db.cache_anime("provA", "A", None, None, None).unwrap();
        db.cache_anime("provB", "B", None, None, None).unwrap();
        db.link_provider_anilist("provA", 42).unwrap();
        // provB claiming the same anilist_id must not error (best-effort).
        db.link_provider_anilist("provB", 42).unwrap();
        assert_eq!(db.provider_id_for_anilist(42).unwrap().as_deref(), Some("provA"));
        assert_eq!(db.anilist_id_for_provider("provB").unwrap(), None);
    }

    #[test]
    fn home_cache_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_home_cache("trending").unwrap().is_none());
        db.put_home_cache("trending", "[{\"a\":1}]").unwrap();
        let (json, fetched) = db.get_home_cache("trending").unwrap().unwrap();
        assert_eq!(json, "[{\"a\":1}]");
        assert!(fetched > 0);
        // Overwrite replaces the blob.
        db.put_home_cache("trending", "[]").unwrap();
        assert_eq!(db.get_home_cache("trending").unwrap().unwrap().0, "[]");
    }

    #[test]
    fn bool_setting_roundtrip_with_default() {
        let db = Database::open_in_memory().unwrap();
        // Unset → default.
        assert!(!db.get_bool_setting("notify_planning", false).unwrap());
        assert!(db.get_bool_setting("notify_planning", true).unwrap());
        db.set_bool_setting("notify_planning", true).unwrap();
        assert!(db.get_bool_setting("notify_planning", false).unwrap());
        db.set_bool_setting("notify_planning", false).unwrap();
        assert!(!db.get_bool_setting("notify_planning", true).unwrap());
    }

    #[test]
    fn notifications_dedupe_and_unread() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.insert_notification(100, 3, Some(5000), "episode").unwrap());
        // Same (anilist_id, episode) is a no-op (de-dupe) — false.
        assert!(!db.insert_notification(100, 3, Some(5000), "episode").unwrap());
        // A different episode fires.
        assert!(db.insert_notification(100, 4, Some(6000), "episode").unwrap());
        assert_eq!(db.unread_notification_count().unwrap(), 2);
        assert_eq!(db.notifications().unwrap().len(), 2);
        db.mark_all_notifications_read().unwrap();
        assert_eq!(db.unread_notification_count().unwrap(), 0);
        db.clear_notifications().unwrap();
        assert_eq!(db.notifications().unwrap().len(), 0);
    }

    #[test]
    fn airing_upsert_and_tracked_ids() {
        let db = Database::open_in_memory().unwrap();
        db.set_list_entry_local(100, MediaListStatus::Current, 1, None).unwrap();
        db.set_list_entry_local(200, MediaListStatus::Planning, 0, None).unwrap();
        db.set_list_entry_local(300, MediaListStatus::Completed, 12, None).unwrap();
        let mut ids = db.tracked_anilist_ids(false).unwrap();
        ids.sort();
        assert_eq!(ids, vec![100]); // only CURRENT/REPEATING
        let mut ids = db.tracked_anilist_ids(true).unwrap();
        ids.sort();
        assert_eq!(ids, vec![100, 200]); // + PLANNING

        let row = AiringRow {
            anilist_id: 100,
            next_episode: Some(2),
            airing_at: Some(9000),
            media_status: Some("RELEASING".into()),
            refreshed_at: 1,
        };
        db.upsert_airing(&row).unwrap();
        assert_eq!(db.all_airing().unwrap().len(), 1);
        // Upsert advances the same row.
        db.upsert_airing(&AiringRow { next_episode: Some(3), ..row.clone() }).unwrap();
        let all = db.all_airing().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].next_episode, Some(3));
        db.delete_airing(100).unwrap();
        assert!(db.all_airing().unwrap().is_empty());
    }

    #[test]
    fn continue_watching_lists_current_with_next_episode() {
        let db = Database::open_in_memory().unwrap();
        db.cache_anime("prov1", "Frieren", None, None, Some(28)).unwrap();
        db.link_provider_anilist("prov1", 154587).unwrap();
        db.upsert_media(154587, Some("Sousou no Frieren"), Some("Frieren"), None, Some(28), Some("TV"))
            .unwrap();
        db.set_list_entry_local(154587, MediaListStatus::Current, 5, None).unwrap();
        // A completed show must not appear.
        db.set_list_entry_local(999, MediaListStatus::Completed, 12, None).unwrap();

        let cw = db.continue_watching().unwrap();
        assert_eq!(cw.len(), 1);
        assert_eq!(cw[0].anilist_id, 154587);
        assert_eq!(cw[0].progress, 5);
        assert_eq!(cw[0].next_episode.as_deref(), Some("6")); // progress + 1
        assert_eq!(cw[0].provider_id.as_deref(), Some("prov1"));

        // A show watched to the finale has no next episode.
        db.set_list_entry_local(154587, MediaListStatus::Current, 28, None).unwrap();
        let cw = db.continue_watching().unwrap();
        assert_eq!(cw[0].next_episode, None);
    }

    #[test]
    fn anime_cache_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.cache_anime("abc", "Sousou no Frieren", Some("Frieren"), Some("http://x/c.jpg"), Some(28))
            .unwrap();
        // Upsert without cover must keep the old cover.
        db.cache_anime("abc", "Sousou no Frieren", None, None, None).unwrap();
        let (romaji, english, cover) = db.get_cached_anime("abc").unwrap().unwrap();
        assert_eq!(romaji, "Sousou no Frieren");
        assert_eq!(english.as_deref(), Some("Frieren"));
        assert_eq!(cover.as_deref(), Some("http://x/c.jpg"));
    }
}
