//! データベースCRUD操作。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use regex::Regex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
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

/// 作品コレクションは `Database` の非公開内部へ触れるため、子モジュールに置く。
mod collections;

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
const UPDATE_SNAPSHOT_CANDIDATE_PAGE_SIZE: i64 = 200;
const UPDATE_SNAPSHOT_LOG_PAGE_SIZE: i64 = 300;
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
    let mmap_size = super::resource_budget::sqlite_mmap_bytes() as i64;
    let cache_kib = (super::resource_budget::sqlite_cache_bytes() / 1024) as i64;
    let manager = SqliteConnectionManager::file(db_path)
        .with_flags(flags)
        .with_init(move |conn| {
            conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
            conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;
                PRAGMA temp_store = MEMORY;
                ",
            )?;
            conn.pragma_update(None, "mmap_size", mmap_size)?;
            conn.pragma_update(None, "cache_size", -cache_kib)
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

#[cfg(test)]
fn normalized_diagnostic_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

const DIAGNOSTIC_SCAN_MAX_DEPTH: usize = 64;
const DIAGNOSTIC_SCAN_MAX_ENTRIES: usize = 5_000_000;
const DIAGNOSTIC_MAX_FILE_REFERENCES: u64 = 20_000_000;
const DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT: usize = 100;

#[derive(Debug, Default, PartialEq, Eq)]
struct LibraryFileIntegrity {
    checked_file_references: u64,
    missing_json_files: u64,
    missing_asset_files: u64,
    missing_profile_files: u64,
    unsafe_referenced_files: u64,
    unreadable_referenced_files: u64,
    empty_referenced_files: u64,
    mismatched_asset_files: u64,
    issue_samples: Vec<LibraryFileIssue>,
}

fn diagnostic_reference_category(kind: i64) -> &'static str {
    match kind {
        0 => "work_json",
        1 => "work_asset",
        2 => "profile",
        3 => "entity_json",
        _ => "unknown",
    }
}

fn bounded_diagnostic_path(path: &str) -> String {
    const MAX_CHARS: usize = 2_048;
    if path.chars().count() <= MAX_CHARS {
        return path.to_string();
    }
    let mut bounded = path.chars().take(MAX_CHARS).collect::<String>();
    bounded.push('…');
    bounded
}

fn record_file_issue(
    integrity: &mut LibraryFileIntegrity,
    issue_type: &str,
    kind: i64,
    path: &str,
    label: Option<&str>,
    expected_size: Option<i64>,
    actual_size: Option<u64>,
) {
    if integrity.issue_samples.len() >= DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT {
        return;
    }
    integrity.issue_samples.push(LibraryFileIssue {
        issue_type: issue_type.to_string(),
        category: diagnostic_reference_category(kind).to_string(),
        path: bounded_diagnostic_path(path),
        label: label.map(|value| value.chars().take(300).collect()),
        expected_size_bytes: expected_size.and_then(|size| u64::try_from(size).ok()),
        actual_size_bytes: actual_size,
    });
}

fn count_missing_reference(integrity: &mut LibraryFileIntegrity, kind: i64) {
    match kind {
        0 => integrity.missing_json_files = integrity.missing_json_files.saturating_add(1),
        1 => integrity.missing_asset_files = integrity.missing_asset_files.saturating_add(1),
        _ => {
            integrity.missing_profile_files = integrity.missing_profile_files.saturating_add(1);
        }
    }
}

/// Compare every path stored in SQLite with the actual storage tree without
/// collecting all paths in RAM. `kind` is 0 for JSON, 1 for work assets, and
/// 2 for profile/series images. Duplicate DB references are intentionally
/// counted as references: this keeps the query streaming and makes it clear
/// how many rows need repair when one shared file disappears.
fn check_library_file_integrity(
    conn: &Connection,
    storage_root: &Path,
    app_data_root: &Path,
) -> Result<LibraryFileIntegrity, String> {
    let canonical_storage = storage_root
        .canonicalize()
        .map_err(|error| format!("Storage path resolution failed: {error}"))?;
    let canonical_app_data = app_data_root
        .canonicalize()
        .map_err(|error| format!("App data path resolution failed: {error}"))?;
    let canonical_profiles = canonical_app_data.join("profiles");
    let canonical_series = canonical_app_data.join("series");
    let mut statement = conn
        .prepare(
            "SELECT kind, path, expected_size, label FROM (
                SELECT 0 AS kind, d.json_path AS path, NULL AS expected_size,
                       d.title AS label FROM downloads d
                UNION ALL SELECT 0, d.original_json_path, NULL, d.title FROM downloads d
                    WHERE d.original_json_path IS NOT NULL AND TRIM(d.original_json_path) != ''
                UNION ALL SELECT 1, d.cover_path, NULL, d.title FROM downloads d
                    WHERE d.cover_path IS NOT NULL AND TRIM(d.cover_path) != ''
                UNION ALL SELECT 0, v.json_path, NULL, d.title
                    FROM download_versions v JOIN downloads d ON d.id = v.download_id
                UNION ALL SELECT 0, v.original_json_path, NULL, d.title
                    FROM download_versions v JOIN downloads d ON d.id = v.download_id
                    WHERE v.original_json_path IS NOT NULL AND TRIM(v.original_json_path) != ''
                UNION ALL SELECT 1, a.local_path, a.file_size_bytes, d.title
                    FROM assets a JOIN downloads d ON d.id = a.download_id
                UNION ALL SELECT 2, p.icon_path, NULL, p.display_name FROM people p
                    WHERE p.icon_path IS NOT NULL AND TRIM(p.icon_path) != ''
                UNION ALL SELECT 2, p.cover_path, NULL, p.display_name FROM people p
                    WHERE p.cover_path IS NOT NULL AND TRIM(p.cover_path) != ''
                UNION ALL SELECT 2, s.cover_path, NULL, s.title FROM series s
                    WHERE s.cover_path IS NOT NULL AND TRIM(s.cover_path) != ''
                UNION ALL SELECT 3, e.json_path, NULL,
                    e.entity_type || ' ' || e.source || ':' || e.source_key
                    FROM entity_versions e
             ) WHERE path IS NOT NULL AND TRIM(path) != ''",
        )
        .map_err(|error| format!("File integrity query prepare failed: {error}"))?;
    let mut rows = statement
        .query([])
        .map_err(|error| format!("File integrity query failed: {error}"))?;
    let mut integrity = LibraryFileIntegrity::default();

    while let Some(row) = rows
        .next()
        .map_err(|error| format!("File integrity row failed: {error}"))?
    {
        integrity.checked_file_references = integrity.checked_file_references.saturating_add(1);
        if integrity.checked_file_references > DIAGNOSTIC_MAX_FILE_REFERENCES {
            return Err(format!(
                "File integrity check exceeded the {}-reference safety limit",
                DIAGNOSTIC_MAX_FILE_REFERENCES
            ));
        }

        let kind = row
            .get::<_, i64>(0)
            .map_err(|error| format!("File integrity kind failed: {error}"))?;
        let stored_path = row
            .get::<_, String>(1)
            .map_err(|error| format!("File integrity path failed: {error}"))?;
        let expected_size = row
            .get::<_, Option<i64>>(2)
            .map_err(|error| format!("File integrity size failed: {error}"))?;
        let label = row
            .get::<_, Option<String>>(3)
            .map_err(|error| format!("File integrity label failed: {error}"))?;
        if stored_path.len() > 32_768 {
            integrity.unsafe_referenced_files = integrity.unsafe_referenced_files.saturating_add(1);
            record_file_issue(
                &mut integrity,
                "unsafe",
                kind,
                &stored_path,
                label.as_deref(),
                expected_size,
                None,
            );
            continue;
        }
        let path = Path::new(&stored_path);
        if !path.is_absolute() {
            integrity.unsafe_referenced_files = integrity.unsafe_referenced_files.saturating_add(1);
            record_file_issue(
                &mut integrity,
                "unsafe",
                kind,
                &stored_path,
                label.as_deref(),
                expected_size,
                None,
            );
            continue;
        }
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                count_missing_reference(&mut integrity, kind);
                record_file_issue(
                    &mut integrity,
                    "missing",
                    kind,
                    &stored_path,
                    label.as_deref(),
                    expected_size,
                    None,
                );
                continue;
            }
            Err(_) => {
                integrity.unreadable_referenced_files =
                    integrity.unreadable_referenced_files.saturating_add(1);
                record_file_issue(
                    &mut integrity,
                    "unreadable",
                    kind,
                    &stored_path,
                    label.as_deref(),
                    expected_size,
                    None,
                );
                continue;
            }
        };
        if diagnostic_metadata_is_link(&metadata) || !metadata.file_type().is_file() {
            integrity.unsafe_referenced_files = integrity.unsafe_referenced_files.saturating_add(1);
            record_file_issue(
                &mut integrity,
                "unsafe",
                kind,
                &stored_path,
                label.as_deref(),
                expected_size,
                Some(metadata.len()),
            );
            continue;
        }
        let canonical_path = match path.canonicalize() {
            Ok(path)
                if match kind {
                    // Work JSON and assets live below downloads/.
                    0 | 1 => path.starts_with(&canonical_storage),
                    // 作者・シリーズの画像は三か所に正当に置かれる。取ってきた
                    // 横顔は profiles/ と series/ の下だが、シリーズの表紙は
                    // 最初に保存した作品の表紙そのもの - つまり downloads/ の
                    // 下を指す。ここを downloads/ 抜きで見ていたころは、その
                    // 1件1件が「許可領域外」として並び、直しようのない警告に
                    // なっていた。
                    2 => {
                        path.starts_with(&canonical_profiles)
                            || path.starts_with(&canonical_series)
                            || path.starts_with(&canonical_storage)
                    }
                    // 版のJSONは、アプリが自分で書く場所しか指さない。
                    3 => {
                        path.starts_with(&canonical_profiles) || path.starts_with(&canonical_series)
                    }
                    _ => false,
                } =>
            {
                path
            }
            _ => {
                integrity.unsafe_referenced_files =
                    integrity.unsafe_referenced_files.saturating_add(1);
                record_file_issue(
                    &mut integrity,
                    "unsafe",
                    kind,
                    &stored_path,
                    label.as_deref(),
                    expected_size,
                    Some(metadata.len()),
                );
                continue;
            }
        };
        // Keep the resolved path alive through all checks. Besides making the
        // containment decision explicit, this prevents a future refactor from
        // accidentally comparing the unchecked input path below.
        let _ = canonical_path;
        if metadata.len() == 0 {
            integrity.empty_referenced_files = integrity.empty_referenced_files.saturating_add(1);
            record_file_issue(
                &mut integrity,
                "empty",
                kind,
                &stored_path,
                label.as_deref(),
                expected_size,
                Some(0),
            );
        }
        if kind == 1
            && expected_size.is_some_and(|expected| {
                expected > 0 && u64::try_from(expected).ok() != Some(metadata.len())
            })
        {
            integrity.mismatched_asset_files = integrity.mismatched_asset_files.saturating_add(1);
            record_file_issue(
                &mut integrity,
                "size_mismatch",
                kind,
                &stored_path,
                label.as_deref(),
                expected_size,
                Some(metadata.len()),
            );
        }
    }
    Ok(integrity)
}

#[derive(Debug, Clone, Copy)]
struct DiagnosticScanLimits {
    max_depth: usize,
    max_entries: usize,
}

impl Default for DiagnosticScanLimits {
    fn default() -> Self {
        Self {
            max_depth: DIAGNOSTIC_SCAN_MAX_DEPTH,
            max_entries: DIAGNOSTIC_SCAN_MAX_ENTRIES,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DiagnosticFileStats {
    storage_size_bytes: u64,
    lexical_index_size_bytes: u64,
    lexical_index_file_count: u64,
    lexical_index_segment_count: u64,
    semantic_index_size_bytes: u64,
    orphan_asset_files: u64,
    orphan_asset_file_bytes: u64,
    transient_files: u64,
    transient_file_bytes: u64,
    transient_file_samples: Vec<LibraryFileIssue>,
    visited_entries: usize,
}

fn diagnostic_path_is_transient(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.ends_with(".part")
            || (name.starts_with('.') && name.contains(".stage"))
            || name.starts_with(".piep-export-")
    })
}

fn diagnostic_metadata_is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn diagnostic_scan_roots(storage: &Path, semantic: &Path) -> Vec<PathBuf> {
    if semantic.starts_with(storage) {
        vec![storage.to_path_buf()]
    } else if storage.starts_with(semantic) {
        vec![semantic.to_path_buf()]
    } else {
        vec![storage.to_path_buf(), semantic.to_path_buf()]
    }
}

fn collect_diagnostic_file_stats_with_limits(
    storage_root: &Path,
    semantic_root: &Path,
    is_known_asset: &mut dyn FnMut(&Path) -> Result<bool, String>,
    limits: DiagnosticScanLimits,
) -> Result<DiagnosticFileStats, String> {
    if limits.max_entries == 0 {
        return Err("Diagnostic scan entry limit must be positive".to_string());
    }
    let lexical_root = storage_root.join("search-index");
    let mut stats = DiagnosticFileStats::default();
    let mut visited_directories = HashSet::new();

    for root in diagnostic_scan_roots(storage_root, semantic_root) {
        let root_metadata = match std::fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        };
        if diagnostic_metadata_is_link(&root_metadata) || !root_metadata.file_type().is_dir() {
            continue;
        }
        let canonical_root = match root.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        let mut pending = vec![(root, 0usize)];

        while let Some((directory, depth)) = pending.pop() {
            let metadata = match std::fs::symlink_metadata(&directory) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if diagnostic_metadata_is_link(&metadata) || !metadata.file_type().is_dir() {
                continue;
            }
            let canonical_directory = match directory.canonicalize() {
                Ok(path) if path.starts_with(&canonical_root) => path,
                _ => continue,
            };
            if !visited_directories.insert(canonical_directory) {
                continue;
            }
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.filter_map(Result::ok) {
                stats.visited_entries = stats.visited_entries.saturating_add(1);
                if stats.visited_entries > limits.max_entries {
                    return Err(format!(
                        "Diagnostic scan exceeded the {}-entry safety limit",
                        limits.max_entries
                    ));
                }
                let path = entry.path();
                let metadata = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                if diagnostic_metadata_is_link(&metadata) {
                    continue;
                }
                if metadata.file_type().is_dir() {
                    if depth >= limits.max_depth {
                        return Err(format!(
                            "Diagnostic scan exceeded the depth-{} safety limit",
                            limits.max_depth
                        ));
                    }
                    pending.push((path, depth + 1));
                    continue;
                }
                if !metadata.file_type().is_file() {
                    continue;
                }

                let bytes = metadata.len();
                let in_storage = path.starts_with(storage_root);
                if in_storage {
                    stats.storage_size_bytes = stats.storage_size_bytes.saturating_add(bytes);
                    if diagnostic_path_is_transient(&path) {
                        stats.transient_files = stats.transient_files.saturating_add(1);
                        stats.transient_file_bytes =
                            stats.transient_file_bytes.saturating_add(bytes);
                        if stats.transient_file_samples.len() < DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT {
                            stats.transient_file_samples.push(LibraryFileIssue {
                                issue_type: "transient".to_string(),
                                category: "transient".to_string(),
                                path: bounded_diagnostic_path(path.to_string_lossy().as_ref()),
                                label: None,
                                expected_size_bytes: None,
                                actual_size_bytes: Some(bytes),
                            });
                        }
                    }
                }
                if path.starts_with(&lexical_root) {
                    stats.lexical_index_size_bytes =
                        stats.lexical_index_size_bytes.saturating_add(bytes);
                    stats.lexical_index_file_count =
                        stats.lexical_index_file_count.saturating_add(1);
                    if path.extension().and_then(|extension| extension.to_str()) == Some("store") {
                        stats.lexical_index_segment_count =
                            stats.lexical_index_segment_count.saturating_add(1);
                    }
                }
                if path.starts_with(semantic_root) {
                    stats.semantic_index_size_bytes =
                        stats.semantic_index_size_bytes.saturating_add(bytes);
                }
                if in_storage
                    && path.components().any(|part| {
                        part.as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case("data_assets")
                    })
                    && !diagnostic_path_is_transient(&path)
                    && !is_known_asset(&path)?
                {
                    stats.orphan_asset_files = stats.orphan_asset_files.saturating_add(1);
                    stats.orphan_asset_file_bytes =
                        stats.orphan_asset_file_bytes.saturating_add(bytes);
                }
            }
        }
    }
    Ok(stats)
}

fn collect_diagnostic_file_stats(
    storage_root: &Path,
    semantic_root: &Path,
    is_known_asset: &mut dyn FnMut(&Path) -> Result<bool, String>,
) -> Result<DiagnosticFileStats, String> {
    collect_diagnostic_file_stats_with_limits(
        storage_root,
        semantic_root,
        is_known_asset,
        DiagnosticScanLimits::default(),
    )
}

fn recursive_file_size(root: &Path) -> u64 {
    let mut no_known_assets = |_path: &Path| Ok(false);
    collect_diagnostic_file_stats(
        root,
        &root.join(".piep-no-semantic-index"),
        &mut no_known_assets,
    )
    .map(|stats| stats.storage_size_bytes)
    .unwrap_or(0)
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
    /// Exact filtered count carried by a sorted-search cursor.
    #[serde(default)]
    total_estimate: Option<i64>,
    /// Raw lexical count carried separately because metadata filters can make
    /// the public total unknown while still allowing later pages to skip
    /// Tantivy's Count collector.
    #[serde(default)]
    tantivy_total_hits: Option<i64>,
    /// Identifies the exact disk-backed match set used by later pages.
    #[serde(default)]
    snapshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntitySeriesCursor {
    scope: String,
    library_generation: i64,
    latest_work_at: String,
    count: i64,
    display_name: String,
    source: String,
    source_key: String,
    total: i64,
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

const QUERY_RESULT_CACHE_TTL: Duration = Duration::from_secs(90);
const QUERY_RESULT_CACHE_MAX_ENTRIES: usize = 128;
const QUERY_RESULT_CACHE_MAX_BYTES: usize = 2 * 1024 * 1024;

struct CachedQueryResult<T> {
    generation: i64,
    value: T,
    bytes: usize,
    expires_at: Instant,
    last_used: u64,
}

struct BoundedQueryCache<T> {
    entries: HashMap<String, CachedQueryResult<T>>,
    tick: u64,
    bytes: usize,
    hits: u64,
    misses: u64,
}

impl<T: Clone> BoundedQueryCache<T> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
            bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    fn get(&mut self, key: &str, generation: i64) -> Option<T> {
        let now = Instant::now();
        self.retain_fresh(generation, now);
        self.tick = self.tick.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.tick;
            self.hits = self.hits.saturating_add(1);
            return Some(entry.value.clone());
        }
        self.misses = self.misses.saturating_add(1);
        None
    }

    fn insert(&mut self, key: String, generation: i64, value: T, bytes: usize) {
        self.retain_fresh(generation, Instant::now());
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        self.tick = self.tick.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            CachedQueryResult {
                generation,
                value,
                bytes,
                expires_at: Instant::now() + QUERY_RESULT_CACHE_TTL,
                last_used: self.tick,
            },
        );
        while self.entries.len() > QUERY_RESULT_CACHE_MAX_ENTRIES
            || self.bytes > QUERY_RESULT_CACHE_MAX_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(removed.bytes);
            }
        }
    }

    fn retain_fresh(&mut self, generation: i64, now: Instant) {
        self.entries.retain(|_, entry| {
            let keep = entry.generation == generation && entry.expires_at > now;
            if !keep {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
            keep
        });
    }
}

const SEARCH_SNAPSHOT_TTL: Duration = Duration::from_secs(90);
const SEARCH_SNAPSHOT_MAX_ENTRIES: usize = 4;
const SEARCH_SNAPSHOT_DIR: &str = ".search-snapshots";
type SnapshotConnection = Arc<Mutex<Option<Connection>>>;
/// WHERE と HAVING、それぞれに差し込む値。順番はSQLに `?` が現れる順。
type EntityFacetClauses = (
    Vec<String>,
    Vec<Box<dyn rusqlite::types::ToSql>>,
    Option<String>,
    Vec<Box<dyn rusqlite::types::ToSql>>,
);

struct DiskSearchSnapshot {
    id: String,
    key: String,
    library_generation: i64,
    index_generation: u64,
    row_count: i64,
    disk_bytes: u64,
    expires_at: Instant,
    path: PathBuf,
    connection: SnapshotConnection,
}

impl Drop for DiskSearchSnapshot {
    fn drop(&mut self) {
        if let Ok(mut connection) = self.connection.lock() {
            connection.take();
        }
        cleanup_search_snapshot_path(&self.path);
    }
}

struct CachedDiskSearchSnapshot {
    snapshot: Arc<DiskSearchSnapshot>,
    last_used: u64,
}

struct SearchSnapshotCache {
    entries: Vec<CachedDiskSearchSnapshot>,
    tick: u64,
    disk_bytes: u64,
    hits: u64,
    misses: u64,
}

impl SearchSnapshotCache {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            tick: 0,
            disk_bytes: 0,
            hits: 0,
            misses: 0,
        }
    }

    fn get(
        &mut self,
        key: &str,
        library_generation: i64,
        index_generation: u64,
        expected_id: Option<&str>,
    ) -> Result<Option<Arc<DiskSearchSnapshot>>, String> {
        self.retain_fresh(library_generation, index_generation);
        self.tick = self.tick.saturating_add(1);
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.snapshot.key == key
                && expected_id
                    .map(|expected| entry.snapshot.id == expected)
                    .unwrap_or(true)
        }) {
            entry.last_used = self.tick;
            self.hits = self.hits.saturating_add(1);
            return Ok(Some(entry.snapshot.clone()));
        }
        self.misses = self.misses.saturating_add(1);
        if expected_id.is_some() {
            return Err(
                "Search snapshot expired or was invalidated; restart the search".to_string(),
            );
        }
        Ok(None)
    }

    fn insert(&mut self, snapshot: Arc<DiskSearchSnapshot>) -> Result<(), String> {
        let disk_budget = super::resource_budget::search_snapshot_disk_bytes();
        if snapshot.disk_bytes > disk_budget {
            return Err(format!(
                "Search snapshot requires {} bytes, above the {}-byte disk budget",
                snapshot.disk_bytes, disk_budget
            ));
        }
        self.retain_fresh(snapshot.library_generation, snapshot.index_generation);
        if let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.snapshot.key == snapshot.key)
        {
            self.disk_bytes = self
                .disk_bytes
                .saturating_sub(self.entries[position].snapshot.disk_bytes);
            self.entries.swap_remove(position);
        }
        self.tick = self.tick.saturating_add(1);
        self.disk_bytes = self.disk_bytes.saturating_add(snapshot.disk_bytes);
        self.entries.push(CachedDiskSearchSnapshot {
            snapshot,
            last_used: self.tick,
        });
        while self.entries.len() > SEARCH_SNAPSHOT_MAX_ENTRIES || self.disk_bytes > disk_budget {
            let Some(position) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(position, _)| position)
            else {
                break;
            };
            self.disk_bytes = self
                .disk_bytes
                .saturating_sub(self.entries[position].snapshot.disk_bytes);
            self.entries.swap_remove(position);
        }
        Ok(())
    }

    fn retain_fresh(&mut self, library_generation: i64, index_generation: u64) {
        let now = Instant::now();
        let mut retained_bytes = 0u64;
        self.entries.retain(|entry| {
            let keep = entry.snapshot.library_generation == library_generation
                && entry.snapshot.index_generation == index_generation
                && entry.snapshot.expires_at > now;
            if keep {
                retained_bytes = retained_bytes.saturating_add(entry.snapshot.disk_bytes);
            }
            keep
        });
        self.disk_bytes = retained_bytes;
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.disk_bytes = 0;
    }
}

pub struct Database {
    conn: RestoreAwareConnection,
    read_pool: Pool<SqliteConnectionManager>,
    generation_conn: Mutex<Connection>,
    db_path: PathBuf,
    storage_dir: PathBuf,
    index_status_cache: Mutex<Option<CachedIndexStatus>>,
    facet_cache: Mutex<BoundedQueryCache<Vec<FacetCount>>>,
    entity_facet_cache: Mutex<BoundedQueryCache<Vec<EntityFacet>>>,
    suggest_cache: Mutex<BoundedQueryCache<SearchSuggestResult>>,
    filter_facets_cache: Mutex<BoundedQueryCache<FilterFacets>>,
    search_snapshot_cache: Mutex<SearchSnapshotCache>,
}

impl Database {
    /// データベースを開く（存在しなければ作成）
    pub fn open(db_path: &Path, storage_dir: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| format!("DB open failed: {}", e))?;
        schema::initialize(&conn).map_err(|e| format!("DB init failed: {}", e))?;
        reconcile_search_index_format(&conn)?;
        let read_pool = build_read_pool(db_path)?;
        let generation_conn = Connection::open(db_path)
            .map_err(|e| format!("DB generation monitor open failed: {e}"))?;
        generation_conn
            .pragma_update(None, "query_only", true)
            .map_err(|e| format!("DB generation monitor setup failed: {e}"))?;

        // ストレージディレクトリを作成
        std::fs::create_dir_all(storage_dir)
            .map_err(|e| format!("Storage dir creation failed: {}", e))?;
        cleanup_stale_search_snapshots(storage_dir)?;
        recover_interrupted_download_saves(&conn, storage_dir)?;

        Ok(Self {
            conn: RestoreAwareConnection::new(conn),
            read_pool,
            generation_conn: Mutex::new(generation_conn),
            db_path: db_path.to_path_buf(),
            storage_dir: storage_dir.to_path_buf(),
            index_status_cache: Mutex::new(None),
            facet_cache: Mutex::new(BoundedQueryCache::new()),
            entity_facet_cache: Mutex::new(BoundedQueryCache::new()),
            suggest_cache: Mutex::new(BoundedQueryCache::new()),
            filter_facets_cache: Mutex::new(BoundedQueryCache::new()),
            search_snapshot_cache: Mutex::new(SearchSnapshotCache::new()),
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

    /// Persists the restore's file rollback directory before live files are
    /// promoted. The row is deliberately outside the library transaction: if
    /// the process stops before COMMIT, startup can restore those files.
    pub(crate) fn create_restore_journal(
        &self,
        journal_id: &str,
        stage_root: &Path,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS restore_journal (
                id TEXT PRIMARY KEY,
                stage_root TEXT NOT NULL,
                committed INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
             )",
        )
        .map_err(|e| format!("Restore journal schema failed: {e}"))?;
        conn.execute(
            "INSERT INTO restore_journal (id, stage_root, committed, created_at)
             VALUES (?1, ?2, 0, ?3)",
            params![
                journal_id,
                stage_root.to_string_lossy().to_string(),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| format!("Restore journal create failed: {e}"))?;
        Ok(())
    }

    /// Written in the same SQLite transaction as all restored rows. Therefore
    /// startup can unambiguously distinguish COMMIT from rollback even if the
    /// process stops immediately after SQLite returns from COMMIT.
    pub(crate) fn mark_restore_journal_committed(&self, journal_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE restore_journal SET committed = 1 WHERE id = ?1",
                params![journal_id],
            )
            .map_err(|e| format!("Restore journal commit marker failed: {e}"))?;
        if changed != 1 {
            return Err("Restore journal disappeared before commit".to_string());
        }
        Ok(())
    }

    pub(crate) fn finish_restore_journal(&self, journal_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM restore_journal WHERE id = ?1",
            params![journal_id],
        )
        .map_err(|e| format!("Restore journal cleanup failed: {e}"))?;
        Ok(())
    }

    pub(crate) fn pending_restore_journals(&self) -> Result<Vec<(String, PathBuf, bool)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS restore_journal (
                id TEXT PRIMARY KEY,
                stage_root TEXT NOT NULL,
                committed INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
             )",
        )
        .map_err(|e| format!("Restore journal schema failed: {e}"))?;
        let mut statement = conn
            .prepare("SELECT id, stage_root, committed FROM restore_journal ORDER BY created_at")
            .map_err(|e| format!("Restore journal read failed: {e}"))?;
        let journals = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    PathBuf::from(row.get::<_, String>(1)?),
                    row.get::<_, i64>(2)? != 0,
                ))
            })
            .map_err(|e| format!("Restore journal query failed: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Restore journal decode failed: {e}"))?;
        Ok(journals)
    }

    pub(crate) fn delete_download_record_for_restore(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .map_err(|e| format!("Restore delete failed: {e}"))?;
        Ok(())
    }

    /// Roll back a failed DB-only re-import while deliberately retaining its
    /// work tree. This is called before search indexing, so no sidecar cleanup
    /// is needed and the next scan can retry the same source files.
    pub(crate) fn delete_download_record_for_reimport(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .map_err(|e| format!("Re-import delete failed: {e}"))?;
        drop(conn);
        self.invalidate_index_status();
        Ok(())
    }

    fn read_conn(&self) -> Result<PooledConnection<SqliteConnectionManager>, String> {
        self.read_pool
            .get()
            .map_err(|e| format!("DB read pool checkout failed: {}", e))
    }

    fn library_generation(&self) -> Result<i64, String> {
        self.generation_conn
            .lock()
            .map_err(|e| e.to_string())?
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(|e| format!("Library generation read failed: {e}"))
    }

    #[cfg(test)]
    fn query_cache_stats(&self) -> ((u64, u64), (u64, u64), (u64, u64)) {
        let facet = self.facet_cache.lock().expect("facet cache");
        let suggest = self.suggest_cache.lock().expect("suggest cache");
        let snapshots = self
            .search_snapshot_cache
            .lock()
            .expect("search snapshot cache");
        (
            (facet.hits, facet.misses),
            (suggest.hits, suggest.misses),
            (snapshots.hits, snapshots.misses),
        )
    }

    pub fn reindex_download(&self, download_id: i64) -> Result<(), String> {
        let result = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            reindex_download_locked(&conn, &self.storage_dir, download_id)
        };
        self.invalidate_index_status();
        result?;
        if let Err(error) = self.refresh_work_links(download_id) {
            // 検索索引は完成しているため、派生グラフだけの失敗で保存全体を失敗扱いにしない。
            log::warn!("Failed to refresh work links for {download_id}: {error}");
        }
        Ok(())
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
        if let Ok(mut cache) = self.search_snapshot_cache.lock() {
            cache.clear();
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
                match super::semantic_index::upsert_documents(&self.storage_dir, &prepared.semantic)
                {
                    Ok(_) => self.record_semantic_indexed_documents(&prepared.indexed)?,
                    Err(error) => log::warn!("Semantic index batch skipped: {error}"),
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

    fn record_semantic_indexed_documents(&self, states: &[IndexedState]) -> Result<(), String> {
        if states.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        record_semantic_indexed_documents_locked(&conn, states)?;
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
        let limit = limit.clamp(1, 200) as usize;
        let generation = self.library_generation()?;
        let cache_key = format!(
            "{}\u{1f}{}\u{1f}{limit}",
            kind,
            query.map(str::trim).unwrap_or("")
        );
        if let Ok(mut cache) = self.facet_cache.lock() {
            if let Some(cached) = cache.get(&cache_key, generation) {
                return Ok(cached);
            }
        }
        let conn = self.read_conn()?;
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

        let result = if normalized_query.is_empty() {
            load(sql(false)?, None, limit)?
        } else {
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
            scored.into_iter().map(|(_, facet)| facet).collect()
        };
        if let Ok(mut cache) = self.facet_cache.lock() {
            cache.insert(
                cache_key,
                generation,
                result.clone(),
                facet_counts_bytes(&result),
            );
        }
        Ok(result)
    }

    pub fn search_suggest(
        &self,
        params: &SearchSuggestParams,
    ) -> Result<SearchSuggestResult, String> {
        let limit = params.limit.unwrap_or(12).clamp(1, 50);
        let text = params.text.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return Ok(SearchSuggestResult { items: Vec::new() });
        }
        let generation = self.library_generation()?;
        let cache_key = format!("{text}\u{1f}{limit}");
        if let Ok(mut cache) = self.suggest_cache.lock() {
            if let Some(cached) = cache.get(&cache_key, generation) {
                return Ok(cached);
            }
        }
        let conn = self.read_conn()?;
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
        let result = SearchSuggestResult { items };
        if let Ok(mut cache) = self.suggest_cache.lock() {
            cache.insert(
                cache_key,
                generation,
                result.clone(),
                suggestions_bytes(&result),
            );
        }
        Ok(result)
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
        let id = upsert_download_in_connection(&tx, dl)?;

        tx.commit()
            .map_err(|e| format!("Transaction commit failed: {}", e))?;
        drop(conn);
        // A newly saved work is unindexed until something indexes it, so the
        // pending count the UI shows has just changed.
        self.invalidate_index_status();
        Ok(id)
    }

    pub(crate) fn create_download_save_journal(
        &self,
        source: &str,
        source_id: &str,
        version: i64,
        stage_path: &Path,
        final_path: &Path,
    ) -> Result<String, String> {
        let stage_parent = stage_path
            .parent()
            .ok_or_else(|| "Download save stage has no parent".to_string())?
            .canonicalize()
            .map_err(|e| format!("Download save stage parent resolution failed: {e}"))?;
        let final_parent = final_path
            .parent()
            .ok_or_else(|| "Download save final path has no parent".to_string())?
            .canonicalize()
            .map_err(|e| format!("Download save final parent resolution failed: {e}"))?;
        if stage_parent != final_parent {
            return Err("Download save paths are not on the same work directory".to_string());
        }
        let stage_path = stage_parent.join(
            stage_path
                .file_name()
                .ok_or_else(|| "Download save stage has no file name".to_string())?,
        );
        let final_path = final_parent.join(
            final_path
                .file_name()
                .ok_or_else(|| "Download save final path has no file name".to_string())?,
        );
        let id = format!(
            "download-{}-{:016x}",
            chrono::Utc::now().timestamp_millis(),
            rand::random::<u64>()
        );
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        ensure_download_save_journal_schema(&conn)?;
        conn.execute(
            "INSERT INTO download_save_journal (
                id, source, source_id, version, stage_path, final_path, committed, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
            params![
                id,
                source,
                source_id,
                version,
                stage_path.to_string_lossy(),
                final_path.to_string_lossy(),
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Download save journal creation failed: {e}"))?;
        Ok(id)
    }

    pub(crate) fn finish_download_save_journal(&self, journal_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM download_save_journal WHERE id = ?1",
            params![journal_id],
        )
        .map_err(|e| format!("Download save journal cleanup failed: {e}"))?;
        Ok(())
    }

    pub(crate) fn commit_download_save_with_journal(
        &self,
        dl: &NewDownload,
        assets: &[NewAsset],
        versions: &[NewVersion],
        journal_id: &str,
    ) -> Result<i64, String> {
        self.commit_download_save_impl(dl, assets, versions, Some(journal_id), false)
    }

    /// Rebuild a DB record from version directories that already exist on
    /// disk. Unlike a normal save there is no filesystem publish journal, but
    /// the download, tags, assets, and every historical version still commit
    /// as one SQLite transaction so a damaged folder cannot leave a half-work
    /// in the library.
    pub(crate) fn commit_reimported_download(
        &self,
        dl: &NewDownload,
        assets: &[NewAsset],
        versions: &[NewVersion],
    ) -> Result<i64, String> {
        self.commit_download_save_impl(dl, assets, versions, None, false)
    }

    fn commit_download_save_impl(
        &self,
        dl: &NewDownload,
        assets: &[NewAsset],
        versions: &[NewVersion],
        journal_id: Option<&str>,
        fail_before_commit: bool,
    ) -> Result<i64, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Download save transaction begin failed: {e}"))?;
        let download_id = upsert_download_in_connection(&tx, dl)?;

        for asset in assets {
            tx.execute(
                "INSERT OR REPLACE INTO assets (
                    download_id, asset_type, filename, local_path,
                    original_url, mime_type, file_size_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    download_id,
                    asset.asset_type,
                    asset.filename,
                    asset.local_path,
                    asset.original_url,
                    asset.mime_type,
                    asset.file_size_bytes,
                ],
            )
            .map_err(|e| format!("Download save asset insert failed: {e}"))?;
        }

        for version in versions {
            tx.execute(
                "INSERT INTO download_versions (
                    download_id, version, content_hash, text_length, json_path,
                    original_json_path, asset_count, file_size_bytes, created_at, change_summary
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    download_id,
                    version.version,
                    version.content_hash,
                    version.text_length,
                    version.json_path,
                    version.original_json_path,
                    version.asset_count,
                    version.file_size_bytes,
                    version.created_at,
                    version.change_summary,
                ],
            )
            .map_err(|e| format!("Download save version insert failed: {e}"))?;
        }

        if fail_before_commit {
            return Err("Injected download save failure".to_string());
        }
        if let Some(journal_id) = journal_id {
            let changed = tx
                .execute(
                    "UPDATE download_save_journal SET committed = 1 WHERE id = ?1",
                    params![journal_id],
                )
                .map_err(|e| format!("Download save journal commit marker failed: {e}"))?;
            if changed != 1 {
                return Err("Download save journal disappeared before commit".to_string());
            }
        }
        tx.commit()
            .map_err(|e| format!("Download save transaction commit failed: {e}"))?;
        drop(conn);
        self.invalidate_index_status();
        Ok(download_id)
    }

    #[cfg(test)]
    pub(crate) fn commit_download_save_with_injected_failure(
        &self,
        dl: &NewDownload,
        assets: &[NewAsset],
        versions: &[NewVersion],
    ) -> Result<i64, String> {
        self.commit_download_save_impl(dl, assets, versions, None, true)
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

    /// Pages every update target in composite-key order. Archive export uses
    /// this instead of materializing the complete table in one metadata blob.
    pub fn list_update_targets_after(
        &self,
        cursor: Option<(&str, &str, &str)>,
        limit: i64,
    ) -> Result<Vec<UpdateTarget>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 5_000);
        let (sql, bind_values): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match cursor {
            Some((target_type, source, source_key)) => (
                "SELECT * FROM update_targets
                 WHERE target_type > ?1
                    OR (target_type = ?1 AND source > ?2)
                    OR (target_type = ?1 AND source = ?2 AND source_key > ?3)
                 ORDER BY target_type ASC, source ASC, source_key ASC
                 LIMIT ?4",
                vec![
                    Box::new(target_type.to_string()),
                    Box::new(source.to_string()),
                    Box::new(source_key.to_string()),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT * FROM update_targets
                 ORDER BY target_type ASC, source ASC, source_key ASC
                 LIMIT ?1",
                vec![Box::new(limit)],
            ),
        };
        let refs = bind_values
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>();
        let mut statement = conn
            .prepare(sql)
            .map_err(|e| format!("Update target page prepare failed: {e}"))?;
        let targets = statement
            .query_map(refs.as_slice(), update_target_from_row)
            .map_err(|e| format!("Update target page query failed: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Update target page row failed: {e}"))?;
        Ok(targets)
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

    /// 対象を確認できたことを記録する。
    ///
    /// `found` が 0 より大きいときだけ「最後に見つけた時刻」を進める。ここが
    /// 何か月も古いままの対象は、監視をやめるか間隔を空ける判断の材料になる。
    /// 確認できた時点で連続失敗は 0 に戻す。
    pub fn mark_update_target_checked(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        last_seen_source_id: Option<&str>,
        last_seen_source_updated_at: Option<&str>,
        found: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE update_targets SET
                last_checked_at = ?1,
                last_seen_source_id = COALESCE(?2, last_seen_source_id),
                last_seen_source_updated_at = COALESCE(?3, last_seen_source_updated_at),
                last_hit_at = CASE WHEN ?7 > 0 THEN ?1 ELSE last_hit_at END,
                consecutive_errors = 0,
                updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?4 AND source = ?5 AND source_key = ?6",
            params![
                now,
                last_seen_source_id,
                last_seen_source_updated_at,
                target_type,
                source,
                source_key,
                found,
            ],
        )
        .map_err(|e| format!("Failed to mark update target checked: {}", e))?;
        Ok(())
    }

    /// 対象の確認に失敗したことを記録する。連続失敗が積み上がる。
    pub fn mark_update_target_failed(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_targets SET
                last_checked_at = ?1,
                consecutive_errors = consecutive_errors + 1,
                updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?2 AND source = ?3 AND source_key = ?4",
            params![
                chrono::Utc::now().to_rfc3339(),
                target_type,
                source,
                source_key,
            ],
        )
        .map_err(|e| format!("Failed to mark update target failed: {}", e))?;
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

    /// Lightweight keyset scan used by multipart backup cataloging. Unlike
    /// `list_people`, this does not run a correlated work count for every row.
    pub fn list_people_keys_after(
        &self,
        cursor: Option<(&str, &str)>,
        limit: i64,
    ) -> Result<Vec<(String, String)>, String> {
        self.list_entity_keys_after("people", cursor, limit)
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

    /// Lightweight keyset scan used by multipart backup cataloging. Orphaned
    /// series are included because the source table, not work relations, owns
    /// the scan.
    pub fn list_series_keys_after(
        &self,
        cursor: Option<(&str, &str)>,
        limit: i64,
    ) -> Result<Vec<(String, String)>, String> {
        self.list_entity_keys_after("series", cursor, limit)
    }

    fn list_entity_keys_after(
        &self,
        table: &str,
        cursor: Option<(&str, &str)>,
        limit: i64,
    ) -> Result<Vec<(String, String)>, String> {
        let table = match table {
            "people" => "people",
            "series" => "series",
            _ => return Err("Unsupported entity key table".to_string()),
        };
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 5_000);
        let (sql, values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match cursor {
            Some((source, source_key)) => (
                format!(
                    "SELECT source, source_key FROM {table}
                     WHERE source > ?1 OR (source = ?1 AND source_key > ?2)
                     ORDER BY source ASC, source_key ASC LIMIT ?3"
                ),
                vec![
                    Box::new(source.to_string()),
                    Box::new(source_key.to_string()),
                    Box::new(limit),
                ],
            ),
            None => (
                format!(
                    "SELECT source, source_key FROM {table}
                     ORDER BY source ASC, source_key ASC LIMIT ?1"
                ),
                vec![Box::new(limit)],
            ),
        };
        let refs = values
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<&dyn rusqlite::types::ToSql>>();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Entity key page prepare failed: {e}"))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Entity key page query failed: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Entity key page row failed: {e}"))
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
    /// 取得元から聞いてきたシリーズの様子。
    ///
    /// `is_concluded` と `published_content_count` は None を「聞けなかった」
    /// として扱い、手元の値を消さない。取得元が黙っていることを、こちらで
    /// 「連載中」と言い切らないため。
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
        is_concluded: Option<bool>,
        published_content_count: Option<i64>,
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
                    is_concluded = COALESCE(?7, is_concluded),
                    published_content_count = COALESCE(?8, published_content_count),
                    updated_at = CURRENT_TIMESTAMP
                 WHERE source = ?5 AND source_key = ?6",
                params![
                    title,
                    description,
                    cover_path,
                    checked_at,
                    source,
                    source_key,
                    is_concluded,
                    published_content_count
                ],
            )
            .map_err(|e| format!("Failed to mark series checked: {}", e))?;
        } else {
            let next_version = current_version + 1;
            conn.execute(
                "INSERT INTO series (
                    source, source_key, title, description, cover_path, content_hash,
                    current_version, last_checked_at, last_fetched_at, created_at, updated_at,
                    is_concluded, published_content_count
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?9, ?10)
                 ON CONFLICT(source, source_key) DO UPDATE SET
                    title = excluded.title,
                    description = excluded.description,
                    cover_path = COALESCE(excluded.cover_path, series.cover_path),
                    content_hash = excluded.content_hash,
                    current_version = excluded.current_version,
                    last_checked_at = COALESCE(excluded.last_checked_at, series.last_checked_at),
                    last_fetched_at = COALESCE(excluded.last_fetched_at, series.last_fetched_at),
                    is_concluded = COALESCE(excluded.is_concluded, series.is_concluded),
                    published_content_count = COALESCE(excluded.published_content_count, series.published_content_count),
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    source,
                    source_key,
                    title,
                    description,
                    cover_path,
                    content_hash,
                    next_version,
                    checked_at,
                    is_concluded,
                    published_content_count
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
        let semantic_complete = status.semantic_pending_downloads == 0;

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
            candidate.kind == "ranked-search" && candidate.scope.as_deref() == Some(&cursor_scope)
        });
        if decoded_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.snapshot_id.is_none())
        {
            return Err("Search cursor predates ranked snapshots; restart the search".to_string());
        }
        let expected_snapshot = decoded_cursor
            .as_ref()
            .and_then(|cursor| cursor.snapshot_id.as_deref());
        let snapshot = self.ranked_search_snapshot(params, query, expected_snapshot)?;
        let total_estimate = Some(snapshot.row_count);

        let connection = snapshot
            .connection
            .lock()
            .map_err(|e| format!("Ranked search connection lock failed: {e}"))?;
        let conn = connection
            .as_ref()
            .ok_or_else(|| "Ranked search snapshot was closed; restart the search".to_string())?;
        let mut sql =
            download_select_sql_for_projection(params.projection.as_deref(), "sm.score", "NULL")
                .replacen(
                    "FROM downloads d",
                    "FROM downloads d JOIN search_matches sm ON sm.id = d.id",
                    1,
                );
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(cursor) = decoded_cursor.as_ref() {
            if let (Some(score), Some(id)) = (cursor.tantivy_score, cursor.id) {
                sql.push_str(" WHERE (sm.score < ? OR (sm.score = ? AND sm.id > ?))");
                bind_values.push(Box::new(score as f64));
                bind_values.push(Box::new(score as f64));
                bind_values.push(Box::new(id));
            }
        }
        sql.push_str(" ORDER BY sm.score DESC, sm.id ASC LIMIT ?");
        bind_values.push(Box::new(limit + 1));
        let mut page_items = query_download_entries(conn, &sql, &bind_values)?;
        drop(connection);

        let has_more = page_items.len() as i64 > limit;
        page_items.truncate(limit as usize);
        let next_cursor = if has_more {
            page_items.last().and_then(|item| {
                encode_cursor(&SearchCursor {
                    kind: "ranked-search".to_string(),
                    scope: Some(cursor_scope.clone()),
                    sort_by: Some("relevance".to_string()),
                    sort_order: Some("desc".to_string()),
                    value: None,
                    id: Some(item.id),
                    score: item.search_score,
                    downloaded_at: None,
                    tantivy_score: item.search_score.map(|score| score as f32),
                    tantivy_segment_ord: None,
                    tantivy_doc_id: None,
                    total_estimate,
                    tantivy_total_hits: None,
                    snapshot_id: Some(snapshot.id.clone()),
                })
            })
        } else {
            None
        };
        let items = decorate_search_results(
            &self.storage_dir,
            page_items,
            parsed_query,
            &HashMap::new(),
            &HashMap::new(),
        );
        let semantic_complete = status.semantic_pending_downloads == 0;

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

    fn ranked_search_snapshot(
        &self,
        params: &SearchV2Params,
        query: &str,
        expected_id: Option<&str>,
    ) -> Result<Arc<DiskSearchSnapshot>, String> {
        self.ranked_search_snapshot_inner(params, query, expected_id, None, None)
    }

    fn ranked_search_snapshot_inner(
        &self,
        params: &SearchV2Params,
        query: &str,
        expected_id: Option<&str>,
        fail_after_batches: Option<usize>,
        disk_budget_override: Option<u64>,
    ) -> Result<Arc<DiskSearchSnapshot>, String> {
        let key = format!("ranked:{}", search_cursor_scope(params));
        for _ in 0..3 {
            let library_generation = self.library_generation()?;
            let index_generation = super::tantivy_index::index_generation(&self.storage_dir)?;
            if let Some(snapshot) = self
                .search_snapshot_cache
                .lock()
                .map_err(|e| format!("Search snapshot cache lock failed: {e}"))?
                .get(&key, library_generation, index_generation, expected_id)?
            {
                return Ok(snapshot);
            }

            let (id, path, connection) =
                create_search_snapshot_connection(&self.storage_dir, &self.db_path)?;
            let build_result = (|| {
                {
                    let guard = connection
                        .lock()
                        .map_err(|e| format!("Ranked search connection lock failed: {e}"))?;
                    let conn = guard.as_ref().ok_or_else(|| {
                        "Ranked search snapshot connection was closed".to_string()
                    })?;
                    reset_search_match_table(conn, true)?;
                    conn.execute_batch("BEGIN IMMEDIATE")
                        .map_err(|e| format!("Ranked search snapshot begin failed: {e}"))?;
                }

                let sink = connection.clone();
                let batch_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let visited_batches = batch_counter.clone();
                let snapshot_storage = self.storage_dir.clone();
                let stream_result = super::tantivy_index::visit_matching_download_scores(
                    &self.storage_dir,
                    query,
                    move |scores| {
                        let batch = visited_batches.fetch_add(1, AtomicOrdering::AcqRel);
                        if fail_after_batches.is_some_and(|limit| batch >= limit) {
                            return Err("Injected ranked snapshot stream failure".to_string());
                        }
                        let guard = sink
                            .lock()
                            .map_err(|e| format!("Ranked search connection lock failed: {e}"))?;
                        let conn = guard.as_ref().ok_or_else(|| {
                            "Ranked search snapshot connection was closed".to_string()
                        })?;
                        insert_search_match_scores(conn, scores)?;
                        ensure_search_snapshot_budget_with_limit(
                            conn,
                            &snapshot_storage,
                            disk_budget_override.unwrap_or_else(|| {
                                super::resource_budget::search_snapshot_disk_bytes()
                            }),
                        )
                        .map(|_| ())
                    },
                );
                if let Err(error) = stream_result {
                    if let Ok(guard) = connection.lock() {
                        if let Some(conn) = guard.as_ref() {
                            let _ = conn.execute_batch("ROLLBACK");
                        }
                    }
                    return Err(error);
                }

                let guard = connection
                    .lock()
                    .map_err(|e| format!("Ranked search connection lock failed: {e}"))?;
                let conn = guard
                    .as_ref()
                    .ok_or_else(|| "Ranked search snapshot connection was closed".to_string())?;
                let mut wheres = vec!["d.id = search_matches.id".to_string()];
                let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
                append_library_filters(params, &mut wheres, &mut bind_values);
                let delete_sql = format!(
                    "DELETE FROM search_matches WHERE NOT EXISTS (SELECT 1 FROM downloads d WHERE {})",
                    wheres.join(" AND ")
                );
                let refs: Vec<&dyn rusqlite::types::ToSql> =
                    bind_values.iter().map(|value| value.as_ref()).collect();
                conn.execute(&delete_sql, refs.as_slice())
                    .map_err(|e| format!("Ranked search filter materialization failed: {e}"))?;
                conn.execute_batch(
                    "CREATE INDEX search_matches_rank ON search_matches(score DESC, id ASC);
                     COMMIT;",
                )
                .map_err(|e| format!("Ranked search snapshot finalize failed: {e}"))?;
                let row_count = conn
                    .query_row("SELECT COUNT(*) FROM search_matches", [], |row| row.get(0))
                    .map_err(|e| format!("Ranked search snapshot count failed: {e}"))?;
                let disk_bytes = ensure_search_snapshot_budget_with_limit(
                    conn,
                    &self.storage_dir,
                    disk_budget_override
                        .unwrap_or_else(super::resource_budget::search_snapshot_disk_bytes),
                )?;
                pin_search_snapshot_library(conn)?;
                Ok((row_count, disk_bytes))
            })();

            let (row_count, disk_bytes) = match build_result {
                Ok(result) => result,
                Err(error) => {
                    close_snapshot_connection(&connection);
                    cleanup_search_snapshot_path(&path);
                    return Err(error);
                }
            };
            if self.library_generation()? != library_generation
                || super::tantivy_index::index_generation(&self.storage_dir)? != index_generation
            {
                close_snapshot_connection(&connection);
                cleanup_search_snapshot_path(&path);
                if expected_id.is_some() {
                    return Err(
                        "Search snapshot was invalidated while paging; restart the search"
                            .to_string(),
                    );
                }
                continue;
            }

            let snapshot = Arc::new(DiskSearchSnapshot {
                id,
                key: key.clone(),
                library_generation,
                index_generation,
                row_count,
                disk_bytes,
                expires_at: Instant::now() + SEARCH_SNAPSHOT_TTL,
                path,
                connection,
            });
            self.search_snapshot_cache
                .lock()
                .map_err(|e| format!("Search snapshot cache lock failed: {e}"))?
                .insert(snapshot.clone())?;
            return Ok(snapshot);
        }
        Err(
            "Library changed repeatedly while preparing ranked search; retry the search"
                .to_string(),
        )
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
        let semantic_complete = status.semantic_pending_downloads == 0;
        let mut explanations =
            search_explanations(exact_entity.as_ref(), "tantivy", status.is_complete);
        explanations.push(format!(
            "{}で並び替えています（関連度順ではありません）",
            sort_label(params)
        ));

        let cursor_scope = search_cursor_scope(params);
        let cursor = decode_cursor(params.cursor.as_deref()).filter(|candidate| {
            candidate.kind == "sorted-search"
                && candidate.scope.as_deref() == Some(&cursor_scope)
                && candidate.sort_by == effective_sort_by(params)
                && candidate.sort_order == effective_sort_order(params)
        });
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.snapshot_id.is_none())
        {
            return Err("Search cursor predates disk snapshots; restart the search".to_string());
        }

        let expected_snapshot = cursor
            .as_ref()
            .and_then(|candidate| candidate.snapshot_id.as_deref());
        // The match set lives in its own bounded SQLite file. Later pages reuse
        // both the Tantivy scan and the disk-backed membership table, including
        // match sets well beyond one million ids.
        let snapshot = self.sorted_search_snapshot(query, expected_snapshot)?;
        let matched_count = snapshot.row_count;

        if matched_count == 0 {
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

        let connection = snapshot
            .connection
            .lock()
            .map_err(|e| format!("Sorted search connection lock failed: {e}"))?;
        let conn = connection
            .as_ref()
            .ok_or_else(|| "Sorted search snapshot was closed; restart the search".to_string())?;
        let mut sql = download_select_sql_for_projection(
            params.projection.as_deref(),
            "NULL",
            &sort_key_select_expr(params),
        )
        .replacen(
            "FROM downloads d",
            "FROM downloads d JOIN search_matches sm ON sm.id = d.id",
            1,
        );
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_library_filters(params, &mut wheres, &mut bind_values);
        append_keyset_filter(params, cursor.as_ref(), &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(&sort_clause(params));
        sql.push_str(" LIMIT ?");
        bind_values.push(Box::new(limit + 1));
        if let Some(offset) = page_offset(params) {
            sql.push_str(" OFFSET ?");
            bind_values.push(Box::new(offset));
        }
        let mut items = query_download_entries(conn, &sql, &bind_values)?;

        let total_estimate = cursor
            .as_ref()
            .and_then(|cursor| cursor.total_estimate)
            .or_else(|| self.count_sorted_search_matches(conn, params).ok());
        drop(connection);
        let has_more = items.len() as i64 > limit;
        items.truncate(limit as usize);

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
                    total_estimate,
                    tantivy_total_hits: None,
                    snapshot_id: Some(snapshot.id.clone()),
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
        conn: &Connection,
        params: &SearchV2Params,
    ) -> Result<i64, String> {
        let mut sql = "SELECT COUNT(*)
            FROM search_matches sm
            JOIN downloads d ON d.id = sm.id"
            .to_string();
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_library_filters(params, &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|value| value.as_ref()).collect();
        conn.query_row(&sql, refs.as_slice(), |row| row.get(0))
            .map_err(|e| format!("Sorted search count failed: {e}"))
    }

    fn sorted_search_snapshot(
        &self,
        query: &str,
        expected_id: Option<&str>,
    ) -> Result<Arc<DiskSearchSnapshot>, String> {
        let key = format!(
            "sorted:{}",
            URL_SAFE_NO_PAD.encode(Sha256::digest(query.as_bytes()))
        );
        for _ in 0..3 {
            let library_generation = self.library_generation()?;
            let index_generation = super::tantivy_index::index_generation(&self.storage_dir)?;
            if let Some(snapshot) = self
                .search_snapshot_cache
                .lock()
                .map_err(|e| format!("Search snapshot cache lock failed: {e}"))?
                .get(&key, library_generation, index_generation, expected_id)?
            {
                return Ok(snapshot);
            }

            let (id, path, connection) =
                create_search_snapshot_connection(&self.storage_dir, &self.db_path)?;
            {
                let guard = connection
                    .lock()
                    .map_err(|e| format!("Sorted search connection lock failed: {e}"))?;
                let conn = guard
                    .as_ref()
                    .ok_or_else(|| "Sorted search snapshot connection was closed".to_string())?;
                reset_search_match_table(conn, false)?;
                conn.execute_batch("BEGIN IMMEDIATE")
                    .map_err(|e| format!("Sorted search snapshot begin failed: {e}"))?;
            }
            let match_conn = connection.clone();
            let snapshot_storage = self.storage_dir.clone();
            let stream_result = super::tantivy_index::visit_matching_download_ids(
                &self.storage_dir,
                query,
                move |ids| {
                    let guard = match_conn
                        .lock()
                        .map_err(|e| format!("Sorted search connection lock failed: {e}"))?;
                    let conn = guard.as_ref().ok_or_else(|| {
                        "Sorted search snapshot connection was closed".to_string()
                    })?;
                    insert_search_match_ids(conn, ids)?;
                    ensure_search_snapshot_budget(conn, &snapshot_storage).map(|_| ())
                },
            );
            if let Err(error) = stream_result {
                if let Ok(guard) = connection.lock() {
                    if let Some(conn) = guard.as_ref() {
                        let _ = conn.execute_batch("ROLLBACK");
                    }
                }
                close_snapshot_connection(&connection);
                cleanup_search_snapshot_path(&path);
                return Err(error);
            }
            let (matched_count, disk_bytes) = {
                let guard = connection
                    .lock()
                    .map_err(|e| format!("Sorted search connection lock failed: {e}"))?;
                let conn = guard
                    .as_ref()
                    .ok_or_else(|| "Sorted search snapshot connection was closed".to_string())?;
                conn.execute_batch("COMMIT")
                    .map_err(|e| format!("Sorted search snapshot commit failed: {e}"))?;
                let matched_count = conn
                    .query_row("SELECT COUNT(*) FROM search_matches", [], |row| row.get(0))
                    .map_err(|e| format!("Sorted search snapshot count failed: {e}"))?;
                let disk_bytes = ensure_search_snapshot_budget(conn, &self.storage_dir)?;
                pin_search_snapshot_library(conn)?;
                (matched_count, disk_bytes)
            };

            if self.library_generation()? != library_generation
                || super::tantivy_index::index_generation(&self.storage_dir)? != index_generation
            {
                close_snapshot_connection(&connection);
                cleanup_search_snapshot_path(&path);
                if expected_id.is_some() {
                    return Err(
                        "Search snapshot was invalidated while paging; restart the search"
                            .to_string(),
                    );
                }
                continue;
            }
            let snapshot = Arc::new(DiskSearchSnapshot {
                id,
                key: key.clone(),
                library_generation,
                index_generation,
                row_count: matched_count,
                disk_bytes,
                expires_at: Instant::now() + SEARCH_SNAPSHOT_TTL,
                path,
                connection,
            });
            self.search_snapshot_cache
                .lock()
                .map_err(|e| format!("Search snapshot cache lock failed: {e}"))?
                .insert(snapshot.clone())?;
            return Ok(snapshot);
        }
        Err(
            "Library changed repeatedly while preparing sorted search; retry the search"
                .to_string(),
        )
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
        self.delete_downloads(&[id]).map(|_| ())
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

    /// このシリーズの作品を書いている人（手元のライブラリから分かる範囲で）。
    ///
    /// 作者を監視していればシリーズの新作もその一覧に出るので、両方を走査すると
    /// 同じものを二度取りに行くことになる。それを避ける判断に使う。
    pub fn series_author_keys(
        &self,
        source: &str,
        series_key: &str,
    ) -> Result<Vec<String>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT d.author_id
                 FROM downloads d
                 JOIN download_series ds ON ds.download_id = d.id
                 WHERE ds.series_source = ?1 AND ds.series_key = ?2 AND d.source = ?1
                   AND d.author_id != ''",
            )
            .map_err(|e| format!("Failed to prepare series authors: {}", e))?;
        let rows = stmt
            .query_map(params![source, series_key], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to read series authors: {}", e))?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|e| format!("Failed to read series author: {}", e))?);
        }
        Ok(keys)
    }

    /// 見つけた候補を、ジョブより長く残す。
    ///
    /// すでに無視すると決めた作品は `pending` に戻さない。同じ作品が
    /// 何度見つかっても、決めた答えの方が新しい。
    pub fn upsert_update_candidate(&self, candidate: &UpdateCandidateInput) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_candidates (
                source, source_id, kind, title, payload_json, target_type, status, first_seen_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)
             ON CONFLICT(source, source_id) DO UPDATE SET
                kind = excluded.kind,
                title = excluded.title,
                payload_json = excluded.payload_json,
                target_type = excluded.target_type,
                updated_at = excluded.updated_at",
            params![
                candidate.source,
                candidate.source_id,
                candidate.kind,
                candidate.title,
                candidate.payload_json,
                candidate.target_type,
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to record update candidate: {}", e))?;
        Ok(())
    }

    /// まだ answer の出ていない候補。新しく見つけた順に返す。
    /// まだ取り込んでいない改稿がある作品の鍵。`{source}:{sourceId}` の並び。
    ///
    /// 「取得元のほうが新しい」という状態は、作品の属性ではなく更新側が
    /// 見つけた事実である。だから `downloads` に列を足さず、ここから引く。
    /// 改稿を見つけたとき基準値をわざと書き換えないのも同じ理由で、
    /// **取り直して初めて追いついたと言える。**
    ///
    /// 返るのは未処理のものだけなので、ふつうは空か、ごく少ない。
    pub fn pending_revision_keys(&self) -> Result<Vec<PendingRevision>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT d.id, c.updated_at
                   FROM update_candidates c
                   JOIN downloads d
                     ON d.source = c.source AND d.source_id = c.source_id
                  WHERE c.status = 'pending' AND c.kind = 'revision'",
            )
            .map_err(|e| format!("Failed to prepare revision keys: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PendingRevision {
                    download_id: row.get(0)?,
                    found_at: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read revision keys: {}", e))
    }

    pub fn list_pending_update_candidates(
        &self,
        limit: i64,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT source, source_id, kind, title, payload_json, target_type, status,
                        first_seen_at, updated_at
                 FROM update_candidates
                 WHERE status = 'pending'
                 ORDER BY updated_at DESC, rowid DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare update candidates: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(UpdateCandidateRow {
                    source: row.get(0)?,
                    source_id: row.get(1)?,
                    kind: row.get(2)?,
                    title: row.get(3)?,
                    payload_json: row.get(4)?,
                    target_type: row.get(5)?,
                    status: row.get(6)?,
                    first_seen_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|e| format!("Failed to read update candidates: {}", e))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("Failed to read update candidate: {}", e))?);
        }
        Ok(items)
    }

    /// この作品を今後は候補に出さない。決定は取り消せる。
    pub fn set_update_candidate_status(
        &self,
        source: &str,
        source_id: &str,
        status: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_candidates SET status = ?3, updated_at = ?4
             WHERE source = ?1 AND source_id = ?2",
            params![source, source_id, status, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to set update candidate status: {}", e))?;
        Ok(())
    }

    /// 答えの出た候補を片付ける（保存できた、あるいは無視の取り消しで再提示）。
    pub fn clear_update_candidate(&self, source: &str, source_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_candidates WHERE source = ?1 AND source_id = ?2",
            params![source, source_id],
        )
        .map_err(|e| format!("Failed to clear update candidate: {}", e))?;
        Ok(())
    }

    pub fn update_candidate_status(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<String>, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT status FROM update_candidates WHERE source = ?1 AND source_id = ?2",
            params![source, source_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read update candidate status: {}", e))
    }

    /// 無視した件数と、いちばん新しいもの。画面の「無視した作品」の表示に使う。
    pub fn count_dismissed_update_candidates(&self) -> Result<i64, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM update_candidates WHERE status = 'dismissed'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count dismissed candidates: {}", e))
    }

    /// 無視の決定をすべて取り消す。次の確認でまた候補に出る。
    pub fn restore_dismissed_update_candidates(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_candidates SET status = 'pending', updated_at = ?1
             WHERE status = 'dismissed'",
            params![chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to restore dismissed candidates: {}", e))
    }

    /// ジョブを始めたときの依頼内容。再開したワーカーも同じ設定で動けるように、
    /// 認証情報を除いたものを保存してある。
    /// このジョブが保存した作品を、そのまま監視に載せるか。
    ///
    /// 依頼のうち、走り出したあとも要るのはこれだけ。再開したワーカーも
    /// 同じ設定で動く。
    pub fn update_job_watch_saved(&self, job_id: &str) -> Result<bool, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT watch_saved FROM update_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|e| format!("Update job not found: {}", e))
    }

    /// 終わったジョブの履歴を整理する。
    ///
    /// 起動時に一度だけ呼ぶ。新しい方から `keep_count` 件は日数に関わらず残し、
    /// それより古いものは `keep_days` を過ぎていれば消す。走っているジョブと
    /// 一時停止中のジョブには触れない。項目とログは外部キーの連鎖で消える。
    pub fn prune_update_jobs(&self, keep_count: i64, keep_days: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(keep_days)).to_rfc3339();
        let removed = conn
            .execute(
                "DELETE FROM update_jobs
                 WHERE status IN ('completed', 'failed', 'canceled')
                   AND COALESCE(finished_at, updated_at) < ?1
                   AND id NOT IN (
                       SELECT id FROM update_jobs ORDER BY updated_at DESC LIMIT ?2
                   )",
                params![cutoff, keep_count],
            )
            .map_err(|e| format!("Failed to prune update jobs: {}", e))?;
        Ok(removed)
    }

    /// 終わった更新ジョブをすべて消す。走っているものには触れない。
    ///
    /// 操作履歴の「完了履歴を消去」は画面側の記録しか消せていなかった。
    /// 更新ジョブは DB にあるので、こちらからも消せる必要がある。
    pub fn clear_finished_update_jobs(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_jobs WHERE status IN ('completed', 'failed', 'canceled')",
            [],
        )
        .map_err(|e| format!("Failed to clear finished update jobs: {}", e))
    }

    /// 1ジョブあたりのログを頭から落として上限に収める。
    ///
    /// 長く走るジョブでは1件ごとに数行積まれるため、上限がないとスナップショット
    /// の読み出しと DB がじりじり重くなる。
    pub fn trim_update_job_logs(&self, job_id: &str, keep: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute(
                "DELETE FROM update_job_logs
                 WHERE job_id = ?1 AND id NOT IN (
                     SELECT id FROM update_job_logs WHERE job_id = ?1 ORDER BY id DESC LIMIT ?2
                 )",
                params![job_id, keep],
            )
            .map_err(|e| format!("Failed to trim update job logs: {}", e))?;
        Ok(removed)
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
        // 依頼そのものは持ち越さない。走行中に読むのは watch_saved だけで、
        // 残りは items と scope/mode の列にもう写っている。認証情報を
        // 抱え込まずに済むのも、必要な分だけを取り出す形の利点。
        tx.execute(
            "INSERT INTO update_jobs (
                id, scope, mode, status, watch_saved, totals, processed,
                candidate_count, saved_count, error_count, active_label,
                started_at, updated_at, finished_at
             ) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, 0, 0, 0, 0, NULL, ?6, ?6, NULL)",
            params![
                job_id,
                request.scope,
                request.mode,
                request.watch_saved.unwrap_or(false),
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
        self.update_job_snapshot_page(job_id, None, None)
    }

    pub fn update_job_snapshot_page(
        &self,
        job_id: &str,
        candidate_after_id: Option<i64>,
        log_before_id: Option<i64>,
    ) -> Result<UpdateJobSnapshot, String> {
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
                    WHERE job_id = ?1 AND (?2 IS NULL OR id < ?2)
                    ORDER BY id DESC
                    LIMIT ?3
                 ) ORDER BY id ASC",
            )
            .map_err(|e| format!("Failed to prepare update job logs: {}", e))?;
        let log_rows = log_stmt
            .query_map(
                params![job_id, log_before_id, UPDATE_SNAPSHOT_LOG_PAGE_SIZE + 1],
                |row| {
                    Ok(UpdateJobLog {
                        id: row.get(0)?,
                        log_type: row.get(1)?,
                        message: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("Failed to read update job logs: {}", e))?;
        let mut logs = Vec::new();
        for row in log_rows {
            logs.push(row.map_err(|e| format!("Failed to read update job log: {}", e))?);
        }
        let previous_log_cursor = if logs.len() as i64 > UPDATE_SNAPSHOT_LOG_PAGE_SIZE {
            logs.remove(0);
            logs.first().map(|log| log.id)
        } else {
            None
        };

        let mut candidate_stmt = conn
            .prepare(
                "SELECT id, source, source_id, title, target_type, payload_json, status, error
                 FROM update_job_items
                 WHERE job_id = ?1 AND item_type = 'candidate' AND id > ?2
                 ORDER BY id ASC
                 LIMIT ?3",
            )
            .map_err(|e| format!("Failed to prepare update job candidates: {}", e))?;
        let candidate_rows = candidate_stmt
            .query_map(
                params![
                    job_id,
                    candidate_after_id.unwrap_or(0),
                    UPDATE_SNAPSHOT_CANDIDATE_PAGE_SIZE + 1
                ],
                |row| {
                    let id: i64 = row.get(0)?;
                    let source: String = row.get(1)?;
                    let source_id: String = row.get(2)?;
                    let title: String = row.get(3)?;
                    let target_type: String = row.get(4)?;
                    let payload_json: String = row.get(5)?;
                    let status: String = row.get(6)?;
                    let error: Option<String> = row.get(7)?;
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
                    // 古いジョブの候補には kind が無い。既定は「新作」。
                    let kind = payload
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("new")
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
                        kind,
                        error,
                    })
                },
            )
            .map_err(|e| format!("Failed to read update job candidates: {}", e))?;
        let mut candidates = Vec::new();
        for row in candidate_rows {
            candidates
                .push(row.map_err(|e| format!("Failed to read update job candidate: {}", e))?);
        }
        let next_candidate_cursor = if candidates.len() as i64 > UPDATE_SNAPSHOT_CANDIDATE_PAGE_SIZE
        {
            candidates.pop();
            candidates.last().map(|candidate| candidate.id)
        } else {
            None
        };

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
            next_candidate_cursor,
            previous_log_cursor,
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
        // Snapshots expose only the latest 300 records. Keeping a bounded
        // diagnostic tail prevents a long-running monitor from growing this
        // derived log table without limit.
        conn.execute(
            "DELETE FROM update_job_logs
             WHERE job_id = ?1 AND id NOT IN (
                SELECT id FROM update_job_logs
                WHERE job_id = ?1 ORDER BY id DESC LIMIT 2000
             )",
            params![job_id],
        )
        .map_err(|e| format!("Failed to trim update job logs: {}", e))?;
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
            // 状態を 'running' へ戻せるのは、まだ誰も止めていないときだけ。
            // 取り出しと同時に上書きすると、直前に届いたキャンセルや一時停止の
            // 要求がここで消える - 記録が消えた要求は、二度と気付かれない。
            conn.execute(
                "UPDATE update_jobs
                 SET active_label = ?1, status = 'running', updated_at = ?2
                 WHERE id = ?3 AND status IN ('queued', 'running')",
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
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "INSERT INTO update_job_items (
                job_id, item_type, source, source_id, target_type, title, payload_json, status
             )
             SELECT ?1, 'candidate', ?2, ?3, ?4, ?5, ?6, ?7
             WHERE NOT EXISTS (
                SELECT 1 FROM update_job_items
                WHERE job_id = ?1 AND item_type = 'candidate'
                  AND source = ?2 AND source_id = ?3
             )",
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
        Ok(changed > 0)
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
            // 候補は「見つけた結果」であって作業ではない。ただし保存するために
            // 待機列へ入れた候補は作業なので、そのぶん総数と処理済みに数える。
            // これを外すと、自動保存で 101 件を保存している最中に進捗が
            // 10/10（＝完了）と出てしまう。
            "UPDATE update_jobs
             SET totals = (SELECT COUNT(*) FROM update_job_items
                           WHERE job_id = ?1 AND (item_type != 'candidate' OR status != 'candidate')),
                 processed = (SELECT COUNT(*) FROM update_job_items
                              WHERE job_id = ?1 AND (item_type != 'candidate' OR status != 'candidate')
                                AND status IN ('done', 'saved', 'skipped', 'failed')),
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
        let unique: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            ids.iter().copied().filter(|id| seen.insert(*id)).collect()
        };
        if unique.is_empty() {
            return Ok(BulkMutationResult {
                matched_count: 0,
                changed_count: 0,
            });
        }

        // Resolve and validate every filesystem target before making any
        // change, then delete all relational rows atomically. A missing or
        // unsafe item therefore cannot leave a half-deleted bulk selection.
        let work_root_dirs = {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Bulk delete transaction failed: {e}"))?;
            let mut work_root_dirs = Vec::with_capacity(unique.len());
            for id in &unique {
                let (source, source_id): (String, String) = tx
                    .query_row(
                        "SELECT source, source_id FROM downloads WHERE id = ?1",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|e| format!("Download not found: {e}"))?;

                // Never infer the work root by walking up from json_path.
                // Legacy records can point directly to
                // `{source}/{source_id}/data.json`.
                work_root_dirs.push(validated_download_work_root(
                    &self.storage_dir,
                    &source,
                    &source_id,
                )?);
            }
            {
                let mut statement = tx
                    .prepare("DELETE FROM downloads WHERE id = ?1")
                    .map_err(|e| format!("Bulk delete prepare failed: {e}"))?;
                for id in &unique {
                    statement
                        .execute(params![id])
                        .map_err(|e| format!("Delete failed for {id}: {e}"))?;
                }
            }
            tx.commit()
                .map_err(|e| format!("Bulk delete commit failed: {e}"))?;
            work_root_dirs
        };

        // Keep slow, retrying filesystem work outside the database lock.
        for work_root_dir in work_root_dirs {
            if work_root_dir.exists() {
                if let Err(error) = remove_dir_all_resilient(&work_root_dir) {
                    log::warn!(
                        "Failed to remove work root directory {:?}: {}",
                        work_root_dir,
                        error
                    );
                }
                if let Some(source_dir) = work_root_dir.parent() {
                    let _ = remove_dir_resilient(source_dir);
                }
            }
        }

        // Sidecar indexes are not covered by the SQLite transaction, but both
        // cleanup attempts must run. Each implementation commits/invalidates
        // once for the entire selection rather than once per work.
        let lexical_result = super::tantivy_index::delete_documents(&self.storage_dir, &unique);
        let semantic_result = super::semantic_index::clear_documents(&self.storage_dir, &unique);
        self.invalidate_index_status();

        match (lexical_result, semantic_result) {
            (Err(lexical), Err(semantic)) => {
                return Err(format!(
                    "Downloads were deleted, but both search indexes failed to update: lexical: {lexical}; semantic: {semantic}"
                ));
            }
            (Err(error), _) => {
                return Err(format!(
                    "Downloads were deleted, but the lexical search index failed to update: {error}"
                ));
            }
            (_, Err(error)) => {
                return Err(format!(
                    "Downloads were deleted, but the semantic search index failed to update: {error}"
                ));
            }
            (Ok(()), Ok(())) => {}
        }

        Ok(BulkMutationResult {
            matched_count: unique.len() as i64,
            changed_count: unique.len() as i64,
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
        const STEPS: i64 = 7;
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
        report(4, "file-integrity");
        let file_integrity = {
            let conn = self.read_conn()?;
            check_library_file_integrity(&conn, &self.storage_dir, app_data)?
        };
        report(5, "storage-scan");
        let asset_lookup_conn = self.read_conn()?;
        let mut asset_lookup = asset_lookup_conn
            .prepare("SELECT 1 FROM assets WHERE local_path = ?1 LIMIT 1")
            .map_err(|error| format!("Asset diagnostic lookup prepare failed: {error}"))?;
        let mut is_known_asset = |path: &Path| {
            asset_lookup
                .query_row(params![path.to_string_lossy().as_ref()], |_| Ok(()))
                .optional()
                .map(|row| row.is_some())
                .map_err(|error| format!("Asset diagnostic lookup failed: {error}"))
        };
        let diagnostic_file_stats = collect_diagnostic_file_stats(
            &self.storage_dir,
            &app_data.join("search"),
            &mut is_known_asset,
        )?;
        log::debug!(
            "Library diagnostics scanned {} filesystem entries",
            diagnostic_file_stats.visited_entries
        );
        let mut file_issue_samples = file_integrity.issue_samples;
        let remaining_sample_capacity =
            DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT.saturating_sub(file_issue_samples.len());
        file_issue_samples.extend(
            diagnostic_file_stats
                .transient_file_samples
                .iter()
                .take(remaining_sample_capacity)
                .cloned(),
        );
        report(6, "index-size");
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
        Ok(LibraryDiagnostics {
            measured_at: chrono::Utc::now().to_rfc3339(),
            total_downloads,
            total_assets,
            total_versions,
            total_text_length,
            database_size_bytes: file_size_or_zero(&self.db_path),
            wal_size_bytes: file_size_or_zero(&wal_path),
            storage_size_bytes: diagnostic_file_stats.storage_size_bytes,
            lexical_index_size_bytes: diagnostic_file_stats.lexical_index_size_bytes,
            lexical_index_file_count: diagnostic_file_stats.lexical_index_file_count,
            lexical_index_segment_count: diagnostic_file_stats.lexical_index_segment_count,
            semantic_index_size_bytes: diagnostic_file_stats.semantic_index_size_bytes,
            sqlite_page_count,
            sqlite_free_pages,
            sqlite_cache_size_bytes,
            live_database_bytes,
            fragmentation_percent,
            orphan_asset_rows,
            orphan_asset_bytes,
            orphan_asset_files: diagnostic_file_stats.orphan_asset_files,
            orphan_asset_file_bytes: diagnostic_file_stats.orphan_asset_file_bytes,
            checked_file_references: file_integrity.checked_file_references,
            missing_json_files: file_integrity.missing_json_files,
            missing_asset_files: file_integrity.missing_asset_files,
            missing_profile_files: file_integrity.missing_profile_files,
            unsafe_referenced_files: file_integrity.unsafe_referenced_files,
            unreadable_referenced_files: file_integrity.unreadable_referenced_files,
            empty_referenced_files: file_integrity.empty_referenced_files,
            mismatched_asset_files: file_integrity.mismatched_asset_files,
            transient_files: diagnostic_file_stats.transient_files,
            transient_file_bytes: diagnostic_file_stats.transient_file_bytes,
            file_issue_samples,
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

        // 取り込んでいない改稿の件数。作品の列ではなく、更新確認が見つけた
        // 事実から数える。改稿の候補は必ず手元にある作品を指すので、結合は当たる。
        let revised = conn
            .query_row(
                "SELECT COUNT(*)
                   FROM update_candidates c
                   JOIN downloads d
                     ON d.source = c.source AND d.source_id = c.source_id
                  WHERE c.status = 'pending' AND c.kind = 'revision'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Revised shelf count failed: {e}"))?;

        Ok(LibraryShelfCounts {
            total,
            favorite,
            watched,
            reading,
            revised,
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
                        ) AS sample_title,
                        MAX(COALESCE(d.source_updated_at, d.source_created_at)) AS latest_source_updated_at,
                        s.is_concluded
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
                    latest_source_updated_at: row.get(9)?,
                    icon_path: None,
                    banner_path: None,
                    is_concluded: row.get(10).unwrap_or(None),
                })
            })
            .map_err(|e| format!("Entity series query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("Entity series row failed: {e}"))?);
        }
        Ok(out)
    }

    /// Returns every series connected to a person through a stable keyset.
    ///
    /// The opaque cursor is scoped to the person and query and pins SQLite's
    /// data generation. Continuing after a library mutation is rejected so a
    /// UI can restart instead of silently showing a gap or duplicate.
    pub fn list_entity_series_paged(
        &self,
        source: &str,
        person_key: &str,
        query: Option<&str>,
        limit: i64,
        cursor: Option<&str>,
    ) -> Result<EntitySeriesPage, String> {
        if source.trim().is_empty() || source.len() > 64 {
            return Err("Entity series source must be 1 to 64 bytes".to_string());
        }
        if person_key.trim().is_empty() || person_key.len() > 1_024 {
            return Err("Entity series source key must be 1 to 1024 bytes".to_string());
        }
        if query.is_some_and(|value| value.chars().count() > 200) {
            return Err("Entity series query must not exceed 200 characters".to_string());
        }
        if cursor.is_some_and(|value| value.len() > 64 * 1024) {
            return Err("Entity series cursor must not exceed 64 KiB".to_string());
        }
        let limit = limit.clamp(1, 200);
        let query = query.map(str::trim).unwrap_or("");
        let scope = entity_series_cursor_scope(source, person_key, query);
        let library_generation = self.library_generation()?;
        let decoded_cursor = decode_entity_series_cursor(cursor)?;
        if let Some(cursor) = decoded_cursor.as_ref() {
            if cursor.scope != scope {
                return Err("Entity series cursor belongs to another person or query".to_string());
            }
            if cursor.library_generation != library_generation {
                return Err(
                    "Library changed while paging entity series; restart the list".to_string(),
                );
            }
        }

        let has_query = !query.is_empty();
        let aggregate_sql = format!(
            "WITH entity_series AS (
                SELECT ds.series_source,
                       ds.series_key,
                       COALESCE(
                           MAX(NULLIF(s.title, '')),
                           MAX(NULLIF(ds.title, '')),
                           ds.series_key
                       ) AS display_name,
                       COUNT(DISTINCT d.id) AS work_count,
                       MAX(s.cover_path) AS cover_path,
                       MAX(s.description) AS description,
                       MAX(s.updated_at) AS updated_at,
                       MAX(d.downloaded_at) AS latest_downloaded_at,
                       MAX(COALESCE(d.source_updated_at, d.source_created_at)) AS latest_source_updated_at,
                       MAX(s.is_concluded) AS is_concluded,
                       MAX(COALESCE(d.source_created_at, d.downloaded_at)) AS latest_work_at
                FROM download_people dp
                JOIN downloads d ON d.id = dp.download_id
                JOIN download_series ds ON ds.download_id = d.id
                LEFT JOIN series s
                  ON s.source = ds.series_source AND s.source_key = ds.series_key
                WHERE dp.person_source = ? AND dp.person_key = ?
                GROUP BY ds.series_source, ds.series_key
                {query_having}
             )",
            query_having = if has_query {
                "HAVING COALESCE(
                            MAX(NULLIF(s.title, '')),
                            MAX(NULLIF(ds.title, '')),
                            ds.series_key
                        ) LIKE ? ESCAPE '\\' COLLATE NOCASE
                    OR COALESCE(MAX(s.description), '') LIKE ? ESCAPE '\\' COLLATE NOCASE"
            } else {
                ""
            }
        );
        let escaped_query = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let query_pattern = format!("%{escaped_query}%");
        let base_bind_values = || {
            let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
                Box::new(source.to_string()),
                Box::new(person_key.to_string()),
            ];
            if has_query {
                values.push(Box::new(query_pattern.clone()));
                values.push(Box::new(query_pattern.clone()));
            }
            values
        };

        let conn = self.read_conn()?;
        let total = if let Some(cursor) = decoded_cursor.as_ref() {
            cursor.total
        } else {
            let count_sql = format!("{aggregate_sql} SELECT COUNT(*) FROM entity_series");
            let values = base_bind_values();
            let refs = values
                .iter()
                .map(|value| value.as_ref())
                .collect::<Vec<_>>();
            conn.query_row(&count_sql, refs.as_slice(), |row| row.get(0))
                .map_err(|e| format!("Entity series count failed: {e}"))?
        };

        let mut page_sql = format!(
            "{aggregate_sql}
             SELECT es.series_source,
                    es.series_key,
                    es.display_name,
                    es.work_count,
                    es.cover_path,
                    es.description,
                    es.updated_at,
                    es.latest_downloaded_at,
                    (
                        SELECT d2.title
                        FROM downloads d2
                        JOIN download_series ds2 ON ds2.download_id = d2.id
                        WHERE ds2.series_source = es.series_source
                          AND ds2.series_key = es.series_key
                        ORDER BY COALESCE(ds2.content_order, 999999), d2.id
                        LIMIT 1
                    ) AS sample_title,
                    es.latest_source_updated_at,
                    es.is_concluded,
                    es.latest_work_at
             FROM entity_series es"
        );
        let mut bind_values = base_bind_values();
        if let Some(cursor) = decoded_cursor.as_ref() {
            page_sql.push_str(
                " WHERE es.latest_work_at < ?
                    OR (es.latest_work_at = ? AND es.work_count < ?)
                    OR (es.latest_work_at = ? AND es.work_count = ?
                        AND es.display_name COLLATE NOCASE > ? COLLATE NOCASE)
                    OR (es.latest_work_at = ? AND es.work_count = ?
                        AND es.display_name COLLATE NOCASE = ? COLLATE NOCASE
                        AND es.series_source > ?)
                    OR (es.latest_work_at = ? AND es.work_count = ?
                        AND es.display_name COLLATE NOCASE = ? COLLATE NOCASE
                        AND es.series_source = ? AND es.series_key > ?)",
            );
            bind_values.push(Box::new(cursor.latest_work_at.clone()));
            bind_values.push(Box::new(cursor.latest_work_at.clone()));
            bind_values.push(Box::new(cursor.count));
            bind_values.push(Box::new(cursor.latest_work_at.clone()));
            bind_values.push(Box::new(cursor.count));
            bind_values.push(Box::new(cursor.display_name.clone()));
            bind_values.push(Box::new(cursor.latest_work_at.clone()));
            bind_values.push(Box::new(cursor.count));
            bind_values.push(Box::new(cursor.display_name.clone()));
            bind_values.push(Box::new(cursor.source.clone()));
            bind_values.push(Box::new(cursor.latest_work_at.clone()));
            bind_values.push(Box::new(cursor.count));
            bind_values.push(Box::new(cursor.display_name.clone()));
            bind_values.push(Box::new(cursor.source.clone()));
            bind_values.push(Box::new(cursor.source_key.clone()));
        }
        page_sql.push_str(
            " ORDER BY es.latest_work_at DESC,
                       es.work_count DESC,
                       es.display_name COLLATE NOCASE ASC,
                       es.series_source ASC,
                       es.series_key ASC
              LIMIT ?",
        );
        bind_values.push(Box::new(limit + 1));
        let refs = bind_values
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>();
        let mut statement = conn
            .prepare(&page_sql)
            .map_err(|e| format!("Entity series page prepare failed: {e}"))?;
        let rows = statement
            .query_map(refs.as_slice(), |row| {
                Ok((
                    EntityFacet {
                        source: row.get(0)?,
                        source_key: row.get(1)?,
                        display_name: row.get(2)?,
                        count: row.get(3)?,
                        cover_path: row.get(4)?,
                        description: row.get(5)?,
                        updated_at: row.get(6)?,
                        latest_downloaded_at: row.get(7)?,
                        sample_title: row.get(8)?,
                        latest_source_updated_at: row.get(9)?,
                        icon_path: None,
                        banner_path: None,
                        is_concluded: row.get(10).unwrap_or(None),
                    },
                    row.get::<_, String>(11)?,
                ))
            })
            .map_err(|e| format!("Entity series page query failed: {e}"))?;
        let mut page = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Entity series page row failed: {e}"))?;
        let has_more = page.len() as i64 > limit;
        if has_more {
            page.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            page.last()
                .map(|(item, latest_work_at)| {
                    encode_entity_series_cursor(&EntitySeriesCursor {
                        scope: scope.clone(),
                        library_generation,
                        latest_work_at: latest_work_at.clone(),
                        count: item.count,
                        display_name: item.display_name.clone(),
                        source: item.source.clone(),
                        source_key: item.source_key.clone(),
                        total,
                    })
                })
                .transpose()?
        } else {
            None
        };
        if self.library_generation()? != library_generation {
            return Err("Library changed while paging entity series; restart the list".to_string());
        }

        Ok(EntitySeriesPage {
            items: page.into_iter().map(|(item, _)| item).collect(),
            next_cursor,
            total,
        })
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
        let query = input.query.as_deref().map(|value| {
            value
                .chars()
                .take(MAX_SAVED_SEARCH_QUERY)
                .collect::<String>()
        });

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
            24,
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
    /// エンティティ集計の「どの行をどう束ねるか」の部分。
    ///
    /// 1ページ分を取り出すクエリと、総件数を数えるクエリの両方がこれを使う。
    /// 二重に書くと、片方だけ条件が変わったときに「総数が、数えているはずの
    /// 行と一致しない」という最も気付きにくい壊れ方をするため、FROM・絞り込み
    /// ・GROUP BY は一箇所にまとめ、呼び出し側は何を SELECT してどう締めるかだけ
    /// を決める。
    ///
    /// `library_wheres` are the library's own filters, written against
    /// `downloads d` by [`append_library_filters`] and so usable here unchanged:
    /// both groupings already have that table in scope. They belong in WHERE
    /// rather than HAVING - the point is to group the works that pass the
    /// filter, so an author's count is how many of their works match.
    ///
    /// Binding is positional and in SQL order: the filters in WHERE come first,
    /// then the name/description LIKE in HAVING, then whatever the caller adds.
    /// 一覧そのものにかける条件。
    ///
    /// 「配下の作品の条件」（保存元・タグ・お気に入りなど）とは層が違う。
    /// 追いかけているかどうか、何作品以上あるか、完結しているか - どれも
    /// 束ね自身の性質で、作品には無い。
    fn entity_facet_scope_clauses(
        kind_is_series: bool,
        scope: Option<&EntityFacetScope>,
    ) -> EntityFacetClauses {
        let mut wheres = Vec::new();
        let mut where_binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut having = None;
        let mut having_binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let Some(scope) = scope else {
            return (wheres, where_binds, having, having_binds);
        };
        match scope.watch.as_deref() {
            // 監視は登録の有無と、止めているかどうかの2段。三つを別々に言える
            // ようにしてあるのは、「登録していない」と「止めている」が別の
            // 決定だから。
            Some("watched") => wheres.push("ut.id IS NOT NULL AND ut.enabled = 1".to_string()),
            Some("paused") => wheres.push("ut.id IS NOT NULL AND ut.enabled = 0".to_string()),
            Some("unwatched") => wheres.push("ut.id IS NULL".to_string()),
            _ => {}
        }
        if kind_is_series {
            if let Some(concluded) = scope.concluded {
                // NULL（まだ聞いていない）はどちらにも入らない。
                wheres.push("s.is_concluded = ?".to_string());
                where_binds.push(Box::new(if concluded { 1_i64 } else { 0_i64 }));
            }
        }
        if let Some(minimum) = scope.min_work_count.filter(|value| *value > 1) {
            having = Some(if kind_is_series {
                "COUNT(DISTINCT ds.download_id) >= ?".to_string()
            } else {
                "COUNT(DISTINCT d.id) >= ?".to_string()
            });
            having_binds.push(Box::new(minimum));
        }
        (wheres, where_binds, having, having_binds)
    }

    /// 一覧を引く土台（FROM から HAVING まで）と、その順に並んだ差し込み値。
    ///
    /// 値の順番を組み立てと同じ場所で決めているのは、WHERE と HAVING に
    /// またがって `?` が並ぶため。別々に足すと、検索語のある/なしで順番が
    /// 入れ替わり、条件が静かに別のものになる。
    fn entity_facet_source(
        kind: &str,
        like: Option<&str>,
        library_wheres: &[String],
        scope: Option<&EntityFacetScope>,
    ) -> Result<(String, Vec<Box<dyn rusqlite::types::ToSql>>), String> {
        let kind_is_series = matches!(kind, "series");
        if !kind_is_series && !matches!(kind, "person" | "people" | "author" | "authors") {
            return Err(format!("Unsupported entity facet kind: {}", kind));
        }
        let (scope_wheres, scope_where_binds, scope_having, scope_having_binds) =
            Self::entity_facet_scope_clauses(kind_is_series, scope);

        let mut all_wheres: Vec<String> = Vec::new();
        if kind_is_series {
            // 作者側は「作者名のある作品」だけを数える。ここは元からの条件。
        } else {
            all_wheres.push("d.author_id IS NOT NULL AND d.author_id != ''".to_string());
            all_wheres.push("d.author_name IS NOT NULL AND d.author_name != ''".to_string());
        }
        all_wheres.extend(library_wheres.iter().cloned());
        all_wheres.extend(scope_wheres);
        let where_clause = if all_wheres.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", all_wheres.join("
                   AND "))
        };

        let mut having_parts: Vec<String> = Vec::new();
        if like.is_some() {
            having_parts.push(if kind_is_series {
                "(COALESCE(s.title, ds.title) LIKE ? ESCAPE '\\'
                       OR COALESCE(s.description, '') LIKE ? ESCAPE '\\')"
                    .to_string()
            } else {
                "(COALESCE(p.display_name, d.author_name) LIKE ? ESCAPE '\\'
                       OR COALESCE(p.description, '') LIKE ? ESCAPE '\\')"
                    .to_string()
            });
        }
        if let Some(clause) = scope_having {
            having_parts.push(clause);
        }
        let having = if having_parts.is_empty() {
            String::new()
        } else {
            format!("HAVING {}", having_parts.join("
                     AND "))
        };

        let sql = if kind_is_series {
            format!(
                "FROM download_series ds
                 LEFT JOIN series s ON s.source = ds.series_source AND s.source_key = ds.series_key
                 JOIN downloads d ON d.id = ds.download_id
                 LEFT JOIN update_targets ut ON ut.target_type = 'series'
                      AND ut.source = ds.series_source AND ut.source_key = ds.series_key
                 {where_clause}
                 GROUP BY ds.series_source, ds.series_key, COALESCE(s.title, ds.title), s.cover_path, s.description, s.updated_at, s.is_concluded
                 {having}"
            )
        } else {
            format!(
                "FROM downloads d
                 LEFT JOIN people p ON p.source = d.source AND p.source_key = d.author_id
                 LEFT JOIN update_targets ut ON ut.target_type = 'author'
                      AND ut.source = d.source AND ut.source_key = d.author_id
                 {where_clause}
                 GROUP BY d.source, d.author_id, COALESCE(p.display_name, d.author_name), p.icon_path, p.cover_path, p.description, p.updated_at
                 {having}"
            )
        };

        // 値は SQL に `?` が現れる順。WHERE（絞り込み → 一覧の条件）、
        // HAVING（検索語 → 件数）。
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        binds.extend(scope_where_binds);
        if let Some(pattern) = like {
            binds.push(Box::new(pattern.to_string()));
            binds.push(Box::new(pattern.to_string()));
        }
        binds.extend(scope_having_binds);
        Ok((sql, binds))
    }

    /// The library filters an entity listing is narrowed by, if any.
    ///
    /// The same builder the works listing uses, so "お気に入りだけ" means the
    /// same thing on every tab rather than one tab's idea of it.
    fn entity_facet_filters(
        filters: Option<&SearchV2Params>,
    ) -> (Vec<String>, Vec<Box<dyn rusqlite::types::ToSql>>) {
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(params) = filters {
            append_library_filters(params, &mut wheres, &mut bind_values);
        }
        (wheres, bind_values)
    }

    /// 作者・シリーズが何件あるか。
    ///
    /// 一覧そのものは1ページずつしか読まないので、これを聞かないと「全何ページ
    /// あるか」が分からない。ページ番号で移動する以上、最後のページが存在する
    /// ことは分かっていなければならない。
    pub fn count_entity_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        filters: Option<&SearchV2Params>,
        scope: Option<&EntityFacetScope>,
    ) -> Result<i64, String> {
        let conn = self.read_conn()?;
        let filter = query.map(str::trim).unwrap_or("");
        let like = (!filter.is_empty())
            .then(|| format!("%{}%", filter.replace('%', "\\%").replace('_', "\\_")));
        let (library_wheres, mut bind_values) = Self::entity_facet_filters(filters);
        let (source, source_binds) =
            Self::entity_facet_source(kind, like.as_deref(), &library_wheres, scope)?;
        let sql = format!("SELECT COUNT(*) FROM (SELECT 1 {source})");
        bind_values.extend(source_binds);
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|value| value.as_ref()).collect();
        conn.query_row(&sql, refs.as_slice(), |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Entity facet count failed: {}", e))
    }

    /// 一覧の並べ替えを決める。
    ///
    /// SQL に外から来た文字列を混ぜないための関門でもある。知らない指定は
    /// 既定（作品が多い順）へ落とす - 「選べるのに効かない」より、「選べない
    /// ものは既定になる」ほうがまだ読める。
    ///
    /// 二番手・三番手まで決めてあるのは、同じ値が並んだときに順番が毎回
    /// 変わらないようにするため。ページ送りは位置で切るので、境目が揺れると
    /// 同じ作者が二度出たり、抜けたりする。
    fn entity_facet_order_clause(sort_by: Option<&str>, sort_order: Option<&str>, name_column: &str) -> String {
        let descending = !matches!(sort_order, Some("asc"));
        let dir = if descending { "DESC" } else { "ASC" };
        match sort_by.unwrap_or("work_count") {
            "downloaded_at" => format!(
                "ORDER BY latest_downloaded_at {dir}, count DESC, {name_column} ASC"
            ),
            "source_updated_at" => format!(
                "ORDER BY latest_source_updated_at {dir}, count DESC, {name_column} ASC"
            ),
            // 名前だけは昇順が「正しい」と読めるので、指定が無ければ昇順。
            "name" | "title" | "author_name" => {
                let dir = if matches!(sort_order, Some("desc")) { "DESC" } else { "ASC" };
                format!("ORDER BY {name_column} COLLATE NOCASE {dir}, count DESC")
            }
            _ => format!("ORDER BY count {dir}, {name_column} ASC"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn search_entity_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        limit: i64,
        offset: i64,
        filters: Option<&SearchV2Params>,
        sort_by: Option<&str>,
        sort_order: Option<&str>,
        scope: Option<&EntityFacetScope>,
    ) -> Result<Vec<EntityFacet>, String> {
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let filter = query.map(str::trim).unwrap_or("");
        let generation = self.library_generation()?;
        // The library filters are part of what was asked for, so they are part
        // of the key. Left out of it, a favourites-only listing would be handed
        // the rows cached for the unfiltered one. 並べ替えも同じ理由で鍵に
        // 入れる - 同じ条件でも順番が違えば、それは違う答えである。
        let cache_key = format!(
            "{kind}\u{1f}{filter}\u{1f}{limit}\u{1f}{offset}\u{1f}{filters:?}\u{1f}{sort_by:?}\u{1f}{sort_order:?}\u{1f}{scope:?}"
        );
        if let Ok(mut cache) = self.entity_facet_cache.lock() {
            if let Some(cached) = cache.get(&cache_key, generation) {
                return Ok(cached);
            }
        }
        let conn = self.read_conn()?;
        let like = (!filter.is_empty())
            .then(|| format!("%{}%", filter.replace('%', "\\%").replace('_', "\\_")));
        let (library_wheres, mut bind_values) = Self::entity_facet_filters(filters);
        let (source, source_binds) =
            Self::entity_facet_source(kind, like.as_deref(), &library_wheres, scope)?;

        let sql = match kind {
            "person" | "people" | "author" | "authors" => format!(
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
                        p.cover_path AS banner_path,
                        MAX(COALESCE(d.source_updated_at, d.source_created_at)) AS latest_source_updated_at,
                        NULL AS is_concluded
                     {source}
                     {order}
                     LIMIT ? OFFSET ?",
                order = Self::entity_facet_order_clause(sort_by, sort_order, "display_name")
            ),
            "series" => format!(
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
                        s.cover_path AS banner_path,
                        MAX(COALESCE(d.source_updated_at, d.source_created_at)) AS latest_source_updated_at,
                        s.is_concluded
                     {source}
                     {order}
                     LIMIT ? OFFSET ?",
                order = Self::entity_facet_order_clause(sort_by, sort_order, "title")
            ),
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
                latest_source_updated_at: row.get(11)?,
                icon_path: row.get(9)?,
                banner_path: row.get(10)?,
                is_concluded: row.get(12).unwrap_or(None),
            })
        };
        bind_values.extend(source_binds);
        bind_values.push(Box::new(limit));
        bind_values.push(Box::new(offset));
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|value| value.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), map_row)
            .map_err(|e| format!("Entity facet search failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Entity facet row read failed: {}", e))?);
        }
        if let Ok(mut cache) = self.entity_facet_cache.lock() {
            cache.insert(
                cache_key,
                generation,
                results.clone(),
                entity_facets_bytes(&results),
            );
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
        let generation = self.library_generation()?;
        let cache_key = if include_entities { "full" } else { "light" };
        if let Ok(mut cache) = self.filter_facets_cache.lock() {
            if let Some(cached) = cache.get(cache_key, generation) {
                return Ok(cached);
            }
        }
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
                        // 絞り込みの引き出しに並べる名前だけを作る一覧なので、
                        // 並べ替えと完結の判定は要らない。
                        latest_source_updated_at: None,
                        icon_path: row.get(9)?,
                        banner_path: row.get(10)?,
                        is_concluded: None,
                    })
                })
                .map_err(|e| format!("Entity facet query failed: {}", e))?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(|e| format!("Entity facet row read failed: {}", e))?);
            }
            Ok(results)
        };

        let result = FilterFacets {
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
        };
        drop(conn);
        if let Ok(mut cache) = self.filter_facets_cache.lock() {
            cache.insert(
                cache_key.to_string(),
                generation,
                result.clone(),
                filter_facets_bytes(&result),
            );
        }
        Ok(result)
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

    /// 本文まで突き合わせたときの覚え書きを残す。
    ///
    /// `meta_hash` は本文を除いたメタデータの指紋、`deep_checked_at` はこの
    /// 突き合わせを行った時刻。次の更新確認は、指紋が同じで時刻も新しければ
    /// 本文を取りに行かずに済ませられる。
    pub fn set_download_meta_state(
        &self,
        download_id: i64,
        meta_hash: Option<&str>,
        deep_checked_at: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET meta_hash = ?1, last_deep_checked_at = ?2 WHERE id = ?3",
            params![meta_hash, deep_checked_at, download_id],
        )
        .map_err(|e| format!("Failed to set meta state: {}", e))?;
        Ok(())
    }

    /// 更新確認が使う覚え書き。(メタデータの指紋, 最後に本文を見た時刻)
    pub fn get_download_meta_state(
        &self,
        download_id: i64,
    ) -> Result<(Option<String>, Option<String>), String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT meta_hash, last_deep_checked_at FROM downloads WHERE id = ?1",
            params![download_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Failed to read meta state: {}", e))
    }

    /// 取得元での最終更新を覚える。
    ///
    /// pixiv の作品詳細APIには更新時刻に当たるフィールドが無く、この値は
    /// web の一覧からしか得られない。次の確認は、この一件と一覧の
    /// `updateDate` を比べるだけで「変わっていない」と判断できる。
    pub fn set_download_source_updated_at(
        &self,
        download_id: i64,
        source_updated_at: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET source_updated_at = ?1 WHERE id = ?2",
            params![source_updated_at, download_id],
        )
        .map_err(|e| format!("Failed to set source updated at: {}", e))?;
        Ok(())
    }

    /// その作者の、保存済み pixiv 作品。(作品ID, 取得元での最終更新, 保存日時)
    ///
    /// 作者を監視しているなら、その作者の作品の改稿も同じ確認で拾えるべき。
    /// web の一覧は 100 件をまとめて返すので、全部を並べても往復は 1% で済む。
    pub fn pixiv_works_for_author(
        &self,
        author_id: &str,
    ) -> Result<Vec<(String, Option<String>, String)>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT source_id, source_updated_at, downloaded_at
                   FROM downloads
                  WHERE source = 'pixiv' AND author_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![author_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to list author works: {}", e))
    }

    /// 更新監視（トグル）を設定する。
    ///
    /// 監視対象の一覧（update_targets）には触れない。作品の監視は
    /// `downloads.watch_updates` だけで表せるうえ、そこへ 'work' の行を作ると
    /// 「自分で選んだ作者・シリーズ」の一覧に作品名が紛れ込む。
    pub fn set_watch_updates(&self, download_id: i64, watch: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET watch_updates = ?1 WHERE id = ?2",
            params![if watch { 1i64 } else { 0i64 }, download_id],
        )
        .map_err(|e| format!("Failed to set watch_updates: {}", e))?;
        Ok(())
    }

    pub fn set_watch_updates_for_search(
        &self,
        params: &SearchV2Params,
        watch: bool,
    ) -> Result<BulkMutationResult, String> {
        let snapshot = self.collect_search_match_snapshot(params, 1_000)?;
        let matched_count = snapshot.row_count;
        if matched_count == 0 {
            return Ok(BulkMutationResult {
                matched_count: 0,
                changed_count: 0,
            });
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        if self.library_generation()? != snapshot.library_generation {
            return Err("Library changed while preparing the bulk selection; retry".to_string());
        }
        conn.execute(
            "ATTACH DATABASE ?1 AS bulk_selection",
            params![snapshot.path.to_string_lossy().to_string()],
        )
        .map_err(|e| format!("Bulk selection attach failed: {e}"))?;
        let update_result: Result<i64, String> = (|| {
            let tx = conn
                .transaction()
                .map_err(|e| format!("Bulk watch update transaction failed: {e}"))?;
            let watch_value = i64::from(watch);
            let changed_count =
                tx.execute(
                    "UPDATE downloads
                     SET watch_updates = ?1
                     WHERE id IN (SELECT id FROM bulk_selection.bulk_matches)",
                    params![watch_value],
                )
                .map_err(|e| format!("Bulk watch update failed: {e}"))? as i64;
            tx.commit()
                .map_err(|e| format!("Bulk watch update commit failed: {e}"))?;
            Ok(changed_count)
        })();
        let detach_result = conn
            .execute_batch("DETACH DATABASE bulk_selection")
            .map_err(|e| format!("Bulk selection detach failed: {e}"));
        let changed_count = update_result?;
        detach_result?;
        Ok(BulkMutationResult {
            matched_count,
            changed_count,
        })
    }

    pub fn delete_downloads_for_search(
        &self,
        params: &SearchV2Params,
    ) -> Result<BulkMutationResult, String> {
        let snapshot = self.collect_search_match_snapshot(params, 1_000)?;
        let matched_count = snapshot.row_count;
        if matched_count == 0 {
            return Ok(BulkMutationResult {
                matched_count: 0,
                changed_count: 0,
            });
        }

        // Validate every filesystem target before committing relational
        // deletion, but do not retain O(N) paths in memory.
        {
            let guard = snapshot
                .connection
                .lock()
                .map_err(|e| format!("Bulk selection connection lock failed: {e}"))?;
            let conn = guard
                .as_ref()
                .ok_or_else(|| "Bulk selection snapshot was closed".to_string())?;
            let mut statement = conn
                .prepare("SELECT source, source_id FROM bulk_matches ORDER BY id")
                .map_err(|e| format!("Bulk delete validation prepare failed: {e}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Bulk delete validation query failed: {e}"))?;
            for row in rows {
                let (source, source_id) =
                    row.map_err(|e| format!("Bulk delete validation row failed: {e}"))?;
                validated_download_work_root(&self.storage_dir, &source, &source_id)?;
            }
        }

        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            if self.library_generation()? != snapshot.library_generation {
                return Err("Library changed while preparing the bulk selection; retry".to_string());
            }
            conn.execute(
                "ATTACH DATABASE ?1 AS bulk_selection",
                params![snapshot.path.to_string_lossy().to_string()],
            )
            .map_err(|e| format!("Bulk selection attach failed: {e}"))?;
            let delete_result: Result<i64, String> = (|| {
                let tx = conn
                    .transaction()
                    .map_err(|e| format!("Bulk delete transaction failed: {e}"))?;
                let changed_count = tx
                    .execute(
                        "DELETE FROM downloads
                         WHERE id IN (SELECT id FROM bulk_selection.bulk_matches)",
                        [],
                    )
                    .map_err(|e| format!("Bulk relational delete failed: {e}"))?
                    as i64;
                tx.commit()
                    .map_err(|e| format!("Bulk delete commit failed: {e}"))?;
                Ok(changed_count)
            })();
            let detach_result = conn
                .execute_batch("DETACH DATABASE bulk_selection")
                .map_err(|e| format!("Bulk selection detach failed: {e}"));
            let changed_count = delete_result?;
            detach_result?;
            if changed_count != matched_count {
                return Err(format!(
                    "Bulk delete changed {changed_count} rows after selecting {matched_count}; indexes were not modified"
                ));
            }
        }

        // Keep the lexical writer alive for one commit while streaming ids
        // directly from the disk snapshot. Semantic cleanup is chunked because
        // its current API intentionally accepts a bounded slice.
        let lexical_result =
            super::tantivy_index::delete_documents_from_snapshot(&self.storage_dir, &snapshot.path);
        let semantic_result = self.clear_semantic_documents_from_snapshot(&snapshot);

        // Slow filesystem work remains outside the database lock and is also
        // streamed from the snapshot instead of retaining every path.
        {
            let guard = snapshot
                .connection
                .lock()
                .map_err(|e| format!("Bulk selection connection lock failed: {e}"))?;
            let conn = guard
                .as_ref()
                .ok_or_else(|| "Bulk selection snapshot was closed".to_string())?;
            let mut statement = conn
                .prepare("SELECT source, source_id FROM bulk_matches ORDER BY id")
                .map_err(|e| format!("Bulk filesystem cleanup prepare failed: {e}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Bulk filesystem cleanup query failed: {e}"))?;
            for row in rows {
                let (source, source_id) =
                    row.map_err(|e| format!("Bulk filesystem cleanup row failed: {e}"))?;
                let work_root_dir =
                    validated_download_work_root(&self.storage_dir, &source, &source_id)?;
                if work_root_dir.exists() {
                    if let Err(error) = remove_dir_all_resilient(&work_root_dir) {
                        log::warn!(
                            "Failed to remove work root directory {:?}: {}",
                            work_root_dir,
                            error
                        );
                    }
                    if let Some(source_dir) = work_root_dir.parent() {
                        let _ = remove_dir_resilient(source_dir);
                    }
                }
            }
        }
        self.invalidate_index_status();
        match (lexical_result, semantic_result) {
            (Err(lexical), Err(semantic)) => Err(format!(
                "Downloads were deleted, but both search indexes failed to update: lexical: {lexical}; semantic: {semantic}"
            )),
            (Err(error), _) => Err(format!(
                "Downloads were deleted, but the lexical search index failed to update: {error}"
            )),
            (_, Err(error)) => Err(format!(
                "Downloads were deleted, but the semantic search index failed to update: {error}"
            )),
            (Ok(()), Ok(())) => Ok(BulkMutationResult {
                matched_count,
                changed_count: matched_count,
            }),
        }
    }

    fn clear_semantic_documents_from_snapshot(
        &self,
        snapshot: &DiskSearchSnapshot,
    ) -> Result<(), String> {
        const CHUNK_SIZE: i64 = 1_000;
        let guard = snapshot
            .connection
            .lock()
            .map_err(|e| format!("Bulk selection connection lock failed: {e}"))?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| "Bulk selection snapshot was closed".to_string())?;
        let mut after_id = 0i64;
        loop {
            let mut statement = conn
                .prepare(
                    "SELECT id FROM bulk_matches
                     WHERE id > ?1 ORDER BY id LIMIT ?2",
                )
                .map_err(|e| format!("Semantic cleanup chunk prepare failed: {e}"))?;
            let rows = statement
                .query_map(params![after_id, CHUNK_SIZE], |row| row.get::<_, i64>(0))
                .map_err(|e| format!("Semantic cleanup chunk query failed: {e}"))?;
            let mut ids = Vec::with_capacity(CHUNK_SIZE as usize);
            for row in rows {
                ids.push(row.map_err(|e| format!("Semantic cleanup id row failed: {e}"))?);
            }
            let Some(last) = ids.last().copied() else {
                break;
            };
            super::semantic_index::clear_documents(&self.storage_dir, &ids)?;
            after_id = last;
        }
        Ok(())
    }

    fn collect_search_match_snapshot(
        &self,
        params: &SearchV2Params,
        page_size: i64,
    ) -> Result<Arc<DiskSearchSnapshot>, String> {
        let library_generation = self.library_generation()?;
        let index_generation = super::tantivy_index::index_generation(&self.storage_dir)?;
        let (id, path, connection) =
            create_search_snapshot_connection(&self.storage_dir, &self.db_path)?;
        let build_result = (|| {
            {
                let guard = connection
                    .lock()
                    .map_err(|e| format!("Bulk selection connection lock failed: {e}"))?;
                let conn = guard
                    .as_ref()
                    .ok_or_else(|| "Bulk selection snapshot was closed".to_string())?;
                conn.execute_batch(
                    "CREATE TABLE bulk_matches (
                        id INTEGER PRIMARY KEY,
                        source TEXT NOT NULL,
                        source_id TEXT NOT NULL
                     ) WITHOUT ROWID;
                     BEGIN IMMEDIATE;",
                )
                .map_err(|e| format!("Bulk selection snapshot setup failed: {e}"))?;
            }

            let mut bulk_params = params.clone();
            bulk_params.cursor = None;
            bulk_params.offset = None;
            let page_size = page_size.clamp(1, 1_000);
            bulk_params.limit = Some(page_size);
            bulk_params.projection = Some("bulk".to_string());
            let mut seen_cursors = HashSet::new();
            loop {
                let result = self.search_downloads_v2_inner(&bulk_params, page_size)?;
                {
                    let guard = connection
                        .lock()
                        .map_err(|e| format!("Bulk selection connection lock failed: {e}"))?;
                    let conn = guard
                        .as_ref()
                        .ok_or_else(|| "Bulk selection snapshot was closed".to_string())?;
                    insert_bulk_match_entries(conn, &result.items)?;
                    ensure_search_snapshot_budget(conn, &self.storage_dir)?;
                }
                let Some(next_cursor) = result.next_cursor else {
                    break;
                };
                if !seen_cursors.insert(next_cursor.clone()) {
                    return Err("Bulk search paging returned a repeated cursor".to_string());
                }
                bulk_params.cursor = Some(next_cursor);
            }

            let guard = connection
                .lock()
                .map_err(|e| format!("Bulk selection connection lock failed: {e}"))?;
            let conn = guard
                .as_ref()
                .ok_or_else(|| "Bulk selection snapshot was closed".to_string())?;
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("Bulk selection snapshot commit failed: {e}"))?;
            let row_count = conn
                .query_row("SELECT COUNT(*) FROM bulk_matches", [], |row| row.get(0))
                .map_err(|e| format!("Bulk selection count failed: {e}"))?;
            Ok((
                row_count,
                ensure_search_snapshot_budget(conn, &self.storage_dir)?,
            ))
        })();

        let (row_count, disk_bytes) = match build_result {
            Ok(result) => result,
            Err(error) => {
                if let Ok(guard) = connection.lock() {
                    if let Some(conn) = guard.as_ref() {
                        let _ = conn.execute_batch("ROLLBACK");
                    }
                }
                close_snapshot_connection(&connection);
                cleanup_search_snapshot_path(&path);
                return Err(error);
            }
        };
        if self.library_generation()? != library_generation
            || super::tantivy_index::index_generation(&self.storage_dir)? != index_generation
        {
            close_snapshot_connection(&connection);
            cleanup_search_snapshot_path(&path);
            return Err("Library changed while preparing the bulk selection; retry".to_string());
        }
        Ok(Arc::new(DiskSearchSnapshot {
            id,
            key: "bulk".to_string(),
            library_generation,
            index_generation,
            row_count,
            disk_bytes,
            expires_at: Instant::now() + SEARCH_SNAPSHOT_TTL,
            path,
            connection,
        }))
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

/// コレクション要約の集計は必ずこの一箇所で組み立てる。
///
/// 所属条件は EXISTS で書き、メンバー表を条件付きで join し直さない。条件に
/// 一致する作品が同じコレクションに複数あると、join では行が複製されて作品数と
/// 総文字数が水増しされるためである。列の並びは
/// work_collection_summary_from_row の添字と対応しているので、片方だけを
/// 変えてはいけない。
const COLLECTION_SUMMARY_SELECT: &str = "
    SELECT c.id, c.name, c.description, c.collection_kind,
           c.cover_download_id, cover.cover_path, c.revision,
           COUNT(m.source_id),
           SUM(CASE WHEN m.download_id IS NOT NULL THEN 1 ELSE 0 END),
           COALESCE(SUM(d.text_length), 0), c.created_at, c.updated_at
      FROM work_collections c
      LEFT JOIN work_collection_members m ON m.collection_id = c.id
      LEFT JOIN downloads d ON d.id = m.download_id
      LEFT JOIN downloads cover ON cover.id = c.cover_download_id";

/// 同名・同時刻のコレクションでも並びが揺れないよう c.id まで比較する。
const COLLECTION_SUMMARY_TAIL: &str = "
      GROUP BY c.id
      ORDER BY c.updated_at DESC, c.name COLLATE NOCASE ASC, c.id ASC";

fn work_collection_summary_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkCollectionSummary> {
    Ok(WorkCollectionSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        collection_kind: row.get(3)?,
        cover_download_id: row.get(4)?,
        cover_path: row.get(5)?,
        revision: row.get(6)?,
        member_count: row.get(7)?,
        available_count: row.get(8)?,
        total_text_length: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn work_collection_member_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkCollectionMember> {
    Ok(WorkCollectionMember {
        collection_id: row.get(0)?,
        source: row.get(1)?,
        source_id: row.get(2)?,
        download_id: row.get(3)?,
        title: row.get(4)?,
        author_name: row.get(5)?,
        cover_path: row.get(6)?,
        text_length: row.get(7)?,
        position: row.get(8)?,
        member_role: row.get(9)?,
        added_by: row.get(10)?,
        pinned: row.get::<_, i64>(11)? != 0,
        note: row.get(12)?,
        missing: row.get::<_, i64>(13)? != 0,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn work_link_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkLink> {
    Ok(WorkLink {
        id: row.get(0)?,
        from_source: row.get(1)?,
        from_source_id: row.get(2)?,
        from_download_id: row.get(3)?,
        to_source: row.get(4)?,
        to_source_id: row.get(5)?,
        to_download_id: row.get(6)?,
        relation_type: row.get(7)?,
        evidence_type: row.get(8)?,
        anchor_text: row.get(9)?,
        context_text: row.get(10)?,
        confidence: row.get(11)?,
        status: row.get(12)?,
        discovered_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

#[derive(Debug, Clone)]
struct SuggestionWork {
    id: i64,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    cover_path: Option<String>,
    text_length: i64,
    published_at: String,
}

const COLLECTION_SUGGEST_RULE_VERSION: &str = "collection-suggest-v2";
const LINK_TRAVERSAL_MAX_DEPTH: usize = 8;
const LINK_TRAVERSAL_MAX_WORKS: usize = 240;
const LINK_INCOMING_SEARCH_LIMIT: usize = 40;
const LINK_REFRESH_BUDGET: usize = 160;

#[derive(Debug, Clone, Copy)]
struct LinkTraversalEvidence {
    depth: usize,
    confidence: f64,
}

#[derive(Debug, Clone)]
struct StoredWorkLinkEdge {
    from_id: i64,
    to_id: i64,
    relation_type: String,
    confidence: f64,
}

#[derive(Debug, Clone)]
struct RankedSuggestionMember {
    work: SuggestionWork,
    member_score: f64,
    /// 番号なしの初回を兄弟と結びつけるための比較用語幹。
    title_stem: String,
    /// 既定でチェックを入れるか。設計提案 6-5 は強い候補だけを全選択の下書きに
    /// し、弱い候補は個別確認を求めている。公式シリーズに同居しているだけの
    /// 作品まで既定で選ぶと、短編集を丸ごと取り込んでしまう。
    default_selected: bool,
    evidence: Vec<CollectionSuggestionEvidence>,
    series_order: Option<i64>,
    link_order: Option<i64>,
    link_depth: Option<usize>,
    episode_order: Option<i64>,
}

fn load_suggestion_works(conn: &Connection, ids: &[i64]) -> Result<Vec<SuggestionWork>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded = serde_json::to_string(ids)
        .map_err(|e| format!("Failed to encode suggestion work IDs: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, source, source_id, title, author_name, COALESCE(author_id, ''),
                    cover_path, text_length,
                    COALESCE(source_created_at, downloaded_at, '')
             FROM downloads
             WHERE id IN (SELECT value FROM json_each(?1))",
        )
        .map_err(|e| format!("Failed to prepare suggestion works: {e}"))?;
    let works = stmt
        .query_map(params![encoded], |row| {
            Ok(SuggestionWork {
                id: row.get(0)?,
                source: row.get(1)?,
                source_id: row.get(2)?,
                title: row.get(3)?,
                author_name: row.get(4)?,
                author_id: row.get(5)?,
                cover_path: row.get(6)?,
                text_length: row.get(7)?,
                published_at: row.get(8)?,
            })
        })
        .map_err(|e| format!("Failed to query suggestion works: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read suggestion works: {e}"))?;
    Ok(works)
}

fn load_link_edges_touching(
    conn: &Connection,
    download_ids: &[i64],
) -> Result<Vec<StoredWorkLinkEdge>, String> {
    if download_ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded = serde_json::to_string(download_ids)
        .map_err(|e| format!("Failed to encode linked work IDs: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT from_download_id, to_download_id, relation_type, confidence
             FROM work_links
             WHERE status != 'rejected'
               AND from_download_id IS NOT NULL
               AND to_download_id IS NOT NULL
               AND (
                 from_download_id IN (SELECT value FROM json_each(?1))
                 OR to_download_id IN (SELECT value FROM json_each(?1))
               )
             ORDER BY confidence DESC, id DESC
             LIMIT 4000",
        )
        .map_err(|e| format!("Failed to prepare linked work traversal: {e}"))?;
    let edges = stmt
        .query_map(params![encoded], |row| {
            Ok(StoredWorkLinkEdge {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                relation_type: row.get(2)?,
                confidence: row.get(3)?,
            })
        })
        .map_err(|e| format!("Failed to query linked work traversal: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read linked work traversal: {e}"))?;
    Ok(edges)
}

/// Build the connected component around the selected works before considering
/// fuzzy title or semantic matches. Links are walked in both directions: an
/// end chapter can therefore find a previous chapter that links to it, and
/// that previous chapter becomes a new starting point for the next hop.
///
/// Older libraries can have a complete text index but no `work_links` rows.
/// The focused reverse lookup refreshes only documents that contain a frontier
/// work URL, and all budgets are bounded so a suggestion cannot reparse the
/// entire library while holding up normal saves.
fn discover_linked_collection_component(
    db: &Database,
    seeds: &[SuggestionWork],
    already_refreshed: &mut HashSet<i64>,
) -> Result<HashMap<i64, LinkTraversalEvidence>, String> {
    let mut paths = seeds
        .iter()
        .map(|seed| {
            (
                seed.id,
                LinkTraversalEvidence {
                    depth: 0,
                    confidence: 1.0,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut frontier = seeds.iter().map(|seed| seed.id).collect::<Vec<_>>();
    let mut refresh_budget = LINK_REFRESH_BUDGET;

    for _ in 0..LINK_TRAVERSAL_MAX_DEPTH {
        if frontier.is_empty() || paths.len() >= LINK_TRAVERSAL_MAX_WORKS {
            break;
        }

        // Outgoing links from every newly discovered work are authoritative
        // enough to parse first and usually reveal the next hop immediately.
        for download_id in &frontier {
            if refresh_budget == 0 || !already_refreshed.insert(*download_id) {
                continue;
            }
            refresh_budget -= 1;
            if let Err(error) = db.refresh_work_links(*download_id) {
                log::debug!("Collection link refresh skipped for {download_id}: {error}");
            }
        }

        // Backfill incoming one-way links for older data. Search hits are not
        // accepted as members by themselves; after refresh they still need an
        // actual normalized work-link edge to enter the component below.
        if refresh_budget > 0 {
            let frontier_works = {
                let conn = db.read_conn()?;
                load_suggestion_works(&conn, &frontier)?
            };
            for work in frontier_works {
                if refresh_budget == 0 {
                    break;
                }
                let source_url =
                    source_url_for_download(&work.source, &work.source_id, &work.author_id);
                let result = match super::tantivy_index::search_with_total(
                    &db.storage_dir,
                    &source_url,
                    LINK_INCOMING_SEARCH_LIMIT,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        log::debug!("Incoming collection-link lookup skipped: {error}");
                        continue;
                    }
                };
                for hit in result.hits {
                    if refresh_budget == 0 {
                        break;
                    }
                    if hit.download_id == work.id || !already_refreshed.insert(hit.download_id) {
                        continue;
                    }
                    refresh_budget -= 1;
                    if let Err(error) = db.refresh_work_links(hit.download_id) {
                        log::debug!(
                            "Incoming collection-link refresh skipped for {}: {error}",
                            hit.download_id
                        );
                    }
                }
            }
        }

        let frontier_set = frontier.iter().copied().collect::<HashSet<_>>();
        let edges = {
            let conn = db.read_conn()?;
            load_link_edges_touching(&conn, &frontier)?
        };
        let mut next = Vec::new();
        for edge in edges {
            for (current, neighbour) in [(edge.from_id, edge.to_id), (edge.to_id, edge.from_id)] {
                if !frontier_set.contains(&current)
                    || paths.contains_key(&neighbour)
                    || paths.len() >= LINK_TRAVERSAL_MAX_WORKS
                {
                    continue;
                }
                let Some(parent) = paths.get(&current).copied() else {
                    continue;
                };
                paths.insert(
                    neighbour,
                    LinkTraversalEvidence {
                        depth: parent.depth + 1,
                        confidence: parent.confidence.min(edge.confidence),
                    },
                );
                next.push(neighbour);
            }
        }
        next.sort_unstable();
        next.dedup();
        frontier = next;
    }

    Ok(paths)
}

fn assign_link_graph_order(
    conn: &Connection,
    ranked: &mut [RankedSuggestionMember],
) -> Result<(), String> {
    let linked_ids = ranked
        .iter()
        .filter(|member| member.link_depth.is_some())
        .map(|member| member.work.id)
        .collect::<HashSet<_>>();
    if linked_ids.len() < 2 {
        return Ok(());
    }
    let ids = linked_ids.iter().copied().collect::<Vec<_>>();
    let edges = load_link_edges_touching(conn, &ids)?;
    let mut outgoing: HashMap<i64, HashSet<i64>> = HashMap::new();
    let mut indegree: HashMap<i64, usize> = HashMap::new();
    for edge in edges {
        if !linked_ids.contains(&edge.from_id) || !linked_ids.contains(&edge.to_id) {
            continue;
        }
        let ordered = match edge.relation_type.as_str() {
            "continues_to" => Some((edge.from_id, edge.to_id)),
            "continues_from" => Some((edge.to_id, edge.from_id)),
            _ => None,
        };
        let Some((before, after)) = ordered else {
            continue;
        };
        if before == after || !outgoing.entry(before).or_default().insert(after) {
            continue;
        }
        indegree.entry(before).or_insert(0);
        *indegree.entry(after).or_insert(0) += 1;
    }
    if indegree.is_empty() {
        return Ok(());
    }

    let fallback = ranked
        .iter()
        .map(|member| {
            (
                member.work.id,
                (member.work.published_at.clone(), member.work.id),
            )
        })
        .collect::<HashMap<_, _>>();
    let compare_ids = |left: &i64, right: &i64| {
        fallback
            .get(left)
            .cmp(&fallback.get(right))
            .then_with(|| left.cmp(right))
    };
    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    ready.sort_by(compare_ids);
    let mut ordered = Vec::with_capacity(indegree.len());
    while !ready.is_empty() {
        let current = ready.remove(0);
        ordered.push(current);
        if let Some(targets) = outgoing.get(&current) {
            for target in targets {
                let Some(degree) = indegree.get_mut(target) else {
                    continue;
                };
                *degree = degree.saturating_sub(1);
                if *degree == 0 {
                    ready.push(*target);
                }
            }
            ready.sort_by(compare_ids);
        }
    }
    // A malformed cycle must not make ordering nondeterministic. Put the
    // remaining linked works after the acyclic prefix in a stable fallback.
    let mut remaining = indegree
        .keys()
        .filter(|id| !ordered.contains(id))
        .copied()
        .collect::<Vec<_>>();
    remaining.sort_by(compare_ids);
    ordered.extend(remaining);
    let positions = ordered
        .into_iter()
        .enumerate()
        .map(|(position, id)| (id, position as i64))
        .collect::<HashMap<_, _>>();
    for member in ranked {
        member.link_order = positions.get(&member.work.id).copied();
    }
    Ok(())
}

fn shared_series_with_seeds(
    conn: &Connection,
    candidate_id: i64,
    seed_json: &str,
) -> Result<Vec<(String, Option<i64>)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT candidate.title, candidate.content_order
             FROM download_series candidate
             JOIN download_series seed
               ON seed.series_source = candidate.series_source
              AND seed.series_key = candidate.series_key
             WHERE candidate.download_id = ?1
               AND seed.download_id IN (SELECT value FROM json_each(?2))
             GROUP BY candidate.series_source, candidate.series_key
             ORDER BY candidate.content_order ASC, candidate.title COLLATE NOCASE ASC",
        )
        .map_err(|e| format!("Failed to prepare shared series: {e}"))?;
    let values = stmt
        .query_map(params![candidate_id, seed_json], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(|e| format!("Failed to query shared series: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read shared series: {e}"))?;
    Ok(values)
}

fn strongest_link_with_seeds(
    conn: &Connection,
    candidate_id: i64,
    seed_json: &str,
) -> Result<Option<(String, f64, Option<i64>)>, String> {
    conn.query_row(
        "SELECT relation_type, confidence, from_download_id, to_download_id
         FROM work_links
         WHERE status != 'rejected'
           AND (
             (from_download_id = ?1 AND to_download_id IN (SELECT value FROM json_each(?2)))
             OR (to_download_id = ?1 AND from_download_id IN (SELECT value FROM json_each(?2)))
           )
         ORDER BY confidence DESC, id DESC LIMIT 1",
        params![candidate_id, seed_json],
        |row| {
            let relation = row.get::<_, String>(0)?;
            let confidence = row.get::<_, f64>(1)?;
            let from_id = row.get::<_, Option<i64>>(2)?;
            let to_id = row.get::<_, Option<i64>>(3)?;
            let relative_order = match relation.as_str() {
                // A --continues_to--> B means A precedes B.
                "continues_to" if from_id == Some(candidate_id) => Some(-1),
                "continues_to" if to_id == Some(candidate_id) => Some(1),
                // A --continues_from--> B means B precedes A.
                "continues_from" if from_id == Some(candidate_id) => Some(1),
                "continues_from" if to_id == Some(candidate_id) => Some(-1),
                _ => None,
            };
            Ok((relation, confidence, relative_order))
        },
    )
    .optional()
    .map_err(|e| format!("Failed to query work-link evidence: {e}"))
}

fn suggestion_pair_is_rejected(
    conn: &Connection,
    candidate: &SuggestionWork,
    seeds: &[SuggestionWork],
) -> Result<bool, String> {
    let candidate_key = WorkKey {
        source: candidate.source.clone(),
        source_id: candidate.source_id.clone(),
    };
    for seed in seeds {
        let seed_key = WorkKey {
            source: seed.source.clone(),
            source_id: seed.source_id.clone(),
        };
        let (left, right) = canonical_work_key_pair(&candidate_key, &seed_key);
        let rejected = conn
            .query_row(
                "SELECT decision, rule_version FROM collection_pair_feedback
                 WHERE left_source = ?1 AND left_source_id = ?2
                   AND right_source = ?3 AND right_source_id = ?4",
                params![left.source, left.source_id, right.source, right.source_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to read collection feedback: {e}"))?
            .is_some_and(|(decision, rule_version)| {
                decision == "reject" && rule_version == COLLECTION_SUGGEST_RULE_VERSION
            });
        if rejected {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_suggestion_evidence(
    evidence: &mut Vec<CollectionSuggestionEvidence>,
    score: &mut f64,
    kind: &str,
    label: &str,
    contribution: f64,
) {
    *score += contribution;
    evidence.push(CollectionSuggestionEvidence {
        kind: kind.to_string(),
        label: label.to_string(),
        contribution,
    });
}

/// 話数を持たない特別な位置。本編の連番（`話数 * 10`）とぶつからない値を選ぶ。
const ORDER_PROLOGUE: i64 = -10;
const ORDER_FINALE: i64 = 1_000_000;
const ORDER_EPILOGUE: i64 = 1_000_010;
const ORDER_SIDE_STORY: i64 = 1_000_020;

/// 単独トークンの数字を話数とみなす上限。`作品 2026` のような西暦を
/// 話数として拾わないための歯止めで、助数詞を伴う明示的な形には適用しない。
const MAX_BARE_EPISODE_NUMBER: i64 = 300;

/// 漢数字を読む。`十二` `三十` `百二十三` と、`〇一二` のような桁並べに対応する。
fn kanji_number(token: &str) -> Option<i64> {
    let digit = |value: char| match value {
        '〇' | '零' => Some(0),
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    };
    let mut total = 0_i64;
    let mut current = 0_i64;
    let mut saw_any = false;
    for value in token.chars() {
        match value {
            '百' => {
                total += if current == 0 { 100 } else { current * 100 };
                current = 0;
                saw_any = true;
            }
            '十' => {
                total += if current == 0 { 10 } else { current * 10 };
                current = 0;
                saw_any = true;
            }
            other => {
                current = current * 10 + digit(other)?;
                saw_any = true;
            }
        }
    }
    saw_any.then(|| total + current)
}

fn roman_to_text(mut value: i64) -> String {
    const TABLE: [(i64, &str); 9] = [
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (amount, text) in TABLE {
        while value >= amount {
            out.push_str(text);
            value -= amount;
        }
    }
    out
}

/// ローマ数字を読む。`civil` のように文字だけ揃った単語を弾くため、読んだ数を
/// 書き戻して元の綴りと一致する場合しか採用しない。一文字は `i` や `c` が
/// 英単語と衝突するため対象外にする。
fn roman_number(token: &str) -> Option<i64> {
    if token.chars().count() < 2 || token.chars().count() > 15 {
        return None;
    }
    let values = token
        .chars()
        .map(|value| match value {
            'i' => Some(1_i64),
            'v' => Some(5),
            'x' => Some(10),
            'l' => Some(50),
            'c' => Some(100),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let mut total = 0_i64;
    for (index, current) in values.iter().enumerate() {
        if values[index + 1..].iter().any(|later| later > current) {
            total -= current;
        } else {
            total += current;
        }
    }
    (total > 0 && roman_to_text(total) == token).then_some(total)
}

fn parse_episode_number(token: &str) -> Option<i64> {
    token
        .parse::<i64>()
        .ok()
        .or_else(|| kanji_number(token))
        .filter(|value| *value >= 0)
}

/// 題名から比較用の共通幹と、読む順を表す数値を取り出す。
///
/// 入力は `normalize_search_text` を通してから扱う。その正規化は NFKC・小文字化・
/// カタカナ→ひらがな変換に加えて、**記号をすべて空白へ潰す**。`#12` はここへ届く
/// 時点で `12`、`【FANBOX】月下の約束（後編）` は `fanbox 月下の約束 後編` に
/// なっている。したがって記号を手掛かりにはできず、数字・漢数字・ローマ数字と
/// 語そのものだけで判定する。
///
/// 返す順序値は「話数 * 10 + 前中後編の位置」を基本とし、話数を持たない
/// プロローグ・最終話・エピローグ・番外編には前後へ十分離れた固定値を与える。
fn title_stem_and_order(title: &str) -> (String, Option<i64>) {
    static COUNTER_RE: OnceLock<Regex> = OnceLock::new();
    static DAI_RE: OnceLock<Regex> = OnceLock::new();
    static SONO_RE: OnceLock<Regex> = OnceLock::new();
    static MARKER_RE: OnceLock<Regex> = OnceLock::new();
    // 助数詞を伴う形。`第12話` `12話` `第三夜` `2部` のいずれも拾う。
    let counter_regex = COUNTER_RE.get_or_init(|| {
        Regex::new(
            r"(?:第\s*)?([0-9]+|[〇零一二三四五六七八九十百]+)\s*(?:話|章|回|編|夜|部|節|幕|巻)",
        )
        .expect("episode counter regex")
    });
    // 助数詞のない `第3` 形。
    let dai_regex = DAI_RE.get_or_init(|| {
        Regex::new(r"第\s*([0-9]+|[〇零一二三四五六七八九十百]+)").expect("episode dai regex")
    });
    // `その12` 形。正規化でカタカナはひらがなへ倒れている。
    let sono_regex = SONO_RE.get_or_init(|| {
        Regex::new(r"その\s*([0-9]+|[〇零一二三四五六七八九十百]+)").expect("episode sono regex")
    });
    // `final` は単語境界を要求する。`finalize` を完結編と誤らせない。
    let marker_regex = MARKER_RE.get_or_init(|| {
        Regex::new(
            r"前編|前篇|中編|中篇|後編|後篇|上巻|中巻|下巻|完結編|最終話|最終回|番外編|番外|外伝|ぷろろーぐ|えぴろーぐ|\bfinal\b",
        )
        .expect("episode marker regex")
    });

    let normalized = normalize_search_text(title);

    let mut number = None;
    for regex in [counter_regex, dai_regex, sono_regex] {
        if number.is_some() {
            break;
        }
        number = regex
            .captures(&normalized)
            .and_then(|capture| capture.get(1))
            .and_then(|value| parse_episode_number(value.as_str()));
    }

    let mut working = counter_regex.replace_all(&normalized, " ").into_owned();
    working = dai_regex.replace_all(&working, " ").into_owned();
    working = sono_regex.replace_all(&working, " ").into_owned();

    // 助数詞のない単独トークン。`航路 01` や `星の記憶 xii` を拾う。
    let mut bare_number_token = None;
    if number.is_none() {
        for token in working.split_whitespace() {
            let parsed = token
                .parse::<i64>()
                .ok()
                .filter(|value| (0..=MAX_BARE_EPISODE_NUMBER).contains(value))
                .or_else(|| kanji_number(token))
                .or_else(|| roman_number(token));
            if let Some(value) = parsed {
                number = Some(value);
                bare_number_token = Some(token.to_string());
                break;
            }
        }
    }

    let marker_offset =
        if working.contains("前編") || working.contains("前篇") || working.contains("上巻") {
            Some(1)
        } else if working.contains("中編") || working.contains("中篇") || working.contains("中巻")
        {
            Some(2)
        } else if working.contains("後編") || working.contains("後篇") || working.contains("下巻")
        {
            Some(3)
        } else {
            // 素の `上` `中` `下` は単独トークンのときだけ位置とみなす。
            // `月下の約束` の `下` を後編と読まないための条件である。
            working.split_whitespace().find_map(|token| match token {
                "上" => Some(1),
                "中" => Some(2),
                "下" => Some(3),
                _ => None,
            })
        };

    let special_order = if working.contains("ぷろろーぐ") {
        Some(ORDER_PROLOGUE)
    } else if working.contains("番外編") || working.contains("番外") || working.contains("外伝")
    {
        Some(ORDER_SIDE_STORY)
    } else if working.contains("えぴろーぐ") {
        Some(ORDER_EPILOGUE)
    } else if working.contains("完結編")
        || working.contains("最終話")
        || working.contains("最終回")
        || marker_regex
            .find(&working)
            .is_some_and(|found| found.as_str() == "final")
    {
        Some(ORDER_FINALE)
    } else {
        None
    };

    // 話数が読めていればそれを主軸にし、前中後編を枝番として足す。話数がない
    // ときだけ、プロローグ・最終話などの固定位置へ落とす。
    let order = match number {
        Some(value) => Some(value.saturating_mul(10) + marker_offset.unwrap_or(0)),
        None => special_order.or(marker_offset),
    };

    let without_markers = marker_regex.replace_all(&working, " ");
    let stem = without_markers
        .split_whitespace()
        .filter(|token| {
            !matches!(*token, "上" | "中" | "下") && bare_number_token.as_deref() != Some(*token)
        })
        .collect::<Vec<_>>()
        .join(" ");
    (stem, order)
}

/// 連載の初回は番号を振らないことが多い。番号付きの兄弟が同じ語幹で存在し、
/// 自身にだけ番号が無い作品は「第 0 話」として先頭へ置く。順序未定を表す
/// None は末尾へ流れるため、この補正が無いと初回が最後に並んでしまう。
/// 本文が二重に入る候補へ印を付け、既定の選択から外す。
///
/// 実ライブラリには 2 種類の重複がある。pixiv と FANBOX の両方へ投稿された
/// 同じ作品と、分冊をまとめ直した合本である。どちらもそのまま EPUB にすると
/// 同じ本文が 2 回入る。候補からは消さず、理由を見せたうえで利用者が明示的に
/// 選んだときだけ含める。どちらを採るかは利用者の判断に属するためである。
fn flag_duplicate_content_members(ranked: &mut [RankedSuggestionMember]) {
    const OMNIBUS_WORDS: [&str; 3] = ["まとめ", "合本", "総集編"];
    const POSITION_WORDS: [&str; 7] = ["前編", "中編", "後編", "完結編", "最終話", "上巻", "下巻"];

    // 取得元をまたぐ同一作品。題名と作者が一致し取得元だけが違うものを束ね、
    // 本文が長い方（取りこぼしの少ない方）を既定に残す。
    let mut by_title: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (index, member) in ranked.iter().enumerate() {
        let key = (
            normalize_search_text(&member.work.title),
            normalize_search_text(&member.work.author_name),
        );
        if key.0.is_empty() {
            continue;
        }
        by_title.entry(key).or_default().push(index);
    }
    let mut duplicates = Vec::new();
    for indexes in by_title.values() {
        if indexes.len() < 2 {
            continue;
        }
        let sources = indexes
            .iter()
            .map(|index| ranked[*index].work.source.clone())
            .collect::<HashSet<_>>();
        if sources.len() < 2 {
            continue;
        }
        // 基準作品は利用者自身が選んだもの。重複していても既定から外さず、
        // もう一方に印を付ける。選んだはずの作品が外れていると混乱する。
        let seeded = indexes
            .iter()
            .find(|index| ranked[**index].evidence.iter().any(|e| e.kind == "seed"));
        let keep = match seeded {
            Some(index) => *index,
            None => *indexes
                .iter()
                .max_by_key(|index| (ranked[**index].work.text_length, -ranked[**index].work.id))
                .expect("non-empty duplicate group"),
        };
        for index in indexes {
            if *index != keep {
                duplicates.push((*index, ranked[keep].work.source.clone()));
            }
        }
    }
    for (index, kept_source) in duplicates {
        ranked[index].default_selected = false;
        ranked[index].evidence.push(CollectionSuggestionEvidence {
            kind: "duplicate_work".to_string(),
            label: format!("{kept_source} にも同じ題名の作品があります"),
            contribution: 0.0,
        });
    }

    // 合本。前編と中編のように位置を表す語を 2 つ以上含むか、まとめ・合本・
    // 総集編と名乗るもの。単に「④+エピローグ」のような 1 か所だけの表記は
    // 分冊そのものなので対象にしない。
    let mut omnibus = Vec::new();
    for (index, member) in ranked.iter().enumerate() {
        let title = normalize_search_text(&member.work.title);
        let positions = POSITION_WORDS
            .iter()
            .filter(|word| title.contains(**word))
            .count();
        let named = OMNIBUS_WORDS.iter().any(|word| title.contains(*word));
        if positions < 2 && !named {
            continue;
        }
        let siblings = ranked
            .iter()
            .filter(|other| {
                other.work.id != member.work.id
                    && stems_are_siblings(&other.title_stem, &member.title_stem)
            })
            .count();
        if siblings >= 2 {
            omnibus.push(index);
        }
    }
    // 抜粋・体験版は本文が途中までしかない。合本とは逆に「足りない」重複で、
    // 一冊にまとめると同じ話の断片が混ざる。
    const SAMPLE_WORDS: [&str; 4] = ["さんぷる", "体験版", "お試し", "抜粋"];
    let samples = ranked
        .iter()
        .enumerate()
        .filter(|(_, member)| {
            let title = normalize_search_text(&member.work.title);
            SAMPLE_WORDS.iter().any(|word| title.contains(*word))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in samples {
        ranked[index].default_selected = false;
        ranked[index].evidence.push(CollectionSuggestionEvidence {
            kind: "partial_sample".to_string(),
            label: "本文の一部だけの版に見えます".to_string(),
            contribution: 0.0,
        });
    }

    for index in omnibus {
        ranked[index].default_selected = false;
        ranked[index].evidence.push(CollectionSuggestionEvidence {
            kind: "omnibus".to_string(),
            label: "分冊をまとめ直した版の可能性があります".to_string(),
            contribution: 0.0,
        });
    }
}

fn assign_unnumbered_opening_order(ranked: &mut [RankedSuggestionMember]) {
    let numbered_stems = ranked
        .iter()
        .filter(|value| value.episode_order.is_some() && !value.title_stem.is_empty())
        .map(|value| value.title_stem.clone())
        .collect::<Vec<_>>();
    let openings = ranked
        .iter()
        .enumerate()
        .filter(|(_, value)| value.episode_order.is_none() && !value.title_stem.is_empty())
        .filter(|(_, value)| {
            numbered_stems
                .iter()
                .any(|stem| stems_are_siblings(stem, &value.title_stem))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in openings {
        ranked[index].episode_order = Some(0);
    }
}

/// 同じ連載の一部と見なせる語幹かどうか。完全一致だけを見ると、副題や
/// 「【本文全体17.5万文字】」のような注記が付いた版を取りこぼす。
fn stems_are_siblings(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left == right
        || left.contains(right)
        || right.contains(left)
        || normalized_levenshtein(left, right) >= 0.8
}

fn compare_ranked_suggestion_members(
    left: &RankedSuggestionMember,
    right: &RankedSuggestionMember,
) -> Ordering {
    // 読む順は明示的な手掛かりだけで決める。スコアは「順序の手掛かりが何も
    // 無い作品同士」の並びにしか使わない。単なる言及リンクで加点された作品が、
    // 題名に書かれた話数を追い越すのを防ぐためである。
    // link_depth は基準作品からのリンク到達段数、つまり「近さ」であって
    // 読む順ではない。話数や公式順より先に効かせると、単に基準作品から
    // 参照されているだけの作品が第1話を追い越す。同点処理にだけ使う。
    compare_optional_order(left.link_order, right.link_order)
        .then_with(|| compare_optional_order(left.series_order, right.series_order))
        .then_with(|| compare_optional_order(left.episode_order, right.episode_order))
        .then_with(|| compare_optional_order(left.link_depth, right.link_depth))
        .then_with(|| right.member_score.total_cmp(&left.member_score))
        .then_with(|| left.work.published_at.cmp(&right.work.published_at))
        .then_with(|| left.work.id.cmp(&right.work.id))
}

fn compare_optional_order<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn proposed_collection_name(
    conn: &Connection,
    ranked: &[RankedSuggestionMember],
    seed_json: &str,
) -> Result<String, String> {
    for member in ranked {
        if let Some((title, _)) = shared_series_with_seeds(conn, member.work.id, seed_json)?.first()
        {
            if !title.trim().is_empty() {
                return Ok(title.trim().to_string());
            }
        }
    }
    let (stem, _) = title_stem_and_order(&ranked[0].work.title);
    if !stem.is_empty() {
        return Ok(format!("{stem} 関連作品"));
    }
    let author = ranked[0].work.author_name.trim();
    if !author.is_empty() {
        Ok(format!("{author}の作品コレクション"))
    } else {
        Ok("読書コレクション".to_string())
    }
}

fn collection_suggestion_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CollectionSuggestion> {
    let members_json: String = row.get(3)?;
    let members = serde_json::from_str(&members_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            members_json.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(CollectionSuggestion {
        id: row.get(0)?,
        proposed_name: row.get(1)?,
        collection_kind: row.get(2)?,
        members,
        score: row.get(4)?,
        rule_version: row.get(5)?,
        state: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn canonical_work_key_pair(left: &WorkKey, right: &WorkKey) -> (WorkKey, WorkKey) {
    if (&left.source, &left.source_id) <= (&right.source, &right.source_id) {
        (left.clone(), right.clone())
    } else {
        (right.clone(), left.clone())
    }
}

fn save_pair_feedback(
    tx: &Transaction<'_>,
    left: &WorkKey,
    right: &WorkKey,
    decision: &str,
    rule_version: &str,
    now: &str,
) -> Result<(), String> {
    let (left, right) = canonical_work_key_pair(left, right);
    tx.execute(
        "INSERT INTO collection_pair_feedback (
            left_source, left_source_id, right_source, right_source_id,
            decision, rule_version, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(left_source, left_source_id, right_source, right_source_id)
         DO UPDATE SET decision = excluded.decision,
                       rule_version = excluded.rule_version,
                       updated_at = excluded.updated_at",
        params![
            left.source,
            left.source_id,
            right.source,
            right.source_id,
            decision,
            rule_version,
            now,
        ],
    )
    .map_err(|e| format!("Failed to save collection feedback: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct ExtractedWorkLink {
    to_source: String,
    to_source_id: String,
    relation_type: String,
    anchor_text: Option<String>,
    context_text: Option<String>,
    confidence: f64,
}

fn extract_work_link_evidence(
    text: &str,
    from_source: &str,
    from_source_id: &str,
) -> Vec<ExtractedWorkLink> {
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    let regex = URL_RE.get_or_init(|| {
        Regex::new(r#"(?i)(?:https?://[^\s\"'<>\\\[\]{}]+|pixiv://novels/\d+)"#)
            .expect("work link URL regex")
    });
    let mut links: HashMap<(String, String), ExtractedWorkLink> = HashMap::new();
    for candidate in regex.find_iter(text).take(2_000) {
        let raw = candidate
            .as_str()
            .replace("&amp;", "&")
            .replace("\\u0026", "&");
        let raw = raw.trim_end_matches([
            '.', ',', ':', ';', '!', '?', ')', ']', '}', '。', '、', '！', '？', '）', '】', '」',
            '』',
        ]);
        let Some((to_source, to_source_id)) = normalize_linked_work_url(raw) else {
            continue;
        };
        if to_source == from_source && to_source_id == from_source_id {
            continue;
        }
        let context = readable_link_context(text, candidate.start(), candidate.end(), 100);
        let normalized_context = normalize_search_text(&context);
        let (relation_type, confidence) = if ["続き", "次話", "次編", "後編", "next"]
            .iter()
            .any(|marker| normalized_context.contains(marker))
        {
            ("continues_to", 0.94)
        } else if ["前話", "前編", "前作", "previous", "prev"]
            .iter()
            .any(|marker| normalized_context.contains(marker))
        {
            ("continues_from", 0.94)
        } else if ["補足", "番外", "おまけ", "関連"]
            .iter()
            .any(|marker| normalized_context.contains(marker))
        {
            ("supplement", 0.86)
        } else {
            ("mentions", 0.72)
        };
        let anchor = (!context.is_empty()).then(|| truncate_chars(&context, 120));
        let value = ExtractedWorkLink {
            to_source: to_source.clone(),
            to_source_id: to_source_id.clone(),
            relation_type: relation_type.to_string(),
            anchor_text: anchor,
            context_text: (!context.is_empty()).then(|| truncate_chars(&context, 240)),
            confidence,
        };
        links
            .entry((to_source, to_source_id))
            .and_modify(|current| {
                if value.confidence > current.confidence {
                    *current = value.clone();
                }
            })
            .or_insert(value);
    }
    let mut links = links.into_values().collect::<Vec<_>>();
    links.sort_by(|left, right| {
        right
            .confidence
            .partial_cmp(&left.confidence)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.to_source.cmp(&right.to_source))
            .then_with(|| left.to_source_id.cmp(&right.to_source_id))
    });
    links
}

fn normalize_linked_work_url(raw: &str) -> Option<(String, String)> {
    if let Some(source_id) = raw
        .strip_prefix("pixiv://novels/")
        .or_else(|| raw.strip_prefix("PIXIV://NOVELS/"))
    {
        let source_id = source_id
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        return (!source_id.is_empty()).then(|| ("pixiv".to_string(), source_id));
    }
    let url = url::Url::parse(raw).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let segments = url
        .path_segments()
        .map(|value| value.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if host == "pixiv.net" || host == "www.pixiv.net" {
        if url.path() == "/novel/show.php" {
            let id = url
                .query_pairs()
                .find(|(key, _)| key == "id")
                .map(|(_, value)| value.into_owned())?;
            return id
                .chars()
                .all(|value| value.is_ascii_digit())
                .then(|| ("pixiv".to_string(), id));
        }
        if segments.first().copied() == Some("novels") {
            let id = segments.get(1)?.to_string();
            return id
                .chars()
                .all(|value| value.is_ascii_digit())
                .then(|| ("pixiv".to_string(), id));
        }
    }
    if host == "fanbox.cc" || host.ends_with(".fanbox.cc") {
        let index = segments.iter().position(|segment| *segment == "posts")?;
        let id = segments.get(index + 1)?.to_string();
        return id
            .chars()
            .all(|value| value.is_ascii_digit())
            .then(|| ("fanbox".to_string(), id));
    }
    None
}

fn readable_link_context(text: &str, start: usize, end: usize, radius: usize) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    let start_char = text[..start].chars().count();
    let match_chars = text[start..end].chars().count();
    let chars = text.chars().collect::<Vec<_>>();
    let from = start_char.saturating_sub(radius);
    let to = (start_char + match_chars + radius).min(chars.len());
    let context = chars[from..to].iter().collect::<String>();
    let without_tags = TAG_RE
        .get_or_init(|| Regex::new(r"(?is)<[^>]*>").expect("HTML tag regex"))
        .replace_all(&context, " ");
    without_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn validate_collection_id(value: &str) -> Result<&str, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 160 {
        return Err("Invalid collection ID".to_string());
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Invalid collection ID".to_string());
    }
    Ok(value)
}

fn validate_collection_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Collection name is required".to_string());
    }
    if value.chars().count() > 200 {
        return Err("Collection name is too long".to_string());
    }
    Ok(value.to_string())
}

fn validate_collection_kind(value: &str) -> Result<String, String> {
    match value.trim() {
        "ordered" => Ok("ordered".to_string()),
        "unordered" => Ok("unordered".to_string()),
        _ => Err("Collection kind must be ordered or unordered".to_string()),
    }
}

fn validate_work_key_part(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 500 {
        return Err(format!("Invalid {label}"));
    }
    Ok(value.to_string())
}

fn normalize_bounded_optional(
    value: &Option<String>,
    max_chars: usize,
    label: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value.as_deref() else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max_chars {
        return Err(format!("{label} is too long"));
    }
    Ok(Some(value.to_string()))
}

fn normalize_member_role(value: Option<&str>) -> Result<String, String> {
    match value.unwrap_or("main").trim() {
        "main" => Ok("main".to_string()),
        "supplement" => Ok("supplement".to_string()),
        "appendix" => Ok("appendix".to_string()),
        _ => Err("Invalid collection member role".to_string()),
    }
}

fn normalize_added_by(value: Option<&str>) -> Result<String, String> {
    match value.unwrap_or("manual").trim() {
        "manual" => Ok("manual".to_string()),
        "suggestion" => Ok("suggestion".to_string()),
        "import" => Ok("import".to_string()),
        _ => Err("Invalid collection member origin".to_string()),
    }
}

fn new_collection_id(prefix: &str) -> String {
    let timestamp = chrono::Utc::now().timestamp_micros();
    let serial = READER_CACHE_TICK.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{prefix}-{timestamp:x}-{serial:x}")
}

fn normalize_collection_positions(tx: &Transaction<'_>, collection_id: &str) -> Result<(), String> {
    let keys = {
        let mut stmt = tx
            .prepare(
                "SELECT source, source_id FROM work_collection_members
                 WHERE collection_id = ?1
                 ORDER BY position ASC, created_at ASC, source ASC, source_id ASC",
            )
            .map_err(|e| format!("Failed to prepare collection normalization: {e}"))?;
        let keys = stmt
            .query_map(params![collection_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query collection normalization: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read collection normalization: {e}"))?;
        keys
    };
    let now = chrono::Utc::now().to_rfc3339();
    for (position, (source, source_id)) in keys.iter().enumerate() {
        tx.execute(
            "UPDATE work_collection_members SET position = ?1, updated_at = ?2
             WHERE collection_id = ?3 AND source = ?4 AND source_id = ?5",
            params![position as i64, now, collection_id, source, source_id],
        )
        .map_err(|e| format!("Failed to normalize collection order: {e}"))?;
    }
    Ok(())
}

fn is_version_stage_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(".v") else {
        return false;
    };
    let Some((version, suffix)) = rest.split_once('.') else {
        return false;
    };
    let suffix = suffix.as_bytes();
    !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit())
        && suffix.len() == 22
        && suffix[..16].iter().all(u8::is_ascii_hexdigit)
        && &suffix[16..] == b".stage"
}

fn ensure_download_save_journal_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS download_save_journal (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            source_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            stage_path TEXT NOT NULL,
            final_path TEXT NOT NULL,
            committed INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
         )",
    )
    .map_err(|error| format!("Download save journal schema failed: {error}"))?;
    Ok(())
}

fn remove_download_save_path(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Download recovery metadata failed: {error}")),
    };
    if metadata.file_type().is_symlink() {
        std::fs::remove_dir(path)
            .or_else(|_| std::fs::remove_file(path))
            .map_err(|error| format!("Download recovery link removal failed: {error}"))
    } else if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)
            .map_err(|error| format!("Interrupted download cleanup failed: {error}"))
    } else {
        Err("Download recovery path is not a directory".to_string())
    }
}

fn validate_download_save_journal_path(
    storage: &Path,
    source: &str,
    source_id: &str,
    version: i64,
    stage_path: &Path,
    final_path: &Path,
) -> Result<(), String> {
    if !matches!(source, "pixiv" | "fanbox")
        || source_id.is_empty()
        || source_id.len() > 128
        || !source_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        || version < 1
    {
        return Err("Download save journal contains invalid work identity".to_string());
    }
    let expected_work = storage.join(source).join(source_id);
    let expected_final = expected_work.join(format!("v{version}"));
    let stage_name = stage_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Download save journal has an invalid stage name".to_string())?;
    if final_path != expected_final
        || stage_path.parent() != Some(expected_work.as_path())
        || !is_version_stage_name(stage_name)
        || !stage_name.starts_with(&format!(".v{version}."))
    {
        return Err("Download save journal path escaped its work directory".to_string());
    }
    for parent in [storage.join(source), expected_work] {
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
                return Err("Download save journal parent is not a real directory".to_string())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Download save journal parent inspection failed: {error}"
                ))
            }
        }
    }
    Ok(())
}

/// Recovers the filesystem gap that cannot be handled by a Rust drop guard:
/// process termination between publication and the SQLite commit. Only paths
/// recorded by this save pipeline are ever treated as disposable.
fn recover_interrupted_download_saves(conn: &Connection, storage_dir: &Path) -> Result<(), String> {
    ensure_download_save_journal_schema(conn)?;

    let storage = storage_dir
        .canonicalize()
        .map_err(|error| format!("Download recovery storage resolution failed: {error}"))?;

    let mut statement = conn
        .prepare(
            "SELECT id, source, source_id, version, stage_path, final_path, committed
             FROM download_save_journal ORDER BY created_at",
        )
        .map_err(|error| format!("Download save journal read failed: {error}"))?;
    let journals = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                PathBuf::from(row.get::<_, String>(4)?),
                PathBuf::from(row.get::<_, String>(5)?),
                row.get::<_, i64>(6)? != 0,
            ))
        })
        .map_err(|error| format!("Download save journal query failed: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Download save journal decode failed: {error}"))?;
    drop(statement);

    for (id, source, source_id, version, stage_path, final_path, committed) in journals {
        validate_download_save_journal_path(
            &storage,
            &source,
            &source_id,
            version,
            &stage_path,
            &final_path,
        )?;
        remove_download_save_path(&stage_path)?;
        if !committed {
            remove_download_save_path(&final_path)?;
        }
        conn.execute(
            "DELETE FROM download_save_journal WHERE id = ?1",
            params![id],
        )
        .map_err(|error| format!("Download save journal cleanup failed: {error}"))?;
    }

    // A crash may occur after creating a unique stage directory but before its
    // journal row is durable. The exact reserved name makes these safe to reap;
    // ordinary vN directories are deliberately never inferred to be garbage.
    for source_entry in std::fs::read_dir(&storage)
        .map_err(|error| format!("Download recovery source scan failed: {error}"))?
    {
        let source_entry = source_entry.map_err(|error| error.to_string())?;
        let source_metadata =
            std::fs::symlink_metadata(source_entry.path()).map_err(|error| error.to_string())?;
        if source_metadata.file_type().is_symlink() || !source_metadata.file_type().is_dir() {
            continue;
        }
        let source = source_entry.file_name().to_string_lossy().to_string();
        if !matches!(source.as_str(), "pixiv" | "fanbox") {
            continue;
        }
        for work_entry in
            std::fs::read_dir(source_entry.path()).map_err(|error| error.to_string())?
        {
            let work_entry = work_entry.map_err(|error| error.to_string())?;
            let work_metadata =
                std::fs::symlink_metadata(work_entry.path()).map_err(|error| error.to_string())?;
            if work_metadata.file_type().is_symlink() || !work_metadata.file_type().is_dir() {
                continue;
            }
            for item_entry in
                std::fs::read_dir(work_entry.path()).map_err(|error| error.to_string())?
            {
                let item_entry = item_entry.map_err(|error| error.to_string())?;
                let metadata = std::fs::symlink_metadata(item_entry.path())
                    .map_err(|error| error.to_string())?;
                if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                    continue;
                }
                let name = item_entry.file_name().to_string_lossy().to_string();
                if is_version_stage_name(&name) {
                    remove_download_save_path(&item_entry.path())?;
                }
            }
        }
    }
    Ok(())
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

fn entity_series_cursor_scope(source: &str, person_key: &str, query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hasher.update([0x1f]);
    hasher.update(person_key.as_bytes());
    hasher.update([0x1f]);
    hasher.update(query.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn encode_entity_series_cursor(cursor: &EntitySeriesCursor) -> Result<String, String> {
    serde_json::to_vec(cursor)
        .map(|bytes| format!("es:{}", URL_SAFE_NO_PAD.encode(bytes)))
        .map_err(|e| format!("Entity series cursor encode failed: {e}"))
}

fn decode_entity_series_cursor(raw: Option<&str>) -> Result<Option<EntitySeriesCursor>, String> {
    let Some(raw) = raw else { return Ok(None) };
    let encoded = raw
        .strip_prefix("es:")
        .ok_or_else(|| "Invalid entity series cursor".to_string())?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|_| "Invalid entity series cursor".to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Invalid entity series cursor".to_string())
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
        total_estimate: None,
        tantivy_total_hits: None,
        snapshot_id: None,
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
        total_estimate: None,
        tantivy_total_hits: None,
        snapshot_id: None,
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

fn facet_counts_bytes(facets: &[FacetCount]) -> usize {
    facets
        .iter()
        .map(|facet| {
            facet
                .name
                .len()
                .saturating_add(std::mem::size_of::<FacetCount>())
        })
        .sum()
}

fn entity_facets_bytes(facets: &[EntityFacet]) -> usize {
    facets
        .iter()
        .map(|facet| {
            std::mem::size_of::<EntityFacet>()
                .saturating_add(facet.source.len())
                .saturating_add(facet.source_key.len())
                .saturating_add(facet.display_name.len())
                .saturating_add(facet.cover_path.as_deref().map(str::len).unwrap_or(0))
                .saturating_add(facet.description.as_deref().map(str::len).unwrap_or(0))
                .saturating_add(facet.sample_title.as_deref().map(str::len).unwrap_or(0))
        })
        .sum()
}

fn filter_facets_bytes(facets: &FilterFacets) -> usize {
    facet_counts_bytes(&facets.tags)
        .saturating_add(facet_counts_bytes(&facets.authors))
        .saturating_add(entity_facets_bytes(&facets.author_entities))
        .saturating_add(entity_facets_bytes(&facets.series))
        .saturating_add(facet_counts_bytes(&facets.content_types))
        .saturating_add(facet_counts_bytes(&facets.asset_types))
}

fn suggestions_bytes(result: &SearchSuggestResult) -> usize {
    result
        .items
        .iter()
        .map(|suggestion| {
            std::mem::size_of::<SearchSuggestion>()
                .saturating_add(suggestion.kind.len())
                .saturating_add(suggestion.label.len())
                .saturating_add(suggestion.value.len())
                .saturating_add(suggestion.source.as_deref().map(str::len).unwrap_or(0))
                .saturating_add(suggestion.source_key.as_deref().map(str::len).unwrap_or(0))
        })
        .sum()
}

const SORTED_SEARCH_ID_BATCH_SIZE: usize = 512;

fn search_snapshot_dir(storage_dir: &Path) -> PathBuf {
    storage_dir.join(SEARCH_SNAPSHOT_DIR)
}

fn cleanup_search_snapshot_path(path: &Path) {
    let _ = std::fs::remove_file(path);
    let path_text = path.to_string_lossy();
    let _ = std::fs::remove_file(format!("{path_text}-wal"));
    let _ = std::fs::remove_file(format!("{path_text}-shm"));
}

fn cleanup_stale_search_snapshots(storage_dir: &Path) -> Result<(), String> {
    let directory = search_snapshot_dir(storage_dir);
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("Search snapshot directory creation failed: {e}"))?;
    let entries = std::fs::read_dir(&directory)
        .map_err(|e| format!("Search snapshot directory read failed: {e}"))?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_snapshot = name.starts_with("snapshot-")
            && (name.ends_with(".sqlite")
                || name.ends_with(".sqlite-wal")
                || name.ends_with(".sqlite-shm"));
        if is_snapshot && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn create_search_snapshot_connection(
    storage_dir: &Path,
    db_path: &Path,
) -> Result<(String, PathBuf, SnapshotConnection), String> {
    let directory = search_snapshot_dir(storage_dir);
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("Search snapshot directory creation failed: {e}"))?;
    for _ in 0..8 {
        let id = format!("{:032x}", rand::random::<u128>());
        let path = directory.join(format!("snapshot-{id}.sqlite"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => drop(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Search snapshot create failed: {error}")),
        }
        let result = (|| {
            let connection = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| format!("Search snapshot open failed: {e}"))?;
            connection
                .busy_timeout(Duration::from_secs(5))
                .map_err(|e| format!("Search snapshot busy timeout setup failed: {e}"))?;
            let cache_kib = (super::resource_budget::search_snapshot_cache_bytes() / 1024) as i64;
            connection
                .execute_batch(
                    "PRAGMA journal_mode = OFF;
                     PRAGMA synchronous = OFF;
                     PRAGMA temp_store = FILE;",
                )
                .map_err(|e| format!("Search snapshot setup failed: {e}"))?;
            connection
                .pragma_update(None, "cache_size", -cache_kib)
                .map_err(|e| format!("Search snapshot cache setup failed: {e}"))?;
            connection
                .execute(
                    "ATTACH DATABASE ?1 AS library",
                    params![db_path.to_string_lossy().to_string()],
                )
                .map_err(|e| format!("Library attach to search snapshot failed: {e}"))?;
            Ok(Arc::new(Mutex::new(Some(connection))))
        })();
        match result {
            Ok(connection) => return Ok((id, path, connection)),
            Err(error) => {
                cleanup_search_snapshot_path(&path);
                return Err(error);
            }
        }
    }
    Err("Could not allocate a unique search snapshot file".to_string())
}

fn search_snapshot_allocated_bytes(conn: &Connection) -> Result<u64, String> {
    let page_count: i64 = conn
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .map_err(|e| format!("Search snapshot page count failed: {e}"))?;
    let page_size: i64 = conn
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(|e| format!("Search snapshot page size failed: {e}"))?;
    Ok((page_count.max(0) as u64).saturating_mul(page_size.max(0) as u64))
}

fn ensure_search_snapshot_budget(conn: &Connection, storage_dir: &Path) -> Result<u64, String> {
    ensure_search_snapshot_budget_with_limit(
        conn,
        storage_dir,
        super::resource_budget::search_snapshot_disk_bytes(),
    )
}

fn ensure_search_snapshot_budget_with_limit(
    conn: &Connection,
    storage_dir: &Path,
    budget: u64,
) -> Result<u64, String> {
    let bytes = search_snapshot_allocated_bytes(conn)?;
    let directory_bytes = std::fs::read_dir(search_snapshot_dir(storage_dir))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.metadata().ok())
                .filter(|metadata| metadata.is_file())
                .map(|metadata| metadata.len())
                .sum::<u64>()
        })
        .unwrap_or(bytes);
    if bytes > budget || directory_bytes > budget {
        return Err(format!(
            "Search snapshots exceeded the shared {budget}-byte disk budget"
        ));
    }
    Ok(bytes)
}

/// Pins the attached library to the same WAL snapshot for every page. The
/// local match table alone is not enough: a concurrent delete from `downloads`
/// would otherwise shorten a page and make its exact total inconsistent.
fn pin_search_snapshot_library(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| format!("Library snapshot begin failed: {e}"))?;
    let _: Option<i64> = conn
        .query_row(
            "SELECT id FROM library.downloads ORDER BY id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Library snapshot pin failed: {e}"))?;
    Ok(())
}

fn close_snapshot_connection(connection: &SnapshotConnection) {
    if let Ok(mut guard) = connection.lock() {
        guard.take();
    }
}

fn reset_search_match_table(conn: &Connection, with_score: bool) -> Result<(), String> {
    let score_column = if with_score {
        ", score REAL NOT NULL"
    } else {
        ""
    };
    conn.execute_batch(&format!(
        "DROP TABLE IF EXISTS search_matches;
         CREATE TABLE search_matches (
            id INTEGER PRIMARY KEY{score_column}
         ) WITHOUT ROWID;"
    ))
    .map_err(|e| format!("Search snapshot table reset failed: {e}"))
}

fn insert_search_match_ids(conn: &Connection, ids: &[i64]) -> Result<(), String> {
    for batch in ids.chunks(SORTED_SEARCH_ID_BATCH_SIZE) {
        if batch.is_empty() {
            continue;
        }
        // 512 values stays below SQLite's historical 999-variable floor. One
        // statement per batch is dramatically cheaper than one VM execution
        // per id and still uses constant memory.
        let placeholders = (1..=batch.len())
            .map(|index| format!("(?{index})"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("INSERT OR IGNORE INTO search_matches (id) VALUES {placeholders}");
        conn.execute(&sql, rusqlite::params_from_iter(batch.iter().copied()))
            .map_err(|e| format!("Sorted search id batch insert failed: {e}"))?;
    }
    Ok(())
}

fn insert_search_match_scores(conn: &Connection, scores: &[(i64, f32)]) -> Result<(), String> {
    for batch in scores.chunks(SORTED_SEARCH_ID_BATCH_SIZE / 2) {
        if batch.is_empty() {
            continue;
        }
        let placeholders = batch
            .iter()
            .enumerate()
            .map(|(index, _)| format!("(?{}, ?{})", index * 2 + 1, index * 2 + 2))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT INTO search_matches (id, score) VALUES {placeholders}
             ON CONFLICT(id) DO UPDATE SET score = MAX(score, excluded.score)"
        );
        let values = batch.iter().flat_map(|(id, score)| {
            [
                rusqlite::types::Value::Integer(*id),
                rusqlite::types::Value::Real(*score as f64),
            ]
        });
        conn.execute(&sql, rusqlite::params_from_iter(values))
            .map_err(|e| format!("Ranked search score batch insert failed: {e}"))?;
    }
    Ok(())
}

fn insert_bulk_match_entries(conn: &Connection, entries: &[DownloadEntry]) -> Result<(), String> {
    // Three values per row keeps each statement below SQLite's historical
    // 999-variable floor while retaining a fixed memory ceiling.
    for batch in entries.chunks(300) {
        if batch.is_empty() {
            continue;
        }
        let placeholders = batch
            .iter()
            .enumerate()
            .map(|(index, _)| {
                format!(
                    "(?{}, ?{}, ?{})",
                    index * 3 + 1,
                    index * 3 + 2,
                    index * 3 + 3
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT OR IGNORE INTO bulk_matches (id, source, source_id) VALUES {placeholders}"
        );
        let values = batch.iter().flat_map(|entry| {
            [
                rusqlite::types::Value::Integer(entry.id),
                rusqlite::types::Value::Text(entry.source.clone()),
                rusqlite::types::Value::Text(entry.source_id.clone()),
            ]
        });
        let inserted = conn
            .execute(&sql, rusqlite::params_from_iter(values))
            .map_err(|e| format!("Bulk selection batch insert failed: {e}"))?;
        if inserted != batch.len() {
            return Err("Bulk search paging returned a duplicate download id".to_string());
        }
    }
    Ok(())
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

fn upsert_download_in_connection(conn: &Connection, dl: &NewDownload) -> Result<i64, String> {
    conn.execute(
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
            author_id = excluded.author_id,
            content_type = excluded.content_type,
            source_created_at = excluded.source_created_at,
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
    .map_err(|e| format!("Insert download failed: {e}"))?;

    let id = conn
        .query_row(
            "SELECT id FROM downloads WHERE source = ?1 AND source_id = ?2",
            params![dl.source, dl.source_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to query upserted download ID: {e}"))?;
    // コレクションと作品リンクの正本は source + source_id。作品を削除してから
    // 再取得しても、現在の downloads.id へ自動的に再解決する。
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE work_collection_members
         SET download_id = ?1, title_snapshot = ?2, author_snapshot = ?3, updated_at = ?4
         WHERE source = ?5 AND source_id = ?6",
        params![id, dl.title, dl.author_name, now, dl.source, dl.source_id],
    )
    .map_err(|e| format!("Failed to reconnect collection memberships: {e}"))?;
    conn.execute(
        "UPDATE work_links SET from_download_id = ?1, updated_at = ?2
         WHERE from_source = ?3 AND from_source_id = ?4",
        params![id, now, dl.source, dl.source_id],
    )
    .map_err(|e| format!("Failed to reconnect outgoing work links: {e}"))?;
    conn.execute(
        "UPDATE work_links SET to_download_id = ?1, updated_at = ?2
         WHERE to_source = ?3 AND to_source_id = ?4",
        params![id, now, dl.source, dl.source_id],
    )
    .map_err(|e| format!("Failed to reconnect incoming work links: {e}"))?;
    conn.execute(
        "DELETE FROM download_tags WHERE download_id = ?1",
        params![id],
    )
    .map_err(|e| format!("Failed to clear old tags: {e}"))?;
    for tag_name in normalized_tag_list(&dl.tags) {
        conn.execute(
            "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
            params![tag_name],
        )
        .map_err(|e| format!("Failed to insert tag: {e}"))?;
        let tag_id = conn
            .query_row(
                "SELECT id FROM tags WHERE name = ?1",
                params![tag_name],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("Failed to retrieve tag ID: {e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO download_tags (download_id, tag_id) VALUES (?1, ?2)",
            params![id, tag_id],
        )
        .map_err(|e| format!("Failed to insert download tag relation: {e}"))?;
    }
    Ok(id)
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

    let semantic_indexed_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM downloads d
             JOIN semantic_index_state s ON s.download_id = d.id
             WHERE s.current_version = d.current_version
               AND COALESCE(s.content_hash, '') = COALESCE(d.content_hash, '')
               AND s.model_id = ?1",
            params![super::semantic_index::model_id()],
            |row| row.get(0),
        )
        .map_err(|e| format!("Semantic index coverage failed: {e}"))?;
    let semantic_pending_downloads = total_downloads.saturating_sub(semantic_indexed_downloads);

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
        semantic_indexed_downloads,
        semantic_pending_downloads,
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
    match super::semantic_index::upsert_documents(storage_dir, &semantic_docs) {
        Ok(_) => {
            let states = docs
                .iter()
                .map(|doc| IndexedState {
                    download_id: doc.download_id,
                    current_version: doc.current_version,
                    content_hash: doc.content_hash.clone(),
                })
                .collect::<Vec<_>>();
            record_semantic_indexed_documents_locked(conn, &states)?;
        }
        Err(error) => log::warn!(
            "Semantic index batch update skipped for {} documents: {}",
            docs.len(),
            error
        ),
    }

    Ok(())
}

fn record_semantic_indexed_documents_locked(
    conn: &Connection,
    states: &[IndexedState],
) -> Result<(), String> {
    if states.is_empty() {
        return Ok(());
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Semantic state transaction failed: {e}"))?;
    let now = chrono::Utc::now().to_rfc3339();
    {
        let mut statement = tx
            .prepare_cached(
                "INSERT INTO semantic_index_state (
                    download_id, current_version, content_hash, model_id, indexed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(download_id) DO UPDATE SET
                    current_version = excluded.current_version,
                    content_hash = excluded.content_hash,
                    model_id = excluded.model_id,
                    indexed_at = excluded.indexed_at",
            )
            .map_err(|e| format!("Semantic state prepare failed: {e}"))?;
        for state in states {
            statement
                .execute(params![
                    state.download_id,
                    state.current_version,
                    state.content_hash,
                    super::semantic_index::model_id(),
                    now
                ])
                .map_err(|e| format!("Semantic state insert failed: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("Semantic state commit failed: {e}"))
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
        "DELETE FROM semantic_index_state WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Semantic state clear failed: {e}"))?;
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
        // あとから足した列。古い行や、列だけ足して一度も確認していない対象では
        // NULL / 0 のままになる。
        last_hit_at: row.get(12).unwrap_or(None),
        consecutive_errors: row.get(13).unwrap_or(0),
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

/// 列の位置ではなく名前で読む。
///
/// `SELECT s.*` に対して番号で読んでいたころは、`ALTER TABLE` で列がひとつ
/// 増えるだけで最後の列がひとつずれ、作品数として別の値を読んでいた。
fn series_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SeriesEntry> {
    Ok(SeriesEntry {
        id: row.get("id")?,
        source: row.get("source")?,
        source_key: row.get("source_key")?,
        title: row.get("title")?,
        description: row.get("description")?,
        cover_path: row.get("cover_path")?,
        content_hash: row.get("content_hash")?,
        current_version: row.get("current_version")?,
        last_checked_at: row.get("last_checked_at")?,
        last_fetched_at: row.get("last_fetched_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        work_count: row.get("work_count").ok(),
        is_concluded: row.get("is_concluded").unwrap_or(None),
        published_content_count: row.get("published_content_count").unwrap_or(None),
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

fn validated_download_work_root(
    storage_dir: &Path,
    source: &str,
    source_id: &str,
) -> Result<PathBuf, String> {
    for (label, value) in [("source", source), ("source id", source_id)] {
        let mut components = Path::new(value).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(format!("Invalid download {label}: {value}"));
        }
    }

    let work_root = storage_dir.join(source).join(source_id);
    if work_root.exists() {
        let storage_canonical = storage_dir
            .canonicalize()
            .map_err(|e| format!("Failed to validate storage directory: {e}"))?;
        let work_canonical = work_root
            .canonicalize()
            .map_err(|e| format!("Failed to validate work directory: {e}"))?;
        if !work_canonical.starts_with(&storage_canonical) {
            return Err(format!(
                "Refusing to delete work directory outside storage: {}",
                work_canonical.display()
            ));
        }
    }
    Ok(work_root)
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
mod title_order_tests {
    use super::*;

    fn order(title: &str) -> Option<i64> {
        title_stem_and_order(title).1
    }

    fn stem(title: &str) -> String {
        title_stem_and_order(title).0
    }

    /// 設計提案 6-3 が挙げる話数表記と、3 の例 C をそのまま通す。
    #[test]
    fn episode_numbers_cover_the_documented_notations() {
        assert_eq!(order("第12話"), Some(120));
        assert_eq!(order("12話"), Some(120));
        assert_eq!(order("#12"), Some(120));
        assert_eq!(order("その12"), Some(120));
        assert_eq!(order("十二"), Some(120));
        assert_eq!(order("XII"), Some(120));

        // 例 C: 表記が三者三様でも同じ連番として並ぶ。
        assert_eq!(order("航路 01"), Some(10));
        assert_eq!(order("航路 #2"), Some(20));
        assert_eq!(order("航路 第三夜"), Some(30));
        assert_eq!(stem("航路 01"), stem("航路 #2"));
        assert_eq!(stem("航路 #2"), stem("航路 第三夜"));
    }

    #[test]
    fn front_and_back_markers_order_within_one_episode() {
        assert!(order("月下の約束 前編") < order("月下の約束 後編"));
        assert_eq!(order("【FANBOX】月下の約束（後編）"), Some(3));
        assert_eq!(stem("【FANBOX】月下の約束（後編）"), "fanbox 月下の約束");
        // 素の 上/中/下 も単独トークンなら位置として読む。
        assert_eq!(order("旅路 上"), Some(1));
        assert_eq!(order("旅路 中"), Some(2));
        assert_eq!(order("旅路 下"), Some(3));
        // 話数があるときは枝番として合成される。
        assert_eq!(order("第3話 後編"), Some(33));
    }

    #[test]
    fn special_positions_sit_outside_the_numbered_run() {
        let prologue = order("ぷろろーぐ").expect("prologue");
        let first = order("第1話").expect("first");
        let finale = order("最終話").expect("finale");
        let epilogue = order("エピローグ").expect("epilogue");
        let side = order("番外編").expect("side story");
        assert!(prologue < first);
        assert!(first < finale);
        assert!(finale < epilogue);
        assert!(epilogue < side);
    }

    /// 語の一部を位置と読み違えないこと。ここが緩むと無関係な作品が混ざる。
    #[test]
    fn ordinary_words_are_not_mistaken_for_positions() {
        // `月下の約束` の `下` は語中なので後編ではない。
        assert_eq!(order("月下の約束"), None);
        // `finalize` は完結編ではない。
        assert_eq!(order("finalize"), None);
        // ローマ数字に使える文字だけの英単語も話数にしない。
        assert_eq!(order("civil"), None);
        // 西暦は単独トークンでも話数として拾わない。
        assert_eq!(order("記録 2026"), None);
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
            "教室",
            "図書館",
            "海岸",
            "旋律",
            "記憶",
            "季節",
            "手紙",
            "灯台",
            "回廊",
            "約束",
            "硝子",
            "残響",
            "標本",
            "封筒",
            "螺旋",
            "夜明",
            "輪郭",
            "潮騒",
            "書架",
            "遠雷",
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
        let (_temp, root, storage) = temp_paths();
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
    fn diagnostic_scan_aggregates_each_tree_in_one_bounded_walk() {
        let (_temp, root, storage) = temp_paths();
        let version = storage.join("pixiv/work/v1");
        let assets = version.join("data_assets");
        let lexical = storage.join("search-index");
        let semantic = root.join("search");
        fs::create_dir_all(&assets).unwrap();
        fs::create_dir_all(&lexical).unwrap();
        fs::create_dir_all(&semantic).unwrap();
        fs::write(version.join("original.json"), b"12345").unwrap();
        let known = assets.join("known.png");
        fs::write(&known, b"123").unwrap();
        fs::write(assets.join("orphan.png"), b"1234").unwrap();
        fs::write(lexical.join("segment.store"), b"123456").unwrap();
        fs::write(lexical.join("meta.json"), b"12").unwrap();
        fs::write(semantic.join("vectors.bin"), b"1234567").unwrap();
        let interrupted = storage.join("pixiv/work/.v2.0123abcd.stage");
        fs::create_dir_all(&interrupted).unwrap();
        fs::write(interrupted.join("data.json"), b"123").unwrap();
        let known_paths = HashSet::from([normalized_diagnostic_path(&known)]);
        let mut is_known_asset =
            |path: &Path| Ok(known_paths.contains(&normalized_diagnostic_path(path)));

        let stats =
            collect_diagnostic_file_stats(&storage, &semantic, &mut is_known_asset).unwrap();

        assert_eq!(stats.storage_size_bytes, 23);
        assert_eq!(stats.lexical_index_size_bytes, 8);
        assert_eq!(stats.lexical_index_file_count, 2);
        assert_eq!(stats.lexical_index_segment_count, 1);
        assert_eq!(stats.semantic_index_size_bytes, 7);
        assert_eq!(stats.orphan_asset_files, 1);
        assert_eq!(stats.orphan_asset_file_bytes, 4);
        assert_eq!(stats.transient_files, 1);
        assert_eq!(stats.transient_file_bytes, 3);
        assert_eq!(
            stats.visited_entries, 13,
            "each directory entry is visited once, including disjoint semantic storage"
        );
    }

    #[test]
    fn file_integrity_check_streams_and_classifies_manual_changes() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let missing_download = storage.join("pixiv/missing/v1/original.json");
        let missing_version = storage.join("pixiv/missing/v2/original.json");
        let asset = storage.join("pixiv/missing/v1/data_assets/image.png");
        let empty_profile = root.join("profiles/pixiv/empty/icon.png");
        let entity_json = root.join("series/pixiv/series-1/v1/data.json");
        let outside_profile = root.join("outside.png");
        fs::create_dir_all(asset.parent().unwrap()).unwrap();
        fs::create_dir_all(empty_profile.parent().unwrap()).unwrap();
        fs::create_dir_all(entity_json.parent().unwrap()).unwrap();
        fs::write(&asset, b"123").unwrap();
        fs::write(&empty_profile, b"").unwrap();
        fs::write(&entity_json, b"{}").unwrap();
        fs::write(&outside_profile, b"outside").unwrap();

        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO downloads (
                source, source_id, title, author_name, author_id, content_type,
                json_path, downloaded_at
             ) VALUES ('pixiv', 'missing', '作品', '作者', 'author', 'novel', ?1, ?2)",
            params![
                missing_download.to_string_lossy(),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .unwrap();
        let download_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO download_versions (
                download_id, version, json_path, created_at
             ) VALUES (?1, 2, ?2, ?3)",
            params![
                download_id,
                missing_version.to_string_lossy(),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO assets (
                download_id, asset_type, filename, local_path, file_size_bytes
             ) VALUES (?1, 'image', 'image.png', ?2, 9)",
            params![download_id, asset.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO people (source, source_key, display_name, icon_path)
             VALUES ('pixiv', 'outside', '外部', ?1)",
            params![outside_profile.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO series (source, source_key, title, cover_path)
             VALUES ('pixiv', 'empty', '空', ?1)",
            params![empty_profile.to_string_lossy()],
        )
        .unwrap();
        // シリーズの表紙は、最初に保存した作品の表紙をそのまま指すことがある。
        // downloads/ の下にあっても、これはアプリが置いたものである。
        conn.execute(
            "INSERT INTO series (source, source_key, title, cover_path)
             VALUES ('pixiv', 'borrowed', '借りた表紙', ?1)",
            params![asset.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entity_versions (
                entity_type, source, source_key, version, json_path, created_at
             ) VALUES ('series', 'pixiv', 'series-1', 1, ?1, ?2)",
            params![
                entity_json.to_string_lossy(),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .unwrap();

        let integrity = check_library_file_integrity(&conn, &storage, &root).unwrap();
        assert_eq!(integrity.checked_file_references, 7);
        assert_eq!(integrity.missing_json_files, 2);
        assert_eq!(integrity.missing_asset_files, 0);
        assert_eq!(integrity.missing_profile_files, 0);
        assert_eq!(
            integrity.unsafe_referenced_files, 1,
            "作品の表紙を借りたシリーズは、許可領域外ではない"
        );
        assert_eq!(integrity.unreadable_referenced_files, 0);
        assert_eq!(integrity.empty_referenced_files, 1);
        assert_eq!(integrity.mismatched_asset_files, 1);
        assert_eq!(integrity.issue_samples.len(), 5);
        assert!(integrity
            .issue_samples
            .iter()
            .any(|issue| issue.issue_type == "unsafe"
                && issue.path.ends_with("outside.png")
                && issue.label.as_deref() == Some("外部")));
        assert!(integrity
            .issue_samples
            .iter()
            .any(|issue| issue.issue_type == "missing" && issue.label.as_deref() == Some("作品")));
        assert!(integrity
            .issue_samples
            .iter()
            .any(|issue| issue.path.ends_with("icon.png")
                && issue.category == "profile"
                && issue.issue_type == "empty"));
        assert!(!integrity
            .issue_samples
            .iter()
            .any(|issue| issue.path.ends_with("data.json")));
        let lookup_plan = conn
            .prepare("EXPLAIN QUERY PLAN SELECT 1 FROM assets WHERE local_path = ?1 LIMIT 1")
            .unwrap()
            .query_map(params![asset.to_string_lossy()], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");
        assert!(
            lookup_plan.contains("idx_assets_local_path"),
            "{lookup_plan}"
        );
        drop(conn);
        drop(db);
    }

    #[test]
    fn diagnostic_scan_skips_links_and_enforces_depth_and_entry_limits() {
        let (_temp, root, storage) = temp_paths();
        let outside = root.join("outside");
        fs::create_dir_all(outside.join("data_assets")).unwrap();
        fs::write(outside.join("data_assets/escape.png"), b"outside").unwrap();
        fs::write(storage.join("inside.bin"), b"i").unwrap();
        let link = storage.join("linked");
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&outside, &link);
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(&outside, &link);
        #[cfg(not(any(unix, windows)))]
        let link_result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "links unavailable",
        ));
        if link_result.is_ok() {
            let mut no_known_assets = |_path: &Path| Ok(false);
            let stats =
                collect_diagnostic_file_stats(&storage, &root.join("search"), &mut no_known_assets)
                    .unwrap();
            assert_eq!(stats.storage_size_bytes, 1);
            assert_eq!(stats.orphan_asset_files, 0);
        }

        let mut no_known_assets = |_path: &Path| Ok(false);
        fs::create_dir_all(storage.join("a/b")).unwrap();
        let depth_error = collect_diagnostic_file_stats_with_limits(
            &storage,
            &root.join("search"),
            &mut no_known_assets,
            DiagnosticScanLimits {
                max_depth: 0,
                max_entries: 100,
            },
        )
        .unwrap_err();
        assert!(depth_error.contains("depth-0"));

        let entry_error = collect_diagnostic_file_stats_with_limits(
            &storage,
            &root.join("search"),
            &mut no_known_assets,
            DiagnosticScanLimits {
                max_depth: 64,
                max_entries: 1,
            },
        )
        .unwrap_err();
        assert!(entry_error.contains("1-entry"));
    }

    #[test]
    fn atomic_restore_transaction_rolls_back_database_rows() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn injected_save_failure_rolls_back_download_tags_assets_and_version() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let json_path = storage.join("pixiv/atomic-save/v1/original.json");
        let download = NewDownload {
            source: "pixiv".to_string(),
            source_id: "atomic-save".to_string(),
            title: "Atomic save".to_string(),
            author_name: "Author".to_string(),
            author_id: "author".to_string(),
            content_type: "novel".to_string(),
            tags: vec!["atomic".to_string()],
            excerpt: None,
            cover_path: None,
            json_path: json_path.to_string_lossy().to_string(),
            original_json_path: Some(json_path.to_string_lossy().to_string()),
            asset_count: 1,
            file_size_bytes: 10,
            downloaded_at: "2026-08-12T00:00:00Z".to_string(),
            source_created_at: None,
            content_hash: Some("hash".to_string()),
            text_length: 4,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        };
        let assets = vec![NewAsset {
            download_id: 0,
            asset_type: "illustration".to_string(),
            filename: "image.png".to_string(),
            local_path: storage
                .join("pixiv/atomic-save/v1/data_assets/image.png")
                .to_string_lossy()
                .to_string(),
            original_url: None,
            mime_type: Some("image/png".to_string()),
            file_size_bytes: 6,
        }];
        let versions = vec![NewVersion {
            download_id: 0,
            version: 1,
            content_hash: Some("hash".to_string()),
            text_length: 4,
            json_path: download.json_path.clone(),
            original_json_path: download.original_json_path.clone(),
            asset_count: 1,
            file_size_bytes: 10,
            created_at: download.downloaded_at.clone(),
            change_summary: None,
        }];

        assert!(db
            .commit_download_save_with_injected_failure(&download, &assets, &versions)
            .is_err());

        let duplicate_versions = vec![versions[0].clone(), versions[0].clone()];
        assert!(db
            .commit_reimported_download(&download, &assets, &duplicate_versions)
            .is_err());

        assert!(db
            .get_download_by_source("pixiv", "atomic-save")
            .unwrap()
            .is_none());
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM download_tags", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM download_versions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        drop(conn);
    }

    #[test]
    fn startup_removes_only_journaled_uncommitted_versions_and_reserved_stages() {
        let (_temp, root, storage) = temp_paths();
        let db_path = root.join("piep.db");
        let db = Database::open(&db_path, &storage).unwrap();
        insert_download_unindexed(
            &db,
            &storage,
            "crash-recovery",
            "Committed work",
            "Author",
            &[],
            "body",
        );
        let work = storage.join("pixiv/crash-recovery");
        let committed = work.join("v1");
        let unjournaled = work.join("v2");
        let stage = work.join(".v2.0123456789abcdef.stage");
        let published_stage = work.join(".v3.fedcba9876543210.stage");
        let published = work.join("v3");
        let unrelated = work.join("notes");
        fs::create_dir_all(&unjournaled).unwrap();
        fs::write(unjournaled.join("original.json"), b"user data").unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("original.json"), b"staged").unwrap();
        fs::create_dir_all(&published_stage).unwrap();
        fs::write(published_stage.join("original.json"), b"published").unwrap();
        db.create_download_save_journal("pixiv", "crash-recovery", 3, &published_stage, &published)
            .unwrap();
        fs::rename(&published_stage, &published).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        drop(db);

        let reopened = Database::open(&db_path, &storage).unwrap();

        assert!(committed.exists());
        assert!(
            unjournaled.exists(),
            "an unregistered version may be user data awaiting import"
        );
        assert!(!stage.exists());
        assert!(!published.exists(), "journal proves v3 was never committed");
        assert!(unrelated.exists(), "unknown directories are user data");
        let conn = reopened.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM download_save_journal", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        drop(conn);
        drop(reopened);
    }

    #[test]
    fn startup_preserves_a_version_committed_with_its_journal_marker() {
        let (_temp, root, storage) = temp_paths();
        let db_path = root.join("piep.db");
        let db = Database::open(&db_path, &storage).unwrap();
        let work = storage.join("pixiv/committed-journal");
        let stage = work.join(".v1.0123456789abcdef.stage");
        let final_path = work.join("v1");
        fs::create_dir_all(&stage).unwrap();
        let json_path = final_path.join("original.json");
        fs::write(stage.join("original.json"), b"committed body").unwrap();
        let journal_id = db
            .create_download_save_journal("pixiv", "committed-journal", 1, &stage, &final_path)
            .unwrap();
        fs::rename(&stage, &final_path).unwrap();
        let download = NewDownload {
            source: "pixiv".to_string(),
            source_id: "committed-journal".to_string(),
            title: "Committed work".to_string(),
            author_name: "Author".to_string(),
            author_id: "author".to_string(),
            content_type: "novel".to_string(),
            tags: Vec::new(),
            excerpt: None,
            cover_path: None,
            json_path: json_path.to_string_lossy().to_string(),
            original_json_path: Some(json_path.to_string_lossy().to_string()),
            asset_count: 0,
            file_size_bytes: 14,
            downloaded_at: "2026-08-12T00:00:00Z".to_string(),
            source_created_at: None,
            content_hash: Some("committed-hash".to_string()),
            text_length: 14,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        };
        let version = NewVersion {
            download_id: 0,
            version: 1,
            content_hash: download.content_hash.clone(),
            text_length: download.text_length,
            json_path: download.json_path.clone(),
            original_json_path: download.original_json_path.clone(),
            asset_count: 0,
            file_size_bytes: download.file_size_bytes,
            created_at: download.downloaded_at.clone(),
            change_summary: None,
        };
        db.commit_download_save_with_journal(&download, &[], &[version], &journal_id)
            .unwrap();
        drop(db);

        let reopened = Database::open(&db_path, &storage).unwrap();

        assert!(final_path.join("original.json").exists());
        assert!(reopened
            .get_download_by_source("pixiv", "committed-journal")
            .unwrap()
            .is_some());
        let conn = reopened.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM download_save_journal", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        drop(conn);
        drop(reopened);
    }

    #[test]
    fn reimport_record_delete_preserves_the_scanned_work_tree() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let id = insert_download_unindexed(
            &db,
            &storage,
            "reimport-preserve",
            "再取り込み作品",
            "作者",
            &[],
            "本文",
        );
        let json_path = PathBuf::from(db.get_download(id).unwrap().json_path);

        db.delete_download_record_for_reimport(id).unwrap();

        assert!(
            json_path.exists(),
            "the scanner still needs to read this file"
        );
        assert!(db
            .get_download_by_source("pixiv", "reimport-preserve")
            .unwrap()
            .is_none());
    }

    #[test]
    fn deleting_a_legacy_work_never_removes_its_source_siblings() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let insert_legacy = |source_id: &str, title: &str| {
            let work_dir = storage.join("pixiv").join(source_id);
            fs::create_dir_all(&work_dir).unwrap();
            let json_path = work_dir.join("data.json");
            fs::write(&json_path, serde_json::json!({ "text": title }).to_string()).unwrap();
            let id = db
                .upsert_download(&NewDownload {
                    source: "pixiv".to_string(),
                    source_id: source_id.to_string(),
                    title: title.to_string(),
                    author_name: "作者".to_string(),
                    author_id: "legacy-author".to_string(),
                    content_type: "novel".to_string(),
                    tags: Vec::new(),
                    excerpt: None,
                    cover_path: None,
                    json_path: json_path.to_string_lossy().to_string(),
                    original_json_path: None,
                    asset_count: 0,
                    file_size_bytes: 0,
                    downloaded_at: "2026-01-01T00:00:00Z".to_string(),
                    source_created_at: None,
                    content_hash: None,
                    text_length: 0,
                    source_updated_at: None,
                    watch_updates: false,
                    current_version: 1,
                    favorite: false,
                })
                .unwrap();
            (id, work_dir, json_path)
        };
        let (deleted_id, deleted_dir, _) = insert_legacy("legacy-one", "削除対象");
        let (_, sibling_dir, sibling_json) = insert_legacy("legacy-two", "残る作品");

        db.delete_download(deleted_id).unwrap();

        assert!(!deleted_dir.exists());
        assert!(sibling_dir.exists());
        assert!(sibling_json.exists(), "another work in pixiv must survive");
    }

    #[test]
    fn entity_reconstruction_preserves_fetched_profiles_and_versions() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let id = insert_download_unindexed(
            &db,
            &storage,
            "profile-work",
            "プロフィール付き作品",
            "作者",
            &[],
            "本文",
        );
        let author_key = "author-profile-work";
        db.upsert_download_person(id, "pixiv", author_key, "author", "作者")
            .unwrap();
        db.upsert_download_relation(id, "series", "pixiv", "series-1", "連作")
            .unwrap();
        db.upsert_download_series(id, "pixiv", "series-1", "連作", Some(1))
            .unwrap();
        db.upsert_person_profile(
            "pixiv",
            author_key,
            "作者",
            Some("profiles/author/icon.png"),
            Some("profiles/author/cover.png"),
            Some("保存済みプロフィール"),
            Some("{\"web\":\"https://example.com\"}"),
            "person-hash",
            "profiles/author/v1/data.json",
            2,
            128,
            EntityProfileFreshness::RemoteChecked,
        )
        .unwrap();
        db.upsert_series_profile(
            "pixiv",
            "series-1",
            "連作",
            Some("保存済みシリーズ説明"),
            Some("series/series-1/cover.png"),
            "series-hash",
            "series/series-1/v1/data.json",
            1,
            64,
            EntityProfileFreshness::RemoteChecked,
            None,
            None,
        )
        .unwrap();

        db.reconstruct_entities_after_import().unwrap();

        let person = db.get_person("pixiv", author_key).unwrap();
        assert_eq!(person.description.as_deref(), Some("保存済みプロフィール"));
        assert_eq!(person.current_version, 1);
        assert_eq!(
            db.list_entity_versions("person", "pixiv", author_key)
                .unwrap()
                .len(),
            1
        );
        let series = db.get_series("pixiv", "series-1").unwrap();
        assert_eq!(series.description.as_deref(), Some("保存済みシリーズ説明"));
        assert_eq!(series.current_version, 1);
        assert_eq!(
            db.list_entity_versions("series", "pixiv", "series-1")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn reader_cache_pages_and_full_document_search_share_one_index() {
        let (_temp, root, storage) = temp_paths();
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

    /// 前の実行が残した一時ディレクトリを一度だけ掃除する。
    ///
    /// 取りこぼしを掃除するための保険。通常の後始末は `TempRoot` が行うが、
    /// それが無かった頃の残骸が temp に積み上がっている。実行中の別プロセスを
    /// 巻き込まないよう、1 時間より古いものだけを対象にする。
    fn sweep_stale_test_dirs() {
        static SWEEP: std::sync::Once = std::sync::Once::new();
        SWEEP.call_once(|| {
            let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
                return;
            };
            let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3_600);
            for entry in entries.flatten() {
                let name = entry.file_name();
                if !name.to_string_lossy().starts_with("piep_search_test_") {
                    continue;
                }
                let stale = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .is_ok_and(|modified| modified < cutoff);
                if stale {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        });
    }

    /// テストの一時ツリー。テスト終了時に削除する。
    ///
    /// 後始末は `Database` より後に走らなければならない。SQLite は削除を許す
    /// 共有モードでファイルを開かないので、`db` が生きている間は Windows の
    /// 削除が共有違反で失敗する。旧コードは `db` が生きたまま `remove_dir_all`
    /// を呼んでいたため、一時ディレクトリを数千個積み残していた。`Database`
    /// より前に束縛すればローカルは宣言と逆順に drop されるので、この順序が
    /// 自然に得られる。末尾の `remove_dir_all` と違って panic や早期 return
    /// でも走る。
    ///
    /// それでも削除は best-effort のままにしてある。接続の後始末が drop の
    /// 直後まで尾を引くことがあり、失敗を致命扱いにすると後始末そのものが
    /// テストを落とす要因になるからである。
    struct TempRoot {
        root: PathBuf,
        storage: PathBuf,
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            super::super::tantivy_index::release_runtime(&self.storage);
            // Windows unlinks a file that still has an open handle by marking it
            // delete-pending: the contents go, but the name survives until the
            // last handle closes, and a directory that still holds names cannot
            // be removed. SQLite's connections finish closing just after
            // `Database` drops, so the first attempt clears the tree and a
            // retry a few milliseconds later clears the directory itself.
            for attempt in 0..5 {
                if fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2 << attempt));
            }
        }
    }

    fn temp_paths() -> (TempRoot, PathBuf, PathBuf) {
        sweep_stale_test_dirs();
        let rand_val: u32 = rand::random();
        let root = std::env::temp_dir().join(format!("piep_search_test_{}", rand_val));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        let guard = TempRoot {
            root: root.clone(),
            storage: storage.clone(),
        };
        (guard, root, storage)
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

    /// Seed a metadata-only browsing fixture in one transaction. Creating one
    /// directory and JSON file per work made a million-row acceptance test
    /// measure NTFS setup/cleanup for hours instead of the library queries we
    /// care about. The generated rows still exercise production indexes,
    /// author aggregation, tags, shelves, dates, and cursor tie-breaks.
    fn seed_metadata_only_library(db: &Database, works: usize) {
        assert!((1..=1_000_000).contains(&works));
        let mut conn = db.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "WITH digits(d) AS (
                 VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9)
             ), numbers(n) AS (
                 SELECT a.d + 10*b.d + 100*c.d + 1000*d.d + 10000*e.d + 100000*f.d
                 FROM digits a
                 CROSS JOIN digits b
                 CROSS JOIN digits c
                 CROSS JOIN digits d
                 CROSS JOIN digits e
                 CROSS JOIN digits f
             )
             INSERT INTO downloads (
                 source, source_id, title, author_name, author_id, content_type,
                 excerpt, json_path, asset_count, file_size_bytes, downloaded_at,
                 source_created_at, content_hash, text_length, watch_updates,
                 current_version, favorite
             )
             SELECT
                 CASE WHEN n % 5 = 0 THEN 'fanbox' ELSE 'pixiv' END,
                 printf('scale-%07d', n),
                 printf('蔵書 %07d', n),
                 printf('作者 %03d', n % 400),
                 printf('author-%03d', n % 400),
                 CASE WHEN n % 5 = 0 THEN 'article' ELSE 'novel' END,
                 '大規模ライブラリ受入試験',
                 printf('synthetic/scale-%07d.json', n),
                 n % 6,
                 4096 + (n % 1048576),
                 printf('2026-%02d-%02dT%02d:00:00Z', 1 + n % 12, 1 + n % 27, n % 24),
                 printf('2025-%02d-%02dT00:00:00Z', 1 + n % 12, 1 + n % 27),
                 printf('fixture-hash-%07d', n),
                 1000 + (n % 200000),
                 CASE WHEN n % 13 = 0 THEN 1 ELSE 0 END,
                 1,
                 CASE WHEN n % 11 = 0 THEN 1 ELSE 0 END
             FROM numbers
             WHERE n < ?1",
            params![works as i64],
        )
        .unwrap();
        for tag in 0..30 {
            tx.execute(
                "INSERT INTO tags (name) VALUES (?1)",
                params![format!("tag{tag}")],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO download_tags (download_id, tag_id)
             SELECT id, 1 + ((id - 1) % 30) FROM downloads",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        conn.execute_batch("ANALYZE; PRAGMA optimize;").unwrap();
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

    /// 実ライブラリで見つかった並び順の崩れを固定する。番号のない初回が
    /// 末尾へ流れ、単なる言及リンクで加点された作品が話数を追い越していた。
    #[test]
    fn unnumbered_opening_leads_and_mentions_do_not_reorder_episodes() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let opening = insert_download_unindexed(
            &db,
            &storage,
            "suggest-open",
            "灯台の話",
            "連載作者",
            &[],
            "初回本文",
        );
        let second = insert_download_unindexed(
            &db,
            &storage,
            "suggest-2",
            "灯台の話#2",
            "連載作者",
            &[],
            "二話本文",
        );
        let third = insert_download_unindexed(
            &db,
            &storage,
            "suggest-3",
            "灯台の話#3",
            "連載作者",
            &[],
            "三話本文",
        );

        // 第2話を種にしても、無印の初回が先頭に来る。
        let suggestion = db
            .generate_collection_suggestion(&CollectionSuggestionRequest {
                seed_download_ids: vec![second],
                limit: Some(20),
            })
            .unwrap();
        let order = suggestion
            .members
            .iter()
            .map(|member| member.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["灯台の話", "灯台の話#2", "灯台の話#3"]);
        assert_eq!(suggestion.collection_kind, "ordered");
        let _ = (opening, third);
    }

    /// 公式シリーズに同居しているだけの作品まで既定で選ぶと、短編集を丸ごと
    /// 取り込んでしまう。順序も過半数で決める。
    #[test]
    fn series_only_siblings_are_offered_but_not_preselected() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let seed = insert_download_unindexed(
            &db,
            &storage,
            "anth-seed",
            "海の便り",
            "短編作者",
            &[],
            "本文",
        );
        let mut others = Vec::new();
        for index in 0..4 {
            others.push(insert_download_unindexed(
                &db,
                &storage,
                &format!("anth-{index}"),
                &format!("まったく別の話{index}"),
                "短編作者",
                &[],
                "本文",
            ));
        }
        for id in std::iter::once(seed).chain(others.iter().copied()) {
            db.upsert_download_series(id, "pixiv", "anthology", "短編集", None)
                .unwrap();
        }
        let suggestion = db
            .generate_collection_suggestion(&CollectionSuggestionRequest {
                seed_download_ids: vec![seed],
                limit: Some(20),
            })
            .unwrap();
        assert_eq!(suggestion.collection_kind, "unordered");
        let preselected = suggestion
            .members
            .iter()
            .filter(|member| member.selected)
            .count();
        assert_eq!(preselected, 1, "種だけが既定で選ばれる");
    }

    #[test]
    fn work_collections_keep_order_and_reconnect_deleted_works() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download_unindexed(
            &db,
            &storage,
            "collection-first",
            "夜の手紙 前編",
            "同じ作者",
            &[],
            "前編本文",
        );
        let second = insert_download_unindexed(
            &db,
            &storage,
            "collection-second",
            "夜の手紙 後編",
            "同じ作者",
            &[],
            "後編本文",
        );
        let collection = db
            .upsert_work_collection(&WorkCollectionInput {
                id: None,
                name: "夜の手紙".to_string(),
                description: Some("前後編".to_string()),
                collection_kind: "ordered".to_string(),
                cover_download_id: Some(first),
            })
            .unwrap();
        let collection = db
            .add_work_collection_members(
                &collection.summary.id,
                &[
                    WorkCollectionMemberInput {
                        source: "pixiv".to_string(),
                        source_id: "collection-first".to_string(),
                        title_snapshot: None,
                        author_snapshot: None,
                        position: None,
                        member_role: None,
                        added_by: None,
                        pinned: None,
                        note: None,
                    },
                    WorkCollectionMemberInput {
                        source: "pixiv".to_string(),
                        source_id: "collection-second".to_string(),
                        title_snapshot: None,
                        author_snapshot: None,
                        position: None,
                        member_role: None,
                        added_by: None,
                        pinned: None,
                        note: None,
                    },
                ],
            )
            .unwrap();
        assert_eq!(collection.summary.member_count, 2);
        assert_eq!(collection.summary.available_count, 2);

        let reordered = db
            .reorder_work_collection_members(
                &collection.summary.id,
                &[
                    WorkKey {
                        source: "pixiv".to_string(),
                        source_id: "collection-second".to_string(),
                    },
                    WorkKey {
                        source: "pixiv".to_string(),
                        source_id: "collection-first".to_string(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(reordered.members[0].download_id, Some(second));

        db.delete_download(first).unwrap();
        let missing = db.get_work_collection(&collection.summary.id).unwrap();
        assert_eq!(missing.summary.member_count, 2);
        assert_eq!(missing.summary.available_count, 1);
        assert!(missing
            .members
            .iter()
            .any(|member| member.source_id == "collection-first" && member.missing));

        let restored = insert_download_unindexed(
            &db,
            &storage,
            "collection-first",
            "夜の手紙 前編・改訂",
            "同じ作者",
            &[],
            "再取得本文",
        );
        assert_ne!(restored, first);
        let reconnected = db.get_work_collection(&collection.summary.id).unwrap();
        let first_member = reconnected
            .members
            .iter()
            .find(|member| member.source_id == "collection-first")
            .unwrap();
        assert_eq!(first_member.download_id, Some(restored));
        assert_eq!(first_member.title, "夜の手紙 前編・改訂");
        assert!(!first_member.missing);
    }

    #[test]
    fn content_links_recognize_pixiv_and_fanbox_with_direction() {
        let links = extract_work_link_evidence(
            "前編はこちら https://www.pixiv.net/novel/show.php?id=123 。続きは https://creator.fanbox.cc/posts/456",
            "pixiv",
            "999",
        );
        assert_eq!(links.len(), 2);
        assert!(links
            .iter()
            .any(|link| link.to_source == "pixiv" && link.to_source_id == "123"));
        assert!(links.iter().any(|link| {
            link.to_source == "fanbox"
                && link.to_source_id == "456"
                && link.relation_type == "continues_to"
        }));
    }

    #[test]
    fn collection_suggestions_use_title_parts_and_learn_rejection() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download_unindexed(
            &db,
            &storage,
            "suggest-first",
            "星を待つ 前編",
            "連載作者",
            &[],
            "導入",
        );
        let _second = insert_download_unindexed(
            &db,
            &storage,
            "suggest-second",
            "星を待つ 後編",
            "連載作者",
            &[],
            "結末",
        );
        let suggestion = db
            .generate_collection_suggestion(&CollectionSuggestionRequest {
                seed_download_ids: vec![first],
                limit: Some(20),
            })
            .unwrap();
        assert_eq!(suggestion.members.len(), 2);
        assert!(suggestion.members.iter().any(|member| {
            member.source_id == "suggest-second"
                && member
                    .evidence
                    .iter()
                    .any(|evidence| evidence.kind == "title_similarity")
        }));
        assert!(db
            .reject_collection_suggestion(&suggestion.id, None)
            .unwrap());

        let after_rejection = db.generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![first],
            limit: Some(20),
        });
        assert_eq!(
            after_rejection.unwrap_err(),
            "関連作品は見つかりませんでした"
        );

        // Feedback belongs to the rule that produced it. A future rule can
        // evaluate the pair again instead of leaving an irreversible dead end.
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE collection_pair_feedback SET rule_version = 'collection-suggest-v1'",
                [],
            )
            .unwrap();
        let reconsidered = db
            .generate_collection_suggestion(&CollectionSuggestionRequest {
                seed_download_ids: vec![first],
                limit: Some(20),
            })
            .unwrap();
        assert_eq!(reconsidered.members.len(), 2);
    }

    #[test]
    fn collection_suggestions_walk_the_whole_link_component_from_either_end() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download(
            &db,
            &storage,
            "410001",
            "倉本エリカを手に入れたあなたが、棚ぼたで鈴原美紗も寝取って発情種付け立ちバック交尾を堪能しちゃうお話",
            "連載作者",
            &[],
            "第一作",
        );
        let second = insert_download(
            &db,
            &storage,
            "410002",
            "倉本エリカと鈴原美紗を堕としたあなたが、温泉旅館で極上美女二人から迫られるハーレム3P交尾を好き放題に堪能しちゃうお話",
            "連載作者",
            &[],
            "第二作",
        );
        let third = insert_download(
            &db,
            &storage,
            "410003",
            "超人気アイドル兼魔法少女の倉本エリカが、あなたを引き留める為にマットプレイでアナル舐め＆スパイダー騎乗位膣内射精をさせてくれる話",
            "連載作者",
            &[],
            "前作はこちら https://www.pixiv.net/novel/show.php?id=410002",
        );
        let fourth = insert_download(
            &db,
            &storage,
            "410004",
            "超人気アイドル兼魔法少女の倉本エリカを庇ったあなたが、生殖本能むき出しの世界一気持ちいい膣内射精立ちバック交尾を出来ちゃう話",
            "連載作者",
            &[],
            "前作はこちら https://www.pixiv.net/novel/show.php?id=410003",
        );

        // The first edge exists only in the caption/excerpt; the others are in
        // the body. Both surfaces must feed the same normalized link graph.
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE downloads SET excerpt = ?1 WHERE id = ?2",
                params![
                    "前作はこちら https://www.pixiv.net/novel/show.php?id=410001",
                    second
                ],
            )
            .unwrap();
        db.reindex_download(second).unwrap();

        // Simulate an existing library whose full-text index predates the
        // normalized work-link graph. Starting at the first work must discover
        // incoming references and keep walking from every newly found work.
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM work_links", [])
            .unwrap();
        let from_first = db
            .generate_collection_suggestion(&CollectionSuggestionRequest {
                seed_download_ids: vec![first],
                limit: Some(20),
            })
            .unwrap();
        assert_eq!(
            from_first
                .members
                .iter()
                .map(|member| member.download_id.unwrap())
                .collect::<Vec<_>>(),
            vec![first, second, third, fourth]
        );
        assert!(from_first.members.iter().skip(1).all(|member| member
            .evidence
            .iter()
            .any(|evidence| evidence.kind == "content_link")));
        assert!(from_first.members.iter().any(|member| member
            .evidence
            .iter()
            .any(|evidence| evidence.label.contains("3段追跡"))));

        // The opposite endpoint works too: its outgoing "previous work" links
        // lead back to the first chapter and are ordered by their direction.
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM work_links", [])
            .unwrap();
        let from_last = db
            .generate_collection_suggestion(&CollectionSuggestionRequest {
                seed_download_ids: vec![fourth],
                limit: Some(20),
            })
            .unwrap();
        assert_eq!(
            from_last
                .members
                .iter()
                .map(|member| member.download_id.unwrap())
                .collect::<Vec<_>>(),
            vec![first, second, third, fourth]
        );
    }

    #[test]
    fn bulk_delete_clears_lexical_and_semantic_sidecars() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download(
            &db,
            &storage,
            "delete-index-one",
            "quartzdeletemarker alpha",
            "作者",
            &[],
            "意味検索からも消える本文 alpha",
        );
        let second = insert_download(
            &db,
            &storage,
            "delete-index-two",
            "quartzdeletemarker beta",
            "作者",
            &[],
            "意味検索からも消える本文 beta",
        );

        let before =
            crate::database::tantivy_index::matching_download_ids(&storage, "quartzdeletemarker")
                .unwrap();
        assert!(before.contains(&first));
        assert!(before.contains(&second));
        assert!(crate::database::semantic_index::status(&storage).indexed_chunks > 0);

        let result = db.delete_downloads(&[first, second, first]).unwrap();
        assert_eq!(result.matched_count, 2);
        assert_eq!(result.changed_count, 2);

        let after =
            crate::database::tantivy_index::matching_download_ids(&storage, "quartzdeletemarker")
                .unwrap();
        assert!(!after.contains(&first));
        assert!(!after.contains(&second));
        assert_eq!(
            crate::database::semantic_index::status(&storage).indexed_chunks,
            0
        );
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
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn update_target_keyset_pages_have_no_gaps_or_duplicates() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for (target_type, source, source_key) in [
            ("work", "pixiv", "w2"),
            ("person", "pixiv", "p2"),
            ("person", "fanbox", "p1"),
            ("person", "pixiv", "p1"),
            ("series", "pixiv", "s1"),
        ] {
            db.upsert_update_target(&UpdateTargetInput {
                target_type: target_type.to_string(),
                source: source.to_string(),
                source_key: source_key.to_string(),
                display_name: source_key.to_string(),
                enabled: true,
                metadata_json: None,
            })
            .unwrap();
        }

        let mut all = Vec::new();
        let mut cursor: Option<(String, String, String)> = None;
        loop {
            let page = db
                .list_update_targets_after(
                    cursor
                        .as_ref()
                        .map(|(kind, source, key)| (kind.as_str(), source.as_str(), key.as_str())),
                    2,
                )
                .unwrap();
            if page.is_empty() {
                break;
            }
            let last = page.last().unwrap();
            cursor = Some((
                last.target_type.clone(),
                last.source.clone(),
                last.source_key.clone(),
            ));
            all.extend(
                page.into_iter()
                    .map(|target| (target.target_type, target.source, target.source_key)),
            );
        }
        assert_eq!(all.len(), 5);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        let unique = all.iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), all.len());
    }

    #[test]
    fn bulk_search_collection_crosses_the_former_single_page_boundary() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const SIMULATED_LEGACY_CAP: usize = 5;
        const MATCHES: usize = SIMULATED_LEGACY_CAP * 2 + 3;
        for index in 0..MATCHES {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("bulk-page-{index:02}"),
                &format!("一括対象 {index:02}"),
                "作者",
                &["一括"],
                "本文",
            );
        }

        let mut selection = v2_params(None, 1, None);
        selection.offset = Some(9);
        let snapshot = db
            .collect_search_match_snapshot(&selection, SIMULATED_LEGACY_CAP as i64)
            .unwrap();
        assert_eq!(snapshot.row_count, MATCHES as i64);
        let guard = snapshot.connection.lock().unwrap();
        let conn = guard.as_ref().unwrap();
        let (rows, distinct): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT id) FROM bulk_matches",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, MATCHES as i64);
        assert_eq!(distinct, MATCHES as i64);
        drop(guard);
    }

    #[test]
    fn bulk_search_snapshot_updates_and_deletes_without_retaining_all_ids() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const MATCHES: usize = 13;
        for index in 0..MATCHES {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("bulk-mutate-{index:02}"),
                &format!("一括変更 {index:02}"),
                "作者",
                &["一括変更"],
                "本文",
            );
        }

        let selection = v2_params(None, 3, None);
        let watched = db.set_watch_updates_for_search(&selection, true).unwrap();
        assert_eq!(watched.matched_count, MATCHES as i64);
        assert_eq!(watched.changed_count, MATCHES as i64);
        {
            let conn = db.conn.lock().unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM downloads WHERE watch_updates = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                MATCHES as i64
            );
            // 作品の監視は downloads.watch_updates だけで表す。監視対象の一覧は
            // 「自分で選んだ作者・シリーズ」のためのもので、作品名を混ぜない。
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM update_targets WHERE target_type = 'work'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
        }

        let deleted = db.delete_downloads_for_search(&selection).unwrap();
        assert_eq!(deleted.matched_count, MATCHES as i64);
        assert_eq!(deleted.changed_count, MATCHES as i64);
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM downloads", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        let snapshot_files = fs::read_dir(search_snapshot_dir(&storage))
            .unwrap()
            .filter_map(Result::ok)
            .count();
        assert_eq!(snapshot_files, 0, "bulk snapshot must be cleaned up");
    }

    #[test]
    fn facet_search_limits_in_sql_and_keeps_direct_rare_matches() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn facet_and_suggestion_caches_hit_then_invalidate_on_commit() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        insert_download(
            &db,
            &storage,
            "cache-generation-1",
            "世代候補 一冊目",
            "世代作者",
            &["世代タグ"],
            "本文",
        );

        let first_facets = db.search_filter_facets("tags", None, 30).unwrap();
        assert_eq!(
            first_facets
                .iter()
                .find(|facet| facet.name == "世代タグ")
                .map(|facet| facet.count),
            Some(1)
        );
        let _ = db.search_filter_facets("tags", None, 30).unwrap();

        let suggest_params = SearchSuggestParams {
            text: Some("世代候補".to_string()),
            limit: Some(12),
        };
        assert_eq!(db.search_suggest(&suggest_params).unwrap().items.len(), 1);
        let _ = db.search_suggest(&suggest_params).unwrap();
        let (facet_before, suggest_before, _) = db.query_cache_stats();
        assert!(facet_before.0 >= 1, "facet cache should have a hit");
        assert!(suggest_before.0 >= 1, "suggest cache should have a hit");

        let second_id = insert_download(
            &db,
            &storage,
            "cache-generation-2",
            "世代候補 二冊目",
            "世代作者",
            &["世代タグ"],
            "本文",
        );
        let updated_facets = db.search_filter_facets("tags", None, 30).unwrap();
        assert_eq!(
            updated_facets
                .iter()
                .find(|facet| facet.name == "世代タグ")
                .map(|facet| facet.count),
            Some(2),
            "a committed write must invalidate cached aggregates"
        );
        assert_eq!(db.search_suggest(&suggest_params).unwrap().items.len(), 2);
        let (facet_after, suggest_after, _) = db.query_cache_stats();
        assert!(facet_after.1 > facet_before.1);
        assert!(suggest_after.1 > suggest_before.1);

        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE downloads SET title = '別の候補' WHERE id = ?1",
                params![second_id],
            )
            .unwrap();
        }
        assert_eq!(
            db.search_suggest(&suggest_params).unwrap().items.len(),
            1,
            "a direct committed update must invalidate cached suggestions"
        );

        db.delete_download(second_id).unwrap();
        let after_delete = db.search_filter_facets("tags", None, 30).unwrap();
        assert_eq!(
            after_delete
                .iter()
                .find(|facet| facet.name == "世代タグ")
                .map(|facet| facet.count),
            Some(1),
            "a committed delete must invalidate cached aggregates"
        );
    }

    #[test]
    fn lexical_search_reports_full_hits_beyond_the_candidate_page() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn lexical_cursor_reaches_every_match_beyond_one_thousand_results() {
        let (_temp, root, storage) = temp_paths();
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
                        assert_eq!(decoded.kind, "ranked-search");
                        assert!(decoded.snapshot_id.is_some());
                        assert!(decoded.tantivy_score.is_some());
                        checked_native_cursor = true;
                    }
                    request.cursor = Some(cursor);
                }
                None => break,
            }
        }

        assert!(checked_native_cursor);
        assert_eq!(seen.len(), ITEM_COUNT);
    }

    #[test]
    fn lexical_search_after_does_not_skip_filtered_hits_inside_a_batch() {
        let (_temp, root, storage) = temp_paths();
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
        let mut checked_internal_total = false;
        loop {
            let result = db.search_downloads_v2(&request).unwrap();
            for item in result.items {
                assert!(seen.insert(item.id));
                assert!(item.tags.iter().any(|tag| tag == "対象"));
            }
            match result.next_cursor {
                Some(cursor) => {
                    if !checked_internal_total {
                        let decoded = decode_cursor(Some(&cursor)).unwrap();
                        assert_eq!(decoded.total_estimate, Some(EXPECTED as i64));
                        assert_eq!(decoded.tantivy_total_hits, None);
                        assert!(decoded.snapshot_id.is_some());
                        checked_internal_total = true;
                    }
                    request.cursor = Some(cursor);
                }
                None => break,
            }
        }
        assert!(checked_internal_total);
        assert_eq!(seen.len(), EXPECTED);
    }

    #[test]
    fn lexical_cursor_carries_the_first_page_total() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..7 {
            insert_download(
                &db,
                &storage,
                &format!("count-cursor-{index}"),
                &format!("総件数カーソル {index}"),
                "作者",
                &["件数"],
                "総件数カーソル本文",
            );
        }

        let mut request = params("総件数カーソル");
        request.limit = Some(2);
        let first = db.search_downloads_v2(&request).unwrap();
        assert_eq!(first.total_estimate, Some(7));
        let decoded = decode_cursor(first.next_cursor.as_deref()).unwrap();
        assert_eq!(decoded.total_estimate, Some(7));
        assert_eq!(decoded.tantivy_total_hits, None);
        assert!(decoded.snapshot_id.is_some());

        request.cursor = first.next_cursor;
        let second = db.search_downloads_v2(&request).unwrap();
        assert_eq!(second.total_estimate, Some(7));
    }

    #[test]
    fn lexical_cursor_survives_equal_score_segment_merge() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const ITEM_COUNT: usize = 137;
        for index in 0..ITEM_COUNT {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("merge-cursor-{index:04}"),
                &format!("同点マージ境界 {index:04}"),
                "同一作者",
                &["マージ"],
                "全作品で同じ語数の同点マージ境界本文",
            );
        }
        while db.get_search_index_status().unwrap().pending_downloads > 0 {
            db.rebuild_search_index_batch(19).unwrap();
        }
        assert!(
            super::super::tantivy_index::searchable_segment_count(&storage).unwrap() > 1,
            "fixture must start with multiple segments"
        );

        let mut request = params("同点マージ境界");
        request.limit = Some(17);
        let first = db.search_downloads_v2(&request).unwrap();
        let mut seen = first
            .items
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>();
        request.cursor = first.next_cursor;

        // DocAddress cursors become invalid here because every segment/doc id
        // can change. The cursor's score + stable download id must not.
        let (_, after) = super::super::tantivy_index::optimize_segments(&storage).unwrap();
        assert_eq!(after, 1);
        while request.cursor.is_some() {
            let page = db.search_downloads_v2(&request).unwrap();
            for item in &page.items {
                assert!(seen.insert(item.id), "duplicate id {}", item.id);
            }
            request.cursor = page.next_cursor;
        }
        assert_eq!(seen.len(), ITEM_COUNT);
    }

    #[test]
    fn ranked_snapshot_failure_rolls_back_and_removes_partial_file() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const MATCHES: usize = 519;
        for index in 0..MATCHES {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("snapshot-fail-{index:04}"),
                &format!("失敗注入検索 {index:04}"),
                "作者",
                &["失敗注入"],
                "失敗注入検索の本文",
            );
        }
        loop {
            let status = db.rebuild_search_index_batch(200).unwrap();
            if status.pending_downloads == 0 {
                break;
            }
        }

        let request = params("失敗注入検索");
        let error = db
            .ranked_search_snapshot_inner(&request, "失敗注入検索", None, Some(1), None)
            .err()
            .expect("the second streamed score batch must fail");
        assert!(error.contains("Injected ranked snapshot stream failure"));
        assert_eq!(
            fs::read_dir(search_snapshot_dir(&storage))
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            0,
            "a failed snapshot must leave neither the database nor sidecars"
        );
        assert!(db.search_snapshot_cache.lock().unwrap().entries.is_empty());

        let quota_error = db
            .ranked_search_snapshot_inner(&request, "失敗注入検索", None, None, Some(4 * 1024))
            .err()
            .expect("the injected disk quota must reject the snapshot");
        assert!(quota_error.contains("shared 4096-byte disk budget"));
        assert_eq!(
            fs::read_dir(search_snapshot_dir(&storage))
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            0,
            "a quota failure must clean its partial snapshot"
        );
    }

    #[test]
    fn ranked_snapshot_cursor_expires_after_library_or_index_generation_changes() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..5 {
            insert_download(
                &db,
                &storage,
                &format!("snapshot-generation-{index}"),
                &format!("世代固定検索 {index}"),
                "作者",
                &["世代"],
                "世代固定検索の本文",
            );
        }
        let mut request = params("世代固定検索");
        request.limit = Some(2);
        let first = db.search_downloads_v2(&request).unwrap();
        let cursor = first.next_cursor.expect("first page cursor");

        insert_download(
            &db,
            &storage,
            "snapshot-generation-new",
            "世代固定検索 新着",
            "作者",
            &["世代"],
            "世代固定検索の本文",
        );
        request.cursor = Some(cursor);
        let error = db.search_downloads_v2(&request).unwrap_err();
        assert!(
            error.contains("expired") || error.contains("invalidated"),
            "a cursor must never continue against a newly ranked generation: {error}"
        );

        request.cursor = None;
        let restarted = db.search_downloads_v2(&request).unwrap();
        assert_eq!(restarted.total_estimate, Some(6));
    }

    #[test]
    fn search_index_optimization_reduces_segments_without_changing_results() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn ordinary_tantivy_writer_is_reused_and_yields_to_bulk_writer() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        insert_download(
            &db,
            &storage,
            "writer-cache-1",
            "writer cache first",
            "author",
            &["writer"],
            "body",
        );
        assert!(
            super::super::tantivy_index::ordinary_writer_is_cached(&storage).unwrap(),
            "a normal save should retain its writer arena"
        );

        let bulk = super::super::tantivy_index::bulk_writer(&storage).unwrap();
        assert!(
            !super::super::tantivy_index::ordinary_writer_is_cached(&storage).unwrap(),
            "the cached writer must release Tantivy's directory lock for a rebuild"
        );
        drop(bulk);

        insert_download(
            &db,
            &storage,
            "writer-cache-2",
            "writer cache second",
            "author",
            &["writer"],
            "body",
        );
        assert!(super::super::tantivy_index::ordinary_writer_is_cached(&storage).unwrap());
    }

    #[test]
    fn entity_facets_search_and_page_beyond_the_dashboard_cap() {
        let (_temp, root, storage) = temp_paths();
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

        let second_page = db.search_entity_facets("person", None, 60, 60, None, None, None, None).unwrap();
        assert_eq!(second_page.len(), 10, "authors past the cap stay reachable");

        let filtered = db
            .search_entity_facets("person", Some("作者69"), 60, 0, None, None, None, None)
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].display_name, "作者69");
        assert_eq!(filtered[0].count, 1);

        let missing = db
            .search_entity_facets("person", Some("存在しない"), 60, 0, None, None, None, None)
            .unwrap();
        assert!(missing.is_empty());

        assert!(db.search_entity_facets("unknown", None, 10, 0, None, None, None, None).is_err());

        // The pager cannot name a last page without this, and a total that does
        // not agree with the rows it is counting is worse than none at all.
        assert_eq!(db.count_entity_facets("person", None, None, None).unwrap(), 70);
        assert_eq!(db.count_entity_facets("person", Some("作者69"), None, None).unwrap(), 1);
        assert_eq!(
            db.count_entity_facets("person", Some("存在しない"), None, None)
                .unwrap(),
            0
        );
        assert!(db.count_entity_facets("unknown", None, None, None).is_err());

        // Walked page by page, the rows add up to exactly what was counted.
        let total = db.count_entity_facets("person", None, None, None).unwrap();
        let mut walked = 0usize;
        for page in 0..(total as usize).div_ceil(20) {
            walked += db
                .search_entity_facets("person", None, 20, (page * 20) as i64, None, None, None, None)
                .unwrap()
                .len();
        }
        assert_eq!(walked as i64, total);
    }

    /// 取得元が黙ったからといって、知っていたことを忘れない。
    ///
    /// 完結の有無は web からしか来ない。保存のついでに作る控え（アプリAPI
    /// だけを見る道）が None を書き戻して消していたら、カードの印が保存の
    /// たびに消えたり点いたりする。
    #[test]
    fn a_silent_source_does_not_erase_what_a_series_already_said() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        db.upsert_series_profile(
            "pixiv",
            "9001",
            "連作",
            Some("説明"),
            Some("series/9001/v1/assets/cover.jpg"),
            "hash-1",
            "series/9001/v1/original.json",
            1,
            64,
            EntityProfileFreshness::RemoteChecked,
            Some(true),
            Some(89),
        )
        .unwrap();
        let stored = db.get_series("pixiv", "9001").unwrap();
        assert_eq!(stored.is_concluded, Some(true));
        assert_eq!(stored.published_content_count, Some(89));

        // 何も聞けなかった更新。表紙も完結も、手元の値が残る。
        db.upsert_series_profile(
            "pixiv",
            "9001",
            "連作",
            Some("説明"),
            None,
            "hash-2",
            "series/9001/v2/original.json",
            0,
            0,
            EntityProfileFreshness::SnapshotOnly,
            None,
            None,
        )
        .unwrap();
        let after = db.get_series("pixiv", "9001").unwrap();
        assert_eq!(after.is_concluded, Some(true), "黙りは「連載中」ではない");
        assert_eq!(after.published_content_count, Some(89));
        assert_eq!(
            after.cover_path.as_deref(),
            Some("series/9001/v1/assets/cover.jpg"),
            "表紙も同じ扱い"
        );

        // 連載が終われば、そう言われたときに変わる。
        db.upsert_series_profile(
            "pixiv",
            "9001",
            "連作",
            Some("説明"),
            None,
            "hash-3",
            "series/9001/v3/original.json",
            0,
            0,
            EntityProfileFreshness::RemoteChecked,
            Some(false),
            Some(90),
        )
        .unwrap();
        let latest = db.get_series("pixiv", "9001").unwrap();
        assert_eq!(latest.is_concluded, Some(false));
        assert_eq!(latest.published_content_count, Some(90));
    }

    /// 並べ替えの決まりそのもの。知らない指定でSQLへ文字列が漏れないことも
    /// ここで固定する。
    #[test]
    fn entity_ordering_only_speaks_words_it_knows() {
        let clause = |by: Option<&str>, order: Option<&str>| {
            Database::entity_facet_order_clause(by, order, "display_name")
        };
        assert_eq!(
            clause(None, None),
            "ORDER BY count DESC, display_name ASC",
            "既定は作品が多い順"
        );
        assert!(clause(Some("downloaded_at"), None).starts_with("ORDER BY latest_downloaded_at DESC"));
        assert!(clause(Some("source_updated_at"), None)
            .starts_with("ORDER BY latest_source_updated_at DESC"));
        assert!(clause(Some("name"), None).starts_with("ORDER BY display_name COLLATE NOCASE ASC"));
        assert!(clause(Some("name"), Some("desc")).starts_with("ORDER BY display_name COLLATE NOCASE DESC"));
        // 知らない指定は既定へ落ちる。文字列はそのままSQLへ入らない。
        let injected = clause(Some("1; DROP TABLE downloads"), Some("desc; --"));
        assert_eq!(injected, "ORDER BY count DESC, display_name ASC");
        assert!(!injected.contains("DROP"));
        // 同じ値が並んだときの順番まで決めておく。ページの境目が揺れる。
        assert!(clause(Some("downloaded_at"), None).contains("count DESC, display_name ASC"));
    }

    /// 一覧そのものにかける条件（監視・作品数・完結）。
    ///
    /// 「配下の作品の条件」と混ざっていないこと、件数（総数）も同じ条件で
    /// 数えられていることを固定する。数が合わないページ送りは、最後の
    /// ページが存在しないという嘘になる。
    #[test]
    fn entity_facets_narrow_by_what_the_listing_itself_is() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        insert_download_unindexed(&db, &storage, "sc-1", "追う人の一", "追う人", &[], "本文");
        insert_download_unindexed(&db, &storage, "sc-2", "追う人の二", "追う人", &[], "本文");
        insert_download_unindexed(&db, &storage, "sc-3", "止めた人の一", "止めた人", &[], "本文");
        insert_download_unindexed(&db, &storage, "sc-4", "未登録の人の一", "未登録の人", &[], "本文");
        {
            let conn = db.conn.lock().unwrap();
            for (id, author) in [("sc-1", "追う人"), ("sc-2", "追う人")] {
                conn.execute(
                    "UPDATE downloads SET author_id = ?1, author_name = ?2 WHERE source_id = ?3",
                    params!["author-watched", author, id],
                )
                .unwrap();
            }
            conn.execute(
                "UPDATE downloads SET author_id = 'author-paused' WHERE source_id = 'sc-3'",
                [],
            )
            .unwrap();
            conn.execute(
                "UPDATE downloads SET author_id = 'author-none' WHERE source_id = 'sc-4'",
                [],
            )
            .unwrap();
        }
        db.upsert_update_target(&UpdateTargetInput {
            target_type: "author".to_string(),
            source: "pixiv".to_string(),
            source_key: "author-watched".to_string(),
            display_name: "追う人".to_string(),
            enabled: true,
            metadata_json: None,
        })
        .unwrap();
        db.upsert_update_target(&UpdateTargetInput {
            target_type: "author".to_string(),
            source: "pixiv".to_string(),
            source_key: "author-paused".to_string(),
            display_name: "止めた人".to_string(),
            enabled: false,
            metadata_json: None,
        })
        .unwrap();

        let names = |scope: EntityFacetScope| {
            let listed = db
                .search_entity_facets("person", None, 60, 0, None, None, None, Some(&scope))
                .unwrap()
                .into_iter()
                .map(|facet| facet.display_name)
                .collect::<Vec<_>>();
            let counted = db
                .count_entity_facets("person", None, None, Some(&scope))
                .unwrap();
            assert_eq!(
                listed.len() as i64,
                counted,
                "総数は、数えている行と一致していなければならない"
            );
            listed
        };

        assert_eq!(
            names(EntityFacetScope { watch: Some("watched".into()), ..Default::default() }),
            vec!["追う人"]
        );
        assert_eq!(
            names(EntityFacetScope { watch: Some("paused".into()), ..Default::default() }),
            vec!["止めた人"]
        );
        let mut unwatched = names(EntityFacetScope { watch: Some("unwatched".into()), ..Default::default() });
        unwatched.sort();
        assert_eq!(unwatched, vec!["未登録の人"]);
        // 知らない言葉は条件なしとして扱う。
        assert_eq!(
            names(EntityFacetScope { watch: Some("いつか".into()), ..Default::default() }).len(),
            3
        );
        // 作品数の下限。1以下は条件なしと同じ。
        assert_eq!(
            names(EntityFacetScope { min_work_count: Some(2), ..Default::default() }),
            vec!["追う人"]
        );
        assert_eq!(
            names(EntityFacetScope { min_work_count: Some(1), ..Default::default() }).len(),
            3
        );
        // 監視と作品数は同時に効く。
        assert_eq!(
            names(EntityFacetScope {
                watch: Some("unwatched".into()),
                min_work_count: Some(2),
                ..Default::default()
            })
            .len(),
            0
        );
    }

    /// 完結の有無で絞る。まだ聞いていないシリーズは、どちらにも入らない。
    #[test]
    fn series_facets_narrow_by_whether_the_source_called_it_finished() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for (index, (source_id, series_key, title)) in [
            ("cs-1", "done", "完結した連作"),
            ("cs-2", "running", "続いている連作"),
            ("cs-3", "unknown", "まだ聞いていない連作"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = insert_download_unindexed(
                &db,
                &storage,
                source_id,
                &format!("{title}#{index}"),
                "連載作者",
                &[],
                "本文",
            );
            db.upsert_download_series(id, "pixiv", series_key, title, Some(1))
                .unwrap();
        }
        for (key, title, concluded) in [
            ("done", "完結した連作", Some(true)),
            ("running", "続いている連作", Some(false)),
            ("unknown", "まだ聞いていない連作", None),
        ] {
            db.upsert_series_profile(
                "pixiv",
                key,
                title,
                None,
                None,
                &format!("hash-{key}"),
                &format!("series/{key}/v1/original.json"),
                0,
                0,
                EntityProfileFreshness::RemoteChecked,
                concluded,
                None,
            )
            .unwrap();
        }
        let names = |concluded: Option<bool>| {
            let scope = EntityFacetScope { concluded, ..Default::default() };
            let listed = db
                .search_entity_facets("series", None, 60, 0, None, Some("name"), None, Some(&scope))
                .unwrap()
                .into_iter()
                .map(|facet| facet.display_name)
                .collect::<Vec<_>>();
            assert_eq!(
                listed.len() as i64,
                db.count_entity_facets("series", None, None, Some(&scope)).unwrap()
            );
            listed
        };
        assert_eq!(names(Some(true)), vec!["完結した連作"]);
        assert_eq!(names(Some(false)), vec!["続いている連作"]);
        assert_eq!(names(None).len(), 3, "指定が無ければ全部");

        // 完結の印は一覧まで届く。
        let all = db
            .search_entity_facets("series", None, 60, 0, None, Some("name"), None, None)
            .unwrap();
        let finished = all.iter().find(|f| f.source_key == "done").unwrap();
        let unknown = all.iter().find(|f| f.source_key == "unknown").unwrap();
        assert_eq!(finished.is_concluded, Some(true));
        assert_eq!(unknown.is_concluded, None);
    }

    /// 作者・シリーズの並びは、その中にある作品を見て決まる。
    #[test]
    fn entity_facets_sort_by_the_works_underneath() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let a1 = insert_download_unindexed(&db, &storage, "sort-a1", "Aの一", "作者A", &[], "本文");
        let a2 = insert_download_unindexed(&db, &storage, "sort-a2", "Aの二", "作者A", &[], "本文");
        let b1 = insert_download_unindexed(&db, &storage, "sort-b1", "Bの一", "作者B", &[], "本文");
        {
            // 作者Aは作品が多く、保存は古く、取得元での更新は新しい。
            // 作者Bはその逆。どの鍵で並べたかが順番に出る。
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE downloads SET author_id = 'author-A', author_name = '作者A',
                    downloaded_at = '2026-01-01T00:00:00Z',
                    source_updated_at = '2026-08-01T00:00:00Z'
                 WHERE id IN (?1, ?2)",
                params![a1, a2],
            )
            .unwrap();
            conn.execute(
                "UPDATE downloads SET author_id = 'author-B', author_name = '作者B',
                    downloaded_at = '2026-08-20T00:00:00Z',
                    source_updated_at = '2026-01-05T00:00:00Z'
                 WHERE id = ?1",
                params![b1],
            )
            .unwrap();
        }
        let names = |by: Option<&str>| {
            db.search_entity_facets("person", None, 60, 0, None, by, None, None)
                .unwrap()
                .into_iter()
                .map(|facet| facet.display_name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(None), vec!["作者A", "作者B"], "作品が多い順");
        assert_eq!(names(Some("downloaded_at")), vec!["作者B", "作者A"], "保存が新しい順");
        assert_eq!(
            names(Some("source_updated_at")),
            vec!["作者A", "作者B"],
            "取得元での更新が新しい順"
        );
        assert_eq!(names(Some("name")), vec!["作者A", "作者B"], "名前順");
    }

    #[test]
    fn entity_facets_narrow_to_the_library_filters() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        // 作者Aは絞り込みに合う作品と合わない作品を持ち、作者Bは合う作品を
        // 持たない。絞り込みが効いていれば、Aだけが残り、その件数も1になる。
        let favourite =
            insert_download_unindexed(&db, &storage, "301", "日常の話", "作者A", &["日常"], "本文");
        let other =
            insert_download_unindexed(&db, &storage, "302", "冒険の話", "作者A", &["冒険"], "本文");
        insert_download_unindexed(&db, &storage, "303", "冒険の続き", "作者B", &["冒険"], "本文");
        // 作品ごとに別の作者IDを振る補助関数なので、この2件を同じ人物にまとめる。
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE downloads SET author_id = 'author-A' WHERE id IN (?1, ?2)",
                rusqlite::params![favourite, other],
            )
            .unwrap();
        }
        db.set_favorite(favourite, true).unwrap();

        let unfiltered = db.search_entity_facets("person", None, 60, 0, None, None, None, None).unwrap();
        assert_eq!(unfiltered.len(), 2);
        assert_eq!(db.count_entity_facets("person", None, None, None).unwrap(), 2);

        let by_tag = SearchV2Params {
            tags_include: Some(vec!["日常".to_string()]),
            ..params("")
        };
        let tagged = db
            .search_entity_facets("person", None, 60, 0, Some(&by_tag), None, None, None)
            .unwrap();
        assert_eq!(tagged.len(), 1, "作者Bはこのタグの作品を持たない");
        assert_eq!(tagged[0].display_name, "作者A");
        // 件数は「条件に合う作品の数」。作者Aの2件すべてではない。
        assert_eq!(tagged[0].count, 1);
        assert_eq!(
            db.count_entity_facets("person", None, Some(&by_tag), None).unwrap(),
            1,
            "総数は数えている行と一致する"
        );

        // 名前での絞り込みと同時に効く。作者Aは条件に合うが、名前が違う。
        let named = db
            .search_entity_facets("person", Some("作者B"), 60, 0, Some(&by_tag), None, None, None)
            .unwrap();
        assert!(named.is_empty());

        let favourites_only = SearchV2Params {
            favorite: Some(true),
            ..params("")
        };
        let favoured = db
            .search_entity_facets("person", None, 60, 0, Some(&favourites_only), None, None, None)
            .unwrap();
        assert_eq!(favoured.len(), 1);
        assert_eq!(favoured[0].display_name, "作者A");

        // 同じキャッシュを別の条件で引かない。
        let unfiltered_again = db.search_entity_facets("person", None, 60, 0, None, None, None, None).unwrap();
        assert_eq!(unfiltered_again.len(), 2);
    }

    #[test]
    fn entity_key_pages_include_orphans_without_gaps() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            for (source, key) in [("fanbox", "p3"), ("pixiv", "p1"), ("pixiv", "p2")] {
                conn.execute(
                    "INSERT INTO people (source, source_key, display_name)
                     VALUES (?1, ?2, ?2)",
                    params![source, key],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO series (source, source_key, title)
                     VALUES (?1, ?2, ?2)",
                    params![source, key],
                )
                .unwrap();
            }
        }

        let people_first = db.list_people_keys_after(None, 2).unwrap();
        assert_eq!(
            people_first,
            vec![
                ("fanbox".to_string(), "p3".to_string()),
                ("pixiv".to_string(), "p1".to_string())
            ]
        );
        let people_second = db
            .list_people_keys_after(Some((&people_first[1].0, &people_first[1].1)), 2)
            .unwrap();
        assert_eq!(people_second, vec![("pixiv".to_string(), "p2".to_string())]);

        let series_first = db.list_series_keys_after(None, 2).unwrap();
        let series_second = db
            .list_series_keys_after(Some((&series_first[1].0, &series_first[1].1)), 2)
            .unwrap();
        assert_eq!(series_first.len() + series_second.len(), 3);
    }

    #[test]
    fn smart_search_ranks_metadata_over_body_and_supports_body_search() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn search_v2_uses_cursor_without_duplicate_pages() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn search_suggest_returns_metadata_candidates() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn exact_author_intent_excludes_other_authors_that_only_mention_the_name() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn series_token_filters_by_series_relation() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn japanese_reading_kana_and_romaji_match_same_work() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn smart_search_does_not_add_semantic_reasons() {
        let (_temp, root, storage) = temp_paths();
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
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn index_status_is_cached_without_going_stale_across_changes() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        assert_eq!(db.get_search_index_status().unwrap().total_downloads, 0);

        // Reading it again immediately is served from the cache, and adding a
        // work has to drop that cache: a count the screens keep showing after
        // the library changed is worse than measuring it again.
        insert_download_unindexed(
            &db,
            &storage,
            "cache-1",
            "追加された作品",
            "作者",
            &["cache"],
            "本文",
        );
        let after_insert = db.get_search_index_status().unwrap();
        assert_eq!(after_insert.total_downloads, 1);
        assert_eq!(after_insert.pending_downloads, 1);

        db.reindex_download(
            db.get_download_by_source("pixiv", "cache-1")
                .unwrap()
                .unwrap()
                .id,
        )
        .unwrap();
        let after_index = db.get_search_index_status().unwrap();
        assert_eq!(after_index.pending_downloads, 0);
        assert!(after_index.is_complete);
    }

    #[test]
    fn semantic_completion_requires_every_current_document() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download_unindexed(
            &db,
            &storage,
            "coverage-1",
            "索引済み作品",
            "作者",
            &[],
            "最初の本文",
        );
        let second = insert_download_unindexed(
            &db,
            &storage,
            "coverage-2",
            "未索引作品",
            "作者",
            &[],
            "次の本文",
        );

        db.reindex_download(first).unwrap();
        let partial = db.get_search_index_status().unwrap();
        assert_eq!(partial.semantic_indexed_downloads, 1);
        assert_eq!(partial.semantic_pending_downloads, 1);

        db.reindex_download(second).unwrap();
        let complete = db.get_search_index_status().unwrap();
        assert_eq!(complete.semantic_indexed_downloads, 2);
        assert_eq!(complete.semantic_pending_downloads, 0);
    }

    #[test]
    fn an_author_page_can_show_their_series_and_tags_and_drill_into_both() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let first = insert_download(
            &db,
            &storage,
            "auth-1",
            "序章",
            "青葉しおり",
            &["ファンタジー", "長編"],
            "本文",
        );
        let second = insert_download(
            &db,
            &storage,
            "auth-2",
            "第二話",
            "青葉しおり",
            &["ファンタジー"],
            "本文",
        );
        let standalone = insert_download(
            &db,
            &storage,
            "auth-3",
            "読み切り",
            "青葉しおり",
            &["短編"],
            "本文",
        );
        let other = insert_download(
            &db,
            &storage,
            "auth-4",
            "別作者の話",
            "別の人",
            &["ファンタジー"],
            "本文",
        );
        db.upsert_download_person(first, "pixiv", "aoba", "author", "青葉しおり")
            .unwrap();
        db.upsert_download_person(second, "pixiv", "aoba", "author", "青葉しおり")
            .unwrap();
        db.upsert_download_person(standalone, "pixiv", "aoba", "author", "青葉しおり")
            .unwrap();
        db.upsert_download_person(other, "pixiv", "hoka", "author", "別の人")
            .unwrap();
        db.upsert_download_series(first, "pixiv", "s1", "季節の栞", Some(1))
            .unwrap();
        db.upsert_download_series(second, "pixiv", "s1", "季節の栞", Some(2))
            .unwrap();

        let series = db.list_entity_series("pixiv", "aoba", 20).unwrap();
        assert_eq!(series.len(), 1, "only the series this author appears in");
        assert_eq!(series[0].display_name, "季節の栞");
        assert_eq!(series[0].count, 2);

        let tags = db.list_entity_tags("person", "pixiv", "aoba", 20).unwrap();
        let named = tags
            .iter()
            .map(|t| (t.name.as_str(), t.count))
            .collect::<Vec<_>>();
        assert_eq!(
            named,
            vec![("ファンタジー", 2), ("短編", 1), ("長編", 1)],
            "the author's own tags, most used first, without the other author's"
        );

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
        assert!(
            !ids.contains(&other),
            "another author's work with the same tag must not appear"
        );

        // An author with nothing recorded is an empty page, not an error.
        assert!(db
            .list_entity_series("pixiv", "unknown", 20)
            .unwrap()
            .is_empty());
        assert!(db
            .list_entity_tags("person", "pixiv", "unknown", 20)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn entity_series_keyset_has_no_gaps_duplicates_or_same_name_instability() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        assert!(db
            .list_entity_series_paged("", "author", None, 20, None)
            .is_err());
        assert!(db
            .list_entity_series_paged(&"s".repeat(65), "author", None, 20, None)
            .is_err());
        assert!(db
            .list_entity_series_paged("pixiv", &"k".repeat(1_025), None, 20, None)
            .is_err());
        assert!(db
            .list_entity_series_paged("pixiv", "author", Some(&"検".repeat(201)), 20, None,)
            .is_err());
        assert!(db
            .list_entity_series_paged(
                "pixiv",
                "author",
                None,
                20,
                Some(&"c".repeat(64 * 1024 + 1)),
            )
            .is_err());
        assert!(db
            .list_entity_series_paged("pixiv", "author", None, 20, Some("malformed"))
            .is_err());

        let add = |source_id: &str,
                   series_source: &str,
                   series_key: &str,
                   title: &str,
                   created_at: &str| {
            let id = insert_download_unindexed(
                &db,
                &storage,
                source_id,
                source_id,
                "ページ作者",
                &[],
                "本文",
            );
            db.upsert_download_person(id, "pixiv", "paged-author", "author", "ページ作者")
                .unwrap();
            db.upsert_download_series(id, series_source, series_key, title, Some(1))
                .unwrap();
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE downloads
                     SET downloaded_at = ?1, source_created_at = ?1
                     WHERE id = ?2",
                    params![created_at, id],
                )
                .unwrap();
            id
        };

        add(
            "count-a",
            "pixiv",
            "counted",
            "最多",
            "2026-05-01T00:00:00Z",
        );
        add(
            "count-b",
            "pixiv",
            "counted",
            "最多",
            "2026-05-01T00:00:00Z",
        );
        add("alpha", "pixiv", "alpha", "Alpha", "2026-05-01T00:00:00Z");
        add("tie-f", "fanbox", "same", "同名", "2026-05-01T00:00:00Z");
        add("tie-p1", "pixiv", "same-1", "同名", "2026-05-01T00:00:00Z");
        add("tie-p2", "pixiv", "same-2", "同名", "2026-05-01T00:00:00Z");
        add("older", "pixiv", "older", "旧作", "2025-01-01T00:00:00Z");

        let mut cursor = None;
        let mut identities = Vec::new();
        let mut first_cursor = None;
        loop {
            let page = db
                .list_entity_series_paged("pixiv", "paged-author", None, 2, cursor.as_deref())
                .unwrap();
            assert_eq!(page.total, 6);
            identities.extend(
                page.items
                    .iter()
                    .map(|item| (item.source.clone(), item.source_key.clone())),
            );
            if first_cursor.is_none() {
                first_cursor = page.next_cursor.clone();
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            identities,
            vec![
                ("pixiv".to_string(), "counted".to_string()),
                ("pixiv".to_string(), "alpha".to_string()),
                ("fanbox".to_string(), "same".to_string()),
                ("pixiv".to_string(), "same-1".to_string()),
                ("pixiv".to_string(), "same-2".to_string()),
                ("pixiv".to_string(), "older".to_string()),
            ]
        );
        assert_eq!(identities.iter().collect::<HashSet<_>>().len(), 6);

        let filtered = db
            .list_entity_series_paged("pixiv", "paged-author", Some("同名"), 2, None)
            .unwrap();
        assert_eq!(filtered.total, 3);
        assert_eq!(filtered.items.len(), 2);
        assert!(db
            .list_entity_series_paged(
                "pixiv",
                "paged-author",
                Some("別query"),
                2,
                first_cursor.as_deref(),
            )
            .is_err());

        add("mutation", "pixiv", "new", "追加", "2027-01-01T00:00:00Z");
        assert!(db
            .list_entity_series_paged("pixiv", "paged-author", None, 2, first_cursor.as_deref(),)
            .is_err());
    }

    #[test]
    #[ignore = "performance smoke test for a prolific author's series"]
    fn entity_series_paging_stays_fast_at_twenty_thousand() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let seeded = std::env::var("PIEP_BENCH_ENTITY_SERIES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20_000);
        {
            let mut conn = db.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            {
                let mut download = tx
                    .prepare(
                        "INSERT INTO downloads (
                            source, source_id, title, author_name, author_id,
                            content_type, json_path, downloaded_at, source_created_at
                         ) VALUES ('pixiv', ?1, ?2, '大量作者', 'large-author',
                                   'novel', 'unused.json', ?3, ?3)",
                    )
                    .unwrap();
                let mut person = tx
                    .prepare(
                        "INSERT INTO download_people (
                            download_id, person_source, person_key, role, display_name
                         ) VALUES (?1, 'pixiv', 'large-author', 'author', '大量作者')",
                    )
                    .unwrap();
                let mut series = tx
                    .prepare(
                        "INSERT INTO series (source, source_key, title)
                         VALUES ('pixiv', ?1, ?2)",
                    )
                    .unwrap();
                let mut relation = tx
                    .prepare(
                        "INSERT INTO download_series (
                            download_id, series_source, series_key, title, content_order
                         ) VALUES (?1, 'pixiv', ?2, ?3, 1)",
                    )
                    .unwrap();
                for index in 0..seeded {
                    let key = format!("series-{index:05}");
                    let title = format!("シリーズ {index:05}");
                    let timestamp =
                        format!("2026-{:02}-{:02}T00:00:00Z", index % 12 + 1, index % 27 + 1);
                    download.execute(params![key, title, timestamp]).unwrap();
                    let id = tx.last_insert_rowid();
                    person.execute(params![id]).unwrap();
                    series.execute(params![key, title]).unwrap();
                    relation.execute(params![id, key, title]).unwrap();
                }
            }
            tx.commit().unwrap();
        }

        let started = Instant::now();
        let first = db
            .list_entity_series_paged("pixiv", "large-author", None, 200, None)
            .unwrap();
        let first_elapsed = started.elapsed();
        assert_eq!(first.total, seeded as i64);
        assert_eq!(first.items.len(), seeded.min(200));

        let mut cursor = first.next_cursor;
        let mut deepest = Duration::ZERO;
        for _ in 0..50 {
            let Some(current) = cursor else { break };
            let started = Instant::now();
            let page = db
                .list_entity_series_paged("pixiv", "large-author", None, 200, Some(&current))
                .unwrap();
            deepest = deepest.max(started.elapsed());
            cursor = page.next_cursor;
        }
        eprintln!(
            "{seeded} entity series: first {:?}, deepest {:?}",
            first_elapsed, deepest
        );
        assert!(first_elapsed < Duration::from_secs(1));
        assert!(deepest < Duration::from_secs(1));
    }

    #[test]
    fn numbered_pages_work_for_an_ordering_and_are_refused_for_relevance() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        for index in 0..7 {
            insert_download(
                &db,
                &storage,
                &format!("page-{index}"),
                &format!("作品{index}"),
                "作者",
                &["頁"],
                "本文",
            );
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
        assert_eq!(
            page_of(3),
            vec!["作品3", "作品4", "作品5"],
            "the third page, without walking to it"
        );
        assert_eq!(page_of(6), vec!["作品6"]);
        assert!(
            page_of(99).is_empty(),
            "past the end is empty, not an error"
        );

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
    }

    #[test]
    fn shelf_counts_ignore_reading_positions_for_works_that_no_longer_exist() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let first = insert_download(&db, &storage, "shelf-1", "一作目", "作者", &["棚"], "本文");
        let second = insert_download(&db, &storage, "shelf-2", "二作目", "作者", &["棚"], "本文");
        insert_download(&db, &storage, "shelf-3", "三作目", "作者", &["棚"], "本文");
        db.set_favorite(first, true).unwrap();
        db.set_watch_updates(second, true).unwrap();

        let empty = db.get_library_shelf_counts(&[]).unwrap();
        assert_eq!(
            (empty.total, empty.favorite, empty.watched, empty.reading),
            (3, 1, 1, 0)
        );

        // Reading positions are per device and nothing prunes them, so a shelf
        // must count works that exist, not entries that were left behind.
        let counts = db
            .get_library_shelf_counts(&[first, second, 999_999, first])
            .unwrap();
        assert_eq!(
            counts.reading, 2,
            "a deleted work and a duplicate must not be counted"
        );

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
    }

    #[test]
    fn saved_searches_survive_reuse_of_a_name_and_reject_nonsense() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let created = db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "  長編ファンタジー  ".to_string(),
                query: Some("tag:ファンタジー".to_string()),
                params_json: "{\"tagsInclude\":[\"ファンタジー\"]}".to_string(),
            })
            .unwrap();
        assert_eq!(
            created.name, "長編ファンタジー",
            "the name is stored trimmed"
        );

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

        assert!(
            db.upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "   ".to_string(),
                query: None,
                params_json: "{}".to_string(),
            })
            .is_err(),
            "a blank name is not a search anyone can find again"
        );
        assert!(
            db.upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "壊れた条件".to_string(),
                query: None,
                params_json: "{not json".to_string(),
            })
            .is_err(),
            "conditions that cannot be read back must not be stored"
        );
        assert!(
            db.upsert_saved_search(&SavedSearchInput {
                id: Some(4_242),
                name: "存在しない".to_string(),
                query: None,
                params_json: "{}".to_string(),
            })
            .is_err(),
            "updating a search that was deleted elsewhere must say so"
        );

        assert!(db.delete_saved_search(created.id).unwrap());
        assert!(
            !db.delete_saved_search(created.id).unwrap(),
            "a second delete is not an error"
        );
        assert!(db.list_saved_searches().unwrap().is_empty());
    }

    #[test]
    fn saved_searches_keep_their_order_and_stop_at_the_limit() {
        let (_temp, root, storage) = temp_paths();
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
        assert_eq!(
            listed[0].name, "検索000",
            "the sidebar order must be stable"
        );
        assert_eq!(listed[listed.len() - 1].name, "検索099");

        let overflow = db.upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "一件多い".to_string(),
            query: None,
            params_json: "{}".to_string(),
        });
        assert!(
            overflow.is_err(),
            "the limit must be refused, not silently applied"
        );
        // Replacing an existing one still works when the list is full.
        assert!(db
            .upsert_saved_search(&SavedSearchInput {
                id: None,
                name: "検索000".to_string(),
                query: Some("更新".to_string()),
                params_json: "{}".to_string(),
            })
            .is_ok());
    }

    #[test]
    fn synonyms_do_not_match_every_work_through_its_source_url() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        // Every pixiv URL is .../novel/show.php?id=N, and the synonym table
        // expands 物語 to "novel". Neither of these works mentions 物語.
        insert_download(
            &db,
            &storage,
            "111",
            "海辺の記録",
            "作者A",
            &["日常"],
            "静かな本文です",
        );
        insert_download(
            &db,
            &storage,
            "222",
            "灯台の手紙",
            "作者B",
            &["日常"],
            "別の本文です",
        );
        let about = insert_download(
            &db,
            &storage,
            "333",
            "夜明けの物語",
            "作者C",
            &["創作"],
            "本文",
        );

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
    }

    #[test]
    fn changing_the_index_format_requeues_the_whole_library() {
        let (_temp, root, storage) = temp_paths();
        let db_path = root.join("piep.db");
        let db = Database::open(&db_path, &storage).unwrap();
        insert_download(
            &db,
            &storage,
            "fmt-1",
            "書式変更の作品",
            "作者",
            &["書式"],
            "本文",
        );
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
    }

    #[test]
    fn sorted_search_orders_matches_by_column_and_pages_without_gaps() {
        let (_temp, root, storage) = temp_paths();
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
            insert_download(
                &db,
                &storage,
                source_id,
                title,
                "並び替え作者",
                &["並替"],
                "共通の本文です",
            );
        }
        // A work that must never appear: it does not match the query.
        insert_download(
            &db,
            &storage,
            "sorted-x",
            "無関係な作品",
            "別作者",
            &["他"],
            "別の本文",
        );

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
    }

    #[test]
    fn sorted_search_respects_library_filters() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let favorite = insert_download(
            &db,
            &storage,
            "filter-1",
            "対象作品 花暦",
            "作者",
            &["絞込"],
            "本文",
        );
        insert_download(
            &db,
            &storage,
            "filter-2",
            "対象外作品 花暦",
            "作者",
            &["絞込"],
            "本文",
        );
        db.set_favorite(favorite, true).unwrap();

        let mut sorted = params("花暦");
        sorted.sort_by = Some("downloaded_at".to_string());
        sorted.favorite = Some(true);
        let page = db.search_downloads_v2(&sorted).unwrap();
        assert_eq!(page.total_estimate, Some(1));
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, favorite);
    }

    #[test]
    fn sorted_search_uses_bounded_temp_table_batches_and_carries_total() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        const MATCHES: usize = SORTED_SEARCH_ID_BATCH_SIZE * 2 + 17;
        for index in 0..MATCHES {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("streamed-{index:04}"),
                &format!("流し込み検索 {index:04}"),
                "作者",
                &["一時表"],
                "流し込み検索の本文",
            );
        }
        loop {
            let status = db.rebuild_search_index_batch(200).unwrap();
            if status.pending_downloads == 0 {
                break;
            }
        }
        super::super::tantivy_index::optimize_segments(&storage).unwrap();

        let mut request = params("流し込み検索");
        request.sort_by = Some("title".to_string());
        request.sort_order = Some("asc".to_string());
        request.limit = Some(31);
        let first = db.search_downloads_v2(&request).unwrap();
        assert_eq!(first.total_estimate, Some(MATCHES as i64));
        assert_eq!(first.items.len(), 31);
        let (_, _, cache_after_first) = db.query_cache_stats();

        let cursor = decode_cursor(first.next_cursor.as_deref()).unwrap();
        assert_eq!(cursor.total_estimate, Some(MATCHES as i64));
        request.cursor = first.next_cursor;
        let second = db.search_downloads_v2(&request).unwrap();
        assert_eq!(second.total_estimate, Some(MATCHES as i64));
        assert!(first
            .items
            .iter()
            .all(|left| second.items.iter().all(|right| left.id != right.id)));
        let (_, _, cache_after_second) = db.query_cache_stats();
        assert!(cache_after_second.0 > cache_after_first.0);

        insert_download(
            &db,
            &storage,
            "streamed-new",
            "流し込み検索 新着",
            "作者",
            &["一時表"],
            "流し込み検索の本文",
        );
        request.cursor = None;
        let after_insert = db.search_downloads_v2(&request).unwrap();
        assert_eq!(after_insert.total_estimate, Some(MATCHES as i64 + 1));
        let (_, _, cache_after_insert) = db.query_cache_stats();
        assert!(
            cache_after_insert.1 > cache_after_second.1,
            "library/index generation change must rebuild the snapshot"
        );
    }

    #[test]
    fn multi_term_search_requires_each_term() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn semantic_mode_returns_body_chunk_highlight_and_reason() {
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    #[ignore = "performance smoke test for large-library browsing"]
    fn library_browsing_stays_fast_on_a_large_library() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let seeded: usize = std::env::var("PIEP_BENCH_WORKS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(20_000);
        let metadata_only =
            seeded >= 250_000 || std::env::var("PIEP_BENCH_METADATA_ONLY").as_deref() == Ok("1");
        let seed_started = Instant::now();
        if metadata_only {
            seed_metadata_only_library(&db, seeded);
        } else {
            for index in 0..seeded {
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
        }
        let seed_elapsed = seed_started.elapsed();
        let database_bytes = fs::metadata(root.join("piep.db"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        // The library browses with an empty query, which is the keyset path.
        let mut first = v2_params(None, 60, None);
        first.projection = Some("libraryGallery".to_string());
        let started = Instant::now();
        let page1 = db.search_downloads_v2(&first).unwrap();
        let first_elapsed = started.elapsed();
        assert_eq!(page1.items.len(), 60);
        assert_eq!(page1.total_estimate, Some(seeded as i64));
        assert!(
            page1.next_cursor.is_some(),
            "a large library must expose more pages"
        );
        let started = Instant::now();
        let warm_page1 = db.search_downloads_v2(&first).unwrap();
        let warm_first_elapsed = started.elapsed();
        assert_eq!(warm_page1.items.len(), page1.items.len());

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
        let authors = db.search_entity_facets("person", None, 60, 300, None, None, None, None).unwrap();
        let entity_elapsed = started.elapsed();
        assert_eq!(authors.len(), 60);
        let started = Instant::now();
        let warm_authors = db.search_entity_facets("person", None, 60, 300, None, None, None, None).unwrap();
        let warm_entity_elapsed = started.elapsed();
        assert_eq!(warm_authors.len(), authors.len());

        let started = Instant::now();
        let facets = db.get_filter_facets_with(false).unwrap();
        let facet_elapsed = started.elapsed();
        assert!(!facets.tags.is_empty());
        assert!(
            facets.author_entities.is_empty(),
            "the light variant must skip the entity aggregates"
        );

        let started = Instant::now();
        let cached_facets = db.get_filter_facets_with(false).unwrap();
        let cached_facet_elapsed = started.elapsed();
        assert_eq!(
            cached_facets
                .tags
                .iter()
                .map(|facet| (&facet.name, facet.count))
                .collect::<Vec<_>>(),
            facets
                .tags
                .iter()
                .map(|facet| (&facet.name, facet.count))
                .collect::<Vec<_>>()
        );

        let suggest_params = SearchSuggestParams {
            text: Some("蔵書".to_string()),
            limit: Some(12),
        };
        let started = Instant::now();
        let suggestions = db.search_suggest(&suggest_params).unwrap();
        let suggest_elapsed = started.elapsed();
        assert_eq!(suggestions.items.len(), 12);
        let started = Instant::now();
        let cached_suggestions = db.search_suggest(&suggest_params).unwrap();
        let cached_suggest_elapsed = started.elapsed();
        assert_eq!(cached_suggestions.items.len(), suggestions.items.len());

        eprintln!(
            "{} works (metadata_only={}, seed {:?}, db {} MiB): first page cold {:?}/warm {:?}, deepest page {:?}, authors cold {:?}/warm {:?}, filter options cold {:?}/warm {:?}, suggestions cold {:?}/warm {:?}",
            seeded,
            metadata_only,
            seed_elapsed,
            database_bytes / (1024 * 1024),
            first_elapsed,
            warm_first_elapsed,
            deepest,
            entity_elapsed,
            warm_entity_elapsed,
            facet_elapsed,
            cached_facet_elapsed,
            suggest_elapsed,
            cached_suggest_elapsed
        );
        let first_page_budget = if seeded >= 500_000 {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(400)
        };
        assert!(
            first_elapsed < first_page_budget,
            "first page took {:?}",
            first_elapsed
        );
        assert!(
            warm_first_elapsed < Duration::from_millis(400),
            "warm first page took {:?}",
            warm_first_elapsed
        );
        assert!(
            deepest < Duration::from_millis(400),
            "deep page took {:?}",
            deepest
        );
        let aggregate_budget = if seeded >= 500_000 {
            Duration::from_secs(3)
        } else {
            Duration::from_millis(500)
        };
        // Without idx_downloads_author_recent this listing takes ~1.9s here.
        assert!(
            entity_elapsed < aggregate_budget,
            "author listing took {:?}",
            entity_elapsed
        );
        assert!(
            warm_entity_elapsed < Duration::from_millis(100),
            "cached author listing took {:?}",
            warm_entity_elapsed
        );
        assert!(
            facet_elapsed < aggregate_budget,
            "filter options took {:?}",
            facet_elapsed
        );
        assert!(
            suggest_elapsed < aggregate_budget,
            "cold suggestions took {:?}",
            suggest_elapsed
        );
        assert!(
            cached_facet_elapsed < Duration::from_millis(50),
            "cached filter options took {:?}",
            cached_facet_elapsed
        );
        assert!(
            cached_suggest_elapsed < Duration::from_millis(50),
            "cached suggestions took {:?}",
            cached_suggest_elapsed
        );

        drop(db);
    }

    #[test]
    #[ignore = "measurement harness for disk-backed lexical snapshots"]
    fn lexical_snapshots_scale_with_fixed_memory_batches() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let works: usize = std::env::var("PIEP_BENCH_WORKS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(100_000);
        assert!((1_000..=1_000_000).contains(&works));

        let seed_started = Instant::now();
        seed_metadata_only_library(&db, works);
        let seed_elapsed = seed_started.elapsed();
        let index_started = Instant::now();
        let mut writer = super::super::tantivy_index::bulk_writer(&storage).unwrap();
        for index in 0..works {
            let prepared = super::super::tantivy_index::prepare_document(
                &storage,
                &super::super::tantivy_index::TantivyIndexDocument {
                    download_id: index as i64 + 1,
                    source: "pixiv".to_string(),
                    source_id: format!("scale-{index:07}"),
                    source_url: format!("https://example.invalid/{index}"),
                    title: format!("snapshotterm work {index:07}"),
                    author_name: format!("author {:03}", index % 400),
                    author_id: format!("author-{:03}", index % 400),
                    tags: format!("tag{}", index % 30),
                    series_title: String::new(),
                    excerpt: "snapshotterm benchmark".to_string(),
                    body: "snapshotterm bounded disk ranking".to_string(),
                    published_at: "2026-01-01T00:00:00Z".to_string(),
                    downloaded_at: "2026-01-01T00:00:00Z".to_string(),
                    favorite: false,
                    watch_updates: false,
                    asset_kinds: String::new(),
                    text_length: 36,
                },
            )
            .unwrap();
            writer.upsert(prepared).unwrap();
            if writer.uncommitted() >= 20_000 {
                writer.commit().unwrap();
            }
        }
        writer.commit().unwrap();
        drop(writer);
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO search_index_state (
                    download_id, current_version, content_hash, indexed_at
                 )
                 SELECT id, current_version, content_hash, CURRENT_TIMESTAMP FROM downloads",
                [],
            )
            .unwrap();
        }
        let index_elapsed = index_started.elapsed();

        let mut ranked = params("snapshotterm");
        ranked.limit = Some(60);
        ranked.projection = Some("bulk".to_string());
        let started = Instant::now();
        let first = db.search_downloads_v2(&ranked).unwrap();
        let ranked_cold = started.elapsed();
        assert_eq!(first.total_estimate, Some(works as i64));
        ranked.cursor = first.next_cursor;
        let started = Instant::now();
        let second = db.search_downloads_v2(&ranked).unwrap();
        let ranked_warm = started.elapsed();
        assert_eq!(second.items.len(), 60);

        let mut sorted = params("snapshotterm");
        sorted.limit = Some(60);
        sorted.projection = Some("bulk".to_string());
        sorted.sort_by = Some("title".to_string());
        sorted.sort_order = Some("asc".to_string());
        let started = Instant::now();
        let first_sorted = db.search_downloads_v2(&sorted).unwrap();
        let sorted_cold = started.elapsed();
        assert_eq!(first_sorted.total_estimate, Some(works as i64));
        sorted.cursor = first_sorted.next_cursor;
        let started = Instant::now();
        let second_sorted = db.search_downloads_v2(&sorted).unwrap();
        let sorted_warm = started.elapsed();
        assert_eq!(second_sorted.items.len(), 60);

        let cache = db.search_snapshot_cache.lock().unwrap();
        let snapshot_bytes = cache.disk_bytes;
        let snapshot_count = cache.entries.len();
        drop(cache);
        eprintln!(
            "{works} lexical matches: seed {seed_elapsed:?}, index {index_elapsed:?}, ranked cold/warm {ranked_cold:?}/{ranked_warm:?}, sorted cold/warm {sorted_cold:?}/{sorted_warm:?}, {snapshot_count} snapshots {} MiB",
            snapshot_bytes / (1024 * 1024)
        );
        assert!(snapshot_count >= 2);
        assert!(snapshot_bytes <= super::super::resource_budget::search_snapshot_disk_bytes());
        assert!(ranked_warm < Duration::from_millis(250));
        assert!(sorted_warm < Duration::from_millis(250));

        drop(db);
    }

    #[test]
    fn entity_facet_cache_is_invalidated_by_a_library_commit() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        insert_download_unindexed(
            &db,
            &storage,
            "facet-cache-one",
            "一冊目",
            "同じ作者",
            &[],
            "本文",
        );
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE downloads SET author_id = 'same-author' WHERE source_id = 'facet-cache-one'",
                [],
            )
            .unwrap();
        let first = db.search_entity_facets("person", None, 60, 0, None, None, None, None).unwrap();
        assert_eq!(first[0].count, 1);
        let cached = db.search_entity_facets("person", None, 60, 0, None, None, None, None).unwrap();
        assert_eq!(cached[0].count, 1);

        insert_download_unindexed(
            &db,
            &storage,
            "facet-cache-two",
            "二冊目",
            "同じ作者",
            &[],
            "本文",
        );
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE downloads SET author_id = 'same-author' WHERE source_id = 'facet-cache-two'",
                [],
            )
            .unwrap();
        let refreshed = db.search_entity_facets("person", None, 60, 0, None, None, None, None).unwrap();
        assert_eq!(refreshed[0].count, 2);
        drop(db);
    }

    #[test]
    #[ignore = "performance smoke test for local search tuning"]
    fn smart_search_handles_5000_seed_items_under_target() {
        let (_temp, root, storage) = temp_paths();
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
        let (_temp, root, storage) = temp_paths();
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
    }

    #[test]
    fn update_job_schema_recovers_interrupted_jobs() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "work".to_string(),
            mode: "auto_save".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
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
    }

    /// 履歴は放っておくと溜まり続ける。整理は「古くて終わったもの」だけに効き、
    /// 走っているジョブと直近の履歴には触れない。
    #[test]
    fn old_finished_jobs_are_pruned_but_recent_and_live_ones_stay() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "all".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
        };
        for id in ["job-old", "job-recent", "job-live"] {
            db.create_update_job(id, &request, &[]).unwrap();
        }
        let long_ago = (chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE update_jobs SET status = 'completed', finished_at = ?1, updated_at = ?1
                 WHERE id = 'job-old'",
                params![long_ago],
            )
            .unwrap();
            conn.execute(
                "UPDATE update_jobs SET status = 'completed', finished_at = ?1, updated_at = ?1
                 WHERE id = 'job-recent'",
                params![chrono::Utc::now().to_rfc3339()],
            )
            .unwrap();
            // job-live は running のまま。日付が古くても消してはいけない。
            conn.execute(
                "UPDATE update_jobs SET status = 'running', updated_at = ?1 WHERE id = 'job-live'",
                params![long_ago],
            )
            .unwrap();
        }

        let removed = db.prune_update_jobs(1, 30).unwrap();
        assert_eq!(removed, 1);
        assert!(db.update_job_snapshot("job-old").is_err());
        assert!(db.update_job_snapshot("job-recent").is_ok());
        assert!(db.update_job_snapshot("job-live").is_ok());
    }

    #[test]
    fn job_logs_are_capped_from_the_oldest_end() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "all".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
        };
        db.create_update_job("job-logs", &request, &[]).unwrap();
        for index in 0..20 {
            db.append_update_job_log("job-logs", "info", &format!("行 {index}"))
                .unwrap();
        }
        // ジョブ作成時にも1行積まれるので、消えた数はそれ以上になる。
        assert!(db.trim_update_job_logs("job-logs", 5).unwrap() >= 15);
        let snapshot = db.update_job_snapshot("job-logs").unwrap();
        let messages: Vec<&str> = snapshot
            .logs
            .iter()
            .map(|log| log.message.as_str())
            .collect();
        assert_eq!(messages, vec!["行 15", "行 16", "行 17", "行 18", "行 19"]);
    }

    /// シリーズの作者が手元のライブラリから分かること。
    ///
    /// 作者を監視しているならシリーズの新作もその一覧に出るので、両方を走査
    /// しないための判断材料になる（同じものを二度取りに行かない）。
    #[test]
    fn a_series_can_name_the_author_behind_it() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let id = insert_download(
            &db,
            &storage,
            "novel-1",
            "夜明けの糸",
            "青葉しおり",
            &[],
            "本文",
        );
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE downloads SET author_id = '7' WHERE id = ?1",
                params![id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO download_series (download_id, series_source, series_key, title)
                 VALUES (?1, 'pixiv', '778120', '星を編む人')",
                params![id],
            )
            .unwrap();
        }

        assert_eq!(
            db.series_author_keys("pixiv", "778120").unwrap(),
            vec!["7".to_string()]
        );
        // 手元に作品が無いシリーズは判断できない。そのときは走査する側に倒す。
        assert!(db.series_author_keys("pixiv", "999999").unwrap().is_empty());
    }

    /// 終わったジョブは操作履歴からまとめて消せる。走っているものは残る。
    #[test]
    fn finished_jobs_can_be_cleared_from_the_history() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "all".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
        };
        db.create_update_job("job-done", &request, &[]).unwrap();
        db.create_update_job("job-running", &request, &[]).unwrap();
        db.set_update_job_status("job-done", "completed", None)
            .unwrap();
        db.set_update_job_status("job-running", "running", None)
            .unwrap();

        assert_eq!(db.clear_finished_update_jobs().unwrap(), 1);
        assert!(db.update_job_snapshot("job-done").is_err());
        assert!(db.update_job_snapshot("job-running").is_ok());
    }

    /// 見つけた候補は、ジョブが変わっても残る。
    ///
    /// 取得元の一覧は「前回見た位置」から先しか返さないので、候補をジョブ限りに
    /// すると、保存しなかった作品が二度と現れない。実際にそれで作品が消えた。
    #[test]
    fn a_candidate_nobody_answered_survives_into_the_next_job() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let candidate = UpdateCandidateInput {
            source: "pixiv".to_string(),
            source_id: "12345".to_string(),
            kind: "new".to_string(),
            title: "モリゾーの最新作".to_string(),
            payload_json: "{}".to_string(),
            target_type: Some("author".to_string()),
        };
        db.upsert_update_candidate(&candidate).unwrap();

        // 同じ作品を何度見つけても増えない。
        db.upsert_update_candidate(&candidate).unwrap();
        let pending = db.list_pending_update_candidates(100).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "モリゾーの最新作");

        // 「今後は出さない」と決めたら候補から外れ、見つけ直しても戻らない。
        db.set_update_candidate_status("pixiv", "12345", "dismissed")
            .unwrap();
        db.upsert_update_candidate(&candidate).unwrap();
        assert!(db.list_pending_update_candidates(100).unwrap().is_empty());
        assert_eq!(
            db.update_candidate_status("pixiv", "12345")
                .unwrap()
                .as_deref(),
            Some("dismissed")
        );
        assert_eq!(db.count_dismissed_update_candidates().unwrap(), 1);

        // 決定は取り消せる。
        assert_eq!(db.restore_dismissed_update_candidates().unwrap(), 1);
        assert_eq!(db.list_pending_update_candidates(100).unwrap().len(), 1);

        // 保存できたら、もう決めていないものではない。
        db.clear_update_candidate("pixiv", "12345").unwrap();
        assert!(db.list_pending_update_candidates(100).unwrap().is_empty());
        assert!(db
            .update_candidate_status("pixiv", "12345")
            .unwrap()
            .is_none());
    }

    /// 進捗は「これから何件やるか」を表す。保存のために待機列へ入れた候補は
    /// 作業なので数え、見つけただけの候補は数えない。
    #[test]
    fn queued_candidates_count_towards_progress_but_unanswered_ones_do_not() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "author".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
        };
        db.create_update_job(
            "job-progress",
            &request,
            &[UpdateJobItemInput {
                item_type: "target".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("7".to_string()),
                target_type: Some("author".to_string()),
                title: "作者".to_string(),
                payload_json: "{}".to_string(),
                status: "queued".to_string(),
            }],
        )
        .unwrap();
        for index in 0..3 {
            db.insert_update_job_candidate(
                "job-progress",
                &UpdateJobItemInput {
                    item_type: "candidate".to_string(),
                    source: Some("pixiv".to_string()),
                    source_id: Some(format!("novel-{index}")),
                    target_type: Some("author".to_string()),
                    title: format!("候補 {index}"),
                    payload_json: "{}".to_string(),
                    status: "candidate".to_string(),
                },
            )
            .unwrap();
        }

        // 見つけただけの段階: 作業は対象1件のみ。
        let snapshot = db.update_job_snapshot("job-progress").unwrap();
        assert_eq!(snapshot.totals, 1);
        assert_eq!(snapshot.candidate_count, 3);

        // 保存を頼んだ2件は作業に加わる。ここで進捗が 1/1 のままだと、
        // 保存の最中に「完了」と見えてしまう。
        let ids: Vec<i64> = snapshot.candidates.iter().take(2).map(|c| c.id).collect();
        db.queue_update_job_candidates("job-progress", &ids)
            .unwrap();
        let snapshot = db.update_job_snapshot("job-progress").unwrap();
        assert_eq!(snapshot.totals, 3);
        assert_eq!(snapshot.processed, 0);
    }

    /// 監視対象の健康状態。「確認した」と「見つかった」は別で、失敗は積み上がる。
    #[test]
    fn a_target_records_when_it_last_found_something_and_when_it_keeps_failing() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        db.upsert_update_target(&UpdateTargetInput {
            target_type: "author".to_string(),
            source: "pixiv".to_string(),
            source_key: "7".to_string(),
            display_name: "青葉しおり".to_string(),
            enabled: true,
            metadata_json: None,
        })
        .unwrap();

        // 何も見つからなかった確認では、最後に見つけた時刻は空のまま。
        db.mark_update_target_checked("author", "pixiv", "7", None, None, 0)
            .unwrap();
        let target = db.list_update_targets(None, false).unwrap().remove(0);
        assert!(target.last_checked_at.is_some());
        assert!(target.last_hit_at.is_none());
        assert_eq!(target.consecutive_errors, 0);

        // 失敗は積み上がる。
        db.mark_update_target_failed("author", "pixiv", "7")
            .unwrap();
        db.mark_update_target_failed("author", "pixiv", "7")
            .unwrap();
        let target = db.list_update_targets(None, false).unwrap().remove(0);
        assert_eq!(target.consecutive_errors, 2);

        // 見つかった確認で、時刻が進み、連続失敗は 0 に戻る。
        db.mark_update_target_checked("author", "pixiv", "7", Some("12002"), None, 3)
            .unwrap();
        let target = db.list_update_targets(None, false).unwrap().remove(0);
        assert!(target.last_hit_at.is_some());
        assert_eq!(target.consecutive_errors, 0);
        assert_eq!(target.last_seen_source_id.as_deref(), Some("12002"));
    }

    #[test]
    fn update_job_candidates_can_be_queued_for_saving() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "author".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
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
        let candidate = UpdateJobItemInput {
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
        };
        assert!(db
            .insert_update_job_candidate("job-candidates", &candidate)
            .unwrap());
        assert!(!db
            .insert_update_job_candidate("job-candidates", &candidate)
            .unwrap());
        let snapshot = db.update_job_snapshot("job-candidates").unwrap();
        assert_eq!(snapshot.candidate_count, 1);
        assert_eq!(snapshot.candidates.len(), 1);
        let candidate_id = snapshot.candidates[0].id;
        let changed = db
            .queue_update_job_candidates("job-candidates", &[candidate_id])
            .unwrap();
        assert_eq!(changed, 1);
        let snapshot = db.update_job_snapshot("job-candidates").unwrap();
        assert_eq!(snapshot.candidates[0].status, "queued");
    }

    #[test]
    fn update_job_snapshot_pages_candidate_payloads() {
        let (_temp, root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "author".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            adhoc_targets: None,
        };
        db.create_update_job("job-paged", &request, &[]).unwrap();
        for index in 0..205 {
            db.insert_update_job_candidate(
                "job-paged",
                &UpdateJobItemInput {
                    item_type: "candidate".to_string(),
                    source: Some("pixiv".to_string()),
                    source_id: Some(format!("novel-{index}")),
                    target_type: Some("author".to_string()),
                    title: format!("Novel {index}"),
                    payload_json: "{}".to_string(),
                    status: "candidate".to_string(),
                },
            )
            .unwrap();
        }

        let first = db.update_job_snapshot("job-paged").unwrap();
        assert_eq!(first.candidates.len(), 200);
        let cursor = first.next_candidate_cursor.expect("next page");
        let second = db
            .update_job_snapshot_page("job-paged", Some(cursor), None)
            .unwrap();
        assert_eq!(second.candidates.len(), 5);
        assert!(second.next_candidate_cursor.is_none());
        assert!(second.candidates[0].id > cursor);
    }
}
