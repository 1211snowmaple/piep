//! サーバーの状態（端末、登録、ファイルの記録、読書位置）。
//!
//! 送信で入れ替わる本棚の DB とは別のファイルに置く。`piep-server init` と
//! 動いている `serve` のように別のプロセスからも触るので、SQLite にしておく。
//! 鍵はすべて SHA-256 だけを持ち、平文は発行した瞬間に返すだけで残さない。

use crate::database::Database;
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

/// 携帯で付け外ししたお気に入り。デスクトップ版が取り込むまでサーバーに残す。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteChange {
    pub source: String,
    pub source_id: String,
    pub favorite: bool,
    pub device_id: String,
    pub device_time: i64,
    pub cursor: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FavoriteUpdate {
    pub favorite: bool,
    pub device_time: i64,
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
    /// 携帯からの書き込みと、入れ替えたあとの当て直しを一列に並べる。
    /// 並べないと、当て直しが読んだ古い値で、そのあいだに届いた新しい値を上書きする。
    favorites: Mutex<()>,
}

pub fn random_token(bytes: usize) -> String {
    let mut random = vec![0_u8; bytes];
    rand::fill(random.as_mut_slice());
    URL_SAFE_NO_PAD.encode(random)
}

pub(super) fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(super) fn db_error(error: rusqlite::Error) -> String {
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
                 last_seen_at TEXT
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
             CREATE INDEX IF NOT EXISTS idx_reading_cursor ON reading(cursor);
             CREATE TABLE IF NOT EXISTS favorite_changes (
                 source TEXT NOT NULL,
                 source_id TEXT NOT NULL,
                 favorite INTEGER NOT NULL,
                 device_id TEXT NOT NULL,
                 device_time INTEGER NOT NULL,
                 cursor INTEGER NOT NULL,
                 PRIMARY KEY (source, source_id)
             );
             CREATE INDEX IF NOT EXISTS idx_favorite_changes_cursor ON favorite_changes(cursor);",
        )
        .map_err(db_error)?;
        // 0.33.0 が記録したメールアドレスを残さない。
        let has_last_user = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('devices') WHERE name = 'last_user'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(db_error)?
            > 0;
        if has_last_user {
            conn.execute_batch("ALTER TABLE devices DROP COLUMN last_user;")
                .map_err(db_error)?;
        }
        conn.execute_batch(super::admin_store::ADMIN_TABLES)
            .map_err(db_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
            favorites: Mutex::new(()),
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
        let mut conn = self.connection()?;
        let now = now_ms();
        let tx = conn.transaction().map_err(db_error)?;
        tx.execute("DELETE FROM pairings WHERE expires_at <= ?1", [now])
            .map_err(db_error)?;
        if role == Role::Admin {
            // 再発行後に以前の登録文字列で管理者を奪い返せないようにする。
            // 登録済みの管理者の鍵は、新しい登録が成功するまで残す。
            tx.execute("DELETE FROM pairings WHERE role = 'admin'", [])
                .map_err(db_error)?;
        }
        tx.execute(
            "INSERT INTO pairings (secret_hash, role, expires_at) VALUES (?1, ?2, ?3)",
            params![
                hash_secret(&secret).as_slice(),
                role.as_str(),
                now + lifetime.as_millis() as i64
            ],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
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
            // 古い版が発行して残した登録文字列も、成功した世代をまたがせない。
            tx.execute("DELETE FROM pairings WHERE role = 'admin'", [])
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

    pub fn has_admin(&self) -> Result<bool, String> {
        let conn = self.connection()?;
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM devices WHERE role = 'admin')",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceSummary>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, role, created_at, last_seen_at FROM devices ORDER BY created_at",
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
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
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

    /// 携帯からのお気に入りを記録する。端末の時刻が持っているものより古ければ書かず、今の値を返す。
    ///
    /// 単独サーバーの本棚は送信のたびに入れ替わる。デスクトップ版が取り込んだと知らせるまで
    /// ここに残し、入れ替えたあとの本棚へ当て直す。
    pub fn put_favorite(
        &self,
        source: &str,
        source_id: &str,
        device_id: &str,
        update: &FavoriteUpdate,
    ) -> Result<FavoriteChange, String> {
        let device_time = update.device_time.min(now_ms() + MAX_CLOCK_SKEW_MS);
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(db_error)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT device_time FROM favorite_changes WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if current.is_none_or(|current| device_time >= current) {
            // 取り込みの知らせで行を消しても番号を戻さないよう、読書位置と同じ通し番号の場所を使う。
            let cursor = next_favorite_cursor(&tx)?;
            tx.execute(
                "INSERT INTO favorite_changes (source, source_id, favorite, device_id, device_time, cursor)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(source, source_id) DO UPDATE SET
                     favorite = excluded.favorite, device_id = excluded.device_id,
                     device_time = excluded.device_time, cursor = excluded.cursor",
                params![source, source_id, update.favorite, device_id, device_time, cursor],
            )
            .map_err(db_error)?;
        }
        let row = tx
            .query_row(
                "SELECT source, source_id, favorite, device_id, device_time, cursor
                 FROM favorite_changes WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                favorite_from_row,
            )
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(row)
    }

    /// 携帯からのお気に入りを記録し、勝ったほうの値を本棚へ書く。作品が無ければ記録しない。
    pub fn apply_favorite(
        &self,
        db: &Database,
        source: &str,
        source_id: &str,
        device_id: &str,
        update: &FavoriteUpdate,
    ) -> Result<Option<FavoriteChange>, String> {
        let _order = self
            .favorites
            .lock()
            .map_err(|_| "お気に入りの記録を開けません")?;
        // 無い作品の変更を残すと、送られてくるたびに当て直そうとする。
        if db.get_download_by_source(source, source_id)?.is_none() {
            return Ok(None);
        }
        let stored = self.put_favorite(source, source_id, device_id, update)?;
        db.set_favorite_by_source(source, source_id, stored.favorite)?;
        Ok(Some(stored))
    }

    /// 送られてきた本棚へ、デスクトップ版がまだ取り込んでいない変更を当て直す。
    /// 当てられなかった作品があっても止めず、当てた数を返す。
    pub fn reapply_favorites(&self, db: &Database) -> Result<usize, String> {
        let _order = self
            .favorites
            .lock()
            .map_err(|_| "お気に入りの記録を開けません")?;
        let mut applied = 0;
        let mut since = 0;
        loop {
            let changes = self.favorite_changes_since(since, 500)?;
            let Some(last) = changes.last() else {
                break;
            };
            since = last.cursor;
            for change in &changes {
                match db.set_favorite_by_source(&change.source, &change.source_id, change.favorite)
                {
                    Ok(Some(_)) => applied += 1,
                    Ok(None) => {}
                    Err(error) => log::warn!(
                        "お気に入りを当て直せません（{} {}）: {error}",
                        change.source,
                        change.source_id
                    ),
                }
            }
        }
        Ok(applied)
    }

    /// `since` より後に記録したお気に入りを、古い順に返す。
    pub fn favorite_changes_since(
        &self,
        since: i64,
        limit: i64,
    ) -> Result<Vec<FavoriteChange>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT source, source_id, favorite, device_id, device_time, cursor
                 FROM favorite_changes WHERE cursor > ?1 ORDER BY cursor LIMIT ?2",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![since, limit], favorite_from_row)
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }

    /// デスクトップ版が `cursor` までを取り込んだ。その分はもう当て直さない。
    pub fn forget_favorite_changes(&self, cursor: i64) -> Result<usize, String> {
        let conn = self.connection()?;
        conn.execute(
            "DELETE FROM favorite_changes WHERE cursor <= ?1",
            params![cursor],
        )
        .map_err(db_error)
    }
}

/// 消した行の番号を使い回すと、デスクトップ版が覚えている続きの番号より前に新しい変更が入り、
/// 取りこぼす。最後に振った番号を別に覚えておく。
fn next_favorite_cursor(tx: &rusqlite::Transaction<'_>) -> Result<i64, String> {
    let last: i64 = tx
        .query_row(
            "SELECT COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'favorite_cursor'), 0)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let next = last + 1;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('favorite_cursor', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![next.to_string()],
    )
    .map_err(db_error)?;
    Ok(next)
}

fn favorite_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FavoriteChange> {
    Ok(FavoriteChange {
        source: row.get(0)?,
        source_id: row.get(1)?,
        favorite: row.get(2)?,
        device_id: row.get(3)?,
        device_time: row.get(4)?,
        cursor: row.get(5)?,
    })
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

    fn favorite(favorite: bool, device_time: i64) -> FavoriteUpdate {
        FavoriteUpdate {
            favorite,
            device_time,
        }
    }

    #[test]
    fn favorites_keep_the_newest_change_and_numbers_never_go_back() {
        let (_dir, store) = store();
        let first = store
            .put_favorite("pixiv", "1", "phone", &favorite(true, 1_000))
            .unwrap();
        // 古い時刻の変更は負ける。
        let stale = store
            .put_favorite("pixiv", "1", "tablet", &favorite(false, 500))
            .unwrap();
        assert_eq!(stale, first);
        let other = store
            .put_favorite("pixiv", "2", "phone", &favorite(true, 2_000))
            .unwrap();
        assert_eq!(store.favorite_changes_since(0, 10).unwrap().len(), 2);
        assert_eq!(store.forget_favorite_changes(other.cursor).unwrap(), 2);
        assert!(store.favorite_changes_since(0, 10).unwrap().is_empty());
        // 消したあとも番号は進む。取り込み済みの続きの番号より前に入ると取りこぼすため。
        let later = store
            .put_favorite("pixiv", "1", "phone", &favorite(false, 3_000))
            .unwrap();
        assert!(later.cursor > other.cursor);
        assert_eq!(
            store.favorite_changes_since(other.cursor, 10).unwrap(),
            vec![later]
        );
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
    fn reissuing_and_consuming_an_admin_pairing_invalidates_other_admin_secrets() {
        let (dir, store) = store();
        let first = store
            .issue_pairing(Role::Admin, Duration::from_secs(60))
            .unwrap();
        let current = store.consume_pairing(&first, "今の管理者").unwrap();
        let reader = store
            .issue_pairing(Role::Reader, Duration::from_secs(60))
            .unwrap();
        let obsolete = store
            .issue_pairing(Role::Admin, Duration::from_secs(60))
            .unwrap();
        let latest = store
            .issue_pairing(Role::Admin, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            store.consume_pairing(&obsolete, "古い案内").unwrap_err(),
            PairError::Rejected
        );
        assert!(
            store.authorize(&current.token).is_some(),
            "発行だけでは今の管理者を失効させない"
        );

        // 修正前の版が複数発行したまま更新された状態も扱う。
        let legacy = random_token(32);
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO pairings(secret_hash, role, expires_at) VALUES (?1, 'admin', ?2)",
                params![hash_secret(&legacy).as_slice(), now_ms() + 60_000],
            )
            .unwrap();
        let replacement = store.consume_pairing(&latest, "次の管理者").unwrap();
        assert!(store.authorize(&current.token).is_none());
        assert!(store.authorize(&replacement.token).is_some());
        assert_eq!(
            store.consume_pairing(&legacy, "古い版の案内").unwrap_err(),
            PairError::Rejected
        );
        assert_eq!(
            store.consume_pairing(&reader, "携帯").unwrap().role,
            Role::Reader
        );
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
