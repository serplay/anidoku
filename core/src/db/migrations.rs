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
    // 004: download-engine columns. `dub` selects the translation type when the
    // job re-resolves its source; `kind`/`segments_total` are resolved at start
    // and drive resume checkpoints + progress math.
    "
    ALTER TABLE downloads ADD COLUMN dub INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE downloads ADD COLUMN kind TEXT;
    ALTER TABLE downloads ADD COLUMN segments_total INTEGER;
    ",
    // 005: M3.5 home page + airing tracker & notification inbox.
    // - home_cache: one JSON blob per section with a 6h TTL (fetched_at),
    //   for instant offline-tolerant render.
    // - airing: the tracker's local view of when each tracked show's next
    //   episode airs (keyed by anilist_id).
    // - notifications: fired episode releases, de-duped on (anilist_id, episode).
    // - app_settings: generic kv for UI/behaviour toggles (e.g. notify_planning).
    "
    CREATE TABLE home_cache (
        section    TEXT PRIMARY KEY,
        json       TEXT NOT NULL,
        fetched_at INTEGER NOT NULL DEFAULT (unixepoch())
    );

    CREATE TABLE airing (
        anilist_id   INTEGER PRIMARY KEY,
        next_episode INTEGER,
        airing_at    INTEGER,
        media_status TEXT,
        refreshed_at INTEGER NOT NULL DEFAULT (unixepoch())
    );

    CREATE TABLE notifications (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        anilist_id INTEGER NOT NULL,
        episode    INTEGER NOT NULL,
        airing_at  INTEGER,
        kind       TEXT NOT NULL DEFAULT 'episode',
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        read       INTEGER NOT NULL DEFAULT 0,
        UNIQUE (anilist_id, episode)
    );

    CREATE TABLE app_settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    ",
    // 006: catalog-source availability cache. Records whether an AniList title
    // resolves to a streamable provider show, so repeat searches don't re-hammer
    // the provider. Positive results are effectively permanent (a mapping in
    // `anime` already implies available); negative results carry a TTL and are
    // re-checked after it lapses.
    "
    CREATE TABLE availability (
        anilist_id INTEGER PRIMARY KEY,
        available  INTEGER NOT NULL,
        checked_at INTEGER NOT NULL DEFAULT (unixepoch())
    );
    ",
    // 007: multi-source. Two assumptions had to go:
    //
    //   1. Show ids were bare provider ids, so ids from two sources could
    //      collide and nothing said who could resolve one. Every id becomes
    //      "<source>:<show_id>" (provider::id::SourceId). The prefix is opaque
    //      to routing and to the schema, so /anime/<id>, the FK, and the
    //      download path all keep working.
    //   2. `anime.anilist_id` was UNIQUE, which hard-capped each AniList show
    //      at one source. Mappings move to `anime_sources` (many per show, one
    //      marked preferred). SQLite cannot drop a constraint, so `anime` is
    //      rebuilt.
    //
    // NOT migrated, deliberately: `downloads.dir_path`. It names real
    // directories on disk (written via downloads::sanitize_component, which
    // maps ':' to '_'). Leaving stored paths alone keeps every completed
    // download playable with zero filesystem work; new jobs simply get the
    // namespaced path. `anime_id` still moves, because that is what joins to
    // `anime` and what the UI routes on.
    "
    PRAGMA foreign_keys = off;

    CREATE TABLE anime_new (
        provider_id   TEXT PRIMARY KEY,
        source        TEXT NOT NULL,
        anilist_id    INTEGER,
        title_romaji  TEXT NOT NULL,
        title_english TEXT,
        cover_url     TEXT,
        episode_count INTEGER,
        format        TEXT,
        cached_at     INTEGER NOT NULL DEFAULT (unixepoch())
    );
    INSERT INTO anime_new
        SELECT 'allanime:'||provider_id, 'allanime', anilist_id, title_romaji,
               title_english, cover_url, episode_count, format, cached_at
        FROM anime;
    DROP TABLE anime;
    ALTER TABLE anime_new RENAME TO anime;
    CREATE INDEX idx_anime_anilist ON anime(anilist_id);
    CREATE INDEX idx_anime_source ON anime(source);

    UPDATE episodes    SET anime_id = 'allanime:'||anime_id;
    UPDATE downloads   SET anime_id = 'allanime:'||anime_id;
    UPDATE watch_state SET anime_id = 'allanime:'||anime_id;

    CREATE TABLE anime_sources (
        anilist_id  INTEGER NOT NULL,
        source      TEXT NOT NULL,
        provider_id TEXT NOT NULL,
        preferred   INTEGER NOT NULL DEFAULT 0,
        linked_at   INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY (anilist_id, source)
    );
    CREATE UNIQUE INDEX idx_anime_sources_provider ON anime_sources(provider_id);
    INSERT INTO anime_sources (anilist_id, source, provider_id, preferred)
        SELECT anilist_id, source, provider_id, 1 FROM anime WHERE anilist_id IS NOT NULL;

    CREATE TABLE availability_new (
        anilist_id INTEGER NOT NULL,
        source     TEXT NOT NULL,
        available  INTEGER NOT NULL,
        checked_at INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY (anilist_id, source)
    );
    INSERT INTO availability_new (anilist_id, source, available, checked_at)
        SELECT anilist_id, 'allanime', available, checked_at FROM availability;
    DROP TABLE availability;
    ALTER TABLE availability_new RENAME TO availability;

    PRAGMA foreign_keys = on;
    ",
];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i64;
        if version < target {
            conn.execute_batch(&format!(
                "BEGIN; {sql} PRAGMA user_version = {target}; COMMIT;"
            ))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a database at the schema version just before 007 and fill it with
    /// the kind of rows a real install would have accumulated.
    fn legacy_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..MIGRATIONS.len() - 1] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "
            INSERT INTO anime (provider_id, anilist_id, title_romaji, episode_count)
                VALUES ('ReooPAxPMsHM4KPMY', 21, 'One Piece', 1100),
                       ('unmappedShowId', NULL, 'Some Show', 12);
            INSERT INTO episodes (anime_id, number) VALUES ('ReooPAxPMsHM4KPMY', '1');
            INSERT INTO downloads (anime_id, episode_number, state, dir_path)
                VALUES ('ReooPAxPMsHM4KPMY', '1', 'done', 'ReooPAxPMsHM4KPMY/1');
            INSERT INTO watch_state (anime_id, episode_number, position_secs)
                VALUES ('ReooPAxPMsHM4KPMY', '1', 421.5);
            INSERT INTO availability (anilist_id, available) VALUES (21, 1);
            ",
        )
        .unwrap();
        conn
    }

    fn migrate_to_007(conn: &Connection) {
        conn.execute_batch(MIGRATIONS[MIGRATIONS.len() - 1])
            .unwrap();
    }

    fn one<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> T {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn migration_007_namespaces_every_id_that_joins() {
        let conn = legacy_db();
        migrate_to_007(&conn);

        assert_eq!(
            one::<String>(&conn, "SELECT provider_id FROM anime WHERE anilist_id = 21"),
            "allanime:ReooPAxPMsHM4KPMY"
        );
        assert_eq!(
            one::<String>(&conn, "SELECT source FROM anime WHERE anilist_id = 21"),
            "allanime"
        );
        for t in ["episodes", "downloads", "watch_state"] {
            assert_eq!(
                one::<String>(&conn, &format!("SELECT anime_id FROM {t}")),
                "allanime:ReooPAxPMsHM4KPMY",
                "{t}.anime_id was not namespaced"
            );
        }
    }

    #[test]
    fn migration_007_leaves_download_paths_on_disk_alone() {
        // dir_path names real directories; rewriting it would orphan every
        // completed download. New jobs get the namespaced path instead.
        let conn = legacy_db();
        migrate_to_007(&conn);
        assert_eq!(
            one::<String>(&conn, "SELECT dir_path FROM downloads"),
            "ReooPAxPMsHM4KPMY/1"
        );
    }

    #[test]
    fn migration_007_preserves_watch_progress() {
        let conn = legacy_db();
        migrate_to_007(&conn);
        assert_eq!(
            one::<f64>(&conn, "SELECT position_secs FROM watch_state"),
            421.5
        );
    }

    #[test]
    fn migration_007_backfills_mappings_as_preferred() {
        let conn = legacy_db();
        migrate_to_007(&conn);
        assert_eq!(
            one::<String>(
                &conn,
                "SELECT provider_id FROM anime_sources WHERE anilist_id = 21"
            ),
            "allanime:ReooPAxPMsHM4KPMY"
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT preferred FROM anime_sources WHERE anilist_id = 21"
            ),
            1
        );
        // An unmapped show contributes no mapping row.
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM anime_sources"), 1);
        // ...but its cache row survives.
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM anime WHERE provider_id = 'allanime:unmappedShowId'"
            ),
            1
        );
    }

    #[test]
    fn migration_007_lets_one_anilist_show_map_to_several_sources() {
        // The whole point: `anime.anilist_id UNIQUE` used to forbid this.
        let conn = legacy_db();
        migrate_to_007(&conn);
        conn.execute_batch(
            "INSERT INTO anime (provider_id, source, anilist_id, title_romaji)
                 VALUES ('hianime:one-piece-100', 'hianime', 21, 'One Piece');
             INSERT INTO anime_sources (anilist_id, source, provider_id, preferred)
                 VALUES (21, 'hianime', 'hianime:one-piece-100', 0);",
        )
        .unwrap();
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM anime_sources WHERE anilist_id = 21"
            ),
            2
        );
        // The preferred one still wins the deep-link lookup.
        assert_eq!(
            one::<String>(
                &conn,
                "SELECT provider_id FROM anime_sources WHERE anilist_id = 21
                 ORDER BY preferred DESC, linked_at ASC LIMIT 1"
            ),
            "allanime:ReooPAxPMsHM4KPMY"
        );
    }

    #[test]
    fn migration_007_carries_availability_over_as_allanime() {
        let conn = legacy_db();
        migrate_to_007(&conn);
        assert_eq!(
            one::<String>(
                &conn,
                "SELECT source FROM availability WHERE anilist_id = 21"
            ),
            "allanime"
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT available FROM availability WHERE anilist_id = 21"
            ),
            1
        );
    }

    #[test]
    fn a_fresh_database_lands_on_the_same_schema_as_a_migrated_one() {
        let fresh = Connection::open_in_memory().unwrap();
        migrate(&fresh).unwrap();
        let migrated = legacy_db();
        migrate_to_007(&migrated);

        let cols = |c: &Connection, t: &str| -> Vec<String> {
            let mut st = c
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{t}') ORDER BY name"
                ))
                .unwrap();
            let v = st.query_map([], |r| r.get::<_, String>(0)).unwrap();
            v.map(|r| r.unwrap()).collect()
        };
        for t in [
            "anime",
            "anime_sources",
            "availability",
            "downloads",
            "watch_state",
        ] {
            assert_eq!(cols(&fresh, t), cols(&migrated, t), "{t} schema drifted");
        }
    }
}
