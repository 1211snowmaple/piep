//! 端末の記録を一つの SQLite に置く。
//!
//! 読書位置（端末内と棚との同期用）、「続きを読む」、保存した作品の本文の索引をここに持つ。
//! 本文と画像そのものはファイルのまま置き、ここは検索と記録のための台帳にする。
//! 食い違ったときはファイルを正として作り直せるようにする。
//!
//! 表の形は `PRAGMA user_version` で数える。形の違う古い記録は移さずに作り直し、
//! 新しい版で作った記録は書き換えない。

use super::*;
use rusqlite::{Connection, Transaction};
use std::path::Path;

const DB_FILE: &str = "piep.sqlite3";
const SCHEMA_VERSION: i64 = 7;

/// 端末内の読書位置と「続きを読む」、保存した作品の本文の索引。
///
/// 読書位置の作品は取得元と取得元の ID で見分け、棚に出すための接続先は別に持つ。
/// 本文の索引は日本語に語の切れ目が無いので、3 文字ずつの trigram で引く。置き場の名前
/// （`shelf-{UUID}`）で作品を指す。
const SCHEMA: &str = "CREATE TABLE readings (
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
    work TEXT);
    CREATE INDEX readings_shelf ON readings(server_url, read_at DESC) WHERE shelved = 1 AND finished = 0;
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
        let mut db = self.connect()?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(records_error)?;
        if version > SCHEMA_VERSION {
            // 新しい版で作った記録を古い版で書き換えない。
            return Err(ClientError::device_records());
        }
        if version != 0 && version < SCHEMA_VERSION {
            // 試しの段階なので、形の違う記録は移さずに作り直す。
            drop(db);
            self.remove_files()?;
            db = self.connect()?;
        }
        if version != SCHEMA_VERSION {
            let tx = db.transaction().map_err(records_error)?;
            tx.execute_batch(SCHEMA).map_err(records_error)?;
            tx.execute_batch(super::shelf_reading::SHELF_READING_SCHEMA)
                .map_err(records_error)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(records_error)?;
            tx.commit().map_err(records_error)?;
        }
        Ok(db)
    }

    fn connect(&self) -> Result<Connection, ClientError> {
        let db = Connection::open(&self.path).map_err(records_error)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(records_error)?;
        Ok(db)
    }

    fn remove_files(&self) -> Result<(), ClientError> {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            match std::fs::remove_file(PathBuf::from(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(ClientError::device_records()),
            }
        }
        Ok(())
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
    fn records_of_an_older_shape_are_made_again() {
        let dir = TempDir::new("older");
        {
            let old = Connection::open(dir.0.join(DB_FILE)).unwrap();
            old.execute_batch("CREATE TABLE readings (key TEXT PRIMARY KEY, work_id INTEGER);")
                .unwrap();
            old.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
                .unwrap();
        }
        let db = DeviceDb::new(&dir.0);
        let old_columns: i64 = db
            .transaction(|tx| {
                tx.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('readings') WHERE name = 'work_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(records_error)
            })
            .unwrap();
        assert_eq!(old_columns, 0);
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
