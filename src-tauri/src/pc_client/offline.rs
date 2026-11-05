//! 利用者が選んだ作品だけを、検証後に版単位で切り替えて保存する。

use super::*;
use super::image_validation::validate_saved_image;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_PAGES: usize = 10_000;
const MAX_ASSETS: usize = 10_000;
const MAX_SNAPSHOT_BYTES: usize = 500 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_COLLECTION_COVERS: usize = 64;
const MAX_COLLECTION_COVER_BYTES: usize = 16 * 1024 * 1024;

pub struct OfflineState {
    root: PathBuf,
    save_lock: Mutex<()>,
    lock: Mutex<()>,
}

impl OfflineState {
    pub fn new(root: PathBuf) -> Self {
        let state = Self {
            root,
            save_lock: Mutex::new(()),
            lock: Mutex::new(()),
        };
        // State 公開前なので保存・読書との競合はない。強制終了で残った作業領域を回収する。
        state.recover_storage();
        state
    }

    fn recover_storage(&self) {
        let Ok(root) = fs::canonicalize(&self.root) else { return; };
        let Ok(works) = fs::read_dir(&root) else { return; };
        for work in works.flatten() {
            let name = work.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_prefix("work-")).and_then(|value| value.parse::<i64>().ok()) else { continue; };
            if id <= 0 || name.to_str() != Some(format!("work-{id}").as_str())
                || !work.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()) { continue; }
            let Ok(work_dir) = fs::canonicalize(work.path()) else { continue; };
            // Junction などによる、別の保存領域への移動も許可しない。
            if work_dir != root.join(&name) { continue; }
            let current = read_manifest(self, id).ok().and_then(|(_, path)| path.file_name().map(|name| name.to_owned()));
            let Ok(entries) = fs::read_dir(&work_dir) else { continue; };
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()) { continue; }
                let entry_name = entry.file_name();
                let Some(name) = entry_name.to_str() else { continue; };
                let abandoned_staging = name.strip_prefix("staging-").is_some_and(|id| Uuid::parse_str(id).is_ok());
                let old_snapshot = current.as_ref().is_some_and(|current| current != &entry_name)
                    && name.strip_prefix("snapshot-").is_some_and(|id| Uuid::parse_str(id).is_ok());
                if !abandoned_staging && !old_snapshot { continue; }
                let Ok(target) = fs::canonicalize(entry.path()) else { continue; };
                // 再帰削除の直前に最終パスがこの作品内の実ディレクトリであることを確認する。
                if target != work_dir.join(&entry_name) || !target.starts_with(&root) { continue; }
                let _ = fs::remove_dir_all(target);
            }
        }
    }

    fn work_dir(&self, work_id: i64) -> Result<PathBuf, ClientError> {
        if work_id <= 0 {
            return Err(ClientError::remote());
        }
        Ok(self.root.join(format!("work-{work_id}")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineWork {
    pub work: MobileWork,
    #[serde(default)]
    pub people: Vec<MobilePerson>,
    #[serde(default)]
    pub series: Vec<MobileSeriesMembership>,
    pub saved_at_ms: u64,
    pub saved_version: i64,
    pub saved_content_hash: Option<String>,
    pub saved_edit_hash: Option<String>,
    pub page_count: usize,
    #[serde(default)]
    pub collections: Vec<MobileCollection>,
    #[serde(default)]
    pub collection_positions: BTreeMap<String, i64>,
    #[serde(default)]
    pub collection_cover_ids: Vec<String>,
    #[serde(default)]
    pub has_author_cover: bool,
    #[serde(default)]
    pub has_series_cover: bool,
    #[serde(default)]
    pub author_cover_entity: Option<OfflineEntityKey>,
    #[serde(default)]
    pub series_cover_entity: Option<OfflineEntityKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineEntityKey {
    pub source: String,
    pub key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentPointer {
    snapshot: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflinePage {
    generation: String,
    #[serde(flatten)]
    page: MobilePage,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageRecord {
    page: usize,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetRecord {
    id: i64,
    mime_type: String,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoverRecord {
    mime_type: String,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionCoverRecord {
    collection_id: String,
    file_number: usize,
    mime_type: String,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    summary: OfflineWork,
    pages: Vec<PageRecord>,
    assets: Vec<AssetRecord>,
    #[serde(default)]
    cover: Option<CoverRecord>,
    #[serde(default)]
    author_cover: Option<CoverRecord>,
    #[serde(default)]
    series_cover: Option<CoverRecord>,
    #[serde(default)]
    collection_covers: Vec<CollectionCoverRecord>,
    #[serde(default)]
    outline_sha256: Option<String>,
}

fn storage() -> ClientError {
    ClientError::offline_storage()
}

fn snapshot_changed() -> ClientError {
    ClientError {
        code: "offline_snapshot_changed",
        message: "保存版が更新されました。作品を読み直してください",
    }
}

fn snapshot_generation(snapshot: &Path) -> Result<String, ClientError> {
    let name = snapshot.file_name().and_then(|name| name.to_str()).ok_or_else(storage)?;
    let id = name.strip_prefix("snapshot-").ok_or_else(storage)?;
    Uuid::parse_str(id).map(|uuid| uuid.to_string()).map_err(|_| storage())
}

fn require_generation(snapshot: &Path, generation: &str) -> Result<(), ClientError> {
    let requested = Uuid::parse_str(generation).map_err(|_| snapshot_changed())?;
    let current = snapshot_generation(snapshot).map_err(|_| snapshot_changed())?;
    if requested.to_string() == current { Ok(()) } else { Err(snapshot_changed()) }
}

fn cleanup_superseded_snapshots(work_dir: &Path, current_snapshot: &str) {
    let Ok(canonical_work_dir) = fs::canonicalize(work_dir) else { return; };
    if let Ok(entries) = fs::read_dir(work_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name != current_snapshot
                && name.strip_prefix("snapshot-").is_some_and(|id| Uuid::parse_str(id).is_ok())
                && entry.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                let Ok(target) = fs::canonicalize(entry.path()) else { continue; };
                if target != canonical_work_dir.join(entry.file_name())
                    || target.parent() != Some(canonical_work_dir.as_path())
                {
                    continue;
                }
                let _ = fs::remove_dir_all(target);
            }
        }
    }
}

fn read_offline_page(state: &OfflineState, work_id: i64, page: usize) -> Result<OfflinePage, ClientError> {
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let generation = snapshot_generation(&snapshot)?;
    let page = page.min(manifest.summary.page_count - 1);
    let record = manifest.pages.get(page).filter(|record| record.page == page).ok_or_else(ClientError::remote)?;
    let bytes = fs::read(snapshot.join("pages").join(format!("{page}.json"))).map_err(|_| storage())?;
    if bytes.len() > MAX_PAGE_BYTES || digest(&bytes) != record.sha256 { return Err(storage()); }
    let page_data: MobilePage = serde_json::from_slice(&bytes).map_err(|_| storage())?;
    if page_data.page != page || page_data.page_count != manifest.summary.page_count { return Err(storage()); }
    Ok(OfflinePage { generation, page: page_data })
}

fn read_offline_asset(state: &OfflineState, work_id: i64, asset_id: i64, generation: &str) -> Result<MobileAssetData, ClientError> {
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    require_generation(&snapshot, generation)?;
    let record = manifest.assets.iter().find(|record| record.id == asset_id).ok_or_else(ClientError::remote)?;
    let bytes = fs::read(snapshot.join("assets").join(format!("{asset_id}.bin"))).map_err(|_| storage())?;
    if bytes.len() > MAX_ASSET_BYTES || digest(&bytes) != record.sha256 { return Err(storage()); }
    Ok(MobileAssetData { mime_type: record.mime_type.clone(), base64: STANDARD.encode(bytes) })
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<Vec<u8>, ClientError> {
    let bytes = serde_json::to_vec(value).map_err(|_| storage())?;
    fs::write(path, &bytes).map_err(|_| storage())?;
    Ok(bytes)
}

fn read_manifest(state: &OfflineState, work_id: i64) -> Result<(Manifest, PathBuf), ClientError> {
    let work_dir = state.work_dir(work_id)?;
    let pointer: CurrentPointer =
        serde_json::from_slice(&fs::read(work_dir.join("current.json")).map_err(|_| storage())?)
            .map_err(|_| storage())?;
    let snapshot_id = pointer
        .snapshot
        .strip_prefix("snapshot-")
        .ok_or_else(storage)?;
    Uuid::parse_str(snapshot_id).map_err(|_| storage())?;
    let snapshot = work_dir.join(pointer.snapshot);
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(snapshot.join("manifest.json")).map_err(|_| storage())?)
            .map_err(|_| storage())?;
    if manifest.summary.work.id != work_id
        || manifest.pages.len() != manifest.summary.page_count
        || manifest.summary.page_count == 0
        || manifest.summary.page_count > MAX_PAGES
    {
        return Err(storage());
    }
    Ok((manifest, snapshot))
}

fn asset_ids(html: &str) -> BTreeSet<i64> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r#"data-asset-id\s*=\s*["']([0-9]+)["']"#).expect("valid asset pattern")
    });
    pattern
        .captures_iter(html)
        .filter_map(|capture| capture[1].parse::<i64>().ok())
        .filter(|id| *id > 0)
        .collect()
}

async fn get_pc(
    client: &reqwest::Client,
    saved: &SavedConnection,
    endpoint: &str,
) -> Result<reqwest::Response, ClientError> {
    get_pc_with_timeout(client, saved, endpoint, REQUEST_TIMEOUT).await
}

async fn get_pc_with_timeout(
    client: &reqwest::Client,
    saved: &SavedConnection,
    endpoint: &str,
    timeout: Duration,
) -> Result<reqwest::Response, ClientError> {
    let response = client
        .get(format!("{}{}", saved.url, endpoint))
        .bearer_auth(&saved.token)
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    check_response(response)
}

async fn validated_image_response(response: reqwest::Response) -> Result<MobileAssetData, ClientError> {
    let image = image_response(response).await?;
    let bytes = STANDARD.decode(&image.base64).map_err(|_| ClientError::protocol())?;
    validate_saved_image(&bytes, &image.mime_type)?;
    Ok(image)
}

async fn save_entity_cover(
    client: &reqwest::Client,
    saved: &SavedConnection,
    source: &str,
    kind: &str,
    key: &str,
    staging: &Path,
    total_bytes: &mut usize,
) -> Result<Option<CoverRecord>, ClientError> {
    if key.is_empty() {
        return Ok(None);
    }
    let mut url = reqwest::Url::parse(&format!("{}/api/v1/entities/{kind}/cover", saved.url))
        .map_err(|_| ClientError::protocol())?;
    url.query_pairs_mut()
        .append_pair("source", source)
        .append_pair("key", key);
    let response = match client
        .get(url)
        .bearer_auth(&saved.token)
        .timeout(ASSET_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => match check_response(response) {
            Ok(response) => response,
            Err(_) => return Ok(None),
        },
        Err(_) => return Ok(None),
    };
    let image = match validated_image_response(response).await {
        Ok(image) => image,
        Err(_) => return Ok(None),
    };
    let bytes = STANDARD
        .decode(&image.base64)
        .map_err(|_| ClientError::protocol())?;
    if total_bytes.saturating_add(bytes.len()) > MAX_SNAPSHOT_BYTES {
        return Ok(None);
    }
    fs::write(staging.join(format!("{kind}-cover.bin")), &bytes).map_err(|_| storage())?;
    *total_bytes += bytes.len();
    Ok(Some(CoverRecord {
        mime_type: image.mime_type,
        sha256: digest(&bytes),
    }))
}

async fn save_collection_covers(
    client: &reqwest::Client,
    saved: &SavedConnection,
    collections: &[MobileCollection],
    staging: &Path,
    total_bytes: &mut usize,
) -> Result<Vec<CollectionCoverRecord>, ClientError> {
    let mut records = Vec::new();
    let mut cover_bytes = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    for collection in collections.iter().filter(|value| value.cover_mode == "file" && value.has_file_cover).take(MAX_COLLECTION_COVERS) {
        if cover_bytes >= MAX_COLLECTION_COVER_BYTES || tokio::time::Instant::now() >= deadline { break; }
        if collection.id.is_empty() || collection.id.len() > 128
            || !collection.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) { continue; }
        let endpoint = format!("/api/v1/collections/{}/cover/file", collection.id);
        let response = match tokio::time::timeout_at(deadline, get_pc_with_timeout(client, saved, &endpoint, ASSET_TIMEOUT)).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => continue,
            Err(_) => break,
        };
        let image = match tokio::time::timeout_at(deadline, validated_image_response(response)).await {
            Ok(Ok(image)) => image,
            Ok(Err(_)) => continue,
            Err(_) => break,
        };
        let bytes = match STANDARD.decode(&image.base64) { Ok(bytes) => bytes, Err(_) => continue };
        if cover_bytes.saturating_add(bytes.len()) > MAX_COLLECTION_COVER_BYTES
            || total_bytes.saturating_add(bytes.len()) > MAX_SNAPSHOT_BYTES { continue; }
        let file_number = records.len();
        fs::write(staging.join(format!("collection-cover-{file_number}.bin")), &bytes).map_err(|_| storage())?;
        cover_bytes += bytes.len();
        *total_bytes += bytes.len();
        records.push(CollectionCoverRecord {
            collection_id: collection.id.clone(),
            file_number,
            mime_type: image.mime_type,
            sha256: digest(&bytes),
        });
    }
    Ok(records)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineIssue {
    work_id: i64,
    issue: &'static str,
}

fn scan_works(state: &OfflineState) -> Result<(Vec<OfflineWork>, Vec<OfflineIssue>), ClientError> {
    let entries = match fs::read_dir(&state.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(_) => return Err(storage()),
    };
    let mut result = Vec::new();
    let mut issues = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| storage())?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("work-"))
            .and_then(|value| value.parse::<i64>().ok())
        else {
            continue;
        };
        if id <= 0 || name.to_str() != Some(format!("work-{id}").as_str())
            || !entry.file_type().map_err(|_| storage())?.is_dir() {
            continue;
        }
        match read_manifest(&state, id) {
            Ok((manifest, _)) => result.push(manifest.summary),
            Err(_) => {
                // 新規保存の通信中は staging だけなので問題として表示しない。
                let has_snapshot = match fs::read_dir(entry.path()) {
                    Ok(mut entries) => entries.any(|item| match item {
                        Ok(item) => item.file_type().is_ok_and(|kind| kind.is_dir())
                            && item.file_name().to_str().and_then(|name| name.strip_prefix("snapshot-")).is_some_and(|value| Uuid::parse_str(value).is_ok()),
                        // この作品の中だけを読めない場合、他作品の一覧を妨げない。
                        Err(_) => true,
                    }),
                    Err(_) => true,
                };
                if entry.path().join("current.json").exists() || has_snapshot {
                    issues.push(OfflineIssue { work_id: id, issue: "offline_storage" });
                }
            }
        }
    }
    result.sort_by(|left, right| right.saved_at_ms.cmp(&left.saved_at_ms));
    issues.sort_by_key(|issue| issue.work_id);
    Ok((result, issues))
}

#[tauri::command]
pub async fn offline_list_works(state: State<'_, OfflineState>) -> Result<Vec<OfflineWork>, ClientError> {
    let _guard = state.lock.lock().await;
    Ok(scan_works(&state)?.0)
}

#[tauri::command]
pub async fn offline_list_issues(state: State<'_, OfflineState>) -> Result<Vec<OfflineIssue>, ClientError> {
    let _guard = state.lock.lock().await;
    Ok(scan_works(&state)?.1)
}

#[tauri::command]
pub async fn offline_save_work(
    pc: State<'_, PcClientState>,
    state: State<'_, OfflineState>,
    work_id: i64,
) -> Result<OfflineWork, ClientError> {
    // 通信と検証の間も、既存のスナップショットを読めるようにする。
    let _save_guard = state.save_lock.lock().await;
    state.work_dir(work_id)?;
    let saved = pc.load()?.ok_or_else(ClientError::invalid_pairing)?;
    let client = pc.client(&saved)?;
    let metadata: MobileMetadata = get_pc(&client, &saved, &format!("/api/v1/works/{work_id}"))
        .await?
        .bounded_json().await?;
    if metadata.work.id != work_id {
        return Err(ClientError::protocol());
    }
    // DB の数値 ID はサーバーごとに異なる。別のサーバーへ登録し直しても、同じ ID の
    // 別作品で持ち歩き中の作品を置き換えない。
    if let Ok((existing, _)) = read_manifest(&state, work_id) {
        if existing.summary.work.source != metadata.work.source
            || existing.summary.work.source_id != metadata.work.source_id
        {
            return Err(ClientError {
                code: "offline_work_conflict",
                message: "別のサーバーから保存した作品と番号が重なっています。保存済み作品を確認してください",
            });
        }
    }
    save_snapshot(&state, &client, &saved, &metadata).await
}

async fn save_snapshot(
    state: &OfflineState,
    client: &reqwest::Client,
    saved: &SavedConnection,
    metadata: &MobileMetadata,
) -> Result<OfflineWork, ClientError> {
    let work_dir = state.work_dir(metadata.work.id)?;
    fs::create_dir_all(&work_dir).map_err(|_| storage())?;
    let snapshot_name = format!("snapshot-{}", Uuid::new_v4());
    let staging = work_dir.join(format!("staging-{}", Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|_| storage())?;
    let result = save_into(client, saved, metadata, &staging).await;
    let manifest = match result {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    // 公開中の版の切替だけ、読み込みと排他にする。
    let _guard = state.lock.lock().await;
    let pointer = publish_snapshot(&work_dir, &staging, snapshot_name)?;
    // ページ/画像取得も同じロックを使うため、旧版を参照中の読み込みが終わってから削除する。
    cleanup_superseded_snapshots(&work_dir, &pointer.snapshot);
    Ok(manifest.summary)
}

fn publish_snapshot(
    work_dir: &Path,
    staging: &Path,
    snapshot_name: String,
) -> Result<CurrentPointer, ClientError> {
    let snapshot = work_dir.join(&snapshot_name);
    if fs::rename(&staging, &snapshot).is_err() {
        let _ = fs::remove_dir_all(&staging);
        return Err(storage());
    }
    let pointer = CurrentPointer {
        snapshot: snapshot_name,
    };
    let temporary_pointer = work_dir.join("current.json.tmp");
    if write_json(&temporary_pointer, &pointer).is_err()
        || fs::rename(&temporary_pointer, work_dir.join("current.json")).is_err()
    {
        let _ = fs::remove_file(&temporary_pointer);
        let _ = fs::remove_dir_all(&snapshot);
        return Err(storage());
    }
    Ok(pointer)
}

async fn save_into(
    client: &reqwest::Client,
    saved: &SavedConnection,
    metadata: &MobileMetadata,
    staging: &Path,
) -> Result<Manifest, ClientError> {
    fs::create_dir(staging.join("pages")).map_err(|_| storage())?;
    fs::create_dir(staging.join("assets")).map_err(|_| storage())?;
    let work_id = metadata.work.id;
    let first: MobilePage = get_pc(client, saved, &format!("/api/v1/works/{work_id}/pages/0"))
        .await?
        .bounded_json().await?;
    if first.page != 0 || first.page_count == 0 || first.page_count > MAX_PAGES {
        return Err(ClientError::protocol());
    }
    let mut pages = Vec::with_capacity(first.page_count);
    let mut assets = BTreeSet::new();
    let mut total_bytes = 0_usize;
    for index in 0..first.page_count {
        let page: MobilePage = if index == 0 {
            first.clone()
        } else {
            get_pc(
                client,
                saved,
                &format!("/api/v1/works/{work_id}/pages/{index}"),
            )
            .await?
            .bounded_json().await?
        };
        if page.page != index || page.page_count != first.page_count {
            return Err(ClientError::protocol());
        }
        assets.extend(asset_ids(&page.html));
        if assets.len() > MAX_ASSETS {
            return Err(ClientError::remote());
        }
        let bytes = serde_json::to_vec(&page).map_err(|_| storage())?;
        if bytes.len() > MAX_PAGE_BYTES {
            return Err(ClientError::remote());
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(ClientError::remote)?;
        if total_bytes > MAX_SNAPSHOT_BYTES {
            return Err(ClientError::remote());
        }
        fs::write(staging.join("pages").join(format!("{index}.json")), &bytes)
            .map_err(|_| storage())?;
        pages.push(PageRecord {
            page: index,
            sha256: digest(&bytes),
        });
    }
    let mut asset_records = Vec::with_capacity(assets.len());
    for id in assets {
        let response = get_pc_with_timeout(
            client,
            saved,
            &format!("/api/v1/works/{work_id}/assets/{id}"),
            ASSET_TIMEOUT,
        )
        .await?;
        let mime_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(ClientError::protocol)?
            .to_string();
        if !matches!(
            mime_type.as_str(),
            "image/jpeg" | "image/png" | "image/webp" | "image/gif" | "image/avif"
        ) {
            return Err(ClientError::protocol());
        }
        let bytes = bounded_response_bytes(response, MAX_ASSET_BYTES).await?;
        validate_saved_image(&bytes, &mime_type)?;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(ClientError::remote)?;
        if total_bytes > MAX_SNAPSHOT_BYTES {
            return Err(ClientError::remote());
        }
        fs::write(staging.join("assets").join(format!("{id}.bin")), &bytes)
            .map_err(|_| storage())?;
        asset_records.push(AssetRecord {
            id,
            mime_type,
            sha256: digest(&bytes),
        });
    }
    // 表紙は本文の画像一覧とは別に保管する。取得できなくても本文の保存は続ける。
    let mut cover = None;
    if metadata.work.has_cover {
        if let Ok(response) = get_pc_with_timeout(
            client,
            saved,
            &format!("/api/v1/works/{work_id}/cover"),
            ASSET_TIMEOUT,
        )
        .await
        {
            if let Ok(image) = validated_image_response(response).await {
                if let Ok(bytes) = STANDARD.decode(&image.base64) {
                    if total_bytes.saturating_add(bytes.len()) <= MAX_SNAPSHOT_BYTES {
                        fs::write(staging.join("cover.bin"), &bytes).map_err(|_| storage())?;
                        total_bytes += bytes.len();
                        cover = Some(CoverRecord {
                            mime_type: image.mime_type,
                            sha256: digest(&bytes),
                        });
                    }
                }
            }
        }
    }
    let author_cover = save_entity_cover(
        client,
        saved,
        &metadata.work.source,
        "people",
        &metadata.work.author_id,
        staging,
        &mut total_bytes,
    )
    .await?;
    let series_cover = if let Some(series_id) = metadata.work.series_id.as_deref() {
        save_entity_cover(
            client,
            saved,
            &metadata.work.source,
            "series",
            series_id,
            staging,
            &mut total_bytes,
        )
        .await?
    } else {
        None
    };
    let mut outline_sha256 = None;
    if let Ok(response) = get_pc(client, saved, &format!("/api/v1/works/{work_id}/outline")).await {
        if let Ok(entries) = response.bounded_json::<Vec<MobileOutlineEntry>>().await {
            if entries.len() <= 2_000 {
                let bytes = serde_json::to_vec(&entries).map_err(|_| storage())?;
                if bytes.len() <= 1_000_000 {
                    fs::write(staging.join("outline.json"), &bytes).map_err(|_| storage())?;
                    outline_sha256 = Some(digest(&bytes));
                }
            }
        }
    }
    let collections: Vec<MobileCollection> = get_pc(
        client,
        saved,
        &format!("/api/v1/works/{work_id}/collections"),
    )
    .await?
    .bounded_json().await?;
    let collection_positions = collections.iter()
        .filter_map(|collection| collection.position.map(|position| (collection.id.clone(), position)))
        .collect::<BTreeMap<_, _>>();
    let collection_covers = save_collection_covers(client, saved, &collections, staging, &mut total_bytes).await?;
    let collection_cover_ids = collection_covers.iter().map(|record| record.collection_id.clone()).collect();
    // 読み込み中にサーバー側が更新された場合、混在した版を保存しない。
    let latest: MobileMetadata = get_pc(client, saved, &format!("/api/v1/works/{work_id}"))
        .await?
        .bounded_json().await?;
    if latest.work.current_version != metadata.work.current_version
        || latest.work.content_hash != metadata.work.content_hash
        || latest.active_edit_hash != metadata.active_edit_hash
    {
        return Err(ClientError::remote());
    }
    let summary = OfflineWork {
        work: metadata.work.clone(),
        people: metadata.people.clone(),
        series: metadata.series.clone(),
        saved_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| storage())?
            .as_millis() as u64,
        saved_version: metadata.work.current_version,
        saved_content_hash: metadata.work.content_hash.clone(),
        saved_edit_hash: metadata.active_edit_hash.clone(),
        page_count: pages.len(),
        collections,
        collection_positions,
        collection_cover_ids,
        has_author_cover: author_cover.is_some(),
        has_series_cover: series_cover.is_some(),
        author_cover_entity: author_cover.as_ref().map(|_| OfflineEntityKey {
            source: metadata.work.source.clone(),
            key: metadata.work.author_id.clone(),
        }),
        series_cover_entity: series_cover.as_ref().and_then(|_| metadata.work.series_id.as_ref().map(|key| OfflineEntityKey {
            source: metadata.work.source.clone(),
            key: key.clone(),
        })),
    };
    let manifest = Manifest {
        summary,
        pages,
        assets: asset_records,
        cover,
        author_cover,
        series_cover,
        collection_covers,
        outline_sha256,
    };
    write_json(&staging.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

#[tauri::command]
pub async fn offline_get_outline(
    state: State<'_, OfflineState>,
    work_id: i64,
    generation: String,
) -> Result<Vec<MobileOutlineEntry>, ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(&state, work_id)?;
    require_generation(&snapshot, &generation)?;
    let Some(expected) = manifest.outline_sha256 else {
        return Ok(Vec::new());
    };
    let bytes = fs::read(snapshot.join("outline.json")).map_err(|_| storage())?;
    if bytes.len() > 1_000_000 || digest(&bytes) != expected {
        return Err(storage());
    }
    serde_json::from_slice(&bytes).map_err(|_| storage())
}

fn verified_cover(path: PathBuf, sha256: &str, mime_type: &str) -> Result<(String, Vec<u8>), ClientError> {
    let bytes = fs::read(path).map_err(|_| storage())?;
    if bytes.len() > MAX_ASSET_BYTES || digest(&bytes) != sha256 {
        return Err(storage());
    }
    Ok((mime_type.to_string(), bytes))
}

pub async fn saved_cover(state: &OfflineState, work_id: i64) -> Result<(String, Vec<u8>), ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let record = manifest.cover.ok_or_else(ClientError::remote)?;
    verified_cover(snapshot.join("cover.bin"), &record.sha256, &record.mime_type)
}

pub async fn saved_collection_cover(
    state: &OfflineState,
    work_id: i64,
    collection_id: &str,
) -> Result<(String, Vec<u8>), ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let record = manifest.collection_covers.iter()
        .find(|record| record.collection_id == collection_id)
        .ok_or_else(ClientError::remote)?;
    verified_cover(snapshot.join(format!("collection-cover-{}.bin", record.file_number)), &record.sha256, &record.mime_type)
}

pub async fn saved_entity_cover(
    state: &OfflineState,
    work_id: i64,
    kind: &str,
) -> Result<(String, Vec<u8>), ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let (record, filename) = match kind {
        "people" => (manifest.author_cover.as_ref(), "people-cover.bin"),
        "series" => (manifest.series_cover.as_ref(), "series-cover.bin"),
        _ => return Err(ClientError::remote()),
    };
    let record = record.ok_or_else(ClientError::remote)?;
    verified_cover(snapshot.join(filename), &record.sha256, &record.mime_type)
}

#[tauri::command]
pub async fn offline_get_page(
    state: State<'_, OfflineState>,
    work_id: i64,
    page: usize,
) -> Result<OfflinePage, ClientError> {
    let _guard = state.lock.lock().await;
    read_offline_page(&state, work_id, page)
}

#[tauri::command]
pub async fn offline_get_asset(
    state: State<'_, OfflineState>,
    work_id: i64,
    asset_id: i64,
    generation: String,
) -> Result<MobileAssetData, ClientError> {
    let _guard = state.lock.lock().await;
    read_offline_asset(&state, work_id, asset_id, &generation)
}

#[tauri::command]
pub async fn offline_delete_work(
    state: State<'_, OfflineState>,
    work_id: i64,
    source: String,
    source_id: String,
) -> Result<(), ClientError> {
    let _save_guard = state.save_lock.lock().await;
    let _guard = state.lock.lock().await;
    delete_saved_work(&state, work_id, &source, &source_id)
}

/// 数値 ID はサーバーごとに振られる。別のサーバーの同じ番号の作品を消さないよう、
/// 消す直前に保存版の出どころを確かめる。読めない保存版は `offline_delete_corrupt_work` で消す。
fn delete_saved_work(state: &OfflineState, work_id: i64, source: &str, source_id: &str) -> Result<(), ClientError> {
    let dir = state.work_dir(work_id)?;
    if !dir.exists() {
        return Ok(());
    }
    let (existing, _) = read_manifest(state, work_id)?;
    if existing.summary.work.source != source || existing.summary.work.source_id != source_id {
        return Err(ClientError {
            code: "offline_work_conflict",
            message: "保存済みの作品とは別の作品です。端末内の保存は消していません。",
        });
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(storage()),
    }
}

fn delete_corrupt_work(state: &OfflineState, work_id: i64) -> Result<(), ClientError> {
    let dir = state.work_dir(work_id)?;
    if read_manifest(state, work_id).is_ok() {
        return Err(ClientError {
            code: "offline_work_recovered",
            message: "作品は保存し直されています。一覧を更新してください",
        });
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(storage()),
    }
}

#[tauri::command]
pub async fn offline_delete_corrupt_work(state: State<'_, OfflineState>, work_id: i64) -> Result<(), ClientError> {
    let _save_guard = state.save_lock.lock().await;
    let _guard = state.lock.lock().await;
    delete_corrupt_work(&state, work_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_required_http_image_never_replaces_the_saved_snapshot() {
        use std::io::{Cursor, Read, Write};
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3, 4).write_to(&mut encoded, image::ImageFormat::Png).unwrap();
        let png = encoded.into_inner();
        tauri::async_runtime::block_on(async {
            for (mime, bytes, expected) in [
                ("image/png", Vec::new(), "invalid_offline_image"),
                ("image/png", png[..png.len() / 2].to_vec(), "invalid_offline_image"),
                ("image/jpeg", png.clone(), "invalid_offline_image"),
                ("image/avif", png.clone(), "unsupported_offline_image"),
            ] {
                let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
                let state = OfflineState::new(root.clone());
                let metadata: MobileMetadata = serde_json::from_value(serde_json::json!({
                    "work": {"id": 3, "source": "pixiv", "sourceId": "3", "title": "作品",
                        "authorName": "作者", "contentType": "novel", "tags": [], "excerpt": null,
                        "textLength": 1, "assetCount": 1, "hasCover": false, "currentVersion": 1,
                        "contentHash": null, "favorite": false, "seriesTitle": null, "seriesId": null},
                    "versions": [], "isEdited": false, "activeEditId": null, "activeEditHash": null
                })).unwrap();
                let work_dir = state.work_dir(3).unwrap();
                let old_name = format!("snapshot-{}", Uuid::new_v4());
                let old = work_dir.join(&old_name);
                fs::create_dir_all(old.join("assets")).unwrap();
                fs::write(old.join("assets/42.bin"), &png).unwrap();
                write_json(&old.join("manifest.json"), &serde_json::json!({
                    "summary": {"work": metadata.work, "savedAtMs": 1, "savedVersion": 1,
                        "savedContentHash": null, "savedEditHash": null, "pageCount": 1},
                    "pages": [{"page": 0, "sha256": "test"}], "assets": []
                })).unwrap();
                write_json(&work_dir.join("current.json"), &CurrentPointer { snapshot: old_name.clone() }).unwrap();
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let server = std::thread::spawn(move || {
                    for _ in 0..2 {
                        let (mut stream, _) = listener.accept().unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        let mut request = [0; 4096];
                        let length = stream.read(&mut request).unwrap();
                        let request = String::from_utf8_lossy(&request[..length]);
                        let (response_mime, body) = if request.starts_with("GET /api/v1/works/3/pages/0 ") {
                            ("application/json", serde_json::to_vec(&serde_json::json!({
                                "page": 0, "pageCount": 1, "html": "<img data-asset-id=\"42\">",
                                "totalPlainTextChars": 0, "sourcePageStarts": [0]
                            })).unwrap())
                        } else {
                            assert!(request.starts_with("GET /api/v1/works/3/assets/42 "));
                            (mime, bytes.clone())
                        };
                        write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: {response_mime}\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
                        stream.write_all(&body).unwrap();
                    }
                });
                let saved = SavedConnection { url: format!("http://{address}"), fingerprint: None,
                    certificate_der: None, token: "test-token".into(), device_id: "test".into(), device_name: "test".into(), capabilities: Vec::new() };
                let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();
                let error = save_snapshot(&state, &client, &saved, &metadata).await.unwrap_err();
                assert_eq!(error.code, expected);
                server.join().unwrap();
                let (_, current) = read_manifest(&state, 3).unwrap();
                assert_eq!(current, old);
                assert_eq!(fs::read(current.join("assets/42.bin")).unwrap(), png);
                assert_eq!(fs::read_dir(&work_dir).unwrap().count(), 2, "failed staging must be removed");
                fs::remove_dir_all(root).unwrap();
            }
        });
    }

    #[test]
    fn restart_reclaims_interrupted_saves_but_preserves_current_and_unrecoverable_snapshots() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let work_dir = root.join("work-3");
        let current_name = format!("snapshot-{}", Uuid::new_v4());
        let current = work_dir.join(&current_name);
        let old = work_dir.join(format!("snapshot-{}", Uuid::new_v4()));
        let staging = root.join("work-4").join(format!("staging-{}", Uuid::new_v4()));
        let damaged = root.join("work-5");
        let retained = damaged.join(format!("snapshot-{}", Uuid::new_v4()));
        for path in [&current, &old, &staging, &retained] { fs::create_dir_all(path).unwrap(); }
        fs::write(staging.join("download.bin"), b"interrupted download").unwrap();
        fs::write(old.join("old.bin"), b"superseded snapshot").unwrap();
        fs::write(retained.join("body.bin"), b"possibly recoverable").unwrap();
        write_json(&current.join("manifest.json"), &serde_json::json!({
            "summary": {
                "work": {"id": 3, "source": "pixiv", "sourceId": "3", "title": "保存作品",
                    "authorName": "作者", "contentType": "novel", "tags": [], "excerpt": null,
                    "textLength": 1, "assetCount": 0, "hasCover": false, "currentVersion": 1,
                    "contentHash": null, "favorite": false, "seriesTitle": null, "seriesId": null},
                "savedAtMs": 1, "savedVersion": 1, "savedContentHash": null,
                "savedEditHash": null, "pageCount": 1
            }, "pages": [{"page": 0, "sha256": "test"}], "assets": []
        })).unwrap();
        write_json(&work_dir.join("current.json"), &CurrentPointer { snapshot: current_name }).unwrap();
        fs::write(damaged.join("current.json"), b"broken pointer").unwrap();
        let unknown = work_dir.join("staging-unknown");
        fs::create_dir(&unknown).unwrap();

        let state = OfflineState::new(root.clone());
        assert!(!staging.exists(), "interrupted downloads should release storage on restart");
        assert!(!old.exists(), "crash after publication should not retain the superseded snapshot");
        assert!(read_manifest(&state, 3).is_ok(), "the active snapshot must remain readable");
        assert!(retained.join("body.bin").exists(), "a broken current pointer must not cause data loss");
        assert!(unknown.exists(), "only known generated names may be removed");
        let (_, issues) = scan_works(&state).unwrap();
        assert!(issues.iter().any(|issue| issue.work_id == 5));
        // 別のサーバーの同じ番号の作品を消そうとしても、保存版は残る。
        let refused = delete_saved_work(&state, 3, "pixiv", "other").unwrap_err();
        assert_eq!(refused.code, "offline_work_conflict");
        assert!(read_manifest(&state, 3).is_ok());
        assert!(delete_saved_work(&state, 3, "fanbox", "3").is_err());
        assert!(read_manifest(&state, 3).is_ok());
        delete_saved_work(&state, 3, "pixiv", "3").unwrap();
        assert!(!state.work_dir(3).unwrap().exists());
        delete_saved_work(&state, 3, "pixiv", "3").unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn restart_does_not_follow_junctions_into_other_storage() {
        use std::os::windows::process::CommandExt;
        let temporary = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let root = temporary.join("offline");
        let outside = temporary.join("outside");
        let work = root.join("work-3");
        let staged_name = format!("staging-{}", Uuid::new_v4());
        fs::create_dir_all(&work).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.bin"), b"other application data").unwrap();
        for link in [work.join(&staged_name), root.join("work-7")] {
            let result = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"]).arg(&link).arg(&outside)
                .creation_flags(0x08000000).output().unwrap();
            assert!(result.status.success(), "junction fixture failed: {}", String::from_utf8_lossy(&result.stderr));
        }
        let _state = OfflineState::new(root);
        assert_eq!(fs::read(outside.join("keep.bin")).unwrap(), b"other application data");
        assert!(work.join(staged_name).exists());
        fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn corrupt_entries_do_not_hide_healthy_works_and_cannot_delete_repaired_works() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let work_dir = state.work_dir(3).unwrap();
        let snapshot_name = format!("snapshot-{}", Uuid::new_v4());
        let snapshot = work_dir.join(&snapshot_name);
        fs::create_dir_all(&snapshot).unwrap();
        let manifest = serde_json::json!({
            "summary": {
                "work": {"id": 3, "source": "pixiv", "sourceId": "3", "title": "健全な作品",
                    "authorName": "作者", "contentType": "novel", "tags": [], "excerpt": null,
                    "textLength": 1, "assetCount": 0, "hasCover": false, "currentVersion": 1,
                    "contentHash": null, "favorite": false, "seriesTitle": null, "seriesId": null},
                "savedAtMs": 1, "savedVersion": 1, "savedContentHash": null,
                "savedEditHash": null, "pageCount": 1
            },
            "pages": [{"page": 0, "sha256": "test"}], "assets": []
        });
        write_json(&snapshot.join("manifest.json"), &manifest).unwrap();
        write_json(&work_dir.join("current.json"), &CurrentPointer { snapshot: snapshot_name }).unwrap();
        for name in ["work-4", "work-0", "work-03", "work--2", "work-999999999999999999999"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::write(root.join(name).join("current.json"), b"broken").unwrap();
        }
        fs::create_dir_all(root.join("work-5").join("staging-unfinished")).unwrap();
        fs::create_dir_all(root.join("work-6").join(format!("snapshot-{}", Uuid::new_v4()))).unwrap();
        let (works, issues) = scan_works(&state).unwrap();
        assert_eq!(works.len(), 1);
        assert_eq!(works[0].work.id, 3);
        assert_eq!(issues.iter().map(|issue| issue.work_id).collect::<Vec<_>>(), vec![4, 6]);
        assert_eq!(delete_corrupt_work(&state, 3).unwrap_err().code, "offline_work_recovered");
        assert!(read_manifest(&state, 3).is_ok());
        delete_corrupt_work(&state, 4).unwrap();
        assert!(!root.join("work-4").exists());
        assert!(read_manifest(&state, 3).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memberships_survive_snapshot_and_legacy_snapshot_defaults_to_singular_fields() {
        let work = serde_json::json!({
            "id": 7, "source": "pixiv", "sourceId": "work-7", "title": "作品",
            "authorName": "主作者", "authorId": "main-author", "contentType": "novel",
            "tags": [], "excerpt": null, "textLength": 10, "assetCount": 0,
            "hasCover": false, "currentVersion": 1, "contentHash": null,
            "favorite": false, "seriesTitle": "主シリーズ", "seriesId": "main-series",
            "downloadedAt": "2026-10-10T00:00:00Z"
        });
        let people = serde_json::json!([
            {"source": "pixiv", "key": "main-author", "role": "author", "displayName": "主作者"},
            {"source": "pixiv", "key": "co-author", "role": "author", "displayName": "共作者"}
        ]);
        let series = serde_json::json!([
            {"source": "pixiv", "key": "main-series", "title": "主シリーズ", "contentOrder": 1},
            {"source": "pixiv", "key": "anthology", "title": "短編集", "contentOrder": null}
        ]);
        let metadata_json = serde_json::json!({
            "work": work, "people": people, "series": series,
            "versions": [], "isEdited": false, "activeEditId": null, "activeEditHash": null
        });
        let metadata: MobileMetadata = serde_json::from_value(metadata_json.clone()).unwrap();
        assert_eq!(metadata.people.len(), 2);
        assert_eq!(metadata.series.len(), 2);
        let mut old_response = metadata_json;
        old_response.as_object_mut().unwrap().remove("people");
        old_response.as_object_mut().unwrap().remove("series");
        let old_metadata: MobileMetadata = serde_json::from_value(old_response).unwrap();
        assert!(old_metadata.people.is_empty());
        assert!(old_metadata.series.is_empty());

        let snapshot = serde_json::json!({
            "work": metadata.work, "people": metadata.people, "series": metadata.series,
            "savedAtMs": 1, "savedVersion": 1, "savedContentHash": null,
            "savedEditHash": null, "pageCount": 1,
            "hasAuthorCover": true, "hasSeriesCover": true,
            "authorCoverEntity": {"source": "pixiv", "key": "co-author"},
            "seriesCoverEntity": {"source": "pixiv", "key": "anthology"}
        });
        let saved: OfflineWork = serde_json::from_value(snapshot.clone()).unwrap();
        assert_eq!(saved.people[1].key, "co-author");
        assert_eq!(saved.series[1].key, "anthology");
        assert!(saved.series[1].content_order.is_none());
        assert_eq!(saved.author_cover_entity.as_ref().unwrap().key, "co-author");
        assert_eq!(saved.series_cover_entity.as_ref().unwrap().key, "anthology");
        let round_trip: OfflineWork = serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
        assert_eq!(round_trip.people.len(), 2);
        assert_eq!(round_trip.series.len(), 2);
        assert_eq!(round_trip.author_cover_entity.as_ref().unwrap().source, "pixiv");

        let mut legacy = snapshot;
        legacy.as_object_mut().unwrap().remove("people");
        legacy.as_object_mut().unwrap().remove("series");
        legacy.as_object_mut().unwrap().remove("authorCoverEntity");
        legacy.as_object_mut().unwrap().remove("seriesCoverEntity");
        let old_saved: OfflineWork = serde_json::from_value(legacy).unwrap();
        assert!(old_saved.people.is_empty());
        assert!(old_saved.series.is_empty());
        assert!(old_saved.author_cover_entity.is_none());
        assert!(old_saved.series_cover_entity.is_none());
        assert_eq!(old_saved.work.author_id, "main-author");
        assert_eq!(old_saved.work.series_id.as_deref(), Some("main-series"));
    }

    #[test]
    fn extracts_only_numeric_image_references() {
        let ids = asset_ids(
            r#"<img data-asset-id="42"><img data-asset-id='7'><img src="file://private"><img data-asset-id="42">"#,
        );
        assert_eq!(ids, BTreeSet::from([7, 42]));
    }

    #[test]
    fn page_generation_rejects_assets_after_snapshot_replacement() {
        fn write_fixture_snapshot(work_dir: &Path, name: &str, marker: &str, image: &[u8]) {
            let snapshot = work_dir.join(name);
            fs::create_dir_all(snapshot.join("pages")).unwrap();
            fs::create_dir_all(snapshot.join("assets")).unwrap();
            let page = serde_json::to_vec(&serde_json::json!({
                "page": 0, "pageCount": 1, "html": format!("<p>{marker}</p><img data-asset-id=\"42\">"),
                "totalPlainTextChars": marker.len(), "sourcePageStarts": [0]
            })).unwrap();
            fs::write(snapshot.join("pages/0.json"), &page).unwrap();
            fs::write(snapshot.join("assets/42.bin"), image).unwrap();
            write_json(&snapshot.join("manifest.json"), &serde_json::json!({
                "summary": {"work": {
                    "id": 3, "source": "pixiv", "sourceId": "3", "title": "作品",
                    "authorName": "作者", "authorId": "1", "contentType": "novel", "tags": [],
                    "excerpt": null, "textLength": 1, "assetCount": 1, "hasCover": false,
                    "currentVersion": 1, "contentHash": null, "favorite": false,
                    "seriesTitle": null, "seriesId": null
                }, "savedAtMs": 1, "savedVersion": 1, "savedContentHash": null,
                    "savedEditHash": null, "pageCount": 1},
                "pages": [{"page": 0, "sha256": digest(&page)}],
                "assets": [{"id": 42, "mimeType": "image/png", "sha256": digest(image)}]
            })).unwrap();
        }

        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let work_dir = state.work_dir(3).unwrap();
        let generation_a = format!("snapshot-{}", Uuid::new_v4());
        let generation_b = format!("snapshot-{}", Uuid::new_v4());
        let image_a = b"image from generation A";
        let image_b = b"image from generation B";
        write_fixture_snapshot(&work_dir, &generation_a, "generation-A", image_a);
        write_json(&work_dir.join("current.json"), &CurrentPointer { snapshot: generation_a.clone() }).unwrap();

        // The reader has obtained A's HTML and will request its referenced asset later.
        let page_a = read_offline_page(&state, 3, 0).unwrap();
        assert!(page_a.page.html.contains("generation-A"));
        write_fixture_snapshot(&work_dir, &generation_b, "generation-B", image_b);
        write_json(&work_dir.join("current.json"), &CurrentPointer { snapshot: generation_b.clone() }).unwrap();
        cleanup_superseded_snapshots(&work_dir, &generation_b);

        let stale = read_offline_asset(&state, 3, 42, &page_a.generation).unwrap_err();
        assert_eq!(stale.code, "offline_snapshot_changed");
        assert!(!work_dir.join(&generation_a).exists());

        // Reloading the page selects B, and B's matching image remains readable.
        let page_b = read_offline_page(&state, 3, 0).unwrap();
        assert!(page_b.page.html.contains("generation-B"));
        assert_eq!(page_b.generation, generation_b.trim_start_matches("snapshot-"));
        let asset_b = read_offline_asset(&state, 3, 42, &page_b.generation).unwrap();
        assert_eq!(STANDARD.decode(asset_b.base64).unwrap(), image_b);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_snapshot_is_rejected_without_pc_connection() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let dir = state.work_dir(3).unwrap();
        fs::create_dir_all(&dir).unwrap();
        write_json(
            &dir.join("current.json"),
            &CurrentPointer {
                snapshot: "../private".into(),
            },
        )
        .unwrap();
        assert!(read_manifest(&state, 3).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pointer_write_failure_preserves_old_snapshot_and_removes_new_one() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let work_dir = root.join("work-3");
        let old_name = format!("snapshot-{}", Uuid::new_v4());
        let new_name = format!("snapshot-{}", Uuid::new_v4());
        let staging = work_dir.join(format!("staging-{}", Uuid::new_v4()));
        fs::create_dir_all(work_dir.join(&old_name)).unwrap();
        fs::create_dir(&staging).unwrap();
        write_json(
            &work_dir.join("current.json"),
            &CurrentPointer {
                snapshot: old_name.clone(),
            },
        )
        .unwrap();
        // 書き込み不能を同名のディレクトリで再現する。
        fs::create_dir(work_dir.join("current.json.tmp")).unwrap();
        assert!(publish_snapshot(&work_dir, &staging, new_name.clone()).is_err());
        let current: CurrentPointer =
            serde_json::from_slice(&fs::read(work_dir.join("current.json")).unwrap()).unwrap();
        assert_eq!(current.snapshot, old_name);
        assert!(work_dir.join(&old_name).is_dir());
        assert!(!work_dir.join(new_name).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
