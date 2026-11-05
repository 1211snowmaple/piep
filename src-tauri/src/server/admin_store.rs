//! 管理画面のアカウント、ログイン、券、記録。置き場は `state.db`。
//!
//! 平文のパスワード、セッションの鍵、券はここに残さない。残すのはハッシュだけである。

use super::store::{db_error, hash_secret, now_ms, random_token, ServerStore};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::time::Duration;

/// 操作がなければ切る。
pub const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);
/// ログインから必ず切る。
pub const SESSION_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
/// この間にパスワードを入れていれば、大事な操作で聞き直さない。
pub const RECENT_AUTH: Duration = Duration::from_secs(5 * 60);
/// 記録の保持。
const AUDIT_DAYS: i64 = 90;
/// 最後の操作を書くのは、これだけ空いたときだけにする。読むだけの要求で書き続けない。
const TOUCH_EVERY_MS: i64 = 30_000;

const META_WEB_DISABLED: &str = "admin_web_disabled";
const META_HOSTS: &str = "admin_hosts";
const META_SERVER_NAME: &str = "server_name";
const MAX_HOSTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TicketKind {
    /// 既にあるアカウントでログインする。
    Login,
    /// 最初の管理者を作る。アカウントがあるときは、パスワードの再設定になる。
    Setup,
}

impl TicketKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Setup => "setup",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text {
            "login" => Some(Self::Login),
            "setup" => Some(Self::Setup),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdminAccount {
    pub username: String,
    pub password_hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSession {
    pub id: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub recent_auth_at: Option<i64>,
    pub ip: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub action: String,
    pub actor: String,
    pub ip: Option<String>,
    pub ok: bool,
    pub detail: Option<String>,
}

pub(super) const ADMIN_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS admin_account (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        username TEXT NOT NULL,
        password_hash TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS admin_sessions (
        id TEXT PRIMARY KEY,
        token_hash BLOB NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        recent_auth_at INTEGER,
        ip TEXT,
        label TEXT
    );
    CREATE TABLE IF NOT EXISTS admin_tickets (
        secret_hash BLOB PRIMARY KEY,
        kind TEXT NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS audit_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        action TEXT NOT NULL,
        actor TEXT NOT NULL,
        ip TEXT,
        ok INTEGER NOT NULL,
        detail TEXT
    );
    CREATE INDEX IF NOT EXISTS audit_log_at ON audit_log (at);
";

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AdminSession> {
    Ok(AdminSession {
        id: row.get(0)?,
        created_at: row.get(1)?,
        last_seen_at: row.get(2)?,
        expires_at: row.get(3)?,
        recent_auth_at: row.get(4)?,
        ip: row.get(5)?,
        label: row.get(6)?,
    })
}

const SESSION_COLUMNS: &str = "id, created_at, last_seen_at, expires_at, recent_auth_at, ip, label";

impl ServerStore {
    pub fn admin_account(&self) -> Result<Option<AdminAccount>, String> {
        let conn = self.connection()?;
        conn.query_row(
            "SELECT username, password_hash FROM admin_account WHERE id = 1",
            [],
            |row| {
                Ok(AdminAccount {
                    username: row.get(0)?,
                    password_hash: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(db_error)
    }

    /// アカウントを作るか、パスワードを置き換える。名前はあとから変えない。
    pub fn put_admin_account(&self, username: &str, password_hash: &str) -> Result<(), String> {
        let conn = self.connection()?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO admin_account (id, username, password_hash, created_at, updated_at)
             VALUES (1, ?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 password_hash = excluded.password_hash,
                 updated_at = excluded.updated_at",
            params![username, password_hash, now],
        )
        .map_err(db_error)?;
        Ok(())
    }

    /// 一度きりの券を発行する。平文はここで返す分だけである。
    pub fn issue_admin_ticket(
        &self,
        kind: TicketKind,
        lifetime: Duration,
    ) -> Result<String, String> {
        let secret = random_token(32);
        let conn = self.connection()?;
        let now = now_ms();
        conn.execute("DELETE FROM admin_tickets WHERE expires_at <= ?1", [now])
            .map_err(db_error)?;
        conn.execute(
            "INSERT INTO admin_tickets (secret_hash, kind, expires_at) VALUES (?1, ?2, ?3)",
            params![
                hash_secret(&secret).as_slice(),
                kind.as_str(),
                now + lifetime.as_millis() as i64
            ],
        )
        .map_err(db_error)?;
        Ok(secret)
    }

    /// 券の種類を見る。使わない。
    pub fn peek_admin_ticket(&self, secret: &str) -> Result<Option<TicketKind>, String> {
        let conn = self.connection()?;
        let kind: Option<String> = conn
            .query_row(
                "SELECT kind FROM admin_tickets WHERE secret_hash = ?1 AND expires_at > ?2",
                params![hash_secret(secret).as_slice(), now_ms()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        Ok(kind.as_deref().and_then(TicketKind::parse))
    }

    /// 券を使う。同時に 2 回来ても、通るのは 1 回だけである。
    pub fn consume_admin_ticket(&self, secret: &str) -> Result<Option<TicketKind>, String> {
        let conn = self.connection()?;
        let kind: Option<String> = conn
            .query_row(
                "DELETE FROM admin_tickets WHERE secret_hash = ?1 AND expires_at > ?2 RETURNING kind",
                params![hash_secret(secret).as_slice(), now_ms()],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        Ok(kind.as_deref().and_then(TicketKind::parse))
    }

    /// ログインを始める。鍵の平文を返す。
    pub fn create_admin_session(
        &self,
        recent_auth: bool,
        ip: Option<&str>,
        label: Option<&str>,
    ) -> Result<(String, AdminSession), String> {
        let id = random_token(12);
        let token = random_token(32);
        let now = now_ms();
        let session = AdminSession {
            id: id.clone(),
            created_at: now,
            last_seen_at: now,
            expires_at: now + SESSION_LIFETIME.as_millis() as i64,
            recent_auth_at: recent_auth.then_some(now),
            ip: ip.map(str::to_string),
            label: label.map(|label| label.chars().take(200).collect()),
        };
        let conn = self.connection()?;
        conn.execute(
            "DELETE FROM admin_sessions WHERE expires_at <= ?1 OR last_seen_at <= ?2",
            params![now, now - SESSION_IDLE.as_millis() as i64],
        )
        .map_err(db_error)?;
        conn.execute(
            "INSERT INTO admin_sessions
                 (id, token_hash, created_at, last_seen_at, expires_at, recent_auth_at, ip, label)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7)",
            params![
                session.id,
                hash_secret(&token).as_slice(),
                now,
                session.expires_at,
                session.recent_auth_at,
                session.ip,
                session.label
            ],
        )
        .map_err(db_error)?;
        Ok((token, session))
    }

    /// 鍵からセッションを引く。切れていれば消す。判定はすべてここで行う。
    /// `touch` が false の要求（画面の自動の読み直し）は、操作として数えない。
    pub fn authorize_admin_session(&self, token: &str, touch: bool) -> Option<AdminSession> {
        if token.is_empty() || token.len() > 128 {
            return None;
        }
        let conn = self.connection().ok()?;
        let submitted = hash_secret(token);
        let session = conn
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM admin_sessions WHERE token_hash = ?1"),
                [submitted.as_slice()],
                session_from_row,
            )
            .optional()
            .ok()??;
        let now = now_ms();
        if now >= session.expires_at
            || now - session.last_seen_at >= SESSION_IDLE.as_millis() as i64
        {
            let _ = conn.execute("DELETE FROM admin_sessions WHERE id = ?1", [&session.id]);
            return None;
        }
        if touch && now - session.last_seen_at >= TOUCH_EVERY_MS {
            let _ = conn.execute(
                "UPDATE admin_sessions SET last_seen_at = ?1 WHERE id = ?2",
                params![now, session.id],
            );
        }
        Some(session)
    }

    pub fn mark_admin_recent_auth(&self, id: &str) -> Result<(), String> {
        let conn = self.connection()?;
        conn.execute(
            "UPDATE admin_sessions SET recent_auth_at = ?1 WHERE id = ?2",
            params![now_ms(), id],
        )
        .map_err(db_error)?;
        Ok(())
    }

    pub fn list_admin_sessions(&self) -> Result<Vec<AdminSession>, String> {
        let conn = self.connection()?;
        let now = now_ms();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SESSION_COLUMNS} FROM admin_sessions
                 WHERE expires_at > ?1 AND last_seen_at > ?2
                 ORDER BY last_seen_at DESC"
            ))
            .map_err(db_error)?;
        let rows = stmt
            .query_map(
                params![now, now - SESSION_IDLE.as_millis() as i64],
                session_from_row,
            )
            .map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }

    pub fn delete_admin_session(&self, id: &str) -> Result<bool, String> {
        let conn = self.connection()?;
        let removed = conn
            .execute("DELETE FROM admin_sessions WHERE id = ?1", [id])
            .map_err(db_error)?;
        Ok(removed > 0)
    }

    /// `except` のほかをすべて切る。切った数を返す。
    pub fn delete_admin_sessions_except(&self, except: Option<&str>) -> Result<usize, String> {
        let conn = self.connection()?;
        conn.execute(
            "DELETE FROM admin_sessions WHERE ?1 IS NULL OR id <> ?1",
            params![except],
        )
        .map_err(db_error)
    }

    pub fn admin_web_disabled(&self) -> bool {
        self.meta(META_WEB_DISABLED)
            .ok()
            .flatten()
            .is_some_and(|value| value == "1")
    }

    pub fn set_admin_web_disabled(&self, disabled: bool) -> Result<(), String> {
        self.set_meta(META_WEB_DISABLED, if disabled { "1" } else { "0" })
    }

    /// 券が通った名前。IP でない `Host` は、ここにあるものしか受けない。
    pub fn admin_hosts(&self) -> Vec<String> {
        self.meta(META_HOSTS)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn allow_admin_host(&self, host: &str) -> Result<(), String> {
        let host = host.to_ascii_lowercase();
        let mut hosts = self.admin_hosts();
        if hosts.contains(&host) {
            return Ok(());
        }
        hosts.insert(0, host);
        hosts.truncate(MAX_HOSTS);
        let text = serde_json::to_string(&hosts).map_err(|error| error.to_string())?;
        self.set_meta(META_HOSTS, &text)
    }

    /// サーバーの名前。決めていなければ、その PC の名前にする。
    pub fn server_name(&self) -> String {
        self.meta(META_SERVER_NAME)
            .ok()
            .flatten()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "サーバー".to_string())
    }

    pub fn set_server_name(&self, name: &str) -> Result<(), String> {
        self.set_meta(META_SERVER_NAME, name.trim())
    }

    /// 記録を足す。書けなくても操作は止めない。
    pub fn audit(
        &self,
        kind: &str,
        action: &str,
        actor: &str,
        ip: Option<&str>,
        ok: bool,
        detail: Option<&str>,
    ) {
        let result = self.connection().and_then(|conn| {
            let now = now_ms();
            conn.execute(
                "DELETE FROM audit_log WHERE at < ?1",
                [now - AUDIT_DAYS * 24 * 60 * 60 * 1000],
            )
            .map_err(db_error)?;
            conn.execute(
                "INSERT INTO audit_log (at, kind, action, actor, ip, ok, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![now, kind, action, actor, ip, ok, detail],
            )
            .map_err(db_error)?;
            Ok(())
        });
        if let Err(error) = result {
            log::warn!("記録を書けません: {error}");
        }
    }

    /// 新しい順に返す。`before` より前の行だけを読むと、続きになる。
    pub fn list_audit(
        &self,
        kind: Option<&str>,
        ok: Option<bool>,
        before: Option<i64>,
        limit: i64,
    ) -> Result<Vec<AuditEntry>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, at, kind, action, actor, ip, ok, detail FROM audit_log
                 WHERE (?1 IS NULL OR kind = ?1)
                   AND (?2 IS NULL OR ok = ?2)
                   AND (?3 IS NULL OR id < ?3)
                 ORDER BY id DESC LIMIT ?4",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![kind, ok, before, limit], |row| {
                Ok(AuditEntry {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    kind: row.get(2)?,
                    action: row.get(3)?,
                    actor: row.get(4)?,
                    ip: row.get(5)?,
                    ok: row.get(6)?,
                    detail: row.get(7)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }
}
