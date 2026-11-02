//! サーバーの状態（端末、登録、ファイルの記録、読書位置）。
//!
//! 送信で入れ替わる本棚の DB とは別のファイルに置く。`piep-server init` と
//! 動いている `serve` のように別のプロセスからも触るので、SQLite にしておく。
//! 鍵はすべて SHA-256 だけを持ち、平文は発行した瞬間に返すだけで残さない。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use subtle::ConstantTimeEq;

pub const MAX_READERS: i64 = 16;
/// 読み手の時計が進みすぎていても、位置が永久に勝ち続けないようにする幅。
const MAX_CLOCK_SKEW_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Admin => "admin",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "reader" => Some(Self::Reader),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedDevice {
    pub id: String,
    pub role: Role,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    /// 最後に tailnet の中から使った利用者。認証には使わない。
    pub last_user: Option<String>,
}

#[derive(Debug)]
pub struct Paired {
    pub device_id: String,
    pub token: String,
    pub role: Role,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    Rejected,
    TooManyDevices,
    Storage(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPosition {
    pub source: String,
    pub source_id: String,
    pub version: Option<i64>,
    pub edit_hash: Option<String>,
    pub page: i64,
    pub position: f64,
    pub finished: bool,
    pub device_id: String,
    pub device_time: i64,
    pub cursor: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingUpdate {
    pub version: Option<i64>,
    pub edit_hash: Option<String>,
    pub page: i64,
    pub position: f64,
    #[serde(default)]
    pub finished: bool,
    pub device_time: i64,
}

pub struct ServerStore {
    conn: Mutex<Connection>,
}

pub fn random_token(bytes: usize) -> String {
    let mut random = vec![0_u8; bytes];
    rand::fill(random.as_mut_slice());
    URL_SAFE_NO_PAD.encode(random)
}

fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn db_error(error: rusqlite::Error) -> String {
    format!("サーバーの状態を読み書きできません: {error}")
}

impl ServerStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("サーバーの状態の置き場を作れません: {error}"))?;
        }
        let conn = Connection::open(path).map_err(db_error)?;
        conn.busy_timeout(Duration::from_secs(10))
            .map_err(db_error)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_error)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS devices (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 role TEXT NOT NULL,
                 token_hash BLOB NOT NULL,
                 created_at TEXT NOT NULL,
                 last_seen_at TEXT,
                 last_user TEXT
             );
             CREATE TABLE IF NOT EXISTS pairings (
                 secret_hash BLOB PRIMARY KEY,
                 role TEXT NOT NULL,
                 expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS files (
                 path TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 mtime_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_files (
                 session_id TEXT NOT NULL,
                 path TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ms INTEGER NOT NULL,
                 needed INTEGER NOT NULL,
                 sha256 TEXT,
                 PRIMARY KEY (session_id, path)
             );
             CREATE TABLE IF NOT EXISTS reading (
                 source TEXT NOT NULL,
                 source_id TEXT NOT NULL,
                 version INTEGER,
                 edit_hash TEXT,
                 page INTEGER NOT NULL,
                 position REAL NOT NULL,
                 finished INTEGER NOT NULL,
                 device_id TEXT NOT NULL,
                 device_time INTEGER NOT NULL,
                 cursor INTEGER NOT NULL,
                 PRIMARY KEY (source, source_id)
             );
             CREATE INDEX IF NOT EXISTS idx_reading_cursor ON reading(cursor);",
        )
        .map_err(db_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub(super) fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "サーバーの状態を開けません".to_string())
    }

    /// 一回限りの登録の鍵を発行する。平文はここで返す分だけで、どこにも残らない。
    pub fn issue_pairing(&self, role: Role, lifetime: Duration) -> Result<String, String> {
        let secret = random_token(32);
        let conn = self.connection()?;
        let now = now_ms();
        conn.execute("DELETE FROM pairings WHERE expires_at <= ?1", [now])
            .map_err(db_error)?;
        conn.execute(
            "INSERT INTO pairings (secret_hash, role, expires_at) VALUES (?1, ?2, ?3)",
            params![
                hash_secret(&secret).as_slice(),
                role.as_str(),
                now + lifetime.as_millis() as i64
            ],
        )
        .map_err(db_error)?;
        Ok(secret)
    }

    /// 鍵が正しければ、端末を登録して鍵を消す。
    ///
    /// 管理者は段階 2 まで 1 台なので、新しい管理者が登録された時点で前の管理者の
    /// 鍵を失効させる。発行の時点で失効させないのは、登録文字列を失くしたときに
    /// 締め出されないようにするためである。
    pub fn consume_pairing(&self, secret: &str, device_name: &str) -> Result<Paired, PairError> {
        let storage = |error: rusqlite::Error| PairError::Storage(db_error(error));
        let mut conn = self.connection().map_err(PairError::Storage)?;
        let now = now_ms();
        let submitted = hash_secret(secret);
        let tx = conn.transaction().map_err(storage)?;
        tx.execute("DELETE FROM pairings WHERE expires_at <= ?1", [now])
            .map_err(storage)?;
        let mut matched = None;
        {
            let mut stmt = tx
                .prepare("SELECT secret_hash, role FROM pairings")
                .map_err(storage)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage)?;
            for row in rows {
                let (hash, role) = row.map_err(storage)?;
                if bool::from(hash.as_slice().ct_eq(submitted.as_slice())) {
                    matched = Role::parse(&role);
                }
            }
        }
        let role = matched.ok_or(PairError::Rejected)?;
        if role == Role::Reader {
            let readers: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM devices WHERE role = 'reader'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            if readers >= MAX_READERS {
                return Err(PairError::TooManyDevices);
            }
        } else {
            tx.execute("DELETE FROM devices WHERE role = 'admin'", [])
                .map_err(storage)?;
        }
        tx.execute(
            "DELETE FROM pairings WHERE secret_hash = ?1",
            [submitted.as_slice()],
        )
        .map_err(storage)?;
        let device_id = random_token(16);
        let token = random_token(32);
        let name: String = device_name.trim().chars().take(64).collect();
        tx.execute(
            "INSERT INTO devices (id, name, role, token_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![device_id, name, role.as_str(), hash_secret(&token).as_slice(), chrono::Utc::now().to_rfc3339()],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(Paired {
            device_id,
            token,
            role,
        })
    }

    /// `Authorization` の鍵から端末を引く。
    pub fn authorize(&self, token: &str) -> Option<AuthorizedDevice> {
        if token.is_empty() || token.len() > 128 {
            return None;
        }
        let submitted = hash_secret(token);
        let conn = self.connection().ok()?;
        let mut stmt = conn
            .prepare_cached("SELECT id, role, token_hash, last_seen_at FROM devices")
            .ok()?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .ok()?;
        let mut found = None;
        for row in rows.flatten() {
            let (id, role, hash, seen) = row;
            if bool::from(hash.as_slice().ct_eq(submitted.as_slice())) {
                found = Role::parse(&role).map(|role| (AuthorizedDevice { id, role }, seen));
            }
        }
        drop(stmt);
        let (device, seen) = found?;
        // 要求のたびに書くと読むだけの API が書き込みを起こすので、1 分に 1 回にする。
        let now = chrono::Utc::now();
        let stale = seen
            .and_then(|seen| chrono::DateTime::parse_from_rfc3339(&seen).ok())
            .is_none_or(|seen| now.signed_duration_since(seen).num_seconds() >= 60);
        if stale {
            let _ = conn.execute(
                "UPDATE devices SET last_seen_at = ?1 WHERE id = ?2",
                params![now.to_rfc3339(), device.id],
            );
        }
        Some(device)
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceSummary>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, role, created_at, last_seen_at, last_user FROM devices ORDER BY created_at",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DeviceSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: Role::parse(&row.get::<_, String>(2)?).unwrap_or(Role::Reader),
                    created_at: row.get(3)?,
                    last_seen_at: row.get(4)?,
                    last_user: row.get(5)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }

    /// 端末を使った利用者を記録する。変わったときだけ書く。
    pub fn note_user(&self, device_id: &str, user: &str) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE devices SET last_user = ?1 WHERE id = ?2 AND last_user IS NOT ?1",
                params![user, device_id],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<bool, String> {
        let conn = self.connection()?;
        let removed = conn
            .execute("DELETE FROM devices WHERE id = ?1", [device_id])
            .map_err(db_error)?;
        Ok(removed > 0)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.connection()?;
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(db_error)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(db_error)?;
        Ok(())
    }

    /// 位置を書く。端末の時刻が持っているものより古ければ書かず、今の値を返す。
    pub fn put_reading(
        &self,
        source: &str,
        source_id: &str,
        device_id: &str,
        update: &ReadingUpdate,
    ) -> Result<ReadingPosition, String> {
        let device_time = update.device_time.min(now_ms() + MAX_CLOCK_SKEW_MS);
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(db_error)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT device_time FROM reading WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if current.is_none_or(|current| device_time >= current) {
            let cursor: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(cursor), 0) + 1 FROM reading",
                    [],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            tx.execute(
                "INSERT INTO reading (source, source_id, version, edit_hash, page, position, finished, device_id, device_time, cursor)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(source, source_id) DO UPDATE SET
                     version = excluded.version, edit_hash = excluded.edit_hash, page = excluded.page,
                     position = excluded.position, finished = excluded.finished, device_id = excluded.device_id,
                     device_time = excluded.device_time, cursor = excluded.cursor",
                params![
                    source, source_id, update.version, update.edit_hash, update.page, update.position,
                    update.finished, device_id, device_time, cursor
                ],
            )
            .map_err(db_error)?;
        }
        let row = tx
            .query_row(
                "SELECT source, source_id, version, edit_hash, page, position, finished, device_id, device_time, cursor
                 FROM reading WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                reading_from_row,
            )
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(row)
    }

    /// `since` より後に変わった位置を、古い順に返す。
    pub fn reading_since(&self, since: i64, limit: i64) -> Result<Vec<ReadingPosition>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT source, source_id, version, edit_hash, page, position, finished, device_id, device_time, cursor
                 FROM reading WHERE cursor > ?1 ORDER BY cursor LIMIT ?2",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![since, limit], reading_from_row)
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }
}

fn reading_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReadingPosition> {
    Ok(ReadingPosition {
        source: row.get(0)?,
        source_id: row.get(1)?,
        version: row.get(2)?,
        edit_hash: row.get(3)?,
        page: row.get(4)?,
        position: row.get(5)?,
        finished: row.get(6)?,
        device_id: row.get(7)?,
        device_time: row.get(8)?,
        cursor: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (std::path::PathBuf, ServerStore) {
        let dir = std::env::temp_dir().join(format!("piep_store_{}", random_token(8)));
        let store = ServerStore::open(&dir.join("state.db")).expect("store");
        (dir, store)
    }

    fn update(page: i64, device_time: i64) -> ReadingUpdate {
        ReadingUpdate {
            version: Some(1),
            edit_hash: None,
            page,
            position: 0.5,
            finished: false,
            device_time,
        }
    }

    #[test]
    fn pairing_is_single_use_and_expires() {
        let (dir, store) = store();
        let secret = store
            .issue_pairing(Role::Reader, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            store.consume_pairing("wrong", "端末").unwrap_err(),
            PairError::Rejected
        );
        let paired = store
            .consume_pairing(&secret, "端末")
            .expect("正しい鍵は通る");
        assert_eq!(paired.role, Role::Reader);
        assert_eq!(
            store.consume_pairing(&secret, "端末").unwrap_err(),
            PairError::Rejected
        );
        let expired = store.issue_pairing(Role::Reader, Duration::ZERO).unwrap();
        assert_eq!(
            store.consume_pairing(&expired, "端末").unwrap_err(),
            PairError::Rejected
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tokens_are_never_stored_in_clear_and_revocation_is_immediate() {
        let (dir, store) = store();
        let secret = store
            .issue_pairing(Role::Reader, Duration::from_secs(60))
            .unwrap();
        let paired = store.consume_pairing(&secret, "端末").unwrap();
        assert!(store.authorize(&paired.token).is_some());
        let stored: Vec<u8> = store
            .connection()
            .unwrap()
            .query_row("SELECT token_hash FROM devices", [], |row| row.get(0))
            .unwrap();
        assert_ne!(stored, paired.token.as_bytes());
        assert!(store.revoke_device(&paired.device_id).unwrap());
        assert!(store.authorize(&paired.token).is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_admin_replaces_the_previous_one_only_when_it_registers() {
        let (dir, store) = store();
        let first = store
            .issue_pairing(Role::Admin, Duration::from_secs(60))
            .unwrap();
        let first = store.consume_pairing(&first, "一台目").unwrap();
        let second = store
            .issue_pairing(Role::Admin, Duration::from_secs(60))
            .unwrap();
        assert!(
            store.authorize(&first.token).is_some(),
            "発行しただけでは締め出さない"
        );
        let second = store.consume_pairing(&second, "二台目").unwrap();
        assert!(store.authorize(&first.token).is_none());
        assert_eq!(store.authorize(&second.token).unwrap().role, Role::Admin);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reader_limit_is_enforced_without_spending_the_secret() {
        let (dir, store) = store();
        for index in 0..MAX_READERS {
            let secret = store
                .issue_pairing(Role::Reader, Duration::from_secs(60))
                .unwrap();
            store
                .consume_pairing(&secret, &format!("端末{index}"))
                .unwrap();
        }
        let secret = store
            .issue_pairing(Role::Reader, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            store.consume_pairing(&secret, "多すぎる").unwrap_err(),
            PairError::TooManyDevices
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn older_reading_positions_lose_and_the_cursor_only_moves_on_writes() {
        let (dir, store) = store();
        let now = now_ms();
        let first = store
            .put_reading("pixiv", "1", "a", &update(5, now))
            .unwrap();
        assert_eq!(first.page, 5);
        let stale = store
            .put_reading("pixiv", "1", "b", &update(2, now - 1_000))
            .unwrap();
        assert_eq!(stale.page, 5, "古い端末の時刻では上書きしない");
        assert_eq!(stale.device_id, "a");
        assert_eq!(stale.cursor, first.cursor);
        let newer = store
            .put_reading("pixiv", "1", "b", &update(9, now + 1_000))
            .unwrap();
        assert_eq!(newer.page, 9);
        assert!(newer.cursor > first.cursor);
        assert_eq!(store.reading_since(first.cursor, 10).unwrap().len(), 1);
        assert!(store.reading_since(newer.cursor, 10).unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_fast_clock_cannot_win_forever() {
        let (dir, store) = store();
        let far_future = now_ms() + 365 * 24 * 60 * 60 * 1000;
        let stored = store
            .put_reading("pixiv", "1", "a", &update(5, far_future))
            .unwrap();
        assert!(stored.device_time < far_future);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
