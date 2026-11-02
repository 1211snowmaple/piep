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
        let database_sha256 = self.store.meta(META_DATABASE_SHA)?;
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
        let database_changed = self.store.meta(META_DATABASE_SHA)?.as_deref()
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

        self.set_phase("placing_files");
        self.place_files(session_id, &needed)?;

        if database_changed {
            self.set_phase("swapping");
            // 索引の作業は本棚を掴んでいるので、先に止める。
            self.stop_index_catch_up();
            let live = self.db.db_path().to_path_buf();
            // SQLite は 260 文字を超えるパスを開けない。かたまりの置き場はハッシュの分だけ長いので、
            // 短い名前へ移してから開く。失敗すれば捨てる。送り手が上げ直せばよい。
            let restoring = self.incoming_root.join("restoring.db");
            fs::rename(&incoming_db, &restoring)?;
            if let Err(error) = restore_on_server(&restoring, &self.library_root, Some(&live)) {
                let _ = fs::remove_file(&restoring);
                return Err(error.into());
            }
            let install_target = live.clone();
            let install_source = restoring.clone();
            self.db
                .replace_with(move || install_database(&install_source, &install_target))?;
        }

        self.set_phase("removing_stale");
        let files_removed = self.move_to_trash(&stale)?;
        self.purge_trash();

        let generation = self.generation() + 1;
        self.store
            .set_meta(META_GENERATION, &generation.to_string())?;
        self.store
            .set_meta(META_DATABASE_SHA, &request.database_sha256)?;
        self.store
            .set_meta(META_COMMITTED_AT, &chrono::Utc::now().to_rfc3339())?;
        self.discard_session(session_id)?;
        self.start_index_catch_up();
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
    ) -> Result<(), SyncError> {
        for (index, (path, size, mtime_ms, sha)) in needed.iter().enumerate() {
            let Some(sha) = sha.as_deref() else {
                return Err(SyncError::Conflict("上がっていないファイルがあります"));
            };
            let (blob, _) = self.blob_paths(session_id, sha);
            let target = self.library_path(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            // 同じ中身のファイルは sha の順に並んでいる。最後の 1 つだけ動かせばよい。
            let used_again = needed
                .get(index + 1)
                .is_some_and(|next| next.3.as_deref() == Some(sha));
            let temporary = target.with_extension(format!("piep-incoming-{}", random_token(6)));
            if used_again {
                fs::copy(&blob, &temporary)?;
            } else {
                fs::rename(&blob, &temporary)?;
            }
            if let Err(error) = fs::rename(&temporary, &target) {
                let _ = fs::remove_file(&temporary);
                return Err(error.into());
            }
            self.store.connection()?.execute(
                "INSERT INTO files (path, size, mtime_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ms = excluded.mtime_ms",
                params![path, size, mtime_ms],
            )?;
        }
        // 変わっていないファイルも、次の比較のために記録を揃える。
        self.store.connection()?.execute(
            "INSERT INTO files (path, size, mtime_ms)
             SELECT path, size, mtime_ms FROM session_files WHERE session_id = ?1 AND needed = 0
             ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ms = excluded.mtime_ms",
            [session_id],
        )?;
        Ok(())
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
fn install_database(incoming: &Path, live: &Path) -> Result<(), String> {
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
    }
    if let Err(detail) = fs::rename(incoming, live) {
        if previous.exists() {
            let _ = fs::rename(&previous, live);
        }
        return Err(error(detail));
    }
    Ok(())
}
