use rusqlite::Connection;

/// Ordered, append-only list of migrations. `user_version` tracks progress.
///
/// Schema follows ARCHITECTURE.md section 3, with two deliberate adjustments:
/// - `anime` is keyed by `provider_id` with `anilist_id` UNIQUE-nullable
///   instead of the reverse: M1 has no AniList linkage yet, so rows must be
///   insertable before an AniList id is known (M2 fills it in).
/// - `watch_state` gains `duration_secs` so the UI can render progress bars
///   and "watched" ticks without re-resolving the stream.
/// - Episode numbers are TEXT everywhere: allanime uses fractional episode
///   strings like "5.5".
const MIGRATIONS: &[&str] = &[
    // 001: initial schema
    "
    CREATE TABLE anime (
        provider_id   TEXT PRIMARY KEY,
        anilist_id    INTEGER UNIQUE,
        title_romaji  TEXT NOT NULL,
        title_english TEXT,
        cover_url     TEXT,
        episode_count INTEGER,
        format        TEXT,
        cached_at     INTEGER NOT NULL DEFAULT (unixepoch())
    );

    CREATE TABLE episodes (
        anime_id TEXT NOT NULL REFERENCES anime(provider_id) ON DELETE CASCADE,
        number   TEXT NOT NULL,
        title    TEXT,
        PRIMARY KEY (anime_id, number)
    );

    CREATE TABLE downloads (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        anime_id       TEXT NOT NULL,
        episode_number TEXT NOT NULL,
        state          TEXT NOT NULL DEFAULT 'queued'
                       CHECK (state IN ('queued','downloading','paused','done','failed')),
        quality        TEXT,
        bytes_total    INTEGER,
        bytes_done     INTEGER NOT NULL DEFAULT 0,
        segments_done  INTEGER NOT NULL DEFAULT 0,
        dir_path       TEXT,
        error          TEXT,
        created_at     INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at     INTEGER NOT NULL DEFAULT (unixepoch())
    );
    CREATE INDEX idx_downloads_state ON downloads(state);

    CREATE TABLE list_entries (
        anilist_id        INTEGER PRIMARY KEY,
        status            TEXT NOT NULL
                          CHECK (status IN ('CURRENT','PLANNING','COMPLETED','DROPPED','PAUSED','REPEATING')),
        progress          INTEGER NOT NULL DEFAULT 0,
        score             REAL,
        local_updated_at  INTEGER NOT NULL DEFAULT (unixepoch()),
        remote_updated_at INTEGER,
        dirty             INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE sync_queue (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        anilist_id    INTEGER NOT NULL,
        mutation_json TEXT NOT NULL,
        queued_at     INTEGER NOT NULL DEFAULT (unixepoch()),
        attempts      INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE watch_state (
        anime_id       TEXT NOT NULL,
        episode_number TEXT NOT NULL,
        position_secs  REAL NOT NULL DEFAULT 0,
        duration_secs  REAL,
        updated_at     INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY (anime_id, episode_number)
    );
    ",
    // 002: sync-queue retry scheduling. `next_attempt_at` lets the drainer
    // apply exponential backoff without a separate bookkeeping table.
    "
    ALTER TABLE sync_queue ADD COLUMN next_attempt_at INTEGER NOT NULL DEFAULT 0;
    ",
    // 003: AniList media metadata cache, keyed by anilist_id. Library entries
    // pulled from AniList often have no provider mapping yet, so their titles /
    // covers / episode counts can't live in the provider-keyed `anime` table.
    "
    CREATE TABLE media_cache (
        anilist_id    INTEGER PRIMARY KEY,
        title_romaji  TEXT,
        title_english TEXT,
        cover_url     TEXT,
        episode_count INTEGER,
        format        TEXT,
        cached_at     INTEGER NOT NULL DEFAULT (unixepoch())
    );
    ",
];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i64;
        if version < target {
            conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {target}; COMMIT;"))?;
        }
    }
    Ok(())
}
