//! サーバーの状態（端末、登録、設定の値）。
//!
//! 送られて入れ替わる棚とは別のファイルに置く。`piep-server init` と
//! 動いている `serve` のように別のプロセスからも触るので、SQLite にしておく。
//! 鍵はすべて SHA-256 だけを持ち、平文は発行した瞬間に返すだけで残さない。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
pub use piep_library_protocol::Role;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use subtle::ConstantTimeEq;

pub const MAX_READERS: i64 = 16;

fn parse_role(value: &str) -> Option<Role> {
    match value {
        "reader" => Some(Role::Reader),
        "admin" => Some(Role::Admin),
        _ => None,
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

// **Debug を付けない。** 端末の鍵を持つので、`{:?}` 一つでログへ落ちる。
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

pub struct ServerStore {
    conn: Mutex<Connection>,
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
             );",
        )
        .map_err(db_error)?;
        conn.execute_batch(super::admin_store::ADMIN_TABLES)
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
                    matched = parse_role(&role);
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
                found = parse_role(&role).map(|role| (AuthorizedDevice { id, role }, seen));
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
                    role: parse_role(&row.get::<_, String>(2)?).unwrap_or(Role::Reader),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (std::path::PathBuf, ServerStore) {
        let dir = std::env::temp_dir().join(format!("piep_store_{}", random_token(8)));
        let store = ServerStore::open(&dir.join("state.db")).expect("store");
        (dir, store)
    }

    #[test]
    fn pairing_is_single_use_and_expires() {
        let (dir, store) = store();
        let secret = store
            .issue_pairing(Role::Reader, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            store.consume_pairing("wrong", "端末").err().unwrap(),
            PairError::Rejected
        );
        let paired = store
            .consume_pairing(&secret, "端末")
            .expect("正しい鍵は通る");
        assert_eq!(paired.role, Role::Reader);
        assert_eq!(
            store.consume_pairing(&secret, "端末").err().unwrap(),
            PairError::Rejected
        );
        let expired = store.issue_pairing(Role::Reader, Duration::ZERO).unwrap();
        assert_eq!(
            store.consume_pairing(&expired, "端末").err().unwrap(),
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
            store.consume_pairing(&obsolete, "古い案内").err().unwrap(),
            PairError::Rejected
        );
        assert!(
            store.authorize(&current.token).is_some(),
            "発行だけでは今の管理者を失効させない"
        );

        // 管理者の登録が複数残っていても、使った一つ以外は断る。
        let stray = random_token(32);
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO pairings(secret_hash, role, expires_at) VALUES (?1, 'admin', ?2)",
                params![hash_secret(&stray).as_slice(), now_ms() + 60_000],
            )
            .unwrap();
        let replacement = store.consume_pairing(&latest, "次の管理者").unwrap();
        assert!(store.authorize(&current.token).is_none());
        assert!(store.authorize(&replacement.token).is_some());
        assert_eq!(
            store.consume_pairing(&stray, "残った案内").err().unwrap(),
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
            store.consume_pairing(&secret, "多すぎる").err().unwrap(),
            PairError::TooManyDevices
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
