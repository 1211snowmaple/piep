use crate::database::{
    EntityVersion, NewAsset, NewDownload, NewVersion, SearchParams, SeriesEntry, UpdateTarget,
};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;

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
    tags: Option<String>,
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

fn backup_path_root(entry_name: &str) -> &'static str {
    if entry_name.starts_with("profiles/") || entry_name.starts_with("series/") {
        "app_data"
    } else {
        "storage"
    }
}

fn resolve_import_path(
    entry_name: &str,
    storage_dir: &Path,
    app_data_dir: &Path,
) -> std::path::PathBuf {
    if backup_path_root(entry_name) == "app_data" {
        app_data_dir.join(entry_name)
    } else {
        storage_dir.join(entry_name)
    }
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

fn all_backup_search_params() -> SearchParams {
    SearchParams {
        query: None,
        source: None,
        content_type: None,
        sort_by: None,
        sort_order: None,
        limit: Some(10000),
        offset: Some(0),
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
        search_mode: None,
    }
}

pub async fn export_all_zip_internal(state: Arc<AppState>, zip_path: String) -> Result<(), String> {
    export_zip_with_params_internal(state, zip_path, all_backup_search_params(), false).await
}

pub async fn export_zip_with_params_internal(
    state: Arc<AppState>,
    zip_path: String,
    params: SearchParams,
    scoped: bool,
) -> Result<(), String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage
        .parent()
        .unwrap_or_else(|| storage.as_path())
        .to_path_buf();

    let all = state.db.search_downloads(&params)?;

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

pub async fn import_zip_internal(state: Arc<AppState>, zip_path: String) -> Result<i64, String> {
    let storage = state.db.storage_dir().to_path_buf();
    let app_data = storage
        .parent()
        .unwrap_or_else(|| storage.as_path())
        .to_path_buf();

    // プレミアム最適化：ZIP解凍からDB挿入までの全同期処理を、非同期ワーカースレッドへ完璧に移譲
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

        let mut imported = 0i64;

        // ZIP内のファイルを展開
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.is_dir() {
                continue;
            }

            let entry_name = file.name();

            // 1. パスの単純検証 (Zip Slip 防御)
            if entry_name.contains("..") || entry_name.starts_with('/') || entry_name.starts_with('\\') {
                return Err(format!(
                    "Security Exception: Invalid zip entry path detected: {}",
                    entry_name
                ));
            }

            let outpath = resolve_import_path(entry_name, &storage, &app_data);

            // 2. 正規化された物理パスによる厳密検証 (Zip Slip 完全防御)
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

                // 親とアプリデータの実パスを取得して比較
                let canon_parent = std::fs::canonicalize(parent)
                    .map_err(|e| format!("Failed to resolve canonical parent path: {}", e))?;
                let canon_app_data = std::fs::canonicalize(&app_data)
                    .map_err(|e| format!("Failed to resolve canonical app data path: {}", e))?;

                if !canon_parent.starts_with(&canon_app_data) {
                    return Err(format!(
                        "Security Exception: Zip entry attempts to escape app data boundaries: {}",
                        entry_name
                    ));
                }
            }

            let mut outfile = std::fs::File::create(&outpath).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
        }

        // 展開されたフォルダからDBに登録
        let metadata_path = storage.join("backup_metadata.json");
        if metadata_path.exists() {
            let metadata_str = std::fs::read_to_string(&metadata_path).map_err(|e| e.to_string())?;
            let metadata: BackupMetadata = serde_json::from_str(&metadata_str).map_err(|e| e.to_string())?;

            for person in &metadata.people {
                let restored = crate::database::PersonEntry {
                    id: 0,
                    source: person.source.clone(),
                    source_key: person.source_key.clone(),
                    display_name: person.display_name.clone(),
                    icon_path: person
                        .relative_icon_path
                        .as_ref()
                        .map(|p| resolve_import_path(p, &storage, &app_data).to_string_lossy().to_string()),
                    cover_path: person
                        .relative_cover_path
                        .as_ref()
                        .map(|p| resolve_import_path(p, &storage, &app_data).to_string_lossy().to_string()),
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
                        json_path: resolve_import_path(
                            &version.relative_json_path,
                            &storage,
                            &app_data,
                        )
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
                        .as_ref()
                        .map(|p| resolve_import_path(p, &storage, &app_data).to_string_lossy().to_string()),
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
                        json_path: resolve_import_path(
                            &version.relative_json_path,
                            &storage,
                            &app_data,
                        )
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
                    let _ = state.db.delete_download(existing.id);
                }

                let latest_ver = entry
                    .versions
                    .iter()
                    .find(|v| v.version == entry.current_version)
                    .or_else(|| entry.versions.first());

                // 相対パスを現在の storage の絶対パスにマッピングし直す
                let final_original_json_path = if let Some(latest_ver) = latest_ver {
                    latest_ver.relative_original_json_path.as_ref()
                        .map(|p| storage.join(p).to_string_lossy().to_string())
                        .or_else(|| {
                            // If relative_original_json_path is None, try to point it to original.json in the same folder if it exists
                            let orig_p = storage.join(&latest_ver.relative_json_path).parent().unwrap().join("original.json");
                            if orig_p.exists() {
                                Some(orig_p.to_string_lossy().to_string())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                };

                let final_json_path = if let Some(ref orig) = final_original_json_path {
                    // Prefer using original.json for json_path as well!
                    orig.clone()
                } else if let Some(latest_ver) = latest_ver {
                    storage.join(&latest_ver.relative_json_path).to_string_lossy().to_string()
                } else {
                    storage.join(format!("{}/{}/v{}/original.json", entry.source, entry.source_id, entry.current_version)).to_string_lossy().to_string()
                };

                let final_cover_path = entry.relative_cover_path.as_ref()
                    .map(|p| storage.join(p).to_string_lossy().to_string());

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

                match state.db.upsert_download(&new_dl) {
                    Ok(dl_id) => {
                        imported += 1;

                        for relation in &entry.relations {
                            let _ = state.db.upsert_download_relation(
                                dl_id,
                                &relation.relation_type,
                                &relation.source,
                                &relation.relation_id,
                                &relation.relation_name,
                            );
                        }

                        for person in &entry.people {
                            let _ = state.db.upsert_download_person(
                                dl_id,
                                &person.person_source,
                                &person.person_key,
                                &person.role,
                                &person.display_name,
                            );
                        }

                        for series in &entry.series {
                            let _ = state.db.upsert_download_series(
                                dl_id,
                                &series.series_source,
                                &series.series_key,
                                &series.title,
                                series.content_order,
                            );
                        }

                        // バージョン履歴の復元
                        for v in &entry.versions {
                            let ver_orig = v.relative_original_json_path.as_ref()
                                .map(|p| storage.join(p).to_string_lossy().to_string())
                                .or_else(|| {
                                    let orig_p = storage.join(&v.relative_json_path).parent().unwrap().join("original.json");
                                    if orig_p.exists() {
                                        Some(orig_p.to_string_lossy().to_string())
                                    } else {
                                        None
                                    }
                                });

                            let ver_json = if let Some(ref orig) = ver_orig {
                                orig.clone()
                            } else {
                                storage.join(&v.relative_json_path).to_string_lossy().to_string()
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

                            let _ = state.db.insert_version(&new_ver);
                        }

                        // アセット情報の復元
                        for asset in &entry.assets {
                            let asset_local = storage.join(&asset.relative_local_path).to_string_lossy().to_string();

                            let new_asset = NewAsset {
                                download_id: dl_id,
                                asset_type: asset.asset_type.clone(),
                                filename: asset.filename.clone(),
                                local_path: asset_local,
                                original_url: asset.original_url.clone(),
                                mime_type: asset.mime_type.clone(),
                                file_size_bytes: asset.file_size_bytes,
                            };

                            let _ = state.db.insert_asset(&new_asset);
                        }

                        if let Err(e) = state.db.reindex_download(dl_id) {
                            log::warn!("Failed to rebuild search index for restored download {}: {}", dl_id, e);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to restore backup download {}/{}: {}", entry.source, entry.source_id, e);
                    }
                }
            }

            // バックアップメタデータファイルをクリーンアップ
            let _ = std::fs::remove_file(&metadata_path);

            Ok::<i64, String>(imported)
        } else {
            Err("Backup metadata file not found. Old backup formats are not supported in this version.".to_string())
        }
    })
    .await
    .map_err(|e| format!("Import thread panicked: {}", e))?
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

        fs::write(&v1_json_path, b"{\"title\":\"Ver 1 Title\"}").unwrap();
        fs::write(&v2_json_path, b"{\"title\":\"Ver 2 Title\"}").unwrap();
        fs::write(&v2_orig_json_path, b"{\"original_info\":\"something\"}").unwrap();

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
            tags: Some("[\"Tag1\", \"Tag2\"]".to_string()),
            excerpt: Some("あらすじ".to_string()),
            cover_path: None,
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
