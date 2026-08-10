//! データベースCRUD操作。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use regex::Regex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::models::*;
use super::schema;
use super::search::{
    extract_search_body, generate_ngrams_limited, make_match_highlights, match_fields_and_score,
    normalize_search_text, normalized_levenshtein, parse_search_query, query_ngrams,
    ParsedSearchQuery, SearchDocument,
};

#[derive(Debug, Clone)]
struct RankedSearchHit {
    download_id: i64,
    score: f64,
    semantic: Option<super::semantic_index::SemanticSearchHit>,
    document: Option<Arc<SearchDocument>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ReaderCacheKey {
    storage: PathBuf,
    download_id: i64,
    requested_version: Option<i64>,
    stamp: String,
}

#[derive(Clone)]
struct ReaderCacheEntry {
    pages: Arc<Vec<String>>,
    total_plain_text_chars: usize,
    bytes: usize,
    last_used: u64,
}

const READER_CACHE_MAX_DOCUMENTS: usize = 8;
const READER_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
static READER_CONTENT_CACHE: OnceLock<
    parking_lot::Mutex<HashMap<ReaderCacheKey, ReaderCacheEntry>>,
> = OnceLock::new();
static READER_CACHE_TICK: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
struct SearchIndexBuildDocument {
    download_id: i64,
    current_version: i64,
    content_hash: Option<String>,
    tantivy: super::tantivy_index::TantivyIndexDocument,
    semantic: super::semantic_index::SemanticIndexDocument,
}

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticsProgress {
    pub step: i64,
    pub total: i64,
    pub phase: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct SearchIndexRebuildOptions {
    /// How many documents are read and analysed together. Larger chunks keep
    /// every core busy; smaller ones report progress more often.
    pub chunk_size: usize,
    /// Documents buffered before a commit. Each commit ends a segment, so this
    /// trades restart granularity against how fragmented the index becomes.
    pub commit_every: usize,
    /// Also build the semantic vectors, which is the only part of indexing that
    /// can use the GPU - and much slower than the lexical pass.
    pub include_semantic: bool,
}

impl Default for SearchIndexRebuildOptions {
    fn default() -> Self {
        Self {
            chunk_size: 64,
            commit_every: 2_000,
            include_semantic: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SearchIndexRebuildProgress {
    pub phase: &'static str,
    pub processed: i64,
    pub total: i64,
    pub failed: i64,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct SearchIndexRebuildOutcome {
    pub processed: i64,
    pub failed: i64,
    pub canceled: bool,
}

#[derive(Debug, Clone)]
struct IndexedState {
    download_id: i64,
    current_version: i64,
    content_hash: Option<String>,
}

struct ReadyChunkEntry {
    prepared: super::tantivy_index::Prepared,
    state: IndexedState,
    semantic: super::semantic_index::SemanticIndexDocument,
}

enum ChunkEntry {
    Missing(i64),
    // Boxed so a chunk of results is not sized by its largest variant: the
    // ready one carries a whole prepared document.
    Ready(Box<ReadyChunkEntry>),
}

#[derive(Default)]
struct PreparedChunk {
    documents: Vec<super::tantivy_index::Prepared>,
    indexed: Vec<IndexedState>,
    semantic: Vec<super::semantic_index::SemanticIndexDocument>,
    missing: Vec<i64>,
    prepared_count: i64,
    failed: i64,
}

/// Indicates whether an entity row came from a real profile fetch or a lightweight work snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityProfileFreshness {
    SnapshotOnly,
    RemoteChecked,
}

impl EntityProfileFreshness {
    fn checked_at(self, now: &str) -> Option<&str> {
        match self {
            EntityProfileFreshness::SnapshotOnly => None,
            EntityProfileFreshness::RemoteChecked => Some(now),
        }
    }
}

fn build_read_pool(db_path: &Path) -> Result<Pool<SqliteConnectionManager>, String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let manager = SqliteConnectionManager::file(db_path)
        .with_flags(flags)
        .with_init(|conn| {
            conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
            conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;
                PRAGMA temp_store = MEMORY;
                PRAGMA mmap_size = 268435456;
                PRAGMA cache_size = -64000;
                ",
            )
        });
    let parallelism = std::thread::available_parallelism()
        .map(|value| value.get() as u32)
        .unwrap_or(4)
        .clamp(4, 16);
    Pool::builder()
        .max_size(parallelism)
        .min_idle(Some(1))
        .build(manager)
        .map_err(|e| format!("DB read pool creation failed: {}", e))
}

fn file_size_or_zero(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn recursive_file_size(root: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_file() => file_size_or_zero(&path),
                Ok(kind) if kind.is_dir() && !kind.is_symlink() => recursive_file_size(&path),
                _ => 0,
            }
        })
        .sum()
}

fn recursive_file_count(root: &Path, extension: Option<&str>) -> u64 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_file() => match extension {
                    Some(expected) => {
                        u64::from(path.extension().and_then(|ext| ext.to_str()) == Some(expected))
                    }
                    None => 1,
                },
                Ok(kind) if kind.is_dir() && !kind.is_symlink() => {
                    recursive_file_count(&path, extension)
                }
                _ => 0,
            }
        })
        .sum()
}

fn normalized_diagnostic_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn orphan_asset_file_stats(root: &Path, known: &std::collections::HashSet<String>) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return (0, 0);
    };
    let mut count = 0u64;
    let mut bytes = 0u64;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && !kind.is_symlink() => {
                let (child_count, child_bytes) = orphan_asset_file_stats(&path, known);
                count = count.saturating_add(child_count);
                bytes = bytes.saturating_add(child_bytes);
            }
            Ok(kind) if kind.is_file() => {
                let is_asset = path.components().any(|part| {
                    part.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("data_assets")
                });
                if is_asset && !known.contains(&normalized_diagnostic_path(&path)) {
                    count = count.saturating_add(1);
                    bytes = bytes.saturating_add(file_size_or_zero(&path));
                }
            }
            _ => {}
        }
    }
    (count, bytes)
}

fn benchmark_percentiles(samples: &mut [f64]) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_by(|left, right| left.total_cmp(right));
    let p50 = samples[(samples.len() - 1) / 2];
    let p95_index = ((samples.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len() - 1);
    (p50, samples[p95_index])
}

#[cfg(windows)]
pub(crate) fn available_space_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_bytes: *mut u64,
            total_free_bytes: *mut u64,
        ) -> i32;
    }
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut available = 0u64;
    // SAFETY: `wide` is a NUL-terminated immutable UTF-16 path and the output
    // pointer is valid for a u64 for the duration of the call.
    let success = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (success != 0).then_some(available)
}

#[cfg(unix)]
pub(crate) fn available_space_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        __spare: [i32; 6],
    }
    extern "C" {
        fn statvfs(path: *const std::ffi::c_char, output: *mut Statvfs) -> i32;
    }
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut output = std::mem::MaybeUninit::<Statvfs>::uninit();
    // SAFETY: the C string is NUL terminated and statvfs initializes the
    // output struct when it returns zero.
    let success = unsafe { statvfs(path.as_ptr(), output.as_mut_ptr()) };
    if success != 0 {
        return None;
    }
    let output = unsafe { output.assume_init() };
    Some(output.f_bavail.saturating_mul(output.f_frsize))
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn available_space_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(windows)]
fn current_process_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    // SAFETY: both functions are stable Win32 APIs. `counters` is initialized,
    // writable and its exact byte size is passed to the operating system.
    let success = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCounters>() as u32,
        )
    };
    (success != 0).then_some(counters.working_set_size as u64)
}

#[cfg(not(windows))]
fn current_process_memory_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    Some(kib.saturating_mul(1024))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchCursor {
    kind: String,
    scope: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    value: Option<String>,
    id: Option<i64>,
    score: Option<f64>,
    downloaded_at: Option<String>,
    #[serde(default)]
    tantivy_score: Option<f32>,
    #[serde(default)]
    tantivy_segment_ord: Option<u32>,
    #[serde(default)]
    tantivy_doc_id: Option<u32>,
}

/// スレッドセーフなデータベースハンドル
struct RestoreAwareConnection {
    inner: Mutex<Connection>,
    restore_owner: Mutex<Option<std::thread::ThreadId>>,
}

impl RestoreAwareConnection {
    fn new(connection: Connection) -> Self {
        Self {
            inner: Mutex::new(connection),
            restore_owner: Mutex::new(None),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        let current = std::thread::current().id();
        let owner = self.restore_owner.lock().map_err(|e| e.to_string())?;
        if owner.is_some_and(|owner| owner != current) {
            return Err("Library restore is in progress; try again when it completes".to_string());
        }
        drop(owner);
        self.inner.lock().map_err(|e| e.to_string())
    }

    fn begin_restore_scope(&self) -> Result<(), String> {
        let mut owner = self.restore_owner.lock().map_err(|e| e.to_string())?;
        if owner.is_some() {
            return Err("Another library restore is already in progress".to_string());
        }
        *owner = Some(std::thread::current().id());
        Ok(())
    }

    fn end_restore_scope(&self) {
        if let Ok(mut owner) = self.restore_owner.lock() {
            *owner = None;
        }
    }
}

/// How long a search index status reading stays usable.
///
/// Every search reads it, and computing it means three library-wide COUNT(*)
/// queries plus opening the semantic sidecar. It only feeds an informational
/// banner and a cache key, so a reading that is a fraction of a second old is
/// indistinguishable from a fresh one - and the paths that actually change it
/// drop the cache outright.
const INDEX_STATUS_CACHE_TTL: Duration = Duration::from_millis(750);

struct CachedIndexStatus {
    read_at: Instant,
    status: SearchIndexStatus,
}

pub struct Database {
    conn: RestoreAwareConnection,
    read_pool: Pool<SqliteConnectionManager>,
    db_path: PathBuf,
    storage_dir: PathBuf,
    index_status_cache: Mutex<Option<CachedIndexStatus>>,
}

impl Database {
    /// データベースを開く（存在しなければ作成）
    pub fn open(db_path: &Path, storage_dir: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| format!("DB open failed: {}", e))?;
        schema::initialize(&conn).map_err(|e| format!("DB init failed: {}", e))?;
        reconcile_search_index_format(&conn)?;
        let read_pool = build_read_pool(db_path)?;

        // ストレージディレクトリを作成
        std::fs::create_dir_all(storage_dir)
            .map_err(|e| format!("Storage dir creation failed: {}", e))?;

        Ok(Self {
            conn: RestoreAwareConnection::new(conn),
            read_pool,
            db_path: db_path.to_path_buf(),
            storage_dir: storage_dir.to_path_buf(),
            index_status_cache: Mutex::new(None),
        })
    }

    /// ストレージディレクトリのパスを取得
    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    pub(crate) fn begin_atomic_restore(&self) -> Result<(), String> {
        self.conn.begin_restore_scope()?;
        let result = self
            .conn
            .lock()?
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("Restore transaction begin failed: {e}"));
        if result.is_err() {
            self.conn.end_restore_scope();
        }
        result
    }

    pub(crate) fn commit_atomic_restore(&self) -> Result<(), String> {
        let result = self
            .conn
            .lock()?
            .execute_batch("COMMIT")
            .map_err(|e| format!("Restore transaction commit failed: {e}"));
        self.conn.end_restore_scope();
        result
    }

    pub(crate) fn rollback_atomic_restore(&self) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute_batch("ROLLBACK");
        }
        self.conn.end_restore_scope();
    }

    pub(crate) fn delete_download_record_for_restore(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .map_err(|e| format!("Restore delete failed: {e}"))?;
        Ok(())
    }

    fn read_conn(&self) -> Result<PooledConnection<SqliteConnectionManager>, String> {
        self.read_pool
            .get()
            .map_err(|e| format!("DB read pool checkout failed: {}", e))
    }

    pub fn reindex_download(&self, download_id: i64) -> Result<(), String> {
        let result = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            reindex_download_locked(&conn, &self.storage_dir, download_id)
        };
        self.invalidate_index_status();
        result
    }

    pub fn get_search_index_status(&self) -> Result<SearchIndexStatus, String> {
        if let Ok(cache) = self.index_status_cache.lock() {
            if let Some(cached) = cache.as_ref() {
                if cached.read_at.elapsed() < INDEX_STATUS_CACHE_TTL {
                    return Ok(cached.status.clone());
                }
            }
        }
        let status = {
            let conn = self.read_conn()?;
            search_index_status_locked(&conn, &self.storage_dir)?
        };
        if let Ok(mut cache) = self.index_status_cache.lock() {
            *cache = Some(CachedIndexStatus {
                read_at: Instant::now(),
                status: status.clone(),
            });
        }
        Ok(status)
    }

    /// Forces the next status read to measure the library again. Called from
    /// every path that adds, removes or reindexes a work, so the settings and
    /// diagnostics screens never show a figure that is already wrong.
    fn invalidate_index_status(&self) {
        if let Ok(mut cache) = self.index_status_cache.lock() {
            *cache = None;
        }
    }

    pub fn rebuild_search_index_batch(&self, limit: i64) -> Result<SearchIndexStatus, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let limit = limit.clamp(1, 200);
        let ids = stale_search_index_ids_locked(&conn, limit, 0)?;
        let mut docs = Vec::with_capacity(ids.len());
        for id in &ids {
            match search_index_document_locked(&conn, &self.storage_dir, *id) {
                Ok(Some(doc)) => docs.push(doc),
                Ok(None) => {
                    if let Err(e) = clear_search_index_locked(&conn, &self.storage_dir, *id) {
                        log::warn!("Failed to clear missing search index row {}: {}", id, e);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to prepare search index document {}: {}", id, e);
                }
            }
        }
        if let Err(e) = index_search_documents_locked(&conn, &self.storage_dir, &docs, false) {
            log::warn!("Failed to rebuild search index batch: {}", e);
        }
        let status = search_index_status_locked(&conn, &self.storage_dir);
        drop(conn);
        self.invalidate_index_status();
        status
    }

    /// Brings the whole search index up to date in one pass.
    ///
    /// The work splits cleanly in two: preparing a document is pure CPU and runs
    /// across every core, while the index writer and the SQLite bookkeeping are
    /// serial. Holding one writer open for the entire run - rather than building
    /// and committing one per small batch - is what keeps the segment count and
    /// the wall clock down.
    pub fn rebuild_search_index<F>(
        &self,
        options: SearchIndexRebuildOptions,
        should_cancel: &dyn Fn() -> bool,
        mut on_progress: F,
    ) -> Result<SearchIndexRebuildOutcome, String>
    where
        F: FnMut(SearchIndexRebuildProgress),
    {
        let started = std::time::Instant::now();
        let total_pending = {
            let conn = self.read_conn()?;
            pending_search_index_count(&conn)?
        };
        on_progress(SearchIndexRebuildProgress {
            phase: "preparing",
            processed: 0,
            total: total_pending,
            failed: 0,
            elapsed_secs: started.elapsed().as_secs_f64(),
        });
        if total_pending == 0 {
            return Ok(SearchIndexRebuildOutcome {
                processed: 0,
                failed: 0,
                canceled: false,
            });
        }

        let mut writer = super::tantivy_index::bulk_writer(&self.storage_dir)?;
        let chunk_size = options.chunk_size.clamp(8, 512) as i64;
        let mut after_id = 0i64;
        let mut processed = 0i64;
        let mut failed = 0i64;
        let mut canceled = false;
        // Bookkeeping is withheld until the matching segment is committed, so
        // an interrupted run resumes from the last durable point.
        let mut uncommitted_state: Vec<IndexedState> = Vec::new();

        loop {
            if should_cancel() {
                canceled = true;
                break;
            }
            // Keyset paging over the id, not "give me the next stale rows":
            // a document that cannot be prepared stays stale forever, and
            // re-asking for stale rows would hand back the same one for ever.
            let ids = {
                let conn = self.read_conn()?;
                stale_search_index_ids_locked(&conn, chunk_size, after_id)?
            };
            if ids.is_empty() {
                break;
            }
            after_id = *ids.last().unwrap_or(&after_id);

            let prepared = self.prepare_search_index_chunk(&ids)?;
            failed += prepared.failed;

            for missing in &prepared.missing {
                let conn = self.conn.lock().map_err(|e| e.to_string())?;
                if let Err(error) = clear_search_index_locked(&conn, &self.storage_dir, *missing) {
                    log::warn!("Failed to clear missing search index row {missing}: {error}");
                }
            }

            for document in prepared.documents {
                writer.upsert(document)?;
            }

            if options.include_semantic && !prepared.semantic.is_empty() {
                // One embedding call per chunk rather than per document: the
                // model is only worth its accelerator when it is handed a batch.
                if let Err(error) =
                    super::semantic_index::upsert_documents(&self.storage_dir, &prepared.semantic)
                {
                    log::warn!("Semantic index batch skipped: {error}");
                }
            }

            uncommitted_state.extend(prepared.indexed);
            if writer.uncommitted() >= options.commit_every {
                writer.commit()?;
                self.record_indexed_documents(&uncommitted_state)?;
                uncommitted_state.clear();
            }

            processed += prepared.prepared_count;
            on_progress(SearchIndexRebuildProgress {
                phase: "indexing",
                processed,
                total: total_pending.max(processed),
                failed,
                elapsed_secs: started.elapsed().as_secs_f64(),
            });
        }

        if canceled {
            // Everything since the last commit is dropped, so the index stays at
            // the last state whose bookkeeping was also written.
            writer.rollback()?;
        } else {
            on_progress(SearchIndexRebuildProgress {
                phase: "committing",
                processed,
                total: total_pending.max(processed),
                failed,
                elapsed_secs: started.elapsed().as_secs_f64(),
            });
            writer.commit()?;
            self.record_indexed_documents(&uncommitted_state)?;
        }

        Ok(SearchIndexRebuildOutcome {
            processed,
            failed,
            canceled,
        })
    }

    /// Reads and analyses one chunk of documents across every available core.
    fn prepare_search_index_chunk(&self, ids: &[i64]) -> Result<PreparedChunk, String> {
        use rayon::prelude::*;

        let results = ids
            .par_iter()
            .map(|id| {
                let conn = self.read_conn()?;
                let Some(doc) = search_index_document_locked(&conn, &self.storage_dir, *id)? else {
                    return Ok(ChunkEntry::Missing(*id));
                };
                drop(conn);
                let prepared =
                    super::tantivy_index::prepare_document(&self.storage_dir, &doc.tantivy)?;
                Ok(ChunkEntry::Ready(Box::new(ReadyChunkEntry {
                    prepared,
                    state: IndexedState {
                        download_id: doc.download_id,
                        current_version: doc.current_version,
                        content_hash: doc.content_hash,
                    },
                    semantic: doc.semantic,
                })))
            })
            .collect::<Vec<Result<ChunkEntry, String>>>();

        let mut chunk = PreparedChunk::default();
        for result in results {
            match result {
                Ok(ChunkEntry::Missing(id)) => chunk.missing.push(id),
                Ok(ChunkEntry::Ready(ready)) => {
                    chunk.documents.push(ready.prepared);
                    chunk.indexed.push(ready.state);
                    chunk.semantic.push(ready.semantic);
                    chunk.prepared_count += 1;
                }
                Err(error) => {
                    // A single unreadable source file must not stop the rebuild.
                    log::warn!("Failed to prepare search index document: {error}");
                    chunk.failed += 1;
                    chunk.prepared_count += 1;
                }
            }
        }
        Ok(chunk)
    }

    /// Marks documents as indexed only after their content reached a committed
    /// segment, so an interrupted rebuild resumes instead of skipping them.
    fn record_indexed_documents(&self, states: &[IndexedState]) -> Result<(), String> {
        if states.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Search meta transaction failed: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO search_index_state (
                        download_id, current_version, content_hash, indexed_at
                     ) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| format!("Search meta prepare failed: {e}"))?;
            for state in states {
                stmt.execute(params![
                    state.download_id,
                    state.current_version,
                    state.content_hash,
                    now
                ])
                .map_err(|e| format!("Search meta insert failed: {e}"))?;
            }
        }
        tx.commit()
            .map_err(|e| format!("Search meta commit failed: {e}"))?;
        drop(conn);
        self.invalidate_index_status();
        Ok(())
    }

    pub fn search_filter_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FacetCount>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 200) as usize;
        let normalized_query = query.map(normalize_search_text).unwrap_or_default();
        let sql = |filtered: bool| -> Result<&'static str, String> {
            match (kind, filtered) {
                ("tags" | "tag", false) => Ok("SELECT t.name, COUNT(dt.download_id) AS count
                     FROM tags t
                     JOIN download_tags dt ON dt.tag_id = t.id
                     GROUP BY t.id, t.name
                     ORDER BY count DESC, t.name ASC
                     LIMIT ?1"),
                ("tags" | "tag", true) => Ok("SELECT t.name, COUNT(dt.download_id) AS count
                     FROM tags t
                     JOIN download_tags dt ON dt.tag_id = t.id
                     WHERE t.name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                     GROUP BY t.id, t.name
                     ORDER BY count DESC, t.name ASC
                     LIMIT ?2"),
                ("authors" | "author", false) => Ok("SELECT author_name, COUNT(*) AS count
                     FROM downloads
                     WHERE author_name IS NOT NULL AND author_name != ''
                     GROUP BY author_name
                     ORDER BY count DESC, author_name ASC
                     LIMIT ?1"),
                ("authors" | "author", true) => Ok("SELECT author_name, COUNT(*) AS count
                     FROM downloads
                     WHERE author_name IS NOT NULL AND author_name != ''
                       AND author_name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                     GROUP BY author_name
                     ORDER BY count DESC, author_name ASC
                     LIMIT ?2"),
                _ => Err(format!("Unsupported facet kind: {kind}")),
            }
        };
        let load = |statement: &str,
                    pattern: Option<&str>,
                    row_limit: usize|
         -> Result<Vec<FacetCount>, String> {
            let mut stmt = conn
                .prepare(statement)
                .map_err(|e| format!("Facet search prepare failed: {e}"))?;
            let map_row = |row: &rusqlite::Row<'_>| {
                Ok(FacetCount {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            };
            let mut facets = Vec::new();
            if let Some(pattern) = pattern {
                let rows = stmt
                    .query_map(params![pattern, row_limit as i64], map_row)
                    .map_err(|e| format!("Facet search failed: {e}"))?;
                for row in rows {
                    facets.push(row.map_err(|e| format!("Facet row read failed: {e}"))?);
                }
            } else {
                let rows = stmt
                    .query_map(params![row_limit as i64], map_row)
                    .map_err(|e| format!("Facet search failed: {e}"))?;
                for row in rows {
                    facets.push(row.map_err(|e| format!("Facet row read failed: {e}"))?);
                }
            }
            Ok(facets)
        };

        if normalized_query.is_empty() {
            return load(sql(false)?, None, limit);
        }

        // Exact/substring candidates are filtered before GROUP BY. A bounded
        // popularity fallback keeps kana/romaji/fuzzy matching useful without
        // allocating every distinct facet on each keystroke.
        let candidate_limit = limit.saturating_mul(20).clamp(400, 4_000);
        let raw_query = query.unwrap_or("").trim();
        let escaped = raw_query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let like = format!("%{escaped}%");
        let mut facets = load(sql(true)?, Some(&like), candidate_limit)?;
        if facets.len() < limit {
            let mut names = facets
                .iter()
                .map(|facet| facet.name.clone())
                .collect::<std::collections::HashSet<_>>();
            for facet in load(sql(false)?, None, candidate_limit)? {
                if names.insert(facet.name.clone()) {
                    facets.push(facet);
                }
            }
        }

        let query_grams = query_ngrams(&normalized_query);
        let mut scored = facets
            .into_iter()
            .filter_map(|facet| {
                let normalized_name = normalize_search_text(&facet.name);
                let mut score = if normalized_name == normalized_query {
                    1000.0
                } else if normalized_name.starts_with(&normalized_query) {
                    800.0
                } else if normalized_name.contains(&normalized_query) {
                    650.0
                } else {
                    let name_grams = generate_ngrams_limited(&normalized_name, 512);
                    let overlap = query_grams
                        .iter()
                        .filter(|gram| name_grams.contains(*gram))
                        .count();
                    let ratio = if query_grams.is_empty() {
                        0.0
                    } else {
                        overlap as f64 / query_grams.len() as f64
                    };
                    let fuzzy = normalized_levenshtein(&normalized_name, &normalized_query);
                    (ratio * 430.0).max(if fuzzy >= 0.72 { fuzzy * 360.0 } else { 0.0 })
                };

                if score <= 0.0 {
                    return None;
                }
                score += (facet.count as f64 + 1.0).ln() * 12.0;
                Some((score, facet))
            })
            .collect::<Vec<(f64, FacetCount)>>();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.1.count.cmp(&a.1.count))
                .then_with(|| a.1.name.cmp(&b.1.name))
        });
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, facet)| facet).collect())
    }

    pub fn search_suggest(
        &self,
        params: &SearchSuggestParams,
    ) -> Result<SearchSuggestResult, String> {
        let conn = self.read_conn()?;
        let limit = params.limit.unwrap_or(12).clamp(1, 50);
        let text = params.text.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return Ok(SearchSuggestResult { items: Vec::new() });
        }
        // Suggestions are deliberately prefix based. Four `%term%` grouped
        // scans on every keystroke become the dominant cost in a six-figure
        // library; infix/full-body matching remains available on Enter.
        let like = format!("{}%", text.replace('%', "\\%").replace('_', "\\_"));
        let mut items = Vec::new();

        collect_suggestions(
            &conn,
            "tag",
            "SELECT t.name, t.name, COUNT(dt.download_id), NULL, NULL
             FROM tags t
             JOIN download_tags dt ON dt.tag_id = t.id
             WHERE t.name LIKE ?1 ESCAPE '\\'
             GROUP BY t.id, t.name
             ORDER BY COUNT(dt.download_id) DESC, t.name ASC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;
        collect_suggestions(
            &conn,
            "author",
            "SELECT d.author_name, d.author_name, COUNT(*), d.source,
                    NULLIF(d.author_id, '')
             FROM downloads d
             WHERE d.author_name LIKE ?1 ESCAPE '\\'
             GROUP BY d.author_name, d.source, NULLIF(d.author_id, '')
             ORDER BY COUNT(*) DESC, d.author_name ASC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;
        collect_suggestions(
            &conn,
            "series",
            "SELECT s.title, s.source || ':' || s.source_key, COUNT(ds.download_id),
                    s.source, s.source_key
             FROM series s
             LEFT JOIN download_series ds ON ds.series_source = s.source AND ds.series_key = s.source_key
             WHERE s.title LIKE ?1 ESCAPE '\\'
             GROUP BY s.source, s.source_key, s.title
             ORDER BY COUNT(ds.download_id) DESC, s.title ASC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;
        collect_suggestions(
            &conn,
            "title",
            "SELECT d.title, d.source_id, 1, d.source, CAST(d.id AS TEXT)
             FROM downloads d
             WHERE d.title LIKE ?1 ESCAPE '\\'
             ORDER BY d.downloaded_at DESC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;

        let normalized_text = normalize_search_text(text);
        for item in &mut items {
            item.exact_match = normalize_search_text(&item.label) == normalized_text;
        }
        items.sort_by(|left, right| {
            right
                .exact_match
                .cmp(&left.exact_match)
                .then_with(|| {
                    suggestion_kind_priority(&left.kind).cmp(&suggestion_kind_priority(&right.kind))
                })
                .then_with(|| right.count.unwrap_or(0).cmp(&left.count.unwrap_or(0)))
                .then_with(|| left.label.cmp(&right.label))
        });
        items.dedup_by(|left, right| {
            left.kind == right.kind
                && left.source == right.source
                && left.source_key == right.source_key
                && left.value == right.value
        });
        items.truncate(limit as usize);
        Ok(SearchSuggestResult { items })
    }

    /// ダウンロードを挿入（UPSERT: 既に存在する場合は更新）
    pub fn upsert_download(&self, dl: &NewDownload) -> Result<i64, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;

        // 正規化インサートをアトミックに行うためのトランザクション開始
        // A savepoint works both standalone and inside the outer transaction
        // used by atomic backup restore.
        let tx = conn
            .savepoint()
            .map_err(|e| format!("Transaction begin failed: {}", e))?;

        tx.execute(
            "INSERT INTO downloads (
                source, source_id, title, author_name, author_id,
                content_type, excerpt, cover_path, json_path,
                original_json_path, asset_count, file_size_bytes,
                downloaded_at, source_created_at,
                content_hash, text_length, source_updated_at, watch_updates, current_version, favorite
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            ON CONFLICT(source, source_id) DO UPDATE SET
                title = excluded.title,
                author_name = excluded.author_name,
                excerpt = excluded.excerpt,
                cover_path = excluded.cover_path,
                json_path = excluded.json_path,
                original_json_path = excluded.original_json_path,
                asset_count = excluded.asset_count,
                file_size_bytes = excluded.file_size_bytes,
                downloaded_at = excluded.downloaded_at,
                content_hash = excluded.content_hash,
                text_length = excluded.text_length,
                source_updated_at = excluded.source_updated_at,
                watch_updates = excluded.watch_updates,
                current_version = excluded.current_version",
            params![
                dl.source,
                dl.source_id,
                dl.title,
                dl.author_name,
                dl.author_id,
                dl.content_type,
                dl.excerpt,
                dl.cover_path,
                dl.json_path,
                dl.original_json_path,
                dl.asset_count,
                dl.file_size_bytes,
                dl.downloaded_at,
                dl.source_created_at,
                dl.content_hash,
                dl.text_length,
                dl.source_updated_at,
                if dl.watch_updates { 1i64 } else { 0i64 },
                dl.current_version,
                if dl.favorite { 1i64 } else { 0i64 },
            ],
        )
        .map_err(|e| format!("Insert download failed: {}", e))?;

        // 外部キー参照整合性を担保するため、主キー id を確実にクエリ
        let id: i64 = tx
            .query_row(
                "SELECT id FROM downloads WHERE source = ?1 AND source_id = ?2",
                params![dl.source, dl.source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query upserted download ID: {}", e))?;

        // 更新時の既存タグ関係を一旦クローン削除
        tx.execute(
            "DELETE FROM download_tags WHERE download_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to clear old tags: {}", e))?;

        // タグは download_tags を唯一の真実として同期する
        for tag_name in normalized_tag_list(&dl.tags) {
            tx.execute(
                "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                params![tag_name],
            )
            .map_err(|e| format!("Failed to insert tag: {}", e))?;

            let tag_id: i64 = tx
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    params![tag_name],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to retrieve tag ID: {}", e))?;

            tx.execute(
                "INSERT OR IGNORE INTO download_tags (download_id, tag_id) VALUES (?1, ?2)",
                params![id, tag_id],
            )
            .map_err(|e| format!("Failed to insert download tag relation: {}", e))?;
        }

        tx.commit()
            .map_err(|e| format!("Transaction commit failed: {}", e))?;
        drop(conn);
        // A newly saved work is unindexed until something indexes it, so the
        // pending count the UI shows has just changed.
        self.invalidate_index_status();
        Ok(id)
    }

    pub fn upsert_update_target(&self, target: &UpdateTargetInput) -> Result<UpdateTarget, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_targets (
                target_type, source, source_key, display_name, enabled, metadata_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                display_name = excluded.display_name,
                enabled = excluded.enabled,
                metadata_json = excluded.metadata_json,
                updated_at = CURRENT_TIMESTAMP",
            params![
                target.target_type,
                target.source,
                target.source_key,
                target.display_name,
                if target.enabled { 1i64 } else { 0i64 },
                target.metadata_json,
            ],
        )
        .map_err(|e| format!("Failed to upsert update target: {}", e))?;
        drop(conn);
        self.get_update_target(&target.target_type, &target.source, &target.source_key)
    }

    pub fn get_update_target(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<UpdateTarget, String> {
        self.find_update_target(target_type, source, source_key)?
            .ok_or_else(|| "Update target not found".to_string())
    }

    /// Looks up one update target by its composite key without requiring the
    /// caller to fetch and deserialize the complete target list.
    pub fn find_update_target(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<Option<UpdateTarget>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM update_targets WHERE target_type = ?1 AND source = ?2 AND source_key = ?3",
            params![target_type, source, source_key],
            update_target_from_row,
        )
        .optional()
        .map_err(|e| format!("Failed to query update target: {e}"))
    }

    pub fn list_update_targets(
        &self,
        target_type: Option<&str>,
        enabled_only: bool,
    ) -> Result<Vec<UpdateTarget>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from("SELECT * FROM update_targets");
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut wheres = Vec::new();
        if let Some(t) = target_type {
            if !t.is_empty() && t != "all" {
                wheres.push("target_type = ?".to_string());
                bind_values.push(Box::new(t.to_string()));
            }
        }
        if enabled_only {
            wheres.push("enabled = 1".to_string());
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" ORDER BY target_type ASC, source ASC, display_name COLLATE NOCASE ASC");
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(refs.as_slice(), update_target_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn set_update_target_enabled(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_targets SET enabled = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?2 AND source = ?3 AND source_key = ?4",
            params![
                if enabled { 1i64 } else { 0i64 },
                target_type,
                source,
                source_key
            ],
        )
        .map_err(|e| format!("Failed to update target enabled state: {}", e))?;
        Ok(())
    }

    pub fn delete_update_target(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_targets WHERE target_type = ?1 AND source = ?2 AND source_key = ?3",
            params![target_type, source, source_key],
        )
        .map_err(|e| format!("Failed to delete update target: {}", e))?;
        Ok(())
    }

    pub fn mark_update_target_checked(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        last_seen_source_id: Option<&str>,
        last_seen_source_updated_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_targets SET
                last_checked_at = ?1,
                last_seen_source_id = COALESCE(?2, last_seen_source_id),
                last_seen_source_updated_at = COALESCE(?3, last_seen_source_updated_at),
                updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?4 AND source = ?5 AND source_key = ?6",
            params![
                chrono::Utc::now().to_rfc3339(),
                last_seen_source_id,
                last_seen_source_updated_at,
                target_type,
                source,
                source_key,
            ],
        )
        .map_err(|e| format!("Failed to mark update target checked: {}", e))?;
        Ok(())
    }

    pub fn upsert_download_relation(
        &self,
        download_id: i64,
        relation_type: &str,
        source: &str,
        relation_id: &str,
        relation_name: &str,
    ) -> Result<(), String> {
        if relation_id.trim().is_empty() || relation_name.trim().is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO download_relations (
                download_id, relation_type, source, relation_id, relation_name
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(download_id, relation_type, source, relation_id) DO UPDATE SET
                relation_name = excluded.relation_name",
            params![
                download_id,
                relation_type,
                source,
                relation_id,
                relation_name
            ],
        )
        .map_err(|e| format!("Failed to upsert download relation: {}", e))?;
        Ok(())
    }

    pub fn upsert_download_person(
        &self,
        download_id: i64,
        source: &str,
        person_key: &str,
        role: &str,
        display_name: &str,
    ) -> Result<(), String> {
        if person_key.trim().is_empty() || display_name.trim().is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO people (source, source_key, display_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![source, person_key, display_name],
        )
        .map_err(|e| format!("Failed to upsert person shell: {}", e))?;
        conn.execute(
            "INSERT INTO download_people (download_id, person_source, person_key, role, display_name)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(download_id, person_source, person_key, role) DO UPDATE SET
                display_name = excluded.display_name",
            params![download_id, source, person_key, role, display_name],
        )
        .map_err(|e| format!("Failed to upsert download person: {}", e))?;
        Ok(())
    }

    pub fn upsert_download_series(
        &self,
        download_id: i64,
        source: &str,
        series_key: &str,
        title: &str,
        content_order: Option<i64>,
    ) -> Result<(), String> {
        if series_key.trim().is_empty() || title.trim().is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO series (source, source_key, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![source, series_key, title],
        )
        .map_err(|e| format!("Failed to upsert series shell: {}", e))?;
        conn.execute(
            "INSERT INTO download_series (download_id, series_source, series_key, title, content_order)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(download_id, series_source, series_key) DO UPDATE SET
                title = excluded.title,
                content_order = excluded.content_order",
            params![download_id, source, series_key, title, content_order],
        )
        .map_err(|e| format!("Failed to upsert download series: {}", e))?;
        Ok(())
    }

    pub fn get_download_relations_for_download(
        &self,
        download_id: i64,
    ) -> Result<Vec<DownloadRelation>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT download_id, relation_type, source, relation_id, relation_name, NULL
                 FROM download_relations
                 WHERE download_id = ?1
                 ORDER BY relation_type, source, relation_id",
            )
            .map_err(|e| format!("Download relation query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadRelation {
                    download_id: row.get(0)?,
                    relation_type: row.get(1)?,
                    source: row.get(2)?,
                    relation_id: row.get(3)?,
                    relation_name: row.get(4)?,
                    work_count: row.get(5)?,
                })
            })
            .map_err(|e| format!("Download relation query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Download relation row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_download_people(&self, download_id: i64) -> Result<Vec<DownloadPerson>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT download_id, person_source, person_key, role, display_name
                 FROM download_people
                 WHERE download_id = ?1
                 ORDER BY person_source, person_key, role",
            )
            .map_err(|e| format!("Download people query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadPerson {
                    download_id: row.get(0)?,
                    person_source: row.get(1)?,
                    person_key: row.get(2)?,
                    role: row.get(3)?,
                    display_name: row.get(4)?,
                })
            })
            .map_err(|e| format!("Download people query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Download people row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_download_series_list(
        &self,
        download_id: i64,
    ) -> Result<Vec<DownloadSeries>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT download_id, series_source, series_key, title, content_order
                 FROM download_series
                 WHERE download_id = ?1
                 ORDER BY series_source, series_key",
            )
            .map_err(|e| format!("Download series query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadSeries {
                    download_id: row.get(0)?,
                    series_source: row.get(1)?,
                    series_key: row.get(2)?,
                    title: row.get(3)?,
                    content_order: row.get(4)?,
                })
            })
            .map_err(|e| format!("Download series query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Download series row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_person(&self, source: &str, source_key: &str) -> Result<PersonEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT p.*,
                (SELECT COUNT(DISTINCT download_id) FROM download_people dp
                 WHERE dp.person_source = p.source AND dp.person_key = p.source_key) AS work_count
             FROM people p WHERE p.source = ?1 AND p.source_key = ?2",
            params![source, source_key],
            person_entry_from_row,
        )
        .map_err(|e| format!("Person not found: {}", e))
    }

    pub fn list_people(&self) -> Result<Vec<PersonEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT p.*,
                    (SELECT COUNT(DISTINCT download_id) FROM download_people dp
                     WHERE dp.person_source = p.source AND dp.person_key = p.source_key) AS work_count
                 FROM people p
                 ORDER BY p.source ASC, p.source_key ASC",
            )
            .map_err(|e| format!("People query prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], person_entry_from_row)
            .map_err(|e| format!("People query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Person row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn restore_update_target(&self, target: &UpdateTarget) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_targets (
                target_type, source, source_key, display_name, enabled,
                last_checked_at, last_seen_source_id, last_seen_source_updated_at,
                metadata_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                display_name = excluded.display_name,
                enabled = excluded.enabled,
                last_checked_at = excluded.last_checked_at,
                last_seen_source_id = excluded.last_seen_source_id,
                last_seen_source_updated_at = excluded.last_seen_source_updated_at,
                metadata_json = excluded.metadata_json,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                target.target_type,
                target.source,
                target.source_key,
                target.display_name,
                if target.enabled { 1i64 } else { 0i64 },
                target.last_checked_at,
                target.last_seen_source_id,
                target.last_seen_source_updated_at,
                target.metadata_json,
                target.created_at,
                target.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore update target: {}", e))?;
        Ok(())
    }

    pub fn get_series(&self, source: &str, source_key: &str) -> Result<SeriesEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT s.*,
                (SELECT COUNT(DISTINCT download_id) FROM download_series ds
                 WHERE ds.series_source = s.source AND ds.series_key = s.source_key) AS work_count
             FROM series s WHERE s.source = ?1 AND s.source_key = ?2",
            params![source, source_key],
            series_entry_from_row,
        )
        .map_err(|e| format!("Series not found: {}", e))
    }

    pub fn list_series(&self) -> Result<Vec<SeriesEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT s.*,
                    (SELECT COUNT(DISTINCT download_id) FROM download_series ds
                     WHERE ds.series_source = s.source AND ds.series_key = s.source_key) AS work_count
                 FROM series s
                 ORDER BY s.source ASC, s.source_key ASC",
            )
            .map_err(|e| format!("Series query prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], series_entry_from_row)
            .map_err(|e| format!("Series query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Series row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn list_entity_versions(
        &self,
        entity_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<Vec<EntityVersion>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM entity_versions
                 WHERE entity_type = ?1 AND source = ?2 AND source_key = ?3
                 ORDER BY version DESC",
            )
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(
                params![entity_type, source, source_key],
                entity_version_from_row,
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn restore_person(&self, person: &PersonEntry) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO people (
                source, source_key, display_name, icon_path, cover_path, description, links_json,
                content_hash, current_version, last_checked_at, last_fetched_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(source, source_key) DO UPDATE SET
                display_name = excluded.display_name,
                icon_path = excluded.icon_path,
                cover_path = excluded.cover_path,
                description = excluded.description,
                links_json = excluded.links_json,
                content_hash = excluded.content_hash,
                current_version = excluded.current_version,
                last_checked_at = excluded.last_checked_at,
                last_fetched_at = excluded.last_fetched_at,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                person.source,
                person.source_key,
                person.display_name,
                person.icon_path,
                person.cover_path,
                person.description,
                person.links_json,
                person.content_hash,
                person.current_version,
                person.last_checked_at,
                person.last_fetched_at,
                person.created_at,
                person.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore person: {}", e))?;
        Ok(())
    }

    pub fn restore_series(&self, series: &SeriesEntry) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO series (
                source, source_key, title, description, cover_path, content_hash,
                current_version, last_checked_at, last_fetched_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(source, source_key) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                cover_path = excluded.cover_path,
                content_hash = excluded.content_hash,
                current_version = excluded.current_version,
                last_checked_at = excluded.last_checked_at,
                last_fetched_at = excluded.last_fetched_at,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                series.source,
                series.source_key,
                series.title,
                series.description,
                series.cover_path,
                series.content_hash,
                series.current_version,
                series.last_checked_at,
                series.last_fetched_at,
                series.created_at,
                series.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore series: {}", e))?;
        Ok(())
    }

    pub fn restore_entity_version(&self, version: &EntityVersion) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO entity_versions (
                entity_type, source, source_key, version, content_hash, json_path,
                asset_count, file_size_bytes, created_at, change_summary
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(entity_type, source, source_key, version) DO UPDATE SET
                content_hash = excluded.content_hash,
                json_path = excluded.json_path,
                asset_count = excluded.asset_count,
                file_size_bytes = excluded.file_size_bytes,
                created_at = excluded.created_at,
                change_summary = excluded.change_summary",
            params![
                version.entity_type,
                version.source,
                version.source_key,
                version.version,
                version.content_hash,
                version.json_path,
                version.asset_count,
                version.file_size_bytes,
                version.created_at,
                version.change_summary,
            ],
        )
        .map_err(|e| format!("Failed to restore entity version: {}", e))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_person_profile(
        &self,
        source: &str,
        source_key: &str,
        display_name: &str,
        icon_path: Option<&str>,
        cover_path: Option<&str>,
        description: Option<&str>,
        links_json: Option<&str>,
        content_hash: &str,
        json_path: &str,
        asset_count: i64,
        file_size_bytes: i64,
        freshness: EntityProfileFreshness,
    ) -> Result<PersonEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (current_hash, current_version): (Option<String>, i64) = conn
            .query_row(
                "SELECT content_hash, current_version FROM people WHERE source = ?1 AND source_key = ?2",
                params![source, source_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, 0));
        let now = chrono::Utc::now().to_rfc3339();
        let checked_at = freshness.checked_at(&now);
        if current_hash.as_deref() == Some(content_hash) {
            conn.execute(
                "UPDATE people SET display_name = ?1, icon_path = ?2, cover_path = ?3,
                    description = ?4, links_json = ?5,
                    last_checked_at = COALESCE(?6, last_checked_at),
                    updated_at = CURRENT_TIMESTAMP
                 WHERE source = ?7 AND source_key = ?8",
                params![
                    display_name,
                    icon_path,
                    cover_path,
                    description,
                    links_json,
                    checked_at,
                    source,
                    source_key
                ],
            )
            .map_err(|e| format!("Failed to mark person checked: {}", e))?;
        } else {
            let next_version = current_version + 1;
            conn.execute(
                "INSERT INTO people (
                    source, source_key, display_name, icon_path, cover_path, description, links_json,
                    content_hash, current_version, last_checked_at, last_fetched_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                 ON CONFLICT(source, source_key) DO UPDATE SET
                    display_name = excluded.display_name,
                    icon_path = excluded.icon_path,
                    cover_path = excluded.cover_path,
                    description = excluded.description,
                    links_json = excluded.links_json,
                    content_hash = excluded.content_hash,
                    current_version = excluded.current_version,
                    last_checked_at = COALESCE(excluded.last_checked_at, people.last_checked_at),
                    last_fetched_at = COALESCE(excluded.last_fetched_at, people.last_fetched_at),
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    source, source_key, display_name, icon_path, cover_path, description,
                    links_json, content_hash, next_version, checked_at
                ],
            )
            .map_err(|e| format!("Failed to upsert person: {}", e))?;
            conn.execute(
                "INSERT OR IGNORE INTO entity_versions (
                    entity_type, source, source_key, version, content_hash, json_path,
                    asset_count, file_size_bytes, created_at, change_summary
                 ) VALUES ('person', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    source,
                    source_key,
                    next_version,
                    content_hash,
                    json_path,
                    asset_count,
                    file_size_bytes,
                    now,
                    if current_version == 0 {
                        "初回プロフィール保存"
                    } else {
                        "プロフィール更新"
                    },
                ],
            )
            .map_err(|e| format!("Failed to insert person version: {}", e))?;
        }
        drop(conn);
        self.get_person(source, source_key)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_series_profile(
        &self,
        source: &str,
        source_key: &str,
        title: &str,
        description: Option<&str>,
        cover_path: Option<&str>,
        content_hash: &str,
        json_path: &str,
        asset_count: i64,
        file_size_bytes: i64,
        freshness: EntityProfileFreshness,
    ) -> Result<SeriesEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (current_hash, current_version): (Option<String>, i64) = conn
            .query_row(
                "SELECT content_hash, current_version FROM series WHERE source = ?1 AND source_key = ?2",
                params![source, source_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, 0));
        let now = chrono::Utc::now().to_rfc3339();
        let checked_at = freshness.checked_at(&now);
        if current_hash.as_deref() == Some(content_hash) {
            conn.execute(
                "UPDATE series SET title = ?1, description = ?2, cover_path = COALESCE(?3, cover_path),
                    last_checked_at = COALESCE(?4, last_checked_at),
                    updated_at = CURRENT_TIMESTAMP
                 WHERE source = ?5 AND source_key = ?6",
                params![
                    title,
                    description,
                    cover_path,
                    checked_at,
                    source,
                    source_key
                ],
            )
            .map_err(|e| format!("Failed to mark series checked: {}", e))?;
        } else {
            let next_version = current_version + 1;
            conn.execute(
                "INSERT INTO series (
                    source, source_key, title, description, cover_path, content_hash,
                    current_version, last_checked_at, last_fetched_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                 ON CONFLICT(source, source_key) DO UPDATE SET
                    title = excluded.title,
                    description = excluded.description,
                    cover_path = COALESCE(excluded.cover_path, series.cover_path),
                    content_hash = excluded.content_hash,
                    current_version = excluded.current_version,
                    last_checked_at = COALESCE(excluded.last_checked_at, series.last_checked_at),
                    last_fetched_at = COALESCE(excluded.last_fetched_at, series.last_fetched_at),
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    source,
                    source_key,
                    title,
                    description,
                    cover_path,
                    content_hash,
                    next_version,
                    checked_at
                ],
            )
            .map_err(|e| format!("Failed to upsert series: {}", e))?;
            conn.execute(
                "INSERT OR IGNORE INTO entity_versions (
                    entity_type, source, source_key, version, content_hash, json_path,
                    asset_count, file_size_bytes, created_at, change_summary
                 ) VALUES ('series', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    source,
                    source_key,
                    next_version,
                    content_hash,
                    json_path,
                    asset_count,
                    file_size_bytes,
                    now,
                    if current_version == 0 {
                        "初回シリーズ保存"
                    } else {
                        "シリーズ情報更新"
                    },
                ],
            )
            .map_err(|e| format!("Failed to insert series version: {}", e))?;
        }
        drop(conn);
        self.get_series(source, source_key)
    }

    pub fn list_download_relations(
        &self,
        relation_type: Option<&str>,
    ) -> Result<Vec<DownloadRelation>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from(
            "SELECT
                MIN(download_id) AS download_id,
                relation_type,
                source,
                relation_id,
                relation_name,
                COUNT(DISTINCT download_id) AS work_count
             FROM download_relations",
        );
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(t) = relation_type {
            if !t.is_empty() && t != "all" {
                sql.push_str(" WHERE relation_type = ?");
                bind_values.push(Box::new(t.to_string()));
            }
        }
        sql.push_str(" GROUP BY relation_type, source, relation_id, relation_name ORDER BY work_count DESC, relation_name COLLATE NOCASE ASC");
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok(DownloadRelation {
                    download_id: row.get(0)?,
                    relation_type: row.get(1)?,
                    source: row.get(2)?,
                    relation_id: row.get(3)?,
                    relation_name: row.get(4)?,
                    work_count: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        if relation_type
            .map(|t| t == "author" || t == "all")
            .unwrap_or(true)
        {
            let mut existing = std::collections::HashSet::new();
            for rel in &results {
                existing.insert(format!(
                    "{}:{}:{}",
                    rel.relation_type, rel.source, rel.relation_id
                ));
            }
            let mut stmt = conn
                .prepare(
                    "SELECT
                        MIN(id) AS download_id,
                        'author' AS relation_type,
                        source,
                        author_id AS relation_id,
                        author_name AS relation_name,
                        COUNT(*) AS work_count
                     FROM downloads
                     WHERE author_id IS NOT NULL AND author_id != ''
                     GROUP BY source, author_id, author_name
                     ORDER BY work_count DESC, relation_name COLLATE NOCASE ASC",
                )
                .map_err(|e| format!("Author relation query prepare failed: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(DownloadRelation {
                        download_id: row.get(0)?,
                        relation_type: row.get(1)?,
                        source: row.get(2)?,
                        relation_id: row.get(3)?,
                        relation_name: row.get(4)?,
                        work_count: row.get(5)?,
                    })
                })
                .map_err(|e| format!("Author relation query failed: {}", e))?;
            for row in rows {
                let rel = row.map_err(|e| format!("Author relation row read failed: {}", e))?;
                let key = format!("{}:{}:{}", rel.relation_type, rel.source, rel.relation_id);
                if existing.insert(key) {
                    results.push(rel);
                }
            }
        }
        Ok(results)
    }

    /// アセットを挿入
    pub fn insert_asset(&self, asset: &NewAsset) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO assets (
                download_id, asset_type, filename, local_path,
                original_url, mime_type, file_size_bytes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                asset.download_id,
                asset.asset_type,
                asset.filename,
                asset.local_path,
                asset.original_url,
                asset.mime_type,
                asset.file_size_bytes,
            ],
        )
        .map_err(|e| format!("Insert asset failed: {}", e))?;
        Ok(conn.last_insert_rowid())
    }

    /// Cursor-based library/search entrypoint backed by Tantivy for full text
    /// and SQLite for metadata filters.
    pub fn search_downloads_v2(&self, params: &SearchV2Params) -> Result<SearchV2Result, String> {
        self.search_downloads_v2_inner(params, 1_000)
    }

    pub fn search_downloads_v2_internal(
        &self,
        params: &SearchV2Params,
        max_limit: i64,
    ) -> Result<SearchV2Result, String> {
        self.search_downloads_v2_inner(params, max_limit)
    }

    fn search_downloads_v2_inner(
        &self,
        params: &SearchV2Params,
        max_limit: i64,
    ) -> Result<SearchV2Result, String> {
        let submitted_query = query_text(params).to_string();
        let mut effective_params = normalize_search_params(params);
        let limit = effective_params.limit.unwrap_or(80).clamp(1, max_limit);
        effective_params.limit = Some(limit);
        let exact_entity =
            self.apply_exact_entity_intent(&submitted_query, &mut effective_params)?;
        let query = query_text(&effective_params).to_string();
        let status = self.get_search_index_status()?;
        let semantic_ready = status.semantic_model_ready;
        let semantic_complete = status.semantic_indexed_chunks > 0 || status.total_downloads == 0;

        if query.is_empty() {
            let cursor = decode_cursor(effective_params.cursor.as_deref()).filter(|cursor| {
                cursor.kind == "sql"
                    && cursor.sort_by == effective_sort_by(&effective_params)
                    && cursor.sort_order == effective_sort_order(&effective_params)
            });
            let mut items = self.search_sql_page(&effective_params, limit + 1, cursor.as_ref())?;
            let has_more = items.len() as i64 > limit;
            if has_more {
                items.truncate(limit as usize);
            }
            let total_estimate = self.count_sql_matches(&effective_params).ok();
            let next_cursor = if has_more {
                items
                    .last()
                    .and_then(|item| encode_sql_cursor(&effective_params, item))
            } else {
                None
            };
            return Ok(SearchV2Result {
                items,
                next_cursor,
                total_estimate,
                search_meta: SearchMeta {
                    engine: if exact_entity.is_some() {
                        "sqlite-exact-entity".to_string()
                    } else {
                        "sqlite-metadata".to_string()
                    },
                    query: (!submitted_query.is_empty()).then_some(submitted_query.clone()),
                    total_estimate,
                    index_complete: status.is_complete,
                    explanations: search_explanations(
                        exact_entity.as_ref(),
                        "sqlite-metadata",
                        status.is_complete,
                    ),
                    exact_entity,
                    semantic_index_complete: Some(semantic_complete),
                    semantic_model_ready: Some(semantic_ready),
                },
                facets_version: status.indexed_downloads,
            });
        }

        let search_mode = effective_search_mode(&effective_params);
        let mut search_limit = search_candidate_limit(&effective_params, limit);
        let parsed_query = parse_search_query(&query);
        // An explicit column sort asked for while searching cannot be answered
        // by relevance paging, so the match set moves to SQL, which owns the
        // ordering and the keyset cursor.
        if search_mode != "semantic" && wants_column_sort(&effective_params) {
            return self.search_sorted_lexical_page(
                &effective_params,
                &query,
                &submitted_query,
                limit,
                &status,
                exact_entity,
                &parsed_query,
            );
        }
        if search_mode != "semantic" {
            return self.search_lexical_page(
                &effective_params,
                &query,
                &submitted_query,
                limit,
                &status,
                exact_entity,
                &parsed_query,
                &search_mode,
            );
        }
        let cursor_scope = search_cursor_scope(&effective_params);
        let cursor = decode_cursor(effective_params.cursor.as_deref()).filter(|candidate| {
            candidate.kind == "search" && candidate.scope.as_deref() == Some(&cursor_scope)
        });
        let maximum_semantic_candidates =
            usize::try_from(status.semantic_indexed_chunks.max(1)).unwrap_or(usize::MAX);
        let mut lexical_total = None;
        let (mut ranked_items, semantic_map, document_map, candidates_exhausted, unpaged_count) = loop {
            let lexical_result = if search_mode != "semantic" {
                super::tantivy_index::search_with_total(&self.storage_dir, &query, search_limit)?
            } else {
                super::tantivy_index::TantivySearchResult::default()
            };
            if search_mode != "semantic" {
                lexical_total = Some(lexical_result.total_hits);
            }
            let semantic_hits = if search_mode == "semantic" {
                match super::semantic_index::search(&self.storage_dir, &query, search_limit) {
                    Ok(hits) => hits,
                    Err(error) => {
                        log::warn!("Semantic search unavailable: {}", error);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let hits = blend_search_hits(&lexical_result.hits, &semantic_hits, &search_mode);
            let semantic_map = hits
                .iter()
                .filter_map(|hit| {
                    hit.semantic
                        .clone()
                        .map(|semantic| (hit.download_id, semantic))
                })
                .collect::<HashMap<_, _>>();
            let document_map = hits
                .iter()
                .filter_map(|hit| {
                    hit.document
                        .clone()
                        .map(|document| (hit.download_id, document))
                })
                .collect::<HashMap<_, _>>();
            let mut candidates = self.fetch_ranked_sql_matches(&effective_params, &hits)?;
            if !parsed_query.exclude.is_empty() {
                candidates = filter_excluded_search_results(
                    &self.storage_dir,
                    candidates,
                    &parsed_query,
                    &document_map,
                );
            }
            candidates.sort_by(|a, b| {
                b.search_score
                    .unwrap_or(0.0)
                    .partial_cmp(&a.search_score.unwrap_or(0.0))
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| b.downloaded_at.cmp(&a.downloaded_at))
                    .then_with(|| b.id.cmp(&a.id))
            });
            let unpaged_count = candidates.len();
            if let Some(cursor) = cursor.as_ref() {
                candidates.retain(|item| search_item_is_after_cursor(item, cursor));
            }

            let exhausted = if search_mode == "semantic" {
                semantic_hits.len() < search_limit || search_limit >= maximum_semantic_candidates
            } else {
                search_limit >= lexical_result.total_hits
            };
            if candidates.len() as i64 > limit || exhausted {
                break (
                    candidates,
                    semantic_map,
                    document_map,
                    exhausted,
                    unpaged_count,
                );
            }

            // Grow with cursor depth and selective metadata filters. Unlike the
            // previous hard 1,000 cap, this eventually reaches every lexical
            // match while keeping the common first page small and fast.
            let maximum = if search_mode == "semantic" {
                maximum_semantic_candidates
            } else {
                lexical_result.total_hits
            };
            let next_limit = search_limit.saturating_mul(2).min(maximum);
            if next_limit <= search_limit {
                break (candidates, semantic_map, document_map, true, unpaged_count);
            }
            search_limit = next_limit;
        };
        let total_estimate =
            if !params_have_library_filters(&effective_params) && search_mode != "semantic" {
                lexical_total.and_then(|count| i64::try_from(count).ok())
            } else if candidates_exhausted {
                i64::try_from(unpaged_count).ok()
            } else {
                // A filtered count cannot be known until all lexical candidates
                // have been intersected with SQLite. `None` is more honest than a
                // fixed top-N value that looks complete to the UI.
                None
            };
        let has_more = ranked_items.len() as i64 > limit;
        let page_items = ranked_items
            .drain(..)
            .take(limit as usize)
            .collect::<Vec<DownloadEntry>>();
        let next_cursor = if has_more {
            page_items
                .last()
                .and_then(|item| encode_search_cursor(&effective_params, item))
        } else {
            None
        };
        let items = decorate_search_results(
            &self.storage_dir,
            page_items,
            &parsed_query,
            &semantic_map,
            &document_map,
        );

        Ok(SearchV2Result {
            items,
            next_cursor,
            total_estimate,
            search_meta: SearchMeta {
                engine: if search_mode == "exact" {
                    "tantivy-exact".to_string()
                } else if search_mode == "semantic" {
                    "semantic-local".to_string()
                } else {
                    "hybrid-local".to_string()
                },
                query: (!submitted_query.is_empty()).then_some(submitted_query),
                total_estimate,
                index_complete: status.is_complete,
                explanations: search_explanations(
                    exact_entity.as_ref(),
                    if search_mode == "semantic" {
                        "semantic"
                    } else {
                        "tantivy"
                    },
                    status.is_complete,
                ),
                exact_entity,
                semantic_index_complete: Some(semantic_complete),
                semantic_model_ready: Some(semantic_ready),
            },
            facets_version: status.indexed_downloads,
        })
    }

    /// Plain, exact entity names are navigation-like intent, not fuzzy free
    /// text. Restricting them at the relational layer prevents a body mention
    /// from another creator from leaking into an otherwise unambiguous author
    /// or series search. Ambiguous names deliberately remain a normal search.
    fn apply_exact_entity_intent(
        &self,
        submitted_query: &str,
        params: &mut SearchV2Params,
    ) -> Result<Option<SearchEntityIntent>, String> {
        let query = submitted_query.trim();
        if !plain_entity_query(query)
            || effective_search_mode(params) == "semantic"
            || params.authors_include.is_some()
            || params.person_key.is_some()
            || params.series_key.is_some()
        {
            return Ok(None);
        }

        let conn = self.read_conn()?;
        let authors = {
            let mut stmt = conn
                .prepare(
                    "SELECT d.author_name, d.source, d.author_id, COUNT(*)
                     FROM downloads d
                     WHERE d.author_name = ?1 COLLATE NOCASE
                     GROUP BY d.author_name, d.source, d.author_id
                     ORDER BY COUNT(*) DESC, d.source, d.author_id
                     LIMIT 8",
                )
                .map_err(|e| format!("Exact author intent prepare failed: {e}"))?;
            let rows = stmt
                .query_map(params![query], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| format!("Exact author intent query failed: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Exact author intent row failed: {e}"))?
        };
        let series = {
            let mut stmt = conn
                .prepare(
                    "SELECT ds.title, ds.series_source, ds.series_key, COUNT(*)
                     FROM download_series ds
                     WHERE ds.title = ?1 COLLATE NOCASE
                     GROUP BY ds.title, ds.series_source, ds.series_key
                     ORDER BY COUNT(*) DESC, ds.series_source, ds.series_key
                     LIMIT 8",
                )
                .map_err(|e| format!("Exact series intent prepare failed: {e}"))?;
            let rows = stmt
                .query_map(params![query], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| format!("Exact series intent query failed: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Exact series intent row failed: {e}"))?
        };

        match (authors.is_empty(), series.is_empty()) {
            (false, true) => {
                let label = authors[0].0.clone();
                params.authors_include = Some(vec![label.clone()]);
                params.query = None;
                params.text = None;
                let unique_identity = (authors.len() == 1).then(|| &authors[0]);
                Ok(Some(SearchEntityIntent {
                    kind: "author".to_string(),
                    label,
                    source: unique_identity.map(|author| author.1.clone()),
                    source_key: unique_identity.map(|author| author.2.clone()),
                    strict: true,
                }))
            }
            (true, false) => {
                let label = series[0].0.clone();
                if series.len() == 1 {
                    params.series_source = Some(series[0].1.clone());
                    params.series_key = Some(series[0].2.clone());
                } else {
                    // The title is still unambiguous as a concept; include all
                    // providers carrying the exact same series title.
                    params.series_source = None;
                    params.series_key = Some(label.clone());
                }
                params.query = None;
                params.text = None;
                Ok(Some(SearchEntityIntent {
                    kind: "series".to_string(),
                    label,
                    source: (series.len() == 1).then(|| series[0].1.clone()),
                    source_key: (series.len() == 1).then(|| series[0].2.clone()),
                    strict: true,
                }))
            }
            _ => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn search_lexical_page(
        &self,
        params: &SearchV2Params,
        query: &str,
        submitted_query: &str,
        limit: i64,
        status: &SearchIndexStatus,
        exact_entity: Option<SearchEntityIntent>,
        parsed_query: &ParsedSearchQuery,
        search_mode: &str,
    ) -> Result<SearchV2Result, String> {
        let cursor_scope = search_cursor_scope(params);
        let decoded_cursor = decode_cursor(params.cursor.as_deref()).filter(|candidate| {
            candidate.kind == "tantivy-search-after"
                && candidate.scope.as_deref() == Some(&cursor_scope)
        });
        let mut after = decoded_cursor.as_ref().and_then(tantivy_cursor);
        let started_at_beginning = after.is_none();
        let batch_size =
            search_candidate_limit(params, limit).max(limit.saturating_add(1) as usize);
        let mut ranked_items = Vec::with_capacity(limit.saturating_add(1) as usize);
        let mut document_map: HashMap<i64, Arc<SearchDocument>> = HashMap::new();
        let mut item_cursors = HashMap::new();
        let semantic_map = HashMap::new();
        let mut total_hits: usize;
        let exhausted;

        loop {
            let lexical = super::tantivy_index::search_after_with_total(
                &self.storage_dir,
                query,
                batch_size,
                after,
            )?;
            total_hits = lexical.total_hits;
            let fetched = lexical.hits.len();
            if fetched == 0 {
                exhausted = true;
                break;
            }

            let mut cursor_for_id = HashMap::with_capacity(fetched);
            let hits = lexical
                .hits
                .iter()
                .map(|hit| {
                    let cursor = super::tantivy_index::TantivySearchCursor {
                        score: hit.score,
                        segment_ord: hit.segment_ord,
                        doc_id: hit.doc_id,
                    };
                    cursor_for_id.insert(hit.download_id, cursor);
                    document_map.insert(hit.download_id, hit.document.clone());
                    RankedSearchHit {
                        download_id: hit.download_id,
                        score: hit.score as f64,
                        semantic: None,
                        document: Some(hit.document.clone()),
                    }
                })
                .collect::<Vec<_>>();
            let last_lexical_cursor =
                lexical
                    .hits
                    .last()
                    .map(|hit| super::tantivy_index::TantivySearchCursor {
                        score: hit.score,
                        segment_ord: hit.segment_ord,
                        doc_id: hit.doc_id,
                    });
            let mut candidates = self.fetch_ranked_sql_matches(params, &hits)?;
            if !parsed_query.exclude.is_empty() {
                candidates = filter_excluded_search_results(
                    &self.storage_dir,
                    candidates,
                    parsed_query,
                    &document_map,
                );
            }
            for item in candidates {
                if let Some(cursor) = cursor_for_id.get(&item.id).copied() {
                    item_cursors.insert(item.id, cursor);
                }
                ranked_items.push(item);
            }

            let page_has_lookahead = ranked_items.len() as i64 > limit;
            let source_exhausted = fetched < batch_size;
            if page_has_lookahead || source_exhausted {
                exhausted = source_exhausted;
                break;
            }
            after = last_lexical_cursor;
        }

        let total_estimate = if !params_have_library_filters(params) {
            i64::try_from(total_hits).ok()
        } else if started_at_beginning && exhausted {
            i64::try_from(ranked_items.len()).ok()
        } else {
            None
        };
        let has_more = ranked_items.len() as i64 > limit || !exhausted;
        let page_items = ranked_items
            .drain(..)
            .take(limit as usize)
            .collect::<Vec<_>>();
        let next_cursor = if has_more {
            page_items.last().and_then(|item| {
                item_cursors
                    .get(&item.id)
                    .copied()
                    .and_then(|cursor| encode_tantivy_search_cursor(params, item, cursor))
            })
        } else {
            None
        };
        let items = decorate_search_results(
            &self.storage_dir,
            page_items,
            parsed_query,
            &semantic_map,
            &document_map,
        );
        let semantic_complete = status.semantic_indexed_chunks > 0 || status.total_downloads == 0;

        Ok(SearchV2Result {
            items,
            next_cursor,
            total_estimate,
            search_meta: SearchMeta {
                engine: if search_mode == "exact" {
                    "tantivy-exact".to_string()
                } else {
                    "hybrid-local".to_string()
                },
                query: (!submitted_query.is_empty()).then(|| submitted_query.to_string()),
                total_estimate,
                index_complete: status.is_complete,
                explanations: search_explanations(
                    exact_entity.as_ref(),
                    "tantivy",
                    status.is_complete,
                ),
                exact_entity,
                semantic_index_complete: Some(semantic_complete),
                semantic_model_ready: Some(status.semantic_model_ready),
            },
            facets_version: status.indexed_downloads,
        })
    }

    /// A text search ordered by a library column rather than by relevance.
    ///
    /// Tantivy answers "which works match"; SQL answers "in what order", using
    /// the same sort clause and keyset cursor as an unsearched library listing,
    /// so paging behaves identically with and without a query.
    #[allow(clippy::too_many_arguments)]
    fn search_sorted_lexical_page(
        &self,
        params: &SearchV2Params,
        query: &str,
        submitted_query: &str,
        limit: i64,
        status: &SearchIndexStatus,
        exact_entity: Option<SearchEntityIntent>,
        parsed_query: &ParsedSearchQuery,
    ) -> Result<SearchV2Result, String> {
        let matched_ids = super::tantivy_index::matching_download_ids(&self.storage_dir, query)?;
        let semantic_complete = status.semantic_indexed_chunks > 0 || status.total_downloads == 0;
        let mut explanations =
            search_explanations(exact_entity.as_ref(), "tantivy", status.is_complete);
        explanations.push(format!(
            "{}で並び替えています（関連度順ではありません）",
            sort_label(params)
        ));

        if matched_ids.is_empty() {
            return Ok(SearchV2Result {
                items: Vec::new(),
                next_cursor: None,
                total_estimate: Some(0),
                search_meta: SearchMeta {
                    engine: "tantivy-sorted".to_string(),
                    query: (!submitted_query.is_empty()).then(|| submitted_query.to_string()),
                    total_estimate: Some(0),
                    index_complete: status.is_complete,
                    explanations,
                    exact_entity,
                    semantic_index_complete: Some(semantic_complete),
                    semantic_model_ready: Some(status.semantic_model_ready),
                },
                facets_version: status.indexed_downloads,
            });
        }

        let cursor_scope = search_cursor_scope(params);
        let cursor = decode_cursor(params.cursor.as_deref()).filter(|candidate| {
            candidate.kind == "sorted-search"
                && candidate.scope.as_deref() == Some(&cursor_scope)
                && candidate.sort_by == effective_sort_by(params)
                && candidate.sort_order == effective_sort_order(params)
        });

        let conn = self.read_conn()?;
        // The id set travels as one JSON parameter: a placeholder per id would
        // blow past SQLite's variable limit on a broad query.
        let id_json = serde_json::to_string(&matched_ids)
            .map_err(|e| format!("Search id encoding failed: {e}"))?;
        let mut sql = download_select_sql_for_projection(
            params.projection.as_deref(),
            "NULL",
            &sort_key_select_expr(params),
        );
        let mut wheres = vec!["d.id IN (SELECT value FROM json_each(?))".to_string()];
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(id_json.clone())];
        append_library_filters(params, &mut wheres, &mut bind_values);
        append_keyset_filter(params, cursor.as_ref(), &mut wheres, &mut bind_values);
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
        sql.push_str(&sort_clause(params));
        sql.push_str(" LIMIT ?");
        bind_values.push(Box::new(limit + 1));
        if let Some(offset) = page_offset(params) {
            sql.push_str(" OFFSET ?");
            bind_values.push(Box::new(offset));
        }
        let mut items = query_download_entries(&conn, &sql, &bind_values)?;
        drop(conn);

        if !parsed_query.exclude.is_empty() {
            items = filter_excluded_search_results(
                &self.storage_dir,
                items,
                parsed_query,
                &HashMap::new(),
            );
        }
        let has_more = items.len() as i64 > limit;
        items.truncate(limit as usize);

        let total_estimate = self.count_sorted_search_matches(&id_json, params).ok();
        let next_cursor = if has_more {
            items.last().and_then(|item| {
                encode_cursor(&SearchCursor {
                    kind: "sorted-search".to_string(),
                    scope: Some(cursor_scope.clone()),
                    sort_by: effective_sort_by(params),
                    sort_order: effective_sort_order(params),
                    value: item
                        .sort_key
                        .clone()
                        .or_else(|| fallback_sort_value(params, item)),
                    id: Some(item.id),
                    score: None,
                    downloaded_at: Some(item.downloaded_at.clone()),
                    tantivy_score: None,
                    tantivy_segment_ord: None,
                    tantivy_doc_id: None,
                })
            })
        } else {
            None
        };

        let items = decorate_search_results(
            &self.storage_dir,
            items,
            parsed_query,
            &HashMap::new(),
            &HashMap::new(),
        );

        Ok(SearchV2Result {
            items,
            next_cursor,
            total_estimate,
            search_meta: SearchMeta {
                engine: "tantivy-sorted".to_string(),
                query: (!submitted_query.is_empty()).then(|| submitted_query.to_string()),
                total_estimate,
                index_complete: status.is_complete,
                explanations,
                exact_entity,
                semantic_index_complete: Some(semantic_complete),
                semantic_model_ready: Some(status.semantic_model_ready),
            },
            facets_version: status.indexed_downloads,
        })
    }

    fn count_sorted_search_matches(
        &self,
        id_json: &str,
        params: &SearchV2Params,
    ) -> Result<i64, String> {
        let conn = self.read_conn()?;
        let mut sql =
            "SELECT COUNT(*) FROM downloads d WHERE d.id IN (SELECT value FROM json_each(?))"
                .to_string();
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(id_json.to_string())];
        append_library_filters(params, &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" AND ");
            sql.push_str(&wheres.join(" AND "));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|value| value.as_ref()).collect();
        conn.query_row(&sql, refs.as_slice(), |row| row.get(0))
            .map_err(|e| format!("Sorted search count failed: {e}"))
    }

    fn search_sql_page(
        &self,
        params: &SearchV2Params,
        limit: i64,
        cursor: Option<&SearchCursor>,
    ) -> Result<Vec<DownloadEntry>, String> {
        let conn = self.read_conn()?;
        let mut sql = download_select_sql_for_projection(
            params.projection.as_deref(),
            "NULL",
            &sort_key_select_expr(params),
        );
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_library_filters(params, &mut wheres, &mut bind_values);
        append_keyset_filter(params, cursor, &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(&sort_clause(params));
        sql.push_str(" LIMIT ?");
        bind_values.push(Box::new(limit));
        // A page number skips ahead instead of walking there with a cursor.
        // Only valid alongside a stable ordering, which is why nothing offsets
        // a relevance search.
        if let Some(offset) = page_offset(params) {
            sql.push_str(" OFFSET ?");
            bind_values.push(Box::new(offset));
        }
        query_download_entries(&conn, &sql, &bind_values)
    }

    fn count_sql_matches(&self, params: &SearchV2Params) -> Result<i64, String> {
        let conn = self.read_conn()?;
        let mut sql = "SELECT COUNT(*) FROM downloads d".to_string();
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_library_filters(params, &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        conn.query_row(&sql, refs.as_slice(), |row| row.get(0))
            .map_err(|e| format!("Count query failed: {}", e))
    }

    fn fetch_ranked_sql_matches(
        &self,
        params: &SearchV2Params,
        hits: &[RankedSearchHit],
    ) -> Result<Vec<DownloadEntry>, String> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        let rank_map = hits
            .iter()
            .enumerate()
            .map(|(idx, hit)| (hit.download_id, (idx, hit.score)))
            .collect::<HashMap<i64, (usize, f64)>>();
        let conn = self.read_conn()?;
        let mut items = Vec::new();

        for chunk in hits.chunks(500) {
            let mut sql =
                download_select_sql_for_projection(params.projection.as_deref(), "NULL", "NULL");
            let mut wheres = Vec::new();
            let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            append_library_filters(params, &mut wheres, &mut bind_values);
            let placeholders = vec!["?"; chunk.len()].join(", ");
            wheres.push(format!("d.id IN ({})", placeholders));
            for hit in chunk {
                bind_values.push(Box::new(hit.download_id));
            }
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
            let mut chunk_items = query_download_entries(&conn, &sql, &bind_values)?;
            for item in &mut chunk_items {
                if let Some((_, score)) = rank_map.get(&item.id) {
                    item.search_score = Some(*score);
                }
            }
            items.extend(chunk_items);
        }

        items.sort_by_key(|item| {
            rank_map
                .get(&item.id)
                .map(|(idx, _)| *idx)
                .unwrap_or(usize::MAX)
        });
        Ok(items)
    }

    /// 単一ダウンロードの取得
    pub fn get_download(&self, id: i64) -> Result<DownloadEntry, String> {
        let conn = self.read_conn()?;
        let sql = format!(
            "{} WHERE d.id = ?1",
            download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL")
        );
        conn.query_row(&sql, params![id], download_entry_from_row)
            .map_err(|e| format!("Download not found: {}", e))
    }

    /// 複数の作品をまとめて取得する。
    ///
    /// EPUBキューのように多数のIDを扱う画面で1件ずつ問い合わせると、
    /// 件数分のIPCとクエリが発生する。存在しないIDは結果から除かれるので、
    /// 呼び出し側は削除済みの項目を検出できる。
    pub fn get_downloads(&self, ids: &[i64]) -> Result<Vec<DownloadEntry>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let unique: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            ids.iter().copied().filter(|id| seen.insert(*id)).collect()
        };
        let conn = self.read_conn()?;
        let mut entries = Vec::with_capacity(unique.len());
        // SQLite caps host parameters, so long queues are read in chunks.
        for chunk in unique.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "{} WHERE d.id IN ({})",
                download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL"),
                placeholders
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    download_entry_from_row(row)
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                entries.push(row.map_err(|e| e.to_string())?);
            }
        }
        // Preserve the caller's ordering, which is the queue order on screen.
        let position: HashMap<i64, usize> = unique
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
        entries.sort_by_key(|entry| position.get(&entry.id).copied().unwrap_or(usize::MAX));
        Ok(entries)
    }

    /// 特定のソースIDのダウンロードが存在するか確認する
    pub fn check_exists(&self, source: &str, source_id: &str) -> Result<bool, String> {
        let conn = self.read_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(count > 0)
    }

    /// ダウンロードのアセット一覧取得
    pub fn get_assets(&self, download_id: i64) -> Result<Vec<AssetEntry>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM assets WHERE download_id = ?1")
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(AssetEntry {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    asset_type: row.get(2)?,
                    filename: row.get(3)?,
                    local_path: row.get(4)?,
                    original_url: row.get(5)?,
                    mime_type: row.get(6)?,
                    file_size_bytes: row.get(7)?,
                })
            })
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_reader_metadata(&self, download_id: i64) -> Result<ReaderMetadata, String> {
        let download = self.get_download(download_id)?;
        let versions = self.get_versions(download_id)?;
        let conn = self.read_conn()?;
        let active_edit_revision = active_edit_revision_locked(&conn, download_id)?;
        Ok(ReaderMetadata {
            asset_count: download.asset_count,
            download,
            versions,
            is_edited: active_edit_revision.is_some(),
            active_edit_revision,
        })
    }

    pub fn get_reader_content_page(
        &self,
        download_id: i64,
        version: Option<i64>,
        page: usize,
    ) -> Result<ReaderContentPage, String> {
        let content = self.get_cached_reader_content(download_id, version)?;
        let page_count = content.pages.len().max(1);
        let page = page.min(page_count.saturating_sub(1));
        let html = content.pages.get(page).cloned().unwrap_or_default();
        Ok(ReaderContentPage {
            page,
            page_count,
            plain_text: plain_text_from_reader_html(&html),
            html,
            total_plain_text_chars: content.total_plain_text_chars,
        })
    }

    pub fn search_reader_content(
        &self,
        download_id: i64,
        version: Option<i64>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ReaderSearchHit>, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let content = self.get_cached_reader_content(download_id, version)?;
        let mut hits = Vec::new();
        for (page, html) in content.pages.iter().enumerate() {
            let plain = plain_text_from_reader_html(html);
            let normalized = plain.to_lowercase();
            let count = normalized.match_indices(&query).count();
            if count == 0 {
                continue;
            }
            let byte_index = normalized.find(&query).unwrap_or(0);
            let char_index = normalized[..byte_index].chars().count();
            let chars = plain.chars().collect::<Vec<_>>();
            let start = char_index.saturating_sub(42);
            let end = (char_index + query.chars().count() + 70).min(chars.len());
            let snippet = format!(
                "{}{}{}",
                if start > 0 { "…" } else { "" },
                chars[start..end].iter().collect::<String>(),
                if end < chars.len() { "…" } else { "" }
            );
            hits.push(ReaderSearchHit {
                page: page + 1,
                snippet,
                count,
            });
            if hits.len() >= limit.clamp(1, 200) {
                break;
            }
        }
        Ok(hits)
    }

    fn get_cached_reader_content(
        &self,
        download_id: i64,
        version: Option<i64>,
    ) -> Result<ReaderCacheEntry, String> {
        let download = self.get_download(download_id)?;
        let versions = self.get_versions(download_id)?;
        let conn = self.read_conn()?;
        let active_edit = if version.is_none() {
            active_edit_revision_locked(&conn, download_id)?
        } else {
            None
        };
        let stamp = if let Some(edit) = &active_edit {
            format!(
                "edit:{}:{}:{}",
                edit.id, edit.updated_at, download.asset_count
            )
        } else {
            let target_version = version.unwrap_or(download.current_version);
            let target_path = reader_version_path(&download, &versions, target_version);
            let metadata = std::fs::metadata(&target_path).ok();
            let modified = metadata
                .as_ref()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            format!(
                "source:{target_version}:{}:{modified}:{}:{}",
                metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
                download.content_hash.as_deref().unwrap_or(""),
                download.asset_count
            )
        };
        let key = ReaderCacheKey {
            storage: self.storage_dir.clone(),
            download_id,
            requested_version: version,
            stamp,
        };
        let tick = READER_CACHE_TICK.fetch_add(1, AtomicOrdering::Relaxed);
        let cache = READER_CONTENT_CACHE.get_or_init(Default::default);
        if let Some(entry) = cache.lock().get_mut(&key) {
            entry.last_used = tick;
            return Ok(entry.clone());
        }

        let assets = self.get_assets(download_id)?;
        let (html, plain_text) = if let Some(edit) = active_edit {
            let blocks = blocks_for_revision_locked(&conn, edit.id)?;
            (
                blocks_to_html(&blocks, &assets),
                blocks_to_plain_text(&blocks),
            )
        } else {
            drop(conn);
            reader_source_content(self, &download, &versions, version, &assets)?
        };
        let pages = Arc::new(paginate_reader_html(&html, &download.source));
        let entry = ReaderCacheEntry {
            bytes: pages.iter().map(String::len).sum::<usize>() + plain_text.len(),
            pages,
            total_plain_text_chars: plain_text.chars().count(),
            last_used: tick,
        };
        let mut cache = cache.lock();
        cache.retain(|candidate, _| {
            candidate.storage != key.storage
                || candidate.download_id != download_id
                || candidate.requested_version != version
                || candidate == &key
        });
        cache.insert(key, entry.clone());
        while cache.len() > READER_CACHE_MAX_DOCUMENTS
            || cache.values().map(|entry| entry.bytes).sum::<usize>() > READER_CACHE_MAX_BYTES
        {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            // Keep one oversized current document cached; otherwise the next
            // page request would immediately parse it again.
            if cache.len() == 1 {
                break;
            }
            cache.remove(&oldest);
        }
        Ok(entry)
    }

    pub fn get_reader_document(
        &self,
        download_id: i64,
        version: Option<i64>,
    ) -> Result<ReaderDocument, String> {
        let download = self.get_download(download_id)?;
        let assets = self.get_assets(download_id)?;
        let versions = self.get_versions(download_id)?;

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let active_edit = active_edit_revision_locked(&conn, download_id)?;
        if version.is_none() {
            if let Some(edit) = active_edit.clone() {
                let blocks = blocks_for_revision_locked(&conn, edit.id)?;
                return Ok(ReaderDocument {
                    download,
                    assets: assets.clone(),
                    versions,
                    html: blocks_to_html(&blocks, &assets),
                    plain_text: blocks_to_plain_text(&blocks),
                    is_edited: true,
                    active_edit_revision: Some(edit),
                });
            }
        }
        drop(conn);

        let target_version = version.unwrap_or(download.current_version);
        let raw_json = self.read_download_json_for_version(&download, &versions, target_version)?;
        let html = if download.source == "pixiv" {
            super::parser::parse_pixiv_to_html(&raw_json, &assets)
        } else if download.source == "fanbox" {
            super::parser::parse_fanbox_to_html(&raw_json, &assets)
        } else {
            String::new()
        };
        let plain_text = serde_json::from_str::<serde_json::Value>(&raw_json)
            .ok()
            .map(|value| extract_search_body(&value, &download.source))
            .unwrap_or_default();

        Ok(ReaderDocument {
            download,
            assets,
            versions,
            html,
            plain_text,
            is_edited: false,
            active_edit_revision: active_edit,
        })
    }

    pub fn get_editor_document(&self, download_id: i64) -> Result<EditorDocument, String> {
        let download = self.get_download(download_id)?;
        let assets = self.get_assets(download_id)?;
        let versions = self.get_versions(download_id)?;
        let base_version = versions
            .iter()
            .map(|v| v.version)
            .max()
            .unwrap_or(download.current_version);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let active_revision = active_edit_revision_locked(&conn, download_id)?;
        let draft_revision = draft_edit_revision_locked(&conn, download_id)?;

        if let Some(draft) = draft_revision.clone() {
            let blocks = blocks_for_revision_locked(&conn, draft.id)?;
            return Ok(EditorDocument {
                download,
                assets,
                active_revision,
                draft_revision: Some(draft),
                base_version,
                blocks,
            });
        }

        if let Some(active) = active_revision.clone() {
            let blocks = blocks_for_revision_locked(&conn, active.id)?;
            return Ok(EditorDocument {
                download,
                assets,
                active_revision: Some(active),
                draft_revision: None,
                base_version,
                blocks,
            });
        }
        drop(conn);

        let raw_json =
            self.read_download_json_for_version(&download, &versions, download.current_version)?;
        let source_html = if download.source == "pixiv" {
            super::parser::parse_pixiv_to_html(&raw_json, &assets)
        } else if download.source == "fanbox" {
            super::parser::parse_fanbox_to_html(&raw_json, &assets)
        } else {
            String::new()
        };
        let blocks = html_to_editor_blocks(&source_html, &assets);

        Ok(EditorDocument {
            download,
            assets,
            active_revision,
            draft_revision: None,
            base_version,
            blocks,
        })
    }

    pub fn save_work_draft(
        &self,
        download_id: i64,
        base_version: i64,
        blocks: &[WorkBlockInput],
    ) -> Result<WorkEditRevision, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let normalized_blocks = normalize_block_inputs(blocks);
        let content_hash = hash_blocks(&normalized_blocks);
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;

        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'archived', updated_at = ?2
             WHERE download_id = ?1 AND status = 'draft'",
            params![download_id, now],
        )
        .map_err(|e| format!("Failed to archive previous draft: {}", e))?;

        tx.execute(
            "INSERT INTO work_edit_revisions (
                download_id, base_version, status, title, content_hash, created_at, updated_at
             ) VALUES (?1, ?2, 'draft', NULL, ?3, ?4, ?4)",
            params![download_id, base_version, content_hash, now],
        )
        .map_err(|e| format!("Failed to insert draft revision: {}", e))?;
        let revision_id = tx.last_insert_rowid();

        insert_work_blocks_locked(&tx, revision_id, &normalized_blocks)?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        get_work_edit_revision_locked(&conn, revision_id)
    }

    pub fn activate_work_edit(&self, edit_revision_id: i64) -> Result<WorkEditRevision, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let revision = get_work_edit_revision_locked(&conn, edit_revision_id)?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;
        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'archived', updated_at = ?2
             WHERE download_id = ?1 AND status = 'active'",
            params![revision.download_id, now],
        )
        .map_err(|e| format!("Failed to archive active revision: {}", e))?;
        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'active', updated_at = ?2
             WHERE id = ?1",
            params![edit_revision_id, now],
        )
        .map_err(|e| format!("Failed to activate revision: {}", e))?;
        tx.execute(
            "DELETE FROM search_index_state WHERE download_id = ?1",
            params![revision.download_id],
        )
        .map_err(|e| format!("Failed to invalidate search index: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        reindex_download_locked(&conn, &self.storage_dir, revision.download_id)?;
        get_work_edit_revision_locked(&conn, edit_revision_id)
    }

    fn read_download_json_for_version(
        &self,
        download: &DownloadEntry,
        versions: &[DownloadVersion],
        version: i64,
    ) -> Result<String, String> {
        let mut target_path = download
            .original_json_path
            .clone()
            .filter(|p| Path::new(p).exists())
            .unwrap_or_else(|| download.json_path.clone());
        if version != download.current_version {
            if let Some(target_version) = versions.iter().find(|v| v.version == version) {
                target_path = target_version
                    .original_json_path
                    .clone()
                    .filter(|p| Path::new(p).exists())
                    .unwrap_or_else(|| target_version.json_path.clone());
            }
        }
        std::fs::read_to_string(&target_path)
            .map_err(|e| format!("Failed to read download JSON: {}", e))
    }

    /// ダウンロードを削除（カスケードでアセットも削除）
    pub fn delete_download(&self, id: i64) -> Result<(), String> {
        let json_path = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;

            let (jp, _ojp): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT json_path, original_json_path FROM downloads WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| format!("Download not found: {}", e))?;

            // DBから削除
            conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
                .map_err(|e| format!("Delete failed: {}", e))?;

            jp
        }; // ここで conn (MutexGuard) がスコープ外になり、ロックが即座に解放される！

        // データベースのロックが解放された安全な状態で、重いフォルダ削除（リトライ待ちが発生し得る）を実行
        if let Some(jp) = json_path {
            let jp_path = std::path::Path::new(&jp);
            // jp_path は downloads/{source}/{source_id}/v{version}/data.json
            // その parent() である version_dir は downloads/{source}/{source_id}/v{version}
            if let Some(version_dir) = jp_path.parent() {
                // その parent() である work_root_dir は downloads/{source}/{source_id}
                if let Some(work_root_dir) = version_dir.parent() {
                    if work_root_dir.exists() {
                        if let Err(e) = remove_dir_all_resilient(work_root_dir) {
                            log::warn!(
                                "Failed to remove work root directory {:?}: {}",
                                work_root_dir,
                                e
                            );
                        }
                        // その親フォルダ（ソースフォルダ: e.g. downloads/pixiv）が空ならクリーンアップ
                        if let Some(source_dir) = work_root_dir.parent() {
                            let _ = remove_dir_resilient(source_dir); // 空のときのみ成功
                        }
                    }
                }
            }
        }

        self.invalidate_index_status();
        Ok(())
    }

    pub fn recover_update_jobs_on_startup(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_jobs
             SET status = 'paused',
                 active_label = '前回の起動中に中断されました',
                 updated_at = ?1,
                 finished_at = NULL
             WHERE status IN ('running', 'queued', 'canceling')",
            params![chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to recover update jobs: {}", e))?;
        conn.execute(
            "UPDATE update_job_items
             SET status = 'queued', updated_at = CURRENT_TIMESTAMP
             WHERE status = 'running'",
            [],
        )
        .map_err(|e| format!("Failed to recover update job items: {}", e))?;
        Ok(())
    }

    pub fn create_update_job(
        &self,
        job_id: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start update job transaction: {}", e))?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut persisted_request = request.clone();
        persisted_request.credentials = None;
        let request_json = serde_json::to_string(&persisted_request).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO update_jobs (
                id, scope, mode, status, request_json, totals, processed,
                candidate_count, saved_count, error_count, active_label,
                started_at, updated_at, finished_at
             ) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, 0, 0, 0, 0, NULL, ?6, ?6, NULL)",
            params![
                job_id,
                request.scope,
                request.mode,
                request_json,
                items.len() as i64,
                now,
            ],
        )
        .map_err(|e| format!("Failed to insert update job: {}", e))?;
        for item in items {
            tx.execute(
                "INSERT INTO update_job_items (
                    job_id, item_type, source, source_id, target_type, title, payload_json, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    job_id,
                    item.item_type,
                    item.source,
                    item.source_id,
                    item.target_type,
                    item.title,
                    item.payload_json,
                    item.status,
                ],
            )
            .map_err(|e| format!("Failed to insert update job item: {}", e))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit update job: {}", e))?;
        drop(conn);
        self.append_update_job_log(job_id, "info", "更新ジョブを作成しました")?;
        self.update_job_snapshot(job_id)
    }

    pub fn list_update_jobs(&self) -> Result<Vec<UpdateJobSummary>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at
                 FROM update_jobs
                 ORDER BY updated_at DESC, started_at DESC
                 LIMIT 30",
            )
            .map_err(|e| format!("Failed to prepare update job list: {}", e))?;
        let rows = stmt
            .query_map([], update_job_summary_from_row)
            .map_err(|e| format!("Failed to list update jobs: {}", e))?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|e| format!("Failed to read update job: {}", e))?);
        }
        Ok(jobs)
    }

    pub fn update_job_snapshot(&self, job_id: &str) -> Result<UpdateJobSnapshot, String> {
        self.sync_update_job_counters(job_id)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let summary = conn
            .query_row(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at
                 FROM update_jobs WHERE id = ?1",
                params![job_id],
                update_job_summary_from_row,
            )
            .map_err(|e| format!("Update job not found: {}", e))?;

        let mut log_stmt = conn
            .prepare(
                "SELECT id, log_type, message, created_at
                 FROM (
                    SELECT id, log_type, message, created_at
                    FROM update_job_logs
                    WHERE job_id = ?1
                    ORDER BY id DESC
                    LIMIT 300
                 ) ORDER BY id ASC",
            )
            .map_err(|e| format!("Failed to prepare update job logs: {}", e))?;
        let log_rows = log_stmt
            .query_map(params![job_id], |row| {
                Ok(UpdateJobLog {
                    id: row.get(0)?,
                    log_type: row.get(1)?,
                    message: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("Failed to read update job logs: {}", e))?;
        let mut logs = Vec::new();
        for row in log_rows {
            logs.push(row.map_err(|e| format!("Failed to read update job log: {}", e))?);
        }

        let mut candidate_stmt = conn
            .prepare(
                "SELECT id, source, source_id, title, target_type, payload_json, status
                 FROM update_job_items
                 WHERE job_id = ?1 AND item_type = 'candidate'
                 ORDER BY id ASC",
            )
            .map_err(|e| format!("Failed to prepare update job candidates: {}", e))?;
        let candidate_rows = candidate_stmt
            .query_map(params![job_id], |row| {
                let id: i64 = row.get(0)?;
                let source: String = row.get(1)?;
                let source_id: String = row.get(2)?;
                let title: String = row.get(3)?;
                let target_type: String = row.get(4)?;
                let payload_json: String = row.get(5)?;
                let status: String = row.get(6)?;
                let payload: serde_json::Value =
                    serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
                let target_label = payload
                    .get("targetLabel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let subtitle = payload
                    .get("subtitle")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(UpdateJobCandidate {
                    id,
                    key: format!("candidate:{}:{}:{}", source, source_id, id),
                    source,
                    source_id,
                    title,
                    subtitle,
                    target_label,
                    target_type,
                    selected: matches!(status.as_str(), "candidate" | "queued"),
                    status,
                })
            })
            .map_err(|e| format!("Failed to read update job candidates: {}", e))?;
        let mut candidates = Vec::new();
        for row in candidate_rows {
            candidates
                .push(row.map_err(|e| format!("Failed to read update job candidate: {}", e))?);
        }

        Ok(UpdateJobSnapshot {
            job_id: summary.job_id,
            status: summary.status,
            scope: summary.scope,
            mode: summary.mode,
            totals: summary.totals,
            processed: summary.processed,
            candidate_count: summary.candidate_count,
            saved_count: summary.saved_count,
            error_count: summary.error_count,
            active_label: summary.active_label,
            logs,
            candidates,
            started_at: summary.started_at,
            updated_at: summary.updated_at,
            finished_at: summary.finished_at,
        })
    }

    pub fn set_update_job_status(
        &self,
        job_id: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let terminal = matches!(
            status,
            "completed" | "failed" | "canceled" | "auth_required"
        );
        conn.execute(
            "UPDATE update_jobs
             SET status = ?1,
                 active_label = ?2,
                 updated_at = ?3,
                 finished_at = CASE WHEN ?4 THEN COALESCE(finished_at, ?3) ELSE NULL END
             WHERE id = ?5",
            params![status, active_label, now, terminal, job_id],
        )
        .map_err(|e| format!("Failed to update job status: {}", e))?;
        Ok(())
    }

    pub fn prepare_update_job_resume(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_job_items
             SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE job_id = ?1 AND status = 'running'",
            params![job_id],
        )
        .map_err(|e| format!("Failed to reset running update items: {}", e))?;
        if retry_failed {
            conn.execute(
                "UPDATE update_job_items
                 SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE job_id = ?1 AND status = 'failed'",
                params![job_id],
            )
            .map_err(|e| format!("Failed to reset failed update items: {}", e))?;
        }
        Ok(())
    }

    pub fn append_update_job_log(
        &self,
        job_id: &str,
        log_type: &str,
        message: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_job_logs (job_id, log_type, message)
             VALUES (?1, ?2, ?3)",
            params![job_id, log_type, message],
        )
        .map_err(|e| format!("Failed to append update job log: {}", e))?;
        conn.execute(
            "UPDATE update_jobs SET updated_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), job_id],
        )
        .map_err(|e| format!("Failed to touch update job: {}", e))?;
        Ok(())
    }

    pub fn update_job_status_value(&self, job_id: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT status FROM update_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Update job not found: {}", e))
    }

    pub fn next_update_job_item(&self, job_id: &str) -> Result<Option<UpdateJobItem>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let item = conn
            .query_row(
                "SELECT id, job_id, item_type, source, source_id, target_type, title,
                        payload_json, status, error, result_download_id
                 FROM update_job_items
                 WHERE job_id = ?1 AND status = 'queued'
                 ORDER BY CASE item_type WHEN 'work' THEN 0 WHEN 'target' THEN 1 ELSE 2 END, id ASC
                 LIMIT 1",
                params![job_id],
                update_job_item_from_row,
            )
            .optional()
            .map_err(|e| format!("Failed to fetch next update item: {}", e))?;
        if let Some(item) = item {
            conn.execute(
                "UPDATE update_job_items SET status = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![item.id],
            )
            .map_err(|e| format!("Failed to mark update item running: {}", e))?;
            conn.execute(
                "UPDATE update_jobs SET active_label = ?1, status = 'running', updated_at = ?2 WHERE id = ?3",
                params![item.title, chrono::Utc::now().to_rfc3339(), job_id],
            )
            .map_err(|e| format!("Failed to mark update job running: {}", e))?;
            return Ok(Some(UpdateJobItem {
                status: "running".to_string(),
                ..item
            }));
        }
        Ok(None)
    }

    pub fn complete_update_job_item(
        &self,
        item_id: i64,
        status: &str,
        error: Option<&str>,
        result_download_id: Option<i64>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_job_items
             SET status = ?1, error = ?2, result_download_id = ?3, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4",
            params![status, error, result_download_id, item_id],
        )
        .map_err(|e| format!("Failed to update job item: {}", e))?;
        Ok(())
    }

    pub fn insert_update_job_candidate(
        &self,
        job_id: &str,
        candidate: &UpdateJobItemInput,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM update_job_items
                 WHERE job_id = ?1 AND item_type = 'candidate' AND source = ?2 AND source_id = ?3",
                params![job_id, candidate.source, candidate.source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check candidate: {}", e))?;
        if exists > 0 {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO update_job_items (
                job_id, item_type, source, source_id, target_type, title, payload_json, status
             ) VALUES (?1, 'candidate', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                job_id,
                candidate.source,
                candidate.source_id,
                candidate.target_type,
                candidate.title,
                candidate.payload_json,
                candidate.status,
            ],
        )
        .map_err(|e| format!("Failed to insert update candidate: {}", e))?;
        Ok(())
    }

    pub fn queue_update_job_candidates(
        &self,
        job_id: &str,
        candidate_ids: &[i64],
    ) -> Result<i64, String> {
        if candidate_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut changed = 0i64;
        for id in candidate_ids {
            changed += conn
                .execute(
                    "UPDATE update_job_items
                     SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
                     WHERE id = ?1 AND job_id = ?2 AND item_type = 'candidate' AND status IN ('candidate', 'failed')",
                    params![id, job_id],
                )
                .map_err(|e| format!("Failed to queue update candidate: {}", e))?
                as i64;
        }
        Ok(changed)
    }

    pub fn clear_update_job(&self, job_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_jobs WHERE id = ?1 AND status NOT IN ('queued', 'running', 'canceling')",
            params![job_id],
        )
        .map_err(|e| format!("Failed to clear update job: {}", e))?;
        Ok(())
    }

    fn sync_update_job_counters(&self, job_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_jobs
             SET totals = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND item_type != 'candidate'),
                 processed = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND item_type != 'candidate' AND status IN ('done', 'saved', 'skipped', 'failed')),
                 candidate_count = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND item_type = 'candidate'),
                 saved_count = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND status = 'saved'),
                 error_count = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND status = 'failed')
             WHERE id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Failed to sync update job counters: {}", e))?;
        Ok(())
    }

    pub fn delete_downloads(&self, ids: &[i64]) -> Result<BulkMutationResult, String> {
        let mut seen = std::collections::HashSet::new();
        let mut changed_count = 0i64;

        for id in ids.iter().copied().filter(|id| seen.insert(*id)) {
            self.delete_download(id)?;
            changed_count += 1;
        }

        Ok(BulkMutationResult {
            matched_count: seen.len() as i64,
            changed_count,
        })
    }

    /// 選択した作品のお気に入り・更新監視をまとめて更新する。
    ///
    /// 1件ずつコマンドを呼ぶと選択数だけIPCとトランザクションが走るため、
    /// 大量選択時に極端に遅くなる。単一トランザクションで処理する。
    pub fn set_flags_for_ids(
        &self,
        ids: &[i64],
        favorite: Option<bool>,
        watch: Option<bool>,
    ) -> Result<BulkMutationResult, String> {
        let unique: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            ids.iter().copied().filter(|id| seen.insert(*id)).collect()
        };
        if unique.is_empty() || (favorite.is_none() && watch.is_none()) {
            return Ok(BulkMutationResult {
                matched_count: unique.len() as i64,
                changed_count: 0,
            });
        }

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut changed_count = 0i64;
        {
            let mut assignments = Vec::new();
            if favorite.is_some() {
                assignments.push("favorite = ?1");
            }
            if watch.is_some() {
                assignments.push(if favorite.is_some() {
                    "watch_updates = ?2"
                } else {
                    "watch_updates = ?1"
                });
            }
            let sql = format!(
                "UPDATE downloads SET {} WHERE id = ?{}",
                assignments.join(", "),
                assignments.len() + 1
            );
            let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
            for id in &unique {
                let affected = match (favorite, watch) {
                    (Some(fav), Some(wat)) => stmt.execute(rusqlite::params![fav, wat, id]),
                    (Some(fav), None) => stmt.execute(rusqlite::params![fav, id]),
                    (None, Some(wat)) => stmt.execute(rusqlite::params![wat, id]),
                    (None, None) => Ok(0),
                }
                .map_err(|e| e.to_string())?;
                changed_count += affected as i64;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;

        Ok(BulkMutationResult {
            matched_count: unique.len() as i64,
            changed_count,
        })
    }

    /// 統計情報の取得
    pub fn get_stats(&self) -> Result<DbStats, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let total_downloads: i64 = conn
            .query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))
            .unwrap_or(0);
        let pixiv_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE source = 'pixiv'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let fanbox_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE source = 'fanbox'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_assets: i64 = conn
            .query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap_or(0);
        let total_size: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(file_size_bytes), 0) FROM downloads",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        Ok(DbStats {
            total_downloads,
            pixiv_count,
            fanbox_count,
            total_assets,
            total_size_bytes: total_size,
        })
    }

    pub fn get_library_diagnostics(&self) -> Result<LibraryDiagnostics, String> {
        self.get_library_diagnostics_with_progress(&|_| {})
    }

    /// Measuring the library is not one operation but six, and the last of them
    /// walks the whole storage tree. Reporting which one is running is the
    /// difference between "this is working" and "this has hung".
    pub fn get_library_diagnostics_with_progress(
        &self,
        on_progress: &dyn Fn(DiagnosticsProgress),
    ) -> Result<LibraryDiagnostics, String> {
        const STEPS: i64 = 6;
        let report = |step: i64, phase: &'static str| {
            on_progress(DiagnosticsProgress {
                step,
                total: STEPS,
                phase,
            });
        };
        report(0, "database");
        let (
            total_downloads,
            total_assets,
            total_versions,
            total_text_length,
            benchmark_query,
            sqlite_page_count,
            sqlite_free_pages,
            sqlite_cache_size_bytes,
            orphan_asset_rows,
            orphan_asset_bytes,
        ) = {
            let conn = self.read_conn()?;
            let total_downloads = conn
                .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
                .unwrap_or(0);
            let total_assets = conn
                .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
                .unwrap_or(0);
            let total_versions = conn
                .query_row("SELECT COUNT(*) FROM download_versions", [], |row| {
                    row.get(0)
                })
                .unwrap_or(0);
            let total_text_length = conn
                .query_row(
                    "SELECT COALESCE(SUM(text_length), 0) FROM downloads",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            // A common real author is stable enough to make repeated measurements
            // comparable and avoids shipping any user content outside the process.
            let benchmark_query = conn
                .query_row(
                    "SELECT author_name FROM downloads
                     WHERE TRIM(author_name) != ''
                     GROUP BY author_name
                     ORDER BY COUNT(*) DESC, author_name ASC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .unwrap_or(None);
            let page_size = conn
                .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
                .unwrap_or(4096)
                .max(1);
            let sqlite_page_count: i64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .unwrap_or(0);
            let sqlite_free_pages: i64 = conn
                .query_row("PRAGMA freelist_count", [], |row| row.get(0))
                .unwrap_or(0);
            let cache_pages = conn
                .query_row("PRAGMA cache_size", [], |row| row.get::<_, i64>(0))
                .unwrap_or(-64_000);
            let sqlite_cache_size_bytes = if cache_pages < 0 {
                cache_pages.unsigned_abs().saturating_mul(1024)
            } else {
                (cache_pages as u64).saturating_mul(page_size as u64)
            };
            let (orphan_asset_rows, orphan_asset_bytes) = conn
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(a.file_size_bytes), 0)
                     FROM assets a LEFT JOIN downloads d ON d.id = a.download_id
                     WHERE d.id IS NULL",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or((0, 0));
            (
                total_downloads,
                total_assets,
                total_versions,
                total_text_length,
                benchmark_query,
                sqlite_page_count,
                sqlite_free_pages,
                sqlite_cache_size_bytes,
                orphan_asset_rows,
                orphan_asset_bytes,
            )
        };

        let base_params = SearchV2Params {
            text: None,
            query: None,
            source: None,
            content_type: None,
            sort_by: Some("downloadedAt".to_string()),
            sort_order: Some("desc".to_string()),
            limit: Some(80),
            cursor: None,
            favorite: None,
            tags_include: None,
            tags_exclude: None,
            tag_filter_mode: None,
            authors_include: None,
            authors_exclude: None,
            min_char_count: None,
            max_char_count: None,
            asset_filter: None,
            watch_filter: None,
            person_source: None,
            person_key: None,
            series_source: None,
            series_key: None,
            offset: None,
            ids_include: None,
            view_mode: Some("list".to_string()),
            projection: Some("libraryList".to_string()),
            search_mode: Some("lexical".to_string()),
        };
        report(1, "list-benchmark");
        let mut list_samples = Vec::with_capacity(7);
        for _ in 0..7 {
            let started = std::time::Instant::now();
            let _ = self.search_downloads_v2(&base_params)?;
            list_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        let list_first_page_ms = list_samples[0];
        let (list_p50_ms, list_p95_ms) = benchmark_percentiles(&mut list_samples);

        report(2, "search-benchmark");
        let lexical_samples = if let Some(query) = benchmark_query.as_ref() {
            let mut params = base_params.clone();
            params.query = Some(format!("\"{}\"", query.replace('"', "")));
            params.sort_by = Some("relevance".to_string());
            let mut samples = Vec::with_capacity(7);
            for _ in 0..7 {
                let started = std::time::Instant::now();
                let _ = self.search_downloads_v2(&params)?;
                samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            Some(samples)
        } else {
            None
        };
        let lexical_search_ms = lexical_samples.as_ref().map(|samples| samples[0]);
        let (lexical_search_p50_ms, lexical_search_p95_ms) = lexical_samples
            .map(|mut samples| {
                let (p50, p95) = benchmark_percentiles(&mut samples);
                (Some(p50), Some(p95))
            })
            .unwrap_or((None, None));
        report(3, "author-benchmark");
        let (exact_author_p50_ms, exact_author_p95_ms) =
            if let Some(query) = benchmark_query.as_ref() {
                let mut params = base_params.clone();
                params.query = Some(query.clone());
                params.sort_by = Some("relevance".to_string());
                let mut samples = Vec::with_capacity(7);
                for _ in 0..7 {
                    let started = std::time::Instant::now();
                    let _ = self.search_downloads_v2(&params)?;
                    samples.push(started.elapsed().as_secs_f64() * 1_000.0);
                }
                let (p50, p95) = benchmark_percentiles(&mut samples);
                (Some(p50), Some(p95))
            } else {
                (None, None)
            };

        let app_data = self
            .storage_dir
            .parent()
            .unwrap_or(self.storage_dir.as_path());
        let known_asset_paths = {
            let conn = self.read_conn()?;
            let mut stmt = conn
                .prepare("SELECT local_path FROM assets")
                .map_err(|e| format!("Asset diagnostics prepare failed: {e}"))?;
            let paths = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Asset diagnostics query failed: {e}"))?;
            let mut known = std::collections::HashSet::new();
            for path in paths {
                known.insert(normalized_diagnostic_path(Path::new(
                    &path.map_err(|e| format!("Asset diagnostics row failed: {e}"))?,
                )));
            }
            known
        };
        report(4, "storage-scan");
        let (orphan_asset_files, orphan_asset_file_bytes) =
            orphan_asset_file_stats(&self.storage_dir, &known_asset_paths);
        report(5, "index-size");
        let wal_path = PathBuf::from(format!("{}-wal", self.db_path.display()));
        let live_pages = sqlite_page_count.saturating_sub(sqlite_free_pages).max(0) as u64;
        let page_size = if sqlite_page_count > 0 {
            file_size_or_zero(&self.db_path) / sqlite_page_count as u64
        } else {
            0
        };
        let live_database_bytes = live_pages.saturating_mul(page_size);
        let fragmentation_percent = if sqlite_page_count > 0 {
            sqlite_free_pages.max(0) as f64 / sqlite_page_count as f64 * 100.0
        } else {
            0.0
        };
        let lexical_index_root = self.storage_dir.join("search-index");
        Ok(LibraryDiagnostics {
            measured_at: chrono::Utc::now().to_rfc3339(),
            total_downloads,
            total_assets,
            total_versions,
            total_text_length,
            database_size_bytes: file_size_or_zero(&self.db_path),
            wal_size_bytes: file_size_or_zero(&wal_path),
            storage_size_bytes: recursive_file_size(&self.storage_dir),
            lexical_index_size_bytes: recursive_file_size(&lexical_index_root),
            lexical_index_file_count: recursive_file_count(&lexical_index_root, None),
            lexical_index_segment_count: recursive_file_count(&lexical_index_root, Some("store")),
            semantic_index_size_bytes: recursive_file_size(&app_data.join("search")),
            sqlite_page_count,
            sqlite_free_pages,
            sqlite_cache_size_bytes,
            live_database_bytes,
            fragmentation_percent,
            orphan_asset_rows,
            orphan_asset_bytes,
            orphan_asset_files,
            orphan_asset_file_bytes,
            process_memory_bytes: current_process_memory_bytes(),
            list_first_page_ms,
            list_p50_ms,
            list_p95_ms,
            lexical_search_ms,
            lexical_search_p50_ms,
            lexical_search_p95_ms,
            exact_author_p50_ms,
            exact_author_p95_ms,
            benchmark_query,
            search_index: self.get_search_index_status()?,
        })
    }

    /// Runs SQLite's lightweight planner maintenance, and optionally performs
    /// an explicit compaction. Compaction is never automatic: it can require a
    /// temporary copy roughly as large as the database and blocks writers.
    pub fn maintain_library_database(
        &self,
        compact: bool,
    ) -> Result<LibraryMaintenanceResult, String> {
        let before_bytes = file_size_or_zero(&self.db_path);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE); PRAGMA optimize;")
            .map_err(|e| format!("SQLite optimize failed: {e}"))?;
        if compact {
            let required = before_bytes
                .saturating_add(before_bytes / 4)
                .saturating_add(256 * 1024 * 1024);
            let parent = self
                .db_path
                .parent()
                .ok_or_else(|| "Database path has no parent".to_string())?;
            let available = available_space_bytes(parent).ok_or_else(|| {
                "Could not determine free disk space; compaction was not started".to_string()
            })?;
            if available < required {
                return Err(format!(
                    "Not enough free disk space for safe compaction (required {required} bytes, available {available} bytes)"
                ));
            }
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA optimize;")
                .map_err(|e| format!("SQLite compaction failed: {e}"))?;
        }
        drop(conn);
        let after_bytes = file_size_or_zero(&self.db_path);
        Ok(LibraryMaintenanceResult {
            compacted: compact,
            before_bytes,
            after_bytes,
            reclaimed_bytes: before_bytes.saturating_sub(after_bytes),
        })
    }

    /// The four numbers the library sidebar shows, in one pass over the table.
    ///
    /// `reading_ids` comes from the client because reading positions are kept
    /// per device; counting how many of them still exist is what stops the
    /// shelf from advertising works that were deleted.
    pub fn get_library_shelf_counts(
        &self,
        reading_ids: &[i64],
    ) -> Result<LibraryShelfCounts, String> {
        let conn = self.read_conn()?;
        let (total, favorite, watched) = conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(favorite), 0),
                        COALESCE(SUM(watch_updates), 0)
                 FROM downloads",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Shelf count failed: {e}"))?;

        let bounded = bounded_id_list(reading_ids);
        let reading = if bounded.is_empty() {
            0
        } else {
            let encoded = serde_json::to_string(&bounded)
                .map_err(|e| format!("Shelf id encoding failed: {e}"))?;
            conn.query_row(
                "SELECT COUNT(*) FROM downloads WHERE id IN (SELECT value FROM json_each(?1))",
                params![encoded],
                |row| row.get(0),
            )
            .map_err(|e| format!("Reading shelf count failed: {e}"))?
        };

        Ok(LibraryShelfCounts {
            total,
            favorite,
            watched,
            reading,
        })
    }

    /// The series an author has works in, most recent first.
    ///
    /// An author page that can only list works flat makes a prolific author
    /// unreadable: the series are how their output is actually organised.
    pub fn list_entity_series(
        &self,
        source: &str,
        person_key: &str,
        limit: i64,
    ) -> Result<Vec<EntityFacet>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT ds.series_source,
                        ds.series_key,
                        COALESCE(s.title, ds.title) AS display_name,
                        COUNT(DISTINCT d.id) AS count,
                        s.cover_path,
                        s.description,
                        s.updated_at,
                        MAX(d.downloaded_at) AS latest_downloaded_at,
                        (
                            SELECT d2.title FROM downloads d2
                            JOIN download_series ds2 ON ds2.download_id = d2.id
                            WHERE ds2.series_source = ds.series_source
                              AND ds2.series_key = ds.series_key
                            ORDER BY COALESCE(ds2.content_order, 999999), d2.id
                            LIMIT 1
                        ) AS sample_title
                 FROM download_series ds
                 JOIN downloads d ON d.id = ds.download_id
                 JOIN download_people dp ON dp.download_id = d.id
                 LEFT JOIN series s ON s.source = ds.series_source AND s.source_key = ds.series_key
                 WHERE dp.person_source = ?1 AND dp.person_key = ?2
                 GROUP BY ds.series_source, ds.series_key, COALESCE(s.title, ds.title)
                 ORDER BY MAX(COALESCE(d.source_created_at, d.downloaded_at)) DESC, count DESC
                 LIMIT ?3",
            )
            .map_err(|e| format!("Entity series prepare failed: {e}"))?;
        let rows = stmt
            .query_map(params![source, person_key, limit.clamp(1, 200)], |row| {
                Ok(EntityFacet {
                    source: row.get(0)?,
                    source_key: row.get(1)?,
                    display_name: row.get(2)?,
                    count: row.get(3)?,
                    cover_path: row.get(4)?,
                    description: row.get(5)?,
                    updated_at: row.get(6)?,
                    latest_downloaded_at: row.get(7)?,
                    sample_title: row.get(8)?,
                    icon_path: None,
                    banner_path: None,
                })
            })
            .map_err(|e| format!("Entity series query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Entity series row failed: {e}"))?);
        }
        Ok(out)
    }

    /// The tags the works of one author or series carry, most used first.
    ///
    /// A series is as worth narrowing as an author: a long one accumulates
    /// enough tags that they are the only practical way through it.
    pub fn list_entity_tags(
        &self,
        kind: &str,
        source: &str,
        entity_key: &str,
        limit: i64,
    ) -> Result<Vec<FacetCount>, String> {
        let conn = self.read_conn()?;
        // The relation table is the only difference; the join is otherwise the
        // same, and the kind is a closed set rather than caller-supplied SQL.
        let sql = if kind == "series" {
            "SELECT t.name, COUNT(DISTINCT d.id) AS count
             FROM download_series ds
             JOIN downloads d ON d.id = ds.download_id
             JOIN download_tags dt ON dt.download_id = d.id
             JOIN tags t ON t.id = dt.tag_id
             WHERE ds.series_source = ?1 AND ds.series_key = ?2
             GROUP BY t.id, t.name
             ORDER BY count DESC, t.name ASC
             LIMIT ?3"
        } else {
            "SELECT t.name, COUNT(DISTINCT d.id) AS count
             FROM download_people dp
             JOIN downloads d ON d.id = dp.download_id
             JOIN download_tags dt ON dt.download_id = d.id
             JOIN tags t ON t.id = dt.tag_id
             WHERE dp.person_source = ?1 AND dp.person_key = ?2
             GROUP BY t.id, t.name
             ORDER BY count DESC, t.name ASC
             LIMIT ?3"
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Entity tag prepare failed: {e}"))?;
        let rows = stmt
            .query_map(params![source, entity_key, limit.clamp(1, 200)], |row| {
                Ok(FacetCount {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            })
            .map_err(|e| format!("Entity tag query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Entity tag row failed: {e}"))?);
        }
        Ok(out)
    }

    pub fn list_saved_searches(&self) -> Result<Vec<SavedSearch>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, query, params_json, created_at, updated_at
                 FROM saved_searches
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|e| format!("Saved search query prepare failed: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SavedSearch {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    query: row.get(2)?,
                    params_json: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("Saved search query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Saved search row read failed: {e}"))?);
        }
        Ok(out)
    }

    /// Creates or updates one saved search.
    ///
    /// Names are unique, and saving again under a name that already exists is
    /// the reader replacing that search, not an error to report back at them.
    pub fn upsert_saved_search(&self, input: &SavedSearchInput) -> Result<SavedSearch, String> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err("保存する検索の名前を入力してください".to_string());
        }
        if name.chars().count() > MAX_SAVED_SEARCH_NAME {
            return Err(format!(
                "名前は{MAX_SAVED_SEARCH_NAME}文字以内にしてください"
            ));
        }
        if input.params_json.len() > MAX_SAVED_SEARCH_PARAMS_BYTES {
            return Err("検索条件が大きすぎます".to_string());
        }
        serde_json::from_str::<serde_json::Value>(&input.params_json)
            .map_err(|_| "検索条件を保存できませんでした".to_string())?;

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let query = input
            .query
            .as_deref()
            .map(|value| value.chars().take(MAX_SAVED_SEARCH_QUERY).collect::<String>());

        let existing_id: Option<i64> = match input.id {
            Some(id) => Some(id),
            None => conn
                .query_row(
                    "SELECT id FROM saved_searches WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("Saved search lookup failed: {e}"))?,
        };

        let id = match existing_id {
            Some(id) => {
                let changed = conn
                    .execute(
                        "UPDATE saved_searches
                         SET name = ?2, query = ?3, params_json = ?4, updated_at = ?5
                         WHERE id = ?1",
                        params![id, name, query, input.params_json, now],
                    )
                    .map_err(|e| format!("Saved search update failed: {e}"))?;
                if changed == 0 {
                    return Err("保存した検索が見つかりません".to_string());
                }
                id
            }
            None => {
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM saved_searches", [], |row| row.get(0))
                    .unwrap_or(0);
                if count >= MAX_SAVED_SEARCHES {
                    return Err(format!(
                        "保存できる検索は{MAX_SAVED_SEARCHES}件までです。不要なものを削除してください"
                    ));
                }
                conn.execute(
                    "INSERT INTO saved_searches (name, query, params_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![name, query, input.params_json, now],
                )
                .map_err(|e| format!("Saved search insert failed: {e}"))?;
                conn.last_insert_rowid()
            }
        };

        conn.query_row(
            "SELECT id, name, query, params_json, created_at, updated_at
             FROM saved_searches WHERE id = ?1",
            params![id],
            |row| {
                Ok(SavedSearch {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    query: row.get(2)?,
                    params_json: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .map_err(|e| format!("Saved search read-back failed: {e}"))
    }

    pub fn delete_saved_search(&self, id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute("DELETE FROM saved_searches WHERE id = ?1", params![id])
            .map_err(|e| format!("Saved search delete failed: {e}"))?;
        Ok(removed > 0)
    }

    pub fn search_index_segment_count(&self) -> Result<u64, String> {
        super::tantivy_index::searchable_segment_count(&self.storage_dir).map(|count| count as u64)
    }

    /// Explicit, disk-preflighted Tantivy maintenance. This is kept separate
    /// from the lightweight SQLite optimization because a multi-gigabyte merge
    /// can take minutes and temporarily needs space for both segment sets.
    pub fn optimize_search_index(&self) -> Result<SearchIndexOptimizationResult, String> {
        let index_root = self.storage_dir.join("search-index");
        let before_bytes = recursive_file_size(&index_root);
        let before_segments =
            super::tantivy_index::searchable_segment_count(&self.storage_dir)? as u64;
        if before_segments <= 1 {
            let started = std::time::Instant::now();
            let (reported_before, reported_after) =
                super::tantivy_index::optimize_segments(&self.storage_dir)?;
            let after_bytes = recursive_file_size(&index_root);
            return Ok(SearchIndexOptimizationResult {
                optimized: false,
                before_segments: reported_before as u64,
                after_segments: reported_after as u64,
                before_bytes,
                after_bytes,
                reclaimed_bytes: before_bytes.saturating_sub(after_bytes),
                elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
            });
        }

        let parent = index_root
            .parent()
            .ok_or_else(|| "Search index path has no parent".to_string())?;
        let required = before_bytes
            .saturating_add(before_bytes / 10)
            .saturating_add(512 * 1024 * 1024);
        let available = available_space_bytes(parent).ok_or_else(|| {
            "Could not determine free disk space; search index optimization was not started"
                .to_string()
        })?;
        if available < required {
            return Err(format!(
                "Not enough free disk space for safe search index optimization (required {required} bytes, available {available} bytes)"
            ));
        }

        let started = std::time::Instant::now();
        let (reported_before, reported_after) =
            super::tantivy_index::optimize_segments(&self.storage_dir)?;
        let after_bytes = recursive_file_size(&index_root);
        Ok(SearchIndexOptimizationResult {
            optimized: reported_after < reported_before,
            before_segments: reported_before as u64,
            after_segments: reported_after as u64,
            before_bytes,
            after_bytes,
            reclaimed_bytes: before_bytes.saturating_sub(after_bytes),
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        })
    }

    pub fn get_dashboard_summary(&self) -> Result<DashboardSummary, String> {
        let stats = self.get_stats()?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let favorite_count = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE favorite = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let watched_count = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE watch_updates = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let update_target_count = conn
            .query_row(
                "SELECT COUNT(*) FROM update_targets WHERE enabled = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let indexed_count = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM downloads d
                 JOIN search_index_state m ON m.download_id = d.id
                 WHERE m.current_version = d.current_version
                   AND COALESCE(m.content_hash, '') = COALESCE(d.content_hash, '')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let pending_index_count = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM downloads d
                 LEFT JOIN search_index_state m ON m.download_id = d.id
                 WHERE m.download_id IS NULL
                    OR m.current_version != d.current_version
                    OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let collect_facets = |sql: &str, limit: i64| -> Result<Vec<FacetCount>, String> {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("Dashboard facet prepare failed: {}", e))?;
            let rows = stmt
                .query_map(params![limit], |row| {
                    Ok(FacetCount {
                        name: row.get(0)?,
                        count: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Dashboard facet query failed: {}", e))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| format!("Dashboard facet row failed: {}", e))?);
            }
            Ok(out)
        };

        let top_tags = collect_facets(
            "SELECT t.name, COUNT(dt.download_id) AS count
             FROM tags t
             JOIN download_tags dt ON dt.tag_id = t.id
             GROUP BY t.id, t.name
             ORDER BY count DESC, t.name ASC
             LIMIT ?1",
            12,
        )?;
        let top_authors = collect_facets(
            "SELECT author_name, COUNT(*) AS count
             FROM downloads
             WHERE author_name IS NOT NULL AND author_name != ''
             GROUP BY author_name
             ORDER BY count DESC, author_name ASC
             LIMIT ?1",
            12,
        )?;

        let mut source_stmt = conn
            .prepare(
                "SELECT source, COUNT(*) AS count, COALESCE(SUM(file_size_bytes), 0)
                 FROM downloads
                 GROUP BY source
                 ORDER BY count DESC",
            )
            .map_err(|e| format!("Dashboard source prepare failed: {}", e))?;
        let source_rows = source_stmt
            .query_map([], |row| {
                Ok(SourceBreakdown {
                    source: row.get(0)?,
                    count: row.get(1)?,
                    total_size_bytes: row.get(2)?,
                })
            })
            .map_err(|e| format!("Dashboard source query failed: {}", e))?;
        let mut source_breakdown = Vec::new();
        for row in source_rows {
            source_breakdown.push(row.map_err(|e| format!("Dashboard source row failed: {}", e))?);
        }

        let mut trend_stmt = conn
            .prepare(
                "SELECT substr(downloaded_at, 1, 7) AS bucket,
                        COUNT(*) AS count,
                        SUM(CASE WHEN source = 'pixiv' THEN 1 ELSE 0 END) AS pixiv_count,
                        SUM(CASE WHEN source = 'fanbox' THEN 1 ELSE 0 END) AS fanbox_count,
                        COALESCE(SUM(file_size_bytes), 0) AS total_size
                 FROM downloads
                 GROUP BY bucket
                 ORDER BY bucket DESC
                 LIMIT 12",
            )
            .map_err(|e| format!("Dashboard trend prepare failed: {}", e))?;
        let trend_rows = trend_stmt
            .query_map([], |row| {
                Ok(DashboardTrendPoint {
                    bucket: row.get(0)?,
                    count: row.get(1)?,
                    pixiv_count: row.get(2)?,
                    fanbox_count: row.get(3)?,
                    total_size_bytes: row.get(4)?,
                })
            })
            .map_err(|e| format!("Dashboard trend query failed: {}", e))?;
        let mut monthly_downloads = Vec::new();
        for row in trend_rows {
            monthly_downloads.push(row.map_err(|e| format!("Dashboard trend row failed: {}", e))?);
        }
        monthly_downloads.reverse();

        drop(trend_stmt);
        drop(source_stmt);
        drop(conn);

        let recent_downloads = self
            .search_downloads_v2(&SearchV2Params {
                text: None,
                query: None,
                source: None,
                content_type: None,
                sort_by: Some("date".to_string()),
                sort_order: Some("desc".to_string()),
                limit: Some(8),
                cursor: None,
                favorite: None,
                tags_include: None,
                tags_exclude: None,
                tag_filter_mode: None,
                authors_include: None,
                authors_exclude: None,
                min_char_count: None,
                max_char_count: None,
                asset_filter: None,
                watch_filter: None,
                person_source: None,
                person_key: None,
                series_source: None,
                series_key: None,
                offset: None,
                ids_include: None,
                view_mode: Some("compact".to_string()),
                projection: Some("libraryCompact".to_string()),
                search_mode: None,
            })?
            .items;

        Ok(DashboardSummary {
            stats,
            favorite_count,
            watched_count,
            update_target_count,
            indexed_count,
            pending_index_count,
            top_tags,
            top_authors,
            recent_downloads,
            source_breakdown,
            monthly_downloads,
        })
    }

    /// ライブラリのフィルター候補一覧を取得
    /// 作者・シリーズの一覧を検索・ページングして返す。
    ///
    /// `get_filter_facets` は上位60件しか返さないため、ライブラリの
    /// 作者/シリーズタブでは大半のエンティティに到達できなかった。絞り込みと
    /// ページングをSQL側で行い、件数に依存せず一覧できるようにする。
    pub fn search_entity_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EntityFacet>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let filter = query.map(str::trim).unwrap_or("");
        let has_filter = !filter.is_empty();
        let like = format!("%{}%", filter.replace('%', "\\%").replace('_', "\\_"));

        let sql = match kind {
            "person" | "people" | "author" | "authors" => {
                let having = if has_filter {
                    "HAVING COALESCE(p.display_name, d.author_name) LIKE ?1 ESCAPE '\\'
                         OR COALESCE(p.description, '') LIKE ?1 ESCAPE '\\'"
                } else {
                    ""
                };
                format!(
                    "SELECT
                        d.source,
                        d.author_id,
                        COALESCE(p.display_name, d.author_name) AS display_name,
                        COUNT(DISTINCT d.id) AS count,
                        COALESCE(p.icon_path, p.cover_path) AS cover_path,
                        p.description,
                        p.updated_at,
                        MAX(d.downloaded_at) AS latest_downloaded_at,
                        (
                            SELECT d2.title
                            FROM downloads d2
                            WHERE d2.source = d.source AND d2.author_id = d.author_id
                            ORDER BY COALESCE(d2.source_created_at, d2.downloaded_at) DESC, d2.id DESC
                            LIMIT 1
                        ) AS sample_title,
                        p.icon_path,
                        p.cover_path AS banner_path
                     FROM downloads d
                     LEFT JOIN people p ON p.source = d.source AND p.source_key = d.author_id
                     WHERE d.author_id IS NOT NULL AND d.author_id != ''
                       AND d.author_name IS NOT NULL AND d.author_name != ''
                     GROUP BY d.source, d.author_id, COALESCE(p.display_name, d.author_name), p.icon_path, p.cover_path, p.description, p.updated_at
                     {having}
                     ORDER BY count DESC, display_name ASC
                     LIMIT ?{limit_index} OFFSET ?{offset_index}",
                    having = having,
                    limit_index = if has_filter { 2 } else { 1 },
                    offset_index = if has_filter { 3 } else { 2 },
                )
            }
            "series" => {
                let having = if has_filter {
                    "HAVING COALESCE(s.title, ds.title) LIKE ?1 ESCAPE '\\'
                         OR COALESCE(s.description, '') LIKE ?1 ESCAPE '\\'"
                } else {
                    ""
                };
                format!(
                    "SELECT
                        ds.series_source,
                        ds.series_key,
                        COALESCE(s.title, ds.title) AS title,
                        COUNT(DISTINCT ds.download_id) AS count,
                        s.cover_path,
                        s.description,
                        s.updated_at,
                        MAX(d.downloaded_at) AS latest_downloaded_at,
                        (
                            SELECT d2.title
                            FROM downloads d2
                            JOIN download_series ds2 ON ds2.download_id = d2.id
                            WHERE ds2.series_source = ds.series_source AND ds2.series_key = ds.series_key
                            ORDER BY COALESCE(ds2.content_order, 9223372036854775807) ASC,
                                     COALESCE(d2.source_created_at, d2.downloaded_at) ASC,
                                     d2.id ASC
                            LIMIT 1
                        ) AS sample_title,
                        NULL AS icon_path,
                        s.cover_path AS banner_path
                     FROM download_series ds
                     LEFT JOIN series s ON s.source = ds.series_source AND s.source_key = ds.series_key
                     JOIN downloads d ON d.id = ds.download_id
                     GROUP BY ds.series_source, ds.series_key, COALESCE(s.title, ds.title), s.cover_path, s.description, s.updated_at
                     {having}
                     ORDER BY count DESC, title ASC
                     LIMIT ?{limit_index} OFFSET ?{offset_index}",
                    having = having,
                    limit_index = if has_filter { 2 } else { 1 },
                    offset_index = if has_filter { 3 } else { 2 },
                )
            }
            other => return Err(format!("Unsupported entity facet kind: {}", other)),
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Entity facet search prepare failed: {}", e))?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(EntityFacet {
                source: row.get(0)?,
                source_key: row.get(1)?,
                display_name: row.get(2)?,
                count: row.get(3)?,
                cover_path: row.get(4)?,
                description: row.get(5)?,
                updated_at: row.get(6)?,
                latest_downloaded_at: row.get(7)?,
                sample_title: row.get(8)?,
                icon_path: row.get(9)?,
                banner_path: row.get(10)?,
            })
        };
        let rows = if has_filter {
            stmt.query_map(rusqlite::params![like, limit, offset], map_row)
        } else {
            stmt.query_map(rusqlite::params![limit, offset], map_row)
        }
        .map_err(|e| format!("Entity facet search failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Entity facet row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_filter_facets(&self) -> Result<FilterFacets, String> {
        self.get_filter_facets_with(true)
    }

    /// `include_entities` を落とすと、作者・シリーズの重い集計を省略する。
    ///
    /// ライブラリの絞り込みUIはタグと種別しか使わないのに、開くたびに相関
    /// サブクエリを含む集計が2本走っていた。大規模ライブラリでは、この2本が
    /// 画面表示の待ち時間の大半を占める。
    pub fn get_filter_facets_with(&self, include_entities: bool) -> Result<FilterFacets, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let collect = |sql: &str| -> Result<Vec<FacetCount>, String> {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("Facet query prepare failed: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(FacetCount {
                        name: row.get(0)?,
                        count: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Facet query failed: {}", e))?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(|e| format!("Facet row read failed: {}", e))?);
            }
            Ok(results)
        };

        let collect_entities = |sql: &str| -> Result<Vec<EntityFacet>, String> {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("Entity facet query prepare failed: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(EntityFacet {
                        source: row.get(0)?,
                        source_key: row.get(1)?,
                        display_name: row.get(2)?,
                        count: row.get(3)?,
                        cover_path: row.get(4)?,
                        description: row.get(5)?,
                        updated_at: row.get(6)?,
                        latest_downloaded_at: row.get(7)?,
                        sample_title: row.get(8)?,
                        icon_path: row.get(9)?,
                        banner_path: row.get(10)?,
                    })
                })
                .map_err(|e| format!("Entity facet query failed: {}", e))?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(|e| format!("Entity facet row read failed: {}", e))?);
            }
            Ok(results)
        };

        Ok(FilterFacets {
            tags: collect(
                "SELECT t.name, COUNT(dt.download_id) AS count
                 FROM tags t
                 JOIN download_tags dt ON dt.tag_id = t.id
                 GROUP BY t.id, t.name
                 ORDER BY count DESC, t.name ASC
                 LIMIT 500",
            )?,
            authors: collect(
                "SELECT author_name, COUNT(*) AS count
                 FROM downloads
                 WHERE author_name IS NOT NULL AND author_name != ''
                 GROUP BY author_name
                 ORDER BY count DESC, author_name ASC
                 LIMIT 500",
            )?,
            author_entities: if !include_entities {
                Vec::new()
            } else {
                collect_entities(
                "SELECT
                    d.source,
                    d.author_id,
                    COALESCE(p.display_name, d.author_name) AS display_name,
                    COUNT(DISTINCT d.id) AS count,
                    COALESCE(p.icon_path, p.cover_path) AS cover_path,
                    p.description,
                    p.updated_at,
                    MAX(d.downloaded_at) AS latest_downloaded_at,
                    (
                        SELECT d2.title
                        FROM downloads d2
                        WHERE d2.source = d.source AND d2.author_id = d.author_id
                        ORDER BY COALESCE(d2.source_created_at, d2.downloaded_at) DESC, d2.id DESC
                        LIMIT 1
                    ) AS sample_title,
                    p.icon_path,
                    p.cover_path AS banner_path
                 FROM downloads d
                 LEFT JOIN people p ON p.source = d.source AND p.source_key = d.author_id
                 WHERE d.author_id IS NOT NULL AND d.author_id != ''
                   AND d.author_name IS NOT NULL AND d.author_name != ''
                 GROUP BY d.source, d.author_id, COALESCE(p.display_name, d.author_name), p.icon_path, p.cover_path, p.description, p.updated_at
                 ORDER BY count DESC, display_name ASC
                 LIMIT 60",
            )?
            },
            series: if !include_entities {
                Vec::new()
            } else {
                collect_entities(
                "SELECT
                    ds.series_source,
                    ds.series_key,
                    COALESCE(s.title, ds.title) AS title,
                    COUNT(DISTINCT ds.download_id) AS count,
                    s.cover_path,
                    s.description,
                    s.updated_at,
                    MAX(d.downloaded_at) AS latest_downloaded_at,
                    (
                        SELECT d2.title
                        FROM downloads d2
                        JOIN download_series ds2 ON ds2.download_id = d2.id
                        WHERE ds2.series_source = ds.series_source AND ds2.series_key = ds.series_key
                        ORDER BY COALESCE(ds2.content_order, 9223372036854775807) ASC,
                                 COALESCE(d2.source_created_at, d2.downloaded_at) ASC,
                                 d2.id ASC
                        LIMIT 1
                    ) AS sample_title,
                    NULL AS icon_path,
                    s.cover_path AS banner_path
                 FROM download_series ds
                 LEFT JOIN series s ON s.source = ds.series_source AND s.source_key = ds.series_key
                 JOIN downloads d ON d.id = ds.download_id
                 GROUP BY ds.series_source, ds.series_key, COALESCE(s.title, ds.title), s.cover_path, s.description, s.updated_at
                 ORDER BY count DESC, title ASC
                 LIMIT 60",
            )?
            },
            content_types: collect(
                "SELECT content_type, COUNT(*) AS count
                 FROM downloads
                 WHERE content_type IS NOT NULL AND content_type != ''
                 GROUP BY content_type
                 ORDER BY count DESC, content_type ASC",
            )?,
            asset_types: collect(
                "SELECT asset_type, COUNT(*) AS count
                 FROM assets
                 WHERE asset_type IS NOT NULL AND asset_type != ''
                 GROUP BY asset_type
                 ORDER BY count DESC, asset_type ASC",
            )?,
        })
    }

    /// バージョン履歴を挿入
    pub fn insert_version(&self, ver: &NewVersion) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO download_versions (
                download_id, version, content_hash, text_length, json_path,
                original_json_path, asset_count, file_size_bytes, created_at, change_summary
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                ver.download_id,
                ver.version,
                ver.content_hash,
                ver.text_length,
                ver.json_path,
                ver.original_json_path,
                ver.asset_count,
                ver.file_size_bytes,
                ver.created_at,
                ver.change_summary,
            ],
        )
        .map_err(|e| format!("Insert version failed: {}", e))?;
        Ok(conn.last_insert_rowid())
    }

    /// ダウンロードの全バージョン履歴取得
    pub fn get_versions(&self, download_id: i64) -> Result<Vec<DownloadVersion>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM download_versions WHERE download_id = ?1 ORDER BY version DESC")
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadVersion {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    version: row.get(2)?,
                    content_hash: row.get(3)?,
                    text_length: row.get(4)?,
                    json_path: row.get(5)?,
                    original_json_path: row.get(6)?,
                    asset_count: row.get(7)?,
                    file_size_bytes: row.get(8)?,
                    created_at: row.get(9)?,
                    change_summary: row.get(10)?,
                })
            })
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    /// 特定のバージョン履歴取得
    pub fn get_version(&self, download_id: i64, version: i64) -> Result<DownloadVersion, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM download_versions WHERE download_id = ?1 AND version = ?2",
            params![download_id, version],
            |row| {
                Ok(DownloadVersion {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    version: row.get(2)?,
                    content_hash: row.get(3)?,
                    text_length: row.get(4)?,
                    json_path: row.get(5)?,
                    original_json_path: row.get(6)?,
                    asset_count: row.get(7)?,
                    file_size_bytes: row.get(8)?,
                    created_at: row.get(9)?,
                    change_summary: row.get(10)?,
                })
            },
        )
        .map_err(|e| format!("Version not found: {}", e))
    }

    /// 指定バージョンを削除する。最新バージョンを削除した場合は、直前のバージョンを現行に戻す。
    pub fn delete_version(&self, download_id: i64, version: i64) -> Result<(), String> {
        let (version_dir, source_dir_to_cleanup) = {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction begin failed: {}", e))?;

            let version_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM download_versions WHERE download_id = ?1",
                    params![download_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to count versions: {}", e))?;
            if version_count <= 1 {
                return Err(
                    "最後のバージョンは削除できません。作品全体の削除を使用してください。"
                        .to_string(),
                );
            }

            let current_version: i64 = tx
                .query_row(
                    "SELECT current_version FROM downloads WHERE id = ?1",
                    params![download_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Download not found: {}", e))?;

            let target: DownloadVersion = tx
                .query_row(
                    "SELECT * FROM download_versions WHERE download_id = ?1 AND version = ?2",
                    params![download_id, version],
                    |row| {
                        Ok(DownloadVersion {
                            id: row.get(0)?,
                            download_id: row.get(1)?,
                            version: row.get(2)?,
                            content_hash: row.get(3)?,
                            text_length: row.get(4)?,
                            json_path: row.get(5)?,
                            original_json_path: row.get(6)?,
                            asset_count: row.get(7)?,
                            file_size_bytes: row.get(8)?,
                            created_at: row.get(9)?,
                            change_summary: row.get(10)?,
                        })
                    },
                )
                .map_err(|e| format!("Version not found: {}", e))?;

            let replacement = if version == current_version {
                Some(
                    tx.query_row(
                        "SELECT * FROM download_versions
                         WHERE download_id = ?1 AND version != ?2
                         ORDER BY version DESC
                         LIMIT 1",
                        params![download_id, version],
                        |row| {
                            Ok(DownloadVersion {
                                id: row.get(0)?,
                                download_id: row.get(1)?,
                                version: row.get(2)?,
                                content_hash: row.get(3)?,
                                text_length: row.get(4)?,
                                json_path: row.get(5)?,
                                original_json_path: row.get(6)?,
                                asset_count: row.get(7)?,
                                file_size_bytes: row.get(8)?,
                                created_at: row.get(9)?,
                                change_summary: row.get(10)?,
                            })
                        },
                    )
                    .map_err(|e| format!("Replacement version not found: {}", e))?,
                )
            } else {
                None
            };

            let version_dir = Path::new(&target.json_path)
                .parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| "Version directory could not be resolved".to_string())?;
            let deleted_prefix = format!("{}%", version_dir.to_string_lossy());

            tx.execute(
                "DELETE FROM assets WHERE download_id = ?1 AND local_path LIKE ?2",
                params![download_id, deleted_prefix],
            )
            .map_err(|e| format!("Failed to delete version assets: {}", e))?;

            tx.execute(
                "DELETE FROM download_versions WHERE download_id = ?1 AND version = ?2",
                params![download_id, version],
            )
            .map_err(|e| format!("Failed to delete version: {}", e))?;

            if let Some(repl) = replacement {
                let repl_dir = Path::new(&repl.json_path)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .ok_or_else(|| {
                        "Replacement version directory could not be resolved".to_string()
                    })?;
                let repl_prefix = format!("{}%", repl_dir.to_string_lossy());
                let cover_path: Option<String> = tx
                    .query_row(
                        "SELECT local_path FROM assets
                         WHERE download_id = ?1 AND local_path LIKE ?2 AND mime_type LIKE 'image/%'
                         ORDER BY id ASC LIMIT 1",
                        params![download_id, repl_prefix],
                        |row| row.get(0),
                    )
                    .ok();

                tx.execute(
                    "UPDATE downloads SET
                        json_path = ?1,
                        original_json_path = ?2,
                        cover_path = ?3,
                        asset_count = ?4,
                        file_size_bytes = ?5,
                        downloaded_at = ?6,
                        content_hash = ?7,
                        text_length = ?8,
                        current_version = ?9
                     WHERE id = ?10",
                    params![
                        repl.json_path,
                        repl.original_json_path,
                        cover_path,
                        repl.asset_count,
                        repl.file_size_bytes,
                        repl.created_at,
                        repl.content_hash,
                        repl.text_length,
                        repl.version,
                        download_id,
                    ],
                )
                .map_err(|e| format!("Failed to promote replacement version: {}", e))?;
            }

            tx.commit()
                .map_err(|e| format!("Transaction commit failed: {}", e))?;

            let source_dir_to_cleanup = version_dir
                .parent()
                .and_then(|work_dir| work_dir.parent())
                .map(|p| p.to_path_buf());
            (version_dir, source_dir_to_cleanup)
        };

        if version_dir.exists() {
            let canon_storage = self
                .storage_dir
                .canonicalize()
                .map_err(|e| format!("Storage path resolution failed: {}", e))?;
            let canon_version_dir = version_dir
                .canonicalize()
                .map_err(|e| format!("Version path resolution failed: {}", e))?;
            if !canon_version_dir.starts_with(&canon_storage) {
                return Err(
                    "Access Denied: Version path is outside of storage directory".to_string(),
                );
            }
            if let Err(e) = remove_dir_all_resilient(&canon_version_dir) {
                log::warn!(
                    "Failed to remove version directory {:?}: {}",
                    canon_version_dir,
                    e
                );
            }
        }

        if let Some(source_dir) = source_dir_to_cleanup {
            let _ = remove_dir_resilient(&source_dir);
        }

        if let Err(e) = self.reindex_download(download_id) {
            log::warn!(
                "Failed to refresh search index after deleting version {} of download {}: {}",
                version,
                download_id,
                e
            );
        }

        Ok(())
    }

    /// 更新監視（トグル）を設定する
    pub fn set_watch_updates(&self, download_id: i64, watch: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET watch_updates = ?1 WHERE id = ?2",
            params![if watch { 1i64 } else { 0i64 }, download_id],
        )
        .map_err(|e| format!("Failed to set watch_updates: {}", e))?;
        if let Ok((source, source_id, title)) = conn.query_row(
            "SELECT source, source_id, title FROM downloads WHERE id = ?1",
            params![download_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        ) {
            conn.execute(
                "INSERT INTO update_targets (
                    target_type, source, source_key, display_name, enabled, created_at, updated_at
                ) VALUES ('work', ?1, ?2, ?3, ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                    display_name = excluded.display_name,
                    enabled = excluded.enabled,
                    updated_at = CURRENT_TIMESTAMP",
                params![source, source_id, title, if watch { 1i64 } else { 0i64 }],
            )
            .map_err(|e| format!("Failed to sync work update target: {}", e))?;
        }
        Ok(())
    }

    pub fn set_watch_updates_for_search(
        &self,
        params: &SearchV2Params,
        watch: bool,
    ) -> Result<BulkMutationResult, String> {
        let mut bulk_params = params.clone();
        bulk_params.cursor = None;
        bulk_params.limit = Some(200_000);
        bulk_params.projection = Some("bulk".to_string());
        let result = self.search_downloads_v2_inner(&bulk_params, 200_000)?;
        let ids = result.items.iter().map(|item| item.id).collect::<Vec<_>>();
        let changed_count = self.set_watch_updates_for_ids(&ids, watch)?;

        Ok(BulkMutationResult {
            matched_count: ids.len() as i64,
            changed_count,
        })
    }

    pub fn delete_downloads_for_search(
        &self,
        params: &SearchV2Params,
    ) -> Result<BulkMutationResult, String> {
        let mut bulk_params = params.clone();
        bulk_params.cursor = None;
        bulk_params.limit = Some(200_000);
        bulk_params.projection = Some("bulk".to_string());
        let result = self.search_downloads_v2_inner(&bulk_params, 200_000)?;
        let ids = result.items.iter().map(|item| item.id).collect::<Vec<_>>();
        self.delete_downloads(&ids)
    }

    fn set_watch_updates_for_ids(&self, ids: &[i64], watch: bool) -> Result<i64, String> {
        if ids.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Bulk watch update transaction failed: {}", e))?;
        let mut changed_count = 0i64;
        let watch_value = if watch { 1i64 } else { 0i64 };

        for id in ids {
            let updated = tx
                .execute(
                    "UPDATE downloads SET watch_updates = ?1 WHERE id = ?2",
                    params![watch_value, id],
                )
                .map_err(|e| format!("Failed to set watch_updates: {}", e))?;
            if updated == 0 {
                continue;
            }
            changed_count += updated as i64;

            if let Ok((source, source_id, title)) = tx.query_row(
                "SELECT source, source_id, title FROM downloads WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            ) {
                tx.execute(
                    "INSERT INTO update_targets (
                        target_type, source, source_key, display_name, enabled, created_at, updated_at
                    ) VALUES ('work', ?1, ?2, ?3, ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                        display_name = excluded.display_name,
                        enabled = excluded.enabled,
                        updated_at = CURRENT_TIMESTAMP",
                    params![source, source_id, title, watch_value],
                )
                .map_err(|e| format!("Failed to sync work update target: {}", e))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Bulk watch update commit failed: {}", e))?;
        Ok(changed_count)
    }

    /// お気に入り（トグル）を設定する
    pub fn set_favorite(&self, download_id: i64, favorite: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET favorite = ?1 WHERE id = ?2",
            params![if favorite { 1i64 } else { 0i64 }, download_id],
        )
        .map_err(|e| format!("Failed to set favorite: {}", e))?;
        Ok(())
    }

    /// 監視対象（watch_updates = 1）のダウンロード作品一覧を取得
    pub fn get_watched_downloads(&self) -> Result<Vec<DownloadEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sql = format!(
            "{} WHERE d.watch_updates = 1 ORDER BY d.downloaded_at DESC",
            download_select_sql_for_projection(Some("libraryCompact"), "NULL", "NULL")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], download_entry_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    /// ソースとソースIDからダウンロードエントリを取得（存在しない場合はOk(None)）
    pub fn get_download_by_source(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<DownloadEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sql = format!(
            "{} WHERE d.source = ?1 AND d.source_id = ?2",
            download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;

        let mut rows = stmt
            .query_map(params![source, source_id], download_entry_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;

        if let Some(row) = rows.next() {
            let entry = row.map_err(|e| format!("Row read failed: {}", e))?;
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    pub fn reconstruct_entities_after_import(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "
            DELETE FROM download_people;
            DELETE FROM download_series;
            DELETE FROM people;
            DELETE FROM series;

            INSERT OR IGNORE INTO people (
                source, source_key, display_name, current_version, created_at, updated_at
            )
            SELECT DISTINCT source, author_id, author_name, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
            FROM downloads
            WHERE author_id IS NOT NULL AND author_id != '';

            INSERT OR IGNORE INTO download_people (
                download_id, person_source, person_key, role, display_name
            )
            SELECT id, source, author_id, CASE WHEN source = 'fanbox' THEN 'creator' ELSE 'author' END, author_name
            FROM downloads
            WHERE author_id IS NOT NULL AND author_id != '';

            INSERT OR IGNORE INTO series (
                source, source_key, title, current_version, created_at, updated_at
            )
            SELECT DISTINCT source, relation_id, relation_name, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
            FROM download_relations
            WHERE relation_type = 'series' AND relation_id IS NOT NULL AND relation_id != '';

            INSERT OR IGNORE INTO download_series (
                download_id, series_source, series_key, title
            )
            SELECT download_id, source, relation_id, relation_name
            FROM download_relations
            WHERE relation_type = 'series' AND relation_id IS NOT NULL AND relation_id != '';
            "
        ).map_err(|e| format!("Reconstructing entities failed: {}", e))?;

        Ok(())
    }
}

fn query_text(params: &SearchV2Params) -> &str {
    params
        .query
        .as_deref()
        .or(params.text.as_deref())
        .unwrap_or("")
        .trim()
}

fn plain_entity_query(query: &str) -> bool {
    !query.is_empty()
        && query.chars().count() <= 160
        && !query.contains(':')
        && !query.contains('"')
        && !query.starts_with('-')
        && !query.contains(" -")
}

fn search_explanations(
    exact_entity: Option<&SearchEntityIntent>,
    engine: &str,
    index_complete: bool,
) -> Vec<String> {
    let mut explanations = Vec::with_capacity(3);
    if let Some(entity) = exact_entity {
        explanations.push(format!(
            "{}「{}」への完全一致として解釈し、関係する作品だけに限定しました",
            if entity.kind == "series" {
                "シリーズ"
            } else {
                "作者"
            },
            entity.label
        ));
    }
    explanations.push(match engine {
        "semantic" => "端末内の意味検索インデックスで関連度を計算しました".to_string(),
        "tantivy" => {
            "端末内の全文検索インデックスでタイトル・作者・タグ・シリーズ・本文を照合しました"
                .to_string()
        }
        _ => "保存済みメタデータをSQLiteで厳密に絞り込みました".to_string(),
    });
    if !index_complete && engine != "sqlite-metadata" {
        explanations.push(
            "全文検索インデックスの構築中のため、未索引の作品は結果に含まれない場合があります"
                .to_string(),
        );
    }
    explanations
}

fn effective_search_mode(params: &SearchV2Params) -> String {
    match params.search_mode.as_deref() {
        Some("exact") => "exact",
        Some("semantic") => "semantic",
        _ => "smart",
    }
    .to_string()
}

fn search_candidate_limit(params: &SearchV2Params, page_limit: i64) -> usize {
    let page_limit = page_limit.max(1) as usize;
    let (multiplier, floor) = if params_have_library_filters(params) {
        (4usize, 400usize)
    } else {
        (2usize, 160usize)
    };
    page_limit.saturating_mul(multiplier).max(floor)
}

fn params_have_library_filters(params: &SearchV2Params) -> bool {
    params
        .source
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || params
            .content_type
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params.favorite == Some(true)
        || params
            .tags_include
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params
            .tags_exclude
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params
            .authors_include
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params
            .authors_exclude
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params.min_char_count.is_some()
        || params.max_char_count.is_some()
        || params
            .asset_filter
            .as_deref()
            .map(|value| !value.trim().is_empty() && value != "all")
            .unwrap_or(false)
        || params
            .watch_filter
            .as_deref()
            .map(|value| !value.trim().is_empty() && value != "all")
            .unwrap_or(false)
        || params
            .person_source
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params
            .person_key
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params
            .series_source
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params
            .series_key
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
}

fn blend_search_hits(
    lexical_hits: &[super::tantivy_index::TantivySearchHit],
    semantic_hits: &[super::semantic_index::SemanticSearchHit],
    search_mode: &str,
) -> Vec<RankedSearchHit> {
    let mut merged: HashMap<i64, RankedSearchHit> = HashMap::new();
    if search_mode != "semantic" {
        for (idx, hit) in lexical_hits.iter().enumerate() {
            let rrf = 1.0 / (60.0 + (idx + 1) as f64);
            let score = (hit.score as f64).min(80.0) + rrf * 900.0;
            let entry = merged.entry(hit.download_id).or_insert(RankedSearchHit {
                download_id: hit.download_id,
                score: 0.0,
                semantic: None,
                document: Some(hit.document.clone()),
            });
            if entry.document.is_none() {
                entry.document = Some(hit.document.clone());
            }
            entry.score += score;
        }
    }
    if search_mode != "exact" {
        for (idx, hit) in semantic_hits.iter().enumerate() {
            let rrf = 1.0 / (60.0 + (idx + 1) as f64);
            let score = hit.score.max(0.0) * 120.0 + rrf * 900.0;
            let entry = merged.entry(hit.download_id).or_insert(RankedSearchHit {
                download_id: hit.download_id,
                score: 0.0,
                semantic: None,
                document: None,
            });
            entry.score += score;
            if entry
                .semantic
                .as_ref()
                .map(|existing| existing.score < hit.score)
                .unwrap_or(true)
            {
                entry.semantic = Some(hit.clone());
            }
        }
    }
    let mut hits = merged.into_values().collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.download_id.cmp(&b.download_id))
    });
    hits
}

fn normalize_search_params(params: &SearchV2Params) -> SearchV2Params {
    let mut normalized = params.clone();
    normalized.projection = Some(normalized_projection(
        normalized.projection.as_deref(),
        normalized.view_mode.as_deref(),
    ));
    let query = query_text(&normalized).to_string();
    if query.is_empty() {
        return normalized;
    }

    let (clean_query, source_id_terms) = extract_structured_search_filters(&query, &mut normalized);
    let mut query_parts = Vec::new();
    if !clean_query.trim().is_empty() {
        query_parts.push(clean_query.trim().to_string());
    }
    query_parts.extend(source_id_terms);
    let next_query = query_parts.join(" ");
    normalized.query = if next_query.is_empty() {
        None
    } else {
        Some(next_query)
    };
    normalized.text = None;
    normalized
}

fn normalized_projection(projection: Option<&str>, view_mode: Option<&str>) -> String {
    match projection {
        Some("libraryGallery") | Some("library") => "libraryGallery".to_string(),
        Some("libraryCompact") => "libraryCompact".to_string(),
        Some("bulk") => "bulk".to_string(),
        Some("entityFacet") => "entityFacet".to_string(),
        Some("minimal") => "bulk".to_string(),
        _ => match view_mode {
            Some("compact") | Some("epubSelection") | Some("updateReview") => {
                "libraryCompact".to_string()
            }
            _ => "libraryGallery".to_string(),
        },
    }
}

fn extract_structured_search_filters(
    query: &str,
    params: &mut SearchV2Params,
) -> (String, Vec<String>) {
    let mut clean_terms = Vec::new();
    let mut source_id_terms = Vec::new();

    for raw_term in split_query_terms(query) {
        let excluded = raw_term.starts_with('-');
        let term = raw_term.trim_start_matches('-');
        let Some((field, raw_value)) = term.split_once(':') else {
            if let Some(id) = source_id_from_url(term) {
                source_id_terms.push(id);
            } else {
                clean_terms.push(raw_term);
            }
            continue;
        };

        let field = field.to_ascii_lowercase();
        let value = raw_value.trim_matches('"').trim();
        if value.is_empty() {
            continue;
        }

        match field.as_str() {
            "tag" | "tags" => {
                if excluded {
                    push_unique(&mut params.tags_exclude, value.to_string());
                } else {
                    push_unique(&mut params.tags_include, value.to_string());
                }
            }
            "author" | "creator" => {
                if excluded {
                    push_unique(&mut params.authors_exclude, value.to_string());
                } else {
                    push_unique(&mut params.authors_include, value.to_string());
                }
            }
            "series" | "series_key" | "series_id" if !excluded => {
                if let Some((source, key)) = value.split_once(':') {
                    let source = source.trim();
                    let key = key.trim();
                    if !source.is_empty() && !key.is_empty() {
                        params.series_source = Some(source.to_ascii_lowercase());
                        params.series_key = Some(key.to_string());
                    }
                } else {
                    params.series_source = None;
                    params.series_key = Some(value.to_string());
                }
            }
            // Compatibility for values emitted by the pre-0.3 suggestion UI.
            // It used `pixiv:<series-id>` as the visible query even though the
            // token identifies a series relation, not searchable prose.
            "pixiv" | "fanbox" if !excluded => {
                params.series_source = Some(field);
                params.series_key = Some(value.to_string());
            }
            "series_title" | "title" => {
                clean_terms.push(if excluded {
                    format!("-{}", value)
                } else {
                    value.to_string()
                });
            }
            "source" if !excluded => {
                params.source = Some(value.to_ascii_lowercase());
            }
            "id" | "source_id" | "sourceid" | "url" if !excluded => {
                if let Some(id) = source_id_from_url(value) {
                    source_id_terms.push(id);
                } else {
                    source_id_terms.push(value.to_string());
                }
            }
            _ => clean_terms.push(raw_term),
        }
    }

    (clean_terms.join(" "), source_id_terms)
}

fn split_query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut chars = query.chars().peekable();
    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut term = String::new();
        let mut in_quote = false;
        for c in chars.by_ref() {
            if c == '"' {
                in_quote = !in_quote;
                term.push(c);
                continue;
            }
            if !in_quote && c.is_whitespace() {
                break;
            }
            term.push(c);
        }
        if !term.trim().is_empty() {
            terms.push(term);
        }
    }
    terms
}

fn source_id_from_url(value: &str) -> Option<String> {
    let pixiv = regex::Regex::new(r"(?:novel/show\.php\?id=|/novels/|/artworks/)(\d+)").ok()?;
    if let Some(caps) = pixiv.captures(value) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    let fanbox = regex::Regex::new(r"fanbox\.cc/(?:@[^/]+/)?posts/(\d+)").ok()?;
    fanbox
        .captures(value)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
}

fn source_url_for_download(source: &str, source_id: &str, author_id: &str) -> String {
    match source {
        "pixiv" => format!("https://www.pixiv.net/novel/show.php?id={}", source_id),
        "fanbox" if !author_id.is_empty() => {
            format!("https://{}.fanbox.cc/posts/{}", author_id, source_id)
        }
        "fanbox" => format!("https://www.fanbox.cc/posts/{}", source_id),
        _ => source_id.to_string(),
    }
}

fn push_unique(slot: &mut Option<Vec<String>>, value: String) {
    let values = slot.get_or_insert_with(Vec::new);
    if !values.iter().any(|item| item == &value) {
        values.push(value);
    }
}

fn encode_cursor(cursor: &SearchCursor) -> Option<String> {
    serde_json::to_vec(cursor)
        .ok()
        .map(|bytes| format!("k:{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn decode_cursor(raw: Option<&str>) -> Option<SearchCursor> {
    let raw = raw?;
    let encoded = raw.strip_prefix("k:")?;
    let bytes = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn normalized_sort_key(params: &SearchV2Params) -> &'static str {
    // Accept both the original UI names and the database-shaped names used by
    // newer screens. The result is a closed internal enum represented as a
    // string, so user input is never interpolated into SQL.
    match params.sort_by.as_deref() {
        Some("title") => "title",
        Some("author" | "author_name") => "author",
        Some("date" | "downloaded_at") => "date",
        Some("published" | "source_created_at") => "published",
        Some("updated" | "source_updated_at") => "updated",
        Some("series_order") => "series_order",
        Some("size" | "file_size_bytes") => "size",
        Some("length" | "text_length") => "length",
        Some("relevance" | "score") => "relevance",
        _ => "date",
    }
}

/// Whether a text search should be ordered by a library column instead of by
/// relevance. Relevance stays the default, so a caller that does not ask for an
/// ordering keeps the behaviour it had.
fn wants_column_sort(params: &SearchV2Params) -> bool {
    params.sort_by.is_some() && normalized_sort_key(params) != "relevance"
}

fn sort_label(params: &SearchV2Params) -> &'static str {
    let descending = effective_sort_order(params).as_deref() != Some("asc");
    match normalized_sort_key(params) {
        "title" => "タイトル順",
        "author" => "作者名順",
        "published" => {
            if descending {
                "公開日の新しい順"
            } else {
                "公開日の古い順"
            }
        }
        "updated" => "更新日順",
        "size" => {
            if descending {
                "容量の大きい順"
            } else {
                "容量の小さい順"
            }
        }
        "length" => {
            if descending {
                "文字数の多い順"
            } else {
                "文字数の少ない順"
            }
        }
        "series_order" => "シリーズ順",
        _ => {
            if descending {
                "保存日の新しい順"
            } else {
                "保存日の古い順"
            }
        }
    }
}

fn effective_sort_by(params: &SearchV2Params) -> Option<String> {
    Some(normalized_sort_key(params).to_string())
}

fn effective_sort_order(params: &SearchV2Params) -> Option<String> {
    Some(
        match params.sort_order.as_deref() {
            Some("asc") => "asc",
            _ => "desc",
        }
        .to_string(),
    )
}

fn encode_sql_cursor(params: &SearchV2Params, item: &DownloadEntry) -> Option<String> {
    encode_cursor(&SearchCursor {
        kind: "sql".to_string(),
        scope: None,
        sort_by: effective_sort_by(params),
        sort_order: effective_sort_order(params),
        value: item
            .sort_key
            .clone()
            .or_else(|| fallback_sort_value(params, item)),
        id: Some(item.id),
        score: None,
        downloaded_at: None,
        tantivy_score: None,
        tantivy_segment_ord: None,
        tantivy_doc_id: None,
    })
}

fn encode_search_cursor(params: &SearchV2Params, item: &DownloadEntry) -> Option<String> {
    encode_cursor(&SearchCursor {
        kind: "search".to_string(),
        scope: Some(search_cursor_scope(params)),
        sort_by: Some("relevance".to_string()),
        sort_order: Some("desc".to_string()),
        value: None,
        id: Some(item.id),
        score: item.search_score,
        downloaded_at: Some(item.downloaded_at.clone()),
        tantivy_score: None,
        tantivy_segment_ord: None,
        tantivy_doc_id: None,
    })
}

fn encode_tantivy_search_cursor(
    params: &SearchV2Params,
    item: &DownloadEntry,
    cursor: super::tantivy_index::TantivySearchCursor,
) -> Option<String> {
    encode_cursor(&SearchCursor {
        kind: "tantivy-search-after".to_string(),
        scope: Some(search_cursor_scope(params)),
        sort_by: Some("relevance".to_string()),
        sort_order: Some("desc".to_string()),
        value: None,
        id: Some(item.id),
        score: item.search_score,
        downloaded_at: None,
        tantivy_score: Some(cursor.score),
        tantivy_segment_ord: Some(cursor.segment_ord),
        tantivy_doc_id: Some(cursor.doc_id),
    })
}

fn tantivy_cursor(cursor: &SearchCursor) -> Option<super::tantivy_index::TantivySearchCursor> {
    Some(super::tantivy_index::TantivySearchCursor {
        score: cursor.tantivy_score?,
        segment_ord: cursor.tantivy_segment_ord?,
        doc_id: cursor.tantivy_doc_id?,
    })
}

fn search_cursor_scope(params: &SearchV2Params) -> String {
    let mut scoped = params.clone();
    scoped.cursor = None;
    scoped.limit = None;
    scoped.projection = None;
    scoped.view_mode = None;
    let mut hasher = Sha256::new();
    hasher.update(format!("{scoped:?}").as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn search_item_is_after_cursor(item: &DownloadEntry, cursor: &SearchCursor) -> bool {
    let cursor_score = cursor.score.unwrap_or(0.0);
    let item_score = item.search_score.unwrap_or(0.0);
    if item_score < cursor_score {
        return true;
    }
    if item_score > cursor_score {
        return false;
    }
    let cursor_date = cursor.downloaded_at.as_deref().unwrap_or("");
    if item.downloaded_at.as_str() < cursor_date {
        return true;
    }
    if item.downloaded_at.as_str() > cursor_date {
        return false;
    }
    item.id < cursor.id.unwrap_or(i64::MIN)
}

fn fallback_sort_value(params: &SearchV2Params, item: &DownloadEntry) -> Option<String> {
    Some(match effective_sort_by(params).as_deref() {
        Some("title") => item.title.clone(),
        Some("author") => item.author_name.clone(),
        Some("published") => item
            .source_created_at
            .clone()
            .unwrap_or_else(|| item.downloaded_at.clone()),
        Some("updated") => item
            .source_updated_at
            .clone()
            .or_else(|| item.source_created_at.clone())
            .unwrap_or_else(|| item.downloaded_at.clone()),
        Some("size") => item.file_size_bytes.to_string(),
        Some("length") => item.text_length.to_string(),
        _ => item.downloaded_at.clone(),
    })
}

fn download_select_sql_for_projection(
    projection: Option<&str>,
    search_score_expr: &str,
    sort_key_expr: &str,
) -> String {
    let projection = normalized_projection(projection, None);
    let tags_expr = "COALESCE((
        SELECT json_group_array(name)
        FROM (
            SELECT t.name AS name
            FROM download_tags dt
            JOIN tags t ON t.id = dt.tag_id
            WHERE dt.download_id = d.id
            ORDER BY t.name
        )
    ), '[]')";
    let person_id_expr =
        "(SELECT dp.person_key FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1)";
    let person_name_expr =
        "(SELECT dp.display_name FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1)";
    let series_id_expr =
        "(SELECT ds.series_key FROM download_series ds WHERE ds.download_id = d.id LIMIT 1)";
    let series_title_expr =
        "(SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1)";
    // Cards show the creator's avatar beside the author name. Keyed on
    // author_id, the same join the author listing uses.
    let person_icon_expr =
        "(SELECT p.icon_path FROM people p WHERE p.source = d.source AND p.source_key = d.author_id)";

    // Only the card projections need the avatar; bulk reads skip the lookup.
    let person_icon = match projection.as_str() {
        "libraryGallery" | "libraryCompact" => person_icon_expr,
        _ => "NULL",
    };
    let (core_columns, person_id, person_name, series_id, series_title) = match projection.as_str()
    {
        "bulk" => (
            "d.id,
                d.source,
                d.source_id,
                d.title,
                '' AS author_name,
                '' AS author_id,
                d.content_type,
                '[]' AS tags,
                NULL AS excerpt,
                NULL AS cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
                .to_string(),
            "NULL",
            "NULL",
            "NULL",
            "NULL",
        ),
        "entityFacet" => (
            "d.id,
                d.source,
                d.source_id,
                d.title,
                d.author_name,
                d.author_id,
                d.content_type,
                '[]' AS tags,
                NULL AS excerpt,
                d.cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
                .to_string(),
            "NULL",
            "NULL",
            "NULL",
            "NULL",
        ),
        // The list view shows tags too; only the excerpt is surplus there.
        "libraryCompact" => (
            format!(
                "d.id,
                d.source,
                d.source_id,
                d.title,
                d.author_name,
                d.author_id,
                d.content_type,
                {tags_expr} AS tags,
                NULL AS excerpt,
                d.cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
            ),
            person_id_expr,
            person_name_expr,
            series_id_expr,
            series_title_expr,
        ),
        _ => (
            format!(
                "d.id,
                d.source,
                d.source_id,
                d.title,
                d.author_name,
                d.author_id,
                d.content_type,
                {tags_expr} AS tags,
                d.excerpt,
                d.cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
            ),
            person_id_expr,
            person_name_expr,
            series_id_expr,
            series_title_expr,
        ),
    };
    format!(
        "SELECT {core_columns},
            {} AS person_id,
            {} AS person_name,
            {} AS series_id,
            {} AS series_title,
            {} AS search_score,
            NULL AS match_fields,
            NULL AS score_reasons,
            NULL AS match_highlights,
            {} AS sort_key,
            {} AS person_icon_path
         FROM downloads d",
        person_id,
        person_name,
        series_id,
        series_title,
        search_score_expr,
        sort_key_expr,
        person_icon
    )
}

fn query_download_entries(
    conn: &Connection,
    sql: &str,
    bind_values: &[Box<dyn rusqlite::types::ToSql>],
) -> Result<Vec<DownloadEntry>, String> {
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Query prepare failed: {}\nSQL: {}", e, sql))?;
    let rows = stmt
        .query_map(refs.as_slice(), download_entry_from_row)
        .map_err(|e| format!("Query failed: {}", e))?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
    }
    Ok(results)
}

fn collect_suggestions(
    conn: &Connection,
    kind: &str,
    sql: &str,
    like: &str,
    limit: i64,
    items: &mut Vec<SearchSuggestion>,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Suggest query prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![like, limit], |row| {
            Ok(SearchSuggestion {
                kind: kind.to_string(),
                label: row.get(0)?,
                value: row.get(1)?,
                count: row.get(2)?,
                exact_match: false,
                source: row.get(3)?,
                source_key: row.get(4)?,
            })
        })
        .map_err(|e| format!("Suggest query failed: {}", e))?;
    for row in rows {
        items.push(row.map_err(|e| format!("Suggest row read failed: {}", e))?);
    }
    Ok(())
}

fn suggestion_kind_priority(kind: &str) -> u8 {
    match kind {
        "author" => 0,
        "series" => 1,
        "title" => 2,
        "tag" => 3,
        _ => 4,
    }
}

fn sort_key_select_expr(params: &SearchV2Params) -> String {
    match effective_sort_by(params).as_deref() {
        Some("title") => "CAST(d.title AS TEXT)".to_string(),
        Some("author") => "CAST(d.author_name AS TEXT)".to_string(),
        Some("published") => {
            "CAST(COALESCE(d.source_created_at, d.downloaded_at) AS TEXT)".to_string()
        }
        Some("updated") => {
            "CAST(COALESCE(d.source_updated_at, d.source_created_at, d.downloaded_at) AS TEXT)"
                .to_string()
        }
        Some("size") => "CAST(d.file_size_bytes AS TEXT)".to_string(),
        Some("length") => "CAST(d.text_length AS TEXT)".to_string(),
        Some("series_order") => format!("CAST({} AS TEXT)", series_order_sort_expr()),
        _ => "CAST(d.downloaded_at AS TEXT)".to_string(),
    }
}

fn sort_compare_expr(params: &SearchV2Params) -> String {
    match effective_sort_by(params).as_deref() {
        Some("title") => "d.title COLLATE NOCASE".to_string(),
        Some("author") => "d.author_name COLLATE NOCASE".to_string(),
        Some("published") => "COALESCE(d.source_created_at, d.downloaded_at)".to_string(),
        Some("updated") => {
            "COALESCE(d.source_updated_at, d.source_created_at, d.downloaded_at)".to_string()
        }
        Some("size") => "d.file_size_bytes".to_string(),
        Some("length") => "d.text_length".to_string(),
        Some("series_order") => series_order_sort_expr(),
        _ => "d.downloaded_at".to_string(),
    }
}

fn append_keyset_filter(
    params: &SearchV2Params,
    cursor: Option<&SearchCursor>,
    wheres: &mut Vec<String>,
    bind_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
) {
    let Some(cursor) = cursor else { return };
    let Some(value) = cursor.value.as_deref() else {
        return;
    };
    let Some(id) = cursor.id else { return };
    let expr = sort_compare_expr(params);
    let asc = effective_sort_order(params).as_deref() == Some("asc");
    let cmp = if asc { ">" } else { "<" };
    let id_cmp = if asc { ">" } else { "<" };
    wheres.push(format!(
        "({expr} {cmp} ? OR ({expr} = ? AND d.id {id_cmp} ?))"
    ));
    if matches!(
        effective_sort_by(params).as_deref(),
        Some("size" | "length")
    ) {
        let parsed = value.parse::<i64>().unwrap_or(0);
        bind_values.push(Box::new(parsed));
        bind_values.push(Box::new(parsed));
    } else {
        bind_values.push(Box::new(value.to_string()));
        bind_values.push(Box::new(value.to_string()));
    }
    bind_values.push(Box::new(id));
}

/// Caps and de-duplicates a client-supplied membership list.
///
/// These lists come from per-device state that nothing prunes, so they grow
/// and can contain works that were deleted long ago. The cap keeps one
/// malformed request from building a megabyte of SQL parameter.
const MAX_ID_FILTER: usize = 20_000;

const MAX_SAVED_SEARCHES: i64 = 100;
const MAX_SAVED_SEARCH_NAME: usize = 80;
const MAX_SAVED_SEARCH_QUERY: usize = 512;
const MAX_SAVED_SEARCH_PARAMS_BYTES: usize = 16 * 1024;

/// The row to start a numbered page at, when one was asked for.
///
/// Ignored without an explicit column ordering: relevance results are walked
/// with a score cursor, and an offset into them would silently mean something
/// different from the page the reader asked for.
fn page_offset(params: &SearchV2Params) -> Option<i64> {
    let offset = params.offset.unwrap_or(0);
    if offset <= 0 || normalized_sort_key(params) == "relevance" {
        return None;
    }
    Some(offset)
}

fn bounded_id_list(ids: &[i64]) -> Vec<i64> {
    let mut seen = std::collections::HashSet::with_capacity(ids.len().min(MAX_ID_FILTER));
    ids.iter()
        .copied()
        .filter(|id| *id > 0)
        .filter(|id| seen.insert(*id))
        .take(MAX_ID_FILTER)
        .collect()
}

fn append_library_filters(
    params: &SearchV2Params,
    wheres: &mut Vec<String>,
    bind_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
) {
    if let Some(ref src) = params.source {
        if !src.is_empty() {
            wheres.push("d.source = ?".to_string());
            bind_values.push(Box::new(src.clone()));
        }
    }

    if let Some(ref ct) = params.content_type {
        if !ct.is_empty() {
            wheres.push("d.content_type = ?".to_string());
            bind_values.push(Box::new(ct.clone()));
        }
    }

    if params.favorite == Some(true) {
        wheres.push("d.favorite = 1".to_string());
    }

    if let Some(ref ids) = params.ids_include {
        // An explicit, possibly large membership list travels as one JSON
        // parameter; a placeholder per id would run into SQLite's variable
        // limit on a well-read library. An empty list means "nothing", which
        // is different from "no filter" and must not quietly match everything.
        let bounded = bounded_id_list(ids);
        let encoded = serde_json::to_string(&bounded).unwrap_or_else(|_| "[]".to_string());
        wheres.push("d.id IN (SELECT value FROM json_each(?))".to_string());
        bind_values.push(Box::new(encoded));
    }

    if let Some(ref tags_inc) = params.tags_include {
        let active_tags = active_strings(tags_inc);
        if !active_tags.is_empty() {
            let placeholders = vec!["?"; active_tags.len()].join(", ");
            if params.tag_filter_mode.as_deref() == Some("or") {
                wheres.push(format!(
                    "d.id IN (
                        SELECT download_id FROM download_tags dt
                        JOIN tags t ON dt.tag_id = t.id
                        WHERE t.name IN ({})
                    )",
                    placeholders
                ));
                for tag in active_tags {
                    bind_values.push(Box::new(tag));
                }
            } else {
                wheres.push(format!(
                    "d.id IN (
                        SELECT download_id FROM download_tags dt
                        JOIN tags t ON dt.tag_id = t.id
                        WHERE t.name IN ({})
                        GROUP BY download_id
                        HAVING COUNT(DISTINCT t.name) = ?
                    )",
                    placeholders
                ));
                let count_inc = active_tags.len() as i64;
                for tag in active_tags {
                    bind_values.push(Box::new(tag));
                }
                bind_values.push(Box::new(count_inc));
            }
        }
    }

    if let Some(ref tags_exc) = params.tags_exclude {
        let active_tags = active_strings(tags_exc);
        if !active_tags.is_empty() {
            let placeholders = vec!["?"; active_tags.len()].join(", ");
            wheres.push(format!(
                "d.id NOT IN (
                    SELECT download_id FROM download_tags dt
                    JOIN tags t ON dt.tag_id = t.id
                    WHERE t.name IN ({})
                )",
                placeholders
            ));
            for tag in active_tags {
                bind_values.push(Box::new(tag));
            }
        }
    }

    if let Some(ref authors_inc) = params.authors_include {
        let active_authors = active_strings(authors_inc);
        if !active_authors.is_empty() {
            let placeholders = vec!["?"; active_authors.len()].join(", ");
            wheres.push(format!("d.author_name IN ({})", placeholders));
            for author in active_authors {
                bind_values.push(Box::new(author));
            }
        }
    }

    if let Some(ref authors_exc) = params.authors_exclude {
        let active_authors = active_strings(authors_exc);
        if !active_authors.is_empty() {
            let placeholders = vec!["?"; active_authors.len()].join(", ");
            wheres.push(format!("d.author_name NOT IN ({})", placeholders));
            for author in active_authors {
                bind_values.push(Box::new(author));
            }
        }
    }

    if let Some(min_char) = params.min_char_count {
        wheres.push("d.text_length >= ?".to_string());
        bind_values.push(Box::new(min_char));
    }

    if let Some(max_char) = params.max_char_count {
        wheres.push("d.text_length <= ?".to_string());
        bind_values.push(Box::new(max_char));
    }

    if let Some(ref asset_filter) = params.asset_filter {
        match asset_filter.as_str() {
            "has_assets" => wheres.push("d.asset_count > 0".to_string()),
            "no_assets" => wheres.push("d.asset_count = 0".to_string()),
            "has_images" => wheres.push(
                "d.id IN (
                    SELECT download_id FROM assets
                    WHERE mime_type LIKE 'image/%'
                )"
                .to_string(),
            ),
            "has_files" => wheres.push(
                "d.id IN (
                    SELECT download_id FROM assets
                    WHERE mime_type IS NULL OR mime_type NOT LIKE 'image/%'
                )"
                .to_string(),
            ),
            "has_images_and_files" => {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM assets
                        WHERE mime_type LIKE 'image/%'
                    )"
                    .to_string(),
                );
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM assets
                        WHERE mime_type IS NULL OR mime_type NOT LIKE 'image/%'
                    )"
                    .to_string(),
                );
            }
            _ => {}
        }
    }

    if let Some(ref watch_filter) = params.watch_filter {
        match watch_filter.as_str() {
            "watched" => wheres.push("d.watch_updates = 1".to_string()),
            "unwatched" => wheres.push("d.watch_updates = 0".to_string()),
            _ => {}
        }
    }

    if let (Some(person_source), Some(person_key)) = (&params.person_source, &params.person_key) {
        if !person_source.trim().is_empty() && !person_key.trim().is_empty() {
            wheres.push(
                "d.id IN (
                    SELECT download_id FROM download_people
                    WHERE person_source = ? AND person_key = ?
                )"
                .to_string(),
            );
            bind_values.push(Box::new(person_source.clone()));
            bind_values.push(Box::new(person_key.clone()));
        }
    }

    if let Some(series_key) = &params.series_key {
        let series_key = series_key.trim();
        if !series_key.is_empty() {
            if let Some(series_source) = params
                .series_source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM download_series
                        WHERE series_source = ? AND series_key = ?
                    )"
                    .to_string(),
                );
                bind_values.push(Box::new(series_source.to_string()));
                bind_values.push(Box::new(series_key.to_string()));
            } else {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM download_series
                        WHERE series_key = ? OR title = ?
                    )"
                    .to_string(),
                );
                bind_values.push(Box::new(series_key.to_string()));
                bind_values.push(Box::new(series_key.to_string()));
            }
        }
    }
}

fn active_strings(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn normalized_tag_list(values: &[String]) -> Vec<String> {
    let mut tags = active_strings(values);
    tags.sort();
    tags.dedup();
    tags
}

fn sort_clause(params: &SearchV2Params) -> String {
    let sort_col = match normalized_sort_key(params) {
        "title" => "d.title COLLATE NOCASE",
        "author" => "d.author_name COLLATE NOCASE",
        "date" => "d.downloaded_at",
        "published" => "COALESCE(d.source_created_at, d.downloaded_at)",
        "updated" => "COALESCE(d.source_updated_at, d.source_created_at, d.downloaded_at)",
        "series_order" => {
            return format!(
                " ORDER BY {} {}, d.id {}",
                series_order_sort_expr(),
                sort_order(params),
                sort_order(params)
            )
        }
        "size" => "d.file_size_bytes",
        "length" => "d.text_length",
        _ => "d.downloaded_at",
    };
    let sort_order = sort_order(params);
    format!(" ORDER BY {} {}, d.id {}", sort_col, sort_order, sort_order)
}

fn sort_order(params: &SearchV2Params) -> &'static str {
    match params.sort_order.as_deref() {
        Some("asc") => "ASC",
        _ => "DESC",
    }
}

fn series_order_sort_expr() -> String {
    "printf('%020lld|%s',
        (SELECT COALESCE(MIN(ds.content_order), 9223372036854775807) FROM download_series ds WHERE ds.download_id = d.id),
        COALESCE(d.source_created_at, d.downloaded_at)
    )"
    .to_string()
}

fn get_work_edit_revision_locked(
    conn: &Connection,
    revision_id: i64,
) -> Result<WorkEditRevision, String> {
    conn.query_row(
        "SELECT id, download_id, base_version, status, title, content_hash, created_at, updated_at
         FROM work_edit_revisions
         WHERE id = ?1",
        params![revision_id],
        work_edit_revision_from_row,
    )
    .map_err(|e| format!("Work edit revision not found: {}", e))
}

fn active_edit_revision_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<WorkEditRevision>, String> {
    conn.query_row(
        "SELECT id, download_id, base_version, status, title, content_hash, created_at, updated_at
         FROM work_edit_revisions
         WHERE download_id = ?1 AND status = 'active'
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![download_id],
        work_edit_revision_from_row,
    )
    .optional()
    .map_err(|e| format!("Failed to load active edit revision: {}", e))
}

fn draft_edit_revision_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<WorkEditRevision>, String> {
    conn.query_row(
        "SELECT id, download_id, base_version, status, title, content_hash, created_at, updated_at
         FROM work_edit_revisions
         WHERE download_id = ?1 AND status = 'draft'
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![download_id],
        work_edit_revision_from_row,
    )
    .optional()
    .map_err(|e| format!("Failed to load draft edit revision: {}", e))
}

fn blocks_for_revision_locked(
    conn: &Connection,
    revision_id: i64,
) -> Result<Vec<WorkBlock>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, edit_revision_id, block_order, block_type, text, asset_id, attrs_json
             FROM work_edit_blocks
             WHERE edit_revision_id = ?1
             ORDER BY block_order ASC",
        )
        .map_err(|e| format!("Work block query prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![revision_id], work_block_from_row)
        .map_err(|e| format!("Work block query failed: {}", e))?;
    let mut blocks = Vec::new();
    for row in rows {
        blocks.push(row.map_err(|e| format!("Work block row failed: {}", e))?);
    }
    Ok(blocks)
}

fn insert_work_blocks_locked(
    tx: &Transaction<'_>,
    revision_id: i64,
    blocks: &[WorkBlockInput],
) -> Result<(), String> {
    for (idx, block) in blocks.iter().enumerate() {
        tx.execute(
            "INSERT INTO work_edit_blocks (
                edit_revision_id, block_order, block_type, text, asset_id, attrs_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                revision_id,
                idx as i64,
                block.block_type,
                block.text,
                block.asset_id,
                block.attrs_json
            ],
        )
        .map_err(|e| format!("Failed to insert work block: {}", e))?;
    }
    Ok(())
}

fn normalize_block_inputs(blocks: &[WorkBlockInput]) -> Vec<WorkBlockInput> {
    let mut normalized = Vec::new();
    for block in blocks {
        let block_type = match block.block_type.as_str() {
            "heading" | "image" | "separator" | "page_break" | "quote" | "link" => {
                block.block_type.clone()
            }
            _ => "paragraph".to_string(),
        };
        let text = block
            .text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if block_type != "image"
            && text.is_none()
            && block_type != "separator"
            && block_type != "page_break"
        {
            continue;
        }
        normalized.push(WorkBlockInput {
            block_type,
            text,
            asset_id: block.asset_id,
            attrs_json: block.attrs_json.clone(),
        });
    }
    if normalized.is_empty() {
        normalized.push(WorkBlockInput {
            block_type: "paragraph".to_string(),
            text: Some(String::new()),
            asset_id: None,
            attrs_json: None,
        });
    }
    normalized
}

fn plain_text_to_editor_blocks(text: &str) -> Vec<WorkBlock> {
    let chunks = text
        .split("\n\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>();
    let chunks = if chunks.is_empty() {
        vec![text.trim()]
    } else {
        chunks
    };

    chunks
        .into_iter()
        .enumerate()
        .map(|(idx, text)| WorkBlock {
            id: 0,
            edit_revision_id: 0,
            order: idx as i64,
            block_type: if text.starts_with('#') {
                "heading".to_string()
            } else {
                "paragraph".to_string()
            },
            text: Some(text.trim_start_matches('#').trim().to_string()),
            asset_id: None,
            attrs_json: None,
        })
        .collect()
}

fn html_to_editor_blocks(html: &str, assets: &[AssetEntry]) -> Vec<WorkBlock> {
    if html.trim().is_empty() {
        return plain_text_to_editor_blocks("");
    }
    let token_re = Regex::new(
        r#"(?is)(<!--\s*newpage\s*-->|<h2\b[^>]*>.*?</h2>|<img\b[^>]*>|<hr\s*/?>|<a\b[^>]*class=\"[^\"]*novel-link-card[^\"]*\"[^>]*>.*?</a>)"#,
    )
    .expect("valid editor HTML token regex");
    let attr_re =
        Regex::new(r#"(?i)([a-z0-9_-]+)=\"([^\"]*)\""#).expect("valid editor attribute regex");
    let mut blocks = Vec::new();
    let mut cursor = 0;

    let push_text = |fragment: &str, blocks: &mut Vec<WorkBlock>| {
        let text = html_fragment_to_text(fragment);
        for chunk in text
            .split("\n\n")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            blocks.push(editor_block(
                blocks.len(),
                "paragraph",
                Some(chunk.to_string()),
                None,
                None,
            ));
        }
    };

    for matched in token_re.find_iter(html) {
        push_text(&html[cursor..matched.start()], &mut blocks);
        let token = matched.as_str();
        let lower = token.to_ascii_lowercase();
        if lower.starts_with("<!--") {
            blocks.push(editor_block(blocks.len(), "page_break", None, None, None));
        } else if lower.starts_with("<h2") {
            blocks.push(editor_block(
                blocks.len(),
                "heading",
                Some(html_fragment_to_text(token).trim().to_string()),
                None,
                None,
            ));
        } else if lower.starts_with("<img") {
            let local_path = attr_re
                .captures_iter(token)
                .find(|capture| {
                    capture
                        .get(1)
                        .map(|v| v.as_str().eq_ignore_ascii_case("data-local-path"))
                        .unwrap_or(false)
                })
                .and_then(|capture| {
                    capture
                        .get(2)
                        .map(|value| decode_editor_entities(value.as_str()))
                });
            let asset_id = local_path.as_deref().and_then(|path| {
                assets
                    .iter()
                    .find(|asset| asset.local_path == path || asset.local_path.ends_with(path))
                    .map(|asset| asset.id)
            });
            let alt = attr_re
                .captures_iter(token)
                .find(|capture| {
                    capture
                        .get(1)
                        .map(|v| v.as_str().eq_ignore_ascii_case("alt"))
                        .unwrap_or(false)
                })
                .and_then(|capture| {
                    capture
                        .get(2)
                        .map(|value| decode_editor_entities(value.as_str()))
                });
            blocks.push(editor_block(blocks.len(), "image", alt, asset_id, None));
        } else if lower.starts_with("<hr") {
            blocks.push(editor_block(blocks.len(), "separator", None, None, None));
        } else {
            let href = attr_re
                .captures_iter(token)
                .find(|capture| {
                    capture
                        .get(1)
                        .map(|v| v.as_str().eq_ignore_ascii_case("href"))
                        .unwrap_or(false)
                })
                .and_then(|capture| {
                    capture
                        .get(2)
                        .map(|value| decode_editor_entities(value.as_str()))
                })
                .unwrap_or_default();
            let label = html_fragment_to_text(token)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let attrs = serde_json::json!({ "label": label }).to_string();
            blocks.push(editor_block(
                blocks.len(),
                "link",
                Some(href),
                None,
                Some(attrs),
            ));
        }
        cursor = matched.end();
    }
    push_text(&html[cursor..], &mut blocks);
    if blocks.is_empty() {
        plain_text_to_editor_blocks("")
    } else {
        blocks
    }
}

fn editor_block(
    order: usize,
    block_type: &str,
    text: Option<String>,
    asset_id: Option<i64>,
    attrs_json: Option<String>,
) -> WorkBlock {
    WorkBlock {
        id: 0,
        edit_revision_id: 0,
        order: order as i64,
        block_type: block_type.to_string(),
        text,
        asset_id,
        attrs_json,
    }
}

fn html_fragment_to_text(fragment: &str) -> String {
    let breaks = Regex::new(r"(?i)<br\s*/?>|</(?:p|div|h[1-6]|blockquote)>")
        .expect("valid HTML break regex");
    let tags = Regex::new(r"(?is)<[^>]+>").expect("valid HTML tag regex");
    let with_breaks = breaks.replace_all(fragment, "\n");
    decode_editor_entities(tags.replace_all(&with_breaks, "").as_ref())
        .replace("\r\n", "\n")
        .replace("\n\n\n", "\n\n")
}

fn decode_editor_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

fn hash_blocks(blocks: &[WorkBlockInput]) -> String {
    let mut hasher = Sha256::new();
    for block in blocks {
        hasher.update(block.block_type.as_bytes());
        hasher.update([0]);
        if let Some(text) = &block.text {
            hasher.update(text.as_bytes());
        }
        hasher.update([0]);
        if let Some(asset_id) = block.asset_id {
            hasher.update(asset_id.to_le_bytes());
        }
        hasher.update([0]);
        if let Some(attrs) = &block.attrs_json {
            hasher.update(attrs.as_bytes());
        }
        hasher.update([0xff]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

fn blocks_to_plain_text(blocks: &[WorkBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block.block_type.as_str() {
            "image" | "separator" | "page_break" => None,
            _ => block.text.as_deref(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

const READER_PAGE_TARGET_BYTES: usize = 128 * 1024;

fn reader_version_path(
    download: &DownloadEntry,
    versions: &[DownloadVersion],
    version: i64,
) -> PathBuf {
    let selected = if version == download.current_version {
        download
            .original_json_path
            .as_ref()
            .filter(|path| Path::new(path).exists())
            .unwrap_or(&download.json_path)
    } else if let Some(target) = versions.iter().find(|item| item.version == version) {
        target
            .original_json_path
            .as_ref()
            .filter(|path| Path::new(path).exists())
            .unwrap_or(&target.json_path)
    } else {
        &download.json_path
    };
    PathBuf::from(selected)
}

fn reader_source_content(
    db: &Database,
    download: &DownloadEntry,
    versions: &[DownloadVersion],
    version: Option<i64>,
    assets: &[AssetEntry],
) -> Result<(String, String), String> {
    let target_version = version.unwrap_or(download.current_version);
    let raw_json = db.read_download_json_for_version(download, versions, target_version)?;
    let html = if download.source == "pixiv" {
        super::parser::parse_pixiv_to_html(&raw_json, assets)
    } else if download.source == "fanbox" {
        super::parser::parse_fanbox_to_html(&raw_json, assets)
    } else {
        String::new()
    };
    let plain_text = serde_json::from_str::<serde_json::Value>(&raw_json)
        .ok()
        .map(|value| extract_search_body(&value, &download.source))
        .unwrap_or_default();
    Ok((html, plain_text))
}

/// Converts source-level boundaries into reasonably sized transport pages.
/// Boundaries always sit between complete source blocks, so a page can be
/// sanitized independently without producing malformed markup.
fn paginate_reader_html(html: &str, source: &str) -> Vec<String> {
    if html.trim().is_empty() {
        return Vec::new();
    }
    let marker = if source.eq_ignore_ascii_case("pixiv") {
        "<!-- newpage -->"
    } else {
        "<!-- content-block -->"
    };
    let blocks = html
        .split(marker)
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let mut pages = Vec::new();
    let mut current = String::new();
    for block in blocks {
        // Pixiv's [newpage] is user-visible page semantics and must never be
        // coalesced. FANBOX/editor boundaries are transport-only and can be
        // grouped to avoid thousands of tiny IPC calls.
        if source.eq_ignore_ascii_case("pixiv") {
            pages.push(block.to_string());
            continue;
        }
        if !current.is_empty()
            && current.len().saturating_add(block.len()) > READER_PAGE_TARGET_BYTES
        {
            pages.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(block);
    }
    if !current.trim().is_empty() {
        pages.push(current);
    }
    if pages.is_empty() {
        pages.push(html.to_string());
    }
    pages
}

fn plain_text_from_reader_html(html: &str) -> String {
    static TAGS: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("valid HTML tag regex"));
    let separated = html
        .replace("<br />", "\n")
        .replace("<br>", "\n")
        .replace("</p>", "\n")
        .replace("</h2>", "\n")
        .replace("</blockquote>", "\n");
    TAGS.replace_all(&separated, "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .trim()
        .to_string()
}

fn blocks_to_html(blocks: &[WorkBlock], assets: &[AssetEntry]) -> String {
    blocks
        .iter()
        .map(|block| match block.block_type.as_str() {
            "heading" => format!(
                "<h2>{}</h2>",
                escape_editor_html(block.text.as_deref().unwrap_or(""))
            ),
            "image" => {
                let asset = block
                    .asset_id
                    .and_then(|id| assets.iter().find(|asset| asset.id == id));
                if let Some(asset) = asset {
                    format!(
                        r#"<img class="novel-image" data-local-path="{}" alt="{}" />"#,
                        escape_editor_html(&asset.local_path),
                        escape_editor_html(&asset.filename)
                    )
                } else {
                    "<div class=\"missing-image-placeholder\">画像が見つかりません</div>"
                        .to_string()
                }
            }
            "separator" => "<hr />".to_string(),
            "page_break" => "<!-- newpage -->".to_string(),
            "quote" => format!(
                "<blockquote>{}</blockquote>",
                escape_editor_html(block.text.as_deref().unwrap_or("")).replace('\n', "<br />\n")
            ),
            "link" => {
                let url = block.text.as_deref().unwrap_or("");
                let label = block
                    .attrs_json
                    .as_deref()
                    .and_then(|attrs| serde_json::from_str::<serde_json::Value>(attrs).ok())
                    .and_then(|attrs| attrs.get("label").and_then(|value| value.as_str()).map(str::to_string))
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or_else(|| url.to_string());
                format!(
                    r#"<a href="{}" target="_blank" rel="noopener noreferrer" class="novel-link-card"><span class="link-card-icon">🔗</span><span class="link-card-info"><span class="link-card-title">{}</span><span class="link-card-host">{}</span></span></a>"#,
                    escape_editor_html(url),
                    escape_editor_html(&label),
                    escape_editor_html(url)
                )
            }
            _ => escape_editor_html(block.text.as_deref().unwrap_or("")).replace('\n', "<br />\n"),
        })
        .collect::<Vec<_>>()
        .join("\n<!-- content-block -->\n")
}

fn active_edit_plain_text_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<String>, String> {
    let Some(revision) = active_edit_revision_locked(conn, download_id)? else {
        return Ok(None);
    };
    let blocks = blocks_for_revision_locked(conn, revision.id)?;
    Ok(Some(blocks_to_plain_text(&blocks)))
}

fn escape_editor_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn stale_search_index_ids_locked(
    conn: &Connection,
    limit: i64,
    after_id: i64,
) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id
             FROM downloads d
             LEFT JOIN search_index_state m ON m.download_id = d.id
             WHERE d.id > ?2
               AND (m.download_id IS NULL
                    OR m.current_version != d.current_version
                    OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, ''))
             ORDER BY d.id ASC
             LIMIT ?1",
        )
        .map_err(|e| format!("Search index stale query prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![limit, after_id], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("Search index stale query failed: {}", e))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| format!("Search index stale row read failed: {}", e))?);
    }
    Ok(ids)
}

/// Discards the "already indexed" bookkeeping when the on-disk index format has
/// changed under it.
///
/// A new format means a new, empty index directory. The rows in
/// `search_index_state` still describe the old one, so without this the library
/// would report itself fully indexed while every search came back empty.
fn reconcile_search_index_format(conn: &Connection) -> Result<(), String> {
    let current = super::tantivy_index::index_format_version();
    let stored: Option<String> = conn
        .query_row(
            "SELECT index_version FROM search_index_meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Search index format read failed: {e}"))?;
    if stored.as_deref() == Some(current) {
        return Ok(());
    }

    let cleared = conn
        .execute("DELETE FROM search_index_state", [])
        .map_err(|e| format!("Search index state reset failed: {e}"))?;
    if cleared > 0 {
        log::info!(
            "Search index format changed to {current}; {cleared} works queued for reindexing"
        );
    }
    conn.execute(
        "INSERT OR REPLACE INTO search_index_meta (id, index_version, updated_at)
         VALUES (1, ?1, ?2)",
        params![current, chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| format!("Search index format write failed: {e}"))?;
    Ok(())
}

fn pending_search_index_count(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM downloads d
         LEFT JOIN search_index_state m ON m.download_id = d.id
         WHERE m.download_id IS NULL
            OR m.current_version != d.current_version
            OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')",
        [],
        |row| row.get(0),
    )
    .map_err(|e| format!("Search index pending count failed: {e}"))
}

fn search_index_status_locked(
    conn: &Connection,
    storage_dir: &Path,
) -> Result<SearchIndexStatus, String> {
    let total_downloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .map_err(|e| format!("Search index status total failed: {}", e))?;
    let indexed_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM downloads d
             JOIN search_index_state m ON m.download_id = d.id
             WHERE m.current_version = d.current_version
               AND COALESCE(m.content_hash, '') = COALESCE(d.content_hash, '')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Search index status indexed failed: {}", e))?;
    let pending_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM downloads d
             LEFT JOIN search_index_state m ON m.download_id = d.id
             WHERE m.download_id IS NULL
                OR m.current_version != d.current_version
                OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Search index status pending failed: {}", e))?;

    let semantic = super::semantic_index::status(storage_dir);

    Ok(SearchIndexStatus {
        total_downloads,
        indexed_downloads,
        pending_downloads,
        is_complete: pending_downloads == 0,
        phase: if pending_downloads == 0 {
            "ready".to_string()
        } else {
            "indexing".to_string()
        },
        indexed_chunks: semantic.indexed_chunks,
        semantic_indexed_chunks: semantic.indexed_chunks,
        semantic_model_ready: semantic.model_ready,
        embedding_provider: semantic.provider,
        gpu_enabled: semantic.gpu_enabled,
        throughput_per_sec: None,
    })
}

fn reindex_download_locked(
    conn: &Connection,
    storage_dir: &Path,
    download_id: i64,
) -> Result<(), String> {
    let Some(doc) = search_index_document_locked(conn, storage_dir, download_id)? else {
        clear_search_index_locked(conn, storage_dir, download_id)?;
        return Ok(());
    };
    index_search_documents_locked(conn, storage_dir, &[doc], true)
}

fn search_index_document_locked(
    conn: &Connection,
    _storage_dir: &Path,
    download_id: i64,
) -> Result<Option<SearchIndexBuildDocument>, String> {
    let row = conn
        .query_row(
            "SELECT d.source, d.source_id, d.title, d.author_name, d.author_id,
                    (SELECT GROUP_CONCAT(t.name, ' ')
                     FROM download_tags dt
                     JOIN tags t ON t.id = dt.tag_id
                     WHERE dt.download_id = d.id),
                    d.excerpt, d.json_path,
                    d.original_json_path, d.current_version, d.content_hash,
                    (SELECT GROUP_CONCAT(ds.title, ' ') FROM download_series ds WHERE ds.download_id = d.id),
                    COALESCE(d.source_created_at, ''),
                    d.downloaded_at,
                    d.favorite,
                    d.watch_updates,
                    d.text_length,
                    (SELECT GROUP_CONCAT(DISTINCT a.asset_type) FROM assets a WHERE a.download_id = d.id)
             FROM downloads d
             WHERE d.id = ?1",
            params![download_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, i64>(14)? != 0,
                    row.get::<_, i64>(15)? != 0,
                    row.get::<_, i64>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Search index download query failed: {}", e))?;

    let Some((
        source,
        source_id,
        title,
        author_name,
        author_id,
        tags_raw,
        excerpt,
        json_path,
        original_json_path,
        current_version,
        content_hash,
        series_title,
        published_at,
        downloaded_at,
        favorite,
        watch_updates,
        text_length,
        asset_kinds,
    )) = row
    else {
        return Ok(None);
    };

    let json_path = original_json_path.unwrap_or(json_path);
    let body = active_edit_plain_text_locked(conn, download_id)?.unwrap_or_else(|| {
        std::fs::read_to_string(&json_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|value| extract_search_body(&value, &source))
            .unwrap_or_default()
    });

    let tags = tags_raw.unwrap_or_default();
    let series_title_raw = series_title.unwrap_or_default();
    let excerpt_raw = excerpt.unwrap_or_default();
    let doc = SearchDocument {
        title: title.clone(),
        author_name: author_name.clone(),
        tags: tags.clone(),
        series_title: series_title_raw.clone(),
        excerpt: excerpt_raw.clone(),
        body: body.clone(),
    };
    Ok(Some(SearchIndexBuildDocument {
        download_id,
        current_version,
        content_hash,
        tantivy: super::tantivy_index::TantivyIndexDocument {
            download_id,
            source: source.clone(),
            source_id: source_id.clone(),
            source_url: source_url_for_download(&source, &source_id, &author_id),
            title: doc.title,
            author_name: doc.author_name,
            author_id: author_id.clone(),
            tags: doc.tags,
            series_title: doc.series_title,
            excerpt: doc.excerpt,
            body: doc.body,
            published_at,
            downloaded_at,
            favorite,
            watch_updates,
            asset_kinds: normalize_search_text(asset_kinds.as_deref().unwrap_or("")),
            text_length,
        },
        semantic: super::semantic_index::SemanticIndexDocument {
            download_id,
            title,
            author_name,
            tags,
            series_title: series_title_raw,
            excerpt: excerpt_raw,
            body,
        },
    }))
}

fn index_search_documents_locked(
    conn: &Connection,
    storage_dir: &Path,
    docs: &[SearchIndexBuildDocument],
    include_semantic: bool,
) -> Result<(), String> {
    if docs.is_empty() {
        return Ok(());
    }
    let tantivy_docs = docs
        .iter()
        .map(|doc| doc.tantivy.clone())
        .collect::<Vec<_>>();
    super::tantivy_index::upsert_documents(storage_dir, &tantivy_docs)?;

    let now = chrono::Utc::now().to_rfc3339();
    for doc in docs {
        conn.execute(
            "INSERT OR REPLACE INTO search_index_state (
                download_id, current_version, content_hash, indexed_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![doc.download_id, doc.current_version, doc.content_hash, now],
        )
        .map_err(|e| format!("Search meta insert failed: {}", e))?;
    }

    if !include_semantic {
        return Ok(());
    }

    let semantic_docs = docs
        .iter()
        .map(|doc| doc.semantic.clone())
        .collect::<Vec<_>>();
    if let Err(error) = super::semantic_index::upsert_documents(storage_dir, &semantic_docs) {
        log::warn!(
            "Semantic index batch update skipped for {} documents: {}",
            docs.len(),
            error
        );
    }

    Ok(())
}

fn clear_search_index_locked(
    conn: &Connection,
    storage_dir: &Path,
    download_id: i64,
) -> Result<(), String> {
    super::tantivy_index::delete_document(storage_dir, download_id)?;
    if let Err(error) = super::semantic_index::clear_document(storage_dir, download_id) {
        log::warn!(
            "Semantic index clear skipped for {}: {}",
            download_id,
            error
        );
    }
    conn.execute(
        "DELETE FROM search_index_state WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Search meta clear failed: {}", e))?;
    Ok(())
}

fn filter_excluded_search_results(
    storage_dir: &Path,
    results: Vec<DownloadEntry>,
    parsed: &ParsedSearchQuery,
    documents: &HashMap<i64, Arc<SearchDocument>>,
) -> Vec<DownloadEntry> {
    if parsed.exclude.is_empty() {
        return results;
    }

    let mut filtered = Vec::with_capacity(results.len());
    for entry in results {
        let loaded_document;
        let doc = if let Some(doc) = documents.get(&entry.id) {
            doc.as_ref()
        } else {
            let Ok(Some(doc)) = super::tantivy_index::load_document(storage_dir, entry.id) else {
                continue;
            };
            loaded_document = doc;
            &loaded_document
        };
        if document_matches_excluded_term(doc, parsed) {
            continue;
        }
        filtered.push(entry);
    }
    filtered
}

fn decorate_search_results(
    storage_dir: &Path,
    results: Vec<DownloadEntry>,
    parsed: &ParsedSearchQuery,
    semantic_hits: &HashMap<i64, super::semantic_index::SemanticSearchHit>,
    documents: &HashMap<i64, Arc<SearchDocument>>,
) -> Vec<DownloadEntry> {
    if parsed.include.is_empty() && semantic_hits.is_empty() {
        return results;
    }

    let mut decorated = Vec::with_capacity(results.len());
    for mut entry in results {
        let loaded_document;
        let doc = if let Some(doc) = documents.get(&entry.id) {
            doc.as_ref()
        } else {
            let Ok(Some(doc)) = super::tantivy_index::load_document(storage_dir, entry.id) else {
                decorated.push(entry);
                continue;
            };
            loaded_document = doc;
            &loaded_document
        };
        let (fields, reasons, computed_score) = match_fields_and_score(doc, parsed);
        if !fields.is_empty() {
            entry.match_fields = fields;
        }
        if !reasons.is_empty() {
            entry.score_reasons = reasons
                .into_iter()
                .map(|reason| ScoreReason {
                    field: reason.field,
                    match_type: reason.match_type,
                    term: reason.term,
                    contribution: reason.contribution,
                    detail: reason.detail,
                })
                .collect();
        }
        if let Some(semantic) = semantic_hits.get(&entry.id) {
            entry.score_reasons.push(ScoreReason {
                field: semantic.field.clone(),
                match_type: "semantic".to_string(),
                term: parsed
                    .include
                    .first()
                    .map(|term| term.raw.clone())
                    .unwrap_or_default(),
                contribution: semantic.score * 120.0,
                detail: Some(format!("semantic chunk score {:.3}", semantic.score)),
            });
            let semantic_highlight = semantic_chunk_highlight(semantic);
            let mut highlights = make_match_highlights(doc, parsed);
            highlights.insert(0, semantic_highlight);
            entry.match_highlights = highlights.into_iter().take(4).collect();
        } else {
            entry.match_highlights = make_match_highlights(doc, parsed);
        }
        if computed_score > 0.0 {
            entry.search_score = Some(entry.search_score.unwrap_or(0.0) + computed_score);
        }
        decorated.push(entry);
    }
    decorated
}

fn document_matches_excluded_term(doc: &SearchDocument, parsed: &ParsedSearchQuery) -> bool {
    parsed.exclude.iter().any(|term| {
        doc.fields().iter().any(|(_, text, _)| {
            let normalized = normalize_search_text(text);
            !text.is_empty()
                && term
                    .variants
                    .iter()
                    .any(|variant| !variant.is_empty() && normalized.contains(variant))
        })
    })
}

fn semantic_chunk_highlight(
    semantic: &super::semantic_index::SemanticSearchHit,
) -> SearchHighlight {
    SearchHighlight {
        field: semantic.field.clone(),
        text: semantic.text.clone(),
        segments: vec![SearchHighlightSegment {
            text: truncate_highlight_text(&semantic.text, 220),
            matched: true,
        }],
        source_chunk_id: Some(semantic.chunk_id.clone()),
        match_type: Some("semantic".to_string()),
    }
}

fn truncate_highlight_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn work_edit_revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkEditRevision> {
    Ok(WorkEditRevision {
        id: row.get(0)?,
        download_id: row.get(1)?,
        base_version: row.get(2)?,
        status: row.get(3)?,
        title: row.get(4)?,
        content_hash: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn work_block_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkBlock> {
    Ok(WorkBlock {
        id: row.get(0)?,
        edit_revision_id: row.get(1)?,
        order: row.get(2)?,
        block_type: row.get(3)?,
        text: row.get(4)?,
        asset_id: row.get(5)?,
        attrs_json: row.get(6)?,
    })
}

fn update_target_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateTarget> {
    Ok(UpdateTarget {
        id: row.get(0)?,
        target_type: row.get(1)?,
        source: row.get(2)?,
        source_key: row.get(3)?,
        display_name: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        last_checked_at: row.get(6)?,
        last_seen_source_id: row.get(7)?,
        last_seen_source_updated_at: row.get(8)?,
        metadata_json: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn update_job_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateJobSummary> {
    Ok(UpdateJobSummary {
        job_id: row.get(0)?,
        status: row.get(1)?,
        scope: row.get(2)?,
        mode: row.get(3)?,
        totals: row.get(4)?,
        processed: row.get(5)?,
        candidate_count: row.get(6)?,
        saved_count: row.get(7)?,
        error_count: row.get(8)?,
        active_label: row.get(9)?,
        started_at: row.get(10)?,
        updated_at: row.get(11)?,
        finished_at: row.get(12)?,
    })
}

fn update_job_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateJobItem> {
    Ok(UpdateJobItem {
        id: row.get(0)?,
        job_id: row.get(1)?,
        item_type: row.get(2)?,
        source: row.get(3)?,
        source_id: row.get(4)?,
        target_type: row.get(5)?,
        title: row.get(6)?,
        payload_json: row.get(7)?,
        status: row.get(8)?,
        error: row.get(9)?,
        result_download_id: row.get(10)?,
    })
}

fn download_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadEntry> {
    let tags = row
        .get::<_, Option<String>>(7)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    let match_fields = row
        .get::<_, Option<String>>(27)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let score_reasons = row
        .get::<_, Option<String>>(28)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let match_highlights = row
        .get::<_, Option<String>>(29)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    Ok(DownloadEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_id: row.get(2)?,
        title: row.get(3)?,
        author_name: row.get(4)?,
        author_id: row.get(5)?,
        content_type: row.get(6)?,
        tags,
        excerpt: row.get(8)?,
        cover_path: row.get(9)?,
        json_path: row.get(10)?,
        original_json_path: row.get(11)?,
        asset_count: row.get(12)?,
        file_size_bytes: row.get(13)?,
        downloaded_at: row.get(14)?,
        source_created_at: row.get(15)?,
        content_hash: row.get(16)?,
        text_length: row.get(17)?,
        source_updated_at: row.get(18)?,
        watch_updates: row.get::<_, i64>(19)? != 0,
        current_version: row.get(20)?,
        favorite: row.get::<_, i64>(21)? != 0,
        person_id: row.get(22).ok(),
        person_name: row.get(23).ok(),
        series_id: row.get(24).ok(),
        series_title: row.get(25).ok(),
        search_score: row.get(26).ok(),
        match_fields,
        score_reasons,
        match_highlights,
        sort_key: row.get(30).ok(),
        // Appended last so the existing positional reads keep their indexes.
        person_icon_path: row.get(31).ok(),
    })
}

fn person_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonEntry> {
    Ok(PersonEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_key: row.get(2)?,
        display_name: row.get(3)?,
        icon_path: row.get(4)?,
        cover_path: row.get(5)?,
        description: row.get(6)?,
        links_json: row.get(7)?,
        content_hash: row.get(8)?,
        current_version: row.get(9)?,
        last_checked_at: row.get(10)?,
        last_fetched_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        work_count: row.get(14).ok(),
    })
}

fn series_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SeriesEntry> {
    Ok(SeriesEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_key: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        cover_path: row.get(5)?,
        content_hash: row.get(6)?,
        current_version: row.get(7)?,
        last_checked_at: row.get(8)?,
        last_fetched_at: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        work_count: row.get(12).ok(),
    })
}

fn entity_version_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityVersion> {
    Ok(EntityVersion {
        id: row.get(0)?,
        entity_type: row.get(1)?,
        source: row.get(2)?,
        source_key: row.get(3)?,
        version: row.get(4)?,
        content_hash: row.get(5)?,
        json_path: row.get(6)?,
        asset_count: row.get(7)?,
        file_size_bytes: row.get(8)?,
        created_at: row.get(9)?,
        change_summary: row.get(10)?,
    })
}

/// Windowsなどでファイルが掴まれているため一時的に削除に失敗するケースに備えた、
/// リトライ機能付きのディレクトリ再帰削除ヘルパー
fn remove_dir_all_resilient(path: &std::path::Path) -> std::io::Result<()> {
    let mut attempts = 5;
    loop {
        match std::fs::remove_dir_all(path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                attempts -= 1;
                if attempts == 0 {
                    return Err(e);
                }
                log::warn!(
                    "Failed to remove directory {:?}, retrying in 150ms... (Attempts left: {}) Error: {}",
                    path,
                    attempts,
                    e
                );
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
}

/// ディレクトリが空の場合にのみ削除する、リトライ機能付きの削除ヘルパー
/// (空ではない場合のエラーは即座に返す)
fn remove_dir_resilient(path: &std::path::Path) -> std::io::Result<()> {
    let mut attempts = 5;
    loop {
        match std::fs::remove_dir(path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                // ディレクトリが空ではない場合はリトライしても無駄なので即座に返す
                // Windows: ERROR_DIR_NOT_EMPTY (145), Unix: ENOTEMPTY (39)
                if let Some(code) = e.raw_os_error() {
                    if code == 145 || code == 39 {
                        return Err(e);
                    }
                }
                attempts -= 1;
                if attempts == 0 {
                    return Err(e);
                }
                log::warn!(
                    "Failed to remove empty directory {:?}, retrying in 150ms... (Attempts left: {}) Error: {}",
                    path,
                    attempts,
                    e
                );
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
}

#[cfg(test)]
mod search_integration_tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::time::{Duration, Instant};

    /// Builds prose whose vocabulary keeps growing, the way a real library does.
    fn synthetic_body(seed: u64, chars: usize) -> String {
        let nouns = [
            "教室", "図書館", "海岸", "旋律", "記憶", "季節", "手紙", "灯台", "回廊", "約束",
            "硝子", "残響", "標本", "封筒", "螺旋", "夜明", "輪郭", "潮騒", "書架", "遠雷",
        ];
        let verbs = [
            "見つめていた",
            "思い出していた",
            "書き留めた",
            "数えていた",
            "聞いていた",
            "受け止めた",
        ];
        let adjectives = ["静かな", "薄い", "淡い", "遠い", "冷たい", "眩しい"];
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        };
        let mut text = String::with_capacity(chars * 3);
        while text.chars().count() < chars {
            text.push_str(adjectives[next() % adjectives.len()]);
            text.push_str(nouns[next() % nouns.len()]);
            text.push('と');
            text.push_str(nouns[next() % nouns.len()]);
            let number = next() % 100_000;
            text.push_str(&format!("{number}番"));
            text.push('を');
            text.push_str(verbs[next() % verbs.len()]);
            text.push_str("。\n");
        }
        text.chars().take(chars).collect()
    }

    #[test]
    #[ignore = "measurement harness, run with --ignored --nocapture"]
    fn measure_full_index_rebuild() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let works: usize = std::env::var("PIEP_BENCH_WORKS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(300);
        let body_chars: usize = std::env::var("PIEP_BENCH_CHARS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(12_000);

        let seeding = Instant::now();
        for index in 0..works {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("bench-{index}"),
                &format!("計測用作品 {index}"),
                &format!("計測作者 {}", index % 40),
                &["計測", "長編"],
                &synthetic_body(index as u64 + 1, body_chars),
            );
        }
        println!(
            "seeded {works} works of {body_chars} chars in {:.1} s",
            seeding.elapsed().as_secs_f64()
        );

        let started = Instant::now();
        let outcome = db
            .rebuild_search_index(
                SearchIndexRebuildOptions::default(),
                &|| false,
                |_progress| {},
            )
            .unwrap();
        let elapsed = started.elapsed().as_secs_f64();
        let status = db.get_search_index_status().unwrap();
        let index_bytes = recursive_file_size(&storage.join("search-index"));
        println!(
            "rebuilt {} works ({} failed) in {elapsed:.2} s = {:.1} works/s | pending {} | index {:.1} MB",
            outcome.processed,
            outcome.failed,
            outcome.processed as f64 / elapsed,
            status.pending_downloads,
            index_bytes as f64 / 1_048_576.0,
        );
        assert_eq!(status.pending_downloads, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reader_transport_pages_keep_complete_source_blocks() {
        let large = "あ".repeat(READER_PAGE_TARGET_BYTES / 3);
        let html = format!(
            "<p>{large}</p><!-- content-block --><p>{large}</p><!-- content-block --><p>{large}</p>"
        );
        let pages = paginate_reader_html(&html, "fanbox");
        assert!(pages.len() >= 2);
        assert!(pages.iter().all(|page| !page.contains("content-block")));
        assert!(pages
            .iter()
            .all(|page| page.starts_with("<p>") && page.ends_with("</p>")));

        let pixiv = paginate_reader_html("first<!-- newpage -->second", "pixiv");
        assert_eq!(pixiv, vec!["first", "second"]);
    }

    #[test]
    fn diagnostic_percentiles_are_stable() {
        let mut samples = [9.0, 1.0, 7.0, 3.0, 5.0];
        assert_eq!(benchmark_percentiles(&mut samples), (5.0, 9.0));
    }

    #[test]
    fn atomic_restore_transaction_rolls_back_database_rows() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let id = insert_download_unindexed(
            &db,
            &storage,
            "restore-rollback",
            "残る作品",
            "作者",
            &[],
            "本文",
        );
        db.begin_atomic_restore().unwrap();
        db.delete_download_record_for_restore(id).unwrap();
        std::thread::scope(|scope| {
            let concurrent = scope
                .spawn(|| db.delete_download_record_for_restore(id))
                .join()
                .unwrap();
            assert!(concurrent.unwrap_err().contains("restore is in progress"));
        });
        db.rollback_atomic_restore();
        assert_eq!(db.get_download(id).unwrap().title, "残る作品");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reader_cache_pages_and_full_document_search_share_one_index() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let id = insert_download_unindexed(
            &db,
            &storage,
            "reader-search",
            "検索できる作品",
            "作者",
            &[],
            "最初のページ [newpage] 次のページにNeedleとneedle",
        );
        let first = db.get_reader_content_page(id, None, 0).unwrap();
        let second = db.get_reader_content_page(id, None, 1).unwrap();
        assert_eq!(first.page_count, 2);
        assert_eq!(second.page_count, 2);
        assert!(second.html.contains("Needle"));

        let hits = db.search_reader_content(id, None, "needle", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 2);
        assert_eq!(hits[0].count, 2);
        assert!(hits[0].snippet.to_lowercase().contains("needle"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn editor_blocks_preserve_rich_content_order() {
        let assets = vec![AssetEntry {
            id: 42,
            download_id: 7,
            asset_type: "image".to_string(),
            filename: "scene.webp".to_string(),
            local_path: "assets/scene.webp".to_string(),
            original_url: None,
            mime_type: Some("image/webp".to_string()),
            file_size_bytes: 128,
        }];
        let html = concat!(
            "<p>導入<br>二行目</p>",
            "<!-- newpage -->",
            "<h2>章題</h2>",
            "<img data-local-path=\"assets/scene.webp\" alt=\"挿絵\">",
            "<a class=\"novel-link-card\" href=\"https://example.com/story\">続きはこちら</a>",
            "<hr>",
            "<p>結び</p>"
        );

        let blocks = html_to_editor_blocks(html, &assets);

        assert_eq!(
            blocks
                .iter()
                .map(|block| block.block_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "paragraph",
                "page_break",
                "heading",
                "image",
                "link",
                "separator",
                "paragraph"
            ]
        );
        assert_eq!(blocks[0].text.as_deref(), Some("導入\n二行目"));
        assert_eq!(blocks[3].asset_id, Some(42));
        assert_eq!(blocks[3].text.as_deref(), Some("挿絵"));
        assert_eq!(blocks[4].text.as_deref(), Some("https://example.com/story"));
        assert!(blocks[4]
            .attrs_json
            .as_deref()
            .is_some_and(|attrs| attrs.contains("続きはこちら")));
        assert!(blocks_to_html(&blocks, &assets).contains("<!-- newpage -->"));
    }

    fn temp_paths() -> (PathBuf, PathBuf) {
        let rand_val: u32 = rand::random();
        let root = std::env::temp_dir().join(format!("piep_search_test_{}", rand_val));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        (root, storage)
    }

    fn params(query: &str) -> SearchV2Params {
        SearchV2Params {
            text: None,
            query: Some(query.to_string()),
            source: None,
            content_type: None,
            sort_by: Some("relevance".to_string()),
            sort_order: Some("desc".to_string()),
            limit: Some(20),
            cursor: None,
            favorite: None,
            tags_include: None,
            tags_exclude: None,
            tag_filter_mode: None,
            authors_include: None,
            authors_exclude: None,
            min_char_count: None,
            max_char_count: None,
            asset_filter: None,
            watch_filter: None,
            person_source: None,
            person_key: None,
            series_source: None,
            series_key: None,
            offset: None,
            ids_include: None,
            view_mode: None,
            projection: None,
            search_mode: None,
        }
    }

    fn v2_params(query: Option<&str>, limit: i64, cursor: Option<String>) -> SearchV2Params {
        SearchV2Params {
            text: None,
            query: query.map(str::to_string),
            source: None,
            content_type: None,
            sort_by: Some(if query.is_some() { "relevance" } else { "date" }.to_string()),
            sort_order: Some("desc".to_string()),
            limit: Some(limit),
            cursor,
            favorite: None,
            tags_include: None,
            tags_exclude: None,
            tag_filter_mode: None,
            authors_include: None,
            authors_exclude: None,
            min_char_count: None,
            max_char_count: None,
            asset_filter: None,
            watch_filter: None,
            person_source: None,
            person_key: None,
            series_source: None,
            series_key: None,
            offset: None,
            ids_include: None,
            view_mode: None,
            projection: None,
            search_mode: None,
        }
    }

    fn insert_download(
        db: &Database,
        storage: &Path,
        source_id: &str,
        title: &str,
        author: &str,
        tags: &[&str],
        body: &str,
    ) -> i64 {
        insert_download_with_reindex(
            db,
            storage,
            TestDownloadInput {
                source_id,
                title,
                author,
                tags,
                body,
                reindex: true,
            },
        )
    }

    fn insert_download_unindexed(
        db: &Database,
        storage: &Path,
        source_id: &str,
        title: &str,
        author: &str,
        tags: &[&str],
        body: &str,
    ) -> i64 {
        insert_download_with_reindex(
            db,
            storage,
            TestDownloadInput {
                source_id,
                title,
                author,
                tags,
                body,
                reindex: false,
            },
        )
    }

    struct TestDownloadInput<'a> {
        source_id: &'a str,
        title: &'a str,
        author: &'a str,
        tags: &'a [&'a str],
        body: &'a str,
        reindex: bool,
    }

    fn insert_download_with_reindex(
        db: &Database,
        storage: &Path,
        input: TestDownloadInput<'_>,
    ) -> i64 {
        let TestDownloadInput {
            source_id,
            title,
            author,
            tags,
            body,
            reindex,
        } = input;
        let dir = storage.join("pixiv").join(source_id).join("v1");
        fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("original.json");
        fs::write(
            &json_path,
            serde_json::json!({ "text": body }).to_string().as_bytes(),
        )
        .unwrap();
        let dl = NewDownload {
            source: "pixiv".to_string(),
            source_id: source_id.to_string(),
            title: title.to_string(),
            author_name: author.to_string(),
            author_id: format!("author-{}", source_id),
            content_type: "novel".to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            excerpt: Some("短い概要".to_string()),
            cover_path: None,
            json_path: json_path.to_string_lossy().to_string(),
            original_json_path: Some(json_path.to_string_lossy().to_string()),
            asset_count: 0,
            file_size_bytes: 0,
            downloaded_at: "2026-01-01T00:00:00Z".to_string(),
            source_created_at: Some("2026-01-01T00:00:00Z".to_string()),
            content_hash: Some(format!("hash-{}", source_id)),
            text_length: body.chars().count() as i64,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        };
        let id = db.upsert_download(&dl).unwrap();
        if reindex {
            db.reindex_download(id).unwrap();
        }
        id
    }

    #[test]
    fn sort_aliases_map_to_the_expected_safe_sql_columns() {
        let cases = [
            ("date", "date", "d.downloaded_at"),
            ("downloaded_at", "date", "d.downloaded_at"),
            ("author_name", "author", "d.author_name COLLATE NOCASE"),
            (
                "source_created_at",
                "published",
                "COALESCE(d.source_created_at, d.downloaded_at)",
            ),
            (
                "source_updated_at",
                "updated",
                "COALESCE(d.source_updated_at, d.source_created_at, d.downloaded_at)",
            ),
            ("text_length", "length", "d.text_length"),
            ("file_size_bytes", "size", "d.file_size_bytes"),
        ];

        for (requested, normalized, expected_sql) in cases {
            let mut search = params("");
            search.sort_by = Some(requested.to_string());
            assert_eq!(effective_sort_by(&search).as_deref(), Some(normalized));
            assert!(sort_clause(&search).contains(expected_sql));
            assert!(sort_compare_expr(&search).contains(expected_sql));
        }

        let mut malicious = params("");
        malicious.sort_by = Some("downloaded_at; DROP TABLE downloads".to_string());
        assert_eq!(effective_sort_by(&malicious).as_deref(), Some("date"));
        assert!(!sort_clause(&malicious).contains("DROP TABLE"));
    }

    #[test]
    fn optional_update_target_lookup_returns_exact_match_or_none() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        db.upsert_update_target(&UpdateTargetInput {
            target_type: "person".to_string(),
            source: "pixiv".to_string(),
            source_key: "author-1".to_string(),
            display_name: "作者1".to_string(),
            enabled: true,
            metadata_json: None,
        })
        .unwrap();

        let found = db
            .find_update_target("person", "pixiv", "author-1")
            .unwrap()
            .unwrap();
        assert_eq!(found.display_name, "作者1");
        assert!(found.enabled);
        assert!(db
            .find_update_target("person", "pixiv", "missing")
            .unwrap()
            .is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facet_search_limits_in_sql_and_keeps_direct_rare_matches() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        insert_download_unindexed(
            &db,
            &storage,
            "facet-1",
            "作品1",
            "人気作者",
            &["人気タグ"],
            "本文",
        );
        insert_download_unindexed(
            &db,
            &storage,
            "facet-2",
            "作品2",
            "作者%特別",
            &["希少タグ"],
            "本文",
        );

        assert_eq!(
            db.search_filter_facets("authors", None, 1).unwrap().len(),
            1
        );
        let escaped = db
            .search_filter_facets("authors", Some("作者%"), 10)
            .unwrap();
        assert_eq!(
            escaped.first().map(|facet| facet.name.as_str()),
            Some("作者%特別")
        );
        let rare = db.search_filter_facets("tags", Some("希少"), 10).unwrap();
        assert_eq!(
            rare.first().map(|facet| facet.name.as_str()),
            Some("希少タグ")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lexical_search_reports_full_hits_beyond_the_candidate_page() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..3 {
            insert_download(
                &db,
                &storage,
                &format!("count-{index}"),
                &format!("共通検索語 作品{index}"),
                "作者",
                &["検索"],
                "本文",
            );
        }

        let result =
            super::super::tantivy_index::search_with_total(&storage, "共通検索語", 1).unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.total_hits, 3);

        let mut deep = params("共通検索語");
        deep.limit = Some(600);
        assert!(search_candidate_limit(&deep, 600) > 1_000);
        let mut another = params("別の検索");
        another.limit = deep.limit;
        assert_ne!(search_cursor_scope(&deep), search_cursor_scope(&another));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lexical_cursor_reaches_every_match_beyond_one_thousand_results() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const ITEM_COUNT: usize = 1_025;

        for index in 0..ITEM_COUNT {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("deep-{index:04}"),
                &format!("全件到達テスト {index:04}"),
                "同一作者",
                &["ページング"],
                "全件到達テストの共通本文",
            );
        }
        loop {
            let status = db.rebuild_search_index_batch(200).unwrap();
            if status.pending_downloads == 0 {
                break;
            }
        }

        let mut request = params("全件到達テスト");
        request.limit = Some(137);
        let mut seen = HashSet::new();
        let mut checked_native_cursor = false;
        loop {
            let result = db.search_downloads_v2(&request).unwrap();
            assert_eq!(result.total_estimate, Some(ITEM_COUNT as i64));
            for item in result.items {
                assert!(
                    seen.insert(item.id),
                    "cursor returned duplicate id {}",
                    item.id
                );
            }
            match result.next_cursor {
                Some(cursor) => {
                    if !checked_native_cursor {
                        let decoded = decode_cursor(Some(&cursor)).unwrap();
                        assert_eq!(decoded.kind, "tantivy-search-after");
                        assert!(tantivy_cursor(&decoded).is_some());
                        checked_native_cursor = true;
                    }
                    request.cursor = Some(cursor);
                }
                None => break,
            }
        }

        assert!(checked_native_cursor);
        assert_eq!(seen.len(), ITEM_COUNT);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lexical_search_after_does_not_skip_filtered_hits_inside_a_batch() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const ITEM_COUNT: usize = 450;
        const EXPECTED: usize = 45;
        for index in 0..ITEM_COUNT {
            let tags: &[&str] = if index % 10 == 0 {
                &["対象"]
            } else {
                &["対象外"]
            };
            insert_download_unindexed(
                &db,
                &storage,
                &format!("filtered-cursor-{index:04}"),
                &format!("絞り込みカーソル {index:04}"),
                "カーソル作者",
                tags,
                "絞り込みカーソルの共通本文",
            );
        }
        loop {
            let status = db.rebuild_search_index_batch(200).unwrap();
            if status.pending_downloads == 0 {
                break;
            }
        }

        let mut request = params("絞り込みカーソル");
        request.limit = Some(17);
        request.tags_include = Some(vec!["対象".to_string()]);
        let mut seen = HashSet::new();
        loop {
            let result = db.search_downloads_v2(&request).unwrap();
            for item in result.items {
                assert!(seen.insert(item.id));
                assert!(item.tags.iter().any(|tag| tag == "対象"));
            }
            match result.next_cursor {
                Some(cursor) => request.cursor = Some(cursor),
                None => break,
            }
        }
        assert_eq!(seen.len(), EXPECTED);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_index_optimization_reduces_segments_without_changing_results() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..24 {
            insert_download(
                &db,
                &storage,
                &format!("merge-{index:02}"),
                &format!("索引統合検証 {index:02}"),
                "統合テスト作者",
                &["索引"],
                "索引統合検証の共通本文",
            );
        }
        let before_segments =
            super::super::tantivy_index::searchable_segment_count(&storage).unwrap();
        assert!(
            before_segments > 1,
            "test setup must create fragmented segments"
        );
        let mut search_params = params("索引統合検証");
        search_params.limit = Some(30);
        let before = db.search_downloads_v2(&search_params).unwrap();
        let before_ids = before
            .items
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>();
        assert_eq!(before_ids.len(), 24);

        let (reported_before, reported_after) =
            super::super::tantivy_index::optimize_segments(&storage).unwrap();
        assert_eq!(reported_before, before_segments);
        assert_eq!(reported_after, 1);
        let after = db.search_downloads_v2(&search_params).unwrap();
        let after_ids = after
            .items
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>();
        assert_eq!(after.total_estimate, before.total_estimate);
        assert_eq!(after_ids, before_ids);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn entity_facets_search_and_page_beyond_the_dashboard_cap() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        // get_filter_facets only ever returns the top 60 authors, so the
        // library tab needs a query that can reach past that.
        for index in 0..70 {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("{}", 100 + index),
                &format!("作品{}", index),
                &format!("作者{:02}", index),
                &["日常"],
                "本文",
            );
        }

        let capped = db.get_filter_facets().unwrap();
        assert_eq!(capped.author_entities.len(), 60);

        let second_page = db.search_entity_facets("person", None, 60, 60).unwrap();
        assert_eq!(second_page.len(), 10, "authors past the cap stay reachable");

        let filtered = db
            .search_entity_facets("person", Some("作者69"), 60, 0)
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].display_name, "作者69");
        assert_eq!(filtered[0].count, 1);

        let missing = db
            .search_entity_facets("person", Some("存在しない"), 60, 0)
            .unwrap();
        assert!(missing.is_empty());

        assert!(db.search_entity_facets("unknown", None, 10, 0).is_err());
    }

    #[test]
    fn smart_search_ranks_metadata_over_body_and_supports_body_search() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let body_only = insert_download(
            &db,
            &storage,
            "1",
            "静かな本文一致",
            "作者A",
            &["日常"],
            "ここだけに秘密キーワードが出てくる長い本文です。",
        );
        let title_hit = insert_download(
            &db,
            &storage,
            "2",
            "秘密キーワードのタイトル",
            "作者B",
            &["冒険"],
            "本文には別の内容を書いておく。",
        );

        let results = db
            .search_downloads_v2(&params("秘密キーワード"))
            .unwrap()
            .items;
        assert_eq!(results.first().map(|dl| dl.id), Some(title_hit));
        assert!(results.iter().any(|dl| dl.id == body_only));
        assert!(results
            .iter()
            .find(|dl| dl.id == body_only)
            .map(|dl| !dl.match_highlights.is_empty())
            .unwrap_or(false));

        let partial = db.search_downloads_v2(&params("秘密キ")).unwrap().items;
        assert!(partial.iter().any(|dl| dl.id == body_only));

        let excluded = db
            .search_downloads_v2(&params("秘密 -タイトル"))
            .unwrap()
            .items;
        assert!(excluded.iter().any(|dl| dl.id == body_only));
        assert!(!excluded.iter().any(|dl| dl.id == title_hit));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_v2_uses_cursor_without_duplicate_pages() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let first = insert_download(&db, &storage, "1", "一番目", "作者A", &["日常"], "本文A");
        let second = insert_download(&db, &storage, "2", "二番目", "作者B", &["日常"], "本文B");
        let third = insert_download(&db, &storage, "3", "三番目", "作者C", &["日常"], "本文C");

        let page1 = db.search_downloads_v2(&v2_params(None, 2, None)).unwrap();
        assert_eq!(page1.items.len(), 2);
        assert!(page1.next_cursor.is_some());
        assert!(page1.next_cursor.as_deref().unwrap().starts_with("k:"));

        let page2 = db
            .search_downloads_v2(&v2_params(None, 2, page1.next_cursor.clone()))
            .unwrap();
        let mut seen = page1.items.iter().map(|dl| dl.id).collect::<Vec<_>>();
        seen.extend(page2.items.iter().map(|dl| dl.id));
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3);
        assert!(seen.contains(&first));
        assert!(seen.contains(&second));
        assert!(seen.contains(&third));
        assert!(page2.next_cursor.is_none());

        let query = db
            .search_downloads_v2(&v2_params(Some("番目"), 10, None))
            .unwrap();
        assert_eq!(query.search_meta.engine, "hybrid-local");
        assert_eq!(query.items.len(), 3);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_suggest_returns_metadata_candidates() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        insert_download(
            &db,
            &storage,
            "suggest-1",
            "候補タイトル",
            "候補作者",
            &["候補タグ"],
            "本文",
        );

        let suggestions = db
            .search_suggest(&SearchSuggestParams {
                text: Some("候補".to_string()),
                limit: Some(10),
            })
            .unwrap();
        assert!(suggestions
            .items
            .iter()
            .any(|item| item.kind == "tag" && item.label == "候補タグ"));
        assert!(suggestions
            .items
            .iter()
            .any(|item| item.kind == "author" && item.label == "候補作者"));
        assert!(suggestions
            .items
            .iter()
            .any(|item| item.kind == "title" && item.label == "候補タイトル"));

        let exact = db
            .search_suggest(&SearchSuggestParams {
                text: Some("候補作者".to_string()),
                limit: Some(2),
            })
            .unwrap();
        assert!(exact.items.len() <= 2);
        assert_eq!(
            exact.items.first().map(|item| item.kind.as_str()),
            Some("author")
        );
        assert!(exact.items.first().is_some_and(|item| item.exact_match));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exact_author_intent_excludes_other_authors_that_only_mention_the_name() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download(
            &db,
            &storage,
            "exact-author-1",
            "作者本人の一作目",
            "明確な作者名",
            &["創作"],
            "本文",
        );
        let second = insert_download(
            &db,
            &storage,
            "exact-author-2",
            "作者本人の二作目",
            "明確な作者名",
            &["創作"],
            "本文",
        );
        insert_download(
            &db,
            &storage,
            "other-author",
            "言及だけの作品",
            "別の作者",
            &["評論"],
            "本文で明確な作者名について触れている",
        );

        let result = db.search_downloads_v2(&params("明確な作者名")).unwrap();
        let ids = result
            .items
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>();
        assert_eq!(ids, HashSet::from([first, second]));
        assert_eq!(result.search_meta.engine, "sqlite-exact-entity");
        let intent = result.search_meta.exact_entity.unwrap();
        assert_eq!(intent.kind, "author");
        assert!(intent.strict);
        assert!(result
            .search_meta
            .explanations
            .iter()
            .any(|line| line.contains("関係する作品だけ")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn series_token_filters_by_series_relation() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let in_series = insert_download(
            &db,
            &storage,
            "series-1",
            "シリーズ内作品",
            "作者A",
            &["連載"],
            "本文",
        );
        let outside = insert_download(
            &db,
            &storage,
            "series-2",
            "シリーズ外作品",
            "作者B",
            &["読切"],
            "本文",
        );
        db.upsert_download_series(in_series, "pixiv", "s-100", "構造化シリーズ", Some(1))
            .unwrap();

        let result = db
            .search_downloads_v2(&params("series:pixiv:s-100"))
            .unwrap();
        assert_eq!(
            result.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
            vec![in_series]
        );
        assert!(!result.items.iter().any(|dl| dl.id == outside));

        let by_title = db
            .search_downloads_v2(&params("series:\"構造化シリーズ\""))
            .unwrap();
        assert_eq!(
            by_title.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
            vec![in_series]
        );

        let legacy_suggestion_value = db.search_downloads_v2(&params("pixiv:s-100")).unwrap();
        assert_eq!(
            legacy_suggestion_value
                .items
                .iter()
                .map(|dl| dl.id)
                .collect::<Vec<_>>(),
            vec![in_series]
        );

        let exact_title = db.search_downloads_v2(&params("構造化シリーズ")).unwrap();
        assert_eq!(
            exact_title.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
            vec![in_series]
        );
        assert_eq!(
            exact_title
                .search_meta
                .exact_entity
                .as_ref()
                .map(|intent| intent.kind.as_str()),
            Some("series")
        );

        let suggestions = db
            .search_suggest(&SearchSuggestParams {
                text: Some("構造化".to_string()),
                limit: Some(10),
            })
            .unwrap();
        assert!(suggestions.items.iter().any(|item| {
            item.kind == "series"
                && item.label == "構造化シリーズ"
                && item.value == "pixiv:s-100"
                && item.source.as_deref() == Some("pixiv")
                && item.source_key.as_deref() == Some("s-100")
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn japanese_reading_kana_and_romaji_match_same_work() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "jp-reading-1",
            "小説テスト作品",
            "作者A",
            &["物語"],
            "本文にも小説という語を含みます。",
        );

        for query in ["てすと", "テスト", "tesuto", "しょうせつ", "shousetsu"] {
            let result = db.search_downloads_v2(&params(query)).unwrap();
            assert!(
                result.items.iter().any(|dl| dl.id == target),
                "query {query} should match the target"
            );
        }

        let romaji = db.search_downloads_v2(&params("shousetsu")).unwrap();
        let target_row = romaji.items.iter().find(|dl| dl.id == target).unwrap();
        assert!(target_row.score_reasons.iter().any(|reason| {
            matches!(
                reason.match_type.as_str(),
                "exact" | "reading" | "romaji" | "synonym"
            )
        }));
        assert!(!target_row
            .score_reasons
            .iter()
            .any(|reason| reason.match_type == "semantic"));
        assert!(!target_row.match_highlights.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn smart_search_does_not_add_semantic_reasons() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "smart-no-semantic-1",
            "小説テスト作品",
            "作者A",
            &["物語"],
            "本文にも小説という語を含みます。",
        );
        let result = db.search_downloads_v2(&params("novel")).unwrap();
        let target_row = result.items.iter().find(|dl| dl.id == target).unwrap();

        assert_eq!(result.search_meta.engine, "hybrid-local");
        assert!(!target_row
            .score_reasons
            .iter()
            .any(|reason| reason.match_type == "semantic"));
        assert!(!target_row
            .match_highlights
            .iter()
            .any(|highlight| highlight.match_type.as_deref() == Some("semantic")));

        let _ = fs::remove_dir_all(root);
    }

    /// Opens a copy of a real library and reports what survived.
    ///
    /// Schema recognition decides between "migrate in place" and "archive and
    /// start empty", and the tests around it necessarily use databases this
    /// code just created. This one can be pointed at a library saved by an
    /// earlier release, which is the case that actually goes wrong.
    #[test]
    #[ignore = "set PIEP_VERIFY_DB to a real library file"]
    fn opens_a_real_library_without_resetting_it() {
        let Ok(source) = std::env::var("PIEP_VERIFY_DB") else {
            panic!("set PIEP_VERIFY_DB to the database to check");
        };
        let source = PathBuf::from(source);
        let before: i64 = {
            let conn = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open source read-only");
            conn.query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
                .unwrap()
        };

        // Never open the real file for writing: work on a copy.
        let (root, storage) = temp_paths();
        let copy = root.join("piep.db");
        fs::copy(&source, &copy).expect("copy the library");

        let db = Database::open(&copy, &storage).expect("open the copied library");
        let after = db.get_search_index_status().unwrap();
        println!(
            "works before: {before} | after opening: {} | pending index: {}",
            after.total_downloads, after.pending_downloads
        );
        assert_eq!(
            after.total_downloads, before,
            "opening a saved library must never discard its works"
        );
        assert!(before > 0, "the source library should not be empty");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn index_status_is_cached_without_going_stale_across_changes() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        assert_eq!(db.get_search_index_status().unwrap().total_downloads, 0);

        // Reading it again immediately is served from the cache, and adding a
        // work has to drop that cache: a count the screens keep showing after
        // the library changed is worse than measuring it again.
        insert_download_unindexed(&db, &storage, "cache-1", "追加された作品", "作者", &["cache"], "本文");
        let after_insert = db.get_search_index_status().unwrap();
        assert_eq!(after_insert.total_downloads, 1);
        assert_eq!(after_insert.pending_downloads, 1);

        db.reindex_download(
            db.get_download_by_source("pixiv", "cache-1").unwrap().unwrap().id,
        )
        .unwrap();
        let after_index = db.get_search_index_status().unwrap();
        assert_eq!(after_index.pending_downloads, 0);
        assert!(after_index.is_complete);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_author_page_can_show_their_series_and_tags_and_drill_into_both() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let first = insert_download(&db, &storage, "auth-1", "序章", "青葉しおり", &["ファンタジー", "長編"], "本文");
        let second = insert_download(&db, &storage, "auth-2", "第二話", "青葉しおり", &["ファンタジー"], "本文");
        let standalone = insert_download(&db, &storage, "auth-3", "読み切り", "青葉しおり", &["短編"], "本文");
        let other = insert_download(&db, &storage, "auth-4", "別作者の話", "別の人", &["ファンタジー"], "本文");
        db.upsert_download_person(first, "pixiv", "aoba", "author", "青葉しおり").unwrap();
        db.upsert_download_person(second, "pixiv", "aoba", "author", "青葉しおり").unwrap();
        db.upsert_download_person(standalone, "pixiv", "aoba", "author", "青葉しおり").unwrap();
        db.upsert_download_person(other, "pixiv", "hoka", "author", "別の人").unwrap();
        db.upsert_download_series(first, "pixiv", "s1", "季節の栞", Some(1)).unwrap();
        db.upsert_download_series(second, "pixiv", "s1", "季節の栞", Some(2)).unwrap();

        let series = db.list_entity_series("pixiv", "aoba", 20).unwrap();
        assert_eq!(series.len(), 1, "only the series this author appears in");
        assert_eq!(series[0].display_name, "季節の栞");
        assert_eq!(series[0].count, 2);

        let tags = db.list_entity_tags("person", "pixiv", "aoba", 20).unwrap();
        let named = tags.iter().map(|t| (t.name.as_str(), t.count)).collect::<Vec<_>>();
        assert_eq!(named, vec![("ファンタジー", 2), ("短編", 1), ("長編", 1)],
            "the author's own tags, most used first, without the other author's");

        // Drilling in: the author's works carrying one of their tags. Author and
        // tag filters have to compose, or a tag on this page cannot be followed.
        let mut params = v2_params(None, 20, None);
        params.person_source = Some("pixiv".to_string());
        params.person_key = Some("aoba".to_string());
        params.tags_include = Some(vec!["ファンタジー".to_string()]);
        let page = db.search_downloads_v2(&params).unwrap();
        let ids = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&first) && ids.contains(&second));
        assert!(!ids.contains(&other), "another author's work with the same tag must not appear");

        // An author with nothing recorded is an empty page, not an error.
        assert!(db.list_entity_series("pixiv", "unknown", 20).unwrap().is_empty());
        assert!(db.list_entity_tags("person", "pixiv", "unknown", 20).unwrap().is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn numbered_pages_work_for_an_ordering_and_are_refused_for_relevance() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..7 {
            insert_download(&db, &storage, &format!("page-{index}"), &format!("作品{index}"), "作者", &["頁"], "本文");
        }

        let page_of = |offset: i64| {
            let mut params = v2_params(None, 3, None);
            params.sort_by = Some("title".to_string());
            params.sort_order = Some("asc".to_string());
            params.offset = Some(offset);
            db.search_downloads_v2(&params)
                .unwrap()
                .items
                .iter()
                .map(|item| item.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(page_of(0), vec!["作品0", "作品1", "作品2"]);
        assert_eq!(page_of(3), vec!["作品3", "作品4", "作品5"], "the third page, without walking to it");
        assert_eq!(page_of(6), vec!["作品6"]);
        assert!(page_of(99).is_empty(), "past the end is empty, not an error");

        // Relevance has no nth page: results are walked with a score cursor, so
        // an offset into them would not be the page that was asked for.
        let mut relevance = params("作品");
        relevance.offset = Some(3);
        let first = relevance.clone();
        let mut without = first.clone();
        without.offset = None;
        assert_eq!(
            db.search_downloads_v2(&relevance).unwrap().items.len(),
            db.search_downloads_v2(&without).unwrap().items.len(),
            "an offset must be ignored rather than silently shifting a relevance page"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn shelf_counts_ignore_reading_positions_for_works_that_no_longer_exist() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download(&db, &storage, "shelf-1", "一作目", "作者", &["棚"], "本文");
        let second = insert_download(&db, &storage, "shelf-2", "二作目", "作者", &["棚"], "本文");
        insert_download(&db, &storage, "shelf-3", "三作目", "作者", &["棚"], "本文");
        db.set_favorite(first, true).unwrap();
        db.set_watch_updates(second, true).unwrap();

        let empty = db.get_library_shelf_counts(&[]).unwrap();
        assert_eq!((empty.total, empty.favorite, empty.watched, empty.reading), (3, 1, 1, 0));

        // Reading positions are per device and nothing prunes them, so a shelf
        // must count works that exist, not entries that were left behind.
        let counts = db
            .get_library_shelf_counts(&[first, second, 999_999, first])
            .unwrap();
        assert_eq!(counts.reading, 2, "a deleted work and a duplicate must not be counted");

        // The same list used as a filter returns exactly those works.
        let mut params = v2_params(None, 20, None);
        params.ids_include = Some(vec![first, 999_999]);
        let page = db.search_downloads_v2(&params).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, first);

        // An empty membership list means an empty shelf, never the whole library.
        let mut none = v2_params(None, 20, None);
        none.ids_include = Some(Vec::new());
        assert_eq!(db.search_downloads_v2(&none).unwrap().items.len(), 0);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn saved_searches_survive_reuse_of_a_name_and_reject_nonsense() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let created = db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "  長編ファンタジー  ".to_string(),
                query: Some("tag:ファンタジー".to_string()),
                params_json: "{\"tagsInclude\":[\"ファンタジー\"]}".to_string(),
            })
            .unwrap();
        assert_eq!(created.name, "長編ファンタジー", "the name is stored trimmed");

        // Saving again under the same name replaces that search. The unique
        // constraint must not surface as an error the reader has to decode.
        let replaced = db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "長編ファンタジー".to_string(),
                query: Some("tag:ファンタジー -短編".to_string()),
                params_json: "{\"tagsExclude\":[\"短編\"]}".to_string(),
            })
            .unwrap();
        assert_eq!(replaced.id, created.id);
        assert_eq!(db.list_saved_searches().unwrap().len(), 1);
        assert_eq!(replaced.query.as_deref(), Some("tag:ファンタジー -短編"));

        assert!(db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "   ".to_string(),
                query: None,
                params_json: "{}".to_string(),
            })
            .is_err(), "a blank name is not a search anyone can find again");
        assert!(db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "壊れた条件".to_string(),
                query: None,
                params_json: "{not json".to_string(),
            })
            .is_err(), "conditions that cannot be read back must not be stored");
        assert!(db
            .upsert_saved_search(&SavedSearchInput {
                id: Some(4_242),
                name: "存在しない".to_string(),
                query: None,
                params_json: "{}".to_string(),
            })
            .is_err(), "updating a search that was deleted elsewhere must say so");

        assert!(db.delete_saved_search(created.id).unwrap());
        assert!(!db.delete_saved_search(created.id).unwrap(), "a second delete is not an error");
        assert!(db.list_saved_searches().unwrap().is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn saved_searches_keep_their_order_and_stop_at_the_limit() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..MAX_SAVED_SEARCHES {
            db.upsert_saved_search(&SavedSearchInput {
                id: None,
                name: format!("検索{index:03}"),
                query: None,
                params_json: "{}".to_string(),
            })
            .unwrap();
        }
        let listed = db.list_saved_searches().unwrap();
        assert_eq!(listed.len() as i64, MAX_SAVED_SEARCHES);
        assert_eq!(listed[0].name, "検索000", "the sidebar order must be stable");
        assert_eq!(listed[listed.len() - 1].name, "検索099");

        let overflow = db.upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "一件多い".to_string(),
            query: None,
            params_json: "{}".to_string(),
        });
        assert!(overflow.is_err(), "the limit must be refused, not silently applied");
        // Replacing an existing one still works when the list is full.
        assert!(db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "検索000".to_string(),
                query: Some("更新".to_string()),
                params_json: "{}".to_string(),
            })
            .is_ok());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn synonyms_do_not_match_every_work_through_its_source_url() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        // Every pixiv URL is .../novel/show.php?id=N, and the synonym table
        // expands 物語 to "novel". Neither of these works mentions 物語.
        insert_download(&db, &storage, "111", "海辺の記録", "作者A", &["日常"], "静かな本文です");
        insert_download(&db, &storage, "222", "灯台の手紙", "作者B", &["日常"], "別の本文です");
        let about = insert_download(&db, &storage, "333", "夜明けの物語", "作者C", &["創作"], "本文");

        let hits = db.search_downloads_v2(&params("物語")).unwrap();
        let ids = hits.items.iter().map(|item| item.id).collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![about],
            "only the work that actually says 物語 should match"
        );

        // The URL itself is still findable: pasting one has to reach its work.
        let pasted = db
            .search_downloads_v2(&params("https://www.pixiv.net/novel/show.php?id=111"))
            .unwrap();
        assert!(
            pasted.items.iter().any(|item| item.source_id == "111"),
            "a pasted source URL must still find its work"
        );
        // As must the bare id, which is what the URL actually identifies.
        let by_id = db.search_downloads_v2(&params("222")).unwrap();
        assert!(by_id.items.iter().any(|item| item.source_id == "222"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changing_the_index_format_requeues_the_whole_library() {
        let (root, storage) = temp_paths();
        let db_path = root.join("piep.db");
        let db = Database::open(&db_path, &storage).unwrap();
        insert_download(&db, &storage, "fmt-1", "書式変更の作品", "作者", &["書式"], "本文");
        assert_eq!(db.get_search_index_status().unwrap().pending_downloads, 0);
        drop(db);

        // Stand in for a release that changes the on-disk index layout: the
        // bookkeeping now describes an index the app no longer reads.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO search_index_meta (id, index_version, updated_at)
                 VALUES (1, 'v-previous', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }

        let db = Database::open(&db_path, &storage).unwrap();
        assert_eq!(
            db.get_search_index_status().unwrap().pending_downloads,
            1,
            "a format change must queue the library for reindexing, not report it as complete"
        );

        // What the app does at launch: notice the backlog and clear it without
        // being asked. Nothing here should need a person to press anything.
        let outcome = db
            .rebuild_search_index(SearchIndexRebuildOptions::default(), &|| false, |_| {})
            .unwrap();
        assert_eq!(outcome.processed, 1);
        assert!(!outcome.canceled);
        let status = db.get_search_index_status().unwrap();
        assert_eq!(status.pending_downloads, 0);
        assert!(status.is_complete);
        // And the work is genuinely searchable again, not merely marked done.
        let found = db.search_downloads_v2(&params("書式変更")).unwrap();
        assert_eq!(found.items.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sorted_search_orders_matches_by_column_and_pages_without_gaps() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        // Titles are deliberately out of insertion order so a title sort cannot
        // pass by accident. The shared token avoids the synonym table, which
        // would otherwise pull in unrelated works through their source URL.
        for (source_id, title) in [
            ("sorted-1", "さくら花暦"),
            ("sorted-2", "あおぞら花暦"),
            ("sorted-3", "なのはな花暦"),
            ("sorted-4", "かえで花暦"),
            ("sorted-5", "たんぽぽ花暦"),
        ] {
            insert_download(&db, &storage, source_id, title, "並び替え作者", &["並替"], "共通の本文です");
        }
        // A work that must never appear: it does not match the query.
        insert_download(&db, &storage, "sorted-x", "無関係な作品", "別作者", &["他"], "別の本文");

        let mut sorted = params("花暦");
        sorted.sort_by = Some("title".to_string());
        sorted.sort_order = Some("asc".to_string());
        sorted.limit = Some(2);

        let mut seen = Vec::new();
        let mut cursor = None;
        for _ in 0..5 {
            let mut page_params = sorted.clone();
            page_params.cursor = cursor.clone();
            let page = db.search_downloads_v2(&page_params).unwrap();
            assert_eq!(page.search_meta.engine, "tantivy-sorted");
            assert_eq!(page.total_estimate, Some(5));
            seen.extend(page.items.iter().map(|item| item.title.clone()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        assert_eq!(
            seen,
            vec![
                "あおぞら花暦",
                "かえで花暦",
                "さくら花暦",
                "たんぽぽ花暦",
                "なのはな花暦",
            ],
            "every match should appear exactly once, in title order"
        );

        // Without an explicit sort the same query still ranks by relevance.
        let relevance = db.search_downloads_v2(&params("花暦")).unwrap();
        assert_ne!(relevance.search_meta.engine, "tantivy-sorted");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sorted_search_respects_library_filters() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let favorite = insert_download(&db, &storage, "filter-1", "対象作品 花暦", "作者", &["絞込"], "本文");
        insert_download(&db, &storage, "filter-2", "対象外作品 花暦", "作者", &["絞込"], "本文");
        db.set_favorite(favorite, true).unwrap();

        let mut sorted = params("花暦");
        sorted.sort_by = Some("downloaded_at".to_string());
        sorted.favorite = Some(true);
        let page = db.search_downloads_v2(&sorted).unwrap();
        assert_eq!(page.total_estimate, Some(1));
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, favorite);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn multi_term_search_requires_each_term() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "multi-term-1",
            "alpha beta target",
            "作者A",
            &["mixed"],
            "本文",
        );
        let alpha_only = insert_download(
            &db,
            &storage,
            "multi-term-2",
            "alpha only",
            "作者B",
            &["mixed"],
            "本文",
        );
        let beta_only = insert_download(
            &db,
            &storage,
            "multi-term-3",
            "beta only",
            "作者C",
            &["mixed"],
            "本文",
        );

        let result = db.search_downloads_v2(&params("alpha beta")).unwrap();
        assert!(result.items.iter().any(|item| item.id == target));
        assert!(!result.items.iter().any(|item| item.id == alpha_only));
        assert!(!result.items.iter().any(|item| item.id == beta_only));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn semantic_mode_returns_body_chunk_highlight_and_reason() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "semantic-1",
            "静かな作品",
            "作者A",
            &["日常"],
            "これは長い小説本文です。読者が物語を探すときに本文チャンクで見つかります。",
        );
        let mut semantic_params = params("novel");
        semantic_params.search_mode = Some("semantic".to_string());

        let result = db.search_downloads_v2(&semantic_params).unwrap();
        let target_row = result.items.iter().find(|dl| dl.id == target).unwrap();
        assert!(target_row
            .score_reasons
            .iter()
            .any(|reason| reason.match_type == "semantic"));
        assert!(target_row.match_highlights.iter().any(|highlight| {
            highlight.match_type.as_deref() == Some("semantic")
                && highlight
                    .segments
                    .iter()
                    .any(|segment| segment.matched && segment.text.contains("小説本文"))
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "performance smoke test for large-library browsing"]
    fn library_browsing_stays_fast_on_a_large_library() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        const SEEDED: usize = 20_000;
        for index in 0..SEEDED {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("scale-{}", index),
                &format!("蔵書 {:05}", index),
                &format!("作者 {:03}", index % 400),
                &[&format!("tag{}", index % 30)],
                "大規模ライブラリの一覧性能を確認するための本文です。",
            );
        }

        // The library browses with an empty query, which is the keyset path.
        let mut first = v2_params(None, 60, None);
        first.projection = Some("libraryGallery".to_string());
        let started = Instant::now();
        let page1 = db.search_downloads_v2(&first).unwrap();
        let first_elapsed = started.elapsed();
        assert_eq!(page1.items.len(), 60);
        assert_eq!(page1.total_estimate, Some(SEEDED as i64));
        assert!(
            page1.next_cursor.is_some(),
            "a large library must expose more pages"
        );

        // Walk deep into the list: keyset paging must not degrade with depth.
        let mut cursor = page1.next_cursor.clone();
        let mut deepest = Duration::ZERO;
        for _ in 0..40 {
            let mut page_params = v2_params(None, 60, cursor.clone());
            page_params.projection = Some("libraryGallery".to_string());
            let started = Instant::now();
            let page = db.search_downloads_v2(&page_params).unwrap();
            deepest = deepest.max(started.elapsed());
            cursor = page.next_cursor.clone();
            assert!(cursor.is_some(), "paging ended earlier than expected");
        }

        let started = Instant::now();
        let authors = db.search_entity_facets("person", None, 60, 300).unwrap();
        let entity_elapsed = started.elapsed();
        assert_eq!(authors.len(), 60);

        let started = Instant::now();
        let facets = db.get_filter_facets_with(false).unwrap();
        let facet_elapsed = started.elapsed();
        assert!(!facets.tags.is_empty());
        assert!(
            facets.author_entities.is_empty(),
            "the light variant must skip the entity aggregates"
        );

        eprintln!(
            "{} works: first page {:?}, deepest page {:?}, authors {:?}, filter options {:?}",
            SEEDED, first_elapsed, deepest, entity_elapsed, facet_elapsed
        );
        assert!(
            first_elapsed < Duration::from_millis(400),
            "first page took {:?}",
            first_elapsed
        );
        assert!(
            deepest < Duration::from_millis(400),
            "deep page took {:?}",
            deepest
        );
        // Without idx_downloads_author_recent this listing takes ~1.9s here.
        assert!(
            entity_elapsed < Duration::from_millis(300),
            "author listing took {:?}",
            entity_elapsed
        );
        assert!(
            facet_elapsed < Duration::from_millis(500),
            "filter options took {:?}",
            facet_elapsed
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "performance smoke test for local search tuning"]
    fn smart_search_handles_5000_seed_items_under_target() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        for index in 0..5_000 {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("perf-{}", index),
                &format!("Seed Work {:04}", index),
                &format!("Seed Author {:02}", index % 50),
                &[&format!("seed{}", index % 25)],
                &format!(
                    "検索性能検証用の本文です。番号 {}、タグ seed{}。日本語とASCII mixed text for search.",
                    index,
                    index % 25
                ),
            );
        }
        loop {
            let status = db.rebuild_search_index_batch(200).unwrap();
            if status.pending_downloads == 0 {
                break;
            }
        }

        let mut search_params = params("検索性能 seed7");
        search_params.limit = Some(120);
        let started = Instant::now();
        let result = db.search_downloads_v2(&search_params).unwrap();
        let elapsed = started.elapsed();
        eprintln!("5000-item smart search: {elapsed:?}");

        assert!(!result.items.is_empty());
        assert!(
            elapsed < Duration::from_secs(1),
            "smart search took {:?}",
            elapsed
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn projection_selects_do_not_include_unneeded_subqueries() {
        let bulk = download_select_sql_for_projection(Some("bulk"), "NULL", "NULL");
        assert!(!bulk.contains("download_tags"));
        assert!(!bulk.contains("download_people"));
        assert!(!bulk.contains("download_series"));

        let entity = download_select_sql_for_projection(Some("entityFacet"), "NULL", "NULL");
        assert!(!entity.contains("download_tags"));
        assert!(!entity.contains("download_people"));
        assert!(!entity.contains("download_series"));
        assert!(entity.contains("d.cover_path"));

        // The list view shows a tag column, so compact now pays for that
        // lookup; the excerpt is still the one thing it does not read.
        let compact = download_select_sql_for_projection(Some("libraryCompact"), "NULL", "NULL");
        assert!(compact.contains("download_tags"));
        assert!(compact.contains("download_people"));
        assert!(compact.contains("download_series"));
        assert!(compact.contains("NULL AS excerpt"));

        let gallery = download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL");
        assert!(gallery.contains("download_tags"));
        assert!(gallery.contains("download_people"));
        assert!(gallery.contains("download_series"));
    }

    #[test]
    fn active_edit_revision_drives_reader_and_search_body() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let download_id = insert_download(
            &db,
            &storage,
            "edit-1",
            "編集対象",
            "作者A",
            &["編集"],
            "原本だけにある本文です。",
        );

        let initial_reader = db.get_reader_document(download_id, None).unwrap();
        assert!(!initial_reader.is_edited);
        assert!(initial_reader.plain_text.contains("原本だけ"));

        let draft = db
            .save_work_draft(
                download_id,
                1,
                &[
                    WorkBlockInput {
                        block_type: "heading".to_string(),
                        text: Some("編集版見出し".to_string()),
                        asset_id: None,
                        attrs_json: None,
                    },
                    WorkBlockInput {
                        block_type: "paragraph".to_string(),
                        text: Some("編集固有キーワードを含む本文です。".to_string()),
                        asset_id: None,
                        attrs_json: None,
                    },
                ],
            )
            .unwrap();
        db.activate_work_edit(draft.id).unwrap();

        let edited_reader = db.get_reader_document(download_id, None).unwrap();
        assert!(edited_reader.is_edited);
        assert!(edited_reader.html.contains("編集版見出し"));
        assert!(edited_reader.plain_text.contains("編集固有キーワード"));

        let search = db
            .search_downloads_v2(&v2_params(Some("編集固有キーワード"), 10, None))
            .unwrap();
        assert_eq!(search.items.first().map(|item| item.id), Some(download_id));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_job_schema_recovers_interrupted_jobs() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "work".to_string(),
            mode: "auto_save".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            concurrency: None,
        };
        db.create_update_job(
            "job-test",
            &request,
            &[UpdateJobItemInput {
                item_type: "work".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("1".to_string()),
                target_type: Some("work".to_string()),
                title: "Test".to_string(),
                payload_json: "{}".to_string(),
                status: "queued".to_string(),
            }],
        )
        .unwrap();
        db.set_update_job_status("job-test", "running", Some("running"))
            .unwrap();
        db.recover_update_jobs_on_startup().unwrap();
        let snapshot = db.update_job_snapshot("job-test").unwrap();
        assert_eq!(snapshot.status, "paused");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_job_candidates_can_be_queued_for_saving() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "author".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            concurrency: None,
        };
        db.create_update_job(
            "job-candidates",
            &request,
            &[UpdateJobItemInput {
                item_type: "target".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("user-1".to_string()),
                target_type: Some("author".to_string()),
                title: "Author".to_string(),
                payload_json: "{}".to_string(),
                status: "done".to_string(),
            }],
        )
        .unwrap();
        db.insert_update_job_candidate(
            "job-candidates",
            &UpdateJobItemInput {
                item_type: "candidate".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("novel-1".to_string()),
                target_type: Some("author".to_string()),
                title: "Novel".to_string(),
                payload_json: serde_json::json!({
                    "targetLabel": "Author",
                    "subtitle": "Author / now"
                })
                .to_string(),
                status: "candidate".to_string(),
            },
        )
        .unwrap();
        let snapshot = db.update_job_snapshot("job-candidates").unwrap();
        let candidate_id = snapshot.candidates[0].id;
        let changed = db
            .queue_update_job_candidates("job-candidates", &[candidate_id])
            .unwrap();
        assert_eq!(changed, 1);
        let snapshot = db.update_job_snapshot("job-candidates").unwrap();
        assert_eq!(snapshot.candidates[0].status, "queued");

        let _ = fs::remove_dir_all(root);
    }
}
