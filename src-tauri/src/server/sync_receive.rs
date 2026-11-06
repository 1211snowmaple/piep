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
use std::cell::{Cell, RefCell};
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

/// 置いたファイル1つ分の、元へ戻すための控え。
struct PlacedFile {
    target: PathBuf,
    /// どけておいた前の中身。
    previous: Option<PathBuf>,
    /// ここから動かしてきたかたまり。写して置いたものは消すだけで戻る。
    blob: Option<PathBuf>,
}

/// 公開中のファイルの控え。DB を開き直すまで、前の中身を戻せるようにする。
#[derive(Default)]
struct FilePublication {
    files: Vec<PlacedFile>,
}

struct RestartIndex<'a>(&'a SyncReceiver);

impl Drop for RestartIndex<'_> {
    fn drop(&mut self) {
        self.0.start_index_catch_up();
    }
}

impl FilePublication {
    fn rollback(&mut self) -> Result<(), String> {
        while let Some(file) = self.files.last() {
            // 途中で失敗したら、もう一度ここから呼ばれる。済んだ手順は飛ばせる形にする。
            let cleared = match &file.blob {
                Some(blob) => fs::rename(&file.target, blob),
                None => fs::remove_file(&file.target),
            };
            let result = match cleared {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
            .and_then(|()| match &file.previous {
                Some(previous) => fs::rename(previous, &file.target),
                None => Ok(()),
            });
            result
                .map_err(|error| format!("{} を元に戻せません: {error}", file.target.display()))?;
            self.files.pop();
        }
        Ok(())
    }

    fn accept(mut self) {
        for file in self.files.drain(..) {
            if let Some(previous) = file.previous {
                if let Err(error) = fs::remove_file(&previous) {
                    log::warn!("確定済みファイルの控えを片付けられません: {error}");
                }
            }
        }
    }
}

impl Drop for FilePublication {
    fn drop(&mut self) {
        if let Err(error) = self.rollback() {
            log::error!("送信の巻き戻しに失敗しました: {error}");
        }
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
        ReceiverStatus {
            generation: self.generation(),
            committed_at: self.store.meta(META_COMMITTED_AT).ok().flatten(),
            session_open: self.store.meta(META_SESSION).ok().flatten().is_some(),
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

    /// 送信を開く。途中の送信があれば続きとして返す。24 時間より古ければ捨てる。
    pub fn open_session(&self) -> Result<OpenedSession, SyncError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| SyncError::Conflict("確定の途中です"))?;
        let database_sha256 = if self.db.current().is_some() {
            self.store.meta(META_DATABASE_SHA)?
        } else {
            None
        };
        if let Some(existing) = self.store.meta(META_SESSION)? {
            let fresh = self
                .store
                .meta(META_SESSION_AT)?
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
                .is_some_and(|at| chrono::Utc::now().signed_duration_since(at) < SESSION_LIFETIME);
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

        // ファイルも本棚を読む要求が終わってから切り替える。途中の失敗では、
        // ファイルを戻してから旧DBを開き直す。
        self.stop_index_catch_up();
        let _restart_index = RestartIndex(self);
        let publication = RefCell::new(None::<FilePublication>);
        let database_moved = Cell::new(false);
        let had_database = live.exists();
        let result = self.db.replace_with(
            || {
                self.set_phase("placing_files");
                *publication.borrow_mut() = Some(
                    self.place_files(session_id, &needed)
                        .map_err(|error| format!("ファイルを配置できません: {error:?}"))?,
                );
                if let Some(restoring) = &restoring {
                    self.set_phase("swapping");
                    install_database(restoring, &live, &database_moved)?;
                }
                Ok(())
            },
            || {
                if let Some(files) = publication.borrow_mut().as_mut() {
                    files.rollback()?;
                }
                if database_moved.get() {
                    restore_previous_database(&live, had_database)?;
                }
                Ok(())
            },
        );
        if let Err(error) = result {
            return Err(error.into());
        }
        if let Some(files) = publication.into_inner() {
            files.accept();
        }
        // 携帯で変えて、デスクトップ版がまだ取り込んでいないお気に入りは、送られてきた本棚に無い。
        // 取り込まれるまでは当て直す。当てられなくても送信は済んでいるので、記録だけ残す。
        if database_changed {
            if let Some(db) = self.db.current() {
                if let Err(error) = self.store.reapply_favorites(&db) {
                    log::warn!("お気に入りを当て直せません: {error}");
                }
            }
        }

        self.set_phase("removing_stale");
        let files_removed = self.move_to_trash(&stale)?;
        self.purge_trash();

        let generation = self.generation() + 1;
        {
            let mut conn = self.store.connection()?;
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO files (path, size, mtime_ms)
                 SELECT path, size, mtime_ms FROM session_files WHERE session_id = ?1
                 ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ms = excluded.mtime_ms",
                [session_id],
            )?;
            for (key, value) in [
                (META_GENERATION, generation.to_string()),
                (META_DATABASE_SHA, request.database_sha256.clone()),
                (META_COMMITTED_AT, chrono::Utc::now().to_rfc3339()),
            ] {
                tx.execute("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
            }
            tx.commit()?;
        }
        self.discard_session(session_id)?;
        Ok(CommitOutcome {
            generation,
            works: self.work_count(),
            files_added: needed.len() as i64,
            files_removed,
            missing_referenced: self.missing_referenced_count(),
            unchanged: false,
        })
    }

    fn place_files(
        &self,
        session_id: &str,
        needed: &[(String, i64, i64, Option<String>)],
    ) -> Result<FilePublication, SyncError> {
        let mut publication = FilePublication::default();
        let rollback_dir = self.session_dir(session_id).join("rollback");
        let result = (|| -> Result<(), SyncError> {
            for (index, (path, _, _, sha)) in needed.iter().enumerate() {
                let Some(sha) = sha.as_deref() else {
                    return Err(SyncError::Conflict("上がっていないファイルがあります"));
                };
                let (blob, _) = self.blob_paths(session_id, sha);
                let target = self.library_path(path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                // 動かして置き、失敗したら動かして戻す。本棚を閉じている間なので、写すと
                // 止まる時間も要る空き容量も本棚の大きさに比例してしまう。写すのは、同じ中身が
                // まだ後に続くときだけ（同じ中身は sha の順に並んでいる）。
                let temporary = needed
                    .get(index + 1)
                    .is_some_and(|next| next.3.as_deref() == Some(sha))
                    .then(|| target.with_extension(format!("piep-incoming-{}", random_token(6))));
                if let Some(temporary) = &temporary {
                    if let Err(error) = fs::copy(&blob, temporary) {
                        let _ = fs::remove_file(temporary);
                        return Err(error.into());
                    }
                }
                let placed = (|| -> Result<(), SyncError> {
                    let previous = if target.exists() {
                        if !target.is_file() {
                            return Err(SyncError::Conflict(
                                "ファイルの置き場にフォルダーがあります",
                            ));
                        }
                        fs::create_dir_all(&rollback_dir)?;
                        let previous = rollback_dir.join(random_token(12));
                        fs::rename(&target, &previous)?;
                        Some(previous)
                    } else {
                        None
                    };
                    publication.files.push(PlacedFile {
                        target: target.clone(),
                        previous,
                        blob: temporary.is_none().then(|| blob.clone()),
                    });
                    fs::rename(temporary.as_ref().unwrap_or(&blob), &target)?;
                    Ok(())
                })();
                if let Some(temporary) = &temporary {
                    let _ = fs::remove_file(temporary);
                }
                placed?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            publication
                .rollback()
                .map_err(|recovery| SyncError::Internal(format!("{error:?}。{recovery}")))?;
            return Err(error);
        }
        Ok(publication)
    }

    /// 一覧から消えたファイルを `library-trash` へ移す。入れ替えのあとなので、読んでいる世代は困らない。
    fn move_to_trash(&self, stale: &[String]) -> Result<i64, SyncError> {
        if stale.is_empty() {
            return Ok(0);
        }
        let batch = self
            .trash_root
            .join(chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string());
        let mut moved = 0;
        for relative in stale {
            let source = self.library_path(relative);
            if source.exists() {
                let mut target = batch.clone();
                target.extend(relative.split('/'));
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(&source, &target)?;
                moved += 1;
            }
            self.store
                .connection()?
                .execute("DELETE FROM files WHERE path = ?1", [relative])?;
        }
        Ok(moved)
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

/// 受け取った DB を本棚の DB に置き換える。直前の DB を 1 つ残す。
fn install_database(incoming: &Path, live: &Path, moved: &Cell<bool>) -> Result<(), String> {
    let error = |detail: io::Error| format!("DB を入れ替えられません: {detail}");
    let previous = live.with_extension("db.previous");
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", live.display()));
        if sidecar.exists() {
            fs::remove_file(sidecar).map_err(error)?;
        }
    }
    if live.exists() {
        if previous.exists() {
            fs::remove_file(&previous).map_err(error)?;
        }
        fs::rename(live, &previous).map_err(error)?;
        moved.set(true);
    }
    fs::rename(incoming, live).map_err(error)?;
    moved.set(true);
    Ok(())
}

fn restore_previous_database(live: &Path, had_database: bool) -> Result<(), String> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", live.display()));
        match fs::remove_file(sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("DB の作業ファイルを戻せません: {error}")),
        }
    }
    if had_database {
        fs::rename(live.with_extension("db.previous"), live)
            .map_err(|error| format!("前の DB を戻せません: {error}"))
    } else {
        fs::remove_file(live).map_err(|error| format!("未確定の DB を消せません: {error}"))
    }
}
