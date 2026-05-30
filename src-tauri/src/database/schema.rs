//! データベーススキーマ定義とマイグレーション。

use rusqlite::Connection;

/// 現在のスキーマバージョン
const SCHEMA_VERSION: u32 = 4;

/// データベースのテーブルを作成・更新する
pub fn initialize(conn: &Connection) -> Result<(), rusqlite::Error> {
    // WALモードを有効化（並行読み取りのパフォーマンス向上）
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let current_version: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap_or(0);

    if current_version < 1 {
        // 後方互換性を無視してスキーマを完全クリーンアップするため、
        // 既存の古いテーブル群がある場合は完全にドロップします。
        conn.execute_batch(
            "
            PRAGMA foreign_keys = OFF;
            DROP TABLE IF EXISTS downloads_fts;
            DROP TABLE IF EXISTS download_versions;
            DROP TABLE IF EXISTS assets;
            DROP TABLE IF EXISTS download_tags;
            DROP TABLE IF EXISTS tags;
            DROP TABLE IF EXISTS downloads;
            PRAGMA foreign_keys = ON;
            ",
        )?;
        migrate_v1(conn)?;
    }
    if current_version < 2 {
        migrate_v2(conn)?;
    }
    if current_version < 3 {
        migrate_v3(conn)?;
    }
    if current_version < 4 {
        migrate_v4(conn)?;
    }

    ensure_performance_indexes(conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// v4: 本文・タグ・シリーズを含むライブラリ Smart Search 用インデックス
fn migrate_v4(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        DROP TRIGGER IF EXISTS downloads_ai;
        DROP TRIGGER IF EXISTS downloads_ad;
        DROP TRIGGER IF EXISTS downloads_au;
        DROP TABLE IF EXISTS downloads_fts;

        CREATE VIRTUAL TABLE IF NOT EXISTS download_search_fts USING fts5(
            download_id UNINDEXED,
            title,
            author_name,
            tags,
            series_title,
            excerpt,
            body,
            tokenize='unicode61 remove_diacritics 2'
        );

        CREATE TABLE IF NOT EXISTS download_search_ngrams (
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            token               TEXT    NOT NULL,
            field               TEXT    NOT NULL,
            weight              REAL    NOT NULL DEFAULT 1.0,
            PRIMARY KEY(download_id, token, field)
        );

        CREATE TABLE IF NOT EXISTS download_search_meta (
            download_id         INTEGER PRIMARY KEY REFERENCES downloads(id) ON DELETE CASCADE,
            current_version     INTEGER NOT NULL,
            content_hash        TEXT,
            indexed_at          TEXT    NOT NULL
        );

        CREATE TRIGGER IF NOT EXISTS downloads_search_ad AFTER DELETE ON downloads BEGIN
            DELETE FROM download_search_fts WHERE download_id = old.id;
            DELETE FROM download_search_ngrams WHERE download_id = old.id;
            DELETE FROM download_search_meta WHERE download_id = old.id;
        END;

        CREATE INDEX IF NOT EXISTS idx_download_search_ngrams_token ON download_search_ngrams(token, download_id);
        CREATE INDEX IF NOT EXISTS idx_download_search_ngrams_download ON download_search_ngrams(download_id);
        CREATE INDEX IF NOT EXISTS idx_download_search_meta_version ON download_search_meta(current_version, content_hash);
        ",
    )?;
    Ok(())
}

/// v3: 作者/クリエイターとシリーズを作品から独立したエンティティとして管理
fn migrate_v3(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
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

        INSERT OR IGNORE INTO people (
            source, source_key, display_name, current_version, created_at, updated_at
        )
        SELECT DISTINCT source, author_id, author_name, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        FROM downloads
        WHERE author_id IS NOT NULL AND author_id != '';

        INSERT OR IGNORE INTO download_people (
            download_id, person_source, person_key, role, display_name
        )
        SELECT id, source, author_id, 'author', author_name
        FROM downloads
        WHERE author_id IS NOT NULL AND author_id != '';

        INSERT OR IGNORE INTO series (
            source, source_key, title, current_version, created_at, updated_at
        )
        SELECT DISTINCT source, relation_id, relation_name, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        FROM download_relations
        WHERE relation_type = 'series' AND relation_id IS NOT NULL AND relation_id != '';

        INSERT OR IGNORE INTO download_series (
            download_id, series_source, series_key, title
        )
        SELECT download_id, source, relation_id, relation_name
        FROM download_relations
        WHERE relation_type = 'series' AND relation_id IS NOT NULL AND relation_id != '';

        CREATE INDEX IF NOT EXISTS idx_people_source_key ON people(source, source_key);
        CREATE INDEX IF NOT EXISTS idx_series_source_key ON series(source, source_key);
        CREATE INDEX IF NOT EXISTS idx_download_people_lookup ON download_people(person_source, person_key);
        CREATE INDEX IF NOT EXISTS idx_download_people_download ON download_people(download_id);
        CREATE INDEX IF NOT EXISTS idx_download_series_lookup ON download_series(series_source, series_key);
        CREATE INDEX IF NOT EXISTS idx_download_series_download ON download_series(download_id);
        CREATE INDEX IF NOT EXISTS idx_entity_versions_lookup ON entity_versions(entity_type, source, source_key, version DESC);
        ",
    )?;
    Ok(())
}

/// v2: 更新管理用の購読対象と作品メタ関係
fn migrate_v2(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
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

        CREATE INDEX IF NOT EXISTS idx_update_targets_type_enabled ON update_targets(target_type, enabled);
        CREATE INDEX IF NOT EXISTS idx_update_targets_source_key ON update_targets(source, source_key);
        CREATE INDEX IF NOT EXISTS idx_download_relations_lookup ON download_relations(relation_type, source, relation_id);
        CREATE INDEX IF NOT EXISTS idx_download_relations_download ON download_relations(download_id);
        ",
    )?;
    Ok(())
}

/// v1: 最新の極限最適化スキーマ定義
fn migrate_v1(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        -- 1. ダウンロード小説メインテーブル
        CREATE TABLE IF NOT EXISTS downloads (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            source              TEXT    NOT NULL,
            source_id           TEXT    NOT NULL,
            title               TEXT    NOT NULL,
            author_name         TEXT    NOT NULL,
            author_id           TEXT    NOT NULL,
            content_type        TEXT    NOT NULL,
            tags                TEXT, -- フロント互換性JSON文字列として保持しつつ、中間テーブルでも正規化管理
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
            UNIQUE(source, source_id)
        );

        -- 2. タグマスタテーブル
        CREATE TABLE IF NOT EXISTS tags (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            name                TEXT    NOT NULL UNIQUE
        );

        -- 3. 小説-タグ多対多中間テーブル
        CREATE TABLE IF NOT EXISTS download_tags (
            download_id         INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
            tag_id              INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY(download_id, tag_id)
        );

        -- 4. アセットテーブル
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

        -- 5. バージョン管理テーブル
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

        -- 6. FTS5 全文検索用仮想テーブル
        CREATE VIRTUAL TABLE IF NOT EXISTS downloads_fts USING fts5(
            download_id UNINDEXED,
            title,
            author_name,
            excerpt,
            tokenize='unicode61'
        );

        -- 7. 自動同期トリガー (FTS5用)
        CREATE TRIGGER IF NOT EXISTS downloads_ai AFTER INSERT ON downloads BEGIN
            INSERT INTO downloads_fts(download_id, title, author_name, excerpt)
            VALUES(new.id, new.title, new.author_name, new.excerpt);
        END;

        CREATE TRIGGER IF NOT EXISTS downloads_ad AFTER DELETE ON downloads BEGIN
            DELETE FROM downloads_fts WHERE download_id = old.id;
        END;

        CREATE TRIGGER IF NOT EXISTS downloads_au AFTER UPDATE ON downloads BEGIN
            UPDATE downloads_fts SET
                title = new.title,
                author_name = new.author_name,
                excerpt = new.excerpt
            WHERE download_id = old.id;
        END;

        -- 8. インデックスの極限最適化
        CREATE INDEX IF NOT EXISTS idx_downloads_source_type_date ON downloads(source, content_type, downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_favorite_date    ON downloads(favorite, downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_author           ON downloads(author_name);
        CREATE INDEX IF NOT EXISTS idx_downloads_text_length      ON downloads(text_length);
        CREATE INDEX IF NOT EXISTS idx_tags_name                  ON tags(name);
        CREATE INDEX IF NOT EXISTS idx_download_tags_tag          ON download_tags(tag_id);
        CREATE INDEX IF NOT EXISTS idx_assets_download            ON assets(download_id);
        CREATE INDEX IF NOT EXISTS idx_versions_download          ON download_versions(download_id);
        ",
    )?;
    Ok(())
}

fn ensure_performance_indexes(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_downloads_date             ON downloads(downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_source_created   ON downloads(source_created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_title            ON downloads(title COLLATE NOCASE);
        CREATE INDEX IF NOT EXISTS idx_downloads_size             ON downloads(file_size_bytes DESC);
        CREATE INDEX IF NOT EXISTS idx_downloads_watch_date       ON downloads(watch_updates, downloaded_at DESC);
        CREATE INDEX IF NOT EXISTS idx_download_tags_download     ON download_tags(download_id);
        CREATE INDEX IF NOT EXISTS idx_assets_mime_download       ON assets(mime_type, download_id);
        ",
    )?;
    Ok(())
}
