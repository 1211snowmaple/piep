//! 端末の記録を一つの SQLite に置く。
//!
//! 読書位置（端末内と同期用）、「続きを読む」、保存した作品の本文の索引をここに持つ。
//! 本文と画像そのものはファイルのまま置き、ここは検索と記録のための台帳にする。
//! 食い違ったときはファイルを正として作り直せるようにする。
//!
//! 表の形は `PRAGMA user_version` で数え、上げるときは一つの transaction で行う。
//! 途中で失敗すれば何も変わらず、次に開いたときにもう一度試す。

use super::*;
use rusqlite::{Connection, Transaction};
use std::path::Path;

const DB_FILE: &str = "piep.sqlite3";
const SCHEMA_VERSION: i64 = 3;

/// 版 1: 読書位置の同期。0.5.1 では `reading-sync.sqlite3` に置いていた。
pub(super) const SYNC_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS sync_meta (
    id INTEGER PRIMARY KEY CHECK(id=1), url TEXT NOT NULL, device_id TEXT NOT NULL, cursor INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS positions (key TEXT PRIMARY KEY, data TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS pending (key TEXT PRIMARY KEY, data TEXT NOT NULL);";

/// 版 2: 端末内の読書位置と「続きを読む」。作品は取得元と取得元の ID で見分ける。
/// サーバーの作品番号はサーバーごとに振られるので、棚に出すための接続先と番号は別に持つ。
const READING_SCHEMA: &str = "CREATE TABLE readings (
    key TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    content_hash TEXT,
    edit_hash TEXT,
    page INTEGER NOT NULL,
    position TEXT,
    progress REAL NOT NULL,
    finished INTEGER NOT NULL,
    shelved INTEGER NOT NULL,
    read_at INTEGER NOT NULL,
    server_url TEXT,
    work_id INTEGER,
    work TEXT);
    CREATE INDEX readings_shelf ON readings(server_url, read_at DESC) WHERE shelved = 1 AND finished = 0;";

/// 版 3: 保存した作品の本文の索引。日本語は語の切れ目が無いので、3 文字ずつの trigram で引く。
const OFFLINE_SCHEMA: &str = "CREATE TABLE offline_catalog (
    work_id INTEGER PRIMARY KEY,
    generation TEXT NOT NULL,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL);
    CREATE VIRTUAL TABLE offline_text USING fts5(text, work_id UNINDEXED, page UNINDEXED, tokenize='trigram');";

pub struct DeviceDb {
    path: PathBuf,
    legacy_sync_json: PathBuf,
    legacy_sync_db: PathBuf,
    connection: Mutex<Option<Connection>>,
}

pub(super) fn records_error(_: rusqlite::Error) -> ClientError {
    ClientError::device_records()
}

impl DeviceDb {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(DB_FILE),
            legacy_sync_json: dir.join("reading-sync.json"),
            legacy_sync_db: dir.join("reading-sync.sqlite3"),
            connection: Mutex::new(None),
        }
    }

    /// 試験用。ファイルを作らない。
    #[cfg(test)]
    pub(super) fn in_memory() -> Self {
        let nowhere = PathBuf::from("piep-test-no-legacy-records");
        Self {
            path: PathBuf::from(":memory:"),
            legacy_sync_json: nowhere.join("reading-sync.json"),
            legacy_sync_db: nowhere.join("reading-sync.sqlite3"),
            connection: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(super) fn legacy_sync_json(&self) -> &Path {
        &self.legacy_sync_json
    }

    #[cfg(test)]
    pub(super) fn legacy_sync_db(&self) -> &Path {
        &self.legacy_sync_db
    }

    /// 試験用。これまでに書いた行の数。
    #[cfg(test)]
    pub(super) fn total_changes(&self) -> u64 {
        let connection = self.connection.lock().unwrap();
        connection.as_ref().map_or(0, Connection::total_changes)
    }

    #[cfg(test)]
    pub(super) fn checkpoint(&self) {
        let connection = self.connection.lock().unwrap();
        connection
            .as_ref()
            .unwrap()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }

    /// 一つの transaction で読み書きする。失敗したら何も書かない。
    pub(super) fn transaction<T>(
        &self,
        action: impl FnOnce(&Transaction<'_>) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ClientError::device_records())?;
        if connection.is_none() {
            *connection = Some(self.open()?);
        }
        let db = connection.as_mut().expect("connection opened");
        let tx = db.transaction().map_err(records_error)?;
        let result = action(&tx)?;
        tx.commit().map_err(records_error)?;
        Ok(result)
    }

    fn open(&self) -> Result<Connection, ClientError> {
        let mut db = Connection::open(&self.path).map_err(records_error)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(records_error)?;
        self.migrate(&mut db)?;
        Ok(db)
    }

    fn migrate(&self, db: &mut Connection) -> Result<(), ClientError> {
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(records_error)?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        if version > SCHEMA_VERSION {
            // 新しい版で作った記録を古い版で書き換えない。
            return Err(ClientError::device_records());
        }
        // ATTACH は transaction の中では使えないので先に付け、終わったら外す。
        let legacy_db = version < 1 && self.legacy_sync_db.is_file();
        if legacy_db {
            db.execute(
                "ATTACH DATABASE ?1 AS legacy",
                [self.legacy_sync_db.to_string_lossy()],
            )
            .map_err(records_error)?;
        }
        let migrated = (|| {
            let tx = db.transaction().map_err(records_error)?;
            if version < 1 {
                tx.execute_batch(SYNC_SCHEMA).map_err(records_error)?;
                let copied = legacy_db && copy_legacy_sync_db(&tx)?;
                if !copied {
                    reading_sync::import_legacy_json(&tx, &self.legacy_sync_json)?;
                }
            }
            if version < 2 {
                tx.execute_batch(READING_SCHEMA).map_err(records_error)?;
            }
            if version < 3 {
                tx.execute_batch(OFFLINE_SCHEMA).map_err(records_error)?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(records_error)?;
            tx.commit().map_err(records_error)
        })();
        if legacy_db {
            // 元の記録は消さずに残す。移し終えれば以後は読まない。
            let _ = db.execute("DETACH DATABASE legacy", []);
        }
        migrated
    }
}

/// 0.5.1 の同期の記録を写す。中身が無ければ false を返し、その前の JSON から移す。
fn copy_legacy_sync_db(tx: &Transaction<'_>) -> Result<bool, ClientError> {
    let initialized: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM legacy.sqlite_master WHERE type='table' AND name='sync_meta')
             AND EXISTS(SELECT 1 FROM legacy.sync_meta)",
            [],
            |row| row.get(0),
        )
        .map_err(records_error)?;
    if !initialized {
        return Ok(false);
    }
    tx.execute_batch(
        "INSERT INTO sync_meta SELECT id, url, device_id, cursor FROM legacy.sync_meta;
         INSERT INTO positions SELECT key, data FROM legacy.positions;
         INSERT INTO pending SELECT key, data FROM legacy.pending;",
    )
    .map_err(records_error)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("piep-device-db-{name}-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn count(db: &DeviceDb, table: &str) -> i64 {
        db.transaction(|tx| {
            tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(records_error)
        })
        .unwrap()
    }

    #[test]
    fn a_new_database_gets_every_table_at_the_current_version() {
        let dir = TempDir::new("fresh");
        let db = DeviceDb::new(&dir.0);
        let version: i64 = db
            .transaction(|tx| {
                tx.query_row("PRAGMA user_version", [], |row| row.get(0))
                    .map_err(records_error)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        for table in [
            "sync_meta",
            "positions",
            "pending",
            "readings",
            "offline_catalog",
            "offline_text",
        ] {
            count(&db, table);
        }
        assert_eq!(count(&db, "sync_meta"), 1);
    }

    #[test]
    fn the_0_5_1_sync_database_is_copied_and_left_in_place() {
        let dir = TempDir::new("legacy-db");
        {
            let legacy = Connection::open(dir.0.join("reading-sync.sqlite3")).unwrap();
            legacy.execute_batch(SYNC_SCHEMA).unwrap();
            legacy
                .execute_batch(
                    "INSERT INTO sync_meta VALUES(1, 'https://pc.example/', 'phone', 42);
                     INSERT INTO positions VALUES('pixiv:1', '{}');
                     INSERT INTO pending VALUES('pixiv:2', '{}');",
                )
                .unwrap();
        }
        // 0.5.1 の前の JSON は、SQLite に中身があれば読まない。
        fs::write(dir.0.join("reading-sync.json"), b"not json").unwrap();
        let db = DeviceDb::new(&dir.0);
        let (url, cursor): (String, i64) = db
            .transaction(|tx| {
                tx.query_row("SELECT url, cursor FROM sync_meta", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(records_error)
            })
            .unwrap();
        assert_eq!((url.as_str(), cursor), ("https://pc.example/", 42));
        assert_eq!(count(&db, "positions"), 1);
        assert_eq!(count(&db, "pending"), 1);
        assert!(db.legacy_sync_db().exists());
    }

    #[test]
    fn a_failed_migration_writes_nothing_and_is_tried_again() {
        let dir = TempDir::new("retry");
        let db = DeviceDb::new(&dir.0);
        fs::write(db.legacy_sync_json(), b"broken").unwrap();
        assert!(db.transaction(|_| Ok(())).is_err());
        let version: i64 = Connection::open(db.path())
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 0);
        fs::remove_file(db.legacy_sync_json()).unwrap();
        db.transaction(|_| Ok(())).unwrap();
        assert_eq!(count(&db, "readings"), 0);
    }

    #[test]
    fn a_database_from_a_newer_app_is_not_rewritten() {
        let dir = TempDir::new("newer");
        Connection::open(dir.0.join(DB_FILE))
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(DeviceDb::new(&dir.0).transaction(|_| Ok(())).is_err());
    }
}
