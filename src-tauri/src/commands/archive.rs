use crate::database::{
    EntityVersion, NewAsset, NewDownload, NewVersion, SearchV2Params, SeriesEntry, UpdateTarget,
};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use tauri::Manager;

static IMPORT_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

const MAX_BACKUP_ENTRIES: usize = 250_000;
const MAX_BACKUP_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_BACKUP_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_BACKUP_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BACKUP_COMPRESSION_RATIO: u64 = 250;

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
        Some("profiles" | "series") => app_data_dir,
        _ => storage_dir,
    };
    Ok((root.join(relative), root))
}

fn resolve_storage_metadata_path(raw: &str, storage_dir: &Path) -> Result<PathBuf, String> {
    let relative = validated_backup_relative_path(raw)?;
    if matches!(
        first_backup_component(&relative),
        Some("profiles" | "series")
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
    fn commit(mut self) {
        self.committed = true;
        for file in &self.files {
            if let Some(backup) = &file.backup {
                let _ = std::fs::remove_file(backup);
            }
        }
        let _ = std::fs::remove_dir_all(&self.stage_root);
    }
}

impl Drop for PromotionGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for file in self.files.iter().rev() {
            let _ = std::fs::remove_file(&file.destination);
            if let Some(backup) = &file.backup {
                let _ = std::fs::rename(backup, &file.destination);
            }
        }
        let _ = std::fs::remove_dir_all(&self.stage_root);
    }
}

fn promote_staged_files(
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
            std::fs::rename(destination, &backup).map_err(|e| {
                format!(
                    "Could not stage existing destination {}: {e}",
                    destination.display()
                )
            })?;
            Some(backup)
        } else {
            None
        };
        if let Err(error) = std::fs::rename(staged, destination) {
            if let Some(backup) = &backup {
                let _ = std::fs::rename(backup, destination);
            }
            return Err(format!(
                "Could not promote restored file {}: {error}",
                destination.display()
            ));
        }
        guard.files.push(PromotedFile {
            destination: destination.clone(),
            backup,
        });
    }
    Ok(guard)
}

fn add_zip_file_once(
    zip: &mut zip::ZipWriter<std::fs::File>,
    options: zip::write::SimpleFileOptions,
    written: &mut HashSet<String>,
    relative_path: &str,
    source_path: &Path,
) -> Result<(), String> {
    if !source_path.exists() || !written.insert(relative_path.to_string()) {
        return Ok(());
    }
    let content = std::fs::read(source_path).map_err(|e| e.to_string())?;
    zip.start_file(relative_path, options)
        .map_err(|e| e.to_string())?;
    zip.write_all(&content).map_err(|e| e.to_string())?;
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
            let asset_type_dest = assets_dest.join(&asset.asset_type);
            tokio::fs::create_dir_all(&asset_type_dest)
                .await
                .map_err(|e| e.to_string())?;
            tokio::fs::copy(src, asset_type_dest.join(&asset.filename))
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
        limit: Some(10000),
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

pub async fn export_all_zip_internal(state: Arc<AppState>, zip_path: String) -> Result<(), String> {
    export_zip_with_params_internal(state, zip_path, all_backup_search_params(), false).await
}

pub async fn export_zip_with_params_internal(
    state: Arc<AppState>,
    zip_path: String,
    params: SearchV2Params,
    scoped: bool,
) -> Result<(), String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();

    let all = state
        .db
        .search_downloads_v2_internal(&params, 20_000)?
        .items;

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&zip_path).map_err(|e| e.to_string())?;
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
            let relations = state.db.get_download_relations_for_download(dl.id)?;
            let people = state.db.get_download_people(dl.id)?;
            let series = state.db.get_download_series_list(dl.id)?;
            for person in &people {
                person_scope.insert((person.person_source.clone(), person.person_key.clone()));
            }
            for item in &series {
                series_scope.insert((item.series_source.clone(), item.series_key.clone()));
            }

            let mut backup_versions = Vec::new();
            for v in &versions {
                let relative_json_path =
                    get_relative_path(&v.json_path, &storage).unwrap_or_else(|| {
                        format!("{}/{}/v{}/data.json", dl.source, dl.source_id, v.version)
                    });

                let relative_original_json_path = v
                    .original_json_path
                    .as_ref()
                    .and_then(|p| get_relative_path(p, &storage));

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
                if let Some(ref ojp) = v.original_json_path {
                    if let Some(ref rel_orig) = relative_original_json_path {
                        add_zip_file_once(
                            &mut zip,
                            options,
                            &mut written_files,
                            rel_orig,
                            Path::new(ojp),
                        )?;
                    }
                }

                backup_versions.push(BackupVersion {
                    version: v.version,
                    content_hash: v.content_hash.clone(),
                    text_length: v.text_length,
                    file_size_bytes: v.file_size_bytes,
                    created_at: v.created_at.clone(),
                    change_summary: v.change_summary.clone(),
                    relative_json_path,
                    relative_original_json_path,
                });
            }

            let mut backup_assets = Vec::new();
            for asset in &assets {
                let relative_local_path = get_relative_path(&asset.local_path, &storage)
                    .unwrap_or_else(|| {
                        format!(
                            "{}/{}/v{}/data_assets/{}/{}",
                            dl.source,
                            dl.source_id,
                            dl.current_version,
                            asset.asset_type,
                            asset.filename
                        )
                    });

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

            let relative_cover_path = dl
                .cover_path
                .as_ref()
                .and_then(|p| get_relative_path(p, &storage));
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
            });
        }

        let mut backup_people = Vec::new();
        for person in state.db.list_people()? {
            if scoped && !person_scope.contains(&(person.source.clone(), person.source_key.clone()))
            {
                continue;
            }
            let mut versions = Vec::new();
            for version in
                state
                    .db
                    .list_entity_versions("person", &person.source, &person.source_key)?
            {
                let relative_json_path = get_relative_path(&version.json_path, &app_data)
                    .unwrap_or_else(|| {
                        format!(
                            "profiles/{}/{}/v{}/original.json",
                            safe_export_name(&person.source),
                            safe_export_name(&person.source_key),
                            version.version
                        )
                    });
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
                .as_ref()
                .and_then(|p| get_relative_path(p, &app_data));
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
                .as_ref()
                .and_then(|p| get_relative_path(p, &app_data));
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

        let mut backup_series = Vec::new();
        for series in state.db.list_series()? {
            if scoped && !series_scope.contains(&(series.source.clone(), series.source_key.clone()))
            {
                continue;
            }
            let mut versions = Vec::new();
            for version in
                state
                    .db
                    .list_entity_versions("series", &series.source, &series.source_key)?
            {
                let relative_json_path = get_relative_path(&version.json_path, &app_data)
                    .unwrap_or_else(|| {
                        format!(
                            "series/{}/{}/v{}/original.json",
                            safe_export_name(&series.source),
                            safe_export_name(&series.source_key),
                            version.version
                        )
                    });
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
                .as_ref()
                .and_then(|p| get_relative_path(p, &app_data));
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
        let mut update_targets = state.db.list_update_targets(None, false)?;
        if scoped {
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

        let metadata = BackupMetadata {
            version: "3.0".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            entries: backup_entries,
            people: backup_people,
            series: backup_series,
            update_targets,
        };

        let metadata_json = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())?;
        zip.start_file("backup_metadata.json", options)
            .map_err(|e| e.to_string())?;
        zip.write_all(metadata_json.as_bytes())
            .map_err(|e| e.to_string())?;

        zip.finish().map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("Export thread panicked: {}", e))?
}

#[tauri::command]
pub async fn export_all_zip(app: tauri::AppHandle, zip_path: String) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    export_all_zip_internal(state, zip_path).await
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
    import_zip_internal(state, zip_path).await
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

pub async fn import_zip_internal(state: Arc<AppState>, zip_path: String) -> Result<i64, String> {
    let _import_guard = IMPORT_LOCK.lock().await;
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage.parent().unwrap_or(storage.as_path()).to_path_buf();

    // プレミアム最適化：ZIP解凍からDB挿入までの全同期処理を、非同期ワーカースレッドへ完璧に移譲
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let expanded_bytes = preflight_backup_archive(&mut archive)?;
        let required_free_bytes = expanded_bytes
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

        // ZIP内のファイルを展開
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.is_dir() {
                continue;
            }

            let entry_name = file.name().to_string();
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

            let mut outfile = std::fs::File::create(&outpath).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            outfile.sync_all().map_err(|e| e.to_string())?;
            if entry_name != "backup_metadata.json" {
                let (destination, _) = resolve_zip_import_path(&entry_name, &storage, &app_data)?;
                promotion_entries.push((outpath, destination));
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

            let promotion = promote_staged_files(&stage_root, &promotion_entries)?;
            staging_cleanup.disarm();
            state.db.begin_atomic_restore()?;
            let restore_result = (|| -> Result<i64, String> {

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

            for entry in &metadata.entries {
                // 重複していたら、既存のものを完全に削除して上書きリストアする
                if let Ok(Some(existing)) = state.db.get_download_by_source(&entry.source, &entry.source_id) {
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
                    tags: entry.tags.clone(),
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
                imported += 1;

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

                            let new_asset = NewAsset {
                                download_id: dl_id,
                                asset_type: asset.asset_type.clone(),
                                filename: asset.filename.clone(),
                                local_path: asset_local,
                                original_url: asset.original_url.clone(),
                                mime_type: asset.mime_type.clone(),
                                file_size_bytes: asset.file_size_bytes,
                            };

                            state.db.insert_asset(&new_asset)?;
                        }

                if let Err(e) = state.db.reindex_download(dl_id) {
                    log::warn!("Failed to rebuild search index for restored download {}: {}", dl_id, e);
                }
            }

                Ok::<i64, String>(imported)
            })();
            match restore_result {
                Ok(count) => {
                    if let Err(error) = state.db.commit_atomic_restore() {
                        state.db.rollback_atomic_restore();
                        return Err(error);
                    }
                    promotion.commit();
                    Ok(count)
                }
                Err(error) => {
                    state.db.rollback_atomic_restore();
                    // Dropping the promotion guard restores overwritten files
                    // and removes newly promoted files in reverse order.
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

#[tauri::command]
pub async fn scan_and_reimport_downloads(app: tauri::AppHandle) -> Result<i64, String> {
    use crate::commands::downloader::{
        collect_assets_recursive, compute_content_details, extract_series_content_order,
        extract_series_relation,
    };

    let state = app.state::<Arc<AppState>>().inner().clone();
    let storage = state.db.storage_dir().to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut total_imported = 0i64;

        if !storage.exists() {
            return Ok(0);
        }

        // Iterate through sources (e.g. "pixiv", "fanbox")
        for source_entry in std::fs::read_dir(&storage).map_err(|e| e.to_string())? {
            let source_entry = source_entry.map_err(|e| e.to_string())?;
            let source_path = source_entry.path();
            if !source_path.is_dir() {
                continue;
            }
            let source = source_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if source != "pixiv" && source != "fanbox" {
                continue;
            }

            // Iterate through work IDs (e.g. "123456")
            for id_entry in std::fs::read_dir(&source_path).map_err(|e| e.to_string())? {
                let id_entry = id_entry.map_err(|e| e.to_string())?;
                let id_path = id_entry.path();
                if !id_path.is_dir() {
                    continue;
                }
                let source_id = id_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                // Find version directories "v1", "v2", ...
                let mut versions = Vec::new();
                for ver_entry in std::fs::read_dir(&id_path).map_err(|e| e.to_string())? {
                    let ver_entry = ver_entry.map_err(|e| e.to_string())?;
                    let ver_path = ver_entry.path();
                    if !ver_path.is_dir() {
                        continue;
                    }
                    let ver_name = ver_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if let Some(stripped) = ver_name.strip_prefix('v') {
                        if let Ok(v_num) = stripped.parse::<i64>() {
                            versions.push((v_num, ver_path));
                        }
                    }
                }

                // Sort versions ascendingly
                versions.sort_by_key(|v| v.0);

                if versions.is_empty() {
                    continue;
                }

                // If already exists, delete first to re-import cleanly
                if let Ok(Some(existing)) = state.db.get_download_by_source(&source, &source_id) {
                    let _ = state.db.delete_download(existing.id);
                }

                let mut dl_id = None;

                // Import each version
                for (v_num, ver_path) in versions {
                    let orig_json_path = ver_path.join("original.json");
                    let data_json_path = ver_path.join("data.json");
                    let json_file = if orig_json_path.exists() {
                        orig_json_path
                    } else if data_json_path.exists() {
                        data_json_path
                    } else {
                        continue;
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

                    if assets_dir.exists() {
                        let _ = collect_assets_recursive(
                            &assets_dir,
                            &mut asset_entries,
                            &mut total_size,
                            &mut cover_path_str,
                            0,
                        );
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

                    let registered_id = state.db.upsert_download(&new_dl)?;
                    dl_id = Some(registered_id);

                    // Re-upsert relations
                    let _ = state.db.upsert_download_relation(
                        registered_id,
                        "author",
                        &source,
                        &author_id,
                        &author_name,
                    );
                    let _ = state.db.upsert_download_person(
                        registered_id,
                        &source,
                        &author_id,
                        if source == "fanbox" {
                            "creator"
                        } else {
                            "author"
                        },
                        &author_name,
                    );

                    if source == "pixiv" {
                        if let Some((series_id, series_title)) = extract_series_relation(&data) {
                            let _ = state.db.upsert_download_relation(
                                registered_id,
                                "series",
                                &source,
                                &series_id,
                                &series_title,
                            );
                            let _ = state.db.upsert_download_series(
                                registered_id,
                                &source,
                                &series_id,
                                &series_title,
                                extract_series_content_order(&data),
                            );
                        }
                    }

                    // Insert assets
                    for mut asset in asset_entries {
                        asset.download_id = registered_id;
                        let _ = state.db.insert_asset(&asset);
                    }

                    // Insert version history record
                    let change_summary = format!("インポート復元 (v{})", v_num);
                    state.db.insert_version(&NewVersion {
                        download_id: registered_id,
                        version: v_num,
                        content_hash: Some(new_hash),
                        text_length: new_text_len,
                        json_path: json_file.to_string_lossy().to_string(),
                        original_json_path: Some(json_file.to_string_lossy().to_string()),
                        asset_count: new_dl.asset_count,
                        file_size_bytes: new_dl.file_size_bytes,
                        created_at: new_dl.downloaded_at.clone(),
                        change_summary: Some(change_summary),
                    })?;
                }

                if let Some(registered_id) = dl_id {
                    let _ = state.db.reindex_download(registered_id);
                    total_imported += 1;
                }
            }
        }

        // Reconstruct people and series tables from downloads & download_relations directly using fast SQLite queries!
        if total_imported > 0 {
            state.db.reconstruct_entities_after_import()?;
        }

        Ok::<i64, String>(total_imported)
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
        fs::remove_dir_all(base).unwrap();
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
        fs::remove_dir_all(base).unwrap();
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
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn import_rejects_zip_slip_before_creating_outside_directories() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = Arc::new(AppState { db });
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
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn import_rejects_traversal_in_metadata_before_database_mutation() {
        let base = create_temp_dir();
        let storage = base.join("downloads");
        let db = Database::open(&base.join("piep.db"), &storage).unwrap();
        let state = Arc::new(AppState { db });
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
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn test_export_and_import_integration() {
        // --- 1. テスト環境 (送信側) のセットアップ ---
        let base_dir = create_temp_dir();
        let db_path = base_dir.join("piep.db");
        let storage_dir = base_dir.join("downloads");

        let db = Database::open(&db_path, &storage_dir).unwrap();
        let state = Arc::new(AppState { db });

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

        // ZIPの書き出しパス
        let zip_path = base_dir.join("backup.zip");

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
        let state_new = Arc::new(AppState { db: db_new });

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

        // クリーンアップ
        let _ = fs::remove_dir_all(&base_dir);
        let _ = fs::remove_dir_all(&base_dir_new);
    }
}
