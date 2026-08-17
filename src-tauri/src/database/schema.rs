//! Database schema definition and initialization.

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Current official schema version.
const SCHEMA_VERSION: u32 = 2;

/// Tables that identify an existing database as the official v2 schema.
///
/// This is a recognition fingerprint, not a list of everything the app creates.
/// A database missing any of these is treated as an unknown preview schema and
/// is archived and replaced - so adding a newly introduced table here makes
/// every existing library look foreign and wipes it. New tables belong only in
/// `initialize_v2_schema`, which adds them in place with CREATE TABLE IF NOT
/// EXISTS. Nothing may be added to this list.
const REQUIRED_TABLES: &[&str] = &[
    "downloads",
    "tags",
    "download_tags",
    "assets",
    "download_versions",
    "update_targets",
    "download_relations",
    "people",
    "series",
    "download_people",
    "download_series",
    "entity_versions",
    "search_index_state",
    "saved_searches",
    "work_edit_revisions",
    "work_edit_blocks",
    "update_jobs",
    "update_job_items",
    "update_job_logs",
    "schema_v2_marker",
];

/// Open the database at the official v2 schema.
///
/// Official schemas are migrated in place so saved works are never discarded.
/// Unknown preview schemas are backed up before a clean database is created.
pub fn initialize(conn: &Connection) -> Result<(), rusqlite::Error> {
    apply_pragmas(conn)?;

    if is_official_v2_schema(conn)? {
        initialize_v2_schema(conn)?;
        migrate_additive_v2_schema(conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        return Ok(());
    }

    if is_official_v1_schema(conn)? {
        migrate_v1_to_v2(conn)?;
        initialize_v2_schema(conn)?;
        migrate_additive_v2_schema(conn)?;
        return Ok(());
    }

    backup_current_database(conn)?;
    reset_to_v2_schema(conn)?;
    migrate_additive_v2_schema(conn)?;
    Ok(())
}

/// Additive tables deliberately stay out of `REQUIRED_TABLES`: adding them to
/// the recognition fingerprint would make an older, otherwise valid v2
/// library look foreign. `CREATE TABLE IF NOT EXISTS` upgrades it in place.
fn migrate_additive_v2_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS semantic_index_state (
            download_id       INTEGER PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
            current_version   INTEGER NOT NULL,
            content_hash      TEXT,
            model_id          TEXT NOT NULL,
            indexed_at        TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_semantic_index_state_version
            ON semantic_index_state(current_version, content_hash, model_id);

         -- 見つけたが、まだ保存も拒否もしていない作品。
         --
         -- 候補はジョブの持ち物だったので、ジョブが変わると消えていた。取得元の
         -- 一覧は「前回見た位置」から先しか返さないため、一度出た作品を保存しな
         -- ければ二度と現れない。ここに残すことで、保存するか無視すると決めるま
         -- で候補が居続ける。
         CREATE TABLE IF NOT EXISTS update_candidates (
            source        TEXT NOT NULL,
            source_id     TEXT NOT NULL,
            kind          TEXT NOT NULL,
            title         TEXT NOT NULL,
            payload_json  TEXT NOT NULL,
            target_type   TEXT,
            status        TEXT NOT NULL DEFAULT 'pending',
            first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source, source_id)
         );
         CREATE INDEX IF NOT EXISTS idx_update_candidates_status
            ON update_candidates(status, updated_at DESC);",
    )?;
    add_missing_columns(conn)?;
    Ok(())
}

/// 既存のライブラリに、あとから増えた列を足す。
///
/// どちらも「更新確認を軽くするための覚え書き」で、無くても動作は変わらない
/// （その場合は本文まで取りに行く従来どおりの確認になる）。
fn add_missing_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    for (table, column, definition) in [
        // 本文以外のメタデータの指紋。取得元の軽い応答だけで変化を判定する。
        ("downloads", "meta_hash", "TEXT"),
        // 最後に本文まで突き合わせた時刻。指紋が同じでも、ここが古ければ深く見る。
        ("downloads", "last_deep_checked_at", "TEXT"),
        // 監視対象の健康状態。最後に何か見つかったのはいつか、連続で失敗していないか。
        ("update_targets", "last_hit_at", "TEXT"),
        (
            "update_targets",
            "consecutive_errors",
            "INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !column_exists(conn, table, column)? {
            conn.execute_batch(&format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                quote_identifier(table),
                quote_identifier(column),
                definition
            ))?;
        }
    }
    Ok(())
}

fn is_official_v1_schema(conn: &Connection) -> Result<bool, rusqlite::Error> {
    let current_version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);
    if current_version != 1 || !sqlite_object_exists(conn, "table", "schema_v1_marker")? {
        return Ok(false);
    }
    let marker_version = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_v1_marker",
            [],
            |row| row.get::<_, u32>(0),
        )
        .unwrap_or(0);
    if marker_version != 1 || column_exists(conn, "downloads", "tags")? {
        return Ok(false);
    }
    for table in REQUIRED_TABLES {
        if matches!(*table, "saved_searches" | "schema_v2_marker") {
            continue;
        }
        if !sqlite_object_exists(conn, "table", table)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn migrate_v1_to_v2(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS saved_searches (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            query       TEXT,
            params_json TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(name)
        );
        CREATE INDEX IF NOT EXISTS idx_saved_searches_updated
            ON saved_searches(updated_at DESC);
        CREATE TABLE IF NOT EXISTS schema_v2_marker (
            version        INTEGER PRIMARY KEY,
            initialized_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT OR REPLACE INTO schema_v2_marker (version) VALUES (2);
        DROP TABLE IF EXISTS schema_v1_marker;
        PRAGMA user_version = 2;
        COMMIT;
        ",
    )?;
    Ok(())
}

fn apply_pragmas(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        ",
    )?;
    conn.pragma_update(
        None,
        "mmap_size",
        super::resource_budget::sqlite_mmap_bytes() as i64,
    )?;
    let cache_kib = (super::resource_budget::sqlite_cache_bytes() / 1024) as i64;
    conn.pragma_update(None, "cache_size", -cache_kib)?;
    Ok(())
}

fn is_official_v2_schema(conn: &Connection) -> Result<bool, rusqlite::Error> {
    let current_version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);
    if current_version != SCHEMA_VERSION {
        return Ok(false);
    }

    let marker_version = if sqlite_object_exists(conn, "table", "schema_v2_marker")? {
        conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_v2_marker",
            [],
            |row| row.get::<_, u32>(0),
        )
        .unwrap_or(0)
    } else {
        0
    };
    if marker_version != SCHEMA_VERSION {
        return Ok(false);
    }

    for table in REQUIRED_TABLES {
        if !sqlite_object_exists(conn, "table", table)? {
            return Ok(false);
        }
    }

    if column_exists(conn, "downloads", "tags")? {
        return Ok(false);
    }

    Ok(true)
}

fn reset_to_v2_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    drop_sqlite_objects(conn, "trigger", "DROP TRIGGER IF EXISTS")?;
    drop_sqlite_objects(conn, "view", "DROP VIEW IF EXISTS")?;
    drop_sqlite_objects(conn, "table", "DROP TABLE IF EXISTS")?;
    drop_sqlite_objects(conn, "index", "DROP INDEX IF EXISTS")?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    initialize_v2_schema(conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn drop_sqlite_objects(
    conn: &Connection,
    object_type: &str,
    drop_prefix: &str,
) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT name
         FROM sqlite_master
         WHERE type = ?1
           AND name NOT LIKE 'sqlite_%'",
    )?;
    let names = stmt
        .query_map([object_type], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for name in names {
        let quoted = quote_identifier(&name);
        conn.execute_batch(&format!("{} {};", drop_prefix, quoted))?;
    }
    Ok(())
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn backup_current_database(conn: &Connection) -> Result<(), rusqlite::Error> {
    let Some(path) = main_database_path(conn)? else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }

    let _ = conn.execute_batch("PRAGMA wal_checkpoint(FULL);");
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let backup_path = path.with_extension(format!(
        "{}.preview-backup-{}",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("db"),
        stamp
    ));
    copy_if_exists(&path, &backup_path)?;

    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    let shm_path = PathBuf::from(format!("{}-shm", path.display()));
    copy_if_exists(
        &wal_path,
        &PathBuf::from(format!("{}-wal", backup_path.display())),
    )?;
    copy_if_exists(
        &shm_path,
        &PathBuf::from(format!("{}-shm", backup_path.display())),
    )?;
    Ok(())
}

fn main_database_path(conn: &Connection) -> Result<Option<PathBuf>, rusqlite::Error> {
    let path: String = conn.query_row("PRAGMA database_list", [], |row| {
        let name: String = row.get(1)?;
        let file: String = row.get(2)?;
        if name == "main" {
            Ok(file)
        } else {
            Ok(String::new())
        }
    })?;
    if path.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(path)))
    }
}

fn copy_if_exists(source: &Path, target: &Path) -> Result<(), rusqlite::Error> {
    if !source.exists() {
        return Ok(());
    }
    std::fs::copy(source, target)
        .map(|_| ())
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    if !sqlite_object_exists(conn, "table", table)? {
        return Ok(false);
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sqlite_object_exists(
    conn: &Connection,
    object_type: &str,
    name: &str,
) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
        [object_type, name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Build the official v2 schema from scratch.
fn initialize_v2_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS downloads (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            source              TEXT    NOT NULL,
            source_id           TEXT    NOT NULL,
            title               TEXT    NOT NULL,
            author_name         TEXT    NOT NULL,
            author_id           TEXT    NOT NULL,
            content_type        TEXT    NOT NULL,
            excerpt             TEXT,
            cover_path          TEXT,
            json_path           TEXT    NOT NULL,
            original_json_path  TEXT,
            asset_count         INTEGER DEFAULT 0,
            file_size_bytes     INTEGER DEFAULT 0,
            downloaded_at       TEXT    NOT NULL,
            source_created_at   TEXT,
            content_hash        TEXT,
            text_length         INTEGER DEFAULT 0,
            source_updated_at   TEXT,
            watch_updates       INTEGER DEFAULT 0,
            current_version     INTEGER DEFAULT 1,
            favorite            INTEGER DEFAULT 0,
            meta_hash           TEXT,
            last_deep_checked_at TEXT,
            UNIQUE(source, source_id)
        );

        CREATE TABLE IF NOT EXISTS tags (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            name                TEXT    NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS download_tags (
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            tag_id              INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY(download_id, tag_id)
        );

        CREATE TABLE IF NOT EXISTS assets (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            asset_type          TEXT    NOT NULL,
            filename            TEXT    NOT NULL,
            local_path          TEXT    NOT NULL,
            original_url        TEXT,
            mime_type           TEXT,
            file_size_bytes     INTEGER DEFAULT 0,
            UNIQUE(download_id, local_path)
        );

        CREATE TABLE IF NOT EXISTS download_versions (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            version             INTEGER NOT NULL,
            content_hash        TEXT,
            text_length         INTEGER DEFAULT 0,
            json_path           TEXT NOT NULL,
            original_json_path  TEXT,
            asset_count         INTEGER DEFAULT 0,
            file_size_bytes     INTEGER DEFAULT 0,
            created_at          TEXT NOT NULL,
            change_summary      TEXT,
            UNIQUE(download_id, version)
        );

        -- last_hit_at / consecutive_errors は add_missing_columns でも足される。
        -- 新しいライブラリはここで、既存のライブラリは起動時に追加される。
        CREATE TABLE IF NOT EXISTS update_targets (
            id                          INTEGER PRIMARY KEY AUTOINCREMENT,
            target_type                 TEXT    NOT NULL,
            source                      TEXT    NOT NULL,
            source_key                  TEXT    NOT NULL,
            display_name                TEXT    NOT NULL,
            enabled                     INTEGER DEFAULT 1,
            last_checked_at             TEXT,
            last_seen_source_id         TEXT,
            last_seen_source_updated_at TEXT,
            metadata_json               TEXT,
            created_at                  TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at                  TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(target_type, source, source_key)
        );

        CREATE TABLE IF NOT EXISTS download_relations (
            download_id                 INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            relation_type               TEXT    NOT NULL,
            source                      TEXT    NOT NULL,
            relation_id                 TEXT    NOT NULL,
            relation_name               TEXT    NOT NULL,
            PRIMARY KEY(download_id, relation_type, source, relation_id)
        );

        CREATE TABLE IF NOT EXISTS people (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            source              TEXT    NOT NULL,
            source_key          TEXT    NOT NULL,
            display_name        TEXT    NOT NULL,
            icon_path           TEXT,
            cover_path          TEXT,
            description         TEXT,
            links_json          TEXT,
            content_hash        TEXT,
            current_version     INTEGER DEFAULT 0,
            last_checked_at     TEXT,
            last_fetched_at     TEXT,
            created_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(source, source_key)
        );

        CREATE TABLE IF NOT EXISTS series (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            source              TEXT    NOT NULL,
            source_key          TEXT    NOT NULL,
            title               TEXT    NOT NULL,
            description         TEXT,
            cover_path          TEXT,
            content_hash        TEXT,
            current_version     INTEGER DEFAULT 0,
            last_checked_at     TEXT,
            last_fetched_at     TEXT,
            created_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(source, source_key)
        );

        CREATE TABLE IF NOT EXISTS download_people (
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            person_source       TEXT    NOT NULL,
            person_key          TEXT    NOT NULL,
            role                TEXT    NOT NULL DEFAULT 'unknown',
            display_name        TEXT    NOT NULL,
            PRIMARY KEY(download_id, person_source, person_key, role)
        );

        CREATE TABLE IF NOT EXISTS download_series (
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            series_source       TEXT    NOT NULL,
            series_key          TEXT    NOT NULL,
            title               TEXT    NOT NULL,
            content_order       INTEGER,
            PRIMARY KEY(download_id, series_source, series_key)
        );

        CREATE TABLE IF NOT EXISTS entity_versions (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            entity_type         TEXT    NOT NULL,
            source              TEXT    NOT NULL,
            source_key          TEXT    NOT NULL,
            version             INTEGER NOT NULL,
            content_hash        TEXT,
            json_path           TEXT    NOT NULL,
            asset_count         INTEGER DEFAULT 0,
            file_size_bytes     INTEGER DEFAULT 0,
            created_at          TEXT    NOT NULL,
            change_summary      TEXT,
            UNIQUE(entity_type, source, source_key, version)
        );

        CREATE TABLE IF NOT EXISTS search_index_state (
            download_id         INTEGER PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
            current_version     INTEGER NOT NULL,
            content_hash        TEXT,
            indexed_at          TEXT    NOT NULL
        );

        -- Which on-disk index format the rows above were written for. A format
        -- change starts an empty index, so without this the app would believe
        -- every work was indexed and quietly find nothing.
        CREATE TABLE IF NOT EXISTS search_index_meta (
            id                  INTEGER PRIMARY KEY CHECK (id = 1),
            index_version       TEXT    NOT NULL,
            updated_at          TEXT    NOT NULL
        );

        CREATE TABLE IF NOT EXISTS saved_searches (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            name                TEXT    NOT NULL,
            query               TEXT,
            params_json         TEXT    NOT NULL,
            created_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(name)
        );

        CREATE TABLE IF NOT EXISTS work_edit_revisions (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            base_version        INTEGER NOT NULL,
            status              TEXT    NOT NULL CHECK(status IN ('draft', 'active', 'archived')),
            title               TEXT,
            content_hash        TEXT,
            created_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS work_edit_blocks (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            edit_revision_id    INTEGER NOT NULL REFERENCES work_edit_revisions(id) ON DELETE CASCADE,
            block_order         INTEGER NOT NULL,
            block_type          TEXT    NOT NULL,
            text                TEXT,
            asset_id            INTEGER REFERENCES assets(id) ON DELETE SET NULL,
            attrs_json          TEXT,
            UNIQUE(edit_revision_id, block_order)
        );

        CREATE TABLE IF NOT EXISTS update_jobs (
            id                  TEXT PRIMARY KEY,
            scope               TEXT    NOT NULL,
            mode                TEXT    NOT NULL,
            status              TEXT    NOT NULL,
            request_json        TEXT    NOT NULL,
            totals              INTEGER NOT NULL DEFAULT 0,
            processed           INTEGER NOT NULL DEFAULT 0,
            candidate_count     INTEGER NOT NULL DEFAULT 0,
            saved_count         INTEGER NOT NULL DEFAULT 0,
            error_count         INTEGER NOT NULL DEFAULT 0,
            active_label        TEXT,
            started_at          TEXT    NOT NULL,
            updated_at          TEXT    NOT NULL,
            finished_at         TEXT
        );

        CREATE TABLE IF NOT EXISTS update_job_items (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id              TEXT    NOT NULL REFERENCES update_jobs(id) ON DELETE CASCADE,
            item_type           TEXT    NOT NULL,
            source              TEXT,
            source_id           TEXT,
            target_type         TEXT,
            title               TEXT    NOT NULL,
            payload_json        TEXT    NOT NULL,
            status              TEXT    NOT NULL,
            error               TEXT,
            result_download_id  INTEGER,
            created_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS update_job_logs (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id              TEXT    NOT NULL REFERENCES update_jobs(id) ON DELETE CASCADE,
            log_type            TEXT    NOT NULL,
            message             TEXT    NOT NULL,
            created_at          TEXT    NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS schema_v2_marker (
            version             INTEGER PRIMARY KEY,
            initialized_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        INSERT OR REPLACE INTO schema_v2_marker (version) VALUES (2);

        CREATE TRIGGER IF NOT EXISTS downloads_search_ad AFTER DELETE ON downloads BEGIN
            DELETE FROM search_index_state WHERE download_id = old.id;
        END;

        CREATE INDEX IF NOT EXISTS idx_downloads_source_type_date ON downloads(source, content_type, downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_favorite_date    ON downloads(favorite, downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_author           ON downloads(author_name);
        CREATE INDEX IF NOT EXISTS idx_downloads_author_nocase    ON downloads(author_name COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_downloads_text_length      ON downloads(text_length);
        CREATE INDEX IF NOT EXISTS idx_downloads_date             ON downloads(downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_source_created   ON downloads(source_created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_title            ON downloads(title COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_downloads_size             ON downloads(file_size_bytes DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_watch_date       ON downloads(watch_updates, downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_source_id        ON downloads(source, source_id);
        -- Every library ordering uses id as its deterministic tie-breaker.
        -- Without the same second key SQLite materializes a temporary B-tree
        -- for ties, which becomes visible on six-figure libraries.
        CREATE INDEX IF NOT EXISTS idx_downloads_source_type_date_id
            ON downloads(source, content_type, downloaded_at DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_favorite_date_id
            ON downloads(favorite, downloaded_at DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_date_id
            ON downloads(downloaded_at DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_title_id
            ON downloads(title COLLATE NOCASE DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_author_id_sort
            ON downloads(author_name COLLATE NOCASE DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_text_length_id
            ON downloads(text_length DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_size_id
            ON downloads(file_size_bytes DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_watch_date_id
            ON downloads(watch_updates, downloaded_at DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_published_id
            ON downloads(COALESCE(source_created_at, downloaded_at) DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_updated_id
            ON downloads(COALESCE(source_updated_at, source_created_at, downloaded_at) DESC, id DESC);
        -- Groups the author listing and lets its per-author newest-title
        -- lookup seek instead of scanning. The COALESCE must match the query
        -- expression exactly for SQLite to use the index.
        CREATE INDEX IF NOT EXISTS idx_downloads_author_recent
            ON downloads(source, author_id, COALESCE(source_created_at, downloaded_at) DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_tags_name                  ON tags(name);
        CREATE INDEX IF NOT EXISTS idx_download_tags_tag          ON download_tags(tag_id);
        CREATE INDEX IF NOT EXISTS idx_download_tags_download     ON download_tags(download_id);
        CREATE INDEX IF NOT EXISTS idx_assets_download            ON assets(download_id);
        -- Diagnostics streams the filesystem and probes one local path at a
        -- time. Without this index a large asset library becomes one full
        -- table scan per file, while loading all paths into RAM is unbounded.
        CREATE INDEX IF NOT EXISTS idx_assets_local_path          ON assets(local_path);
        CREATE INDEX IF NOT EXISTS idx_assets_mime_download       ON assets(mime_type, download_id);
        CREATE INDEX IF NOT EXISTS idx_assets_type                ON assets(asset_type);
        CREATE INDEX IF NOT EXISTS idx_versions_download          ON download_versions(download_id);

        CREATE INDEX IF NOT EXISTS idx_people_source_key ON people(source, source_key);
        CREATE INDEX IF NOT EXISTS idx_series_source_key ON series(source, source_key);
        CREATE INDEX IF NOT EXISTS idx_download_people_lookup ON download_people(person_source, person_key);
        CREATE INDEX IF NOT EXISTS idx_download_people_download ON download_people(download_id);
        CREATE INDEX IF NOT EXISTS idx_download_series_lookup ON download_series(series_source, series_key);
        CREATE INDEX IF NOT EXISTS idx_download_series_download ON download_series(download_id);
        CREATE INDEX IF NOT EXISTS idx_download_series_title_nocase ON download_series(title COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_entity_versions_lookup ON entity_versions(entity_type, source, source_key, version DESC);

        CREATE INDEX IF NOT EXISTS idx_update_targets_type_enabled ON update_targets(target_type, enabled);
        CREATE INDEX IF NOT EXISTS idx_update_targets_source_key ON update_targets(source, source_key);
        CREATE INDEX IF NOT EXISTS idx_download_relations_lookup ON download_relations(relation_type, source, relation_id);
        CREATE INDEX IF NOT EXISTS idx_download_relations_download ON download_relations(download_id);
        CREATE INDEX IF NOT EXISTS idx_search_index_state_version ON search_index_state(current_version, content_hash);
        CREATE INDEX IF NOT EXISTS idx_saved_searches_updated ON saved_searches(updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_work_edit_revisions_download_status
            ON work_edit_revisions(download_id, status, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_work_edit_blocks_revision_order
            ON work_edit_blocks(edit_revision_id, block_order);
        CREATE INDEX IF NOT EXISTS idx_update_job_items_job_status
            ON update_job_items(job_id, status, item_type, id);
        CREATE INDEX IF NOT EXISTS idx_update_job_items_candidate
            ON update_job_items(job_id, item_type, status);
        CREATE INDEX IF NOT EXISTS idx_update_job_logs_job_id
            ON update_job_logs(job_id, id DESC);
        CREATE INDEX IF NOT EXISTS idx_update_jobs_status_updated
            ON update_jobs(status, updated_at DESC);
        ",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn initializes_official_v2_schema() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();

        let user_version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, SCHEMA_VERSION);
        assert!(is_official_v2_schema(&conn).unwrap());
        assert!(!column_exists(&conn, "downloads", "tags").unwrap());
        assert!(sqlite_object_exists(&conn, "table", "saved_searches").unwrap());
    }

    /// あとから増えた列は、すでにあるライブラリにも足される。
    /// 保存済みの行は消えず、新しい列は空のまま（＝従来どおりの確認になる）。
    #[test]
    fn an_existing_library_gains_the_columns_added_later() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn.execute_batch(
            "ALTER TABLE downloads DROP COLUMN meta_hash;
             ALTER TABLE downloads DROP COLUMN last_deep_checked_at;
             INSERT INTO downloads (source, source_id, title, author_name, author_id,
                                    content_type, json_path, downloaded_at)
             VALUES ('pixiv', '1', '既存の作品', '作者', '7', 'novel', 'a.json', '2026-08-01');",
        )
        .unwrap();
        assert!(!column_exists(&conn, "downloads", "meta_hash").unwrap());

        initialize(&conn).unwrap();

        assert!(column_exists(&conn, "downloads", "meta_hash").unwrap());
        assert!(column_exists(&conn, "downloads", "last_deep_checked_at").unwrap());
        let (title, hash): (String, Option<String>) = conn
            .query_row(
                "SELECT title, meta_hash FROM downloads WHERE source_id = '1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "既存の作品");
        assert_eq!(hash, None);
    }

    #[test]
    fn library_sort_indexes_include_the_id_tie_breaker() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();

        for (name, first_column) in [
            ("idx_downloads_date_id", "downloaded_at"),
            ("idx_downloads_title_id", "title"),
            ("idx_downloads_author_id_sort", "author_name"),
            ("idx_downloads_text_length_id", "text_length"),
            ("idx_downloads_size_id", "file_size_bytes"),
        ] {
            let columns = conn
                .prepare(&format!("PRAGMA index_info('{name}')"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(columns, vec![first_column, "id"], "index {name}");
        }

        let plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT id FROM downloads
                 ORDER BY downloaded_at DESC, id DESC LIMIT 60",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("idx_downloads_date_id"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }

    /// A release that adds a table must add it to existing libraries, not
    /// decide those libraries are unrecognisable.
    ///
    /// This is not hypothetical: adding `search_index_meta` to REQUIRED_TABLES
    /// made every existing v2 database fail recognition, so opening the app
    /// archived the user's library and started an empty one.
    #[test]
    fn a_v2_database_missing_a_newly_added_table_is_still_recognised() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO downloads (
                source, source_id, title, author_name, author_id, content_type,
                excerpt, cover_path, json_path, original_json_path, asset_count,
                file_size_bytes, downloaded_at, source_created_at, content_hash,
                text_length, source_updated_at, watch_updates, current_version, favorite
             ) VALUES (
                'pixiv', '7', 'kept work', 'author', 'a1', 'novel', NULL, NULL,
                '/tmp/kept.json', NULL, 0, 0, '2026-01-01T00:00:00Z', NULL,
                NULL, 100, NULL, 0, 1, 0
             );",
        )
        .unwrap();

        // Stand in for a library saved by an earlier release: everything the
        // schema has ever required is present, the newest table is not.
        conn.execute_batch("DROP TABLE search_index_meta;").unwrap();
        assert!(
            is_official_v2_schema(&conn).unwrap(),
            "a library from an earlier release must still be recognised"
        );

        initialize(&conn).unwrap();
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 1, "reopening must never discard saved works");
        assert!(
            sqlite_object_exists(&conn, "table", "search_index_meta").unwrap(),
            "the new table must be added in place"
        );
    }

    #[test]
    fn migrates_official_v1_without_losing_downloads() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_v2_schema(&conn).unwrap();
        conn.execute_batch(
            "
            DROP TABLE saved_searches;
            DROP TABLE schema_v2_marker;
            CREATE TABLE schema_v1_marker (
                version INTEGER PRIMARY KEY,
                initialized_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO schema_v1_marker (version) VALUES (1);
            PRAGMA user_version = 1;
            INSERT INTO downloads (
                source, source_id, title, author_name, author_id, content_type,
                excerpt, cover_path, json_path, original_json_path, asset_count,
                file_size_bytes, downloaded_at, source_created_at, content_hash,
                text_length, source_updated_at, watch_updates, current_version, favorite
            ) VALUES (
                'pixiv', '42', 'kept', 'author', 'a1', 'novel', NULL, NULL,
                '/tmp/kept.json', NULL, 0, 0, '2026-01-01T00:00:00Z', NULL,
                NULL, 100, NULL, 0, 1, 0
            );
            ",
        )
        .unwrap();

        initialize(&conn).unwrap();

        assert!(is_official_v2_schema(&conn).unwrap());
        let title: String = conn
            .query_row(
                "SELECT title FROM downloads WHERE source_id = '42'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "kept");
    }

    #[test]
    fn incompatible_preview_schema_is_recreated() {
        let root =
            std::env::temp_dir().join(format!("piep_schema_reset_{}", rand::random::<u32>()));
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("piep.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                PRAGMA user_version = 2;
                CREATE TABLE downloads (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    author_name TEXT NOT NULL,
                    author_id TEXT NOT NULL,
                    content_type TEXT NOT NULL,
                    tags TEXT,
                    json_path TEXT NOT NULL,
                    downloaded_at TEXT NOT NULL
                );
                CREATE TABLE schema_v1_marker (
                    version INTEGER PRIMARY KEY,
                    initialized_at TEXT DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO schema_v1_marker (version) VALUES (2);
                INSERT INTO downloads (
                    source, source_id, title, author_name, author_id, content_type,
                    tags, json_path, downloaded_at
                ) VALUES (
                    'pixiv', '1', 'old', 'author', 'a1', 'novel',
                    '[\"legacy\"]', '/tmp/old.json', '2026-01-01T00:00:00Z'
                );
                ",
            )
            .unwrap();
        }

        let conn = Connection::open(&db_path).unwrap();
        initialize(&conn).unwrap();
        assert!(is_official_v2_schema(&conn).unwrap());
        assert!(!column_exists(&conn, "downloads", "tags").unwrap());
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        assert!(fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains("preview-backup")));

        let _ = fs::remove_dir_all(root);
    }
}
