//! 単独サーバーの側の受信。差分の問い合わせ、かたまりの受け取り、確定と入れ替え。
//!
//! 送信は同時に 1 つだけ。切れても、受け取ったかたまりは残るので続きから送れる。
//! 確定するまで、読んでいる本棚には何も起きない。

use super::library::SwappableDb;
use super::snapshot::{restore_on_server, validate_relative};
use super::store::{random_token, ServerStore};
use crate::database::queries::SearchIndexRebuildOptions;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SESSION_LIFETIME: chrono::TimeDelta = chrono::TimeDelta::hours(24);
const TRASH_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024 * 1024;

const META_SESSION: &str = "sync_session";
const META_SESSION_AT: &str = "sync_session_at";
const META_DATABASE_SHA: &str = "database_sha256";
const META_GENERATION: &str = "generation";
const META_COMMITTED_AT: &str = "committed_at";
const META_PUBLICATION: &str = "sync_publication";

#[derive(Debug)]
pub enum SyncError {
    Invalid(&'static str),
    NotFound,
    Conflict(&'static str),
    Internal(String),
}

impl From<String> for SyncError {
    fn from(detail: String) -> Self {
        Self::Internal(detail)
    }
}

impl From<io::Error> for SyncError {
    fn from(error: io::Error) -> Self {
        Self::Internal(format!("ファイルを読み書きできません: {error}"))
    }
}

impl From<rusqlite::Error> for SyncError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Internal(format!("サーバーの状態を読み書きできません: {error}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitRequest {
    pub database_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedSession {
    pub session_id: String,
    pub resumed: bool,
    /// 今の本棚の DB の SHA-256。同じなら DB を上げ直さなくてよい。
    pub database_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlobState {
    pub received: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitOutcome {
    pub generation: i64,
    pub works: i64,
    pub files_added: i64,
    pub files_removed: i64,
    /// DB が指しているのに本棚に無いファイルの数。送り元で欠けているものも含む。
    pub missing_referenced: i64,
    pub unchanged: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverStatus {
    pub generation: i64,
    pub committed_at: Option<String>,
    pub session_open: bool,
    pub phase: String,
    pub index_pending: Option<i64>,
    pub index_error: Option<String>,
}

#[derive(Default)]
struct IndexWorker {
    cancel: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// 公開前に全操作を書き出す。各 rename の前後どちらで止まっても、控えの有無で
/// 巻き戻せる。状態 DB の publication ID が一致するときだけ新世代を採用する。
#[derive(Debug, Serialize, Deserialize)]
struct PublicationJournal {
    id: String,
    session_id: String,
    changes: Vec<FileChange>,
    database_changed: bool,
    had_database: bool,
    trash_batch: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileChange {
    path: String,
    had_previous: bool,
    removal: bool,
}

struct RestartIndex<'a>(&'a SyncReceiver);

impl Drop for RestartIndex<'_> {
    fn drop(&mut self) {
        self.0.start_index_catch_up();
    }
}

pub struct SyncReceiver {
    /// `downloads`・`profiles` などの親で、DB もここに置く。
    library_root: PathBuf,
    trash_root: PathBuf,
    incoming_root: PathBuf,
    store: Arc<ServerStore>,
    db: Arc<SwappableDb>,
    /// 確定と、同じ送信へのかたまりの書き込みを一列に並べる。
    write_lock: Mutex<()>,
    phase: Mutex<String>,
    index: Mutex<IndexWorker>,
    index_pending: Arc<Mutex<Option<i64>>>,
    index_error: Arc<Mutex<Option<String>>>,
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn file_sha256(path: &Path) -> Result<String, SyncError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

impl SyncReceiver {
    pub fn new(
        library_root: PathBuf,
        trash_root: PathBuf,
        incoming_root: PathBuf,
        store: Arc<ServerStore>,
        db: Arc<SwappableDb>,
    ) -> Self {
        Self {
            library_root,
            trash_root,
            incoming_root,
            store,
            db,
            write_lock: Mutex::new(()),
            phase: Mutex::new("idle".into()),
            index: Mutex::new(IndexWorker::default()),
            index_pending: Arc::new(Mutex::new(None)),
            index_error: Arc::new(Mutex::new(None)),
        }
    }

    fn set_phase(&self, phase: &str) {
        if let Ok(mut current) = self.phase.lock() {
            *current = phase.to_string();
        }
    }

    pub fn status(&self) -> ReceiverStatus {
        let writing = matches!(
            self.write_lock.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        );
        let resumable = self
            .store
            .meta(META_SESSION)
            .ok()
            .flatten()
            .is_some_and(|session| {
                self.session_is_fresh().unwrap_or(false) && self.session_dir(&session).is_dir()
            });
        let recovering = !writing
            && self.publication_dir().join("journal.json").exists()
            && self.db.current().is_none();
        ReceiverStatus {
            generation: self.generation(),
            committed_at: self.store.meta(META_COMMITTED_AT).ok().flatten(),
            // 放置された期限切れの記録で、自己更新の quiet 待機を妨げ続けない。
            // 期限の境界をまたいだ書込み・確定そのものは引き続き待つ。
            session_open: writing || resumable || recovering,
            phase: self
                .phase
                .lock()
                .map(|phase| phase.clone())
                .unwrap_or_default(),
            index_pending: self.index_pending.lock().ok().and_then(|pending| *pending),
            index_error: self.index_error.lock().ok().and_then(|error| error.clone()),
        }
    }

    fn generation(&self) -> i64 {
        self.store
            .meta(META_GENERATION)
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.incoming_root.join(session_id)
    }

    fn require_session(&self, session_id: &str) -> Result<(), SyncError> {
        if self.store.meta(META_SESSION)?.as_deref() == Some(session_id) {
            Ok(())
        } else {
            Err(SyncError::NotFound)
        }
    }

    fn session_is_fresh(&self) -> Result<bool, String> {
        Ok(self
            .store
            .meta(META_SESSION_AT)?
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
            .is_some_and(|at| chrono::Utc::now().signed_duration_since(at) < SESSION_LIFETIME))
    }

    /// 送信を開く。途中の送信があれば続きとして返す。24 時間より古ければ捨てる。
    pub fn open_session(&self) -> Result<OpenedSession, SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("確定の途中です"))?;
        self.recover_before_write()?;
        let database_sha256 = if self.db.current().is_some() {
            self.store.meta(META_DATABASE_SHA)?
        } else {
            None
        };
        if let Some(existing) = self.store.meta(META_SESSION)? {
            let fresh = self.session_is_fresh()?;
            if fresh && self.session_dir(&existing).exists() {
                // 受け取ったかたまりは残すが、一覧は今回の送信のものに取り替える。
                // 前回のあとで手元から消えたファイルを、置いてしまわないように。
                self.store.connection()?.execute(
                    "DELETE FROM session_files WHERE session_id = ?1",
                    [&existing],
                )?;
                return Ok(OpenedSession {
                    session_id: existing,
                    resumed: true,
                    database_sha256,
                });
            }
            self.discard_session(&existing)?;
        }
        let session_id = random_token(16);
        fs::create_dir_all(self.session_dir(&session_id).join("blobs"))?;
        self.store.set_meta(META_SESSION, &session_id)?;
        self.store
            .set_meta(META_SESSION_AT, &chrono::Utc::now().to_rfc3339())?;
        Ok(OpenedSession {
            session_id,
            resumed: false,
            database_sha256,
        })
    }

    fn discard_session(&self, session_id: &str) -> Result<(), SyncError> {
        self.store.connection()?.execute(
            "DELETE FROM session_files WHERE session_id = ?1",
            [session_id],
        )?;
        self.store.connection()?.execute(
            "DELETE FROM meta WHERE key IN (?1, ?2)",
            [META_SESSION, META_SESSION_AT],
        )?;
        let dir = self.session_dir(session_id);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn abort_session(&self, session_id: &str) -> Result<(), SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("確定の途中です"))?;
        self.recover_before_write()?;
        self.require_session(session_id)?;
        self.discard_session(session_id)
    }

    /// 一覧の一部を受け取り、上げてほしいパスを返す。
    ///
    /// 比べるのは大きさと時刻だけで、毎回 20 GB を読み直さない。実物も見るので、
    /// サーバーの側でファイルが消えていれば上げ直しになる。
    pub fn add_manifest(
        &self,
        session_id: &str,
        files: &[ManifestEntry],
    ) -> Result<Vec<String>, SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("送信の状態を確保できません"))?;
        self.require_session(session_id)?;
        for entry in files {
            validate_relative(&entry.path)
                .map_err(|_| SyncError::Invalid("正しくないパスが含まれています"))?;
            if entry.size > MAX_BLOB_BYTES {
                return Err(SyncError::Invalid("ファイルが大きすぎます"));
            }
        }
        let mut conn = self.store.connection()?;
        let tx = conn.transaction()?;
        let mut needed = Vec::new();
        {
            let mut known =
                tx.prepare_cached("SELECT size, mtime_ms FROM files WHERE path = ?1")?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO session_files (session_id, path, size, mtime_ms, needed)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(session_id, path) DO UPDATE SET
                     size = excluded.size, mtime_ms = excluded.mtime_ms, needed = excluded.needed,
                     sha256 = CASE WHEN session_files.size = excluded.size AND session_files.mtime_ms = excluded.mtime_ms
                                   THEN session_files.sha256 END",
            )?;
            for entry in files {
                let recorded: Option<(i64, i64)> = known
                    .query_row([&entry.path], |row| Ok((row.get(0)?, row.get(1)?)))
                    .ok();
                let unchanged = recorded == Some((entry.size as i64, entry.mtime_ms))
                    && fs::metadata(self.library_path(&entry.path))
                        .is_ok_and(|meta| meta.is_file() && meta.len() == entry.size);
                insert.execute(params![
                    session_id,
                    entry.path,
                    entry.size as i64,
                    entry.mtime_ms,
                    !unchanged
                ])?;
                if !unchanged {
                    needed.push(entry.path.clone());
                }
            }
        }
        tx.commit()?;
        Ok(needed)
    }

    fn library_path(&self, relative: &str) -> PathBuf {
        let mut path = self.library_root.clone();
        path.extend(relative.split('/'));
        path
    }

    fn blob_paths(&self, session_id: &str, sha: &str) -> (PathBuf, PathBuf) {
        let dir = self.session_dir(session_id).join("blobs");
        (dir.join(sha), dir.join(format!("{sha}.part")))
    }

    pub fn blob_state(&self, session_id: &str, sha: &str) -> Result<BlobState, SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("送信の状態を確保できません"))?;
        self.require_session(session_id)?;
        if !valid_sha(sha) {
            return Err(SyncError::Invalid("ハッシュの形が正しくありません"));
        }
        let (done, part) = self.blob_paths(session_id, sha);
        if let Ok(meta) = fs::metadata(&done) {
            return Ok(BlobState {
                received: meta.len(),
                complete: true,
            });
        }
        let received = fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
        Ok(BlobState {
            received,
            complete: false,
        })
    }

    /// かたまりを 1 つ書く。揃ったら SHA-256 を照合して確定する。
    ///
    /// 空でないファイルは `offset` が今の長さと一致するときだけ受ける。合わなければ
    /// 今の長さを返し、送り手がそこから続ける。
    pub fn write_blob(
        &self,
        session_id: &str,
        sha: &str,
        offset: u64,
        total: u64,
        bytes: &[u8],
    ) -> Result<BlobState, SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("送信の状態を確保できません"))?;
        self.require_session(session_id)?;
        if !valid_sha(sha) {
            return Err(SyncError::Invalid("ハッシュの形が正しくありません"));
        }
        if total > MAX_BLOB_BYTES || offset.saturating_add(bytes.len() as u64) > total {
            return Err(SyncError::Invalid("かたまりの範囲が正しくありません"));
        }
        let (done, part) = self.blob_paths(session_id, sha);
        if let Ok(meta) = fs::metadata(&done) {
            return Ok(BlobState {
                received: meta.len(),
                complete: true,
            });
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(false)
            .write(true)
            .truncate(false)
            .open(&part)?;
        let current = file.metadata()?.len();
        if offset != current {
            return Ok(BlobState {
                received: current,
                complete: false,
            });
        }
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        let received = offset + bytes.len() as u64;
        if received < total {
            return Ok(BlobState {
                received,
                complete: false,
            });
        }
        if file_sha256(&part)? != sha {
            fs::remove_file(&part)?;
            return Err(SyncError::Conflict(
                "受け取った内容がハッシュと一致しません",
            ));
        }
        fs::rename(&part, &done)?;
        Ok(BlobState {
            received,
            complete: true,
        })
    }

    /// どのパスがどの中身かを結びつける。
    pub fn bind_files(&self, session_id: &str, files: &[BindEntry]) -> Result<(), SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("送信の状態を確保できません"))?;
        self.require_session(session_id)?;
        let mut conn = self.store.connection()?;
        let tx = conn.transaction()?;
        {
            let mut update = tx.prepare_cached(
                "UPDATE session_files SET sha256 = ?1 WHERE session_id = ?2 AND path = ?3",
            )?;
            for entry in files {
                if !valid_sha(&entry.sha256) {
                    return Err(SyncError::Invalid("ハッシュの形が正しくありません"));
                }
                if update.execute(params![entry.sha256, session_id, entry.path])? == 0 {
                    return Err(SyncError::Invalid("一覧に無いパスです"));
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 確定する。一覧のファイルが揃っていなければ、何も変えずに断る。
    pub fn commit(
        &self,
        session_id: &str,
        request: &CommitRequest,
    ) -> Result<CommitOutcome, SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("確定の途中です"))?;
        self.recover_before_write()?;
        self.require_session(session_id)?;
        if !valid_sha(&request.database_sha256) {
            return Err(SyncError::Invalid("ハッシュの形が正しくありません"));
        }
        let result = self.commit_locked(session_id, request);
        self.set_phase("idle");
        result
    }

    fn commit_locked(
        &self,
        session_id: &str,
        request: &CommitRequest,
    ) -> Result<CommitOutcome, SyncError> {
        self.set_phase("verifying");
        let needed: Vec<(String, i64, i64, Option<String>)> = {
            let conn = self.store.connection()?;
            let mut stmt = conn.prepare(
                "SELECT path, size, mtime_ms, sha256 FROM session_files
                 WHERE session_id = ?1 AND needed = 1 ORDER BY sha256",
            )?;
            let rows = stmt.query_map([session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for (_, size, _, sha) in &needed {
            let sha = sha
                .as_deref()
                .ok_or(SyncError::Conflict("上がっていないファイルがあります"))?;
            let (done, _) = self.blob_paths(session_id, sha);
            let actual = fs::metadata(&done)
                .map_err(|_| SyncError::Conflict("上がっていないファイルがあります"))?;
            if actual.len() as i64 != *size {
                return Err(SyncError::Conflict(
                    "上がったファイルの大きさが一覧と違います",
                ));
            }
        }
        let (incoming_db, _) = self.blob_paths(session_id, &request.database_sha256);
        let database_changed = self.db.current().is_none()
            || self.store.meta(META_DATABASE_SHA)?.as_deref()
                != Some(request.database_sha256.as_str());
        if database_changed && !incoming_db.exists() {
            return Err(SyncError::Conflict("DB の写しが上がっていません"));
        }
        let stale: Vec<String> = {
            let conn = self.store.connection()?;
            let mut stmt = conn.prepare(
                "SELECT path FROM files WHERE path NOT IN (SELECT path FROM session_files WHERE session_id = ?1)",
            )?;
            let rows = stmt.query_map([session_id], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        if !database_changed && needed.is_empty() && stale.is_empty() {
            self.discard_session(session_id)?;
            return Ok(CommitOutcome {
                generation: self.generation(),
                works: self.work_count(),
                files_added: 0,
                files_removed: 0,
                missing_referenced: self.missing_referenced_count(),
                unchanged: true,
            });
        }

        let live = self.db.db_path().to_path_buf();
        let restoring = if database_changed {
            // SQLite は 260 文字を超えるパスを開けない。かたまりの置き場はハッシュの分だけ長いので、
            // 短い名前へ写してから開く。検証が済むまでは、いま読めるファイルも
            // 受信済みのかたまりも変えない。断ったあとでも再試行できる。
            let restoring = self.incoming_root.join("restoring.db");
            fs::copy(&incoming_db, &restoring)?;
            if let Err(error) = restore_on_server(&restoring, &self.library_root, Some(&live)) {
                let _ = fs::remove_file(&restoring);
                return Err(error.into());
            }
            Some(restoring)
        } else {
            None
        };

        let journal =
            self.prepare_publication(session_id, &needed, &stale, restoring.as_deref())?;
        self.stop_index_catch_up();
        let _restart_index = RestartIndex(self);
        let generation = self.generation() + 1;
        let result = self.db.replace_with_finalized(
            || self.apply_publication(&journal),
            || self.recover_pending_publication(),
            |candidate| {
                if journal.database_changed {
                    if let Some(db) = candidate {
                        // 世代の確定・公開より前に当てる。ここで止まっても、未確定なら旧DBへ
                        // 戻り、確定済みなら携帯の変更を含む新DBとして復旧できる。
                        self.store.reapply_favorites(db)?;
                    }
                }
                self.record_publication(&journal, generation, &request.database_sha256)
                    .map_err(|error| format!("世代を確定できません: {error:?}"))
            },
        );
        result?;
        // 世代はすでに確定した。片付けの失敗を送信失敗として扱わず、次の起動・送信で続ける。
        if let Err(error) = self.finish_publication(&journal) {
            log::warn!("確定済みの送信を片付けられません: {error}");
        }
        self.purge_trash();
        Ok(CommitOutcome {
            generation,
            works: self.work_count(),
            files_added: needed.len() as i64,
            files_removed: journal
                .changes
                .iter()
                .filter(|change| change.removal)
                .count() as i64,
            missing_referenced: self.missing_referenced_count(),
            unchanged: false,
        })
    }

    fn publication_dir(&self) -> PathBuf {
        self.incoming_root.join("publication")
    }

    fn backup_path(&self, journal: &PublicationJournal, index: usize) -> PathBuf {
        let change = &journal.changes[index];
        if change.removal {
            self.trash_root
                .join(&journal.trash_batch)
                .join(&change.path)
        } else {
            self.publication_dir().join(format!("old-{index}"))
        }
    }

    fn prepare_publication(
        &self,
        session_id: &str,
        needed: &[(String, i64, i64, Option<String>)],
        stale: &[String],
        restoring: Option<&Path>,
    ) -> Result<PublicationJournal, SyncError> {
        let dir = self.publication_dir();
        // この時点には未復旧の journal は無い。公開前の準備だけ残った場合は捨てられる。
        self.remove_publication()?;
        fs::create_dir_all(&dir)?;
        let mut journal = PublicationJournal {
            id: random_token(16),
            session_id: session_id.to_string(),
            changes: Vec::new(),
            database_changed: restoring.is_some(),
            had_database: self.db.db_path().exists(),
            trash_batch: format!(
                "{}-{}",
                chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
                random_token(6)
            ),
        };
        for (index, (path, _, _, sha)) in needed.iter().enumerate() {
            let target = self.library_path(path);
            if target.exists() && !target.is_file() {
                return Err(SyncError::Conflict(
                    "ファイルの置き場にフォルダーがあります",
                ));
            }
            let (blob, _) = self.blob_paths(
                session_id,
                sha.as_deref()
                    .ok_or(SyncError::Conflict("上がっていないファイルがあります"))?,
            );
            let prepared = dir.join(format!("new-{index}"));
            // 受信 blob は確定まで保持する。NTFS の hard link なら大きな本文も写さずに済み、
            // 対応しない置き場では本棚を閉じる前に写す。
            if fs::hard_link(&blob, &prepared).is_err() {
                fs::copy(&blob, &prepared)?;
            }
            OpenOptions::new().write(true).open(&prepared)?.sync_all()?;
            journal.changes.push(FileChange {
                path: path.clone(),
                had_previous: target.exists(),
                removal: false,
            });
        }
        for path in stale {
            let target = self.library_path(path);
            if target.exists() {
                journal.changes.push(FileChange {
                    path: path.clone(),
                    had_previous: true,
                    removal: true,
                });
            }
        }
        if let Some(restoring) = restoring {
            let previous = self.db.db_path().with_extension("db.previous");
            if previous.exists() && !previous.is_file() {
                return Err(SyncError::Conflict(
                    "DB の控えの置き場にフォルダーがあります",
                ));
            }
            fs::rename(restoring, dir.join("new-db"))?;
            OpenOptions::new()
                .write(true)
                .open(dir.join("new-db"))?
                .sync_all()?;
        }
        Ok(journal)
    }

    fn persist_publication(&self, journal: &PublicationJournal) -> Result<(), String> {
        let dir = self.publication_dir();
        let bytes = serde_json::to_vec(journal).map_err(|error| error.to_string())?;
        let temporary = dir.join("journal.tmp");
        let mut file = File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        rename_publication_file(&temporary, &dir.join("journal.json"))
            .map_err(|error| error.to_string())?;
        sync_directory(&dir).map_err(|error| error.to_string())?;
        publication_boundary();
        Ok(())
    }

    fn apply_publication(&self, journal: &PublicationJournal) -> Result<(), String> {
        self.persist_publication(journal)?;
        self.set_phase("placing_files");
        let dir = self.publication_dir();
        for (index, change) in journal.changes.iter().enumerate() {
            let target = self.library_path(&change.path);
            let backup = self.backup_path(journal, index);
            if change.had_previous {
                if let Some(parent) = backup.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                rename_publication_file(&target, &backup).map_err(|error| error.to_string())?;
                publication_boundary();
            }
            if !change.removal {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                rename_publication_file(&dir.join(format!("new-{index}")), &target)
                    .map_err(|error| error.to_string())?;
                publication_boundary();
            }
        }
        if journal.database_changed {
            self.set_phase("swapping");
            let live = self.db.db_path();
            remove_database_sidecars(live)?;
            publication_boundary();
            if journal.had_database {
                rename_publication_file(live, &dir.join("old-db"))
                    .map_err(|error| error.to_string())?;
                publication_boundary();
            }
            rename_publication_file(&dir.join("new-db"), live)
                .map_err(|error| error.to_string())?;
            publication_boundary();
        }
        Ok(())
    }

    fn rollback_publication(&self, journal: &PublicationJournal) -> Result<(), String> {
        for (index, change) in journal.changes.iter().enumerate().rev() {
            restore_file(
                &self.library_path(&change.path),
                &self.backup_path(journal, index),
                change.had_previous,
            )?;
        }
        if journal.database_changed {
            remove_database_sidecars(self.db.db_path())?;
            restore_file(
                self.db.db_path(),
                &self.publication_dir().join("old-db"),
                journal.had_database,
            )?;
        }
        Ok(())
    }

    fn record_publication(
        &self,
        journal: &PublicationJournal,
        generation: i64,
        database_sha: &str,
    ) -> Result<(), SyncError> {
        let mut conn = self.store.connection()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM files", [])?;
        tx.execute(
            "INSERT INTO files (path, size, mtime_ms)
             SELECT path, size, mtime_ms FROM session_files WHERE session_id = ?1",
            [&journal.session_id],
        )?;
        for (key, value) in [
            (META_GENERATION, generation.to_string()),
            (META_DATABASE_SHA, database_sha.to_string()),
            (META_COMMITTED_AT, chrono::Utc::now().to_rfc3339()),
            (META_PUBLICATION, journal.id.clone()),
        ] {
            tx.execute("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        }
        tx.execute(
            "DELETE FROM session_files WHERE session_id = ?1",
            [&journal.session_id],
        )?;
        tx.execute(
            "DELETE FROM meta WHERE key IN (?1, ?2)",
            [META_SESSION, META_SESSION_AT],
        )?;
        tx.commit()?;
        publication_boundary();
        Ok(())
    }

    fn finish_publication(&self, journal: &PublicationJournal) -> Result<(), String> {
        let old_db = self.publication_dir().join("old-db");
        if old_db.exists() {
            let previous = self.db.db_path().with_extension("db.previous");
            remove_file_if_present(&previous)?;
            rename_publication_file(&old_db, &previous).map_err(|error| error.to_string())?;
        }
        let session = self.session_dir(&journal.session_id);
        if session.exists() {
            fs::remove_dir_all(session).map_err(|error| error.to_string())?;
        }
        self.remove_publication()
    }

    fn remove_publication(&self) -> Result<(), String> {
        match fs::remove_dir_all(self.publication_dir()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("送信の復旧記録を片付けられません: {error}")),
        }
    }

    /// DB を開く前に呼ぶ。巻き戻しが失敗したら控えと journal を残し、公開を始めない。
    pub fn recover_pending_publication(&self) -> Result<(), String> {
        let path = self.publication_dir().join("journal.json");
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.remove_publication()
            }
            Err(error) => return Err(format!("送信の復旧記録を読めません: {error}")),
        };
        let journal: PublicationJournal = serde_json::from_slice(&bytes)
            .map_err(|error| format!("送信の復旧記録が壊れています: {error}"))?;
        if self.store.meta(META_PUBLICATION)?.as_deref() == Some(journal.id.as_str()) {
            self.finish_publication(&journal)
        } else {
            self.rollback_publication(&journal)?;
            self.remove_publication()
        }
    }

    fn recover_before_write(&self) -> Result<(), SyncError> {
        if self.publication_dir().exists() {
            // 未確定の復旧記録が残る場合、前の巻き戻しも失敗しており DB は閉じている。
            // 確定済みなら cleanup だけなので、読んでいる DB はそのままでよい。
            self.recover_pending_publication()?;
            if self.db.current().is_none() {
                self.db.open_existing()?;
            }
        }
        Ok(())
    }

    fn purge_trash(&self) {
        let Ok(entries) = fs::read_dir(&self.trash_root) else {
            return;
        };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok())
                .is_some_and(|age| age > TRASH_RETENTION);
            if old {
                if let Err(error) = fs::remove_dir_all(entry.path()) {
                    log::warn!("古いファイルを片付けられません: {error}");
                }
            }
        }
    }

    fn work_count(&self) -> i64 {
        self.db
            .current()
            .and_then(|db| db.get_stats().ok())
            .map_or(0, |stats| stats.total_downloads)
    }

    fn missing_referenced_count(&self) -> i64 {
        let Some(db) = self.db.current() else {
            return 0;
        };
        match db.server_referenced_paths() {
            Ok(paths) => paths
                .iter()
                .filter(|path| !Path::new(path).exists())
                .count() as i64,
            Err(error) => {
                log::warn!("参照先のファイルを確かめられません: {error}");
                0
            }
        }
    }

    /// 起動時と確定のあとに、変わった作品だけを索引し直す。意味索引は作らない。
    pub fn start_index_catch_up(&self) {
        self.stop_index_catch_up();
        let Some(db) = self.db.current() else {
            return;
        };
        let Ok(mut worker) = self.index.lock() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        worker.cancel = Arc::clone(&cancel);
        let pending = Arc::clone(&self.index_pending);
        let failure = Arc::clone(&self.index_error);
        worker.handle = Some(std::thread::spawn(move || {
            let run = || -> Result<(), String> {
                db.resync_search_index_state()?;
                let removed = db.prune_search_indexes()?;
                if removed > 0 {
                    log::info!("本棚から消えた作品 {removed} 件を索引から外しました");
                }
                let options = SearchIndexRebuildOptions {
                    include_semantic: false,
                    ..Default::default()
                };
                let stop = Arc::clone(&cancel);
                let progress_pending = Arc::clone(&pending);
                db.rebuild_search_index(
                    options,
                    &move || stop.load(Ordering::Relaxed),
                    move |progress| {
                        if let Ok(mut slot) = progress_pending.lock() {
                            *slot = Some((progress.total - progress.processed).max(0));
                        }
                    },
                )?;
                Ok(())
            };
            let outcome = run();
            if let Ok(mut slot) = failure.lock() {
                *slot = outcome.as_ref().err().cloned();
            }
            match outcome {
                Ok(()) => {
                    if !cancel.load(Ordering::Relaxed) {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(0);
                        }
                    }
                }
                Err(error) => log::error!("索引の追いつきに失敗しました: {error}"),
            }
        }));
    }

    /// 走っている索引の作業を止めて、終わるのを待つ。
    pub fn stop_index_catch_up(&self) {
        let handle = {
            let Ok(mut worker) = self.index.lock() else {
                return;
            };
            worker.cancel.store(true, Ordering::Relaxed);
            worker.handle.take()
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{} を消せません: {error}", path.display())),
    }
}

fn restore_file(target: &Path, backup: &Path, had_previous: bool) -> Result<(), String> {
    if backup.exists() {
        remove_file_if_present(target)?;
        rename_publication_file(backup, target)
            .map_err(|error| format!("{} を元に戻せません: {error}", target.display()))?;
    } else if !had_previous {
        remove_file_if_present(target)?;
    }
    Ok(())
}

fn remove_database_sidecars(live: &Path) -> Result<(), String> {
    for suffix in ["-wal", "-shm"] {
        remove_file_if_present(&PathBuf::from(format!("{}{suffix}", live.display())))?;
    }
    Ok(())
}

/// journal と公開ファイルの順序を、OS の書込みキャッシュにも反映させる。
#[cfg(windows)]
fn rename_publication_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    let source = source.canonicalize()?;
    let destination = destination
        .parent()
        .ok_or_else(|| io::Error::other("配置先の親がありません"))?
        .canonicalize()?
        .join(
            destination
                .file_name()
                .ok_or_else(|| io::Error::other("配置先の名前がありません"))?,
        );
    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: 両方のパスはNUL終端で、同期呼出しが終わるまで有効である。
    // 置換先は事前に退避済みなので、予期せず存在するファイルは上書きしない。
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(io::Error::other)
}

#[cfg(not(windows))]
fn rename_publication_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)?;
    if let Some(parent) = source.parent() {
        sync_directory(parent)?;
    }
    if let Some(parent) = destination.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

fn publication_boundary() {
    #[cfg(test)]
    publication_tests::interrupt_at_boundary();
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::database::{Database, NewDownload};
    use crate::server::snapshot::prepare_for_send;
    use crate::server::standalone::{assemble, ServerPaths, StandaloneParts};
    use crate::server::tests::TestRoot;
    use std::cell::Cell;

    const CHANGED: &str = "downloads/pixiv/1/original.json";
    const ADDED: &str = "downloads/pixiv/1/added.bin";
    const STALE: &str = "downloads/pixiv/1/stale.bin";
    thread_local! {
        static INTERRUPT_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(super) fn interrupt_at_boundary() {
        INTERRUPT_AFTER.with(|remaining| match remaining.get() {
            Some(1) => {
                remaining.set(None);
                panic!("simulated process interruption");
            }
            Some(count) => remaining.set(Some(count - 1)),
            None => {}
        });
    }

    fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn snapshot(root: &Path, title: &str) -> Vec<u8> {
        let source = root.join(title);
        fs::create_dir_all(&source).unwrap();
        let db = Database::open(&source.join("source.db"), &source.join("downloads")).unwrap();
        let body = source.join(CHANGED);
        db.upsert_download(&NewDownload {
            source: "pixiv".into(),
            source_id: "1".into(),
            title: title.into(),
            author_name: "author".into(),
            author_id: "author".into(),
            content_type: "novel".into(),
            tags: vec![],
            excerpt: None,
            cover_path: None,
            json_path: body.to_string_lossy().into_owned(),
            original_json_path: Some(body.to_string_lossy().into_owned()),
            asset_count: 0,
            file_size_bytes: 0,
            downloaded_at: "2026-11-06T00:00:00Z".into(),
            source_created_at: None,
            content_hash: Some(title.into()),
            text_length: 3,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        })
        .unwrap();
        let snapshot = source.join("snapshot.db");
        db.write_vacuumed_copy(&snapshot).unwrap();
        prepare_for_send(&snapshot, &source).unwrap();
        fs::read(snapshot).unwrap()
    }

    fn queue(receiver: &SyncReceiver, session: &str, path: &str, bytes: &[u8], mtime: i64) {
        receiver
            .add_manifest(
                session,
                &[ManifestEntry {
                    path: path.into(),
                    size: bytes.len() as u64,
                    mtime_ms: mtime,
                }],
            )
            .unwrap();
        let sha = hash(bytes);
        receiver
            .write_blob(session, &sha, 0, bytes.len() as u64, bytes)
            .unwrap();
        receiver
            .bind_files(
                session,
                &[BindEntry {
                    path: path.into(),
                    sha256: sha,
                }],
            )
            .unwrap();
    }

    fn seed(paths: &ServerPaths, database: &[u8]) -> StandaloneParts {
        let parts = assemble(paths, None, None).unwrap();
        let receiver = &parts.receiver;
        let session = receiver.open_session().unwrap().session_id;
        queue(receiver, &session, CHANGED, b"old", 1);
        queue(receiver, &session, STALE, b"stale", 1);
        let sha = hash(database);
        receiver
            .write_blob(&session, &sha, 0, database.len() as u64, database)
            .unwrap();
        receiver
            .commit(
                &session,
                &CommitRequest {
                    database_sha256: sha,
                },
            )
            .unwrap();
        receiver.stop_index_catch_up();
        parts
    }

    fn next(parts: &StandaloneParts, database: &[u8]) -> (String, CommitRequest) {
        let receiver = &parts.receiver;
        let session = receiver.open_session().unwrap().session_id;
        queue(receiver, &session, CHANGED, b"new", 2);
        queue(receiver, &session, ADDED, b"added", 2);
        let sha = hash(database);
        receiver
            .write_blob(&session, &sha, 0, database.len() as u64, database)
            .unwrap();
        (
            session,
            CommitRequest {
                database_sha256: sha,
            },
        )
    }

    fn assert_generation(parts: &StandaloneParts, paths: &ServerPaths, new: bool) {
        assert_eq!(parts.receiver.generation(), if new { 2 } else { 1 });
        assert_eq!(
            parts.db.current().unwrap().get_download(1).unwrap().title,
            if new { "new" } else { "old" }
        );
        assert_eq!(
            fs::read(paths.library().join(CHANGED)).unwrap(),
            if new { b"new" } else { b"old" }
        );
        assert_eq!(paths.library().join(STALE).exists(), !new);
        assert_eq!(paths.library().join(ADDED).exists(), new);
        let conn = parts.context.store.connection().unwrap();
        let mtime: i64 = conn
            .query_row(
                "SELECT mtime_ms FROM files WHERE path = ?1",
                [CHANGED],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mtime, if new { 2 } else { 1 });
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = ?1",
                [STALE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, i64::from(!new));
    }

    #[test]
    fn interrupted_publication_recovers_at_every_rename_and_commit_boundary() {
        let root = TestRoot::created("publication_interrupt");
        let old = snapshot(&root.0, "old");
        let new = snapshot(&root.0, "new");
        // journal、変更/追加/不要ファイル、sidecar、旧/新DB、状態DBのcommit。
        for boundary in 1..=9 {
            let paths = ServerPaths::new(&root.0.join(format!("server-{boundary}"))).unwrap();
            let parts = seed(&paths, &old);
            parts
                .context
                .store
                .apply_favorite(
                    &parts.db.current().unwrap(),
                    "pixiv",
                    "1",
                    "phone",
                    &crate::server::store::FavoriteUpdate {
                        favorite: true,
                        device_time: 1,
                    },
                )
                .unwrap()
                .unwrap();
            let (session, request) = next(&parts, &new);
            INTERRUPT_AFTER.with(|remaining| remaining.set(Some(boundary)));
            let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                parts.receiver.commit(&session, &request)
            }));
            assert!(interrupted.is_err(), "boundary {boundary}");
            INTERRUPT_AFTER.with(|remaining| assert_eq!(remaining.replace(None), None));
            parts.receiver.stop_index_catch_up();
            drop(parts);

            let reopened = assemble(&paths, None, None).unwrap();
            assert_generation(&reopened, &paths, boundary == 9);
            assert!(
                reopened
                    .db
                    .current()
                    .unwrap()
                    .get_download(1)
                    .unwrap()
                    .favorite
            );
            assert!(!reopened.receiver.publication_dir().exists());
            if boundary < 9 {
                assert!(
                    reopened
                        .receiver
                        .blob_state(&session, &hash(b"new"))
                        .unwrap()
                        .complete
                );
                assert_eq!(
                    reopened
                        .receiver
                        .commit(&session, &request)
                        .unwrap()
                        .generation,
                    2
                );
                reopened.receiver.stop_index_catch_up();
            } else {
                // commit 直後の中断は新世代を採用し、次の送信はアップロード無しで済む。
                assert!(!reopened.receiver.status().session_open);
                let session = reopened.receiver.open_session().unwrap().session_id;
                assert!(reopened
                    .receiver
                    .add_manifest(
                        &session,
                        &[
                            ManifestEntry {
                                path: CHANGED.into(),
                                size: 3,
                                mtime_ms: 2
                            },
                            ManifestEntry {
                                path: ADDED.into(),
                                size: 5,
                                mtime_ms: 2
                            },
                        ]
                    )
                    .unwrap()
                    .is_empty());
                assert!(
                    reopened
                        .receiver
                        .commit(&session, &request)
                        .unwrap()
                        .unchanged
                );
            }
            assert_generation(&reopened, &paths, true);
            assert!(
                reopened
                    .db
                    .current()
                    .unwrap()
                    .get_download(1)
                    .unwrap()
                    .favorite
            );
        }
    }

    #[test]
    fn a_state_database_write_failure_restores_files_database_and_retryable_blobs() {
        let root = TestRoot::created("publication_metadata_failure");
        let paths = ServerPaths::new(&root.0.join("server")).unwrap();
        let parts = seed(&paths, &snapshot(&root.0, "old"));
        let (session, request) = next(&parts, &snapshot(&root.0, "new"));
        parts.context.store.connection().unwrap().execute_batch(
            "CREATE TEMP TRIGGER fail_generation BEFORE INSERT ON meta WHEN NEW.key = 'generation'
             BEGIN SELECT RAISE(FAIL, 'simulated metadata write failure'); END;"
        ).unwrap();
        assert!(parts.receiver.commit(&session, &request).is_err());
        parts.receiver.stop_index_catch_up();
        assert_generation(&parts, &paths, false);
        assert!(
            parts
                .receiver
                .blob_state(&session, &hash(b"new"))
                .unwrap()
                .complete
        );
        parts
            .context
            .store
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_generation")
            .unwrap();
        assert_eq!(
            parts
                .receiver
                .commit(&session, &request)
                .unwrap()
                .generation,
            2
        );
        parts.receiver.stop_index_catch_up();
        assert_generation(&parts, &paths, true);
    }

    #[test]
    fn expired_abandoned_sessions_do_not_keep_updates_waiting_but_active_writes_do() {
        let root = TestRoot::created("session_expiry");
        let paths = ServerPaths::new(&root.0.join("server")).unwrap();
        let parts = assemble(&paths, None, None).unwrap();
        let receiver = &parts.receiver;
        assert!(!receiver.status().session_open);
        let first = receiver.open_session().unwrap().session_id;
        assert!(receiver.status().session_open);

        let expired = chrono::Utc::now() - SESSION_LIFETIME - chrono::TimeDelta::seconds(1);
        receiver
            .store
            .set_meta(META_SESSION_AT, &expired.to_rfc3339())
            .unwrap();
        assert!(
            !receiver.status().session_open,
            "再送を開かなくても quiet 待機を通過する"
        );
        {
            let _writing = receiver.write_lock.lock().unwrap();
            assert!(
                receiver.status().session_open,
                "期限をまたいだ実際の書込みは待つ"
            );
        }
        assert!(!receiver.status().session_open);
        assert!(
            receiver.session_dir(&first).exists(),
            "status はファイルを削除しない"
        );
        let fresh = receiver.open_session().unwrap();
        assert!(!fresh.resumed);
        assert_ne!(fresh.session_id, first);
        assert!(receiver.status().session_open);
        assert!(!receiver.session_dir(&first).exists());
        receiver.store.set_meta(META_SESSION_AT, "invalid").unwrap();
        assert!(!receiver.status().session_open);
    }

    #[test]
    #[cfg(windows)]
    fn a_locked_stale_file_leaves_the_old_generation_and_the_same_session_can_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = TestRoot::created("publication_stale_lock");
        let paths = ServerPaths::new(&root.0.join("server")).unwrap();
        let parts = seed(&paths, &snapshot(&root.0, "old"));
        let (session, request) = next(&parts, &snapshot(&root.0, "new"));
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(paths.library().join(STALE))
            .unwrap();
        assert!(parts.receiver.commit(&session, &request).is_err());
        parts.receiver.stop_index_catch_up();
        drop(locked);
        assert_generation(&parts, &paths, false);
        assert_eq!(
            parts
                .receiver
                .commit(&session, &request)
                .unwrap()
                .generation,
            2
        );
        parts.receiver.stop_index_catch_up();
        assert_generation(&parts, &paths, true);
    }

    #[test]
    #[cfg(windows)]
    fn a_failed_rollback_keeps_the_journal_and_recovers_before_a_same_session_retry() {
        use std::cell::RefCell;
        use std::os::windows::fs::OpenOptionsExt;
        let root = TestRoot::created("publication_rollback_lock");
        let paths = ServerPaths::new(&root.0.join("server")).unwrap();
        let parts = seed(&paths, &snapshot(&root.0, "old"));
        let (session, request) = next(&parts, &snapshot(&root.0, "new"));
        let receiver = &parts.receiver;
        let restoring = paths.incoming().join("restoring.db");
        fs::copy(
            receiver.blob_paths(&session, &request.database_sha256).0,
            &restoring,
        )
        .unwrap();
        restore_on_server(&restoring, &paths.library(), Some(&paths.database())).unwrap();
        let needed = vec![
            (CHANGED.into(), 3, 2, Some(hash(b"new"))),
            (ADDED.into(), 5, 2, Some(hash(b"added"))),
        ];
        let journal = receiver
            .prepare_publication(&session, &needed, &[STALE.into()], Some(&restoring))
            .unwrap();
        let locked = RefCell::new(None);
        assert!(parts
            .db
            .replace_with(
                || {
                    receiver.apply_publication(&journal)?;
                    *locked.borrow_mut() = Some(
                        OpenOptions::new()
                            .read(true)
                            .share_mode(0)
                            .open(paths.library().join(CHANGED))
                            .unwrap(),
                    );
                    Err("simulated failure after installation".into())
                },
                || receiver.recover_pending_publication(),
            )
            .is_err());
        assert!(parts.db.current().is_none());
        assert!(receiver.publication_dir().join("journal.json").exists());
        assert!(receiver.publication_dir().join("old-db").exists());
        let expired = chrono::Utc::now() - SESSION_LIFETIME - chrono::TimeDelta::seconds(1);
        receiver
            .store
            .set_meta(META_SESSION_AT, &expired.to_rfc3339())
            .unwrap();
        assert!(
            receiver.status().session_open,
            "期限切れでも未復旧の公開DBを更新へ渡さない"
        );
        drop(locked);
        assert_eq!(receiver.commit(&session, &request).unwrap().generation, 2);
        receiver.stop_index_catch_up();
        assert_generation(&parts, &paths, true);
    }
}
