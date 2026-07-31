//! `downloads` table access: the persistent job queue behind the download
//! engine. Rows survive restarts; `bytes_done` (MP4) and `segments_done` (HLS)
//! are the resume checkpoints.

use super::Database;
use crate::models::{AnimeStorage, DownloadRow, DownloadState, StreamKind};
use crate::Result;
use rusqlite::{params, OptionalExtension, Row};

const ROW_COLS: &str = "d.id, d.anime_id, d.episode_number, d.state, d.quality, d.dub, d.kind,
     d.bytes_total, d.bytes_done, d.segments_done, d.segments_total,
     d.dir_path, d.error, d.created_at, d.updated_at";

fn row_to_download(r: &Row, title_idx: Option<usize>) -> rusqlite::Result<DownloadRow> {
    let state: String = r.get(3)?;
    let kind: Option<String> = r.get(6)?;
    Ok(DownloadRow {
        id: r.get(0)?,
        anime_id: r.get(1)?,
        episode_number: r.get(2)?,
        state: DownloadState::parse(&state).unwrap_or(DownloadState::Failed),
        quality: r.get(4)?,
        dub: r.get::<_, i64>(5)? != 0,
        kind: kind.as_deref().and_then(StreamKind::parse),
        bytes_total: r.get(7)?,
        bytes_done: r.get(8)?,
        segments_done: r.get(9)?,
        segments_total: r.get(10)?,
        dir_path: r.get(11)?,
        error: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
        title: match title_idx {
            Some(i) => r.get(i)?,
            None => None,
        },
    })
}

impl Database {
    /// Enqueue a download job. Returns `None` when the episode already has a
    /// live row (queued/downloading/paused/done) — duplicates are rejected, and
    /// a previously failed row is reset to queued instead of duplicated.
    pub fn enqueue_download(
        &self,
        anime_id: &str,
        episode_number: &str,
        quality: Option<&str>,
        dub: bool,
    ) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, state FROM downloads WHERE anime_id = ?1 AND episode_number = ?2",
                params![anime_id, episode_number],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match existing {
            Some((id, state)) if state == "failed" => {
                conn.execute(
                    "UPDATE downloads SET state = 'queued', error = NULL, quality = ?2, dub = ?3,
                            updated_at = unixepoch()
                     WHERE id = ?1",
                    params![id, quality, dub as i64],
                )?;
                Ok(Some(id))
            }
            Some(_) => Ok(None),
            None => {
                conn.execute(
                    "INSERT INTO downloads (anime_id, episode_number, state, quality, dub)
                     VALUES (?1, ?2, 'queued', ?3, ?4)",
                    params![anime_id, episode_number, quality, dub as i64],
                )?;
                Ok(Some(conn.last_insert_rowid()))
            }
        }
    }

    pub fn get_download(&self, id: i64) -> Result<Option<DownloadRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!("SELECT {ROW_COLS} FROM downloads d WHERE d.id = ?1"),
                params![id],
                |r| row_to_download(r, None),
            )
            .optional()?;
        Ok(row)
    }

    pub fn download_for_episode(
        &self,
        anime_id: &str,
        episode_number: &str,
    ) -> Result<Option<DownloadRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!(
                    "SELECT {ROW_COLS} FROM downloads d
                     WHERE d.anime_id = ?1 AND d.episode_number = ?2"
                ),
                params![anime_id, episode_number],
                |r| row_to_download(r, None),
            )
            .optional()?;
        Ok(row)
    }

    /// All download rows, newest first, with the show title joined in for the
    /// manager view.
    pub fn list_downloads(&self) -> Result<Vec<DownloadRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ROW_COLS}, COALESCE(a.title_english, a.title_romaji)
             FROM downloads d LEFT JOIN anime a ON a.provider_id = d.anime_id
             ORDER BY d.id DESC"
        ))?;
        let rows = stmt
            .query_map([], |r| row_to_download(r, Some(15)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn downloads_for_anime(&self, anime_id: &str) -> Result<Vec<DownloadRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ROW_COLS} FROM downloads d WHERE d.anime_id = ?1 ORDER BY d.id ASC"
        ))?;
        let rows = stmt
            .query_map(params![anime_id], |r| row_to_download(r, None))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_download_state(
        &self,
        id: i64,
        state: DownloadState,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET state = ?2, error = ?3, updated_at = unixepoch() WHERE id = ?1",
            params![id, state.as_str(), error],
        )?;
        Ok(())
    }

    /// Persist a progress checkpoint. `bytes_total` / `segments_total` only
    /// overwrite when known (Some), so an early update can't erase them.
    pub fn update_download_progress(
        &self,
        id: i64,
        bytes_done: i64,
        bytes_total: Option<i64>,
        segments_done: i64,
        segments_total: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET
               bytes_done = ?2,
               bytes_total = COALESCE(?3, bytes_total),
               segments_done = ?4,
               segments_total = COALESCE(?5, segments_total),
               updated_at = unixepoch()
             WHERE id = ?1",
            params![id, bytes_done, bytes_total, segments_done, segments_total],
        )?;
        Ok(())
    }

    /// Record what the job resolved to: episode dir (relative to the downloads
    /// root), stream kind, and the concrete quality label.
    pub fn set_download_meta(
        &self,
        id: i64,
        dir_path: &str,
        kind: StreamKind,
        quality: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET dir_path = ?2, kind = ?3, quality = ?4, updated_at = unixepoch()
             WHERE id = ?1",
            params![id, dir_path, kind.as_str(), quality],
        )?;
        Ok(())
    }

    /// Atomically claim the oldest queued job: flips it to `downloading` and
    /// returns it. All access is serialized through the connection mutex, so
    /// two schedulers cannot claim the same row.
    pub fn claim_next_queued(&self) -> Result<Option<DownloadRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                &format!(
                    "SELECT {ROW_COLS} FROM downloads d
                     WHERE d.state = 'queued' ORDER BY d.id ASC LIMIT 1"
                ),
                [],
                |r| row_to_download(r, None),
            )
            .optional()?;
        let Some(mut row) = row else { return Ok(None) };
        conn.execute(
            "UPDATE downloads SET state = 'downloading', error = NULL, updated_at = unixepoch()
             WHERE id = ?1",
            params![row.id],
        )?;
        row.state = DownloadState::Downloading;
        row.error = None;
        Ok(Some(row))
    }

    pub fn count_downloading(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT count(*) FROM downloads WHERE state = 'downloading'",
            [],
            |r| r.get(0),
        )?)
    }

    /// Rows that still need the process alive (queued or actively
    /// downloading) — drives the Android foreground service.
    pub fn count_active_downloads(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT count(*) FROM downloads WHERE state IN ('queued', 'downloading')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Startup recovery: rows stuck in `downloading` (app was killed mid-job)
    /// revert to `queued` so the scheduler resumes them from their checkpoints.
    pub fn recover_in_flight_downloads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "UPDATE downloads SET state = 'queued', updated_at = unixepoch()
             WHERE state = 'downloading'",
            [],
        )?)
    }

    pub fn delete_download_row(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Storage accounting for the manager view: completed bytes and episode
    /// counts per show (title/cover joined from the anime cache).
    pub fn download_storage(&self) -> Result<Vec<AnimeStorage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT d.anime_id, COALESCE(a.title_english, a.title_romaji), a.cover_url,
                    COUNT(*), COALESCE(SUM(d.bytes_done), 0)
             FROM downloads d LEFT JOIN anime a ON a.provider_id = d.anime_id
             WHERE d.state = 'done'
             GROUP BY d.anime_id
             ORDER BY 5 DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AnimeStorage {
                    anime_id: r.get(0)?,
                    title: r.get(1)?,
                    cover_url: r.get(2)?,
                    episodes: r.get(3)?,
                    bytes: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Total bytes across all completed downloads.
    pub fn download_total_bytes(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(bytes_done), 0) FROM downloads WHERE state = 'done'",
            [],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_rejects_duplicates_and_resets_failed() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .enqueue_download("show", "1", Some("best"), false)
            .unwrap()
            .unwrap();
        // Duplicate while queued -> None.
        assert_eq!(
            db.enqueue_download("show", "1", Some("best"), false)
                .unwrap(),
            None
        );
        // Failed rows are reset to queued instead of duplicated.
        db.set_download_state(id, DownloadState::Failed, Some("boom"))
            .unwrap();
        let re = db
            .enqueue_download("show", "1", Some("720"), true)
            .unwrap()
            .unwrap();
        assert_eq!(re, id);
        let row = db.get_download(id).unwrap().unwrap();
        assert_eq!(row.state, DownloadState::Queued);
        assert_eq!(row.error, None);
        assert_eq!(row.quality.as_deref(), Some("720"));
        assert!(row.dub);
    }

    #[test]
    fn claim_is_fifo_and_marks_downloading() {
        let db = Database::open_in_memory().unwrap();
        db.enqueue_download("show", "1", None, false).unwrap();
        db.enqueue_download("show", "2", None, false).unwrap();
        let a = db.claim_next_queued().unwrap().unwrap();
        assert_eq!(a.episode_number, "1");
        assert_eq!(a.state, DownloadState::Downloading);
        assert_eq!(db.count_downloading().unwrap(), 1);
        let b = db.claim_next_queued().unwrap().unwrap();
        assert_eq!(b.episode_number, "2");
        assert!(db.claim_next_queued().unwrap().is_none());
    }

    #[test]
    fn progress_checkpoint_roundtrip_preserves_totals() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .enqueue_download("show", "1", None, false)
            .unwrap()
            .unwrap();
        db.update_download_progress(id, 1000, Some(5000), 0, None)
            .unwrap();
        // A later update without totals keeps them.
        db.update_download_progress(id, 2000, None, 3, Some(24))
            .unwrap();
        let row = db.get_download(id).unwrap().unwrap();
        assert_eq!(row.bytes_done, 2000);
        assert_eq!(row.bytes_total, Some(5000));
        assert_eq!(row.segments_done, 3);
        assert_eq!(row.segments_total, Some(24));
    }

    #[test]
    fn recover_reverts_downloading_to_queued() {
        let db = Database::open_in_memory().unwrap();
        db.enqueue_download("show", "1", None, false).unwrap();
        db.enqueue_download("show", "2", None, false).unwrap();
        db.claim_next_queued().unwrap();
        assert_eq!(db.recover_in_flight_downloads().unwrap(), 1);
        assert_eq!(db.count_downloading().unwrap(), 0);
        // Both claimable again, checkpoints intact.
        assert!(db.claim_next_queued().unwrap().is_some());
        assert!(db.claim_next_queued().unwrap().is_some());
    }

    #[test]
    fn storage_accounting_sums_done_only() {
        let db = Database::open_in_memory().unwrap();
        db.cache_anime("showA", "Show A", Some("A!"), Some("http://c"), None)
            .unwrap();
        let a1 = db
            .enqueue_download("showA", "1", None, false)
            .unwrap()
            .unwrap();
        let a2 = db
            .enqueue_download("showA", "2", None, false)
            .unwrap()
            .unwrap();
        let b1 = db
            .enqueue_download("showB", "1", None, false)
            .unwrap()
            .unwrap();
        db.update_download_progress(a1, 100, Some(100), 0, None)
            .unwrap();
        db.update_download_progress(a2, 250, Some(250), 0, None)
            .unwrap();
        db.update_download_progress(b1, 999, Some(2000), 0, None)
            .unwrap();
        db.set_download_state(a1, DownloadState::Done, None)
            .unwrap();
        db.set_download_state(a2, DownloadState::Done, None)
            .unwrap();
        // b1 still downloading: excluded.
        let storage = db.download_storage().unwrap();
        assert_eq!(storage.len(), 1);
        assert_eq!(storage[0].anime_id, "showA");
        assert_eq!(storage[0].title.as_deref(), Some("A!"));
        assert_eq!(storage[0].episodes, 2);
        assert_eq!(storage[0].bytes, 350);
        assert_eq!(db.download_total_bytes().unwrap(), 350);
    }

    #[test]
    fn meta_and_listing_join_title() {
        let db = Database::open_in_memory().unwrap();
        db.cache_anime("show", "Romaji", None, None, None).unwrap();
        let id = db
            .enqueue_download("show", "5.5", Some("best"), false)
            .unwrap()
            .unwrap();
        db.set_download_meta(id, "show/5.5", StreamKind::Hls, "1080")
            .unwrap();
        let all = db.list_downloads().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].dir_path.as_deref(), Some("show/5.5"));
        assert_eq!(all[0].kind, Some(StreamKind::Hls));
        assert_eq!(all[0].quality.as_deref(), Some("1080"));
        assert_eq!(all[0].title.as_deref(), Some("Romaji"));
        // Episode lookup.
        let one = db.download_for_episode("show", "5.5").unwrap().unwrap();
        assert_eq!(one.id, id);
        // Delete removes the row.
        db.delete_download_row(id).unwrap();
        assert!(db.get_download(id).unwrap().is_none());
    }
}
