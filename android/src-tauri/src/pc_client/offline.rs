//! 利用者が選んだ作品だけを、検証後に版単位で切り替えて保存する。

use super::device_db::DeviceDb;
use super::image_validation::validate_saved_image;
use super::shelf::{self, ShelfScope};
use super::shelf_reader;
use super::*;
use piep_library_protocol::{Id, ResourceScope, WorkReadingView};
use piep_reader_render::pages::reader_outline;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_PAGES: usize = 10_000;
const MAX_ASSETS: usize = 10_000;
const MAX_SNAPSHOT_BYTES: usize = 500 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_COLLECTION_COVERS: usize = 64;
const MAX_COLLECTION_COVER_BYTES: usize = 16 * 1024 * 1024;

#[path = "offline_index.rs"]
mod index;

/// 保存した作品の鍵。棚の作品の UUID で、置き場は `shelf-{UUID}`。
/// 画面とは UUID の文字列でやり取りする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OfflineKey(pub(super) Uuid);

impl TryFrom<String> for OfflineKey {
    type Error = &'static str;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or("invalid shelf work")
    }
}

impl From<OfflineKey> for String {
    fn from(key: OfflineKey) -> Self {
        key.0.to_string()
    }
}

impl OfflineKey {
    /// 小文字・ハイフン区切りの UUID だけ（同じ作品に別の置き場を作らない）。
    pub(super) fn parse(text: &str) -> Option<Self> {
        let id = Uuid::parse_str(text).ok()?;
        (!id.is_nil() && id.to_string() == text).then_some(Self(id))
    }

    fn of(work: &piep_mobile_api::MobileWork) -> Option<Self> {
        Self::parse(&work.id)
    }

    pub(super) fn dir_name(&self) -> String {
        format!("shelf-{}", self.0)
    }

    fn from_dir_name(name: &str) -> Option<Self> {
        Self::parse(name.strip_prefix("shelf-")?)
    }

    fn valid(&self) -> bool {
        !self.0.is_nil()
    }
}

pub struct OfflineState {
    root: PathBuf,
    /// 本文の索引。保存版そのものはファイルに置く。
    db: Arc<DeviceDb>,
    save_lock: Mutex<()>,
    lock: Mutex<()>,
}

impl OfflineState {
    /// 試験用。索引はメモリーに置く。
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self::with_db(root, Arc::new(DeviceDb::in_memory()))
    }

    pub fn with_db(root: PathBuf, db: Arc<DeviceDb>) -> Self {
        let state = Self {
            root,
            db,
            save_lock: Mutex::new(()),
            lock: Mutex::new(()),
        };
        // State 公開前なので保存・読書との競合はない。強制終了で残った作業領域を回収する。
        state.recover_storage();
        state
    }

    fn recover_storage(&self) {
        let Ok(root) = fs::canonicalize(&self.root) else {
            return;
        };
        let Ok(works) = fs::read_dir(&root) else {
            return;
        };
        for work in works.flatten() {
            let name = work.file_name();
            let Some(id) = name.to_str().and_then(OfflineKey::from_dir_name) else {
                continue;
            };
            if !work
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                continue;
            }
            let Ok(work_dir) = fs::canonicalize(work.path()) else {
                continue;
            };
            // Junction などによる、別の保存領域への移動も許可しない。
            if work_dir != root.join(&name) {
                continue;
            }
            let current = read_manifest(self, &id)
                .ok()
                .and_then(|(_, path)| path.file_name().map(|name| name.to_owned()));
            let Ok(entries) = fs::read_dir(&work_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                {
                    continue;
                }
                let entry_name = entry.file_name();
                let Some(name) = entry_name.to_str() else {
                    continue;
                };
                let abandoned_staging = name
                    .strip_prefix("staging-")
                    .is_some_and(|id| Uuid::parse_str(id).is_ok());
                let old_snapshot = current
                    .as_ref()
                    .is_some_and(|current| current != &entry_name)
                    && name
                        .strip_prefix("snapshot-")
                        .is_some_and(|id| Uuid::parse_str(id).is_ok());
                if !abandoned_staging && !old_snapshot {
                    continue;
                }
                let Ok(target) = fs::canonicalize(entry.path()) else {
                    continue;
                };
                // 再帰削除の直前に最終パスがこの作品内の実ディレクトリであることを確認する。
                if target != work_dir.join(&entry_name) || !target.starts_with(&root) {
                    continue;
                }
                let _ = fs::remove_dir_all(target);
            }
        }
    }

    fn work_dir(&self, key: &OfflineKey) -> Result<PathBuf, ClientError> {
        if !key.valid() {
            return Err(ClientError::remote());
        }
        Ok(self.root.join(key.dir_name()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineWork {
    /// 棚の一覧と同じ形で残す。
    pub work: piep_mobile_api::MobileWork,
    /// 保存した棚の UUID。
    pub shelf_library: String,
    #[serde(default)]
    pub people: Vec<MobilePerson>,
    #[serde(default)]
    pub series: Vec<MobileSeriesMembership>,
    pub saved_at_ms: u64,
    pub saved_version: i64,
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
    pub author_cover_entity: Option<OfflineEntityKey>,
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
    cover: Option<CoverRecord>,
    author_cover: Option<CoverRecord>,
    series_cover: Option<CoverRecord>,
    #[serde(default)]
    collection_covers: Vec<CollectionCoverRecord>,
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
    let name = snapshot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(storage)?;
    let id = name.strip_prefix("snapshot-").ok_or_else(storage)?;
    Uuid::parse_str(id)
        .map(|uuid| uuid.to_string())
        .map_err(|_| storage())
}

fn require_generation(snapshot: &Path, generation: &str) -> Result<(), ClientError> {
    let requested = Uuid::parse_str(generation).map_err(|_| snapshot_changed())?;
    let current = snapshot_generation(snapshot).map_err(|_| snapshot_changed())?;
    if requested.to_string() == current {
        Ok(())
    } else {
        Err(snapshot_changed())
    }
}

fn cleanup_superseded_snapshots(work_dir: &Path, current_snapshot: &str) {
    let Ok(canonical_work_dir) = fs::canonicalize(work_dir) else {
        return;
    };
    if let Ok(entries) = fs::read_dir(work_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name != current_snapshot
                && name
                    .strip_prefix("snapshot-")
                    .is_some_and(|id| Uuid::parse_str(id).is_ok())
                && entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                let Ok(target) = fs::canonicalize(entry.path()) else {
                    continue;
                };
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

fn read_offline_page(
    state: &OfflineState,
    work_id: &OfflineKey,
    page: usize,
) -> Result<OfflinePage, ClientError> {
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let generation = snapshot_generation(&snapshot)?;
    let page = page.min(manifest.summary.page_count - 1);
    let record = manifest
        .pages
        .get(page)
        .filter(|record| record.page == page)
        .ok_or_else(ClientError::remote)?;
    let bytes =
        fs::read(snapshot.join("pages").join(format!("{page}.json"))).map_err(|_| storage())?;
    if bytes.len() > MAX_PAGE_BYTES || digest(&bytes) != record.sha256 {
        return Err(storage());
    }
    let page_data: MobilePage = serde_json::from_slice(&bytes).map_err(|_| storage())?;
    if page_data.page != page || page_data.page_count != manifest.summary.page_count {
        return Err(storage());
    }
    Ok(OfflinePage {
        generation,
        page: page_data,
    })
}

fn read_offline_asset(
    state: &OfflineState,
    work_id: &OfflineKey,
    asset_id: i64,
    generation: &str,
) -> Result<MobileAssetData, ClientError> {
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    require_generation(&snapshot, generation)?;
    let record = manifest
        .assets
        .iter()
        .find(|record| record.id == asset_id)
        .ok_or_else(ClientError::remote)?;
    let bytes =
        fs::read(snapshot.join("assets").join(format!("{asset_id}.bin"))).map_err(|_| storage())?;
    if bytes.len() > MAX_ASSET_BYTES || digest(&bytes) != record.sha256 {
        return Err(storage());
    }
    Ok(MobileAssetData {
        mime_type: record.mime_type.clone(),
        base64: STANDARD.encode(bytes),
    })
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

fn read_manifest(
    state: &OfflineState,
    work_id: &OfflineKey,
) -> Result<(Manifest, PathBuf), ClientError> {
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
    if OfflineKey::of(&manifest.summary.work).as_ref() != Some(work_id)
        || manifest.pages.len() != manifest.summary.page_count
        || manifest.summary.page_count == 0
        || manifest.summary.page_count > MAX_PAGES
    {
        return Err(storage());
    }
    Ok((manifest, snapshot))
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

async fn validated_image_response(
    response: reqwest::Response,
) -> Result<MobileAssetData, ClientError> {
    let image = image_response(response).await?;
    let bytes = STANDARD
        .decode(&image.base64)
        .map_err(|_| ClientError::protocol())?;
    validate_saved_image(&bytes, &image.mime_type)?;
    Ok(image)
}

/// 作者・シリーズの画像を棚から取って残す。取れなければ残さない。
#[allow(clippy::too_many_arguments)]
async fn save_entity_cover(
    client: &reqwest::Client,
    saved: &SavedConnection,
    library: Id,
    source: &str,
    kind: &str,
    key: &str,
    staging: &Path,
    total_bytes: &mut usize,
) -> Result<Option<CoverRecord>, ClientError> {
    if key.is_empty() {
        return Ok(None);
    }
    let mut url = reqwest::Url::parse(&format!(
        "{}/api/libraries/{}/{kind}/cover",
        saved.url,
        library.get()
    ))
    .map_err(|_| ClientError::protocol())?;
    url.query_pairs_mut()
        .append_pair("provider", source)
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

/// 選んだ画像を表紙にしたコレクションの表紙を棚から取って残す。
async fn save_collection_covers(
    client: &reqwest::Client,
    saved: &SavedConnection,
    library: Id,
    collections: &[MobileCollection],
    staging: &Path,
    total_bytes: &mut usize,
) -> Result<Vec<CollectionCoverRecord>, ClientError> {
    let mut records = Vec::new();
    let mut cover_bytes = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    for collection in collections
        .iter()
        .filter(|value| value.cover_mode == "file" && value.has_file_cover)
        .take(MAX_COLLECTION_COVERS)
    {
        if cover_bytes >= MAX_COLLECTION_COVER_BYTES || tokio::time::Instant::now() >= deadline {
            break;
        }
        if collection.id.is_empty()
            || collection.id.len() > 128
            || !collection
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            continue;
        }
        let endpoint = format!(
            "/api/libraries/{}/collections/{}/cover/file",
            library.get(),
            collection.id
        );
        let response = match tokio::time::timeout_at(
            deadline,
            get_pc_with_timeout(client, saved, &endpoint, ASSET_TIMEOUT),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => continue,
            Err(_) => break,
        };
        let image =
            match tokio::time::timeout_at(deadline, validated_image_response(response)).await {
                Ok(Ok(image)) => image,
                Ok(Err(_)) => continue,
                Err(_) => break,
            };
        let bytes = match STANDARD.decode(&image.base64) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if cover_bytes.saturating_add(bytes.len()) > MAX_COLLECTION_COVER_BYTES
            || total_bytes.saturating_add(bytes.len()) > MAX_SNAPSHOT_BYTES
        {
            continue;
        }
        let file_number = records.len();
        fs::write(
            staging.join(format!("collection-cover-{file_number}.bin")),
            &bytes,
        )
        .map_err(|_| storage())?;
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
    work_id: OfflineKey,
    issue: &'static str,
}

fn scan_works(state: &OfflineState) -> Result<(Vec<OfflineWork>, Vec<OfflineIssue>), ClientError> {
    let entries = match fs::read_dir(&state.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(_) => return Err(storage()),
    };
    let mut result = Vec::new();
    let mut issues = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| storage())?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(OfflineKey::from_dir_name) else {
            continue;
        };
        if !entry.file_type().map_err(|_| storage())?.is_dir() {
            continue;
        }
        match read_manifest(state, &id) {
            Ok((manifest, _)) => result.push(manifest.summary),
            Err(_) => {
                // 新規保存の通信中は staging だけなので問題として表示しない。
                let has_snapshot = match fs::read_dir(entry.path()) {
                    Ok(mut entries) => entries.any(|item| match item {
                        Ok(item) => {
                            item.file_type().is_ok_and(|kind| kind.is_dir())
                                && item
                                    .file_name()
                                    .to_str()
                                    .and_then(|name| name.strip_prefix("snapshot-"))
                                    .is_some_and(|value| Uuid::parse_str(value).is_ok())
                        }
                        // この作品の中だけを読めない場合、他作品の一覧を妨げない。
                        Err(_) => true,
                    }),
                    Err(_) => true,
                };
                if entry.path().join("current.json").exists() || has_snapshot {
                    issues.push(OfflineIssue {
                        work_id: id,
                        issue: "offline_storage",
                    });
                }
            }
        }
    }
    result.sort_by_key(|work| std::cmp::Reverse(work.saved_at_ms));
    issues.sort_by_key(|issue| issue.work_id);
    Ok((result, issues))
}

/// 保存した作品の本文を探す。索引が保存版とずれていれば、先に作り直す。
#[tauri::command]
pub async fn offline_search_text(
    state: State<'_, OfflineState>,
    query: String,
) -> Result<Vec<index::TextHit>, ClientError> {
    let _guard = state.lock.lock().await;
    index::reconcile(&state)?;
    index::search(&state, &query)
}

#[tauri::command]
pub async fn offline_list_works(
    state: State<'_, OfflineState>,
) -> Result<Vec<OfflineWork>, ClientError> {
    let _guard = state.lock.lock().await;
    Ok(scan_works(&state)?.0)
}

#[tauri::command]
pub async fn offline_list_issues(
    state: State<'_, OfflineState>,
) -> Result<Vec<OfflineIssue>, ClientError> {
    let _guard = state.lock.lock().await;
    Ok(scan_works(&state)?.1)
}

/// 作業領域に一式を組ませ、組み終えたら公開中の版と切り替える。
async fn save_snapshot<F, Fut>(
    state: &OfflineState,
    key: &OfflineKey,
    fill: F,
) -> Result<OfflineWork, ClientError>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<Manifest, ClientError>>,
{
    let work_dir = state.work_dir(key)?;
    fs::create_dir_all(&work_dir).map_err(|_| storage())?;
    let snapshot_name = format!("snapshot-{}", Uuid::new_v4());
    let staging = work_dir.join(format!("staging-{}", Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|_| storage())?;
    let result = fill(staging.clone()).await;
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
    // 索引は控えなので、作れなくても保存は済んでいる。次の検索の前に作り直す。
    if index::index_work(state, key).is_err() {
        let _ = index::remove_work(state, key);
    }
    Ok(manifest.summary)
}

fn publish_snapshot(
    work_dir: &Path,
    staging: &Path,
    snapshot_name: String,
) -> Result<CurrentPointer, ClientError> {
    let snapshot = work_dir.join(&snapshot_name);
    if fs::rename(staging, &snapshot).is_err() {
        let _ = fs::remove_dir_all(staging);
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

/// 棚の作品を保存する。読む画面と同じ一式から同じ組み立てでページを作るので、
/// 保存版のページの切れ目と読書位置は、オンラインで読むときと同じ場所を指す。
#[tauri::command]
pub async fn offline_save_shelf_work(
    pc: State<'_, PcClientState>,
    state: State<'_, OfflineState>,
    library: ShelfScope,
    work: Id,
) -> Result<OfflineWork, ClientError> {
    let _save_guard = state.save_lock.lock().await;
    let key = OfflineKey(work.get());
    state.work_dir(&key)?;
    let detail = shelf::work_detail(&pc, library, work).await?;
    if let Ok((existing, _)) = read_manifest(&state, &key)
        && (existing.summary.work.source != detail.work.source
            || existing.summary.work.source_id != detail.work.source_id)
    {
        return Err(ClientError {
            code: "offline_work_conflict",
            message: "保存済みの作品とは別の作品です。保存済み作品を確認してください",
        });
    }
    let saved = pc.load()?.ok_or_else(ClientError::invalid_pairing)?;
    let client = pc.client(&saved)?;
    let pc = &*pc;
    save_snapshot(&state, &key, |staging| async move {
        save_shelf_into(pc, &client, &saved, library, &detail, &staging).await
    })
    .await
}

/// 棚が今出している版と、作品の画面で見た版が同じか。
fn same_view(view: &WorkReadingView, work: &piep_mobile_api::MobileWork) -> bool {
    view.snapshot_id.get().to_string() == work.snapshot_id
        && view.edit_id.map(|id| id.get().to_string()) == work.edit_id
}

async fn save_shelf_into(
    pc: &PcClientState,
    client: &reqwest::Client,
    saved: &SavedConnection,
    library: ShelfScope,
    detail: &piep_mobile_api::MobileWorkDetail,
    staging: &Path,
) -> Result<Manifest, ClientError> {
    fs::create_dir(staging.join("pages")).map_err(|_| storage())?;
    fs::create_dir(staging.join("assets")).map_err(|_| storage())?;
    let work = &detail.work;
    let found = shelf::lookup(pc, library, &work.source, &work.source_id).await?;
    // 作品の画面を出した後に改稿・編集されていれば、画面と違う版を保存しない。
    if found.entity_id.get().to_string() != work.id || !same_view(&found, work) {
        return Err(ClientError::remote());
    }
    let view = match found.edit_id {
        Some(edit) => ResourceScope::Edit(edit),
        None => ResourceScope::Snapshot(found.snapshot_id),
    };
    let bundle = shelf::bundle(pc, library, found.entity_id, view).await?;
    let document = shelf_reader::build(library, bundle)?;
    let page_count = document.pages().len();
    if page_count == 0 || page_count > MAX_PAGES || document.image_count() > MAX_ASSETS {
        return Err(ClientError::remote());
    }
    let mut total_bytes = 0_usize;
    let mut pages = Vec::with_capacity(page_count);
    for (index, html) in document.pages().iter().enumerate() {
        let page = MobilePage {
            page: index,
            page_count,
            html: html.clone(),
            total_plain_text_chars: document.text_chars(),
            source_page_starts: document.source_page_starts().to_vec(),
        };
        let bytes = serde_json::to_vec(&page).map_err(|_| storage())?;
        if bytes.len() > MAX_PAGE_BYTES {
            return Err(ClientError::remote());
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_SNAPSHOT_BYTES)
            .ok_or_else(ClientError::remote)?;
        fs::write(staging.join("pages").join(format!("{index}.json")), &bytes)
            .map_err(|_| storage())?;
        pages.push(PageRecord {
            page: index,
            sha256: digest(&bytes),
        });
    }
    // 本文の画像は一式に載ったものだけ。部分実体ごとと全体のハッシュを確かめてから置く。
    let mut assets = Vec::with_capacity(document.image_count());
    for index in 0..document.image_count() {
        let target = document.image(index).ok_or_else(ClientError::protocol)?;
        if target.file.length > MAX_ASSET_BYTES as u64 {
            return Err(ClientError::remote());
        }
        let bytes = shelf_reader::fetch_image(pc, &target).await?;
        let mime_type =
            super::cover_protocol::sniffed_mime(&bytes).ok_or_else(ClientError::protocol)?;
        validate_saved_image(&bytes, mime_type)?;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_SNAPSHOT_BYTES)
            .ok_or_else(ClientError::remote)?;
        fs::write(staging.join("assets").join(format!("{index}.bin")), &bytes)
            .map_err(|_| storage())?;
        assets.push(AssetRecord {
            id: index as i64,
            mime_type: mime_type.to_string(),
            sha256: digest(&bytes),
        });
    }
    // 表紙は取れなくても本文の保存は続ける。
    let mut cover = None;
    if work.has_cover
        && let Ok(response) = get_pc_with_timeout(
            client,
            saved,
            &format!(
                "/api/libraries/{}/works/{}/cover?size=large",
                library.library_id().get(),
                work.id
            ),
            ASSET_TIMEOUT,
        )
        .await
        && let Ok(image) = validated_image_response(response).await
        && let Ok(bytes) = STANDARD.decode(&image.base64)
        && total_bytes.saturating_add(bytes.len()) <= MAX_SNAPSHOT_BYTES
    {
        fs::write(staging.join("cover.bin"), &bytes).map_err(|_| storage())?;
        total_bytes += bytes.len();
        cover = Some(CoverRecord {
            mime_type: image.mime_type,
            sha256: digest(&bytes),
        });
    }
    let shelf = library.library_id();
    let author_cover = save_entity_cover(
        client,
        saved,
        shelf,
        &work.source,
        "people",
        &work.author_id,
        staging,
        &mut total_bytes,
    )
    .await?;
    let series_cover = match work.series_id.as_deref() {
        Some(series_id) => {
            save_entity_cover(
                client,
                saved,
                shelf,
                &work.source,
                "series",
                series_id,
                staging,
                &mut total_bytes,
            )
            .await?
        }
        None => None,
    };
    let mut outline_sha256 = None;
    let outline: Vec<MobileOutlineEntry> = reader_outline(document.pages())
        .into_iter()
        .take(2_000)
        .map(|entry| MobileOutlineEntry {
            page: entry.page,
            index: entry.index,
            title: entry.title,
        })
        .collect();
    if !outline.is_empty() {
        let bytes = serde_json::to_vec(&outline).map_err(|_| storage())?;
        if bytes.len() <= 1_000_000 {
            fs::write(staging.join("outline.json"), &bytes).map_err(|_| storage())?;
            outline_sha256 = Some(digest(&bytes));
        }
    }
    // 作品の入ったコレクション（読む画面の欄と、保存した作品の一覧の絞り込みに使う）。
    let work_id = Id::new(uuid::Uuid::parse_str(&work.id).map_err(|_| ClientError::protocol())?)
        .map_err(|_| ClientError::protocol())?;
    let collections = shelf::work_collections(pc, library, work_id).await?;
    let collection_positions = collections
        .iter()
        .filter_map(|collection| {
            collection
                .position
                .map(|position| (collection.id.clone(), position))
        })
        .collect::<BTreeMap<_, _>>();
    let collection_covers = save_collection_covers(
        client,
        saved,
        shelf,
        &collections,
        staging,
        &mut total_bytes,
    )
    .await?;
    let collection_cover_ids = collection_covers
        .iter()
        .map(|record| record.collection_id.clone())
        .collect();
    // 受け取っている間に版が変わっていれば、混ざった版を保存しない。
    let latest = shelf::lookup(pc, library, &work.source, &work.source_id).await?;
    if latest.entity_id != found.entity_id || !same_view(&latest, work) {
        return Err(ClientError::remote());
    }
    let summary = OfflineWork {
        work: work.clone(),
        shelf_library: library.library_id().get().to_string(),
        people: detail.people.clone(),
        series: detail.series.clone(),
        saved_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| storage())?
            .as_millis() as u64,
        saved_version: work.current_version,
        // 版は、残した作品の版と編集の UUID で見分ける。
        saved_edit_hash: work.edit_id.as_ref().map(|id| format!("edit:{id}")),
        page_count,
        collections,
        collection_positions,
        collection_cover_ids,
        has_author_cover: author_cover.is_some(),
        has_series_cover: series_cover.is_some(),
        author_cover_entity: author_cover.as_ref().map(|_| OfflineEntityKey {
            source: work.source.clone(),
            key: work.author_id.clone(),
        }),
        series_cover_entity: series_cover.as_ref().and_then(|_| {
            work.series_id.as_ref().map(|key| OfflineEntityKey {
                source: work.source.clone(),
                key: key.clone(),
            })
        }),
    };
    let manifest = Manifest {
        summary,
        pages,
        assets,
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
    work_id: OfflineKey,
    generation: String,
) -> Result<Vec<MobileOutlineEntry>, ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(&state, &work_id)?;
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

fn verified_cover(
    path: PathBuf,
    sha256: &str,
    mime_type: &str,
) -> Result<(String, Vec<u8>), ClientError> {
    let bytes = fs::read(path).map_err(|_| storage())?;
    if bytes.len() > MAX_ASSET_BYTES || digest(&bytes) != sha256 {
        return Err(storage());
    }
    Ok((mime_type.to_string(), bytes))
}

pub async fn saved_cover(
    state: &OfflineState,
    work_id: &OfflineKey,
) -> Result<(String, Vec<u8>), ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let record = manifest.cover.ok_or_else(ClientError::remote)?;
    verified_cover(
        snapshot.join("cover.bin"),
        &record.sha256,
        &record.mime_type,
    )
}

pub async fn saved_collection_cover(
    state: &OfflineState,
    work_id: &OfflineKey,
    collection_id: &str,
) -> Result<(String, Vec<u8>), ClientError> {
    let _guard = state.lock.lock().await;
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let record = manifest
        .collection_covers
        .iter()
        .find(|record| record.collection_id == collection_id)
        .ok_or_else(ClientError::remote)?;
    verified_cover(
        snapshot.join(format!("collection-cover-{}.bin", record.file_number)),
        &record.sha256,
        &record.mime_type,
    )
}

pub async fn saved_entity_cover(
    state: &OfflineState,
    work_id: &OfflineKey,
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
    work_id: OfflineKey,
    page: usize,
) -> Result<OfflinePage, ClientError> {
    let _guard = state.lock.lock().await;
    read_offline_page(&state, &work_id, page)
}

#[tauri::command]
pub async fn offline_get_asset(
    state: State<'_, OfflineState>,
    work_id: OfflineKey,
    asset_id: i64,
    generation: String,
) -> Result<MobileAssetData, ClientError> {
    let _guard = state.lock.lock().await;
    read_offline_asset(&state, &work_id, asset_id, &generation)
}

#[tauri::command]
pub async fn offline_delete_work(
    state: State<'_, OfflineState>,
    work_id: OfflineKey,
    source: String,
    source_id: String,
) -> Result<(), ClientError> {
    let _save_guard = state.save_lock.lock().await;
    let _guard = state.lock.lock().await;
    delete_saved_work(&state, &work_id, &source, &source_id)
}

/// 数値 ID はサーバーごとに振られる。別のサーバーの同じ番号の作品を消さないよう、
/// 消す直前に保存版の出どころを確かめる。読めない保存版は `offline_delete_corrupt_work` で消す。
fn delete_saved_work(
    state: &OfflineState,
    work_id: &OfflineKey,
    source: &str,
    source_id: &str,
) -> Result<(), ClientError> {
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
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(storage()),
    }
    // 外せなくても、次の検索の前にそろえる。
    let _ = index::remove_work(state, work_id);
    Ok(())
}

fn delete_corrupt_work(state: &OfflineState, work_id: &OfflineKey) -> Result<(), ClientError> {
    let dir = state.work_dir(work_id)?;
    if read_manifest(state, work_id).is_ok() {
        return Err(ClientError {
            code: "offline_work_recovered",
            message: "作品は保存し直されています。一覧を更新してください",
        });
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(storage()),
    }
    // 外せなくても、次の検索の前にそろえる。
    let _ = index::remove_work(state, work_id);
    Ok(())
}

#[tauri::command]
pub async fn offline_delete_corrupt_work(
    state: State<'_, OfflineState>,
    work_id: OfflineKey,
) -> Result<(), ClientError> {
    let _save_guard = state.save_lock.lock().await;
    let _guard = state.lock.lock().await;
    delete_corrupt_work(&state, &work_id)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// 試験用の鍵。番号から決まった UUID を作る。
    pub(in super::super) fn key(number: u128) -> OfflineKey {
        OfflineKey(Uuid::from_u128(number))
    }

    /// 棚の一覧の行（保存した作品の `work`）。
    pub(in super::super) fn work_json(
        key: &OfflineKey,
        source_id: &str,
        title: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": key.0.to_string(), "snapshotId": Uuid::from_u128(1).to_string(),
            "source": "pixiv", "sourceId": source_id, "title": title,
            "authorName": "作者", "authorId": "1", "contentType": "novel", "tags": [],
            "excerpt": null, "textLength": 1, "assetCount": 0, "hasCover": false,
            "currentVersion": 1, "edited": false, "editId": null, "favorite": false,
            "favoriteGeneration": "0", "seriesTitle": null, "seriesId": null,
            "sourceCreatedAt": null, "savedAt": "2026-11-19T00:00:00Z"
        })
    }

    /// 保存版の目録の要約。
    pub(in super::super) fn summary_json(
        key: &OfflineKey,
        source_id: &str,
        title: &str,
        pages: usize,
    ) -> serde_json::Value {
        serde_json::json!({
            "work": work_json(key, source_id, title), "shelfLibrary": Uuid::from_u128(2).to_string(),
            "savedAtMs": 1, "savedVersion": 1,
            "savedEditHash": null, "pageCount": pages
        })
    }

    #[test]
    fn restart_reclaims_interrupted_saves_but_preserves_current_and_unrecoverable_snapshots() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let work_dir = root.join(key(3).dir_name());
        let current_name = format!("snapshot-{}", Uuid::new_v4());
        let current = work_dir.join(&current_name);
        let old = work_dir.join(format!("snapshot-{}", Uuid::new_v4()));
        let staging = root
            .join(key(4).dir_name())
            .join(format!("staging-{}", Uuid::new_v4()));
        let damaged = root.join(key(5).dir_name());
        let retained = damaged.join(format!("snapshot-{}", Uuid::new_v4()));
        for path in [&current, &old, &staging, &retained] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(staging.join("download.bin"), b"interrupted download").unwrap();
        fs::write(old.join("old.bin"), b"superseded snapshot").unwrap();
        fs::write(retained.join("body.bin"), b"possibly recoverable").unwrap();
        write_json(
            &current.join("manifest.json"),
            &serde_json::json!({
                "summary": summary_json(&key(3), "3", "保存作品", 1),
                "pages": [{"page": 0, "sha256": "test"}], "assets": []
            }),
        )
        .unwrap();
        write_json(
            &work_dir.join("current.json"),
            &CurrentPointer {
                snapshot: current_name,
            },
        )
        .unwrap();
        fs::write(damaged.join("current.json"), b"broken pointer").unwrap();
        let unknown = work_dir.join("staging-unknown");
        fs::create_dir(&unknown).unwrap();

        let state = OfflineState::new(root.clone());
        assert!(
            !staging.exists(),
            "interrupted downloads should release storage on restart"
        );
        assert!(
            !old.exists(),
            "crash after publication should not retain the superseded snapshot"
        );
        assert!(
            read_manifest(&state, &key(3)).is_ok(),
            "the active snapshot must remain readable"
        );
        assert!(
            retained.join("body.bin").exists(),
            "a broken current pointer must not cause data loss"
        );
        assert!(
            unknown.exists(),
            "only known generated names may be removed"
        );
        let (_, issues) = scan_works(&state).unwrap();
        assert!(issues.iter().any(|issue| issue.work_id == key(5)));
        // 別の作品として消そうとしても、保存版は残る。
        let refused = delete_saved_work(&state, &key(3), "pixiv", "other").unwrap_err();
        assert_eq!(refused.code, "offline_work_conflict");
        assert!(read_manifest(&state, &key(3)).is_ok());
        assert!(delete_saved_work(&state, &key(3), "fanbox", "3").is_err());
        assert!(read_manifest(&state, &key(3)).is_ok());
        delete_saved_work(&state, &key(3), "pixiv", "3").unwrap();
        assert!(!state.work_dir(&key(3)).unwrap().exists());
        delete_saved_work(&state, &key(3), "pixiv", "3").unwrap();
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
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "junction fixture failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let _state = OfflineState::new(root);
        assert_eq!(
            fs::read(outside.join("keep.bin")).unwrap(),
            b"other application data"
        );
        assert!(work.join(staged_name).exists());
        fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn corrupt_entries_do_not_hide_healthy_works_and_cannot_delete_repaired_works() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let work_dir = state.work_dir(&key(3)).unwrap();
        let snapshot_name = format!("snapshot-{}", Uuid::new_v4());
        let snapshot = work_dir.join(&snapshot_name);
        fs::create_dir_all(&snapshot).unwrap();
        let manifest = serde_json::json!({
            "summary": summary_json(&key(3), "3", "健全な作品", 1),
            "pages": [{"page": 0, "sha256": "test"}], "assets": []
        });
        write_json(&snapshot.join("manifest.json"), &manifest).unwrap();
        write_json(
            &work_dir.join("current.json"),
            &CurrentPointer {
                snapshot: snapshot_name,
            },
        )
        .unwrap();
        // 置き場の名前として正しくないもの（番号だけの置き場を含む）は見ない。
        for name in [
            key(4).dir_name(),
            format!("shelf-{}", Uuid::nil()),
            format!("shelf-{}", key(8).0.simple()),
            key(9).dir_name().to_uppercase(),
            "work-4".to_string(),
        ] {
            fs::create_dir(root.join(&name)).unwrap();
            fs::write(root.join(&name).join("current.json"), b"broken").unwrap();
        }
        fs::create_dir_all(root.join(key(5).dir_name()).join("staging-unfinished")).unwrap();
        fs::create_dir_all(
            root.join(key(6).dir_name())
                .join(format!("snapshot-{}", Uuid::new_v4())),
        )
        .unwrap();
        let (works, issues) = scan_works(&state).unwrap();
        assert_eq!(works.len(), 1);
        assert_eq!(OfflineKey::of(&works[0].work), Some(key(3)));
        assert_eq!(
            issues.iter().map(|issue| issue.work_id).collect::<Vec<_>>(),
            vec![key(4), key(6)]
        );
        assert_eq!(
            delete_corrupt_work(&state, &key(3)).unwrap_err().code,
            "offline_work_recovered"
        );
        assert!(read_manifest(&state, &key(3)).is_ok());
        delete_corrupt_work(&state, &key(4)).unwrap();
        assert!(!root.join(key(4).dir_name()).exists());
        assert!(read_manifest(&state, &key(3)).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memberships_survive_snapshot_and_missing_fields_default_to_empty() {
        let mut work = work_json(&key(7), "work-7", "作品");
        work["authorName"] = "主作者".into();
        work["authorId"] = "main-author".into();
        work["seriesTitle"] = "主シリーズ".into();
        work["seriesId"] = "main-series".into();
        let people = serde_json::json!([
            {"source": "pixiv", "key": "main-author", "role": "author", "displayName": "主作者"},
            {"source": "pixiv", "key": "co-author", "role": "author", "displayName": "共作者"}
        ]);
        let series = serde_json::json!([
            {"source": "pixiv", "key": "main-series", "title": "主シリーズ", "contentOrder": 1},
            {"source": "pixiv", "key": "anthology", "title": "短編集", "contentOrder": null}
        ]);
        let snapshot = serde_json::json!({
            "work": work, "shelfLibrary": Uuid::from_u128(2).to_string(),
            "people": people, "series": series, "savedAtMs": 1, "savedVersion": 1,
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
        let round_trip: OfflineWork =
            serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
        assert_eq!(round_trip.people.len(), 2);
        assert_eq!(round_trip.series.len(), 2);
        assert_eq!(
            round_trip.author_cover_entity.as_ref().unwrap().source,
            "pixiv"
        );

        let mut bare = snapshot;
        for field in ["people", "series", "authorCoverEntity", "seriesCoverEntity"] {
            bare.as_object_mut().unwrap().remove(field);
        }
        let bare: OfflineWork = serde_json::from_value(bare).unwrap();
        assert!(bare.people.is_empty());
        assert!(bare.series.is_empty());
        assert!(bare.author_cover_entity.is_none());
        assert!(bare.series_cover_entity.is_none());
        assert_eq!(bare.work.author_id, "main-author");
        assert_eq!(bare.work.series_id.as_deref(), Some("main-series"));
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
            write_json(
                &snapshot.join("manifest.json"),
                &serde_json::json!({
                    "summary": summary_json(&key(3), "3", "作品", 1),
                    "pages": [{"page": 0, "sha256": digest(&page)}],
                    "assets": [{"id": 42, "mimeType": "image/png", "sha256": digest(image)}]
                }),
            )
            .unwrap();
        }

        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let work_dir = state.work_dir(&key(3)).unwrap();
        let generation_a = format!("snapshot-{}", Uuid::new_v4());
        let generation_b = format!("snapshot-{}", Uuid::new_v4());
        let image_a = b"image from generation A";
        let image_b = b"image from generation B";
        write_fixture_snapshot(&work_dir, &generation_a, "generation-A", image_a);
        write_json(
            &work_dir.join("current.json"),
            &CurrentPointer {
                snapshot: generation_a.clone(),
            },
        )
        .unwrap();

        // The reader has obtained A's HTML and will request its referenced asset later.
        let page_a = read_offline_page(&state, &key(3), 0).unwrap();
        assert!(page_a.page.html.contains("generation-A"));
        write_fixture_snapshot(&work_dir, &generation_b, "generation-B", image_b);
        write_json(
            &work_dir.join("current.json"),
            &CurrentPointer {
                snapshot: generation_b.clone(),
            },
        )
        .unwrap();
        cleanup_superseded_snapshots(&work_dir, &generation_b);

        let stale = read_offline_asset(&state, &key(3), 42, &page_a.generation).unwrap_err();
        assert_eq!(stale.code, "offline_snapshot_changed");
        assert!(!work_dir.join(&generation_a).exists());

        // Reloading the page selects B, and B's matching image remains readable.
        let page_b = read_offline_page(&state, &key(3), 0).unwrap();
        assert!(page_b.page.html.contains("generation-B"));
        assert_eq!(
            page_b.generation,
            generation_b.trim_start_matches("snapshot-")
        );
        let asset_b = read_offline_asset(&state, &key(3), 42, &page_b.generation).unwrap();
        assert_eq!(STANDARD.decode(asset_b.base64).unwrap(), image_b);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn offline_keys_are_canonical_uuids_and_name_their_own_folder() {
        let id = Uuid::now_v7();
        let shelf = OfflineKey(id);
        assert_eq!(
            serde_json::to_value(shelf).unwrap(),
            serde_json::json!(id.to_string())
        );
        assert_eq!(
            serde_json::from_value::<OfflineKey>(serde_json::json!(id.to_string())).unwrap(),
            shelf
        );
        for bad in [
            serde_json::json!(7),
            serde_json::json!("7"),
            serde_json::json!(id.to_string().to_uppercase()),
            serde_json::json!(id.simple().to_string()),
            serde_json::json!(Uuid::nil().to_string()),
            serde_json::json!("../work-3"),
        ] {
            assert!(
                serde_json::from_value::<OfflineKey>(bad.clone()).is_err(),
                "{bad}"
            );
        }
        assert_eq!(OfflineKey::from_dir_name(&shelf.dir_name()), Some(shelf));
        for bad in [
            "work-7",
            "shelf-",
            &format!("shelf-{}", id.simple()),
            &format!("work-{id}"),
        ] {
            assert_eq!(OfflineKey::from_dir_name(bad), None, "{bad}");
        }
        assert_eq!(OfflineKey::parse(&id.to_string()), Some(shelf));
        assert_eq!(OfflineKey::parse("7"), None);
    }

    #[test]
    fn a_saved_shelf_work_is_listed_searched_and_deleted_by_its_uuid() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let id = Uuid::now_v7();
        let key = OfflineKey(id);
        let work_dir = state.work_dir(&key).unwrap();
        assert_eq!(work_dir, root.join(format!("shelf-{id}")));
        let snapshot_name = format!("snapshot-{}", Uuid::new_v4());
        let snapshot = work_dir.join(&snapshot_name);
        fs::create_dir_all(snapshot.join("pages")).unwrap();
        fs::create_dir_all(snapshot.join("assets")).unwrap();
        // 棚の画像の番号は 0 から。
        let page = serde_json::to_vec(&serde_json::json!({
            "page": 0, "pageCount": 1, "html": "<p>棚の目印の文</p><img data-asset-id=\"0\">",
            "totalPlainTextChars": 7, "sourcePageStarts": [0]
        }))
        .unwrap();
        fs::write(snapshot.join("pages/0.json"), &page).unwrap();
        let image = b"shelf image";
        fs::write(snapshot.join("assets/0.bin"), image).unwrap();
        let snapshot_id = Uuid::now_v7().to_string();
        write_json(
            &snapshot.join("manifest.json"),
            &serde_json::json!({
                "summary": {"work": {
                    "id": id.to_string(), "snapshotId": snapshot_id, "source": "pixiv",
                    "sourceId": "91", "title": "作品", "authorName": "作者", "authorId": "1",
                    "contentType": "novel", "tags": [], "excerpt": null, "textLength": 7,
                    "assetCount": 1, "hasCover": false, "currentVersion": 2, "edited": false,
                    "editId": null, "favorite": false, "favoriteGeneration": "0",
                    "seriesTitle": null, "seriesId": null, "sourceCreatedAt": null,
                    "savedAt": "2026-11-19T00:00:00Z"
                }, "shelfLibrary": Uuid::now_v7().to_string(), "savedAtMs": 1, "savedVersion": 2,
                    "savedEditHash": null,
                    "pageCount": 1},
                "pages": [{"page": 0, "sha256": digest(&page)}],
                "assets": [{"id": 0, "mimeType": "image/png", "sha256": digest(image)}]
            }),
        )
        .unwrap();
        write_json(
            &work_dir.join("current.json"),
            &CurrentPointer {
                snapshot: snapshot_name,
            },
        )
        .unwrap();

        let (works, issues) = scan_works(&state).unwrap();
        assert!(issues.is_empty());
        assert_eq!(works.len(), 1);
        assert_eq!(OfflineKey::of(&works[0].work), Some(key));
        // 画面へは棚の一覧と同じ形（id は UUID の文字列）で出る。
        let listed = serde_json::to_value(&works[0]).unwrap();
        assert_eq!(listed["work"]["id"], serde_json::json!(id.to_string()));
        assert_eq!(listed["work"]["snapshotId"], serde_json::json!(snapshot_id));

        let page = read_offline_page(&state, &key, 0).unwrap();
        let asset = read_offline_asset(&state, &key, 0, &page.generation).unwrap();
        assert_eq!(STANDARD.decode(asset.base64).unwrap(), image);

        index::reconcile(&state).unwrap();
        let hits = index::search(&state, "目印の").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            serde_json::to_value(&hits[0]).unwrap()["workId"],
            serde_json::json!(id.to_string())
        );

        assert!(delete_saved_work(&state, &key, "pixiv", "other").is_err());
        delete_saved_work(&state, &key, "pixiv", "91").unwrap();
        assert!(!work_dir.exists());
        assert!(index::search(&state, "目印の").unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_snapshot_is_rejected_without_pc_connection() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let dir = state.work_dir(&key(3)).unwrap();
        fs::create_dir_all(&dir).unwrap();
        write_json(
            &dir.join("current.json"),
            &CurrentPointer {
                snapshot: "../private".into(),
            },
        )
        .unwrap();
        assert!(read_manifest(&state, &key(3)).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pointer_write_failure_preserves_old_snapshot_and_removes_new_one() {
        let root = std::env::temp_dir().join(format!("piep-offline-test-{}", Uuid::new_v4()));
        let work_dir = root.join(key(3).dir_name());
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

#[cfg(test)]
#[path = "offline_regressions.rs"]
mod regressions;
