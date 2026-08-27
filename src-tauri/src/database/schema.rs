//! Database schema definition and initialization.

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Current schema version.
///
/// The preview-era v1 and v2 numbering was folded into a single definition:
/// `create_schema` builds every table with CREATE TABLE IF NOT EXISTS, so one
/// idempotent pass brings any earlier official library up to date. Existing
/// libraries keep their rows and are simply re-stamped to this version.
const SCHEMA_VERSION: u32 = 1;

/// Stable columns that identify the `downloads` table as ours.
///
/// Table presence is not a safe fingerprint: a partially damaged library can
/// lose one auxiliary table, and that must be repaired in place rather than
/// interpreted as permission to erase every surviving row.
const DOWNLOAD_FINGERPRINT_COLUMNS: &[&str] = &[
    "id",
    "source",
    "source_id",
    "title",
    "json_path",
    "downloaded_at",
];

/// Open the database at the current schema.
///
/// Any library this app wrote - whatever version stamp it carries - is brought
/// up to date in place, so saved works are never discarded. Only a genuinely
/// foreign database is backed up and replaced.
pub fn initialize(conn: &Connection) -> Result<(), rusqlite::Error> {
    apply_pragmas(conn)?;

    if is_known_library(conn)? {
        // DDL も SQLite のトランザクション対象。列追加や値の移送の途中で
        // 起動が止まっても、「半分だけ新しいDB」を残さない。
        let tx = conn.unchecked_transaction()?;
        create_schema(&tx)?;
        add_missing_columns(&tx)?;
        retire_update_job_request_blob(&tx)?;
        stamp_schema_version(&tx)?;
        tx.commit()?;
        return Ok(());
    }

    backup_current_database(conn)?;
    reset_schema(conn)?;
    Ok(())
}

/// Recognises a database this app wrote, regardless of which build wrote it.
///
/// A valid `downloads` fingerprint or one of our schema markers is enough.
/// Missing auxiliary tables are created by `create_schema`; requiring every
/// table here would turn a recoverable partial database into an empty one.
fn is_known_library(conn: &Connection) -> Result<bool, rusqlite::Error> {
    let has_marker = sqlite_object_exists(conn, "table", "schema_marker")?
        || sqlite_object_exists(conn, "table", "schema_v1_marker")?
        || sqlite_object_exists(conn, "table", "schema_v2_marker")?;
    if !sqlite_object_exists(conn, "table", "downloads")? {
        return Ok(has_marker);
    }
    // The abandoned preview layout kept tags as a column on downloads rather
    // than in its own table. Those rows cannot be read by the current queries.
    if column_exists(conn, "downloads", "tags")? {
        return Ok(false);
    }
    for column in DOWNLOAD_FINGERPRINT_COLUMNS {
        if !column_exists(conn, "downloads", column)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Records the current version and clears the superseded markers, so a library
/// carries exactly one stamp no matter which build it came from.
fn stamp_schema_version(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS schema_v1_marker;
         DROP TABLE IF EXISTS schema_v2_marker;",
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO schema_marker (version) VALUES (?1)",
        [SCHEMA_VERSION],
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Tables introduced after the first release. They deliberately stay out of
/// `CORE_TABLES`: adding them to the recognition fingerprint would make an
/// older, otherwise valid library look foreign and get archived.
/// Builds every table the app uses. Idempotent, so it doubles as the upgrade
/// path for a library written by an earlier build.
fn create_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    create_core_tables(conn)?;
    create_additional_tables(conn)?;
    Ok(())
}

fn create_additional_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
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
            ON update_candidates(status, updated_at DESC);

         -- 利用者が作品を横断してまとめる、順序付きまたは順序なしの集合。
         -- 取得元の series や検索条件とは別の正本なので、どちらを変更しても
         -- 互いを暗黙に書き換えない。
         CREATE TABLE IF NOT EXISTS work_collections (
            id                TEXT PRIMARY KEY,
            name              TEXT NOT NULL,
            description       TEXT,
            collection_kind   TEXT NOT NULL DEFAULT 'ordered'
                                      CHECK(collection_kind IN ('ordered', 'unordered')),
            cover_download_id INTEGER REFERENCES downloads(id) ON DELETE SET NULL,
            revision          INTEGER NOT NULL DEFAULT 1,
            created_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
         );
         CREATE INDEX IF NOT EXISTS idx_work_collections_updated
            ON work_collections(updated_at DESC, name COLLATE NOCASE);

         -- source + source_id が永続的な作品参照。download_id は現在保存されている
         -- 行への解決結果に過ぎず、作品削除時は NULL になって再保存時に戻る。
         CREATE TABLE IF NOT EXISTS work_collection_members (
            collection_id  TEXT NOT NULL REFERENCES work_collections(id) ON DELETE CASCADE,
            source         TEXT NOT NULL,
            source_id      TEXT NOT NULL,
            download_id    INTEGER REFERENCES downloads(id) ON DELETE SET NULL,
            title_snapshot TEXT NOT NULL,
            author_snapshot TEXT NOT NULL,
            position       INTEGER NOT NULL,
            member_role    TEXT NOT NULL DEFAULT 'main',
            added_by       TEXT NOT NULL DEFAULT 'manual',
            pinned         INTEGER NOT NULL DEFAULT 0,
            note           TEXT,
            created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(collection_id, source, source_id)
         );
         CREATE INDEX IF NOT EXISTS idx_collection_members_order
            ON work_collection_members(collection_id, position, source, source_id);
         CREATE INDEX IF NOT EXISTS idx_collection_members_download
            ON work_collection_members(download_id);
         CREATE INDEX IF NOT EXISTS idx_collection_members_work_key
            ON work_collection_members(source, source_id);

         -- 本文・キャプション・公式シリーズ・利用者操作から得た作品間の辺。
         -- コレクション所属とは分離し、単なる言及が自動で所属にならないようにする。
         CREATE TABLE IF NOT EXISTS work_links (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            from_source      TEXT NOT NULL,
            from_source_id   TEXT NOT NULL,
            from_download_id INTEGER REFERENCES downloads(id) ON DELETE SET NULL,
            to_source        TEXT NOT NULL,
            to_source_id     TEXT NOT NULL,
            to_download_id   INTEGER REFERENCES downloads(id) ON DELETE SET NULL,
            relation_type    TEXT NOT NULL DEFAULT 'mentions',
            evidence_type    TEXT NOT NULL,
            anchor_text      TEXT,
            context_text     TEXT,
            confidence       REAL NOT NULL DEFAULT 0,
            status           TEXT NOT NULL DEFAULT 'observed'
                                      CHECK(status IN ('observed', 'accepted', 'rejected')),
            discovered_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(from_source, from_source_id, to_source, to_source_id, evidence_type)
         );
         CREATE INDEX IF NOT EXISTS idx_work_links_from
            ON work_links(from_source, from_source_id, status);
         CREATE INDEX IF NOT EXISTS idx_work_links_to
            ON work_links(to_source, to_source_id, status);
         CREATE INDEX IF NOT EXISTS idx_work_links_downloads
            ON work_links(from_download_id, to_download_id);

         -- 自動下書きは承認済みコレクションと分けて保存する。規則を更新しても
         -- 利用者の並びを勝手に変えず、却下した提案も再表示しない。
         CREATE TABLE IF NOT EXISTS collection_suggestions (
            id              TEXT PRIMARY KEY,
            seed_json       TEXT NOT NULL,
            proposed_name   TEXT NOT NULL,
            collection_kind TEXT NOT NULL DEFAULT 'ordered',
            members_json    TEXT NOT NULL,
            score           REAL NOT NULL DEFAULT 0,
            rule_version    TEXT NOT NULL,
            state           TEXT NOT NULL DEFAULT 'pending'
                                    CHECK(state IN ('pending', 'accepted', 'rejected')),
            created_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
         );
         CREATE INDEX IF NOT EXISTS idx_collection_suggestions_state
            ON collection_suggestions(state, updated_at DESC);

         -- モデルに書いてもらった覚え書き。あらすじ・前回のあらすじ・作風。
         --
         -- 本文から作るものなので作り直しが高くつく。ただし**失われても
         -- 作品は失われない**派生物なので、消えても作り直せばよい。
         -- どのモデルが書いたかを残すのは、モデルを替えたときに
         -- 古い文が混ざっていることに気づけるようにするため。
         CREATE TABLE IF NOT EXISTS ai_notes (
            subject_type  TEXT NOT NULL CHECK(subject_type IN ('work', 'person', 'collection')),
            subject_key   TEXT NOT NULL,
            note_kind     TEXT NOT NULL,
            text          TEXT NOT NULL,
            model_id      TEXT NOT NULL,
            created_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(subject_type, subject_key, note_kind)
         );

         CREATE TABLE IF NOT EXISTS collection_pair_feedback (
            left_source   TEXT NOT NULL,
            left_source_id TEXT NOT NULL,
            right_source  TEXT NOT NULL,
            right_source_id TEXT NOT NULL,
            decision      TEXT NOT NULL CHECK(decision IN ('accept', 'reject')),
            rule_version  TEXT NOT NULL,
            updated_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(left_source, left_source_id, right_source, right_source_id)
         );",
    )?;
    add_missing_columns(conn)?;
    retire_update_job_request_blob(conn)?;
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
        // 依頼の塊をやめて、走行中に要る1つだけを列にした。
        ("update_jobs", "watch_saved", "INTEGER NOT NULL DEFAULT 0"),
        // 完結しているか。NULL は「まだ聞いていない」で、連載中（0）とは別。
        // 取得元が言っていないことを、こちらで言い切らないための区別。
        ("series", "is_concluded", "INTEGER"),
        // 取得元で公開されている話数。手元の数と突き合わせるためではなく、
        // 完結の判断と同じ返事から来る値なので、一緒に覚えておく。
        ("series", "published_content_count", "INTEGER"),
        // 表紙の作り方。メンバーの表紙を並べる（mosaic）、重ねる（spine）、
        // 1作を選ぶ（single）、紋を描く（sigil）、画像を指す（file）。
        // 既定を自動にしておくと、作った直後から表紙のある棚になる。
        (
            "work_collections",
            "cover_mode",
            "TEXT NOT NULL DEFAULT 'mosaic'",
        ),
        // cover_mode = 'file' のときだけ使う。表紙用に選んだ画像の場所。
        ("work_collections", "cover_image_path", "TEXT"),
        // 名前がどこから来たか。命名規則を後で入れ替えたときに、
        // **手で直した名前を上書きしない**ための目印。
        (
            "work_collections",
            "name_source",
            "TEXT NOT NULL DEFAULT 'manual'",
        ),
        // 束の出自。sequence（読む順のある続き物）／theme（味が同じ）／manual。
        // collection_kind が「利用者がどう並べたいか」なのに対し、こちらは
        // 「どうやって見つかったか」で、後から証拠を説明するのに要る。
        (
            "work_collections",
            "track",
            "TEXT NOT NULL DEFAULT 'manual'",
        ),
        // 候補の側にも同じ区別を持たせる。画面のタブがこれで分かれる。
        (
            "collection_suggestions",
            "track",
            "TEXT NOT NULL DEFAULT 'sequence'",
        ),
        // 1作から広げたのか、棚全体の走査で見つかったのか。
        (
            "collection_suggestions",
            "origin",
            "TEXT NOT NULL DEFAULT 'seed'",
        ),
        // 「確度74%」の代わりに出す、なぜ束なのかの一行。
        ("collection_suggestions", "evidence_summary", "TEXT"),
        // 名前の案。利用者に選ばせるので、1つに決めずに持っておく。
        ("collection_suggestions", "name_options_json", "TEXT"),
        // タグの出どころ。`origin`（取得元が付けていた）／`manual`（利用者）／
        // `llm`（モデルの案を利用者が採った）。
        //
        // 混ぜてはいけない。取得元が付けたタグとモデルが足したタグは、
        // 確からしさが違う。**どちらか分からなくなった時点で、両方が信用
        // できなくなる。** 取り直しのときに消してよいのも `origin` だけである。
        (
            "download_tags",
            "tag_source",
            "TEXT NOT NULL DEFAULT 'origin'",
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

/// 依頼の塊をやめる。
///
/// `request_json` には画面から届いた要求がまるごと入っていたが、走り出した
/// あと読まれるのは `watchSaved` の一つだけだった。塊のままにしておくと、
/// 使われなくなった項目が黙って残り続け、次に形を変える人が「これは今も
/// 効いているのか」を毎回調べ直すことになる。
///
/// 値は落とさずに列へ移してから、列ごと落とす。読めない行は既定（監視しない）
/// として扱う - 掃除のために起動を止めるほどの値ではない。
fn retire_update_job_request_blob(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !column_exists(conn, "update_jobs", "request_json")? {
        return Ok(());
    }
    let carried: Vec<(String, bool)> = {
        let mut statement = conn.prepare("SELECT id, request_json FROM update_jobs")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            row.map(|(id, request_json)| {
                let watch_saved = serde_json::from_str::<serde_json::Value>(&request_json)
                    .ok()
                    .and_then(|value| value.get("watchSaved").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false);
                (id, watch_saved)
            })
        })
        .collect::<Result<Vec<_>, _>>()?
    };
    for (id, watch_saved) in carried {
        if watch_saved {
            conn.execute(
                "UPDATE update_jobs SET watch_saved = 1 WHERE id = ?1",
                rusqlite::params![id],
            )?;
        }
    }
    conn.execute_batch("ALTER TABLE update_jobs DROP COLUMN request_json;")?;
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

fn reset_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let reset_result = (|| {
        let tx = conn.unchecked_transaction()?;
        drop_sqlite_objects(&tx, "trigger", "DROP TRIGGER IF EXISTS")?;
        drop_sqlite_objects(&tx, "view", "DROP VIEW IF EXISTS")?;
        drop_sqlite_objects(&tx, "table", "DROP TABLE IF EXISTS")?;
        drop_sqlite_objects(&tx, "index", "DROP INDEX IF EXISTS")?;
        create_schema(&tx)?;
        add_missing_columns(&tx)?;
        retire_update_job_request_blob(&tx)?;
        stamp_schema_version(&tx)?;
        tx.commit()
    })();
    // 失敗時も同じ接続を外部鍵無効のまま返さない。元のエラーを
    // 優先しつつ、復帰に失敗した場合も必ず呼び出し側へ伝える。
    let foreign_keys_result = conn.execute_batch("PRAGMA foreign_keys = ON;");
    reset_result?;
    foreign_keys_result
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
fn create_core_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
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
            -- 依頼のうち、走り出したあとも要るのはこれだけ。画面から来た
            -- 要求をまるごと漬けていたころは、使われなくなった項目が
            -- そのまま残り続け、どれが今も効いているのか読めなくなった。
            watch_saved         INTEGER NOT NULL DEFAULT 0,
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

        CREATE TABLE IF NOT EXISTS schema_marker (
            version             INTEGER PRIMARY KEY,
            initialized_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

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
    fn initializes_the_current_schema() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();

        let user_version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 1);
        assert!(is_known_library(&conn).unwrap());
        let marker: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_marker", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(marker, 1);
        assert!(!column_exists(&conn, "downloads", "tags").unwrap());
        assert!(sqlite_object_exists(&conn, "table", "saved_searches").unwrap());
    }

    /// 依頼の塊をやめる移行。値は落とさずに列へ移し、塊そのものは消す。
    ///
    /// 読まれない項目を抱えた JSON が残っていると、次に形を変える人が
    /// 「これは今も効いているのか」を毎回調べ直すことになる。
    #[test]
    fn the_update_job_request_blob_is_carried_into_a_column_and_removed() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();

        // 旧ビルドの形に戻す。使われなくなった項目まで含めて再現する。
        conn.execute_batch(
            r#"ALTER TABLE update_jobs DROP COLUMN watch_saved;
               ALTER TABLE update_jobs ADD COLUMN request_json TEXT NOT NULL DEFAULT '{}';
               INSERT INTO update_jobs (id, scope, mode, status, request_json, started_at, updated_at)
               VALUES ('job-watch', 'all', 'auto_save',  'completed',
                       '{"scope":"all","mode":"auto_save","watchSaved":true,"concurrency":{"fetch":3}}',
                       '2026-08-01', '2026-08-01'),
                      ('job-plain', 'all', 'check_only', 'completed',
                       '{"scope":"all","mode":"check_only"}', '2026-08-01', '2026-08-01'),
                      ('job-broken','all', 'check_only', 'completed',
                       'not json', '2026-08-01', '2026-08-01');"#,
        )
        .unwrap();
        assert!(column_exists(&conn, "update_jobs", "request_json").unwrap());

        initialize(&conn).unwrap();

        // 塊は消え、走行中に要る一つだけが列として残る。
        assert!(!column_exists(&conn, "update_jobs", "request_json").unwrap());
        assert!(column_exists(&conn, "update_jobs", "watch_saved").unwrap());

        let watch_saved = |id: &str| -> i64 {
            conn.query_row(
                "SELECT watch_saved FROM update_jobs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(watch_saved("job-watch"), 1, "設定は移行で失われない");
        assert_eq!(watch_saved("job-plain"), 0);
        // 読めない行で起動を止めない。既定（監視しない）として扱う。
        assert_eq!(watch_saved("job-broken"), 0);

        // 二度目は何もしない。起動のたびに走っても同じ結果になる。
        initialize(&conn).unwrap();
        assert!(!column_exists(&conn, "update_jobs", "request_json").unwrap());
        assert_eq!(watch_saved("job-watch"), 1);
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
    /// This is not hypothetical: adding `search_index_meta` to CORE_TABLES
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
            is_known_library(&conn).unwrap(),
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
    fn a_library_missing_an_auxiliary_table_is_repaired_without_losing_works() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO downloads (
                source, source_id, title, author_name, author_id, content_type,
                excerpt, cover_path, json_path, original_json_path, asset_count,
                file_size_bytes, downloaded_at, source_created_at, content_hash,
                text_length, source_updated_at, watch_updates, current_version, favorite
             ) VALUES (
                'pixiv', 'repair-1', '残す作品', 'author', 'a1', 'novel', NULL,
                NULL, '/tmp/repair.json', NULL, 0, 0, '2026-01-01T00:00:00Z',
                NULL, NULL, 100, NULL, 0, 1, 0
             );
             DROP TABLE update_job_logs;",
        )
        .unwrap();

        assert!(is_known_library(&conn).unwrap());
        initialize(&conn).unwrap();

        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, 1);
        assert!(sqlite_object_exists(&conn, "table", "update_job_logs").unwrap());
    }

    #[test]
    /// 旧番号のライブラリを、行を失わずそのまま現行スキーマとして取り込む。
    /// 認識に失敗すると「見知らぬDB」と判定され、退避のうえ初期化されるため、
    /// ここは実ライブラリの安全に直結する。
    fn an_earlier_stamp_is_adopted_without_losing_downloads() {
        let conn = Connection::open_in_memory().unwrap();
        create_core_tables(&conn).unwrap();
        conn.execute_batch(
            "
            DROP TABLE saved_searches;
            DROP TABLE schema_marker;
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

        assert!(is_known_library(&conn).unwrap());
        let user_version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 1, "旧番号は現行へ振り直される");
        assert!(!sqlite_object_exists(&conn, "table", "schema_v1_marker").unwrap());
        assert!(sqlite_object_exists(&conn, "table", "saved_searches").unwrap());
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
        assert!(is_known_library(&conn).unwrap());
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
