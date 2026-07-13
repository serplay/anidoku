mod migrations;

use crate::models::WatchState;
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
