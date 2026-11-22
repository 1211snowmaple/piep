//! 端末の記録を一つの SQLite に置く。
//!
//! 読書位置（端末内と棚との同期用）、「続きを読む」、保存した作品の本文の索引をここに持つ。
//! 本文と画像そのものはファイルのまま置き、ここは検索と記録のための台帳にする。
//! 食い違ったときはファイルを正として作り直せるようにする。
//!
//! 表の形は `PRAGMA user_version` で数え、上げるときは一つの transaction で行う。
//! 途中で失敗すれば何も変わらず、次に開いたときにもう一度試す。

use super::*;
use rusqlite::{Connection, Transaction};
use std::path::Path;

const DB_FILE: &str = "piep.sqlite3";
const SCHEMA_VERSION: i64 = 6;

/// 版 6: 旧 API の読書位置の同期（版 1）は使わない。表を捨てる。
const DROP_SYNC_SCHEMA: &str = "DROP TABLE IF EXISTS sync_meta;
    DROP TABLE IF EXISTS positions;
    DROP TABLE IF EXISTS pending;";

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

/// 版 4: 保存した作品を置き場の名前（`work-{番号}` か `shelf-{UUID}`）で引く。棚の作品は
/// 番号を持たないため。索引は保存版から作り直せる控えなので、古い表は捨てて作り直す。
const OFFLINE_KEY_SCHEMA: &str = "DROP TABLE IF EXISTS offline_catalog;
    DROP TABLE IF EXISTS offline_text;
    CREATE TABLE offline_catalog (
    work_key TEXT PRIMARY KEY,
    generation TEXT NOT NULL,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL);
    CREATE VIRTUAL TABLE offline_text USING fts5(text, work_key UNINDEXED, page UNINDEXED, tokenize='trigram');";

pub struct DeviceDb {
    path: PathBuf,
    connection: Mutex<Option<Connection>>,
}

pub(super) fn records_error(_: rusqlite::Error) -> ClientError {
    ClientError::device_records()
}

impl DeviceDb {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(DB_FILE),
            connection: Mutex::new(None),
        }
    }

    /// 試験用。ファイルを作らない。
    #[cfg(test)]
    pub(super) fn in_memory() -> Self {
        Self {
            path: PathBuf::from(":memory:"),
            connection: Mutex::new(None),
        }
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
        let tx = db.transaction().map_err(records_error)?;
        // 版 1 は旧 API の読書位置の同期だった。版 6 で捨てるので、新しく作るときは置かない。
        if version < 2 {
            tx.execute_batch(READING_SCHEMA).map_err(records_error)?;
        }
        if version < 3 {
            tx.execute_batch(OFFLINE_SCHEMA).map_err(records_error)?;
        }
        if version < 4 {
            tx.execute_batch(OFFLINE_KEY_SCHEMA)
                .map_err(records_error)?;
        }
        // 版 5: 棚の読書位置の同期（作品は棚の UUID で見分ける）。
        if version < 5 {
            tx.execute_batch(super::shelf_reading::SHELF_READING_SCHEMA)
                .map_err(records_error)?;
        }
        if version < 6 {
            tx.execute_batch(DROP_SYNC_SCHEMA).map_err(records_error)?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(records_error)?;
        tx.commit().map_err(records_error)
    }
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
            "readings",
            "offline_catalog",
            "offline_text",
            "shelf_reading_meta",
        ] {
            count(&db, table);
        }
    }

    #[test]
    fn the_old_reading_sync_tables_are_dropped() {
        let dir = TempDir::new("drop-sync");
        {
            let old = Connection::open(dir.0.join(DB_FILE)).unwrap();
            old.execute_batch(
                "CREATE TABLE sync_meta (id INTEGER PRIMARY KEY, url TEXT, device_id TEXT, cursor INTEGER);
                 CREATE TABLE positions (key TEXT PRIMARY KEY, data TEXT NOT NULL);
                 CREATE TABLE pending (key TEXT PRIMARY KEY, data TEXT NOT NULL);
                 INSERT INTO positions VALUES('pixiv:1', '{}');",
            )
            .unwrap();
            old.pragma_update(None, "user_version", 5).unwrap();
        }
        let db = DeviceDb::new(&dir.0);
        let left: i64 = db
            .transaction(|tx| {
                tx.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('sync_meta', 'positions', 'pending')",
                    [],
                    |row| row.get(0),
                )
                .map_err(records_error)
            })
            .unwrap();
        assert_eq!(left, 0);
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
