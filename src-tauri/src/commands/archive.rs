use crate::database::queries::{PortableCollectionPairFeedback, PortableTag, PortableWorkEdit};
use crate::database::{
    Database, DownloadEntry, EntityVersion, NewAsset, NewDownload, NewVersion, SavedSearch,
    SearchV2Params, SeriesEntry, UpdateCandidateRow, UpdateTarget, WorkCollectionInput,
    WorkCollectionMemberInput, WorkKey,
};
use crate::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use tauri::{Emitter, Manager};

static IMPORT_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// 復元の進み具合。**進捗も中止も無いままでよい長さではない。**
///
/// 数万件・数十GBの棚では復元は数時間かかりうる。それまで画面に出るのは
/// 「検証済み」の一行だけで、進捗は 0% のまま最後に 100% になり、押し間違えた
/// 復元を止める手段が無かった。その間ライブラリの錠は握りっぱなしなので、
/// ほかの操作もすべて待たされる。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveProgress {
    pub job_id: String,
    /// `extract` / `promote` / `database` / `part`。
    pub phase: String,
    pub processed: i64,
    pub total: i64,
    /// いま扱っているものの名前。ファイル名やパート名。
    pub label: Option<String>,
}

/// 走っている復元と、その中止の合図。
static ARCHIVE_JOBS: LazyLock<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// 復元の進み具合を、いま走っているジョブへ結び付けて運ぶ。
///
/// **`tauri::AppHandle` を持たない。** 持たせていたころ、テスト用バイナリが
/// Wry の実体を道連れに引き込み、WebView2 のシンボルを解決できずに
/// `STATUS_ENTRYPOINT_NOT_FOUND` で起動しなくなった（コンパイルは通るので、
/// 走らせるまで分からない）。送り先は呼ぶ側が閉じ込めて渡す。
#[derive(Clone)]
pub(crate) struct ArchiveReporter {
    emit: Option<Arc<dyn Fn(ArchiveProgress) + Send + Sync>>,
    job_id: String,
    cancel: Arc<AtomicBool>,
}

impl ArchiveReporter {
    /// 画面を持たない経路（テスト、`example`）では何も送らず、中止もされない。
    pub(crate) fn detached() -> Self {
        Self {
            emit: None,
            job_id: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn register(app: tauri::AppHandle, job_id: String) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut jobs) = ARCHIVE_JOBS.lock() {
            jobs.insert(job_id.clone(), cancel.clone());
        }
        Self {
            emit: Some(Arc::new(move |progress: ArchiveProgress| {
                // 受け手が居ないことは異常ではない（方針書のとおり）。
                let _ = app.emit("archive-progress", progress);
            })),
            job_id,
            cancel,
        }
    }

    fn finish(&self) {
        if let Ok(mut jobs) = ARCHIVE_JOBS.lock() {
            jobs.remove(&self.job_id);
        }
    }

    /// **止めてよい場所でだけ確かめる。** ライブラリを書き換え始めたあとは、
    /// 途中で降りるほうが危ない。
    pub(crate) fn check_cancelled(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("復元を中止しました".to_string());
        }
        Ok(())
    }

    pub(crate) fn report(&self, phase: &str, processed: i64, total: i64, label: Option<String>) {
        let Some(emit) = &self.emit else { return };
        emit(ArchiveProgress {
            job_id: self.job_id.clone(),
            phase: phase.to_string(),
            processed,
            total,
            label,
        });
    }
}

const MAX_BACKUP_ENTRIES: usize = 250_000;
const MAX_BACKUP_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_BACKUP_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_BACKUP_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BACKUP_COMPRESSION_RATIO: u64 = 250;
const EXPORT_TEMP_PREFIX: &str = ".piep-export-";
/// Keep each library query bounded. A complete backup follows the cursor until
/// exhaustion; this is a page size, not a cap on the number of works exported.
const BACKUP_PAGE_SIZE: i64 = 1_000;
const MULTIPART_WORKS_PER_PART: usize = 2_000;
const MULTIPART_ENTITIES_PER_PART: i64 = 5_000;
const MAX_MULTIPART_PARTS: usize = 2_048;
const MAX_MULTIPART_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REIMPORT_VERSIONS_PER_WORK: usize = 10_000;

fn archive_metadata_is_link(metadata: &std::fs::Metadata) -> bool {
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

fn canonical_reimport_directory(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Reimport directory metadata failed: {error}"))?;
    if archive_metadata_is_link(&metadata) || !metadata.file_type().is_dir() {
        return Err("Reimport directory must be a real directory, not a link".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Reimport directory resolution failed: {error}"))?;
    if !canonical.starts_with(root) {
        return Err("Reimport directory escapes the library storage".to_string());
    }
    Ok(canonical)
}

fn canonical_reimport_json(path: &Path, version_root: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Reimport JSON metadata failed: {error}"))?;
    if archive_metadata_is_link(&metadata) || !metadata.file_type().is_file() {
        return Err("Reimport JSON must be a real regular file, not a link".to_string());
    }
    if metadata.len() > MAX_BACKUP_METADATA_BYTES {
        return Err(format!(
            "Reimport JSON is too large: {} bytes (limit {})",
            metadata.len(),
            MAX_BACKUP_METADATA_BYTES
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Reimport JSON resolution failed: {error}"))?;
    if !canonical.starts_with(version_root) {
        return Err("Reimport JSON escapes its version directory".to_string());
    }
    Ok(canonical)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultipartBackupManifest {
    format: String,
    created_at: String,
    total_works: i64,
    parts: Vec<MultipartBackupPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultipartBackupPart {
    file: String,
    work_count: i64,
    bytes: u64,
    sha256: String,
}

fn restore_failpoint(configured: Option<&str>, name: &'static str) -> Result<(), String> {
    if configured.is_some_and(|configured| configured == name) {
        return Err(format!("Injected restore failure at {name}"));
    }
    Ok(())
}

/// パスがストレージ内にあるか検証するヘルパー (Zip Slip や Directory Traversal を防止)
fn validate_path_in_storage(path: &str, storage_dir: &std::path::Path) -> Result<(), String> {
    let p = std::path::Path::new(path);
    // 物理絶対パスに解決（..の除去と正規化）
    let canon_p = p
        .canonicalize()
        .map_err(|e| format!("Invalid path or file not found: {}", e))?;
    let canon_storage = storage_dir
        .canonicalize()
        .map_err(|e| format!("Storage path resolution failed: {}", e))?;

    if !canon_p.starts_with(&canon_storage) {
        return Err("Access Denied: Path is outside of storage directory".to_string());
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupVersion {
    version: i64,
    content_hash: Option<String>,
    text_length: i64,
    file_size_bytes: i64,
    created_at: String,
    change_summary: Option<String>,
    relative_json_path: String,
    relative_original_json_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupAsset {
    asset_type: String,
    filename: String,
    original_url: Option<String>,
    mime_type: Option<String>,
    file_size_bytes: i64,
    relative_local_path: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupEntry {
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    content_type: String,
    #[serde(default, deserialize_with = "deserialize_backup_tags")]
    tags: Vec<String>,
    /// `tags` remains for old archives. New archives also preserve where every tag came from.
    #[serde(default)]
    tag_sources: Vec<PortableTag>,
    excerpt: Option<String>,
    relative_cover_path: Option<String>,
    watch_updates: bool,
    favorite: bool,
    downloaded_at: String,
    source_created_at: Option<String>,
    source_updated_at: Option<String>,
    current_version: i64,
    versions: Vec<BackupVersion>,
    assets: Vec<BackupAsset>,
    relations: Vec<BackupDownloadRelation>,
    people: Vec<BackupDownloadPerson>,
    series: Vec<BackupDownloadSeries>,
    #[serde(default)]
    work_edits: Vec<PortableWorkEdit>,
}

fn deserialize_backup_tags<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.map(tags_from_json_value).unwrap_or_default())
}

fn tags_from_json_value(value: serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(tag) => Some(tag),
                serde_json::Value::Object(mut obj) => match obj.remove("name") {
                    Some(serde_json::Value::String(tag)) => Some(tag),
                    _ => None,
                },
                _ => None,
            })
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect(),
        serde_json::Value::String(raw) => parse_legacy_tag_string(&raw),
        _ => Vec::new(),
    }
}

fn parse_legacy_tag_string(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return tags_from_json_value(value);
    }
    trimmed
        .replace(['[', ']', '"', '\''], "")
        .split(',')
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupDownloadRelation {
    relation_type: String,
    source: String,
    relation_id: String,
    relation_name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupDownloadPerson {
    person_source: String,
    person_key: String,
    role: String,
    display_name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupDownloadSeries {
    series_source: String,
    series_key: String,
    title: String,
    content_order: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupPerson {
    source: String,
    source_key: String,
    display_name: String,
    relative_icon_path: Option<String>,
    relative_cover_path: Option<String>,
    description: Option<String>,
    links_json: Option<String>,
    content_hash: Option<String>,
    current_version: i64,
    last_checked_at: Option<String>,
    last_fetched_at: Option<String>,
    created_at: String,
    updated_at: String,
    versions: Vec<BackupEntityVersion>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupSeries {
    source: String,
    source_key: String,
    title: String,
    description: Option<String>,
    relative_cover_path: Option<String>,
    content_hash: Option<String>,
    current_version: i64,
    last_checked_at: Option<String>,
    last_fetched_at: Option<String>,
    created_at: String,
    updated_at: String,
    versions: Vec<BackupEntityVersion>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupEntityVersion {
    entity_type: String,
    source: String,
    source_key: String,
    version: i64,
    content_hash: Option<String>,
    relative_json_path: String,
    asset_count: i64,
    file_size_bytes: i64,
    created_at: String,
    change_summary: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupMetadata {
    version: String,
    created_at: String,
    entries: Vec<BackupEntry>,
    people: Vec<BackupPerson>,
    series: Vec<BackupSeries>,
    update_targets: Vec<UpdateTarget>,
    /// Durable pending discoveries and dismissed decisions. Without these,
    /// restoring the target cursor can skip a work that the old library had
    /// already discovered but had not saved yet.
    #[serde(default)]
    update_candidates: Vec<UpdateCandidateRow>,
    #[serde(default)]
    collections: Vec<BackupWorkCollection>,
    #[serde(default)]
    saved_searches: Vec<SavedSearch>,
    #[serde(default)]
    collection_pair_feedback: Vec<PortableCollectionPairFeedback>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupWorkCollection {
    id: String,
    name: String,
    description: Option<String>,
    collection_kind: String,
    cover_work: Option<WorkKey>,
    #[serde(default)]
    cover_mode: Option<String>,
    /// A managed, app-data relative file. Absolute source paths never enter an archive.
    #[serde(default)]
    relative_cover_image_path: Option<String>,
    #[serde(default)]
    name_source: Option<String>,
    #[serde(default)]
    track: Option<String>,
    members: Vec<BackupWorkCollectionMember>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupWorkCollectionMember {
    source: String,
    source_id: String,
    #[serde(default)]
    title_snapshot: String,
    #[serde(default)]
    author_snapshot: String,
    position: i64,
    member_role: String,
    added_by: String,
    pinned: bool,
    note: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BackupInspection {
    pub valid: bool,
    pub error: Option<String>,
    pub backup_version: Option<String>,
    pub entry_count: usize,
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
    pub required_free_bytes: u64,
    pub available_free_bytes: Option<u64>,
    pub work_count: usize,
    pub person_count: usize,
    pub series_count: usize,
    pub version_count: usize,
    pub asset_count: usize,
    pub warnings: Vec<String>,
}

fn get_relative_path(full_path: &str, storage_dir: &Path) -> Option<String> {
    let full = Path::new(full_path);
    // canonicalize して比較できるようにする
    let canon_full = full.canonicalize().ok()?;
    let canon_storage = storage_dir.canonicalize().ok()?;

    if let Ok(rel) = canon_full.strip_prefix(&canon_storage) {
        Some(rel.to_string_lossy().replace('\\', "/"))
    } else {
        None
    }
}

fn validated_backup_relative_path(raw: &str) -> Result<PathBuf, String> {
    // ZIP names are defined with '/' separators. Rejecting '\\' also makes an
    // archive produced on Unix safe to import on Windows later.
    if raw.is_empty()
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
        || raw.split('/').any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains(':')
                || segment.len() > 255
                || windows_reserved_path_component(segment)
        })
    {
        return Err(format!("Invalid backup relative path: {raw}"));
    }

    let path = Path::new(raw);
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(format!("Invalid backup relative path: {raw}"));
    }
    Ok(path.to_path_buf())
}

fn windows_reserved_path_component(segment: &str) -> bool {
    if segment.ends_with('.') || segment.ends_with(' ') {
        return true;
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9'))
}

fn first_backup_component(path: &Path) -> Option<&str> {
    path.components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
}

fn resolve_zip_import_path<'a>(
    entry_name: &str,
    storage_dir: &'a Path,
    app_data_dir: &'a Path,
) -> Result<(PathBuf, &'a Path), String> {
    let relative = validated_backup_relative_path(entry_name)?;
    let root = match first_backup_component(&relative) {
        Some("profiles" | "series" | "collection-covers") => app_data_dir,
        _ => storage_dir,
    };
    Ok((root.join(relative), root))
}

fn resolve_storage_metadata_path(raw: &str, storage_dir: &Path) -> Result<PathBuf, String> {
    let relative = validated_backup_relative_path(raw)?;
    if matches!(
        first_backup_component(&relative),
        Some("profiles" | "series" | "collection-covers")
    ) {
        return Err(format!(
            "Invalid storage metadata path (reserved app-data root): {raw}"
        ));
    }
    Ok(storage_dir.join(relative))
}

fn resolve_entity_metadata_path(
    raw: &str,
    expected_prefix: &str,
    app_data_dir: &Path,
) -> Result<PathBuf, String> {
    let relative = validated_backup_relative_path(raw)?;
    if first_backup_component(&relative) != Some(expected_prefix) {
        return Err(format!(
            "Invalid {expected_prefix} metadata path outside its expected root: {raw}"
        ));
    }
    Ok(app_data_dir.join(relative))
}

fn canonical_path_is_within(path: &Path, root: &Path) -> Result<(), String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("Failed to resolve import root: {e}"))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|e| format!("Failed to resolve import path: {e}"))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("Security Exception: import path escapes its expected root".to_string());
    }
    Ok(())
}

fn validate_import_parent_before_create(outpath: &Path, root: &Path) -> Result<(), String> {
    let mut existing = if outpath.exists() {
        outpath
    } else {
        outpath
            .parent()
            .ok_or_else(|| "Invalid import path without a parent".to_string())?
    };
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| "Import path has no existing ancestor".to_string())?;
    }
    canonical_path_is_within(existing, root)
}

fn import_collision_key(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

fn validate_backup_metadata_paths(
    metadata: &BackupMetadata,
    storage_dir: &Path,
    app_data_dir: &Path,
) -> Result<(), String> {
    for person in &metadata.people {
        if let Some(path) = &person.relative_icon_path {
            resolve_entity_metadata_path(path, "profiles", app_data_dir)?;
        }
        if let Some(path) = &person.relative_cover_path {
            resolve_entity_metadata_path(path, "profiles", app_data_dir)?;
        }
        for version in &person.versions {
            resolve_entity_metadata_path(&version.relative_json_path, "profiles", app_data_dir)?;
        }
    }

    for series in &metadata.series {
        if let Some(path) = &series.relative_cover_path {
            resolve_entity_metadata_path(path, "series", app_data_dir)?;
        }
        for version in &series.versions {
            resolve_entity_metadata_path(&version.relative_json_path, "series", app_data_dir)?;
        }
    }

    for collection in &metadata.collections {
        if let Some(path) = &collection.relative_cover_image_path {
            resolve_entity_metadata_path(path, "collection-covers", app_data_dir)?;
        }
    }

    for entry in &metadata.entries {
        if let Some(path) = &entry.relative_cover_path {
            resolve_storage_metadata_path(path, storage_dir)?;
        }
        for version in &entry.versions {
            resolve_storage_metadata_path(&version.relative_json_path, storage_dir)?;
            if let Some(path) = &version.relative_original_json_path {
                resolve_storage_metadata_path(path, storage_dir)?;
            }
        }
        for asset in &entry.assets {
            resolve_storage_metadata_path(&asset.relative_local_path, storage_dir)?;
        }

        // This fallback is used for old metadata without version records. Source
        // identifiers are untrusted metadata too, so validate the derived path.
        resolve_storage_metadata_path(
            &format!(
                "{}/{}/v{}/original.json",
                entry.source, entry.source_id, entry.current_version
            ),
            storage_dir,
        )?;
    }
    Ok(())
}

fn validate_backup_metadata_files_exist(
    metadata: &BackupMetadata,
    storage_dir: &Path,
    app_data_dir: &Path,
) -> Result<(), String> {
    let require_file = |path: PathBuf, label: &str| {
        if path.is_file() {
            Ok(())
        } else {
            Err(format!("Backup is missing {label}: {}", path.display()))
        }
    };
    for person in &metadata.people {
        for path in [
            person.relative_icon_path.as_ref(),
            person.relative_cover_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            require_file(
                resolve_entity_metadata_path(path, "profiles", app_data_dir)?,
                "person asset",
            )?;
        }
        for version in &person.versions {
            require_file(
                resolve_entity_metadata_path(
                    &version.relative_json_path,
                    "profiles",
                    app_data_dir,
                )?,
                "person metadata",
            )?;
        }
    }
    for series in &metadata.series {
        if let Some(path) = &series.relative_cover_path {
            require_file(
                resolve_entity_metadata_path(path, "series", app_data_dir)?,
                "series cover",
            )?;
        }
        for version in &series.versions {
            require_file(
                resolve_entity_metadata_path(&version.relative_json_path, "series", app_data_dir)?,
                "series metadata",
            )?;
        }
    }
    for collection in &metadata.collections {
        if let Some(path) = &collection.relative_cover_image_path {
            require_file(
                resolve_entity_metadata_path(path, "collection-covers", app_data_dir)?,
                "collection cover",
            )?;
        }
    }
    for entry in &metadata.entries {
        if entry.versions.is_empty() {
            return Err(format!(
                "Work {}/{} has no version payload",
                entry.source, entry.source_id
            ));
        }
        if let Some(path) = &entry.relative_cover_path {
            require_file(
                resolve_storage_metadata_path(path, storage_dir)?,
                "work cover",
            )?;
        }
        for version in &entry.versions {
            require_file(
                resolve_storage_metadata_path(&version.relative_json_path, storage_dir)?,
                "work metadata",
            )?;
            if let Some(path) = &version.relative_original_json_path {
                require_file(
                    resolve_storage_metadata_path(path, storage_dir)?,
                    "work original metadata",
                )?;
            }
        }
        for asset in &entry.assets {
            require_file(
                resolve_storage_metadata_path(&asset.relative_local_path, storage_dir)?,
                "work asset",
            )?;
        }
    }
    Ok(())
}

fn referenced_backup_paths(metadata: &BackupMetadata) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for person in &metadata.people {
        paths.extend(person.relative_icon_path.iter().cloned());
        paths.extend(person.relative_cover_path.iter().cloned());
        paths.extend(
            person
                .versions
                .iter()
                .map(|version| version.relative_json_path.clone()),
        );
    }
    for series in &metadata.series {
        paths.extend(series.relative_cover_path.iter().cloned());
        paths.extend(
            series
                .versions
                .iter()
                .map(|version| version.relative_json_path.clone()),
        );
    }
    for collection in &metadata.collections {
        paths.extend(collection.relative_cover_image_path.iter().cloned());
    }
    for entry in &metadata.entries {
        if entry.versions.is_empty() {
            return Err(format!(
                "Work {}/{} has no version payload",
                entry.source, entry.source_id
            ));
        }
        paths.extend(entry.relative_cover_path.iter().cloned());
        for version in &entry.versions {
            paths.push(version.relative_json_path.clone());
            paths.extend(version.relative_original_json_path.iter().cloned());
        }
        paths.extend(
            entry
                .assets
                .iter()
                .map(|asset| asset.relative_local_path.clone()),
        );
    }
    for path in &paths {
        validated_backup_relative_path(path)?;
    }
    Ok(paths)
}

fn preflight_backup_archive(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<u64, String> {
    if archive.is_empty() || archive.len() > MAX_BACKUP_ENTRIES {
        return Err(format!(
            "Backup entry count is outside the allowed quota: {}",
            archive.len()
        ));
    }
    let mut total = 0u64;
    let mut metadata_count = 0usize;
    let mut names = HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        validated_backup_relative_path(&name)
            .map_err(|e| format!("Security Exception for zip entry {name}: {e}"))?;
        if !names.insert(name.clone()) {
            return Err(format!("Duplicate zip entry path: {name}"));
        }
        if file.is_dir() {
            continue;
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!(
                "Symbolic links are not accepted in backups: {name}"
            ));
        }
        let size = file.size();
        let compressed = file.compressed_size();
        if size > MAX_BACKUP_ENTRY_BYTES {
            return Err(format!("Backup entry exceeds the per-file quota: {name}"));
        }
        if size > 1024 * 1024
            && (compressed == 0 || size / compressed.max(1) > MAX_BACKUP_COMPRESSION_RATIO)
        {
            return Err(format!(
                "Suspicious compression ratio in backup entry: {name}"
            ));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| "Backup expanded-size overflow".to_string())?;
        if total > MAX_BACKUP_TOTAL_BYTES {
            return Err("Backup exceeds the expanded-size quota".to_string());
        }
        if name == "backup_metadata.json" {
            metadata_count += 1;
            if size > MAX_BACKUP_METADATA_BYTES {
                return Err("Backup metadata exceeds its size quota".to_string());
            }
        }
    }
    if metadata_count != 1 {
        return Err("Backup must contain exactly one backup_metadata.json".to_string());
    }
    Ok(total)
}

struct PromotedFile {
    staged: PathBuf,
    destination: PathBuf,
    backup: Option<PathBuf>,
}

struct StagingGuard {
    root: PathBuf,
    armed: bool,
}

impl StagingGuard {
    fn new(root: PathBuf) -> Self {
        Self { root, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

struct PromotionGuard {
    stage_root: PathBuf,
    files: Vec<PromotedFile>,
    committed: bool,
}

impl PromotionGuard {
    fn promote(&mut self) -> Result<(), String> {
        for file in &self.files {
            atomic_replace_file(&file.staged, &file.destination).map_err(|error| {
                format!(
                    "Could not promote restored file {}: {error}",
                    file.destination.display()
                )
            })?;
        }
        Ok(())
    }

    fn commit(mut self) {
        self.committed = true;
        for file in &self.files {
            if let Some(backup) = &file.backup {
                let _ = std::fs::remove_file(backup);
            }
        }
        let _ = std::fs::remove_dir_all(&self.stage_root);
    }

    fn preserve_for_recovery(mut self) {
        self.committed = true;
    }
}

#[derive(Serialize, Deserialize)]
struct RestoreFileJournal {
    files: Vec<RestoreFileJournalEntry>,
    #[serde(default)]
    stale_index_ids: Vec<i64>,
}

#[derive(Serialize, Deserialize)]
struct RestoreFileJournalEntry {
    destination: String,
    backup: Option<String>,
}

const RESTORE_FILE_JOURNAL: &str = "file-journal.json";

fn write_restore_file_journal(stage_root: &Path, files: &[PromotedFile]) -> Result<(), String> {
    let journal = RestoreFileJournal {
        files: files
            .iter()
            .map(|file| RestoreFileJournalEntry {
                destination: file.destination.to_string_lossy().to_string(),
                backup: file
                    .backup
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
            })
            .collect(),
        stale_index_ids: Vec::new(),
    };
    let path = stage_root.join(RESTORE_FILE_JOURNAL);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not create restore file journal: {error}"))?;
    serde_json::to_writer(&mut file, &journal)
        .map_err(|error| format!("Could not serialize restore file journal: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not sync restore file journal: {error}"))?;
    sync_export_parent(&path)?;
    Ok(())
}

fn record_restore_stale_index_ids(stage_root: &Path, ids: &[i64]) -> Result<(), String> {
    let mut journal = read_restore_file_journal(stage_root)?;
    journal.stale_index_ids = ids.to_vec();
    let journal_path = stage_root.join(RESTORE_FILE_JOURNAL);
    let temp_path = stage_root.join(format!("{RESTORE_FILE_JOURNAL}.tmp"));
    let mut temp = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|error| format!("Could not create restore journal update: {error}"))?;
    let result = (|| {
        serde_json::to_writer(&mut temp, &journal)
            .map_err(|error| format!("Could not update restore file journal: {error}"))?;
        temp.sync_all()
            .map_err(|error| format!("Could not sync restore journal update: {error}"))?;
        drop(temp);
        atomic_replace_file(&temp_path, &journal_path)?;
        sync_export_parent(&journal_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

fn read_restore_file_journal(stage_root: &Path) -> Result<RestoreFileJournal, String> {
    let path = stage_root.join(RESTORE_FILE_JOURNAL);
    let file = std::fs::File::open(&path)
        .map_err(|error| format!("Could not open restore file journal: {error}"))?;
    serde_json::from_reader(file)
        .map_err(|error| format!("Could not decode restore file journal: {error}"))
}

fn validate_recovery_path(path: &Path, storage: &Path, app_data: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Restore recovery path has no parent".to_string())?;
    let canonical_app_data = app_data
        .canonicalize()
        .map_err(|error| format!("Could not resolve app data for recovery: {error}"))?;
    let existing = if parent.exists() { parent } else { storage };
    let canonical_existing = existing
        .canonicalize()
        .map_err(|error| format!("Could not resolve restore recovery path: {error}"))?;
    if !canonical_existing.starts_with(&canonical_app_data) {
        return Err(format!(
            "Restore recovery path escapes app data: {}",
            path.display()
        ));
    }
    Ok(())
}

fn rollback_promoted_files_from_journal(
    stage_root: &Path,
    storage: &Path,
    app_data: &Path,
) -> Result<(), String> {
    let journal = read_restore_file_journal(stage_root)?;
    for entry in journal.files.iter().rev() {
        let destination = PathBuf::from(&entry.destination);
        validate_recovery_path(&destination, storage, app_data)?;
        if destination.exists() {
            if !destination.is_file() {
                return Err(format!(
                    "Restore recovery destination is not a file: {}",
                    destination.display()
                ));
            }
            std::fs::remove_file(&destination).map_err(|error| {
                format!(
                    "Could not remove uncommitted restored file {}: {error}",
                    destination.display()
                )
            })?;
        }
        if let Some(backup) = &entry.backup {
            let backup = PathBuf::from(backup);
            let canonical_stage = stage_root
                .canonicalize()
                .map_err(|error| format!("Could not resolve restore staging: {error}"))?;
            let canonical_backup = backup
                .canonicalize()
                .map_err(|error| format!("Could not resolve restore rollback copy: {error}"))?;
            let backup_type = std::fs::symlink_metadata(&backup)
                .map_err(|error| format!("Could not inspect restore rollback copy: {error}"))?
                .file_type();
            if backup_type.is_symlink()
                || !canonical_backup.is_file()
                || !canonical_backup.starts_with(&canonical_stage)
            {
                return Err(format!(
                    "Restore rollback copy is missing or unsafe: {}",
                    backup.display()
                ));
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            // **控えは消費せずに複製する。** かつては `rename` で戻していたが、
            // それだと途中の1件で失敗した時点で、そこまでに戻した控えが
            // 手元から消えている。次の起動でやり直そうとしても控えが無く、
            // 「戻せない」と言って落ちるだけになる - つまり一度失敗すると
            // 二度と回復できない。複製なら何度でも同じ結果になる。
            // 控えごと staging を消すのは、全件を戻し終えたあとである。
            std::fs::copy(&backup, &destination).map_err(|error| {
                format!(
                    "Could not restore previous file {}: {error}",
                    destination.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Completes or rolls back any restore that stopped between file promotion and
/// cleanup. The SQLite marker is the commit authority: it changes in the same
/// transaction as the restored rows, so there is no ambiguous COMMIT window.
///
/// **回復できなかったことを、起動できない理由にしない。** 戻せなかったファイルが
/// 一つあるだけで窓が出ないと、読める棚を人質に取ることになる。回復は次の起動でも
/// 試せる（控えは消費しない）ので、ここでは何が残ったかを返して先へ進む。
/// 返り値は回復できなかったジャーナルの説明で、空なら全部片付いている。
pub fn recover_interrupted_restores(state: &AppState) -> Vec<String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();
    let pending = match state.db.pending_restore_journals() {
        Ok(pending) => pending,
        Err(error) => return vec![format!("中断した復元の記録を読めません: {error}")],
    };
    let mut unresolved = Vec::new();
    for (journal_id, stage_root, committed) in pending {
        // 一つのジャーナルの失敗で、残りのジャーナルを見ないのは筋が悪い。
        if let Err(error) = recover_one_restore_journal(
            state,
            &storage,
            &app_data,
            &journal_id,
            &stage_root,
            committed,
        ) {
            log::error!("中断した復元を回復できません（{journal_id}）: {error}");
            unresolved.push(error);
        }
    }
    unresolved
}

fn recover_one_restore_journal(
    state: &AppState,
    storage: &Path,
    app_data: &Path,
    journal_id: &str,
    stage_root: &Path,
    committed: bool,
) -> Result<(), String> {
    {
        let stage_root = stage_root.to_path_buf();
        if !stage_root.starts_with(app_data)
            || stage_root.parent() != Some(app_data)
            || !stage_root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".restore-staging-"))
        {
            return Err(format!(
                "Restore journal has an unsafe staging root: {}",
                stage_root.display()
            ));
        }
        if stage_root.exists() {
            let canonical_app_data = app_data
                .canonicalize()
                .map_err(|error| format!("Could not resolve app data for recovery: {error}"))?;
            let stage_type = std::fs::symlink_metadata(&stage_root)
                .map_err(|error| format!("Could not inspect restore staging: {error}"))?
                .file_type();
            let canonical_stage = stage_root
                .canonicalize()
                .map_err(|error| format!("Could not resolve restore staging: {error}"))?;
            if stage_type.is_symlink() || !canonical_stage.starts_with(canonical_app_data) {
                return Err(format!(
                    "Restore journal staging root resolves outside app data: {}",
                    stage_root.display()
                ));
            }
        }
        if committed {
            // Files and DB are the new generation. Only old rollback copies and
            // derived-index catch-up remain.
            if stage_root.exists() {
                let journal = read_restore_file_journal(&stage_root)?;
                let lexical = crate::database::tantivy_index::delete_documents(
                    storage,
                    &journal.stale_index_ids,
                );
                let semantic = crate::database::semantic_index::clear_documents(
                    storage,
                    &journal.stale_index_ids,
                );
                if let Err(error) = lexical.and(semantic) {
                    // The committed DB/files generation is safe to use and
                    // normal queries still filter stale sidecar ids through
                    // SQLite. Keep the journal for another startup retry rather
                    // than making a derived-index outage block the application.
                    log::warn!("Interrupted restore index cleanup will retry: {error}");
                    return Ok(());
                }
            }
            if stage_root.exists() {
                std::fs::remove_dir_all(&stage_root).map_err(|error| {
                    format!("Could not clean committed restore staging: {error}")
                })?;
            }
        } else {
            rollback_promoted_files_from_journal(&stage_root, storage, app_data)?;
            std::fs::remove_dir_all(&stage_root)
                .map_err(|error| format!("Could not clean rolled-back restore staging: {error}"))?;
        }
        state.db.finish_restore_journal(journal_id)?;
    }
    Ok(())
}

impl Drop for PromotionGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for file in self.files.iter().rev() {
            if let Some(backup) = &file.backup {
                let _ = atomic_replace_file(backup, &file.destination);
            } else {
                let _ = std::fs::remove_file(&file.destination);
            }
        }
        let _ = std::fs::remove_dir_all(&self.stage_root);
    }
}

fn prepare_staged_file_promotion(
    stage_root: &Path,
    entries: &[(PathBuf, PathBuf)],
) -> Result<PromotionGuard, String> {
    let rollback = stage_root.join("rollback");
    std::fs::create_dir_all(&rollback).map_err(|e| e.to_string())?;
    let mut guard = PromotionGuard {
        stage_root: stage_root.to_path_buf(),
        files: Vec::with_capacity(entries.len()),
        committed: false,
    };
    for (index, (staged, destination)) in entries.iter().enumerate() {
        if !staged.is_file() {
            return Err(format!(
                "Staged backup file disappeared: {}",
                staged.display()
            ));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let backup = if destination.exists() {
            if !destination.is_file() {
                return Err(format!(
                    "Restore destination is not a file: {}",
                    destination.display()
                ));
            }
            let backup = rollback.join(index.to_string());
            std::fs::copy(destination, &backup).map_err(|e| {
                format!(
                    "Could not preserve existing destination {}: {e}",
                    destination.display()
                )
            })?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&backup)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    format!(
                        "Could not sync preserved destination {}: {error}",
                        destination.display()
                    )
                })?;
            Some(backup)
        } else {
            None
        };
        guard.files.push(PromotedFile {
            staged: staged.clone(),
            destination: destination.clone(),
            backup,
        });
    }
    write_restore_file_journal(stage_root, &guard.files)?;
    Ok(guard)
}

#[cfg(test)]
fn promote_staged_files(
    stage_root: &Path,
    entries: &[(PathBuf, PathBuf)],
) -> Result<PromotionGuard, String> {
    let mut promotion = prepare_staged_file_promotion(stage_root, entries)?;
    promotion.promote()?;
    Ok(promotion)
}

struct ExportTempGuard {
    path: PathBuf,
    armed: bool,
}

impl ExportTempGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ExportTempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn create_export_temp(destination: &Path) -> Result<(std::fs::File, ExportTempGuard), String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "Backup destination directory does not exist: {}",
            parent.display()
        ));
    }
    if destination.exists() && !destination.is_file() {
        return Err(format!(
            "Backup destination is not a file: {}",
            destination.display()
        ));
    }

    for _ in 0..64 {
        let path = parent.join(format!(
            "{EXPORT_TEMP_PREFIX}{}-{}.tmp",
            std::process::id(),
            rand::random::<u64>()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, ExportTempGuard { path, armed: true })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create temporary backup in {}: {error}",
                    parent.display()
                ));
            }
        }
    }
    Err(format!(
        "Could not reserve a unique temporary backup in {}",
        parent.display()
    ))
}

fn required_export_relative_path(
    source_path: &str,
    root: &Path,
    label: &str,
) -> Result<String, String> {
    let source = Path::new(source_path);
    if !source.is_file() {
        return Err(format!(
            "Backup source {label} is missing or not a file: {}",
            source.display()
        ));
    }
    let relative = get_relative_path(source_path, root).ok_or_else(|| {
        format!(
            "Backup source {label} is outside its managed storage root: {}",
            source.display()
        )
    })?;
    validated_backup_relative_path(&relative)
        .map_err(|error| format!("Backup source {label} has an unsafe path: {error}"))?;
    Ok(relative)
}

fn validate_finished_export(path: &Path, storage: &Path, app_data: &Path) -> Result<(), String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Could not reopen completed backup: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("Completed backup is not a valid ZIP: {error}"))?;
    preflight_backup_archive(&mut archive)
        .map_err(|error| format!("Completed backup failed preflight: {error}"))?;

    let mut archive_names = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if !entry.is_dir() {
            archive_names.insert(entry.name().to_string());
        }
    }
    let mut metadata_text = String::new();
    archive
        .by_name("backup_metadata.json")
        .map_err(|error| format!("Completed backup metadata is missing: {error}"))?
        .take(MAX_BACKUP_METADATA_BYTES + 1)
        .read_to_string(&mut metadata_text)
        .map_err(|error| format!("Completed backup metadata cannot be read: {error}"))?;
    let metadata: BackupMetadata = serde_json::from_str(&metadata_text)
        .map_err(|error| format!("Completed backup metadata is invalid: {error}"))?;
    validate_backup_metadata_paths(&metadata, storage, app_data)
        .map_err(|error| format!("Completed backup metadata is unsafe: {error}"))?;
    for referenced in referenced_backup_paths(&metadata)? {
        if !archive_names.contains(&referenced) {
            return Err(format!(
                "Completed backup is missing a referenced file: {referenced}"
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        REPLACEFILE_IGNORE_MERGE_ERRORS,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let source_pcw = PCWSTR(source_wide.as_ptr());
    let destination_pcw = PCWSTR(destination_wide.as_ptr());

    let result = unsafe {
        if destination.exists() {
            ReplaceFileW(
                destination_pcw,
                source_pcw,
                PCWSTR::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                None,
                None,
            )
        } else {
            MoveFileExW(
                source_pcw,
                destination_pcw,
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    result.map_err(|error| {
        format!(
            "Could not atomically replace backup {}: {error}",
            destination.display()
        )
    })
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| {
        format!(
            "Could not atomically replace backup {}: {error}",
            destination.display()
        )
    })
}

#[cfg(unix)]
fn sync_export_parent(destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Could not sync backup directory: {error}"))
}

#[cfg(not(unix))]
fn sync_export_parent(_destination: &Path) -> Result<(), String> {
    Ok(())
}

fn add_zip_file_once(
    zip: &mut zip::ZipWriter<std::fs::File>,
    options: zip::write::SimpleFileOptions,
    written: &mut HashSet<String>,
    relative_path: &str,
    source_path: &Path,
) -> Result<(), String> {
    let link_metadata = std::fs::symlink_metadata(source_path).map_err(|error| {
        format!(
            "Backup source file is missing or unreadable ({}): {error}",
            source_path.display()
        )
    })?;
    if archive_metadata_is_link(&link_metadata) {
        return Err(format!(
            "Backup source must not be a link or reparse point: {}",
            source_path.display()
        ));
    }
    let mut source = std::fs::File::open(source_path).map_err(|error| {
        format!(
            "Backup source file is missing or unreadable ({}): {error}",
            source_path.display()
        )
    })?;
    if !source
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", source_path.display()))?
        .is_file()
    {
        return Err(format!(
            "Backup source is not a regular file: {}",
            source_path.display()
        ));
    }
    if !written.insert(relative_path.to_string()) {
        return Ok(());
    }
    zip.start_file(relative_path, options)
        .map_err(|e| e.to_string())?;
    std::io::copy(&mut source, zip).map_err(|error| {
        format!(
            "Could not write backup payload {}: {error}",
            source_path.display()
        )
    })?;
    Ok(())
}

#[tauri::command]
pub async fn export_single(
    app: tauri::AppHandle,
    download_id: i64,
    dest_dir: String,
) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>();
    let storage = state.db.storage_dir();
    let dl = state.db.get_download(download_id)?;
    let assets = state.db.get_assets(download_id)?;

    // コピー元JSONファイルの物理パス境界検証 (セキュリティ防壁)
    validate_path_in_storage(&dl.json_path, storage)?;
    if let Some(ref ojp) = dl.original_json_path {
        validate_path_in_storage(ojp, storage)?;
    }

    // エクスポート先に作品名ディレクトリとバージョンディレクトリを作成
    let base_dest = std::path::Path::new(&dest_dir);
    let work_name = safe_export_name(&format!("{} [{}-{}]", dl.title, dl.source, dl.source_id));
    let work_dest = base_dest.join(work_name);
    let ver_dest = work_dest.join(format!("v{}", dl.current_version));
    tokio::fs::create_dir_all(&ver_dest)
        .await
        .map_err(|e| e.to_string())?;

    let json_name = safe_export_name(&dl.title);

    // JSONコピー (バージョンフォルダ配下に、作品名ベースで)
    if std::path::Path::new(&dl.json_path).exists() {
        tokio::fs::copy(&dl.json_path, ver_dest.join(format!("{}.json", json_name)))
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref ojp) = dl.original_json_path {
        if std::path::Path::new(ojp).exists() && ojp != &dl.json_path {
            tokio::fs::copy(ojp, ver_dest.join(format!("{}_original.json", json_name)))
                .await
                .map_err(|e| e.to_string())?;
        }
    }

    // アセットコピー
    let assets_dest = ver_dest.join("data_assets");
    for asset in &assets {
        // コピー元アセットの物理パス境界検証 (セキュリティ防壁)
        validate_path_in_storage(&asset.local_path, storage)?;

        let src = std::path::Path::new(&asset.local_path);
        if src.exists() {
            // 古い書庫から復元した棚には、清められる前の名前が残っている。
            // 書き出す側でも通す - DB の中身を無条件に信じてパスを組み立てない。
            let asset_type_dest = assets_dest.join(
                crate::downloader::asset_downloader::sanitize_filename(&asset.asset_type),
            );
            tokio::fs::create_dir_all(&asset_type_dest)
                .await
                .map_err(|e| e.to_string())?;
            let filename = crate::downloader::asset_downloader::sanitize_filename(&asset.filename);
            tokio::fs::copy(src, asset_type_dest.join(filename))
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(work_dest.to_string_lossy().to_string())
}

fn safe_export_name(name: &str) -> String {
    let mut safe = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control() {
            safe.push('_');
        } else {
            safe.push(ch);
        }
    }

    let safe = safe.trim().trim_matches('.').to_string();
    if safe.is_empty() {
        "export".to_string()
    } else {
        safe.chars().take(120).collect()
    }
}

fn all_backup_search_params() -> SearchV2Params {
    SearchV2Params {
        text: None,
        query: None,
        source: None,
        content_type: None,
        sort_by: None,
        sort_order: None,
        limit: Some(BACKUP_PAGE_SIZE),
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
        view_mode: Some("gallery".to_string()),
        projection: Some("libraryGallery".to_string()),
        search_mode: None,
    }
}

/// Resolves the complete, stable work scope before creating the ZIP.
///
/// A backup used to issue one search with a 10,000 item limit. Libraries above
/// that size produced a valid-looking, successful archive that silently left
/// every later work out. Cursor paging keeps each DB response bounded while
/// making an incomplete scope a hard error instead of a successful backup.
fn collect_backup_downloads(
    state: &AppState,
    mut params: SearchV2Params,
) -> Result<Vec<DownloadEntry>, String> {
    params.limit = Some(BACKUP_PAGE_SIZE);
    params.offset = None;

    let mut downloads = Vec::new();
    let mut seen_downloads = HashSet::new();
    let mut seen_cursors = HashSet::new();
    let mut expected_total = None;

    loop {
        let mut page = state
            .db
            .search_downloads_v2_internal(&params, BACKUP_PAGE_SIZE)?;

        if let Some(total) = page.total_estimate {
            match expected_total {
                Some(expected) if expected != total => {
                    return Err(
                        "Library changed while the backup scope was being prepared; retry the backup"
                            .to_string(),
                    );
                }
                None => expected_total = Some(total),
                _ => {}
            }
        }

        for item in page.items.drain(..) {
            if !seen_downloads.insert(item.id) {
                return Err(format!(
                    "Backup paging returned work {} more than once; retry the backup",
                    item.id
                ));
            }
            downloads.push(item);
        }

        let Some(cursor) = page.next_cursor else {
            break;
        };
        if !seen_cursors.insert(cursor.clone()) {
            return Err("Backup paging did not advance; retry the backup".to_string());
        }
        params.cursor = Some(cursor);
    }

    if let Some(expected) = expected_total {
        if downloads.len() as i64 != expected {
            return Err(format!(
                "Backup scope is incomplete: expected {expected} works but collected {}",
                downloads.len()
            ));
        }
    }

    Ok(downloads)
}

pub async fn export_all_zip_internal(state: Arc<AppState>, zip_path: String) -> Result<(), String> {
    export_zip_with_params_internal(state, zip_path, all_backup_search_params(), false).await
}

struct MultipartPartsGuard {
    paths: Vec<PathBuf>,
    armed: bool,
}

impl Drop for MultipartPartsGuard {
    fn drop(&mut self) {
        if self.armed {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("Could not hash backup part: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not hash backup part: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn read_and_validate_multipart_manifest(
    manifest_path: &Path,
) -> Result<(MultipartBackupManifest, Vec<PathBuf>), String> {
    let manifest_metadata = std::fs::symlink_metadata(manifest_path)
        .map_err(|error| format!("Could not inspect multipart manifest: {error}"))?;
    if !manifest_metadata.file_type().is_file() || manifest_metadata.file_type().is_symlink() {
        return Err("Multipart manifest must be a regular file".to_string());
    }
    if manifest_metadata.len() > MAX_MULTIPART_MANIFEST_BYTES {
        return Err("Multipart manifest exceeds the 4 MiB safety limit".to_string());
    }
    let bytes = std::fs::read(manifest_path)
        .map_err(|error| format!("Could not read multipart manifest: {error}"))?;
    let manifest: MultipartBackupManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid multipart manifest: {error}"))?;
    if manifest.format != "piep-multipart-1" {
        return Err(format!(
            "Unsupported multipart backup format: {}",
            manifest.format
        ));
    }
    if manifest.parts.len() > MAX_MULTIPART_PARTS {
        return Err("Multipart backup exceeds the 2,048-part safety limit".to_string());
    }
    if manifest.total_works < 0
        || (manifest.total_works > 0 && manifest.parts.is_empty())
        || manifest.created_at.trim().is_empty()
    {
        return Err("Multipart manifest contains inconsistent totals".to_string());
    }

    let parent = manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("Could not resolve multipart backup directory: {error}"))?;
    let mut names = HashSet::new();
    let mut paths = Vec::with_capacity(manifest.parts.len());
    let mut work_count = 0i64;
    for part in &manifest.parts {
        let relative = validated_backup_relative_path(&part.file)?;
        if relative.components().count() != 1
            || !part.file.to_ascii_lowercase().ends_with(".zip")
            || !names.insert(import_collision_key(&relative))
            || part.work_count < 0
            || part.bytes == 0
            || part.sha256.len() != 64
            || !part.sha256.bytes().all(|value| value.is_ascii_hexdigit())
        {
            return Err(format!("Invalid multipart backup part: {}", part.file));
        }
        work_count = work_count
            .checked_add(part.work_count)
            .ok_or_else(|| "Multipart work count overflow".to_string())?;
        let path = parent.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("Missing multipart backup part {}: {error}", part.file))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "Multipart part is not a regular file: {}",
                part.file
            ));
        }
        if metadata.len() != part.bytes {
            return Err(format!("Multipart part size mismatch: {}", part.file));
        }
        let canonical_path = path
            .canonicalize()
            .map_err(|error| format!("Could not resolve multipart part {}: {error}", part.file))?;
        if canonical_path.parent() != Some(canonical_parent.as_path()) {
            return Err(format!(
                "Multipart part escapes its manifest directory: {}",
                part.file
            ));
        }
        if !sha256_file(&canonical_path)?.eq_ignore_ascii_case(&part.sha256) {
            return Err(format!("Multipart part checksum mismatch: {}", part.file));
        }
        paths.push(canonical_path);
    }
    if work_count != manifest.total_works {
        return Err("Multipart manifest work count does not match its parts".to_string());
    }
    Ok((manifest, paths))
}

fn read_backup_work_keys(path: &Path) -> Result<Vec<(String, String)>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Could not open multipart part metadata: {error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("Invalid multipart ZIP: {error}"))?;
    let mut metadata_text = String::new();
    archive
        .by_name("backup_metadata.json")
        .map_err(|error| format!("Multipart part metadata is missing: {error}"))?
        .take(MAX_BACKUP_METADATA_BYTES + 1)
        .read_to_string(&mut metadata_text)
        .map_err(|error| format!("Could not read multipart part metadata: {error}"))?;
    let metadata: BackupMetadata = serde_json::from_str(&metadata_text)
        .map_err(|error| format!("Invalid multipart part metadata: {error}"))?;
    Ok(metadata
        .entries
        .into_iter()
        .map(|entry| (entry.source, entry.source_id))
        .collect())
}

fn inspect_multipart_backup_internal(
    manifest_path: &Path,
    storage: &Path,
    app_data: &Path,
) -> Result<BackupInspection, String> {
    let (manifest, paths) = read_and_validate_multipart_manifest(manifest_path)?;
    inspect_validated_multipart_parts(&manifest, &paths, storage, app_data)
}

fn inspect_validated_multipart_parts(
    manifest: &MultipartBackupManifest,
    paths: &[PathBuf],
    storage: &Path,
    app_data: &Path,
) -> Result<BackupInspection, String> {
    let mut aggregate = BackupInspection {
        valid: true,
        error: None,
        backup_version: Some(manifest.format.clone()),
        entry_count: 0,
        compressed_bytes: 0,
        expanded_bytes: 0,
        required_free_bytes: 0,
        available_free_bytes: crate::database::queries::available_space_bytes(app_data),
        work_count: 0,
        person_count: 0,
        series_count: 0,
        version_count: 0,
        asset_count: 0,
        warnings: vec![format!(
            "{}個の検証済みパートから順番に復元します。中断した場合は同じマニフェストで再開できます。",
            paths.len()
        )],
    };
    let mut work_keys = HashSet::new();
    for (part, path) in manifest.parts.iter().zip(paths.iter()) {
        let inspection = inspect_backup_internal(&path.to_string_lossy(), storage, app_data)?;
        if !inspection.valid {
            aggregate.valid = false;
            aggregate.error = Some(format!(
                "Multipart part {} is invalid: {}",
                part.file,
                inspection
                    .error
                    .unwrap_or_else(|| "unknown validation error".to_string())
            ));
            return Ok(aggregate);
        }
        if inspection.work_count as i64 != part.work_count {
            aggregate.valid = false;
            aggregate.error = Some(format!(
                "Multipart part {} work count does not match its manifest",
                part.file
            ));
            return Ok(aggregate);
        }
        for key in read_backup_work_keys(path)? {
            if !work_keys.insert(key) {
                aggregate.valid = false;
                aggregate.error = Some(format!(
                    "Multipart backup contains the same work in more than one part: {}",
                    part.file
                ));
                return Ok(aggregate);
            }
        }
        aggregate.entry_count = aggregate
            .entry_count
            .checked_add(inspection.entry_count)
            .ok_or_else(|| "Multipart entry count overflow".to_string())?;
        aggregate.compressed_bytes = aggregate
            .compressed_bytes
            .checked_add(inspection.compressed_bytes)
            .ok_or_else(|| "Multipart compressed size overflow".to_string())?;
        aggregate.expanded_bytes = aggregate
            .expanded_bytes
            .checked_add(inspection.expanded_bytes)
            .ok_or_else(|| "Multipart expanded size overflow".to_string())?;
        aggregate.work_count = aggregate
            .work_count
            .checked_add(inspection.work_count)
            .ok_or_else(|| "Multipart work count overflow".to_string())?;
        aggregate.person_count = aggregate
            .person_count
            .saturating_add(inspection.person_count);
        aggregate.series_count = aggregate
            .series_count
            .saturating_add(inspection.series_count);
        aggregate.version_count = aggregate
            .version_count
            .saturating_add(inspection.version_count);
        aggregate.asset_count = aggregate.asset_count.saturating_add(inspection.asset_count);
    }
    aggregate.required_free_bytes = aggregate
        .expanded_bytes
        .saturating_add(aggregate.expanded_bytes / 10)
        .saturating_add(256 * 1024 * 1024);
    if aggregate.work_count as i64 != manifest.total_works {
        aggregate.valid = false;
        aggregate.error = Some("Multipart inspection work count mismatch".to_string());
    } else if aggregate
        .available_free_bytes
        .is_some_and(|available| available < aggregate.required_free_bytes)
    {
        aggregate.valid = false;
        aggregate.error = Some(format!(
            "Not enough free disk space for multipart restore (required {} bytes)",
            aggregate.required_free_bytes
        ));
    }
    Ok(aggregate)
}

fn write_multipart_manifest_atomically(
    destination: &Path,
    manifest: &MultipartBackupManifest,
) -> Result<(), String> {
    let (mut file, mut guard) = create_export_temp(destination)?;
    serde_json::to_writer_pretty(&mut file, manifest)
        .map_err(|error| format!("Could not serialize multipart manifest: {error}"))?;
    file.flush()
        .map_err(|error| format!("Could not flush multipart manifest: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not sync multipart manifest: {error}"))?;
    drop(file);
    atomic_replace_file(&guard.path, destination)?;
    guard.disarm();
    if let Err(error) = sync_export_parent(destination) {
        // The manifest has already been atomically published and every part
        // was flushed before it was referenced. Treat an unsupported parent
        // directory fsync the same way as the single-ZIP exporter; returning
        // an error here would make the cleanup guard delete valid new parts.
        log::warn!("{error}");
    }
    Ok(())
}

fn is_owned_multipart_part_name(stem: &str, file: &str) -> bool {
    let Some(rest) = file.strip_prefix(&format!("{stem}-")) else {
        return false;
    };
    let Some((generation, number)) = rest.split_once(".part-") else {
        return false;
    };
    generation.len() == 16
        && generation.bytes().all(|value| value.is_ascii_hexdigit())
        && number.strip_suffix(".zip").is_some_and(|value| {
            value.len() == 5 && value.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub async fn export_all_multipart_internal(
    state: Arc<AppState>,
    manifest_path: String,
) -> Result<(), String> {
    let _library_snapshot_guard = state.library_gate.clone().read_owned().await;
    let destination = PathBuf::from(&manifest_path);
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err("Multipart backup destination directory does not exist".to_string());
    }
    let stem = safe_export_name(
        destination
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("piep-backup"),
    );
    let generation = format!("{:016x}", rand::random::<u64>());
    let previous_manifest = std::fs::read(&destination)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<MultipartBackupManifest>(&bytes).ok());
    let mut cleanup = MultipartPartsGuard {
        paths: Vec::new(),
        armed: true,
    };
    let mut manifest = MultipartBackupManifest {
        format: "piep-multipart-1".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        total_works: 0,
        parts: Vec::new(),
    };
    let mut params = all_backup_search_params();
    let mut seen_ids = HashSet::new();
    let mut expected_total = None;
    let mut pending_ids = Vec::with_capacity(MULTIPART_WORKS_PER_PART);
    let mut part_number = 0usize;

    loop {
        let mut page = state
            .db
            .search_downloads_v2_internal(&params, BACKUP_PAGE_SIZE)?;
        if let Some(total) = page.total_estimate {
            if expected_total.is_some_and(|expected| expected != total) {
                return Err("Library changed while multipart backup was being prepared".to_string());
            }
            expected_total = Some(total);
        }
        for item in page.items.drain(..) {
            if !seen_ids.insert(item.id) {
                return Err("Multipart backup paging returned a duplicate work".to_string());
            }
            pending_ids.push(item.id);
            manifest.total_works += 1;
            if pending_ids.len() == MULTIPART_WORKS_PER_PART {
                export_multipart_batches(
                    &state,
                    parent,
                    &stem,
                    &generation,
                    &mut part_number,
                    std::mem::take(&mut pending_ids),
                    &mut manifest,
                    &mut cleanup,
                )
                .await?;
            }
        }
        let Some(cursor) = page.next_cursor else {
            break;
        };
        params.cursor = Some(cursor);
    }
    if !pending_ids.is_empty() {
        export_multipart_batches(
            &state,
            parent,
            &stem,
            &generation,
            &mut part_number,
            pending_ids,
            &mut manifest,
            &mut cleanup,
        )
        .await?;
    }

    // Profiles are library-owned records and can outlive their final work.
    // Export every person/series exactly once in dedicated catalog parts so
    // orphaned profiles are preserved and prolific entities are not copied
    // into every work part that happens to reference them.
    let mut catalog_written = false;
    let mut person_cursor: Option<(String, String)> = None;
    loop {
        let keys = state.db.list_people_keys_after(
            person_cursor
                .as_ref()
                .map(|(source, key)| (source.as_str(), key.as_str())),
            MULTIPART_ENTITIES_PER_PART,
        )?;
        if keys.is_empty() {
            break;
        }
        person_cursor = keys.last().cloned();
        export_multipart_catalog_batches(
            &state,
            parent,
            &stem,
            &generation,
            &mut part_number,
            keys,
            Vec::new(),
            &mut catalog_written,
            &mut manifest,
            &mut cleanup,
        )
        .await?;
    }
    let mut series_cursor: Option<(String, String)> = None;
    loop {
        let keys = state.db.list_series_keys_after(
            series_cursor
                .as_ref()
                .map(|(source, key)| (source.as_str(), key.as_str())),
            MULTIPART_ENTITIES_PER_PART,
        )?;
        if keys.is_empty() {
            break;
        }
        series_cursor = keys.last().cloned();
        export_multipart_catalog_batches(
            &state,
            parent,
            &stem,
            &generation,
            &mut part_number,
            Vec::new(),
            keys,
            &mut catalog_written,
            &mut manifest,
            &mut cleanup,
        )
        .await?;
    }
    let mut update_target_cursor: Option<(String, String, String)> = None;
    loop {
        let targets = state.db.list_update_targets_after(
            update_target_cursor
                .as_ref()
                .map(|(kind, source, key)| (kind.as_str(), source.as_str(), key.as_str())),
            MULTIPART_ENTITIES_PER_PART,
        )?;
        if targets.is_empty() {
            break;
        }
        update_target_cursor = targets.last().map(|target| {
            (
                target.target_type.clone(),
                target.source.clone(),
                target.source_key.clone(),
            )
        });
        export_multipart_update_target_batches(
            &state,
            parent,
            &stem,
            &generation,
            &mut part_number,
            targets,
            &mut catalog_written,
            &mut manifest,
            &mut cleanup,
        )
        .await?;
    }
    let mut update_candidate_cursor: Option<(String, String)> = None;
    loop {
        let candidates = state.db.list_update_candidates_after(
            update_candidate_cursor
                .as_ref()
                .map(|(source, source_id)| (source.as_str(), source_id.as_str())),
            MULTIPART_ENTITIES_PER_PART,
        )?;
        if candidates.is_empty() {
            break;
        }
        update_candidate_cursor = candidates
            .last()
            .map(|candidate| (candidate.source.clone(), candidate.source_id.clone()));
        export_multipart_update_candidate_batches(
            &state,
            parent,
            &stem,
            &generation,
            &mut part_number,
            candidates,
            &mut catalog_written,
            &mut manifest,
            &mut cleanup,
        )
        .await?;
    }
    if !catalog_written {
        // A library can legitimately have update targets before its first
        // work or profile is saved. Preserve that catalog in a metadata-only
        // ZIP rather than publishing a manifest that silently loses it.
        export_multipart_part(
            &state,
            parent,
            &stem,
            &generation,
            part_number.saturating_add(1),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &mut manifest,
            &mut cleanup,
        )
        .await?;
    }
    if expected_total.is_some_and(|expected| expected != manifest.total_works) {
        return Err("Multipart backup scope was incomplete".to_string());
    }
    write_multipart_manifest_atomically(&destination, &manifest)?;
    cleanup.armed = false;

    if let Some(previous) = previous_manifest.filter(|value| value.format == "piep-multipart-1") {
        let current = manifest
            .parts
            .iter()
            .map(|part| part.file.as_str())
            .collect::<HashSet<_>>();
        for old in previous.parts {
            let is_owned_part = is_owned_multipart_part_name(&stem, &old.file)
                && validated_backup_relative_path(&old.file)
                    .is_ok_and(|path| path.components().count() == 1);
            if is_owned_part && !current.contains(old.file.as_str()) {
                let path = parent.join(&old.file);
                if path.parent() == Some(parent) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn export_multipart_batches(
    state: &Arc<AppState>,
    parent: &Path,
    stem: &str,
    generation: &str,
    part_number: &mut usize,
    ids: Vec<i64>,
    manifest: &mut MultipartBackupManifest,
    cleanup: &mut MultipartPartsGuard,
) -> Result<(), String> {
    let mut batches = VecDeque::from([ids]);
    while let Some(ids) = batches.pop_front() {
        let next_part = part_number.saturating_add(1);
        match export_multipart_part(
            state,
            parent,
            stem,
            generation,
            next_part,
            ids.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            manifest,
            cleanup,
        )
        .await
        {
            Ok(()) => *part_number = next_part,
            Err(error)
                if ids.len() > 1
                    && (error.contains("entry count is outside the allowed quota")
                        || error.contains("expanded-size quota")
                        || error.contains("metadata exceeds its size quota")) =>
            {
                let midpoint = ids.len() / 2;
                let right = ids[midpoint..].to_vec();
                let left = ids[..midpoint].to_vec();
                batches.push_front(right);
                batches.push_front(left);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn export_multipart_catalog_batches(
    state: &Arc<AppState>,
    parent: &Path,
    stem: &str,
    generation: &str,
    part_number: &mut usize,
    people: Vec<(String, String)>,
    series: Vec<(String, String)>,
    catalog_written: &mut bool,
    manifest: &mut MultipartBackupManifest,
    cleanup: &mut MultipartPartsGuard,
) -> Result<(), String> {
    let mut batches = VecDeque::from([(people, series)]);
    while let Some((people, series)) = batches.pop_front() {
        let next_part = part_number.saturating_add(1);
        match export_multipart_part(
            state,
            parent,
            stem,
            generation,
            next_part,
            Vec::new(),
            people.clone(),
            series.clone(),
            Vec::new(),
            Vec::new(),
            manifest,
            cleanup,
        )
        .await
        {
            Ok(()) => {
                *part_number = next_part;
                *catalog_written = true;
            }
            Err(error)
                if people.len().saturating_add(series.len()) > 1
                    && (error.contains("entry count is outside the allowed quota")
                        || error.contains("expanded-size quota")
                        || error.contains("metadata exceeds its size quota")) =>
            {
                if people.len() >= series.len() && people.len() > 1 {
                    let midpoint = people.len() / 2;
                    batches.push_front((people[midpoint..].to_vec(), series.clone()));
                    batches.push_front((people[..midpoint].to_vec(), Vec::new()));
                } else if series.len() > 1 {
                    let midpoint = series.len() / 2;
                    batches.push_front((Vec::new(), series[midpoint..].to_vec()));
                    batches.push_front((people, series[..midpoint].to_vec()));
                } else {
                    batches.push_front((Vec::new(), series));
                    batches.push_front((people, Vec::new()));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn export_multipart_update_target_batches(
    state: &Arc<AppState>,
    parent: &Path,
    stem: &str,
    generation: &str,
    part_number: &mut usize,
    targets: Vec<UpdateTarget>,
    catalog_written: &mut bool,
    manifest: &mut MultipartBackupManifest,
    cleanup: &mut MultipartPartsGuard,
) -> Result<(), String> {
    let mut batches = VecDeque::from([targets]);
    while let Some(targets) = batches.pop_front() {
        let next_part = part_number.saturating_add(1);
        match export_multipart_part(
            state,
            parent,
            stem,
            generation,
            next_part,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            targets.clone(),
            Vec::new(),
            manifest,
            cleanup,
        )
        .await
        {
            Ok(()) => {
                *part_number = next_part;
                *catalog_written = true;
            }
            Err(error)
                if targets.len() > 1
                    && (error.contains("entry count is outside the allowed quota")
                        || error.contains("expanded-size quota")
                        || error.contains("metadata exceeds its size quota")) =>
            {
                let midpoint = targets.len() / 2;
                batches.push_front(targets[midpoint..].to_vec());
                batches.push_front(targets[..midpoint].to_vec());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn export_multipart_update_candidate_batches(
    state: &Arc<AppState>,
    parent: &Path,
    stem: &str,
    generation: &str,
    part_number: &mut usize,
    candidates: Vec<UpdateCandidateRow>,
    catalog_written: &mut bool,
    manifest: &mut MultipartBackupManifest,
    cleanup: &mut MultipartPartsGuard,
) -> Result<(), String> {
    let mut batches = VecDeque::from([candidates]);
    while let Some(candidates) = batches.pop_front() {
        let next_part = part_number.saturating_add(1);
        match export_multipart_part(
            state,
            parent,
            stem,
            generation,
            next_part,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            candidates.clone(),
            manifest,
            cleanup,
        )
        .await
        {
            Ok(()) => {
                *part_number = next_part;
                *catalog_written = true;
            }
            Err(error)
                if candidates.len() > 1
                    && (error.contains("entry count is outside the allowed quota")
                        || error.contains("expanded-size quota")
                        || error.contains("metadata exceeds its size quota")) =>
            {
                let midpoint = candidates.len() / 2;
                batches.push_front(candidates[midpoint..].to_vec());
                batches.push_front(candidates[..midpoint].to_vec());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn export_multipart_part(
    state: &Arc<AppState>,
    parent: &Path,
    stem: &str,
    generation: &str,
    part_number: usize,
    ids: Vec<i64>,
    people: Vec<(String, String)>,
    series: Vec<(String, String)>,
    update_targets: Vec<UpdateTarget>,
    update_candidates: Vec<UpdateCandidateRow>,
    manifest: &mut MultipartBackupManifest,
    cleanup: &mut MultipartPartsGuard,
) -> Result<(), String> {
    if part_number > MAX_MULTIPART_PARTS {
        return Err("Multipart backup exceeded the 2,048-part safety limit".to_string());
    }
    let file = format!("{stem}-{generation}.part-{part_number:05}.zip");
    let path = parent.join(&file);
    if path.exists() {
        return Err(format!(
            "Refusing to overwrite an unexpected multipart part: {file}"
        ));
    }
    let mut params = all_backup_search_params();
    params.ids_include = Some(if ids.is_empty() {
        // Empty means "no filter" in the public search model. Use an invalid
        // sentinel id to deliberately select zero works for a catalog part.
        vec![-1]
    } else {
        ids.clone()
    });
    export_zip_with_params_locked(
        state.clone(),
        path.to_string_lossy().to_string(),
        params,
        true,
        false,
        false,
        people,
        series,
        update_targets,
        update_candidates,
    )
    .await?;
    cleanup.paths.push(path.clone());
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("Could not inspect multipart part: {error}"))?;
    let sha256 = sha256_file(&path)?;
    manifest.parts.push(MultipartBackupPart {
        file,
        work_count: ids.len() as i64,
        bytes: metadata.len(),
        sha256,
    });
    Ok(())
}

pub async fn export_zip_with_params_internal(
    state: Arc<AppState>,
    zip_path: String,
    params: SearchV2Params,
    scoped: bool,
) -> Result<(), String> {
    let _library_snapshot_guard = state.library_gate.clone().read_owned().await;
    export_zip_with_params_locked(
        state,
        zip_path,
        params,
        scoped,
        true,
        true,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn export_zip_with_params_locked(
    state: Arc<AppState>,
    zip_path: String,
    params: SearchV2Params,
    scoped: bool,
    include_scoped_update_targets: bool,
    include_related_entities: bool,
    extra_people: Vec<(String, String)>,
    extra_series: Vec<(String, String)>,
    extra_update_targets: Vec<UpdateTarget>,
    extra_update_candidates: Vec<UpdateCandidateRow>,
) -> Result<(), String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();

    let all = collect_backup_downloads(&state, params)?;

    tokio::task::spawn_blocking(move || {
        let destination = PathBuf::from(&zip_path);
        let (file, mut temp_guard) = create_export_temp(&destination)?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut written_files = HashSet::new();

        let mut backup_entries = Vec::new();
        let mut download_scope = HashSet::new();
        let mut person_scope = HashSet::new();
        let mut series_scope = HashSet::new();

        for dl in &all {
            download_scope.insert((dl.source.clone(), dl.source_id.clone()));
            let versions = state.db.get_versions(dl.id)?;
            let assets = state.db.get_assets(dl.id)?;
            let tag_sources = state.db.archive_tags(dl.id)?;
            let mut work_edits = state.db.archive_work_edits(dl.id)?;
            let relations = state.db.get_download_relations_for_download(dl.id)?;
            let people = state.db.get_download_people(dl.id)?;
            let series = state.db.get_download_series_list(dl.id)?;
            for person in &people {
                person_scope.insert((person.person_source.clone(), person.person_key.clone()));
            }
            for item in &series {
                series_scope.insert((item.series_source.clone(), item.series_key.clone()));
            }

            // The main download row is the authoritative current payload. It
            // can exist without a matching history row in migrated libraries,
            // so preserve and validate it independently of `download_versions`.
            let current_relative_json_path = required_export_relative_path(
                &dl.json_path,
                &storage,
                &format!("work {}/{} current metadata", dl.source, dl.source_id),
            )?;
            let current_relative_original_json_path = dl
                .original_json_path
                .as_deref()
                .map(|path| {
                    required_export_relative_path(
                        path,
                        &storage,
                        &format!(
                            "work {}/{} current original metadata",
                            dl.source, dl.source_id
                        ),
                    )
                })
                .transpose()?;
            add_zip_file_once(
                &mut zip,
                options,
                &mut written_files,
                &current_relative_json_path,
                Path::new(&dl.json_path),
            )?;
            if let (Some(path), Some(relative)) = (
                dl.original_json_path.as_deref(),
                current_relative_original_json_path.as_deref(),
            ) {
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    relative,
                    Path::new(path),
                )?;
            }

            let mut backup_versions = Vec::new();
            for v in &versions {
                let relative_json_path = required_export_relative_path(
                    &v.json_path,
                    &storage,
                    &format!(
                        "work {}/{} version {} metadata",
                        dl.source, dl.source_id, v.version
                    ),
                )?;
                let relative_original_json_path = v
                    .original_json_path
                    .as_deref()
                    .map(|path| {
                        required_export_relative_path(
                            path,
                            &storage,
                            &format!(
                                "work {}/{} version {} original metadata",
                                dl.source, dl.source_id, v.version
                            ),
                        )
                    })
                    .transpose()?;

                // ZIPへの書き込み
                // data.json (legacy - only if it is different from original.json to prevent duplicate entries)
                let paths_are_same = v
                    .original_json_path
                    .as_ref()
                    .map(|p| p == &v.json_path)
                    .unwrap_or(false);

                if !paths_are_same {
                    add_zip_file_once(
                        &mut zip,
                        options,
                        &mut written_files,
                        &relative_json_path,
                        Path::new(&v.json_path),
                    )?;
                }

                // original.json
                if let (Some(path), Some(relative)) = (
                    v.original_json_path.as_deref(),
                    relative_original_json_path.as_deref(),
                ) {
                    add_zip_file_once(
                        &mut zip,
                        options,
                        &mut written_files,
                        relative,
                        Path::new(path),
                    )?;
                }

                let is_current = v.version == dl.current_version;
                backup_versions.push(BackupVersion {
                    version: v.version,
                    content_hash: v.content_hash.clone(),
                    text_length: v.text_length,
                    file_size_bytes: v.file_size_bytes,
                    created_at: v.created_at.clone(),
                    change_summary: v.change_summary.clone(),
                    relative_json_path: if is_current {
                        current_relative_json_path.clone()
                    } else {
                        relative_json_path
                    },
                    relative_original_json_path: if is_current {
                        current_relative_original_json_path.clone()
                    } else {
                        relative_original_json_path
                    },
                });
            }
            if !backup_versions
                .iter()
                .any(|version| version.version == dl.current_version)
            {
                backup_versions.push(BackupVersion {
                    version: dl.current_version,
                    content_hash: dl.content_hash.clone(),
                    text_length: dl.text_length,
                    file_size_bytes: dl.file_size_bytes,
                    created_at: dl.downloaded_at.clone(),
                    change_summary: Some("Recovered from current library record".to_string()),
                    relative_json_path: current_relative_json_path,
                    relative_original_json_path: current_relative_original_json_path,
                });
            }

            let mut backup_assets = Vec::new();
            for asset in &assets {
                let relative_local_path = required_export_relative_path(
                    &asset.local_path,
                    &storage,
                    &format!(
                        "work {}/{} asset {}",
                        dl.source, dl.source_id, asset.filename
                    ),
                )?;

                // ZIPへの書き込み
                let src = Path::new(&asset.local_path);
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    &relative_local_path,
                    src,
                )?;

                backup_assets.push(BackupAsset {
                    asset_type: asset.asset_type.clone(),
                    filename: asset.filename.clone(),
                    original_url: asset.original_url.clone(),
                    mime_type: asset.mime_type.clone(),
                    file_size_bytes: asset.file_size_bytes,
                    relative_local_path,
                });
            }

            for revision in &mut work_edits {
                for block in &mut revision.blocks {
                    if let Some(path) = block.asset_path.as_deref() {
                        block.asset_path = Some(required_export_relative_path(
                            path,
                            &storage,
                            &format!("work {}/{} edit asset", dl.source, dl.source_id),
                        )?);
                    }
                }
            }

            let relative_cover_path = dl
                .cover_path
                .as_deref()
                .map(|path| {
                    required_export_relative_path(
                        path,
                        &storage,
                        &format!("work {}/{} cover", dl.source, dl.source_id),
                    )
                })
                .transpose()?;
            if let (Some(path), Some(relative)) = (&dl.cover_path, &relative_cover_path) {
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    relative,
                    Path::new(path),
                )?;
            }

            backup_entries.push(BackupEntry {
                source: dl.source.clone(),
                source_id: dl.source_id.clone(),
                title: dl.title.clone(),
                author_name: dl.author_name.clone(),
                author_id: dl.author_id.clone(),
                content_type: dl.content_type.clone(),
                tags: dl.tags.clone(),
                tag_sources,
                excerpt: dl.excerpt.clone(),
                relative_cover_path,
                watch_updates: dl.watch_updates,
                favorite: dl.favorite,
                downloaded_at: dl.downloaded_at.clone(),
                source_created_at: dl.source_created_at.clone(),
                source_updated_at: dl.source_updated_at.clone(),
                current_version: dl.current_version,
                versions: backup_versions,
                assets: backup_assets,
                relations: relations
                    .into_iter()
                    .map(|r| BackupDownloadRelation {
                        relation_type: r.relation_type,
                        source: r.source,
                        relation_id: r.relation_id,
                        relation_name: r.relation_name,
                    })
                    .collect(),
                people: people
                    .into_iter()
                    .map(|p| BackupDownloadPerson {
                        person_source: p.person_source,
                        person_key: p.person_key,
                        role: p.role,
                        display_name: p.display_name,
                    })
                    .collect(),
                series: series
                    .into_iter()
                    .map(|s| BackupDownloadSeries {
                        series_source: s.series_source,
                        series_key: s.series_key,
                        title: s.title,
                        content_order: s.content_order,
                    })
                    .collect(),
                work_edits,
            });
        }

        if !include_related_entities {
            person_scope.clear();
            series_scope.clear();
        }
        person_scope.extend(extra_people);
        series_scope.extend(extra_series);

        let people_to_export = if scoped {
            let mut keys = person_scope.iter().cloned().collect::<Vec<_>>();
            keys.sort();
            keys.into_iter()
                .map(|(source, source_key)| state.db.get_person(&source, &source_key))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            state.db.list_people()?
        };
        let mut backup_people = Vec::new();
        for person in people_to_export {
            let mut versions = Vec::new();
            for version in
                state
                    .db
                    .list_entity_versions("person", &person.source, &person.source_key)?
            {
                let relative_json_path = required_export_relative_path(
                    &version.json_path,
                    &app_data,
                    &format!(
                        "person {}/{} version {} metadata",
                        person.source, person.source_key, version.version
                    ),
                )?;
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    &relative_json_path,
                    Path::new(&version.json_path),
                )?;
                versions.push(BackupEntityVersion {
                    entity_type: version.entity_type,
                    source: version.source,
                    source_key: version.source_key,
                    version: version.version,
                    content_hash: version.content_hash,
                    relative_json_path,
                    asset_count: version.asset_count,
                    file_size_bytes: version.file_size_bytes,
                    created_at: version.created_at,
                    change_summary: version.change_summary,
                });
            }

            let relative_icon_path = person
                .icon_path
                .as_deref()
                .map(|path| {
                    required_export_relative_path(
                        path,
                        &app_data,
                        &format!("person {}/{} icon", person.source, person.source_key),
                    )
                })
                .transpose()?;
            if let (Some(path), Some(relative)) = (&person.icon_path, &relative_icon_path) {
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    relative,
                    Path::new(path),
                )?;
            }

            let relative_cover_path = person
                .cover_path
                .as_deref()
                .map(|path| {
                    required_export_relative_path(
                        path,
                        &app_data,
                        &format!("person {}/{} cover", person.source, person.source_key),
                    )
                })
                .transpose()?;
            if let (Some(path), Some(relative)) = (&person.cover_path, &relative_cover_path) {
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    relative,
                    Path::new(path),
                )?;
            }

            backup_people.push(BackupPerson {
                source: person.source,
                source_key: person.source_key,
                display_name: person.display_name,
                relative_icon_path,
                relative_cover_path,
                description: person.description,
                links_json: person.links_json,
                content_hash: person.content_hash,
                current_version: person.current_version,
                last_checked_at: person.last_checked_at,
                last_fetched_at: person.last_fetched_at,
                created_at: person.created_at,
                updated_at: person.updated_at,
                versions,
            });
        }

        let series_to_export = if scoped {
            let mut keys = series_scope.iter().cloned().collect::<Vec<_>>();
            keys.sort();
            keys.into_iter()
                .map(|(source, source_key)| state.db.get_series(&source, &source_key))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            state.db.list_series()?
        };
        let mut backup_series = Vec::new();
        for series in series_to_export {
            let mut versions = Vec::new();
            for version in
                state
                    .db
                    .list_entity_versions("series", &series.source, &series.source_key)?
            {
                let relative_json_path = required_export_relative_path(
                    &version.json_path,
                    &app_data,
                    &format!(
                        "series {}/{} version {} metadata",
                        series.source, series.source_key, version.version
                    ),
                )?;
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    &relative_json_path,
                    Path::new(&version.json_path),
                )?;
                versions.push(BackupEntityVersion {
                    entity_type: version.entity_type,
                    source: version.source,
                    source_key: version.source_key,
                    version: version.version,
                    content_hash: version.content_hash,
                    relative_json_path,
                    asset_count: version.asset_count,
                    file_size_bytes: version.file_size_bytes,
                    created_at: version.created_at,
                    change_summary: version.change_summary,
                });
            }

            let relative_cover_path = series
                .cover_path
                .as_deref()
                .map(|path| {
                    required_export_relative_path(
                        path,
                        &app_data,
                        &format!("series {}/{} cover", series.source, series.source_key),
                    )
                })
                .transpose()?;
            if let (Some(path), Some(relative)) = (&series.cover_path, &relative_cover_path) {
                add_zip_file_once(
                    &mut zip,
                    options,
                    &mut written_files,
                    relative,
                    Path::new(path),
                )?;
            }

            backup_series.push(BackupSeries {
                source: series.source,
                source_key: series.source_key,
                title: series.title,
                description: series.description,
                relative_cover_path,
                content_hash: series.content_hash,
                current_version: series.current_version,
                last_checked_at: series.last_checked_at,
                last_fetched_at: series.last_fetched_at,
                created_at: series.created_at,
                updated_at: series.updated_at,
                versions,
            });
        }

        // メタデータJSONの作成とZIPへの書き込み
        let mut update_targets = extra_update_targets;
        if include_scoped_update_targets {
            update_targets.extend(state.db.list_update_targets(None, false)?);
        }
        if scoped && include_scoped_update_targets {
            update_targets.retain(|target| match target.target_type.as_str() {
                "work" => {
                    download_scope.contains(&(target.source.clone(), target.source_key.clone()))
                }
                "author" | "person" => {
                    person_scope.contains(&(target.source.clone(), target.source_key.clone()))
                }
                "series" => {
                    series_scope.contains(&(target.source.clone(), target.source_key.clone()))
                }
                _ => false,
            });
        }

        let update_candidates = if include_scoped_update_targets {
            let mut candidates = Vec::new();
            let mut cursor: Option<(String, String)> = None;
            loop {
                let page = state.db.list_update_candidates_after(
                    cursor
                        .as_ref()
                        .map(|(source, source_id)| (source.as_str(), source_id.as_str())),
                    MULTIPART_ENTITIES_PER_PART,
                )?;
                if page.is_empty() {
                    break;
                }
                cursor = page
                    .last()
                    .map(|candidate| (candidate.source.clone(), candidate.source_id.clone()));
                candidates.extend(page);
            }
            candidates
        } else {
            extra_update_candidates
        };

        let mut backup_collections = Vec::new();
        for summary in state.db.list_work_collections()? {
            let collection = state.db.get_work_collection(&summary.id)?;
            let cover_work = collection
                .summary
                .cover_download_id
                .and_then(|cover_id| {
                    collection
                        .members
                        .iter()
                        .find(|member| member.download_id == Some(cover_id))
                })
                .map(|member| WorkKey {
                    source: member.source.clone(),
                    source_id: member.source_id.clone(),
                });
            let members = collection
                .members
                .iter()
                .filter(|member| {
                    !scoped
                        || download_scope
                            .contains(&(member.source.clone(), member.source_id.clone()))
                })
                .map(|member| BackupWorkCollectionMember {
                    source: member.source.clone(),
                    source_id: member.source_id.clone(),
                    title_snapshot: member.title.clone(),
                    author_snapshot: member.author_name.clone(),
                    position: member.position,
                    member_role: member.member_role.clone(),
                    added_by: member.added_by.clone(),
                    pinned: member.pinned,
                    note: member.note.clone(),
                })
                .collect::<Vec<_>>();
            // **束そのものは、目録の側で必ず書き出す。**
            //
            // 作品ごとに分けたパートでは、その作品を含まない束のメンバーは空に
            // なる。それを理由に飛ばしていたので、まだ一作も入れていない束
            // （名前と説明と表紙だけ作った束）は、どのパートの条件にも当たらず
            // 一度も書き出されなかった。復元しても戻らず、警告も出ない。
            // 目録のパート（更新監視と候補を持つ側）だけは、中身が空でも書く。
            // 復元側の `restore_work_collection` は upsert なので、定義と
            // メンバーが別のパートに分かれていても安全に合流する。
            if scoped && members.is_empty() && !include_scoped_update_targets {
                continue;
            }
            let relative_cover_image_path = collection
                .summary
                .cover_image_path
                .as_deref()
                .filter(|_| collection.summary.cover_mode == "file")
                .map(|source| {
                    let source_path = Path::new(source);
                    let extension = source_path
                        .extension()
                        .and_then(|value| value.to_str())
                        .map(|value| value.to_ascii_lowercase())
                        .filter(|value| {
                            matches!(
                                value.as_str(),
                                "png" | "jpg" | "jpeg" | "webp" | "avif" | "gif"
                            )
                        })
                        .unwrap_or_else(|| "img".to_string());
                    let relative = format!(
                        "collection-covers/{}/cover.{extension}",
                        safe_export_name(&collection.summary.id)
                    );
                    validated_backup_relative_path(&relative)?;
                    add_zip_file_once(
                        &mut zip,
                        options,
                        &mut written_files,
                        &relative,
                        source_path,
                    )?;
                    Ok::<_, String>(relative)
                })
                .transpose()?;
            backup_collections.push(BackupWorkCollection {
                id: collection.summary.id,
                name: collection.summary.name,
                description: collection.summary.description,
                collection_kind: collection.summary.collection_kind,
                cover_work,
                cover_mode: Some(collection.summary.cover_mode),
                relative_cover_image_path,
                name_source: Some(collection.summary.name_source),
                track: Some(collection.summary.track),
                members,
            });
        }

        let saved_searches = state.db.list_saved_searches()?;
        // Pair feedback is keyed by stable provider ids and can span multipart boundaries.
        // Repeating this small idempotent set in each part is safer than silently losing every
        // decision whose two works happened to land in different parts.
        let collection_pair_feedback = state.db.archive_collection_pair_feedback()?;

        let metadata = BackupMetadata {
            version: "3.0".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            entries: backup_entries,
            people: backup_people,
            series: backup_series,
            update_targets,
            update_candidates,
            collections: backup_collections,
            saved_searches,
            collection_pair_feedback,
        };

        let metadata_json = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())?;
        zip.start_file("backup_metadata.json", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(metadata_json.as_bytes())
            .map_err(|e| e.to_string())?;

        let completed_file = zip.finish().map_err(|e| e.to_string())?;
        completed_file
            .sync_all()
            .map_err(|error| format!("Could not sync completed backup: {error}"))?;
        drop(completed_file);

        validate_finished_export(&temp_guard.path, &storage, &app_data)?;
        atomic_replace_file(&temp_guard.path, &destination)?;
        temp_guard.disarm();
        if let Err(error) = sync_export_parent(&destination) {
            // The atomic replacement has already succeeded, so reporting a
            // failure here would invite a retry even though the new backup is
            // complete. Directory fsync is unavailable on some filesystems;
            // the ZIP itself was synchronously flushed before promotion.
            log::warn!("{error}");
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("Export thread panicked: {}", e))?
}

#[tauri::command]
pub async fn export_all_multipart(
    app: tauri::AppHandle,
    manifest_path: String,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    export_all_multipart_internal(state, manifest_path).await
}

#[tauri::command]
pub async fn export_entity_zip(
    app: tauri::AppHandle,
    entity_type: String,
    source: String,
    source_key: String,
    zip_path: String,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let mut params = all_backup_search_params();
    match entity_type.as_str() {
        "person" | "author" => {
            params.person_source = Some(source);
            params.person_key = Some(source_key);
            params.sort_by = Some("published".to_string());
            params.sort_order = Some("desc".to_string());
        }
        "series" => {
            params.series_source = Some(source);
            params.series_key = Some(source_key);
            params.sort_by = Some("series_order".to_string());
            params.sort_order = Some("asc".to_string());
        }
        _ => return Err("Unsupported entity type for backup".to_string()),
    }
    export_zip_with_params_internal(state, zip_path, params, true).await
}

#[tauri::command]
pub async fn import_zip(app: tauri::AppHandle, zip_path: String) -> Result<i64, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let reporter = ArchiveReporter::register(app, new_archive_job_id());
    let result = import_zip_with_reporter(state, zip_path, &reporter).await;
    reporter.finish();
    result
}

/// 走っている復元を止める。
///
/// 一度に走るのは一つ（`IMPORT_LOCK` とライブラリの錠がそれを保証する）なので、
/// 番号を渡してもらう必要はない。**止まるのはライブラリを書き換える前まで**で、
/// 書き換えが始まったあとは途中で降りるほうが危ない。
#[tauri::command]
pub async fn cancel_archive_restore() -> Result<bool, String> {
    let jobs = ARCHIVE_JOBS.lock().map_err(|e| e.to_string())?;
    let mut stopped = false;
    for cancel in jobs.values() {
        cancel.store(true, Ordering::Relaxed);
        stopped = true;
    }
    Ok(stopped)
}

fn new_archive_job_id() -> String {
    format!("archive-{}-{}", std::process::id(), rand::random::<u64>())
}

#[tauri::command]
pub async fn import_multipart_backup(
    app: tauri::AppHandle,
    manifest_path: String,
) -> Result<i64, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let reporter = ArchiveReporter::register(app, new_archive_job_id());
    let result = import_multipart_with_reporter(state, manifest_path, &reporter).await;
    reporter.finish();
    result
}

#[tauri::command]
pub async fn inspect_backup(
    app: tauri::AppHandle,
    zip_path: String,
) -> Result<BackupInspection, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();
    tokio::task::spawn_blocking(move || inspect_backup_internal(&zip_path, &storage, &app_data))
        .await
        .map_err(|e| format!("Backup inspection thread panicked: {e}"))?
}

#[tauri::command]
pub async fn inspect_multipart_backup(
    app: tauri::AppHandle,
    manifest_path: String,
) -> Result<BackupInspection, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();
    tokio::task::spawn_blocking(move || {
        inspect_multipart_backup_internal(Path::new(&manifest_path), &storage, &app_data)
    })
    .await
    .map_err(|error| format!("Multipart inspection thread panicked: {error}"))?
}

fn inspect_backup_internal(
    zip_path: &str,
    storage: &Path,
    app_data: &Path,
) -> Result<BackupInspection, String> {
    let compressed_bytes = std::fs::metadata(zip_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let entry_count = archive.len();
    let expanded_bytes = match preflight_backup_archive(&mut archive) {
        Ok(size) => size,
        Err(error) => {
            return Ok(BackupInspection {
                valid: false,
                error: Some(error),
                backup_version: None,
                entry_count,
                compressed_bytes,
                expanded_bytes: 0,
                required_free_bytes: 0,
                available_free_bytes: crate::database::queries::available_space_bytes(app_data),
                work_count: 0,
                person_count: 0,
                series_count: 0,
                version_count: 0,
                asset_count: 0,
                warnings: Vec::new(),
            });
        }
    };
    let mut archive_names = HashSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|e| e.to_string())?;
        if !file.is_dir() {
            archive_names.insert(file.name().to_string());
        }
    }
    let mut metadata_text = String::new();
    archive
        .by_name("backup_metadata.json")
        .map_err(|e| e.to_string())?
        .take(MAX_BACKUP_METADATA_BYTES + 1)
        .read_to_string(&mut metadata_text)
        .map_err(|e| e.to_string())?;
    let metadata: BackupMetadata = match serde_json::from_str(&metadata_text) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(BackupInspection {
                valid: false,
                error: Some(format!("Backup metadata is invalid: {error}")),
                backup_version: None,
                entry_count,
                compressed_bytes,
                expanded_bytes,
                required_free_bytes: 0,
                available_free_bytes: crate::database::queries::available_space_bytes(app_data),
                work_count: 0,
                person_count: 0,
                series_count: 0,
                version_count: 0,
                asset_count: 0,
                warnings: Vec::new(),
            });
        }
    };
    if let Err(error) = validate_backup_metadata_paths(&metadata, storage, app_data) {
        return Ok(BackupInspection {
            valid: false,
            error: Some(format!("Invalid backup metadata: {error}")),
            backup_version: Some(metadata.version),
            entry_count,
            compressed_bytes,
            expanded_bytes,
            required_free_bytes: 0,
            available_free_bytes: crate::database::queries::available_space_bytes(app_data),
            work_count: metadata.entries.len(),
            person_count: metadata.people.len(),
            series_count: metadata.series.len(),
            version_count: 0,
            asset_count: 0,
            warnings: Vec::new(),
        });
    }
    let version_count = metadata
        .entries
        .iter()
        .map(|entry| entry.versions.len())
        .sum::<usize>()
        + metadata
            .people
            .iter()
            .map(|entry| entry.versions.len())
            .sum::<usize>()
        + metadata
            .series
            .iter()
            .map(|entry| entry.versions.len())
            .sum::<usize>();
    let asset_count = metadata
        .entries
        .iter()
        .map(|entry| entry.assets.len())
        .sum();
    let required_free_bytes = expanded_bytes
        .saturating_add(expanded_bytes / 10)
        .saturating_add(256 * 1024 * 1024);
    let available_free_bytes = crate::database::queries::available_space_bytes(app_data);
    let mut warnings = Vec::new();
    if metadata.version != "3.0" {
        warnings.push(format!(
            "This backup uses format {}; the current format is 3.0",
            metadata.version
        ));
    }
    let reference_error = match referenced_backup_paths(&metadata) {
        Ok(paths) => paths
            .into_iter()
            .find(|path| !archive_names.contains(path))
            .map(|path| format!("Backup is missing a referenced file: {path}")),
        Err(error) => Some(error),
    };
    let disk_error = available_free_bytes
        .filter(|available| *available < required_free_bytes)
        .map(|available| {
            format!(
                "Not enough free disk space for staging (required {required_free_bytes} bytes, available {available} bytes)"
            )
        });
    let error = reference_error.or(disk_error);
    Ok(BackupInspection {
        valid: error.is_none(),
        error,
        backup_version: Some(metadata.version),
        entry_count,
        compressed_bytes,
        expanded_bytes,
        required_free_bytes,
        available_free_bytes,
        work_count: metadata.entries.len(),
        person_count: metadata.people.len(),
        series_count: metadata.series.len(),
        version_count,
        asset_count,
        warnings,
    })
}

/// いま棚にあって、この書庫が上書きするファイルの合計サイズ。
///
/// 昇格の前に、置き換える相手を `stage_root/rollback/` へ丸ごと控える。つまり
/// 復元のピークは「展開ぶん」ではなく「展開ぶん + 控えぶん」である。事前検査が
/// 展開ぶんしか見ていなかったので、40GB の空きで 30GB の棚を戻し始め、
/// **何時間も展開したあとに**控えのコピーで力尽きる、ということが起きえた。
fn overwritten_bytes_for_backup(
    archive: &mut zip::ZipArchive<std::fs::File>,
    storage: &Path,
    app_data: &Path,
) -> u64 {
    let Ok(mut entry) = archive.by_name("backup_metadata.json") else {
        return 0;
    };
    let mut text = String::new();
    if (&mut entry)
        .take(MAX_BACKUP_METADATA_BYTES + 1)
        .read_to_string(&mut text)
        .is_err()
    {
        return 0;
    }
    drop(entry);
    let Ok(metadata) = serde_json::from_str::<BackupMetadata>(&text) else {
        return 0;
    };
    let Ok(paths) = referenced_backup_paths(&metadata) else {
        return 0;
    };
    let mut total = 0u64;
    for path in paths {
        let Ok((destination, _)) = resolve_zip_import_path(&path, storage, app_data) else {
            continue;
        };
        if let Ok(meta) = std::fs::metadata(&destination) {
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

pub async fn import_zip_internal(state: Arc<AppState>, zip_path: String) -> Result<i64, String> {
    import_zip_internal_with_failpoint(state, zip_path, None).await
}

async fn import_zip_with_reporter(
    state: Arc<AppState>,
    zip_path: String,
    reporter: &ArchiveReporter,
) -> Result<i64, String> {
    let _import_guard = IMPORT_LOCK.lock().await;
    let _library_write_guard = state.library_gate.clone().write_owned().await;
    import_zip_locked(state, zip_path, None, reporter).await
}

async fn import_zip_internal_with_failpoint(
    state: Arc<AppState>,
    zip_path: String,
    failpoint: Option<&'static str>,
) -> Result<i64, String> {
    let _import_guard = IMPORT_LOCK.lock().await;
    let _library_write_guard = state.library_gate.clone().write_owned().await;
    import_zip_locked(state, zip_path, failpoint, &ArchiveReporter::detached()).await
}

async fn import_zip_locked(
    state: Arc<AppState>,
    zip_path: String,
    failpoint: Option<&'static str>,
    reporter: &ArchiveReporter,
) -> Result<i64, String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();
    let reporter = reporter.clone();

    // プレミアム最適化：ZIP解凍からDB挿入までの全同期処理を、非同期ワーカースレッドへ完璧に移譲
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let expanded_bytes = preflight_backup_archive(&mut archive)?;
        // 控えのぶんも数える。読めなかったときは 0 になるので、少なくとも
        // 今までより厳しくなることはあっても緩くはならない。
        let rollback_bytes = overwritten_bytes_for_backup(&mut archive, &storage, &app_data);
        let required_free_bytes = expanded_bytes
            .saturating_add(rollback_bytes)
            .saturating_add(expanded_bytes / 10)
            .saturating_add(256 * 1024 * 1024);
        if let Some(available) = crate::database::queries::available_space_bytes(&app_data) {
            if available < required_free_bytes {
                return Err(format!(
                    "Not enough free disk space for staging (required {required_free_bytes} bytes, available {available} bytes)"
                ));
            }
        }
        let stage_root = app_data.join(format!(
            ".restore-staging-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let stage_storage = stage_root.join("downloads");
        let stage_app_data = stage_root.join("app-data");
        std::fs::create_dir_all(&stage_storage).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&stage_app_data).map_err(|e| e.to_string())?;
        let mut staging_cleanup = StagingGuard::new(stage_root.clone());

        // Fail before touching the live library if the staging quota is
        // obviously unreasonable for this installation. The hard quota above
        // remains the final protection on platforms without a free-space API.
        if expanded_bytes > MAX_BACKUP_TOTAL_BYTES {
            return Err("Backup staging quota exceeded".to_string());
        }

        let mut imported = 0i64;
        let mut extracted_paths = HashSet::new();
        let mut promotion_entries = Vec::new();
        // 事前検査（`preflight_backup_archive`）が見ているのは、書庫が自分で
        // 申告したサイズである。**申告と実際が一致する保証は無い。** 1MiBだと
        // 名乗って1GiBに膨らむ枝を並べれば、検査は通り、展開でディスクが埋まる。
        // 書いたバイトを実際に数えて、申告と総量の両方で止める。
        let mut written_total: u64 = 0;

        // ZIP内のファイルを展開
        let entry_total = archive.len() as i64;
        for i in 0..archive.len() {
            // **展開の途中は、まだライブラリを触っていない。** 止めるならここ。
            // 昇格が始まったあとは、途中で降りるほうが危ない。
            reporter.check_cancelled()?;
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.is_dir() {
                continue;
            }

            let entry_name = file.name().to_string();
            // 数万ファイルでは1件ごとに送ると多すぎる。区切って知らせる。
            if i % 64 == 0 {
                reporter.report("extract", i as i64, entry_total, Some(entry_name.clone()));
            }
            let (outpath, root) = resolve_zip_import_path(&entry_name, &stage_storage, &stage_app_data)
                .map_err(|e| format!("Security Exception for zip entry {entry_name}: {e}"))?;
            if !extracted_paths.insert(import_collision_key(&outpath)) {
                return Err(format!("Duplicate zip entry path: {entry_name}"));
            }

            // Verify the nearest existing ancestor before creating anything. This
            // prevents a malicious archive from creating directories outside the
            // root before the containment check runs. Existing symlinks/reparse
            // points are resolved by canonicalize and rejected when they escape.
            validate_import_parent_before_create(&outpath, root)
                .map_err(|e| format!("Security Exception for zip entry {entry_name}: {e}"))?;
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                canonical_path_is_within(parent, root).map_err(|e| {
                    format!("Security Exception for zip entry {entry_name}: {e}")
                })?;
            }

            let declared = file.size();
            let mut outfile = std::fs::File::create(&outpath).map_err(|e| e.to_string())?;
            // 申告より1バイトだけ多く読ませる。余分が出たら申告が嘘である。
            let written = std::io::copy(&mut (&mut file).take(declared.saturating_add(1)), &mut outfile)
                .map_err(|e| e.to_string())?;
            if written > declared {
                return Err(format!(
                    "Zip entry {entry_name} expands beyond its declared size ({declared} bytes)"
                ));
            }
            written_total = written_total.saturating_add(written);
            if written_total > MAX_BACKUP_TOTAL_BYTES {
                return Err("Backup staging quota exceeded".to_string());
            }
            outfile.sync_all().map_err(|e| e.to_string())?;
            if entry_name != "backup_metadata.json" {
                let (destination, _) = resolve_zip_import_path(&entry_name, &storage, &app_data)?;
                promotion_entries.push((entry_name.clone(), outpath, destination));
            }
        }

        // 展開されたフォルダからDBに登録
        let metadata_path = stage_storage.join("backup_metadata.json");
        if metadata_path.exists() {
            let metadata_str = std::fs::read_to_string(&metadata_path).map_err(|e| e.to_string())?;
            let metadata: BackupMetadata =
                serde_json::from_str(&metadata_str).map_err(|e| e.to_string())?;
            // Validate all metadata paths before any database row is mutated. ZIP
            // entry validation alone is insufficient because these strings are
            // independently supplied by backup_metadata.json.
            validate_backup_metadata_paths(&metadata, &stage_storage, &stage_app_data)
                .map_err(|e| format!("Invalid backup metadata: {e}"))?;
            validate_backup_metadata_files_exist(&metadata, &stage_storage, &stage_app_data)
                .map_err(|e| format!("Incomplete backup: {e}"))?;
            // Re-validate the same paths against their final roots before any
            // promotion. This also rejects a reserved-root mismatch.
            validate_backup_metadata_paths(&metadata, &storage, &app_data)
                .map_err(|e| format!("Invalid backup metadata: {e}"))?;

            // **目録が指していないものは、棚へ上げない。**
            //
            // ここまでは書庫の中身を丸ごとライブラリへ置いていた。Zip Slip は
            // 塞いであるので置き場所はライブラリの中に収まるが、DBに記録されない
            // ので二度と回収されない。無関係なファイルを何万個でも置ける。
            // 上げるのは `backup_metadata.json` が名前を挙げているものだけにする。
            let referenced: HashSet<String> = referenced_backup_paths(&metadata)?
                .into_iter()
                .collect();
            let promotion_entries: Vec<(PathBuf, PathBuf)> = promotion_entries
                .into_iter()
                .filter_map(|(name, staged, destination)| {
                    referenced
                        .contains(&name)
                        .then_some((staged, destination))
                })
                .collect();

            let journal_id = format!(
                "restore-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            );
            let mut promotion = prepare_staged_file_promotion(&stage_root, &promotion_entries)?;
            state.db.create_restore_journal(&journal_id, &stage_root)?;
            if let Err(error) = promotion.promote() {
                drop(promotion);
                let _ = state.db.finish_restore_journal(&journal_id);
                return Err(error);
            }
            staging_cleanup.disarm();
            if let Err(error) = restore_failpoint(failpoint, "before_db_commit") {
                drop(promotion);
                state.db.finish_restore_journal(&journal_id)?;
                return Err(error);
            }
            if let Err(error) = state.db.begin_atomic_restore() {
                drop(promotion);
                state.db.finish_restore_journal(&journal_id)?;
                return Err(error);
            }
            let mut restored_ids = Vec::with_capacity(metadata.entries.len());
            let mut stale_index_ids = Vec::new();
            let restore_result = (|| -> Result<(i64, Vec<i64>, Vec<i64>), String> {

            for person in &metadata.people {
                let restored = crate::database::PersonEntry {
                    id: 0,
                    source: person.source.clone(),
                    source_key: person.source_key.clone(),
                    display_name: person.display_name.clone(),
                    icon_path: person
                        .relative_icon_path
                        .as_deref()
                        .map(|p| resolve_entity_metadata_path(p, "profiles", &app_data))
                        .transpose()?
                        .map(|p| p.to_string_lossy().to_string()),
                    cover_path: person
                        .relative_cover_path
                        .as_deref()
                        .map(|p| resolve_entity_metadata_path(p, "profiles", &app_data))
                        .transpose()?
                        .map(|p| p.to_string_lossy().to_string()),
                    description: person.description.clone(),
                    links_json: person.links_json.clone(),
                    content_hash: person.content_hash.clone(),
                    current_version: person.current_version,
                    last_checked_at: person.last_checked_at.clone(),
                    last_fetched_at: person.last_fetched_at.clone(),
                    created_at: person.created_at.clone(),
                    updated_at: person.updated_at.clone(),
                    work_count: None,
                };
                state.db.restore_person(&restored)?;
                for version in &person.versions {
                    state.db.restore_entity_version(&EntityVersion {
                        id: 0,
                        entity_type: version.entity_type.clone(),
                        source: version.source.clone(),
                        source_key: version.source_key.clone(),
                        version: version.version,
                        content_hash: version.content_hash.clone(),
                        json_path: resolve_entity_metadata_path(
                            &version.relative_json_path,
                            "profiles",
                            &app_data,
                        )?
                        .to_string_lossy()
                        .to_string(),
                        asset_count: version.asset_count,
                        file_size_bytes: version.file_size_bytes,
                        created_at: version.created_at.clone(),
                        change_summary: version.change_summary.clone(),
                    })?;
                }
            }

            for series in &metadata.series {
                let restored = SeriesEntry {
                    id: 0,
                    source: series.source.clone(),
                    source_key: series.source_key.clone(),
                    title: series.title.clone(),
                    description: series.description.clone(),
                    cover_path: series
                        .relative_cover_path
                        .as_deref()
                        .map(|p| resolve_entity_metadata_path(p, "series", &app_data))
                        .transpose()?
                        .map(|p| p.to_string_lossy().to_string()),
                    content_hash: series.content_hash.clone(),
                    current_version: series.current_version,
                    last_checked_at: series.last_checked_at.clone(),
                    last_fetched_at: series.last_fetched_at.clone(),
                    created_at: series.created_at.clone(),
                    updated_at: series.updated_at.clone(),
                    work_count: None,
                    // 取得元に聞き直せば分かることなので、控えには持たない。
                    is_concluded: None,
                    published_content_count: None,
                };
                state.db.restore_series(&restored)?;
                for version in &series.versions {
                    state.db.restore_entity_version(&EntityVersion {
                        id: 0,
                        entity_type: version.entity_type.clone(),
                        source: version.source.clone(),
                        source_key: version.source_key.clone(),
                        version: version.version,
                        content_hash: version.content_hash.clone(),
                        json_path: resolve_entity_metadata_path(
                            &version.relative_json_path,
                            "series",
                            &app_data,
                        )?
                        .to_string_lossy()
                        .to_string(),
                        asset_count: version.asset_count,
                        file_size_bytes: version.file_size_bytes,
                        created_at: version.created_at.clone(),
                        change_summary: version.change_summary.clone(),
                    })?;
                }
            }

            for target in &metadata.update_targets {
                state.db.restore_update_target(target)?;
            }
            for candidate in &metadata.update_candidates {
                state.db.restore_update_candidate(candidate)?;
            }

            // ここから先はライブラリを書き換えている。**中止は受け付けない** -
            // 途中で降りるより、最後まで通してから戻すほうが安全である。
            // 進み具合だけは知らせる。
            let entry_total = metadata.entries.len() as i64;
            for (position, entry) in metadata.entries.iter().enumerate() {
                if position % 32 == 0 {
                    reporter.report("database", position as i64, entry_total, None);
                }
                // 重複していたら、既存のものを完全に削除して上書きリストアする
                if let Ok(Some(existing)) = state.db.get_download_by_source(&entry.source, &entry.source_id) {
                    stale_index_ids.push(existing.id);
                    state.db.delete_download_record_for_restore(existing.id)?;
                }

                let latest_ver = entry
                    .versions
                    .iter()
                    .find(|v| v.version == entry.current_version)
                    .or_else(|| entry.versions.first());

                // 相対パスを現在の storage の絶対パスにマッピングし直す
                let final_original_json_path = if let Some(latest_ver) = latest_ver {
                    if let Some(path) = &latest_ver.relative_original_json_path {
                        Some(
                            resolve_storage_metadata_path(path, &storage)?
                                .to_string_lossy()
                                .to_string(),
                        )
                    } else {
                        // If relative_original_json_path is absent, use an
                        // original.json next to the already-validated JSON path.
                        let json_path = resolve_storage_metadata_path(
                            &latest_ver.relative_json_path,
                            &storage,
                        )?;
                        let orig_p = json_path
                            .parent()
                            .ok_or_else(|| "Version JSON path has no parent".to_string())?
                            .join("original.json");
                        orig_p
                            .exists()
                            .then(|| orig_p.to_string_lossy().to_string())
                    }
                } else {
                    None
                };

                let final_json_path = if let Some(ref orig) = final_original_json_path {
                    // Prefer using original.json for json_path as well!
                    orig.clone()
                } else if let Some(latest_ver) = latest_ver {
                    resolve_storage_metadata_path(&latest_ver.relative_json_path, &storage)?
                        .to_string_lossy()
                        .to_string()
                } else {
                    resolve_storage_metadata_path(
                        &format!(
                            "{}/{}/v{}/original.json",
                            entry.source, entry.source_id, entry.current_version
                        ),
                        &storage,
                    )?
                    .to_string_lossy()
                    .to_string()
                };

                let final_cover_path = entry
                    .relative_cover_path
                    .as_deref()
                    .map(|p| resolve_storage_metadata_path(p, &storage))
                    .transpose()?
                    .map(|p| p.to_string_lossy().to_string());

                let new_dl = NewDownload {
                    source: entry.source.clone(),
                    source_id: entry.source_id.clone(),
                    title: entry.title.clone(),
                    author_name: entry.author_name.clone(),
                    author_id: entry.author_id.clone(),
                    content_type: entry.content_type.clone(),
                    tags: if entry.tag_sources.is_empty() {
                        entry.tags.clone()
                    } else {
                        entry.tag_sources.iter().map(|tag| tag.name.clone()).collect()
                    },
                    excerpt: entry.excerpt.clone(),
                    cover_path: final_cover_path,
                    json_path: final_json_path,
                    original_json_path: final_original_json_path,
                    asset_count: entry.assets.len() as i64,
                    file_size_bytes: latest_ver.map(|v| v.file_size_bytes).unwrap_or(0),
                    downloaded_at: entry.downloaded_at.clone(),
                    source_created_at: entry.source_created_at.clone(),
                    content_hash: latest_ver.and_then(|v| v.content_hash.clone()),
                    text_length: latest_ver.map(|v| v.text_length).unwrap_or(0),
                    source_updated_at: entry.source_updated_at.clone(),
                    watch_updates: entry.watch_updates,
                    current_version: entry.current_version,
                    favorite: entry.favorite,
                };

                let dl_id = state.db.upsert_download(&new_dl)?;
                if !entry.tag_sources.is_empty() {
                    state.db.restore_tags(dl_id, &entry.tag_sources)?;
                }
                imported += 1;
                restored_ids.push(dl_id);

                for relation in &entry.relations {
                            state.db.upsert_download_relation(
                                dl_id,
                                &relation.relation_type,
                                &relation.source,
                                &relation.relation_id,
                                &relation.relation_name,
                            )?;
                        }

                        for person in &entry.people {
                            state.db.upsert_download_person(
                                dl_id,
                                &person.person_source,
                                &person.person_key,
                                &person.role,
                                &person.display_name,
                            )?;
                        }

                        for series in &entry.series {
                            state.db.upsert_download_series(
                                dl_id,
                                &series.series_source,
                                &series.series_key,
                                &series.title,
                                series.content_order,
                            )?;
                        }

                        // バージョン履歴の復元
                        for v in &entry.versions {
                            let ver_orig = if let Some(path) = &v.relative_original_json_path {
                                Some(
                                    resolve_storage_metadata_path(path, &storage)?
                                        .to_string_lossy()
                                        .to_string(),
                                )
                            } else {
                                let json_path = resolve_storage_metadata_path(
                                    &v.relative_json_path,
                                    &storage,
                                )?;
                                let orig_p = json_path
                                    .parent()
                                    .ok_or_else(|| "Version JSON path has no parent".to_string())?
                                    .join("original.json");
                                orig_p
                                    .exists()
                                    .then(|| orig_p.to_string_lossy().to_string())
                            };

                            let ver_json = if let Some(ref orig) = ver_orig {
                                orig.clone()
                            } else {
                                resolve_storage_metadata_path(&v.relative_json_path, &storage)?
                                    .to_string_lossy()
                                    .to_string()
                            };

                            let new_ver = NewVersion {
                                download_id: dl_id,
                                version: v.version,
                                content_hash: v.content_hash.clone(),
                                text_length: v.text_length,
                                json_path: ver_json,
                                original_json_path: ver_orig,
                                asset_count: entry.assets.len() as i64,
                                file_size_bytes: v.file_size_bytes,
                                created_at: v.created_at.clone(),
                                change_summary: v.change_summary.clone(),
                            };

                            state.db.insert_version(&new_ver)?;
                        }

                        // アセット情報の復元
                        for asset in &entry.assets {
                            let asset_local = resolve_storage_metadata_path(
                                &asset.relative_local_path,
                                &storage,
                            )?
                            .to_string_lossy()
                            .to_string();

                            // **名前はパスとして使われる。** 取得経路は必ず
                            // `sanitize_filename` を通るのに、復元だけがここを
                            // 迂回して、書庫の言うままの名前を DB へ書いていた。
                            // 絶対パスを名乗る名前が入ると、あとで作品を書き出す
                            // ときの `join` がその絶対パスへ飛ぶ。
                            let new_asset = NewAsset {
                                download_id: dl_id,
                                asset_type: crate::downloader::asset_downloader::sanitize_filename(
                                    &asset.asset_type,
                                ),
                                filename: crate::downloader::asset_downloader::sanitize_filename(
                                    &asset.filename,
                                ),
                                local_path: asset_local,
                                original_url: asset.original_url.clone(),
                                mime_type: asset.mime_type.clone(),
                                file_size_bytes: asset.file_size_bytes,
                            };

                            state.db.insert_asset(&new_asset)?;
                        }

                        let mut work_edits = entry.work_edits.clone();
                        for revision in &mut work_edits {
                            for block in &mut revision.blocks {
                                if let Some(path) = block.asset_path.as_deref() {
                                    block.asset_path = Some(
                                        resolve_storage_metadata_path(path, &storage)?
                                            .to_string_lossy()
                                            .to_string(),
                                    );
                                }
                            }
                        }
                        state.db.restore_work_edits(dl_id, &work_edits)?;

            }

                for search in &metadata.saved_searches {
                    state.db.restore_saved_search(search)?;
                }
                for feedback in &metadata.collection_pair_feedback {
                    state.db.restore_collection_pair_feedback(feedback)?;
                }

                for collection in &metadata.collections {
                    let members = collection
                        .members
                        .iter()
                        .map(|member| WorkCollectionMemberInput {
                            source: member.source.clone(),
                            source_id: member.source_id.clone(),
                            title_snapshot: Some(member.title_snapshot.clone()),
                            author_snapshot: Some(member.author_snapshot.clone()),
                            position: Some(member.position),
                            member_role: Some(member.member_role.clone()),
                            added_by: Some(member.added_by.clone()),
                            pinned: Some(member.pinned),
                            note: member.note.clone(),
                        })
                        .collect::<Vec<_>>();
                    state.db.restore_work_collection(
                        &WorkCollectionInput {
                            id: Some(collection.id.clone()),
                            name: collection.name.clone(),
                            description: collection.description.clone(),
                            collection_kind: collection.collection_kind.clone(),
                            cover_download_id: None,
                            cover_mode: collection.cover_mode.clone(),
                            cover_image_path: collection
                                .relative_cover_image_path
                                .as_deref()
                                .map(|path| {
                                    resolve_entity_metadata_path(
                                        path,
                                        "collection-covers",
                                        &app_data,
                                    )
                                })
                                .transpose()?
                                .map(|path| path.to_string_lossy().to_string()),
                            name_source: collection
                                .name_source
                                .clone()
                                .or_else(|| Some("manual".to_string())),
                            track: collection.track.clone(),
                        },
                        collection.cover_work.as_ref(),
                        &members,
                    )?;
                }

                record_restore_stale_index_ids(&stage_root, &stale_index_ids)?;
                state.db.mark_restore_journal_committed(&journal_id)?;
                Ok::<(i64, Vec<i64>, Vec<i64>), String>((
                    imported,
                    restored_ids,
                    stale_index_ids,
                ))
            })();
            match restore_result {
                Ok((count, restored_ids, stale_index_ids)) => {
                    if let Err(error) = state.db.commit_atomic_restore() {
                        state.db.rollback_atomic_restore();
                        let committed = state
                            .db
                            .pending_restore_journals()?
                            .into_iter()
                            .find(|(id, _, _)| id == &journal_id)
                            .is_some_and(|(_, _, committed)| committed);
                        if committed {
                            promotion.preserve_for_recovery();
                        } else {
                            drop(promotion);
                            state.db.finish_restore_journal(&journal_id)?;
                        }
                        return Err(error);
                    }
                    if let Err(error) = restore_failpoint(failpoint, "after_db_commit") {
                        // Simulate a process stop: leave the committed journal
                        // and rollback copies for startup recovery to finalize.
                        promotion.preserve_for_recovery();
                        return Err(error);
                    }
                    // Search indexes are derived data. Updating them before the
                    // SQLite COMMIT could publish documents for rows that later
                    // roll back, so all sidecar work happens after commit.
                    let mut derived_error = None;
                    if let Err(error) = crate::database::tantivy_index::delete_documents(
                        &storage,
                        &stale_index_ids,
                    ) {
                        derived_error = Some(format!("lexical cleanup failed: {error}"));
                    }
                    if let Err(error) = crate::database::semantic_index::clear_documents(
                        &storage,
                        &stale_index_ids,
                    ) {
                        derived_error = Some(format!("semantic cleanup failed: {error}"));
                    }
                    for download_id in restored_ids {
                        if let Err(error) = state.db.reindex_download(download_id) {
                            log::warn!(
                                "Failed to rebuild search index for restored download {}: {}",
                                download_id,
                                error
                            );
                            derived_error = Some(format!(
                                "restored work {download_id} reindex failed: {error}"
                            ));
                        }
                    }
                    if let Some(error) = derived_error {
                        // The restore is committed and usable; retain its
                        // journal so startup can retry stale-sidecar cleanup.
                        log::warn!("Restore committed with derived-index work pending: {error}");
                        promotion.preserve_for_recovery();
                        return Ok(count);
                    }
                    promotion.commit();
                    state.db.finish_restore_journal(&journal_id)?;
                    Ok(count)
                }
                Err(error) => {
                    state.db.rollback_atomic_restore();
                    // Dropping the promotion guard restores overwritten files
                    // and removes newly promoted files in reverse order.
                    drop(promotion);
                    state.db.finish_restore_journal(&journal_id)?;
                    Err(error)
                }
            }
        } else {
            Err("Backup metadata file not found. Old backup formats are not supported in this version.".to_string())
        }
    })
    .await
    .map_err(|e| format!("Import thread panicked: {}", e))?
}

pub async fn import_multipart_backup_internal(
    state: Arc<AppState>,
    manifest_path: String,
) -> Result<i64, String> {
    import_multipart_with_reporter(state, manifest_path, &ArchiveReporter::detached()).await
}

async fn import_multipart_with_reporter(
    state: Arc<AppState>,
    manifest_path: String,
    reporter: &ArchiveReporter,
) -> Result<i64, String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();
    let manifest_path_buf = PathBuf::from(&manifest_path);
    let (inspection, manifest, paths) = tokio::task::spawn_blocking(move || {
        let (manifest, paths) = read_and_validate_multipart_manifest(&manifest_path_buf)?;
        let inspection = inspect_validated_multipart_parts(&manifest, &paths, &storage, &app_data)?;
        Ok::<_, String>((inspection, manifest, paths))
    })
    .await
    .map_err(|error| format!("Multipart preflight thread panicked: {error}"))??;
    if !inspection.valid {
        return Err(inspection
            .error
            .unwrap_or_else(|| "Multipart backup failed validation".to_string()));
    }

    let _import_guard = IMPORT_LOCK.lock().await;
    let _library_write_guard = state.library_gate.clone().write_owned().await;

    // **どこまで入ったかを残す。** パートごとの取り込みはそれぞれ原子的だが、
    // パートをまたぐと原子的にならない。途中で落ちれば「バックアップ由来の
    // 作品」と「元の作品」が混ざった棚が残るのに、それを記録している場所が
    // どこにも無かった。身元はパートの指紋から作るので、同じ書庫なら置き場所が
    // 変わっても同じになる。
    let manifest_id = multipart_manifest_id(&manifest);
    let total_parts = manifest.parts.len() as i64;
    let completed = state
        .db
        .begin_restore_manifest(&manifest_id, &manifest_path, total_parts)?;
    if completed > 0 {
        log::info!(
            "分割復元を再開します（{completed}/{total_parts} パートまで済み）: {manifest_path}"
        );
    }

    let mut imported = 0i64;
    let mut done = completed;
    for (index, path) in paths.into_iter().enumerate() {
        // 済んだパートはやり直さない。取り込み自体は冪等だが、900/1000 で
        // 失敗したときに 900 パートを読み直すのは、待つ側には失敗と変わらない。
        if (index as i64) < completed {
            continue;
        }
        // パートの切れ目は、まだライブラリを書き換えていない安全な場所。
        reporter.check_cancelled()?;
        reporter.report(
            "part",
            done,
            total_parts,
            path.file_name()
                .map(|name| name.to_string_lossy().to_string()),
        );
        match import_zip_locked(
            state.clone(),
            path.to_string_lossy().to_string(),
            None,
            reporter,
        )
        .await
        {
            Ok(count) => {
                imported = imported.saturating_add(count);
                done += 1;
                if let Err(error) = state.db.advance_restore_manifest(&manifest_id, done) {
                    log::warn!("分割復元の進み具合を記録できません: {error}");
                }
            }
            Err(error) => {
                return Err(format!(
                    "Multipart restore stopped after {index} of {} parts: {error}. Re-run the same manifest to resume safely.",
                    manifest.parts.len()
                ));
            }
        }
    }
    if done != total_parts {
        return Err(format!(
            "Multipart restore did not finish: {done} of {total_parts} parts"
        ));
    }
    state.db.finish_restore_manifest(&manifest_id)?;
    // 再開したときの `imported` は、この回で入れたぶんだけである。件数の
    // 突き合わせは一度で通したときにしかできない。
    if completed == 0 && imported != manifest.total_works {
        return Err(format!(
            "Multipart restore count mismatch: expected {}, imported {imported}",
            manifest.total_works
        ));
    }
    Ok(imported)
}

/// 同じ書庫なら、置き場所が変わっても同じ身元になる。
fn multipart_manifest_id(manifest: &MultipartBackupManifest) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(manifest.total_works.to_string().as_bytes());
    for part in &manifest.parts {
        hasher.update(
            b"
",
        );
        hasher.update(part.sha256.as_bytes());
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("multipart-{digest}")
}

fn work_needs_reimport(db: &Database, source: &str, source_id: &str) -> Result<bool, String> {
    Ok(db.get_download_by_source(source, source_id)?.is_none())
}

/// 保存フォルダーを走査した結果。
///
/// **件数だけでは足りない。** 途中で読めないものがあっても取り込みは続くので、
/// 何件入って何件を飛ばしたのかを両方返す。飛ばした理由は画面に出す。
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReimportOutcome {
    pub imported: i64,
    pub skipped: Vec<String>,
}

#[tauri::command]
pub async fn scan_and_reimport_downloads(app: tauri::AppHandle) -> Result<ReimportOutcome, String> {
    use crate::commands::downloader::{
        collect_assets_recursive, compute_content_details, extract_series_content_order,
        extract_series_relation,
    };

    let state = app.state::<Arc<AppState>>().inner().clone();
    let library_write_guard = state.library_gate.clone().write_owned().await;
    let storage = state.db.storage_dir().to_path_buf();

    tokio::task::spawn_blocking(move || {
        let _library_write_guard = library_write_guard;
        let mut total_imported = 0i64;
        // 飛ばした作品と理由。**途中結果を捨てない。**
        let mut skipped: Vec<String> = Vec::new();

        if !storage.exists() {
            return Ok(ReimportOutcome {
                imported: 0,
                skipped: Vec::new(),
            });
        }
        let storage_metadata = std::fs::symlink_metadata(&storage)
            .map_err(|error| format!("Storage metadata failed: {error}"))?;
        if archive_metadata_is_link(&storage_metadata) || !storage_metadata.file_type().is_dir() {
            return Err("Library storage must be a real directory, not a link".to_string());
        }
        let canonical_storage = storage
            .canonicalize()
            .map_err(|error| format!("Storage path resolution failed: {error}"))?;

        // Iterate through sources (e.g. "pixiv", "fanbox")
        for source_entry in std::fs::read_dir(&storage).map_err(|e| e.to_string())? {
            let source_entry = source_entry.map_err(|e| e.to_string())?;
            let source_path = source_entry.path();
            let source = source_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if source != "pixiv" && source != "fanbox" {
                continue;
            }
            let canonical_source =
                canonical_reimport_directory(&source_path, &canonical_storage)?;

            // Iterate through work IDs (e.g. "123456")
            for id_entry in std::fs::read_dir(&source_path).map_err(|e| e.to_string())? {
                let id_entry = id_entry.map_err(|e| e.to_string())?;
                let id_path = id_entry.path();
                let source_id = id_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if source_id.is_empty()
                    || source_id.len() > 128
                    || !source_id
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
                {
                    continue;
                }
                let canonical_id = canonical_reimport_directory(&id_path, &canonical_source)?;

                // Find version directories "v1", "v2", ...
                let mut versions = Vec::new();
                for ver_entry in std::fs::read_dir(&id_path).map_err(|e| e.to_string())? {
                    let ver_entry = ver_entry.map_err(|e| e.to_string())?;
                    let ver_path = ver_entry.path();
                    let ver_name = ver_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if let Some(stripped) = ver_name.strip_prefix('v') {
                        if let Ok(v_num) = stripped.parse::<i64>() {
                            if v_num <= 0 || ver_name != format!("v{v_num}") {
                                continue;
                            }
                            let canonical_version =
                                canonical_reimport_directory(&ver_path, &canonical_id)?;
                            versions.push((v_num, canonical_version));
                            if versions.len() > MAX_REIMPORT_VERSIONS_PER_WORK {
                                return Err(format!(
                                    "Reimport work {source}/{source_id} exceeds the {}-version safety limit",
                                    MAX_REIMPORT_VERSIONS_PER_WORK
                                ));
                            }
                        }
                    }
                }

                // Sort versions ascendingly
                versions.sort_by_key(|v| v.0);

                if versions.is_empty() {
                    continue;
                }

                // This operation is recovery for a DB record that disappeared,
                // not an implicit overwrite mechanism. Rebuilding an existing row
                // from manually edited files could discard favorites, edits, and
                // normalized relations if even one version is incomplete.
                if !work_needs_reimport(&state.db, &source, &source_id)? {
                    continue;
                }

                let mut prepared_download = None;
                let mut prepared_assets = Vec::new();
                let mut prepared_versions = Vec::with_capacity(versions.len());
                let mut latest_data = None;

                // Import each version
                // **一つの版が読めないことを、全体の失敗にしない。**
                //
                // かつてここは `return Err` で関数ごと抜けていた。保存フォルダーに
                // 更新の中断跡（JSON の無い `v3`）が一つ混じっているだけで、
                // それまでに取り込んだ何百件も破棄され、画面には「操作に失敗
                // しました」としか出ない。作者・シリーズの再構築も走らない。
                // 再実行しても同じ場所で止まるので、利用者が自力でその
                // フォルダーを見つけるまで永久に完了しない。
                let version_result = (|| -> Result<(), String> {
                    for (v_num, ver_path) in versions {
                        let orig_json_path = ver_path.join("original.json");
                        let data_json_path = ver_path.join("data.json");
                        let json_file = if orig_json_path.try_exists().map_err(|error| {
                            format!("Reimport JSON existence check failed: {error}")
                        })? {
                            canonical_reimport_json(&orig_json_path, &ver_path)?
                        } else if data_json_path.try_exists().map_err(|error| {
                            format!("Reimport JSON existence check failed: {error}")
                        })? {
                            canonical_reimport_json(&data_json_path, &ver_path)?
                        } else {
                            return Err(format!(
                                "Reimport work {source}/{source_id} v{v_num} has no original.json or data.json"
                            ));
                        };

                        let content_str =
                            std::fs::read_to_string(&json_file).map_err(|e| e.to_string())?;
                        let data: serde_json::Value =
                            serde_json::from_str(&content_str).map_err(|e| e.to_string())?;

                        let (new_hash, new_text_len, new_source_updated) =
                            compute_content_details(&data, &source);

                        let title = data
                            .get("title")
                            .or_else(|| data.get("detail").and_then(|d| d.get("title")))
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown Title")
                            .to_string();

                        let author_name = if source == "pixiv" {
                            data.get("detail")
                                .and_then(|d| d.get("user"))
                                .and_then(|u| u.get("name"))
                                .and_then(|v| v.as_str())
                                .or_else(|| {
                                    data.get("user")
                                        .and_then(|u| u.get("name"))
                                        .and_then(|v| v.as_str())
                                })
                                .unwrap_or("Unknown User")
                                .to_string()
                        } else {
                            data.get("user")
                                .and_then(|u| u.get("name"))
                                .and_then(|v| v.as_str())
                                .or_else(|| data.get("creatorId").and_then(|v| v.as_str()))
                                .unwrap_or("Unknown Creator")
                                .to_string()
                        };

                        let author_id = if source == "pixiv" {
                            let u_id = data
                                .get("detail")
                                .and_then(|d| d.get("user"))
                                .and_then(|u| u.get("id"))
                                .or_else(|| data.get("user").and_then(|u| u.get("id")));
                            if let Some(uid) = u_id {
                                if let Some(s) = uid.as_str() {
                                    s.to_string()
                                } else if let Some(n) = uid.as_u64() {
                                    n.to_string()
                                } else {
                                    "0".to_string()
                                }
                            } else {
                                "0".to_string()
                            }
                        } else {
                            let u_id = data
                                .get("creatorId")
                                .or_else(|| data.get("user").and_then(|u| u.get("userId")));
                            if let Some(uid) = u_id {
                                if let Some(s) = uid.as_str() {
                                    s.to_string()
                                } else if let Some(n) = uid.as_u64() {
                                    n.to_string()
                                } else {
                                    "0".to_string()
                                }
                            } else {
                                "0".to_string()
                            }
                        };

                        let content_type = if source == "pixiv" {
                            "novel".to_string()
                        } else {
                            data.get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("article")
                                .to_string()
                        };

                        let mut tags_list = Vec::new();
                        if let Some(tags_val) = data
                            .get("tags")
                            .or_else(|| data.get("detail").and_then(|d| d.get("tags")))
                        {
                            if let Some(arr) = tags_val.as_array() {
                                for t in arr {
                                    if let Some(s) = t.as_str() {
                                        tags_list.push(s.to_string());
                                    } else if let Some(s) = t.get("name").and_then(|n| n.as_str()) {
                                        tags_list.push(s.to_string());
                                    }
                                }
                            }
                        }
                        let excerpt = data
                            .get("excerpt")
                            .or_else(|| data.get("detail").and_then(|d| d.get("excerpt")))
                            .or_else(|| data.get("caption"))
                            .or_else(|| data.get("detail").and_then(|d| d.get("caption")))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        let source_created_at = if source == "pixiv" {
                            data.get("detail")
                                .and_then(|d| d.get("create_date").or_else(|| d.get("createDate")))
                                .and_then(|v| v.as_str())
                                .or_else(|| {
                                    data.get("create_date")
                                        .or_else(|| data.get("createDate"))
                                        .and_then(|v| v.as_str())
                                })
                                .map(|s| s.to_string())
                        } else {
                            data.get("publishedDatetime")
                                .or_else(|| data.get("published_datetime"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        };

                        // Scan assets
                        let assets_dir = ver_path.join("data_assets");
                        let mut asset_entries = Vec::new();
                        let mut total_size = content_str.len() as i64;
                        let mut cover_path_str = None;

                        if assets_dir.try_exists().map_err(|error| {
                            format!("Reimport asset directory existence check failed: {error}")
                        })? {
                            let assets_dir = canonical_reimport_directory(&assets_dir, &ver_path)?;
                            collect_assets_recursive(
                                &assets_dir,
                                &mut asset_entries,
                                &mut total_size,
                                &mut cover_path_str,
                                0,
                            )?;
                        }

                        let new_dl = NewDownload {
                            source: source.clone(),
                            source_id: source_id.clone(),
                            title: title.clone(),
                            author_name: author_name.clone(),
                            author_id: author_id.clone(),
                            content_type: content_type.clone(),
                            tags: tags_list,
                            excerpt,
                            cover_path: cover_path_str,
                            json_path: json_file.to_string_lossy().to_string(),
                            original_json_path: Some(json_file.to_string_lossy().to_string()),
                            asset_count: asset_entries.len() as i64,
                            file_size_bytes: total_size,
                            downloaded_at: chrono::Utc::now().to_rfc3339(),
                            source_created_at,
                            content_hash: Some(new_hash.clone()),
                            text_length: new_text_len,
                            source_updated_at: new_source_updated.clone(),
                            watch_updates: false,
                            current_version: v_num,
                            favorite: false,
                        };
                        prepared_versions.push(NewVersion {
                            // The transaction replaces this placeholder with the
                            // ID assigned to the recovered work.
                            download_id: 0,
                            version: v_num,
                            content_hash: Some(new_hash),
                            text_length: new_text_len,
                            json_path: json_file.to_string_lossy().to_string(),
                            original_json_path: Some(json_file.to_string_lossy().to_string()),
                            asset_count: new_dl.asset_count,
                            file_size_bytes: new_dl.file_size_bytes,
                            created_at: new_dl.downloaded_at.clone(),
                            change_summary: Some(format!("インポート復元 (v{})", v_num)),
                        });
                        prepared_assets.extend(asset_entries.into_iter().map(|mut asset| {
                            asset.download_id = 0;
                            asset
                        }));
                        prepared_download = Some(new_dl);
                        latest_data = Some(data);
                    }
                    Ok(())
                })();
                if let Err(error) = version_result {
                    log::warn!("再取り込みで {source}/{source_id} を飛ばしました: {error}");
                    skipped.push(format!("{source}/{source_id}: {error}"));
                    continue;
                }

                let Some(new_dl) = prepared_download else {
                    continue;
                };
                let registered_id = state.db.commit_reimported_download(
                    &new_dl,
                    &prepared_assets,
                    &prepared_versions,
                )?;

                let relation_result = (|| -> Result<(), String> {
                    state.db.upsert_download_relation(
                        registered_id,
                        "author",
                        &source,
                        &new_dl.author_id,
                        &new_dl.author_name,
                    )?;
                    state.db.upsert_download_person(
                        registered_id,
                        &source,
                        &new_dl.author_id,
                        if source == "fanbox" {
                            "creator"
                        } else {
                            "author"
                        },
                        &new_dl.author_name,
                    )?;

                    if source == "pixiv" {
                        if let Some(data) = latest_data.as_ref() {
                            if let Some((series_id, series_title)) = extract_series_relation(data) {
                                state.db.upsert_download_relation(
                                    registered_id,
                                    "series",
                                    &source,
                                    &series_id,
                                    &series_title,
                                )?;
                                state.db.upsert_download_series(
                                    registered_id,
                                    &source,
                                    &series_id,
                                    &series_title,
                                    extract_series_content_order(data),
                                )?;
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = relation_result {
                    state
                        .db
                        .delete_download_record_for_reimport(registered_id)
                        .map_err(|cleanup| {
                            format!(
                                "Reimport relation failed: {error}; DB rollback also failed: {cleanup}"
                            )
                        })?;
                    return Err(format!("Reimport relation failed: {error}"));
                }
                state.db.reindex_download(registered_id)?;
                total_imported += 1;
            }
        }

        // Reconstruct people and series tables from downloads & download_relations directly using fast SQLite queries!
        if total_imported > 0 {
            state.db.reconstruct_entities_after_import()?;
        }
        if !skipped.is_empty() {
            log::warn!(
                "再取り込みで {} 件を飛ばしました: {}",
                skipped.len(),
                skipped.join(" / ")
            );
        }

        Ok::<ReimportOutcome, String>(ReimportOutcome {
            imported: total_imported,
            skipped,
        })
    })
    .await
    .map_err(|e| format!("Reimport thread panicked: {}", e))?
}

#[tauri::command]
pub async fn get_storage_path(app: tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>();
    Ok(state.db.storage_dir().to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::queries::EntityProfileFreshness;
    use crate::database::Database;
    use std::fs;
    use std::sync::Arc;

    // ヘルパー：ランダムな一時ディレクトリを作成する
    fn create_temp_dir() -> std::path::PathBuf {
        let rand_val: u32 = rand::random();
        let path = std::env::temp_dir().join(format!("piep_test_archive_{}", rand_val));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn remove_temp_dir(path: &Path) {
        let mut last_error = None;
        for _ in 0..20 {
            match fs::remove_dir_all(path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) => {
                    last_error = Some(error);
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }
        }
        panic!(
            "failed to remove test directory {}: {}",
            path.display(),
            last_error.expect("cleanup attempted")
        );
    }

    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
    }

    fn write_restore_test_zip(path: &Path, title: &str, marker: &str) {
        write_restore_test_zip_for_id(path, title, marker, "crash-restore");
    }

    fn write_restore_test_zip_for_id(path: &Path, title: &str, marker: &str, source_id: &str) {
        let relative_json_path = format!("pixiv/{source_id}/v1/data.json");
        let metadata = BackupMetadata {
            version: "3.0".to_string(),
            created_at: "2026-08-12T00:00:00Z".to_string(),
            entries: vec![BackupEntry {
                source: "pixiv".to_string(),
                source_id: source_id.to_string(),
                title: title.to_string(),
                author_name: "Restore author".to_string(),
                author_id: "restore-author".to_string(),
                content_type: "novel".to_string(),
                tags: Vec::new(),
                tag_sources: Vec::new(),
                excerpt: None,
                relative_cover_path: None,
                watch_updates: false,
                favorite: false,
                downloaded_at: "2026-08-12T00:00:00Z".to_string(),
                source_created_at: None,
                source_updated_at: None,
                current_version: 1,
                versions: vec![BackupVersion {
                    version: 1,
                    content_hash: Some(format!("hash-{marker}")),
                    text_length: marker.len() as i64,
                    file_size_bytes: marker.len() as i64,
                    created_at: "2026-08-12T00:00:00Z".to_string(),
                    change_summary: None,
                    relative_json_path: relative_json_path.clone(),
                    relative_original_json_path: None,
                }],
                assets: Vec::new(),
                relations: Vec::new(),
                people: Vec::new(),
                series: Vec::new(),
                work_edits: Vec::new(),
            }],
            people: Vec::new(),
            series: Vec::new(),
            update_targets: Vec::new(),
            update_candidates: Vec::new(),
            collections: vec![BackupWorkCollection {
                id: format!("collection-{source_id}"),
                name: "復元コレクション".to_string(),
                description: Some("バックアップされた並び".to_string()),
                collection_kind: "ordered".to_string(),
                cover_work: Some(WorkKey {
                    source: "pixiv".to_string(),
                    source_id: source_id.to_string(),
                }),
                cover_mode: Some("mosaic".to_string()),
                relative_cover_image_path: None,
                name_source: Some("title".to_string()),
                track: Some("sequence".to_string()),
                members: vec![BackupWorkCollectionMember {
                    source: "pixiv".to_string(),
                    source_id: source_id.to_string(),
                    title_snapshot: title.to_string(),
                    author_snapshot: "Restore author".to_string(),
                    position: 0,
                    member_role: "main".to_string(),
                    added_by: "import".to_string(),
                    pinned: false,
                    note: None,
                }],
            }],
            saved_searches: Vec::new(),
            collection_pair_feedback: Vec::new(),
        };
        let metadata = serde_json::to_vec(&metadata).unwrap();
        let payload = serde_json::to_vec(&serde_json::json!({ "text": marker })).unwrap();
        write_test_zip(
            path,
            &[
                ("backup_metadata.json", metadata.as_slice()),
                (relative_json_path.as_str(), payload.as_slice()),
            ],
        );
    }

    fn restore_target(base: &Path) -> (Arc<AppState>, PathBuf, i64) {
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = Arc::new(AppState::new(db));
        let work_dir = storage.join("pixiv").join("crash-restore").join("v1");
        fs::create_dir_all(&work_dir).unwrap();
        let json_path = work_dir.join("data.json");
        fs::write(&json_path, br#"{"text":"oldrestoremarker"}"#).unwrap();
        let id = state
            .db
            .upsert_download(&NewDownload {
                source: "pixiv".to_string(),
                source_id: "crash-restore".to_string(),
                title: "Old restore title".to_string(),
                author_name: "Restore author".to_string(),
                author_id: "restore-author".to_string(),
                content_type: "novel".to_string(),
                tags: Vec::new(),
                excerpt: None,
                cover_path: None,
                json_path: json_path.to_string_lossy().to_string(),
                original_json_path: None,
                asset_count: 0,
                file_size_bytes: 1,
                downloaded_at: "2026-08-11T00:00:00Z".to_string(),
                source_created_at: None,
                content_hash: Some("old-hash".to_string()),
                text_length: 16,
                source_updated_at: None,
                watch_updates: false,
                current_version: 1,
                favorite: false,
            })
            .unwrap();
        state.db.reindex_download(id).unwrap();
        (state, json_path, id)
    }

    #[test]
    fn backup_paths_reject_cross_platform_traversal_and_wrong_roots() {
        for path in [
            "../outside.txt",
            "safe/../../outside.txt",
            "/absolute.txt",
            r"C:/outside.txt",
            r"safe\..\outside.txt",
            "//server/share/file.txt",
            "safe/./file.txt",
            "safe//file.txt",
            "safe/CON/file.txt",
            "safe/NUL.txt",
            "safe/trailing. ",
        ] {
            assert!(
                validated_backup_relative_path(path).is_err(),
                "unsafe path was accepted: {path}"
            );
        }

        let base = create_temp_dir();
        let storage = base.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        assert!(resolve_storage_metadata_path("pixiv/1/v1/data.json", &storage).is_ok());
        assert!(resolve_storage_metadata_path("profiles/pixiv/1/icon.png", &storage).is_err());
        assert!(
            resolve_entity_metadata_path("profiles/pixiv/1/data.json", "profiles", &base).is_ok()
        );
        assert!(
            resolve_entity_metadata_path("series/pixiv/1/data.json", "profiles", &base).is_err()
        );
        remove_temp_dir(&base);
    }

    #[test]
    fn preflight_rejects_zip_bomb_compression_ratio() {
        let base = create_temp_dir();
        let zip_path = base.join("bomb.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("backup_metadata.json", options).unwrap();
        writer.write_all(b"{}").unwrap();
        writer
            .start_file("pixiv/1/v1/original.json", options)
            .unwrap();
        writer.write_all(&vec![0u8; 2 * 1024 * 1024]).unwrap();
        writer.finish().unwrap();

        let file = fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let error = preflight_backup_archive(&mut archive).unwrap_err();
        assert!(error.contains("compression ratio"));
        remove_temp_dir(&base);
    }

    #[test]
    fn backup_scope_follows_the_cursor_past_the_first_page() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = AppState::new(db);
        let total = BACKUP_PAGE_SIZE + 7;

        for index in 0..total {
            state
                .db
                .upsert_download(&NewDownload {
                    source: "pixiv".to_string(),
                    source_id: format!("backup-{index}"),
                    title: format!("Backup work {index}"),
                    author_name: "Backup author".to_string(),
                    author_id: "backup-author".to_string(),
                    content_type: "novel".to_string(),
                    tags: Vec::new(),
                    excerpt: None,
                    cover_path: None,
                    json_path: storage
                        .join(format!("backup-{index}/v1/data.json"))
                        .to_string_lossy()
                        .to_string(),
                    original_json_path: None,
                    asset_count: 0,
                    file_size_bytes: 0,
                    // Identical sort values exercise the id tie-breaker in the
                    // SQL cursor rather than accidentally relying on timestamps.
                    downloaded_at: "2026-08-12T00:00:00Z".to_string(),
                    source_created_at: None,
                    content_hash: None,
                    text_length: 0,
                    source_updated_at: None,
                    watch_updates: false,
                    current_version: 1,
                    favorite: false,
                })
                .unwrap();
        }

        let collected = collect_backup_downloads(&state, all_backup_search_params()).unwrap();
        assert_eq!(collected.len() as i64, total);
        assert_eq!(
            collected
                .iter()
                .map(|item| item.id)
                .collect::<HashSet<_>>()
                .len(),
            total as usize,
            "cursor paging must not duplicate works"
        );

        drop(state);
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn failed_export_keeps_existing_backup_and_removes_temporary_file() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = Arc::new(AppState::new(db));
        let work_dir = storage.join("pixiv").join("incomplete").join("v1");
        fs::create_dir_all(&work_dir).unwrap();
        let json_path = work_dir.join("data.json");
        fs::write(&json_path, b"{\"text\":\"complete metadata\"}").unwrap();
        let download_id = state
            .db
            .upsert_download(&NewDownload {
                source: "pixiv".to_string(),
                source_id: "incomplete".to_string(),
                title: "Incomplete backup work".to_string(),
                author_name: "Author".to_string(),
                author_id: "author".to_string(),
                content_type: "novel".to_string(),
                tags: Vec::new(),
                excerpt: None,
                cover_path: None,
                json_path: json_path.to_string_lossy().to_string(),
                original_json_path: None,
                asset_count: 1,
                file_size_bytes: 1,
                downloaded_at: "2026-08-12T00:00:00Z".to_string(),
                source_created_at: None,
                content_hash: None,
                text_length: 1,
                source_updated_at: None,
                watch_updates: false,
                current_version: 1,
                favorite: false,
            })
            .unwrap();
        let missing_asset = work_dir.join("data_assets").join("missing.png");
        state
            .db
            .insert_asset(&NewAsset {
                download_id,
                asset_type: "illustration".to_string(),
                filename: "missing.png".to_string(),
                local_path: missing_asset.to_string_lossy().to_string(),
                original_url: None,
                mime_type: Some("image/png".to_string()),
                file_size_bytes: 1,
            })
            .unwrap();

        let destination = base.join("backup.zip");
        fs::write(&destination, b"previous known-good backup").unwrap();
        let error =
            export_all_zip_internal(state.clone(), destination.to_string_lossy().to_string())
                .await
                .unwrap_err();

        assert!(
            error.contains("asset missing.png"),
            "unexpected error: {error}"
        );
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"previous known-good backup",
            "a failed export must never truncate or replace the last backup"
        );
        assert!(
            !fs::read_dir(&base)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(EXPORT_TEMP_PREFIX)),
            "a failed export must remove its same-directory temporary ZIP"
        );

        drop(state);
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn restore_failure_before_commit_rolls_back_files_database_and_journal() {
        let base = create_temp_dir();
        let (state, json_path, old_id) = restore_target(&base);
        let zip_path = base.join("restore.zip");
        write_restore_test_zip(&zip_path, "New restore title", "newrestoremarker");

        let error = import_zip_internal_with_failpoint(
            state.clone(),
            zip_path.to_string_lossy().to_string(),
            Some("before_db_commit"),
        )
        .await
        .unwrap_err();
        assert!(
            error.contains("before_db_commit"),
            "unexpected error: {error}"
        );

        let download = state
            .db
            .get_download_by_source("pixiv", "crash-restore")
            .unwrap()
            .unwrap();
        assert_eq!(download.id, old_id);
        assert_eq!(download.title, "Old restore title");
        assert!(fs::read_to_string(&json_path)
            .unwrap()
            .contains("oldrestoremarker"));
        assert!(state.db.pending_restore_journals().unwrap().is_empty());
        assert!(!fs::read_dir(&base)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".restore-staging-")));
        assert!(crate::database::tantivy_index::matching_download_ids(
            state.db.storage_dir(),
            "newrestoremarker"
        )
        .unwrap()
        .is_empty());
        assert!(state
            .db
            .get_work_collection("collection-crash-restore")
            .is_err());

        drop(state);
        remove_temp_dir(&base);
    }

    /// 回復できない後始末が残っていても、起動そのものは止めない。
    ///
    /// かつてここは `Err` を返し、`lib.rs` の `?` がそれを起動失敗にしていた。
    /// しかも控えを `rename` で消費していたので、一度失敗すると次の起動では
    /// 控えが無く、同じ場所で永久に落ち続けた。読める棚を人質に取る形だった。
    #[test]
    fn unrecoverable_restore_journal_is_reported_without_blocking_startup() {
        let base = create_temp_dir();
        let (state, _json_path, _old_id) = restore_target(&base);
        let missing_stage = base.join(format!(".restore-staging-{}-gone", std::process::id()));
        state
            .db
            .create_restore_journal("journal-gone", &missing_stage)
            .unwrap();

        let first = recover_interrupted_restores(&state);
        assert_eq!(first.len(), 1, "unexpected recovery result: {first:?}");
        // 何度でも同じ答えになる。次の起動でやり直せる。
        let second = recover_interrupted_restores(&state);
        assert_eq!(second, first);
        assert_eq!(state.db.pending_restore_journals().unwrap().len(), 1);

        drop(state);
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn restore_failure_after_commit_is_finalized_by_startup_recovery() {
        let base = create_temp_dir();
        let (state, json_path, old_id) = restore_target(&base);
        let zip_path = base.join("restore.zip");
        write_restore_test_zip(&zip_path, "New restore title", "newrestoremarker");

        let error = import_zip_internal_with_failpoint(
            state.clone(),
            zip_path.to_string_lossy().to_string(),
            Some("after_db_commit"),
        )
        .await
        .unwrap_err();
        assert!(
            error.contains("after_db_commit"),
            "unexpected error: {error}"
        );

        let restored = state
            .db
            .get_download_by_source("pixiv", "crash-restore")
            .unwrap()
            .unwrap();
        assert_ne!(restored.id, old_id);
        assert_eq!(restored.title, "New restore title");
        let restored_collection = state
            .db
            .get_work_collection("collection-crash-restore")
            .unwrap();
        assert_eq!(restored_collection.members.len(), 1);
        assert_eq!(
            restored_collection.members[0].download_id,
            Some(restored.id)
        );
        assert!(fs::read_to_string(&json_path)
            .unwrap()
            .contains("newrestoremarker"));
        let pending = state.db.pending_restore_journals().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].2, "SQLite commit marker is the authority");
        assert!(
            crate::database::tantivy_index::matching_download_ids(
                state.db.storage_dir(),
                "newrestoremarker"
            )
            .unwrap()
            .is_empty(),
            "derived index must not update before DB commit processing"
        );

        assert!(recover_interrupted_restores(&state).is_empty());
        assert!(state.db.pending_restore_journals().unwrap().is_empty());
        assert!(!crate::database::tantivy_index::matching_download_ids(
            state.db.storage_dir(),
            "oldrestoremarker"
        )
        .unwrap()
        .contains(&old_id));
        state.db.reindex_download(restored.id).unwrap();
        assert!(crate::database::tantivy_index::matching_download_ids(
            state.db.storage_dir(),
            "newrestoremarker"
        )
        .unwrap()
        .contains(&restored.id));

        drop(state);
        remove_temp_dir(&base);
    }

    #[test]
    fn failed_promotion_rolls_back_overwritten_files() {
        let base = create_temp_dir();
        let stage = base.join("stage");
        let live = base.join("live");
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&live).unwrap();
        let first_staged = stage.join("first-new");
        let second_staged = stage.join("second-new");
        let first_live = live.join("first");
        let second_live = live.join("second");
        fs::write(&first_staged, b"new").unwrap();
        fs::write(&second_staged, b"new-second").unwrap();
        fs::write(&first_live, b"old").unwrap();
        fs::create_dir_all(&second_live).unwrap();

        let result = promote_staged_files(
            &stage,
            &[
                (first_staged, first_live.clone()),
                (second_staged, second_live),
            ],
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&first_live).unwrap(), b"old");
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn import_rejects_zip_slip_before_creating_outside_directories() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = Arc::new(AppState::new(db));
        let zip_path = base.join("malicious.zip");
        let outside_name = format!("{}_outside", base.file_name().unwrap().to_string_lossy());
        let outside_path = base.parent().unwrap().join(&outside_name);
        let malicious_name = format!("../{outside_name}/escaped.txt");
        write_test_zip(&zip_path, &[(malicious_name.as_str(), b"owned")]);

        let error = import_zip_internal(state, zip_path.to_string_lossy().to_string())
            .await
            .unwrap_err();
        assert!(error.contains("Security Exception"));
        assert!(!outside_path.exists());
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn import_rejects_traversal_in_metadata_before_database_mutation() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = Arc::new(AppState::new(db));
        let zip_path = base.join("malicious_metadata.zip");
        let metadata = BackupMetadata {
            version: "1".to_string(),
            created_at: "2026-08-09T00:00:00Z".to_string(),
            entries: vec![],
            people: vec![BackupPerson {
                source: "pixiv".to_string(),
                source_key: "attacker".to_string(),
                display_name: "attacker".to_string(),
                relative_icon_path: Some("../outside.png".to_string()),
                relative_cover_path: None,
                description: None,
                links_json: None,
                content_hash: None,
                current_version: 1,
                last_checked_at: None,
                last_fetched_at: None,
                created_at: "2026-08-09T00:00:00Z".to_string(),
                updated_at: "2026-08-09T00:00:00Z".to_string(),
                versions: vec![],
            }],
            series: vec![],
            update_targets: vec![],
            update_candidates: vec![],
            collections: vec![],
            saved_searches: vec![],
            collection_pair_feedback: vec![],
        };
        let metadata_json = serde_json::to_vec(&metadata).unwrap();
        write_test_zip(
            &zip_path,
            &[("backup_metadata.json", metadata_json.as_slice())],
        );

        let error = import_zip_internal(state.clone(), zip_path.to_string_lossy().to_string())
            .await
            .unwrap_err();
        assert!(error.contains("Invalid backup metadata"));
        assert!(state.db.get_person("pixiv", "attacker").is_err());
        assert!(!fs::read_dir(&base)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".restore-staging-")
            }));
        drop(state);
        remove_temp_dir(&base);
    }

    #[test]
    fn reimport_recovery_never_selects_an_existing_library_record() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let json = storage.join("pixiv/existing/v1/original.json");
        fs::create_dir_all(json.parent().unwrap()).unwrap();
        fs::write(&json, br#"{"title":"kept"}"#).unwrap();
        db.upsert_download(&NewDownload {
            source: "pixiv".to_string(),
            source_id: "existing".to_string(),
            title: "DBで保持する作品".to_string(),
            author_name: "作者".to_string(),
            author_id: "author".to_string(),
            content_type: "novel".to_string(),
            tags: vec!["保持".to_string()],
            excerpt: None,
            cover_path: None,
            json_path: json.to_string_lossy().to_string(),
            original_json_path: Some(json.to_string_lossy().to_string()),
            asset_count: 0,
            file_size_bytes: 16,
            downloaded_at: "2026-08-16T00:00:00Z".to_string(),
            source_created_at: None,
            content_hash: Some("kept".to_string()),
            text_length: 0,
            source_updated_at: None,
            watch_updates: true,
            current_version: 1,
            favorite: true,
        })
        .unwrap();

        assert!(!work_needs_reimport(&db, "pixiv", "existing").unwrap());
        assert!(work_needs_reimport(&db, "pixiv", "missing").unwrap());
        assert!(
            json.exists(),
            "the recovery decision must not touch source files"
        );
        drop(db);
        remove_temp_dir(&base);
    }

    #[test]
    fn reimport_path_validation_rejects_oversized_json_and_links() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let version = storage.join("pixiv/work/v1");
        fs::create_dir_all(&version).unwrap();
        let oversized = version.join("original.json");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(MAX_BACKUP_METADATA_BYTES + 1).unwrap();
        assert!(canonical_reimport_json(&oversized, &version)
            .unwrap_err()
            .contains("too large"));

        let outside = base.join("outside.json");
        fs::write(&outside, b"{}").unwrap();
        let link = version.join("data.json");
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&outside, &link);
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&outside, &link);
        #[cfg(not(any(unix, windows)))]
        let link_result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "links unavailable",
        ));
        if link_result.is_ok() {
            assert!(canonical_reimport_json(&link, &version)
                .unwrap_err()
                .contains("regular file"));
        }
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn test_export_and_import_integration() {
        // --- 1. テスト環境 (送信側) のセットアップ ---
        let base_dir = create_temp_dir();
        let db_path = base_dir.join("piep.db");
        let storage_dir = base_dir.join("downloads");

        let db = Database::open(&db_path, &storage_dir).unwrap();
        let state = Arc::new(AppState::new(db));

        // サンプル作品データの定義
        let source = "pixiv".to_string();
        let source_id = "123456".to_string();

        // 物理ファイル（バージョン1, 2のデータ）の作成
        let v1_dir = storage_dir.join(&source).join(&source_id).join("v1");
        let v2_dir = storage_dir.join(&source).join(&source_id).join("v2");
        fs::create_dir_all(&v1_dir).unwrap();
        fs::create_dir_all(&v2_dir).unwrap();

        let v1_json_path = v1_dir.join("data.json");
        let v2_json_path = v2_dir.join("data.json");
        let v2_orig_json_path = v2_dir.join("original.json");
        let cover_path = v2_dir.join("cover.jpg");

        fs::write(&v1_json_path, b"{\"title\":\"Ver 1 Title\"}").unwrap();
        fs::write(&v2_json_path, b"{\"title\":\"Ver 2 Title\"}").unwrap();
        fs::write(&v2_orig_json_path, b"{\"original_info\":\"something\"}").unwrap();
        fs::write(&cover_path, b"cover bytes").unwrap();

        // アセットフォルダと物理ファイルの作成
        let asset_dir = v2_dir.join("data_assets").join("illustration");
        fs::create_dir_all(&asset_dir).unwrap();
        let asset_file_path = asset_dir.join("image1.png");
        fs::write(&asset_file_path, b"dummy png binary data").unwrap();

        // お気に入り・監視設定ON、最新バージョン2の作品をDB登録
        let new_dl = NewDownload {
            source: source.clone(),
            source_id: source_id.clone(),
            title: "テスト小説".to_string(),
            author_name: "テスト著者".to_string(),
            author_id: "999".to_string(),
            content_type: "novel".to_string(),
            tags: vec!["Tag1".to_string(), "Tag2".to_string()],
            excerpt: Some("あらすじ".to_string()),
            cover_path: Some(cover_path.to_string_lossy().to_string()),
            json_path: v2_json_path.to_string_lossy().to_string(),
            original_json_path: Some(v2_orig_json_path.to_string_lossy().to_string()),
            asset_count: 1,
            file_size_bytes: 100,
            downloaded_at: "2026-05-21T10:00:00Z".to_string(),
            source_created_at: Some("2026-05-21T00:00:00Z".to_string()),
            content_hash: Some("v2hash".to_string()),
            text_length: 500,
            source_updated_at: None,
            watch_updates: true, // 更新監視ON
            current_version: 2,
            favorite: true, // お気に入りON
        };
        let dl_id = state.db.upsert_download(&new_dl).unwrap();
        state
            .db
            .upsert_download_relation(dl_id, "author", &source, "999", "テスト著者")
            .unwrap();
        state
            .db
            .upsert_download_person(dl_id, &source, "999", "author", "テスト著者")
            .unwrap();

        // バージョン1履歴をDB登録
        let v1_history = NewVersion {
            download_id: dl_id,
            version: 1,
            content_hash: Some("v1hash".to_string()),
            text_length: 450,
            json_path: v1_json_path.to_string_lossy().to_string(),
            original_json_path: None,
            asset_count: 0,
            file_size_bytes: 50,
            created_at: "2026-05-21T05:00:00Z".to_string(),
            change_summary: Some("初版".to_string()),
        };
        state.db.insert_version(&v1_history).unwrap();

        // バージョン2履歴をDB登録
        let v2_history = NewVersion {
            download_id: dl_id,
            version: 2,
            content_hash: Some("v2hash".to_string()),
            text_length: 500,
            json_path: v2_json_path.to_string_lossy().to_string(),
            original_json_path: Some(v2_orig_json_path.to_string_lossy().to_string()),
            asset_count: 1,
            file_size_bytes: 100,
            created_at: "2026-05-21T10:00:00Z".to_string(),
            change_summary: Some("誤字修正".to_string()),
        };
        state.db.insert_version(&v2_history).unwrap();

        // アセット情報をDB登録
        let new_asset = NewAsset {
            download_id: dl_id,
            asset_type: "illustration".to_string(),
            filename: "image1.png".to_string(),
            local_path: asset_file_path.to_string_lossy().to_string(),
            original_url: Some("http://example.com/img1.png".to_string()),
            mime_type: Some("image/png".to_string()),
            file_size_bytes: 21,
        };
        state.db.insert_asset(&new_asset).unwrap();

        state
            .db
            .restore_tags(
                dl_id,
                &[
                    PortableTag {
                        name: "Tag1".into(),
                        source: "origin".into(),
                    },
                    PortableTag {
                        name: "手動".into(),
                        source: "manual".into(),
                    },
                    PortableTag {
                        name: "モデル".into(),
                        source: "llm".into(),
                    },
                ],
            )
            .unwrap();
        state
            .db
            .restore_work_edits(
                dl_id,
                &[PortableWorkEdit {
                    base_version: 2,
                    status: "active".into(),
                    title: Some("利用者が直した題名".into()),
                    content_hash: Some("edit-hash".into()),
                    created_at: "2026-05-22T00:00:00Z".into(),
                    updated_at: "2026-05-23T00:00:00Z".into(),
                    blocks: vec![crate::database::queries::PortableWorkEditBlock {
                        order: 0,
                        block_type: "image".into(),
                        text: Some("利用者のキャプション".into()),
                        asset_path: Some(asset_file_path.to_string_lossy().to_string()),
                        attrs_json: Some("{\"align\":\"center\"}".into()),
                    }],
                }],
            )
            .unwrap();
        state
            .db
            .restore_saved_search(&SavedSearch {
                id: 0,
                name: "あとで読む催眠".into(),
                query: Some("催眠".into()),
                params_json: "{\"favorite\":true}".into(),
                created_at: "2026-05-22T00:00:00Z".into(),
                updated_at: "2026-05-23T00:00:00Z".into(),
            })
            .unwrap();
        state
            .db
            .upsert_update_candidate(&crate::database::UpdateCandidateInput {
                source: "pixiv".into(),
                source_id: "portable-pending-work".into(),
                kind: "new".into(),
                title: "まだ保存していない作品".into(),
                payload_json: r#"{"id":"portable-pending-work"}"#.into(),
                target_type: Some("author".into()),
            })
            .unwrap();
        state
            .db
            .set_update_candidate_status("pixiv", "portable-pending-work", "dismissed")
            .unwrap();
        state
            .db
            .restore_collection_pair_feedback(&PortableCollectionPairFeedback {
                left_source: source.clone(),
                left_source_id: source_id.clone(),
                right_source: "fanbox".into(),
                right_source_id: "other-work".into(),
                decision: "reject".into(),
                rule_version: "collection-suggest-v2".into(),
                updated_at: "2026-05-23T00:00:00Z".into(),
            })
            .unwrap();
        let custom_cover = base_dir.join("chosen-cover.png");
        fs::write(&custom_cover, b"custom collection cover").unwrap();
        state
            .db
            .restore_work_collection(
                &WorkCollectionInput {
                    id: Some("collection-backup-state".into()),
                    name: "モデルが付けた束名".into(),
                    description: Some("利用者の説明".into()),
                    collection_kind: "unordered".into(),
                    cover_download_id: Some(dl_id),
                    cover_mode: Some("file".into()),
                    cover_image_path: Some(custom_cover.to_string_lossy().to_string()),
                    name_source: Some("llm".into()),
                    track: Some("theme".into()),
                },
                Some(&WorkKey {
                    source: source.clone(),
                    source_id: source_id.clone(),
                }),
                &[WorkCollectionMemberInput {
                    source: source.clone(),
                    source_id: source_id.clone(),
                    title_snapshot: Some("テスト小説".into()),
                    author_snapshot: Some("テスト著者".into()),
                    position: Some(0),
                    member_role: Some("main".into()),
                    added_by: Some("manual".into()),
                    pinned: Some(true),
                    note: Some("利用者のメモ".into()),
                }],
            )
            .unwrap();

        // ZIPの書き出しパス
        let zip_path = base_dir.join("backup.zip");
        fs::write(&zip_path, b"previous backup").unwrap();

        // --- 2. バックアップエクスポートの実行 ---
        export_all_zip_internal(state.clone(), zip_path.to_string_lossy().to_string())
            .await
            .unwrap();
        assert!(zip_path.exists(), "ZIP backup file should be created");

        // --- 3. 新しいクリーンな環境のセットアップ (受信側) ---
        let base_dir_new = create_temp_dir();
        let db_path_new = base_dir_new.join("piep.db");
        let storage_dir_new = base_dir_new.join("downloads");

        let db_new = Database::open(&db_path_new, &storage_dir_new).unwrap();
        let state_new = Arc::new(AppState::new(db_new));

        let inspection =
            inspect_backup_internal(&zip_path.to_string_lossy(), &storage_dir_new, &base_dir_new)
                .unwrap();
        assert!(
            inspection.valid,
            "inspection failed: {:?}",
            inspection.error
        );
        assert_eq!(inspection.work_count, 1);
        assert_eq!(inspection.version_count, 2);
        assert_eq!(inspection.asset_count, 1);

        // --- 4. バックアップインポートの実行 ---
        let count = import_zip_internal(state_new.clone(), zip_path.to_string_lossy().to_string())
            .await
            .unwrap();
        assert_eq!(count, 1, "Should successfully restore 1 download");

        // --- 5. 復元データの厳密アサーション ---
        let restored_dl = state_new
            .db
            .get_download_by_source(&source, &source_id)
            .unwrap()
            .unwrap();
        assert_eq!(restored_dl.title, "テスト小説");
        assert_eq!(restored_dl.author_name, "テスト著者");
        assert_eq!(
            state_new.db.archive_tags(restored_dl.id).unwrap(),
            vec![
                PortableTag {
                    name: "Tag1".into(),
                    source: "origin".into()
                },
                PortableTag {
                    name: "モデル".into(),
                    source: "llm".into()
                },
                PortableTag {
                    name: "手動".into(),
                    source: "manual".into()
                },
            ]
        );
        assert!(restored_dl
            .cover_path
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file()));
        assert!(restored_dl.favorite, "Favorite flag must be restored");
        assert!(
            restored_dl.watch_updates,
            "Watch updates flag must be restored"
        );
        assert_eq!(
            restored_dl.current_version, 2,
            "Current version must be restored"
        );
        assert_eq!(
            restored_dl.content_hash,
            Some("v2hash".to_string()),
            "Current content hash must be restored to the main row"
        );
        assert_eq!(
            restored_dl.text_length, 500,
            "Current text length must be restored to the main row"
        );
        assert_eq!(
            restored_dl.file_size_bytes, 100,
            "Current file size must be restored to the main row"
        );
        let restored_person = state_new.db.get_person(&source, "999").unwrap();
        assert_eq!(restored_person.display_name, "テスト著者");
        assert_eq!(restored_person.work_count, Some(1));

        // バージョン履歴の復元チェック
        let restored_versions = state_new.db.get_versions(restored_dl.id).unwrap();
        assert_eq!(
            restored_versions.len(),
            2,
            "Should restore exactly 2 versions"
        );

        let v2_restored = restored_versions.iter().find(|v| v.version == 2).unwrap();
        assert_eq!(v2_restored.content_hash, Some("v2hash".to_string()));
        assert_eq!(v2_restored.change_summary, Some("誤字修正".to_string()));
        assert!(
            Path::new(&v2_restored.json_path).exists(),
            "Restored json file must exist physically"
        );

        let v1_restored = restored_versions.iter().find(|v| v.version == 1).unwrap();
        assert_eq!(v1_restored.content_hash, Some("v1hash".to_string()));
        assert_eq!(v1_restored.change_summary, Some("初版".to_string()));
        assert!(
            Path::new(&v1_restored.json_path).exists(),
            "Restored json file must exist physically"
        );

        // アセット情報の復元チェック
        let restored_assets = state_new.db.get_assets(restored_dl.id).unwrap();
        assert_eq!(restored_assets.len(), 1, "Should restore exactly 1 asset");
        assert_eq!(restored_assets[0].filename, "image1.png");
        assert_eq!(restored_assets[0].asset_type, "illustration");
        assert!(
            Path::new(&restored_assets[0].local_path).exists(),
            "Restored asset physical file must exist"
        );
        assert_eq!(
            fs::read_to_string(&restored_assets[0].local_path).unwrap(),
            "dummy png binary data"
        );
        let edits = state_new.db.archive_work_edits(restored_dl.id).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].title.as_deref(), Some("利用者が直した題名"));
        assert_eq!(
            edits[0].blocks[0].text.as_deref(),
            Some("利用者のキャプション")
        );
        assert!(edits[0].blocks[0]
            .asset_path
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file()));
        let searches = state_new.db.list_saved_searches().unwrap();
        assert_eq!(searches.len(), 1);
        assert_eq!(searches[0].name, "あとで読む催眠");
        let restored_candidates = state_new.db.list_update_candidates_after(None, 10).unwrap();
        assert_eq!(restored_candidates.len(), 1);
        assert_eq!(restored_candidates[0].source_id, "portable-pending-work");
        assert_eq!(restored_candidates[0].status, "dismissed");
        assert_eq!(
            state_new.db.archive_collection_pair_feedback().unwrap(),
            vec![PortableCollectionPairFeedback {
                left_source: source.clone(),
                left_source_id: source_id.clone(),
                right_source: "fanbox".into(),
                right_source_id: "other-work".into(),
                decision: "reject".into(),
                rule_version: "collection-suggest-v2".into(),
                updated_at: "2026-05-23T00:00:00Z".into(),
            }]
        );
        let restored_collection = state_new
            .db
            .get_work_collection("collection-backup-state")
            .unwrap();
        assert_eq!(restored_collection.summary.cover_mode, "file");
        assert_eq!(restored_collection.summary.name_source, "llm");
        assert_eq!(restored_collection.summary.track, "theme");
        assert!(restored_collection
            .summary
            .cover_image_path
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file()));

        // クリーンアップ
        let _ = fs::remove_dir_all(&base_dir);
        let _ = fs::remove_dir_all(&base_dir_new);
    }

    #[test]
    fn multipart_manifest_rejects_tampered_or_escaping_parts() {
        let base = create_temp_dir();
        let part_path = base.join("backup-part-00001.zip");
        fs::write(&part_path, b"test-part").unwrap();
        let manifest_path = base.join("backup.json");
        let mut manifest = MultipartBackupManifest {
            format: "piep-multipart-1".to_string(),
            created_at: "2026-08-12T00:00:00Z".to_string(),
            total_works: 1,
            parts: vec![MultipartBackupPart {
                file: "backup-part-00001.zip".to_string(),
                work_count: 1,
                bytes: fs::metadata(&part_path).unwrap().len(),
                sha256: sha256_file(&part_path).unwrap(),
            }],
        };
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let (_, paths) = read_and_validate_multipart_manifest(&manifest_path).unwrap();
        assert_eq!(paths, vec![part_path.canonicalize().unwrap()]);

        fs::write(&part_path, b"tampered").unwrap();
        assert!(read_and_validate_multipart_manifest(&manifest_path)
            .unwrap_err()
            .contains("mismatch"));

        manifest.parts[0].file = "../outside.zip".to_string();
        manifest.parts[0].bytes = 1;
        manifest.parts[0].sha256 = "0".repeat(64);
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(read_and_validate_multipart_manifest(&manifest_path).is_err());
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn multipart_restore_preflights_and_imports_every_part() {
        let base = create_temp_dir();
        let source_dir = base.join("source");
        let target_dir = base.join("target");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&target_dir).unwrap();
        let first = source_dir.join("backup-part-00001.zip");
        let second = source_dir.join("backup-part-00002.zip");
        write_restore_test_zip_for_id(&first, "First", "first-marker", "multipart-1");
        write_restore_test_zip_for_id(&second, "Second", "second-marker", "multipart-2");
        let manifest = MultipartBackupManifest {
            format: "piep-multipart-1".to_string(),
            created_at: "2026-08-12T00:00:00Z".to_string(),
            total_works: 2,
            parts: [&first, &second]
                .iter()
                .enumerate()
                .map(|(index, path)| MultipartBackupPart {
                    file: format!("backup-part-{:05}.zip", index + 1),
                    work_count: 1,
                    bytes: fs::metadata(path).unwrap().len(),
                    sha256: sha256_file(path).unwrap(),
                })
                .collect(),
        };
        let manifest_path = source_dir.join("backup.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let state = Arc::new(AppState::new(
            Database::open(&target_dir.join("piep.db"), &target_dir.join("downloads")).unwrap(),
        ));

        let imported = import_multipart_backup_internal(
            state.clone(),
            manifest_path.to_string_lossy().to_string(),
        )
        .await
        .unwrap();
        assert_eq!(imported, 2);
        assert_eq!(
            state
                .db
                .get_download_by_source("pixiv", "multipart-1")
                .unwrap()
                .unwrap()
                .title,
            "First"
        );
        assert_eq!(
            state
                .db
                .get_download_by_source("pixiv", "multipart-2")
                .unwrap()
                .unwrap()
                .title,
            "Second"
        );
        // 全パートが入り切ったら、途中経過の記録は残さない。残っていると
        // 起動のたびに「復元が途中で終わっています」と言い続けることになる。
        assert!(state.db.unfinished_restore_manifests().unwrap().is_empty());
        drop(state);
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn empty_library_multipart_preserves_update_targets_and_candidates() {
        let base = create_temp_dir();
        let source_root = base.join("source-library");
        let target_root = base.join("target-library");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        let source_state = Arc::new(AppState::new(
            Database::open(&source_root.join("piep.db"), &source_root.join("downloads")).unwrap(),
        ));
        source_state
            .db
            .upsert_update_target(&crate::database::UpdateTargetInput {
                target_type: "author".to_string(),
                source: "pixiv".to_string(),
                source_key: "empty-library-author".to_string(),
                display_name: "監視中の作者".to_string(),
                enabled: true,
                metadata_json: Some(r#"{"reason":"followed"}"#.to_string()),
            })
            .unwrap();
        source_state
            .db
            .mark_update_target_checked(
                "author",
                "pixiv",
                "empty-library-author",
                Some("unhandled-work-42"),
                Some("2026-08-27T00:00:00Z"),
                1,
            )
            .unwrap();
        source_state
            .db
            .upsert_update_candidate(&crate::database::UpdateCandidateInput {
                source: "pixiv".into(),
                source_id: "unhandled-work-42".into(),
                kind: "new".into(),
                title: "次に判断する作品".into(),
                payload_json: r#"{"id":"unhandled-work-42"}"#.into(),
                target_type: Some("author".into()),
            })
            .unwrap();
        let manifest_path = base.join("empty-backup.json");
        export_all_multipart_internal(
            source_state.clone(),
            manifest_path.to_string_lossy().to_string(),
        )
        .await
        .unwrap();
        let (manifest, _) = read_and_validate_multipart_manifest(&manifest_path).unwrap();
        assert_eq!(manifest.total_works, 0);
        assert_eq!(manifest.parts.len(), 2);
        assert!(manifest.parts.iter().all(|part| part.work_count == 0));

        let target_state = Arc::new(AppState::new(
            Database::open(&target_root.join("piep.db"), &target_root.join("downloads")).unwrap(),
        ));
        assert_eq!(
            import_multipart_backup_internal(
                target_state.clone(),
                manifest_path.to_string_lossy().to_string(),
            )
            .await
            .unwrap(),
            0
        );
        let restored = target_state
            .db
            .find_update_target("author", "pixiv", "empty-library-author")
            .unwrap()
            .unwrap();
        assert_eq!(restored.display_name, "監視中の作者");
        assert_eq!(
            restored.last_seen_source_id.as_deref(),
            Some("unhandled-work-42")
        );
        assert_eq!(
            restored.metadata_json.as_deref(),
            Some(r#"{"reason":"followed"}"#)
        );
        assert_eq!(
            target_state
                .db
                .update_candidate_status("pixiv", "unhandled-work-42")
                .unwrap()
                .as_deref(),
            Some("pending")
        );
        drop(source_state);
        drop(target_state);
        // A Windows SQLite/WAL handle owned by a just-finished blocking import
        // can outlive this assertion by one scheduler tick. The temporary
        // directory is OS-scoped; cleanup must not turn a successful portable
        // round trip into a flaky product failure.
        remove_temp_dir(&base);
    }

    #[tokio::test]
    async fn multipart_catalog_preserves_orphan_people_and_series() {
        let base = create_temp_dir();
        let source_root = base.join("source-library");
        let target_root = base.join("target-library");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        let source_state = Arc::new(AppState::new(
            Database::open(&source_root.join("piep.db"), &source_root.join("downloads")).unwrap(),
        ));

        let person_json = source_root.join("profiles/pixiv/orphan-person/v1/data.json");
        let series_json = source_root.join("series/pixiv/orphan-series/v1/data.json");
        fs::create_dir_all(person_json.parent().unwrap()).unwrap();
        fs::create_dir_all(series_json.parent().unwrap()).unwrap();
        fs::write(&person_json, r#"{"name":"孤立した作者"}"#.as_bytes()).unwrap();
        fs::write(&series_json, r#"{"title":"孤立したシリーズ"}"#.as_bytes()).unwrap();
        source_state
            .db
            .upsert_person_profile(
                "pixiv",
                "orphan-person",
                "孤立した作者",
                None,
                None,
                Some("作品がなくても保持されるプロフィール"),
                None,
                "orphan-person-hash",
                &person_json.to_string_lossy(),
                0,
                fs::metadata(&person_json).unwrap().len() as i64,
                EntityProfileFreshness::RemoteChecked,
            )
            .unwrap();
        source_state
            .db
            .upsert_series_profile(
                "pixiv",
                "orphan-series",
                "孤立したシリーズ",
                Some("作品との関連がなくても保持"),
                None,
                "orphan-series-hash",
                &series_json.to_string_lossy(),
                0,
                fs::metadata(&series_json).unwrap().len() as i64,
                EntityProfileFreshness::RemoteChecked,
                None,
                None,
            )
            .unwrap();
        source_state
            .db
            .upsert_update_target(&crate::database::UpdateTargetInput {
                target_type: "author".to_string(),
                source: "pixiv".to_string(),
                source_key: "orphan-person".to_string(),
                display_name: "孤立した作者".to_string(),
                enabled: true,
                metadata_json: None,
            })
            .unwrap();

        let manifest_path = base.join("orphan-catalog-backup.json");
        export_all_multipart_internal(
            source_state.clone(),
            manifest_path.to_string_lossy().to_string(),
        )
        .await
        .unwrap();
        let (manifest, paths) = read_and_validate_multipart_manifest(&manifest_path).unwrap();
        assert_eq!(manifest.total_works, 0);
        assert_eq!(manifest.parts.len(), 3);
        assert!(manifest.parts.iter().all(|part| part.work_count == 0));
        let mut person_count = 0;
        let mut series_count = 0;
        let mut update_target_count = 0;
        for path in paths {
            let file = fs::File::open(path).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            let mut metadata_json = String::new();
            zip.by_name("backup_metadata.json")
                .unwrap()
                .read_to_string(&mut metadata_json)
                .unwrap();
            let metadata: BackupMetadata = serde_json::from_str(&metadata_json).unwrap();
            person_count += metadata.people.len();
            series_count += metadata.series.len();
            update_target_count += metadata.update_targets.len();
        }
        assert_eq!(person_count, 1);
        assert_eq!(series_count, 1);
        assert_eq!(update_target_count, 1);

        let target_state = Arc::new(AppState::new(
            Database::open(&target_root.join("piep.db"), &target_root.join("downloads")).unwrap(),
        ));
        assert_eq!(
            import_multipart_backup_internal(
                target_state.clone(),
                manifest_path.to_string_lossy().to_string(),
            )
            .await
            .unwrap(),
            0
        );
        let person = target_state
            .db
            .get_person("pixiv", "orphan-person")
            .unwrap();
        assert_eq!(person.display_name, "孤立した作者");
        assert_eq!(person.work_count, Some(0));
        let series = target_state
            .db
            .get_series("pixiv", "orphan-series")
            .unwrap();
        assert_eq!(series.title, "孤立したシリーズ");
        assert_eq!(series.work_count, Some(0));
        assert!(target_state
            .db
            .find_update_target("author", "pixiv", "orphan-person")
            .unwrap()
            .is_some());

        drop(source_state);
        drop(target_state);
        remove_temp_dir(&base);
    }
}
