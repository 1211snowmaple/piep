use crate::database::queries::EntityProfileFreshness;
use crate::database::{Database, DownloadEntry, NewAsset, NewDownload, NewVersion};
use crate::downloader::fanbox::get_post_detail;
use crate::downloader::pixiv::{get_novel_detail, PixivNovelContent};
use crate::fanbox_api::client::FanboxAPI;
use crate::fanbox_api::models::FanboxPost;
use crate::pixiv_api;
use crate::AppState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once, OnceLock, Weak};
use tauri::Manager;

type WorkSaveMutex = tokio::sync::Mutex<()>;
static WORK_SAVE_LOCKS: OnceLock<Mutex<HashMap<String, Weak<WorkSaveMutex>>>> = OnceLock::new();

fn work_save_mutex(source: &str, source_id: &str) -> Arc<WorkSaveMutex> {
    let key = format!("{source}\0{source_id}");
    let mut locks = WORK_SAVE_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(WorkSaveMutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

const VERSION_STAGE_SYNC_MAX_DEPTH: usize = 32;
const VERSION_STAGE_SYNC_MAX_ENTRIES: usize = 50_000;
#[cfg(windows)]
const WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
static DIRECTORY_SYNC_UNSUPPORTED_WARNING: Once = Once::new();

#[derive(Clone, Copy)]
struct StageSyncLimits {
    max_depth: usize,
    max_entries: usize,
}

impl Default for StageSyncLimits {
    fn default() -> Self {
        Self {
            max_depth: VERSION_STAGE_SYNC_MAX_DEPTH,
            max_entries: VERSION_STAGE_SYNC_MAX_ENTRIES,
        }
    }
}

fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn is_unpublished_part_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.ends_with(".part"))
}

#[cfg(windows)]
fn open_directory_for_sync(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
}

#[cfg(not(windows))]
fn open_directory_for_sync(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn directory_sync_is_unsupported(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
    ) {
        return true;
    }
    #[cfg(windows)]
    {
        // Windows filesystems commonly reject FlushFileBuffers for directory
        // handles even when they support write-through rename. Only the known
        // unsupported/invalid-handle errors are tolerated; media and I/O
        // failures still abort the save.
        matches!(error.raw_os_error(), Some(1 | 5 | 6 | 50 | 87))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn sync_directory_durable(path: &Path) -> Result<(), String> {
    let result = open_directory_for_sync(path).and_then(|directory| directory.sync_all());
    match result {
        Ok(()) => Ok(()),
        Err(error) if directory_sync_is_unsupported(&error) => {
            DIRECTORY_SYNC_UNSUPPORTED_WARNING.call_once(|| {
                log::warn!(
                    "Directory fsync is unsupported on this filesystem; relying on synced files and atomic/write-through rename"
                );
            });
            Ok(())
        }
        Err(error) => Err(format!("Directory durability sync failed: {error}")),
    }
}

fn durable_sync_stage_tree_with_limits(root: &Path, limits: StageSyncLimits) -> Result<(), String> {
    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|error| format!("Version stage inspection failed: {error}"))?;
    if metadata_is_link_or_reparse(&root_metadata) || !root_metadata.file_type().is_dir() {
        return Err("Version stage is a link/reparse point or not a directory".to_string());
    }

    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut entry_count = 1usize;
    while let Some((directory, depth)) = pending.pop() {
        if depth > limits.max_depth {
            return Err(format!(
                "Version stage nesting exceeds {} levels",
                limits.max_depth
            ));
        }
        let metadata = std::fs::symlink_metadata(&directory)
            .map_err(|error| format!("Version stage directory inspection failed: {error}"))?;
        if metadata_is_link_or_reparse(&metadata) || !metadata.file_type().is_dir() {
            return Err("Version stage contains a linked/reparse directory".to_string());
        }
        directories.push(directory.clone());

        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("Version stage traversal failed: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("Version stage entry failed: {error}"))?;
            entry_count = entry_count
                .checked_add(1)
                .ok_or_else(|| "Version stage entry count overflow".to_string())?;
            if entry_count > limits.max_entries {
                return Err(format!(
                    "Version stage exceeds the {} entry safety limit",
                    limits.max_entries
                ));
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("Version stage entry inspection failed: {error}"))?;
            if metadata_is_link_or_reparse(&metadata) {
                return Err("Version stage contains a filesystem link/reparse point".to_string());
            }
            if metadata.file_type().is_dir() {
                pending.push((path, depth + 1));
            } else if metadata.file_type().is_file() {
                if is_unpublished_part_file(&path) {
                    return Err("Version stage contains an unpublished partial file".to_string());
                }
                files.push(path);
            } else {
                return Err("Version stage contains a special filesystem entry".to_string());
            }
        }
    }

    for path in files {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("Version file open for durability failed: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("Version file durability inspection failed: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err("Version file changed type during durability sync".to_string());
        }
        file.sync_all()
            .map_err(|error| format!("Version file durability sync failed: {error}"))?;
    }

    // Children are synced before their parents so every directory entry is
    // stable before the root itself is made durable.
    for directory in directories.into_iter().rev() {
        sync_directory_durable(&directory)?;
    }
    Ok(())
}

fn durable_sync_stage_tree(root: &Path) -> Result<(), String> {
    durable_sync_stage_tree_with_limits(root, StageSyncLimits::default())
}

#[cfg(windows)]
fn durable_rename_version(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    // `canonicalize` yields a verbatim/extended-length Windows path, retaining
    // compatibility with version trees whose absolute path exceeds MAX_PATH.
    let source = source
        .canonicalize()
        .map_err(|error| format!("Version source resolution before publish failed: {error}"))?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| "Version destination has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("Version destination parent resolution failed: {error}"))?;
    let destination = destination_parent.join(
        destination
            .file_name()
            .ok_or_else(|| "Version destination has no file name".to_string())?,
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
    // SAFETY: both paths are NUL-terminated and remain alive throughout the
    // synchronous Win32 call. Omitting REPLACE_EXISTING preserves an existing
    // version if another actor somehow creates it before publish.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| format!("Version write-through publish failed: {error}"))
    }
}

#[cfg(not(windows))]
fn durable_rename_version(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| format!("Version publish failed: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishFailurePoint {
    None,
    BeforeTreeSync,
    AfterTreeSync,
    AfterRename,
}

#[derive(Debug)]
struct VersionStage {
    staging_path: PathBuf,
    final_path: PathBuf,
    published: bool,
    committed: bool,
    cleanup_complete: bool,
    failure_point: PublishFailurePoint,
}

#[derive(Debug)]
struct VersionCommitError {
    message: String,
    rollback_durable: bool,
    published_by_stage: Option<bool>,
}

impl VersionStage {
    fn create(work_root: &Path, version: i64) -> Result<Self, String> {
        let final_path = work_root.join(format!("v{version}"));
        match std::fs::symlink_metadata(&final_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(format!(
                    "Version directory already exists and will not be overwritten: {}",
                    final_path.display()
                ))
            }
            Err(error) => {
                return Err(format!(
                    "Version destination inspection failed before staging: {error}"
                ))
            }
        }
        for _ in 0..16 {
            let staging_path =
                work_root.join(format!(".v{version}.{:016x}.stage", rand::random::<u64>()));
            match std::fs::create_dir(&staging_path) {
                Ok(()) => {
                    return Ok(Self {
                        staging_path,
                        final_path,
                        published: false,
                        committed: false,
                        cleanup_complete: false,
                        failure_point: PublishFailurePoint::None,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("Version staging creation failed: {error}")),
            }
        }
        Err("Failed to allocate a unique version staging directory".to_string())
    }

    fn publish_blocking(&mut self) -> Result<(), String> {
        if self.failure_point == PublishFailurePoint::BeforeTreeSync {
            return Err("Injected version publish failure before tree sync".to_string());
        }
        durable_sync_stage_tree(&self.staging_path)?;
        let parent = self
            .staging_path
            .parent()
            .ok_or_else(|| "Version stage has no parent directory".to_string())?;
        sync_directory_durable(parent)?;
        if self.failure_point == PublishFailurePoint::AfterTreeSync {
            return Err("Injected version publish failure after tree sync".to_string());
        }
        match std::fs::symlink_metadata(&self.final_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err("Version destination appeared before publish".to_string()),
            Err(error) => {
                return Err(format!(
                    "Version destination inspection before publish failed: {error}"
                ))
            }
        }
        if let Err(error) = durable_rename_version(&self.staging_path, &self.final_path) {
            // A write-through rename can theoretically report a late flush
            // failure after the namespace move. Detect that state so rollback
            // removes the directory we published instead of looking only for
            // the now-absent stage path.
            let stage_is_absent = matches!(
                std::fs::symlink_metadata(&self.staging_path),
                Err(inspect_error) if inspect_error.kind() == std::io::ErrorKind::NotFound
            );
            let final_is_directory =
                std::fs::symlink_metadata(&self.final_path).is_ok_and(|metadata| {
                    !metadata_is_link_or_reparse(&metadata) && metadata.file_type().is_dir()
                });
            self.published = stage_is_absent && final_is_directory;
            return Err(error);
        }
        self.published = true;
        if self.failure_point == PublishFailurePoint::AfterRename {
            return Err("Injected version publish failure after rename".to_string());
        }
        // On Unix this is the durability boundary for the rename. Windows
        // normally reports directory FlushFileBuffers as unsupported, in which
        // case MOVEFILE_WRITE_THROUGH above is the platform guarantee.
        sync_directory_durable(parent)?;
        Ok(())
    }

    async fn publish(self) -> Result<Self, VersionCommitError> {
        let result = tokio::task::spawn_blocking(move || {
            let mut stage = self;
            match stage.publish_blocking() {
                Ok(()) => Ok(stage),
                Err(error) => Err((stage, error)),
            }
        })
        .await;
        match result {
            Ok(Ok(stage)) => Ok(stage),
            Ok(Err((mut stage, message))) => {
                let published_by_stage = stage.published;
                let rollback_durable = stage.rollback().is_ok();
                Err(VersionCommitError {
                    message,
                    rollback_durable,
                    published_by_stage: Some(published_by_stage),
                })
            }
            Err(error) => Err(VersionCommitError {
                message: format!("Version durability worker failed: {error}"),
                // The worker owns and drops the stage while unwinding, but the
                // journal must remain because its directory sync result cannot
                // be observed here.
                rollback_durable: false,
                published_by_stage: None,
            }),
        }
    }

    #[cfg(test)]
    fn inject_publish_failure(&mut self, failure_point: PublishFailurePoint) {
        self.failure_point = failure_point;
    }

    fn commit(&mut self) {
        self.committed = true;
    }

    fn rollback(&mut self) -> Result<(), String> {
        if self.committed || self.cleanup_complete {
            return Ok(());
        }
        let cleanup_path = if self.published {
            self.final_path.clone()
        } else {
            self.staging_path.clone()
        };
        match std::fs::remove_dir_all(&cleanup_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Version rollback removal failed: {error}")),
        }
        let parent = cleanup_path
            .parent()
            .ok_or_else(|| "Version rollback path has no parent".to_string())?;
        sync_directory_durable(parent)?;
        self.cleanup_complete = true;
        Ok(())
    }

    fn final_path_for(&self, staged_path: &Path) -> Result<PathBuf, String> {
        let relative = staged_path
            .strip_prefix(&self.staging_path)
            .map_err(|_| "Staged file escaped its version directory".to_string())?;
        Ok(self.final_path.join(relative))
    }
}

impl Drop for VersionStage {
    fn drop(&mut self) {
        if let Err(error) = self.rollback() {
            log::warn!("Failed to durably roll back version directory: {error}");
        }
    }
}

async fn commit_published_version<F>(
    mut stage: VersionStage,
    commit_database: F,
) -> Result<i64, VersionCommitError>
where
    F: FnOnce() -> Result<i64, String>,
{
    stage = stage.publish().await?;
    let download_id = match commit_database() {
        Ok(download_id) => download_id,
        Err(message) => {
            let rollback_durable = stage.rollback().is_ok();
            return Err(VersionCommitError {
                message,
                rollback_durable,
                published_by_stage: Some(true),
            });
        }
    };
    stage.commit();
    Ok(download_id)
}

async fn commit_journaled_version<F>(
    db: &Database,
    source: &str,
    source_id: &str,
    version: i64,
    stage: VersionStage,
    commit_database: F,
) -> Result<i64, String>
where
    F: FnOnce(&str) -> Result<i64, String>,
{
    let journal_id = db.create_download_save_journal(
        source,
        source_id,
        version,
        &stage.staging_path,
        &stage.final_path,
    )?;
    let staging_path = stage.staging_path.clone();
    let final_path = stage.final_path.clone();
    let result = commit_published_version(stage, || commit_database(&journal_id)).await;
    match result {
        Ok(download_id) => {
            // Files, directory entries, and the publish rename reached their
            // durability boundary before the database transaction began. No
            // filesystem mutation occurs after commit, so deleting only the
            // SQLite recovery row does not require another filesystem fsync.
            if let Err(error) = db.finish_download_save_journal(&journal_id) {
                // The marker is committed in the same transaction as the
                // version, so startup can safely finish journal cleanup.
                log::warn!("Failed to finish committed download save journal: {error}");
            }
            Ok(download_id)
        }
        Err(error) => {
            // Remove the staged/published tree before forgetting the durable
            // rollback record. A crash during either step remains recoverable.
            let is_absent = |path: &Path| {
                matches!(
                    std::fs::symlink_metadata(path),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                )
            };
            let final_state_is_safe =
                matches!(error.published_by_stage, Some(false)) || is_absent(&final_path);
            if error.rollback_durable && is_absent(&staging_path) && final_state_is_safe {
                if let Err(cleanup_error) = db.finish_download_save_journal(&journal_id) {
                    log::warn!(
                        "Failed to finish rolled-back download save journal: {cleanup_error}"
                    );
                }
            } else {
                log::warn!(
                    "Retaining download save journal because durable filesystem rollback was incomplete"
                );
            }
            Err(error.message)
        }
    }
}

#[tauri::command]
pub async fn fetch_pixiv_novel(
    novel_id: String,
    refresh_token: String,
) -> Result<PixivNovelContent, String> {
    let detail = get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())?;
    let (text, webview_cover_url, illusts, images) =
        crate::downloader::pixiv::get_novel_text_and_assets(&novel_id, &refresh_token)
            .await
            .map_err(|e| e.to_string())?;
    let cover_url =
        crate::downloader::pixiv::best_novel_cover(webview_cover_url, detail.cover_url.as_deref());
    Ok(PixivNovelContent {
        detail,
        text,
        cover_url: Some(cover_url),
        illusts: Some(illusts),
        images: Some(images),
    })
}

#[tauri::command]
pub async fn fetch_pixiv_novel_metadata(
    novel_id: String,
    refresh_token: String,
) -> Result<crate::downloader::pixiv::PixivNovelDetail, String> {
    get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_fanbox_post(
    post_id: String,
    cookie: String,
    user_agent: String,
) -> Result<serde_json::Value, String> {
    get_post_detail(&post_id, &cookie, &user_agent)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_fanbox_creator_posts(
    creator_id: String,
    cookie: String,
    user_agent: String,
) -> Result<Vec<FanboxPost>, String> {
    let api = FanboxAPI::new(cookie, user_agent);
    api.get_all_creator_posts(&creator_id)
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn fetch_fanbox_creator_posts_since(
    creator_id: String,
    cookie: String,
    user_agent: String,
    stop_source_id: Option<&str>,
) -> Result<Vec<FanboxPost>, String> {
    FanboxAPI::new(cookie, user_agent)
        .get_creator_posts_since(&creator_id, stop_source_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_pixiv_series_novels(
    series_id: String,
    refresh_token: String,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    fetch_pixiv_series_novels_since(series_id, refresh_token, None).await
}

pub(crate) async fn fetch_pixiv_series_novels_since(
    series_id: String,
    refresh_token: String,
    stop_source_id: Option<&str>,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let api = pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(refresh_token);
    let id_u64: u64 = series_id.parse().map_err(|_| "Invalid series ID")?;

    let mut last_order: Option<String> = None;
    let mut all_novels = Vec::new();
    let mut page_count = 0;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut seen_ids = std::collections::HashSet::new();

    loop {
        page_count += 1;
        if page_count > 100 {
            return Err("Pixiv series history exceeded the 100-page safety limit".to_string());
        }

        let res = api
            .novel_series(id_u64, None, last_order.as_deref(), true)
            .await
            .map_err(|e| e.to_string())?;

        #[derive(serde::Deserialize)]
        struct NovelSeriesResponse {
            novels: Vec<pixiv_api::models::NovelInfo>,
            next_url: Option<String>,
        }

        let parsed: NovelSeriesResponse = serde_json::from_value(res).map_err(|e| e.to_string())?;
        let mut reached_stop = false;
        for novel in parsed.novels {
            let id = novel.id.to_string();
            if stop_source_id.is_some_and(|stop| stop == id) {
                reached_stop = true;
                break;
            }
            if seen_ids.insert(novel.id) {
                all_novels.push(novel);
            }
            if all_novels.len() >= 10_000 {
                return Err(
                    "Pixiv series history exceeded the 10,000-item safety limit".to_string()
                );
            }
        }
        if reached_stop {
            break;
        }

        if let Some(url) = parsed.next_url {
            let parsed_url = reqwest::Url::parse(&url)
                .map_err(|error| format!("Invalid Pixiv series cursor URL: {error}"))?;
            let cursor = parsed_url
                .query_pairs()
                .find_map(|(key, value)| (key == "last_order").then(|| value.into_owned()))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Pixiv series response omitted last_order cursor".to_string())?;
            if !seen_cursors.insert(cursor.clone()) {
                return Err("Pixiv series cursor repeated without progress".to_string());
            }
            last_order = Some(cursor);
        } else {
            break;
        }
    }

    Ok(all_novels)
}

#[tauri::command]
pub async fn fetch_pixiv_user_novels(
    user_id: String,
    refresh_token: String,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    fetch_pixiv_user_novels_since(user_id, refresh_token, None).await
}

pub(crate) async fn fetch_pixiv_user_novels_since(
    user_id: String,
    refresh_token: String,
    stop_source_id: Option<&str>,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let api = pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(refresh_token);
    let id_u64: u64 = user_id.parse().map_err(|_| "Invalid user ID")?;

    let mut offset: Option<String> = None;
    let mut all_novels = Vec::new();
    let mut page_count = 0;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut seen_ids = std::collections::HashSet::new();

    loop {
        page_count += 1;
        if page_count > 100 {
            return Err("Pixiv author history exceeded the 100-page safety limit".to_string());
        }

        let res = api
            .user_novels(id_u64, None, offset.as_deref(), true)
            .await
            .map_err(|e| e.to_string())?;

        let mut reached_stop = false;
        for novel in res.novels {
            let id = novel.id.to_string();
            if stop_source_id.is_some_and(|stop| stop == id) {
                reached_stop = true;
                break;
            }
            if seen_ids.insert(novel.id) {
                all_novels.push(novel);
            }
            if all_novels.len() >= 10_000 {
                return Err(
                    "Pixiv author history exceeded the 10,000-item safety limit".to_string()
                );
            }
        }
        if reached_stop {
            break;
        }

        if let Some(url) = res.next_url {
            let parsed_url = reqwest::Url::parse(&url)
                .map_err(|error| format!("Invalid Pixiv author cursor URL: {error}"))?;
            let cursor = parsed_url
                .query_pairs()
                .find_map(|(key, value)| (key == "offset").then(|| value.into_owned()))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "Pixiv author response omitted offset cursor".to_string())?;
            if !seen_cursors.insert(cursor.clone()) {
                return Err("Pixiv author cursor repeated without progress".to_string());
            }
            offset = Some(cursor);
        } else {
            break;
        }
    }

    Ok(all_novels)
}

#[tauri::command]
pub async fn db_check_exists(
    app: tauri::AppHandle,
    source: String,
    source_id: String,
) -> Result<bool, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.check_exists(&source, &source_id)
}

#[tauri::command]
pub async fn fetch_pixiv_novel_by_url(
    url: String,
    refresh_token: String,
) -> Result<PixivNovelContent, String> {
    let novel_id =
        crate::downloader::pixiv::extract_novel_id(&url).ok_or("Invalid Pixiv Novel URL")?;
    let detail = get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())?;
    let (text, webview_cover_url, illusts, images) =
        crate::downloader::pixiv::get_novel_text_and_assets(&novel_id, &refresh_token)
            .await
            .map_err(|e| e.to_string())?;
    let cover_url =
        crate::downloader::pixiv::best_novel_cover(webview_cover_url, detail.cover_url.as_deref());
    Ok(PixivNovelContent {
        detail,
        text,
        cover_url: Some(cover_url),
        illusts: Some(illusts),
        images: Some(images),
    })
}

/// 本文テキストのハッシュ値、文字数、更新時間を算出する（Pixivはメタデータを含む統合ハッシュ）
pub(crate) fn compute_content_details(
    data: &serde_json::Value,
    source: &str,
) -> (String, i64, Option<String>) {
    use sha2::{Digest, Sha256};
    let mut text = String::new();
    let mut source_updated_at = None;
    let mut meta_merged = String::new();

    if source == "pixiv" {
        // 1. 本文テキスト
        text = data
            .get("text")
            .or_else(|| data.get("detail").and_then(|d| d.get("text")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // 2. 作者が編集できる主要メタデータを抽出
        let title = data
            .get("title")
            .or_else(|| data.get("detail").and_then(|d| d.get("title")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let caption = data
            .get("caption")
            .or_else(|| data.get("detail").and_then(|d| d.get("caption")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let cover_url = data
            .get("coverUrl")
            .or_else(|| data.get("detail").and_then(|d| d.get("coverUrl")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tags_str = data
            .get("tags")
            .or_else(|| data.get("detail").and_then(|d| d.get("tags")))
            .map(|t| {
                if let Some(arr) = t.as_array() {
                    arr.iter()
                        .map(|v| v.as_str().unwrap_or(""))
                        .collect::<Vec<&str>>()
                        .join(",")
                } else {
                    t.as_str().unwrap_or("").to_string()
                }
            })
            .unwrap_or_default();

        let series_id = data
            .get("seriesId")
            .or_else(|| data.get("detail").and_then(|d| d.get("seriesId")))
            .map(|v| {
                if v.is_number() {
                    v.as_i64().map(|n| n.to_string()).unwrap_or_default()
                } else {
                    v.as_str().unwrap_or("").to_string()
                }
            })
            .unwrap_or_default();

        // 3. ハッシュ用にすべてを統合
        meta_merged = format!(
            "TITLE:{}\nCAPTION:{}\nCOVER:{}\nTAGS:{}\nSERIES:{}\nBODY:{}",
            title, caption, cover_url, tags_str, series_id, text
        );
    } else if source == "fanbox" {
        if let Some(body) = data.get("body") {
            if let Some(blocks) = body.get("blocks").and_then(|b| b.as_array()) {
                let parts: Vec<String> = blocks
                    .iter()
                    .map(|block| {
                        let block_type = block.get("type").and_then(|t| t.as_str());
                        if block_type == Some("p") || block_type == Some("header") {
                            block
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string()
                        } else {
                            "".to_string()
                        }
                    })
                    .filter(|t| !t.trim().is_empty())
                    .collect();
                text = parts.join("\n\n");
            } else if let Some(txt) = body.get("text").and_then(|t| t.as_str()) {
                text = txt.to_string();
            }
        }
        source_updated_at = data
            .get("updatedDatetime")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    let text_len = text.chars().count() as i64;
    let mut hasher = Sha256::new();
    if source == "pixiv" {
        hasher.update(meta_merged.as_bytes());
    } else {
        if !text.is_empty() {
            hasher.update(text.as_bytes());
        } else {
            hasher.update(serde_json::to_string(data).unwrap_or_default().as_bytes());
        }
    }
    let hash_bytes = hasher.finalize();
    let hash = hash_bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    (hash, text_len, source_updated_at)
}

// ============================================================
// pixiv の軽量な更新確認
// ============================================================

/// 本文を除いたメタデータだけの指紋。
///
/// pixiv の作品詳細 API（`fetch_pixiv_novel_metadata`）は本文を返さないが、
/// ここで使う材料はすべて返す。保存時と更新確認時に同じ関数で作ることで、
/// 「変わっていない作品の本文を取りに行かない」判断ができる。
///
/// **取りこぼし**: 文字数が1字も変わらない修正（誤字の置換、句読点の差し替え
/// など）は、この指紋には現れない。呼び出し側は `last_deep_checked_at` を見て、
/// 一定期間ごとに必ず本文まで突き合わせること。
pub(crate) fn pixiv_meta_signature(
    title: &str,
    caption: &str,
    cover_url: &str,
    tags: &[String],
    series_id: &str,
    text_length: u64,
) -> String {
    use sha2::{Digest, Sha256};
    let merged = format!(
        "TITLE:{}\nCAPTION:{}\nCOVER:{}\nTAGS:{}\nSERIES:{}\nLENGTH:{}",
        title,
        caption,
        cover_url,
        tags.join(","),
        series_id,
        text_length
    );
    let mut hasher = Sha256::new();
    hasher.update(merged.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

/// 保存する JSON から指紋を作る。
///
/// **必ず `detail` を先に見る。** `PixivNovelContent` は最上位にも `cover_url` を
/// 持つが、そちらは webview 側の原寸 URL を優先して選び直したもので、更新確認が
/// 受け取る詳細 API の `image_urls.large` とは別物になる。最上位を先に読むと
/// 指紋が永久に一致せず、短絡が一度も効かない。
///
/// 文字数が読めない形（取得経路によっては持たない）のときは `None` を返す。
/// 指紋が無ければ更新確認は従来どおり本文まで取りに行くだけなので、精度は落ちない。
pub(crate) fn pixiv_meta_signature_from_json(data: &serde_json::Value) -> Option<String> {
    let detail = data.get("detail");
    let field = |key: &str| -> Option<&serde_json::Value> {
        detail
            .and_then(|d| d.get(key))
            .filter(|value| !value.is_null())
            .or_else(|| data.get(key).filter(|value| !value.is_null()))
    };
    let text = |key: &str| -> String {
        field(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };

    let text_length = field("text_length")
        .or_else(|| field("textLength"))
        .and_then(|value| value.as_u64())?;
    let cover_url = {
        let snake = text("cover_url");
        if snake.is_empty() {
            text("coverUrl")
        } else {
            snake
        }
    };
    let series_id = {
        let value = field("series_id").or_else(|| field("seriesId"));
        match value {
            Some(value) if value.is_number() => value.as_i64().unwrap_or_default().to_string(),
            Some(value) => value.as_str().unwrap_or("").to_string(),
            None => String::new(),
        }
    };
    let tags = field("tags")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| {
                            item.get("name")
                                .and_then(|name| name.as_str())
                                .map(str::to_string)
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(pixiv_meta_signature(
        &text("title"),
        &text("caption"),
        &cover_url,
        &tags,
        &series_id,
        text_length,
    ))
}

/// pixiv の取得結果に、作品と呼べる中身があるか。
///
/// webview の応答は「読めなかった」を綺麗な JSON で返してくることがある
/// （閲覧制限・年齢確認・本文だけ落ちた部分障害）。パースが通る以上、
/// エラーとしては上がってこない。
fn pixiv_has_material_content(data: &serde_json::Value) -> bool {
    let has_text = ["text"]
        .iter()
        .filter_map(|key| {
            data.get(*key)
                .or_else(|| data.get("detail").and_then(|detail| detail.get(*key)))
        })
        .any(|value| value.as_str().is_some_and(|text| !text.trim().is_empty()));
    if has_text {
        return true;
    }
    ["illusts", "images"].iter().any(|key| {
        data.get(*key)
            .map(|value| {
                value.as_array().is_some_and(|items| !items.is_empty())
                    || value.as_object().is_some_and(|items| !items.is_empty())
            })
            .unwrap_or(false)
    })
}

/// 取得してきたものに、保存するだけの中身があるか。
///
/// 相手のサーバーは、落ちているときも 200 と整形式の JSON で答える。だから
/// 「通信が成功した」は「作品が取れた」の保証にならない。手元にある版が
/// 中身を持っているのに、取ってきたほうが空なら、それは更新ではなく欠落で、
/// 版を上げれば読めていた作品が読めなくなる。
fn fetched_has_material_content(data: &serde_json::Value, source: &str) -> bool {
    match source {
        "fanbox" => fanbox_has_material_content(data),
        "pixiv" => pixiv_has_material_content(data),
        // 知らない取得元は判断材料が無い。止めるほうに倒すと保存が通らなく
        // なるので、ここは通し、既存の版と衝突しない限り従来どおり扱う。
        _ => true,
    }
}

fn fanbox_has_material_content(data: &serde_json::Value) -> bool {
    let Some(body) = data.get("body").filter(|v| !v.is_null()) else {
        return false;
    };

    if body
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }

    if body
        .get("blocks")
        .and_then(|v| v.as_array())
        .map(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
                    || block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(|t| matches!(t, "image" | "file" | "embed" | "url_embed"))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
    {
        return true;
    }

    ["imageMap", "fileMap", "images", "files"]
        .iter()
        .any(|key| {
            body.get(key)
                .map(|v| {
                    v.as_array().map(|a| !a.is_empty()).unwrap_or(false)
                        || v.as_object().map(|o| !o.is_empty()).unwrap_or(false)
                })
                .unwrap_or(false)
        })
}

pub(crate) fn json_string_at<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(s) = value.get(*key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
        if let Some(s) = value
            .get("detail")
            .and_then(|d| d.get(*key))
            .and_then(|v| v.as_str())
        {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
    }
    None
}

pub(crate) fn json_id_at(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let direct = value.get(*key);
        let nested = value.get("detail").and_then(|d| d.get(*key));
        for candidate in [direct, nested].into_iter().flatten() {
            if let Some(s) = candidate.as_str() {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
            if let Some(n) = candidate.as_u64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

pub(crate) fn extract_series_relation(data: &serde_json::Value) -> Option<(String, String)> {
    let series_id = json_id_at(data, &["seriesId", "series_id"])?;
    let series_title = json_string_at(data, &["seriesTitle", "series_title"])
        .or_else(|| {
            data.get("seriesNavigation")
                .and_then(|v| v.get("seriesTitle"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            data.get("detail")
                .and_then(|d| d.get("seriesNavigation"))
                .and_then(|v| v.get("seriesTitle"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("シリーズ");
    Some((series_id, series_title.to_string()))
}

pub(crate) fn extract_series_content_order(data: &serde_json::Value) -> Option<i64> {
    json_id_at(data, &["contentOrder", "content_order", "order"])
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            first_string_at(
                data,
                &[
                    &["detail", "contentOrder"],
                    &["detail", "content_order"],
                    &["detail", "order"],
                ],
            )
            .and_then(|s| s.parse::<i64>().ok())
        })
}

fn safe_entity_segment(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

/// Resolve a work directory from provider-owned identifiers without allowing
/// an IPC caller to turn either component into an absolute or relative path.
///
/// The character allowlist matches the identifiers currently used by Pixiv
/// and FANBOX while retaining compatibility with synthetic/test identifiers.
/// Canonical containment also rejects a pre-existing junction or symlink that
/// points outside the library storage directory.
fn secure_download_item_dir(
    storage_dir: &Path,
    source: &str,
    source_id: &str,
) -> Result<PathBuf, String> {
    if !matches!(source, "pixiv" | "fanbox") {
        return Err(format!("Unsupported download source: {source}"));
    }
    if source_id.is_empty()
        || source_id.len() > 128
        || !source_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("Invalid source id: expected one safe provider identifier".to_string());
    }

    std::fs::create_dir_all(storage_dir)
        .map_err(|e| format!("Storage directory creation failed: {e}"))?;
    let canonical_storage = storage_dir
        .canonicalize()
        .map_err(|e| format!("Storage path resolution failed: {e}"))?;
    let provider_dir = canonical_storage.join(source);
    if !provider_dir.exists() {
        std::fs::create_dir(&provider_dir)
            .map_err(|e| format!("Provider directory creation failed: {e}"))?;
    }
    let canonical_provider = provider_dir
        .canonicalize()
        .map_err(|e| format!("Provider path resolution failed: {e}"))?;
    if !canonical_provider.starts_with(&canonical_storage) || !canonical_provider.is_dir() {
        return Err("Access denied: provider path escapes library storage".to_string());
    }

    // Join to the already resolved provider directory. This avoids creating a
    // directory outside storage before detecting a malicious provider junction.
    let candidate = canonical_provider.join(source_id);
    std::fs::create_dir_all(&candidate)
        .map_err(|e| format!("Download directory creation failed: {e}"))?;
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|e| format!("Download path resolution failed: {e}"))?;
    if !canonical_candidate.starts_with(&canonical_storage) || !canonical_candidate.is_dir() {
        return Err("Access denied: download path escapes library storage".to_string());
    }
    Ok(canonical_candidate)
}

fn sha256_json(value: &serde_json::Value) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

fn first_string_at<'a>(value: &'a serde_json::Value, paths: &[&[&str]]) -> Option<&'a str> {
    for path in paths {
        let mut cursor = value;
        let mut ok = true;
        for key in *path {
            if let Some(next) = cursor.get(*key) {
                cursor = next;
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            if let Some(s) = cursor.as_str() {
                if !s.trim().is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

async fn save_person_snapshot_from_download(
    state: &Arc<AppState>,
    data: &serde_json::Value,
    source: &str,
    author_id: &str,
    author_name: &str,
) -> Result<(), String> {
    if author_id.trim().is_empty() || author_name.trim().is_empty() {
        return Ok(());
    }
    let icon_url = first_string_at(
        data,
        &[
            &["detail", "user", "profile_image_url"],
            &["detail", "user", "profile_image_urls", "medium"],
            &["user", "profile_image_urls", "medium"],
            &["user", "iconUrl"],
            &["user", "icon_url"],
        ],
    );
    let existing = state.db.get_person(source, author_id).ok();
    if existing
        .as_ref()
        .and_then(|person| person.content_hash.as_ref())
        .is_some()
        && (existing
            .as_ref()
            .and_then(|person| person.icon_path.as_ref())
            .is_some()
            || icon_url.is_none())
    {
        return Ok(());
    }
    let description = first_string_at(
        data,
        &[&["detail", "user", "comment"], &["user", "comment"]],
    );
    let profile_url = if source == "pixiv" {
        format!("https://www.pixiv.net/users/{}", author_id)
    } else {
        format!("https://{}.fanbox.cc/", author_id)
    };
    let normalized = serde_json::json!({
        "source": source,
        "sourceKey": author_id,
        "displayName": author_name,
        "iconUrl": icon_url,
        "coverUrl": null,
        "description": description,
        "links": [profile_url],
    });
    let hash = sha256_json(&normalized)?;
    let next_version = existing
        .as_ref()
        .map(|person| person.current_version + 1)
        .unwrap_or(1);
    let json_path =
        if existing.as_ref().and_then(|p| p.content_hash.as_deref()) == Some(hash.as_str()) {
            String::new()
        } else {
            let dir = state
                .db
                .storage_dir()
                .parent()
                .unwrap_or_else(|| state.db.storage_dir())
                .join("profiles")
                .join(safe_entity_segment(source))
                .join(safe_entity_segment(author_id))
                .join(format!("v{}", next_version));
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| e.to_string())?;
            let path = dir.join("original.json");
            let content = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| e.to_string())?;
            path.to_string_lossy().to_string()
        };
    let (icon_path, icon_size) = match icon_url {
        Some(url) => {
            match download_person_snapshot_icon(state, source, author_id, next_version, url).await {
                Ok(value) => value.map_or((None, 0), |(path, size)| (Some(path), size)),
                Err(error) => {
                    log::warn!(
                        "Failed to cache profile icon for {}:{}: {}",
                        source,
                        author_id,
                        error
                    );
                    (
                        existing
                            .as_ref()
                            .and_then(|person| person.icon_path.clone()),
                        0,
                    )
                }
            }
        }
        None => (
            existing
                .as_ref()
                .and_then(|person| person.icon_path.clone()),
            0,
        ),
    };
    let links_json = serde_json::to_string(&normalized["links"]).ok();
    state.db.upsert_person_profile(
        source,
        author_id,
        author_name,
        icon_path.as_deref(),
        None,
        description,
        links_json.as_deref(),
        &hash,
        &json_path,
        i64::from(icon_path.is_some()),
        icon_size,
        EntityProfileFreshness::SnapshotOnly,
    )?;
    Ok(())
}

async fn save_series_snapshot_from_download(
    state: &Arc<AppState>,
    data: &serde_json::Value,
    source: &str,
    local_cover_path: Option<&str>,
) -> Result<(), String> {
    let Some((series_id, series_title)) = extract_series_relation(data) else {
        return Ok(());
    };
    if state
        .db
        .get_series(source, &series_id)
        .ok()
        .and_then(|s| s.content_hash)
        .is_some()
    {
        return Ok(());
    }
    let normalized = serde_json::json!({
        "source": source,
        "sourceKey": series_id,
        "title": series_title,
        "description": null,
        "coverUrl": first_string_at(data, &[&["coverUrl"], &["cover_url"], &["detail", "coverUrl"], &["detail", "cover_url"]]),
        "seriesNavigation": data.get("seriesNavigation").or_else(|| data.get("detail").and_then(|d| d.get("seriesNavigation"))),
    });
    let hash = sha256_json(&normalized)?;
    let existing = state.db.get_series(source, &series_id).ok();
    let json_path =
        if existing.as_ref().and_then(|s| s.content_hash.as_deref()) == Some(hash.as_str()) {
            String::new()
        } else {
            let next_version = existing
                .as_ref()
                .map(|s| s.current_version + 1)
                .unwrap_or(1);
            let dir = state
                .db
                .storage_dir()
                .parent()
                .unwrap_or_else(|| state.db.storage_dir())
                .join("series")
                .join(safe_entity_segment(source))
                .join(safe_entity_segment(&series_id))
                .join(format!("v{}", next_version));
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| e.to_string())?;
            let path = dir.join("original.json");
            let content = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| e.to_string())?;
            path.to_string_lossy().to_string()
        };
    state.db.upsert_series_profile(
        source,
        &series_id,
        &series_title,
        None,
        local_cover_path,
        &hash,
        &json_path,
        0,
        0,
        EntityProfileFreshness::SnapshotOnly,
        // 保存のついでに作る控えなので、取得元へ聞きには行かない。完結の
        // 有無と公開話数は「情報を更新」のときに埋まる。
        None,
        None,
    )?;
    Ok(())
}

async fn download_person_snapshot_icon(
    state: &Arc<AppState>,
    source: &str,
    author_id: &str,
    version: i64,
    url: &str,
) -> Result<Option<(String, i64)>, String> {
    if url.trim().is_empty() {
        return Ok(None);
    }
    let ext = crate::downloader::asset_downloader::extract_extension(url, "jpg");
    let dir = state
        .db
        .storage_dir()
        .parent()
        .unwrap_or_else(|| state.db.storage_dir())
        .join("profiles")
        .join(safe_entity_segment(source))
        .join(safe_entity_segment(author_id))
        .join(format!("v{}", version))
        .join("assets");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| error.to_string())?;
    let path = dir.join(format!("icon.{}", ext));
    if crate::downloader::asset_downloader::asset_path_is_valid_image(
        &path,
        crate::downloader::asset_downloader::MAX_PROFILE_IMAGE_BYTES,
    )
    .await
    {
        let bytes = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|error| error.to_string())?
            .len();
        return Ok(Some((path.to_string_lossy().to_string(), bytes as i64)));
    }
    if let Ok(metadata) = tokio::fs::symlink_metadata(&path).await {
        if metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|error| format!("Failed to remove invalid profile image: {error}"))?;
        } else {
            return Err("Profile image destination is not a regular file".to_string());
        }
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Profile image client creation failed: {error}"))?;
    let response = client
        .get(url)
        .header(
            "referer",
            if source == "fanbox" {
                "https://www.fanbox.cc/"
            } else {
                "https://www.pixiv.net/"
            },
        )
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let bytes = crate::downloader::asset_downloader::save_response_atomically(
        response,
        &path,
        crate::downloader::asset_downloader::MAX_PROFILE_IMAGE_BYTES,
        true,
    )
    .await?;
    Ok(Some((path.to_string_lossy().to_string(), bytes as i64)))
}

async fn sync_download_entities(
    state: &Arc<AppState>,
    data: &serde_json::Value,
    download_id: i64,
    source: &str,
    author_id: &str,
    author_name: &str,
    cover_path: Option<&str>,
) {
    if let Err(e) =
        state
            .db
            .upsert_download_relation(download_id, "author", source, author_id, author_name)
    {
        log::warn!("Failed to register author relation: {}", e);
    }
    if let Err(e) = state.db.upsert_download_person(
        download_id,
        source,
        author_id,
        if source == "fanbox" {
            "creator"
        } else {
            "author"
        },
        author_name,
    ) {
        log::warn!("Failed to register person relation: {}", e);
    }
    if let Err(e) =
        save_person_snapshot_from_download(state, data, source, author_id, author_name).await
    {
        log::warn!("Failed to save person snapshot: {}", e);
    }
    if source == "pixiv" {
        if let Some((series_id, series_title)) = extract_series_relation(data) {
            if let Err(e) = state.db.upsert_download_relation(
                download_id,
                "series",
                source,
                &series_id,
                &series_title,
            ) {
                log::warn!("Failed to register series relation: {}", e);
            }
            if let Err(e) = state.db.upsert_download_series(
                download_id,
                source,
                &series_id,
                &series_title,
                extract_series_content_order(data),
            ) {
                log::warn!("Failed to register normalized series relation: {}", e);
            }
        }
        if let Err(e) = save_series_snapshot_from_download(state, data, source, cover_path).await {
            log::warn!("Failed to save series snapshot: {}", e);
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn download_and_save(
    app: tauri::AppHandle,
    data: serde_json::Value,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    content_type: String,
    tags: Option<Vec<String>>,
    excerpt: Option<String>,
    source_created_at: Option<String>,
    cookie: Option<String>,
    user_agent: Option<String>,
) -> Result<DownloadEntry, String> {
    let state = app.state::<Arc<AppState>>();
    let storage = state.db.storage_dir().to_path_buf();
    // 同じ作品を二重に保存させないのはこちらの錠。関数の最後まで持ち続ける。
    let work_lock = work_save_mutex(&source, &source_id);
    let _work_guard = work_lock.lock().await;
    let root_item_dir = secure_download_item_dir(&storage, &source, &source_id)?;

    // 1. ハッシュ値と文字数を算出
    let (new_hash, new_text_len, new_source_updated) = compute_content_details(&data, &source);

    // 2. 既存チェックとバージョン番号の特定
    //
    // ライブラリ全体の錠は、DBに触る間だけ握る。取得元からアセットを落として
    // いる数分間ずっと握っていたころは、その裏で★を押しただけの操作まで
    // 待たされていた - 待たされている理由が画面のどこにも出ないまま。
    let existing_dl = state.db.get_download_by_source(&source, &source_id)?;
    let mut next_version = 1i64;
    let mut is_update = false;

    // 初めて保存するものが空なら、そもそもライブラリに入れない。
    //
    // FANBOX は支援プランが足りない投稿に 200 と `body: null` を返し、pixiv も
    // 閲覧制限のかかった作品を本文だけ欠けた形で返す。どちらも通信としては
    // 成功しているので、止めるならここしかない。題名だけの殻を並べておくと、
    // 開くまで読めないことに気付けず、更新確認の対象としても数え続ける。
    if existing_dl.is_none() && !fetched_has_material_content(&data, &source) {
        return Err(if source == "fanbox" {
            "この投稿の本文を取得できませんでした。支援プランが足りないか、非公開の可能性があります".to_string()
        } else {
            "この作品の本文を取得できませんでした。閲覧制限がかかっている可能性があります".to_string()
        });
    }

    if let Some(ref dl) = existing_dl {
        let _library_write_guard = state.library_gate.write().await;
        // 中身が消えて見えるときは、版を上げずに手元の版を守る。作者・
        // シリーズの結び付けと索引だけは取り直す - そちらは今回の応答でも
        // 確かに読めた部分なので、更新して困らない。
        if (dl.text_length > 0 || dl.asset_count > 0)
            && !fetched_has_material_content(&data, &source)
        {
            sync_download_entities(
                &state,
                &data,
                dl.id,
                &source,
                &dl.author_id,
                &dl.author_name,
                dl.cover_path.as_deref(),
            )
            .await;
            if let Err(e) = state.db.reindex_download(dl.id) {
                log::warn!("Failed to refresh search index for {}: {}", dl.id, e);
            }
            log::warn!(
                "Skipping version update for {source}:{source_id} because the fetched payload has no material body/assets"
            );
            return state.db.get_download(dl.id);
        }

        let hash_changed = dl.content_hash.as_deref() != Some(&new_hash);
        let updated_changed = if source == "fanbox" {
            dl.source_updated_at != new_source_updated
        } else {
            false
        };

        if hash_changed || updated_changed {
            is_update = true;
            next_version = dl.current_version + 1;
        } else {
            sync_download_entities(
                &state,
                &data,
                dl.id,
                &source,
                &dl.author_id,
                &dl.author_name,
                dl.cover_path.as_deref(),
            )
            .await;
            if let Err(e) = state.db.reindex_download(dl.id) {
                log::warn!("Failed to refresh search index for {}: {}", dl.id, e);
            }
            // 中身は変わっていなかったが、本文まで見たのは確か。次の確認を
            // 軽く済ませられるよう、指紋と時刻を取り直しておく。
            record_pixiv_meta_state(&state, dl.id, &source, &data);
            return state.db.get_download(dl.id);
        }
    }

    let mut legacy_v1_version = None;
    // Legacy files are left in place until the new version commits. Moving
    // them before the transaction made a failed update non-retryable.
    if existing_dl.is_some() && is_update && next_version == 2 && !root_item_dir.join("v1").exists()
    {
        if let Some(ref dl) = existing_dl {
            let legacy_path = root_item_dir.join("original.json");
            let legacy_path = if legacy_path.exists() {
                legacy_path
            } else {
                root_item_dir.join("data.json")
            };
            if legacy_path.exists() && state.db.get_versions(dl.id)?.iter().all(|v| v.version != 1)
            {
                legacy_v1_version = Some(NewVersion {
                    download_id: dl.id,
                    version: 1,
                    content_hash: dl.content_hash.clone(),
                    text_length: dl.text_length,
                    json_path: legacy_path.to_string_lossy().to_string(),
                    original_json_path: Some(legacy_path.to_string_lossy().to_string()),
                    asset_count: dl.asset_count,
                    file_size_bytes: dl.file_size_bytes,
                    created_at: dl.downloaded_at.clone(),
                    change_summary: Some("初期バージョン (旧形式)".to_string()),
                });
            }
        }
    }

    let is_fanbox = source == "fanbox";
    let version_stage = VersionStage::create(&root_item_dir, next_version)?;
    let item_dir = version_stage.staging_path.clone();

    // 1. オリジナルJSON保存
    let original_json_path = item_dir.join("original.json");
    let original_content = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    tokio::fs::write(&original_json_path, &original_content)
        .await
        .map_err(|e| e.to_string())?;

    // 2. アセットダウンロード + ローカルパスインジェクション (インメモリで実行するため、パスインジェクション結果は disk に保存せず DB登録やアセット情報収集のためにのみ一時的に使用)
    let mut modified_data = data.clone();
    let mut source_targets = Vec::new();
    crate::downloader::asset_downloader::extract_download_targets(
        &data,
        is_fanbox,
        &mut source_targets,
    );
    let asset_origins = source_targets
        .into_iter()
        .map(|target| {
            let asset_type = match target.sub_folder {
                "illustrations" => "illustration",
                "cover" => "cover",
                _ => "file",
            };
            ((asset_type.to_string(), target.filename), target.url)
        })
        .collect::<std::collections::HashMap<_, _>>();
    // Assetsフォルダ名は "data_assets" にするため、dummyとして data.json のパスを指定します
    let dummy_json_path = item_dir.join("data.json");

    crate::downloader::asset_downloader::download_and_link_assets(
        &app,
        &mut modified_data,
        &dummy_json_path,
        is_fanbox,
        cookie,
        user_agent,
    )
    .await?;

    // 3. アセットの情報を収集
    let assets_dir = item_dir.join("data_assets");

    // プレミアム最適化：激重な再帰ディレクトリ走査とサイズ計算を非同期ワーカースレッドへ完璧に移譲
    let (mut asset_entries, total_size, cover_path_str) = if assets_dir.exists() {
        let assets_dir_clone = assets_dir.clone();
        let initial_size = original_content.len() as i64;

        tokio::task::spawn_blocking(move || {
            let mut entries = Vec::new();
            let mut size = initial_size;
            let mut cover = None;
            collect_assets_recursive(&assets_dir_clone, &mut entries, &mut size, &mut cover, 0)?;
            Ok::<(Vec<NewAsset>, i64, Option<String>), String>((entries, size, cover))
        })
        .await
        .map_err(|e| format!("Asset collect thread panicked: {}", e))??
    } else {
        (Vec::new(), original_content.len() as i64, None)
    };
    for asset in &mut asset_entries {
        asset.original_url = asset_origins
            .get(&(asset.asset_type.clone(), asset.filename.clone()))
            .cloned();
    }

    let json_path = version_stage.final_path.join("original.json");

    // 5. DB登録
    let new_dl = NewDownload {
        source: source.clone(),
        source_id: source_id.clone(),
        title,
        author_name,
        author_id,
        content_type,
        tags: tags.unwrap_or_default(),
        excerpt,
        cover_path: cover_path_str
            .as_deref()
            .map(|path| version_stage.final_path_for(Path::new(path)))
            .transpose()?
            .map(|path| path.to_string_lossy().to_string()),
        json_path: json_path.to_string_lossy().to_string(),
        original_json_path: Some(json_path.to_string_lossy().to_string()),
        asset_count: asset_entries.len() as i64,
        file_size_bytes: total_size,
        downloaded_at: chrono::Utc::now().to_rfc3339(),
        source_created_at,
        content_hash: Some(new_hash.clone()),
        text_length: new_text_len,
        source_updated_at: new_source_updated.clone(),
        watch_updates: existing_dl
            .as_ref()
            .map(|dl| dl.watch_updates)
            .unwrap_or(false),
        current_version: next_version,
        favorite: existing_dl.as_ref().map(|dl| dl.favorite).unwrap_or(false),
    };

    for asset in &mut asset_entries {
        asset.local_path = version_stage
            .final_path_for(Path::new(&asset.local_path))?
            .to_string_lossy()
            .to_string();
    }

    let mut versions_to_insert = legacy_v1_version.into_iter().collect::<Vec<_>>();
    versions_to_insert.push(NewVersion {
        download_id: existing_dl.as_ref().map(|dl| dl.id).unwrap_or(0),
        version: next_version,
        content_hash: Some(new_hash),
        text_length: new_text_len,
        json_path: json_path.to_string_lossy().to_string(),
        original_json_path: Some(json_path.to_string_lossy().to_string()),
        asset_count: new_dl.asset_count,
        file_size_bytes: new_dl.file_size_bytes,
        created_at: new_dl.downloaded_at.clone(),
        change_summary: Some(if is_update {
            format!("本文・コンテンツの更新 (v{})", next_version)
        } else {
            format!("新規ダウンロード (v{})", next_version)
        }),
    });

    // ここから先はDBに触る。錠を取り直し、手放していた間にこの作品が
    // 動いていないことを確かめてから確定する。消された作品の上に版だけを
    // 積むくらいなら、保存しなかったことにして呼び出し側にやり直させる。
    let _library_write_guard = state.library_gate.write().await;
    let current_dl = state.db.get_download_by_source(&source, &source_id)?;
    let unchanged = match (&existing_dl, &current_dl) {
        (None, None) => true,
        (Some(before), Some(now)) => {
            before.id == now.id && before.current_version == now.current_version
        }
        _ => false,
    };
    if !unchanged {
        return Err(
            "保存している間にこの作品が変更されました。保存を取りやめたので、もう一度お試しください"
                .to_string(),
        );
    }

    let dl_id = commit_journaled_version(
        &state.db,
        &source,
        &source_id,
        next_version,
        version_stage,
        |journal_id| {
            state.db.commit_download_save_with_journal(
                &new_dl,
                &asset_entries,
                &versions_to_insert,
                journal_id,
            )
        },
    )
    .await?;

    sync_download_entities(
        &state,
        &data,
        dl_id,
        &source,
        &new_dl.author_id,
        &new_dl.author_name,
        new_dl.cover_path.as_deref(),
    )
    .await;

    if let Err(e) = state.db.reindex_download(dl_id) {
        log::warn!("Failed to refresh search index for {}: {}", dl_id, e);
    }

    record_pixiv_meta_state(&state, dl_id, &source, &data);

    state.db.get_download(dl_id)
}

/// 本文まで取り込んだ直後の覚え書きを残す（pixiv のみ）。
///
/// 失敗しても保存そのものは成功しているので、記録できないことは警告に留める。
/// 次の更新確認が本文まで取りに行くだけで、結果は変わらない。
fn record_pixiv_meta_state(
    state: &AppState,
    download_id: i64,
    source: &str,
    data: &serde_json::Value,
) {
    if source != "pixiv" {
        return;
    }
    let signature = pixiv_meta_signature_from_json(data);
    if let Err(error) = state.db.set_download_meta_state(
        download_id,
        signature.as_deref(),
        &chrono::Utc::now().to_rfc3339(),
    ) {
        log::warn!("メタデータ指紋を保存できません ({download_id}): {error}");
    }
}

pub(crate) fn collect_assets_recursive(
    dir: &std::path::Path,
    assets: &mut Vec<NewAsset>,
    total_size: &mut i64,
    cover_path: &mut Option<String>,
    depth: u32,
) -> Result<(), String> {
    collect_assets_recursive_with_limits(
        dir,
        assets,
        total_size,
        cover_path,
        depth,
        AssetScanLimits {
            max_depth: 16,
            max_files: 20_000,
        },
    )
}

#[derive(Clone, Copy)]
struct AssetScanLimits {
    max_depth: u32,
    max_files: usize,
}

fn collect_assets_recursive_with_limits(
    dir: &Path,
    assets: &mut Vec<NewAsset>,
    total_size: &mut i64,
    cover_path: &mut Option<String>,
    depth: u32,
    limits: AssetScanLimits,
) -> Result<(), String> {
    let root = dir
        .canonicalize()
        .map_err(|error| format!("Failed to resolve asset root: {error}"))?;
    let mut visited = std::collections::HashSet::new();
    let mut scanned_files = 0usize;
    collect_assets_recursive_inner(
        dir,
        &root,
        assets,
        total_size,
        cover_path,
        depth,
        limits,
        &mut scanned_files,
        &mut visited,
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_assets_recursive_inner(
    dir: &Path,
    root: &Path,
    assets: &mut Vec<NewAsset>,
    total_size: &mut i64,
    cover_path: &mut Option<String>,
    depth: u32,
    limits: AssetScanLimits,
    scanned_files: &mut usize,
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<(), String> {
    if depth > limits.max_depth {
        return Err(format!(
            "Asset directory nesting exceeds {} levels",
            limits.max_depth
        ));
    }
    let metadata = std::fs::symlink_metadata(dir)
        .map_err(|error| format!("Failed to inspect asset directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("Asset directory is a link or not a directory".to_string());
    }
    let canonical_dir = dir
        .canonicalize()
        .map_err(|error| format!("Failed to resolve asset directory: {error}"))?;
    if !canonical_dir.starts_with(root) {
        return Err("Asset directory escapes its version root".to_string());
    }
    if !visited.insert(canonical_dir) {
        return Err("Asset directory contains a filesystem cycle".to_string());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("Failed to inspect asset entry: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Asset scan refuses filesystem links: {}",
                path.display()
            ));
        }
        if metadata.file_type().is_dir() {
            collect_assets_recursive_inner(
                &path,
                root,
                assets,
                total_size,
                cover_path,
                depth + 1,
                limits,
                scanned_files,
                visited,
            )?;
        } else if metadata.file_type().is_file() {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if is_unpublished_part_file(&path) {
                continue;
            }
            *scanned_files += 1;
            if *scanned_files > limits.max_files {
                return Err(format!(
                    "Asset file count exceeds the {} file limit",
                    limits.max_files
                ));
            }
            let size = i64::try_from(metadata.len())
                .map_err(|_| "Asset file is too large to index".to_string())?;
            *total_size = total_size
                .checked_add(size)
                .ok_or_else(|| "Total asset size overflow".to_string())?;

            let parent_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");

            let asset_type = if parent_name == "cover" {
                if cover_path.is_none() {
                    *cover_path = Some(path.to_string_lossy().to_string());
                }
                "cover"
            } else if parent_name == "illustrations" {
                "illustration"
            } else {
                "file"
            };

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let mime = match ext {
                "jpg" | "jpeg" => Some("image/jpeg".to_string()),
                "png" => Some("image/png".to_string()),
                "gif" => Some("image/gif".to_string()),
                "webp" => Some("image/webp".to_string()),
                "pdf" => Some("application/pdf".to_string()),
                _ => None,
            };

            assets.push(NewAsset {
                download_id: 0,
                asset_type: asset_type.to_string(),
                filename,
                local_path: path.to_string_lossy().to_string(),
                original_url: None,
                mime_type: mime,
                file_size_bytes: size,
            });
        } else {
            return Err(format!(
                "Asset scan refuses special filesystem entries: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod path_security_tests {
    use super::*;

    fn temp_storage() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("piep_download_path_test_{}", rand::random::<u64>()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn download_path_accepts_known_sources_and_stays_under_storage() {
        let storage = temp_storage();
        let resolved = secure_download_item_dir(&storage, "pixiv", "123_abc-9").unwrap();
        assert!(resolved.starts_with(storage.canonicalize().unwrap()));
        assert!(resolved.ends_with(Path::new("pixiv").join("123_abc-9")));
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn download_path_rejects_traversal_absolute_and_unknown_sources() {
        let storage = temp_storage();
        for (source, source_id) in [
            ("../outside", "123"),
            ("pixiv", "../outside"),
            ("pixiv", "..\\outside"),
            ("pixiv", "/outside"),
            ("pixiv", "C:\\outside"),
            ("unknown", "123"),
            ("fanbox", ""),
        ] {
            assert!(
                secure_download_item_dir(&storage, source, source_id).is_err(),
                "accepted unsafe source={source:?}, source_id={source_id:?}"
            );
        }
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn asset_scan_enforces_depth_and_file_count_limits() {
        let root = temp_storage();
        let deep = root.join("one").join("two");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("asset.bin"), b"asset").unwrap();
        let mut assets = Vec::new();
        let mut total_size = 0;
        let mut cover = None;
        let depth_error = collect_assets_recursive_with_limits(
            &root,
            &mut assets,
            &mut total_size,
            &mut cover,
            0,
            AssetScanLimits {
                max_depth: 1,
                max_files: 10,
            },
        )
        .unwrap_err();
        assert!(depth_error.contains("nesting"));

        let flat = temp_storage();
        for index in 0..3 {
            std::fs::write(flat.join(format!("asset-{index}.bin")), b"asset").unwrap();
        }
        let mut assets = Vec::new();
        let mut total_size = 0;
        let mut cover = None;
        let count_error = collect_assets_recursive_with_limits(
            &flat,
            &mut assets,
            &mut total_size,
            &mut cover,
            0,
            AssetScanLimits {
                max_depth: 1,
                max_files: 2,
            },
        )
        .unwrap_err();
        assert!(count_error.contains("file count"));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(flat);
    }

    #[test]
    fn asset_scan_ignores_unpublished_part_files() {
        let root = temp_storage();
        std::fs::write(root.join("asset.bin"), b"complete").unwrap();
        std::fs::write(root.join(".asset.bin.123.part"), b"partial").unwrap();
        let mut assets = Vec::new();
        let mut total_size = 0;
        let mut cover = None;

        collect_assets_recursive(&root, &mut assets, &mut total_size, &mut cover, 0).unwrap();

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].filename, "asset.bin");
        assert_eq!(total_size, 8);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn asset_scan_refuses_directory_links_when_supported() {
        let root = temp_storage();
        let outside = temp_storage();
        std::fs::write(outside.join("outside.bin"), b"outside").unwrap();
        let link = root.join("linked");
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
            let mut assets = Vec::new();
            let mut total_size = 0;
            let mut cover = None;
            let error =
                collect_assets_recursive(&root, &mut assets, &mut total_size, &mut cover, 0)
                    .unwrap_err();
            assert!(error.contains("links"));
        }
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn same_work_save_lock_serializes_two_concurrent_callers() {
        let first_lock = work_save_mutex("pixiv", "concurrent-work");
        let first_guard = first_lock.lock().await;
        let acquired_second = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acquired_second_task = acquired_second.clone();
        let second = tokio::spawn(async move {
            let second_lock = work_save_mutex("pixiv", "concurrent-work");
            let _second_guard = second_lock.lock().await;
            acquired_second_task.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        tokio::task::yield_now().await;
        assert!(!acquired_second.load(std::sync::atomic::Ordering::SeqCst));
        drop(first_guard);
        second.await.unwrap();
        assert!(acquired_second.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_concurrent_save_pipelines_publish_distinct_versions() {
        let root = temp_storage();
        let storage = root.join("downloads");
        let db =
            Arc::new(crate::database::Database::open(&root.join("piep.db"), &storage).unwrap());
        let save = |db: Arc<crate::database::Database>, marker: &'static str| {
            let storage = storage.clone();
            async move {
                let work_lock = work_save_mutex("pixiv", "parallel-save");
                let _guard = work_lock.lock().await;
                let work_root = secure_download_item_dir(&storage, "pixiv", "parallel-save")?;
                let existing = db.get_download_by_source("pixiv", "parallel-save")?;
                let version = existing
                    .as_ref()
                    .map(|download| download.current_version + 1)
                    .unwrap_or(1);
                let stage = VersionStage::create(&work_root, version)?;
                std::fs::write(stage.staging_path.join("original.json"), marker)
                    .map_err(|error| error.to_string())?;
                let json_path = stage.final_path.join("original.json");
                let download = NewDownload {
                    source: "pixiv".to_string(),
                    source_id: "parallel-save".to_string(),
                    title: marker.to_string(),
                    author_name: "Author".to_string(),
                    author_id: "author".to_string(),
                    content_type: "novel".to_string(),
                    tags: Vec::new(),
                    excerpt: None,
                    cover_path: None,
                    json_path: json_path.to_string_lossy().to_string(),
                    original_json_path: Some(json_path.to_string_lossy().to_string()),
                    asset_count: 0,
                    file_size_bytes: marker.len() as i64,
                    downloaded_at: chrono::Utc::now().to_rfc3339(),
                    source_created_at: None,
                    content_hash: Some(marker.to_string()),
                    text_length: marker.len() as i64,
                    source_updated_at: None,
                    watch_updates: false,
                    current_version: version,
                    favorite: false,
                };
                let version_row = NewVersion {
                    download_id: existing.as_ref().map(|entry| entry.id).unwrap_or(0),
                    version,
                    content_hash: download.content_hash.clone(),
                    text_length: download.text_length,
                    json_path: download.json_path.clone(),
                    original_json_path: download.original_json_path.clone(),
                    asset_count: 0,
                    file_size_bytes: download.file_size_bytes,
                    created_at: download.downloaded_at.clone(),
                    change_summary: None,
                };
                commit_journaled_version(
                    &db,
                    "pixiv",
                    "parallel-save",
                    version,
                    stage,
                    |journal_id| {
                        db.commit_download_save_with_journal(
                            &download,
                            &[],
                            &[version_row],
                            journal_id,
                        )
                    },
                )
                .await
            }
        };
        let first = tokio::spawn(save(db.clone(), "first"));
        let second = tokio::spawn(save(db.clone(), "second"));

        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();

        let download = db
            .get_download_by_source("pixiv", "parallel-save")
            .unwrap()
            .unwrap();
        assert_eq!(download.current_version, 2);
        assert_eq!(db.get_versions(download.id).unwrap().len(), 2);
        let mut bodies = [
            std::fs::read_to_string(storage.join("pixiv/parallel-save/v1/original.json")).unwrap(),
            std::fs::read_to_string(storage.join("pixiv/parallel-save/v2/original.json")).unwrap(),
        ];
        bodies.sort();
        assert_eq!(bodies, ["first", "second"]);
        drop(db);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn durable_stage_walk_enforces_depth_and_entry_limits() {
        let deep_root = temp_storage();
        let deep_file = deep_root.join("one").join("two").join("asset.bin");
        std::fs::create_dir_all(deep_file.parent().unwrap()).unwrap();
        std::fs::write(&deep_file, b"asset").unwrap();
        let depth_error = durable_sync_stage_tree_with_limits(
            &deep_root,
            StageSyncLimits {
                max_depth: 1,
                max_entries: 20,
            },
        )
        .unwrap_err();
        assert!(depth_error.contains("nesting"));

        let wide_root = temp_storage();
        for index in 0..3 {
            std::fs::write(wide_root.join(format!("asset-{index}.bin")), b"asset").unwrap();
        }
        let count_error = durable_sync_stage_tree_with_limits(
            &wide_root,
            StageSyncLimits {
                max_depth: 1,
                // Root plus only two children are permitted.
                max_entries: 3,
            },
        )
        .unwrap_err();
        assert!(count_error.contains("entry safety limit"));

        let _ = std::fs::remove_dir_all(deep_root);
        let _ = std::fs::remove_dir_all(wide_root);
    }

    #[tokio::test]
    async fn durable_publish_rejects_links_and_preserves_their_targets() {
        let root = temp_storage();
        let work = root.join("work");
        let outside = root.join("outside");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("outside.bin"), b"outside").unwrap();
        let stage = VersionStage::create(&work, 1).unwrap();
        let link = stage.staging_path.join("linked");
        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&outside, &link);
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_dir(&outside, &link);
        #[cfg(not(any(unix, windows)))]
        let link_result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "links unavailable",
        ));

        match link_result {
            Ok(()) => {
                let final_path = stage.final_path.clone();
                let error = stage.publish().await.unwrap_err();
                assert!(error.message.contains("link/reparse"));
                assert!(!final_path.exists());
                assert_eq!(
                    std::fs::read(outside.join("outside.bin")).unwrap(),
                    b"outside"
                );
            }
            Err(_) => drop(stage),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn durable_publish_rejects_and_cleans_unpublished_part_files() {
        let root = temp_storage();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        let stage = VersionStage::create(&work, 1).unwrap();
        let staging_path = stage.staging_path.clone();
        let final_path = stage.final_path.clone();
        std::fs::write(
            stage.staging_path.join(".original.json.test.part"),
            b"partial",
        )
        .unwrap();

        let error = stage.publish().await.unwrap_err();
        assert!(error.message.contains("partial file"));
        assert!(!staging_path.exists());
        assert!(!final_path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_destination_appearing_before_publish_is_never_removed_as_rollback() {
        let root = temp_storage();
        let storage = root.join("downloads");
        let db_path = root.join("piep.db");
        let db = Database::open(&db_path, &storage).unwrap();
        let work = secure_download_item_dir(&storage, "pixiv", "publish-race").unwrap();
        let stage = VersionStage::create(&work, 1).unwrap();
        std::fs::write(stage.staging_path.join("original.json"), b"staged").unwrap();
        std::fs::create_dir(&stage.final_path).unwrap();
        let external_file = stage.final_path.join("external.txt");
        std::fs::write(&external_file, b"must-survive").unwrap();

        let error = commit_journaled_version(&db, "pixiv", "publish-race", 1, stage, |_| {
            panic!("database commit must not run when the destination already exists")
        })
        .await
        .unwrap_err();
        assert!(error.contains("destination appeared"));
        assert_eq!(std::fs::read(&external_file).unwrap(), b"must-survive");

        drop(db);
        let reopened = Database::open(&db_path, &storage).unwrap();
        assert_eq!(std::fs::read(&external_file).unwrap(), b"must-survive");
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn publish_failures_keep_old_database_and_files_and_remove_partial_stage() {
        let root = temp_storage();
        let storage = root.join("downloads");
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let work = secure_download_item_dir(&storage, "pixiv", "durable-failure").unwrap();
        let old_dir = work.join("v1");
        std::fs::create_dir(&old_dir).unwrap();
        let old_json = old_dir.join("original.json");
        std::fs::write(&old_json, b"old-version").unwrap();
        let old_download = NewDownload {
            source: "pixiv".to_string(),
            source_id: "durable-failure".to_string(),
            title: "old-title".to_string(),
            author_name: "Author".to_string(),
            author_id: "author".to_string(),
            content_type: "novel".to_string(),
            tags: Vec::new(),
            excerpt: None,
            cover_path: None,
            json_path: old_json.to_string_lossy().to_string(),
            original_json_path: Some(old_json.to_string_lossy().to_string()),
            asset_count: 0,
            file_size_bytes: 11,
            downloaded_at: chrono::Utc::now().to_rfc3339(),
            source_created_at: None,
            content_hash: Some("old-hash".to_string()),
            text_length: 11,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        };
        let old_id = db.upsert_download(&old_download).unwrap();

        for failure_point in [
            PublishFailurePoint::BeforeTreeSync,
            PublishFailurePoint::AfterTreeSync,
            PublishFailurePoint::AfterRename,
        ] {
            let mut stage = VersionStage::create(&work, 2).unwrap();
            std::fs::write(stage.staging_path.join("original.json"), b"new-version").unwrap();
            let partial_dir = stage.staging_path.join("data_assets");
            std::fs::create_dir(&partial_dir).unwrap();
            let payload_name = if failure_point == PublishFailurePoint::BeforeTreeSync {
                ".asset.bin.test.part"
            } else {
                "asset.bin"
            };
            std::fs::write(partial_dir.join(payload_name), b"payload").unwrap();
            stage.inject_publish_failure(failure_point);
            let commit_called = std::sync::atomic::AtomicBool::new(false);

            let error = commit_journaled_version(&db, "pixiv", "durable-failure", 2, stage, |_| {
                commit_called.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(old_id)
            })
            .await
            .unwrap_err();
            assert!(error.contains("Injected version publish failure"));
            assert!(!commit_called.load(std::sync::atomic::Ordering::SeqCst));

            let unchanged = db.get_download(old_id).unwrap();
            assert_eq!(unchanged.title, "old-title");
            assert_eq!(unchanged.current_version, 1);
            assert_eq!(std::fs::read(&old_json).unwrap(), b"old-version");
            assert!(!work.join("v2").exists());
            assert!(std::fs::read_dir(&work).unwrap().all(|entry| {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                !name.contains(".stage") && !name.ends_with(".part")
            }));
        }
        drop(db);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_write_through_publish_moves_a_synced_complete_tree() {
        use std::os::windows::ffi::OsStrExt;

        let root = temp_storage();
        let mut work = root.join("work");
        for index in 0..6 {
            work = work.join(format!("long-version-parent-{index}-xxxxxxxxxxxxxxxxxxxx"));
        }
        std::fs::create_dir_all(&work).unwrap();
        let stage = VersionStage::create(&work, 1).unwrap();
        let staging_path = stage.staging_path.clone();
        let final_path = stage.final_path.clone();
        assert!(staging_path.as_os_str().encode_wide().count() > 260);
        let nested = stage.staging_path.join("data_assets").join("cover");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(stage.staging_path.join("original.json"), b"json").unwrap();
        std::fs::write(nested.join("cover.bin"), b"cover").unwrap();

        let mut stage = stage.publish().await.unwrap();
        assert!(!staging_path.exists());
        assert_eq!(
            std::fs::read(final_path.join("original.json")).unwrap(),
            b"json"
        );
        assert_eq!(
            std::fs::read(final_path.join("data_assets/cover/cover.bin")).unwrap(),
            b"cover"
        );
        stage.commit();
        drop(stage);
        assert!(final_path.is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn database_failure_removes_the_published_version_directory() {
        let root = temp_storage();
        let storage = root.join("downloads");
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let work = secure_download_item_dir(&storage, "pixiv", "failed-save").unwrap();
        let stage = VersionStage::create(&work, 1).unwrap();
        std::fs::write(stage.staging_path.join("original.json"), b"staged").unwrap();
        let final_path = stage.final_path.clone();

        let error = commit_journaled_version(&db, "pixiv", "failed-save", 1, stage, |_| {
            Err("injected database failure".to_string())
        })
        .await
        .unwrap_err();
        assert!(error.contains("injected"));
        assert!(!final_path.exists());
        assert!(std::fs::read_dir(&work).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("stage")));
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod pixiv_meta_signature_tests {
    use super::*;
    use crate::downloader::pixiv::{PixivNovelDetail, PixivNovelTag, PixivNovelUser};

    fn detail() -> PixivNovelDetail {
        PixivNovelDetail {
            id: 8_842_013,
            title: "夜明けの糸".into(),
            user: PixivNovelUser {
                id: 7,
                name: "青葉しおり".into(),
                account: "aoba_shiori".into(),
                profile_image_url: None,
            },
            tags: ["創作", "ファンタジー"]
                .iter()
                .map(|name| PixivNovelTag {
                    name: (*name).to_string(),
                })
                .collect(),
            caption: "港町の話です".into(),
            create_date: "2026-04-18T21:52:01+09:00".into(),
            text_length: 12_840,
            series_id: Some("778120".into()),
            series_title: Some("星を編む人".into()),
            cover_url: Some("https://i.pximg.net/c/cover.jpg".into()),
        }
    }

    fn signature_of(detail: &PixivNovelDetail) -> String {
        let tags: Vec<String> = detail.tags.iter().map(|tag| tag.name.clone()).collect();
        pixiv_meta_signature(
            &detail.title,
            &detail.caption,
            detail.cover_url.as_deref().unwrap_or(""),
            &tags,
            detail.series_id.as_deref().unwrap_or(""),
            u64::from(detail.text_length),
        )
    }

    /// 実際に保存される形。最上位の `cover_url` は webview 側の原寸を優先して
    /// 選び直したもので、`detail.cover_url`（詳細 API の large）とは別物になる。
    fn saved_json(detail: &PixivNovelDetail, text: &str) -> serde_json::Value {
        serde_json::json!({
            "detail": detail,
            "text": text,
            "cover_url": "https://i.pximg.net/novel-cover-original/img/master.jpg",
            "illusts": {},
            "images": {},
        })
    }

    /// 保存した JSON から作る指紋と、更新確認で受け取る詳細から作る指紋が
    /// 一致すること。ここがずれると短絡が一度も効かない。
    ///
    /// 保存側には表紙 URL が2つある（最上位＝原寸優先で選び直したもの、
    /// detail＝詳細 API の large）。更新確認が受け取るのは後者だけなので、
    /// 指紋も detail から作らなければならない。
    #[test]
    fn the_saved_work_and_the_fetched_detail_agree_on_one_signature() {
        let detail = detail();
        let saved = saved_json(&detail, "本文はここ");
        assert_ne!(
            saved.get("cover_url"),
            saved.get("detail").and_then(|d| d.get("cover_url")),
            "保存形の前提: 表紙 URL は2つあり、中身が違う"
        );
        let stored = pixiv_meta_signature_from_json(&saved).unwrap();
        assert_eq!(stored, signature_of(&detail));
    }

    #[test]
    fn a_changed_title_tag_or_length_shows_up_in_the_signature() {
        let base = signature_of(&detail());

        let mut renamed = detail();
        renamed.title = "夜明けの糸（改稿）".into();
        assert_ne!(signature_of(&renamed), base);

        let mut retagged = detail();
        retagged.tags.push(PixivNovelTag {
            name: "長編".into(),
        });
        assert_ne!(signature_of(&retagged), base);

        let mut longer = detail();
        longer.text_length += 1;
        assert_ne!(signature_of(&longer), base);
    }

    /// 取りこぼしの明文化。文字数の変わらない修正は指紋に現れない。
    /// これを拾うのは DEEP_CHECK_INTERVAL_DAYS ごとの本文突き合わせの役目。
    #[test]
    fn a_same_length_body_edit_is_invisible_to_the_signature() {
        let detail = detail();
        let before = pixiv_meta_signature_from_json(&saved_json(&detail, "港の灯が落ちる")).unwrap();
        let after = pixiv_meta_signature_from_json(&saved_json(&detail, "港の灯が消える")).unwrap();
        assert_eq!(
            before, after,
            "本文だけの差は指紋に出ない — 深い確認で拾う前提"
        );
    }

    /// 文字数を持たない形で保存された作品は指紋を作らない。
    /// 指紋が無ければ更新確認は従来どおり本文まで取りに行く。
    #[test]
    fn a_payload_without_a_length_has_no_signature() {
        let payload = serde_json::json!({ "title": "題", "caption": "", "tags": [] });
        assert!(pixiv_meta_signature_from_json(&payload).is_none());
    }
}

/// 取得元は、落ちているときも 200 と整形式の JSON で答えることがある。
/// その応答で版を上げると、読めていた作品が空の版に置き換わる。ここは
/// 「保存しないほうが正しい」数少ない場面なので、判定を固定しておく。
#[cfg(test)]
mod material_content_tests {
    use super::fetched_has_material_content;
    use serde_json::json;

    #[test]
    fn a_pixiv_novel_with_a_body_is_material() {
        assert!(fetched_has_material_content(
            &json!({ "detail": { "title": "題" }, "text": "港の灯が落ちる" }),
            "pixiv",
        ));
        assert!(fetched_has_material_content(
            &json!({ "detail": { "title": "題", "text": "本文" } }),
            "pixiv",
        ));
    }

    /// 挿絵だけの作品もある。本文が空でも中身が無いとは限らない。
    #[test]
    fn a_pixiv_novel_carrying_only_illustrations_is_material() {
        assert!(fetched_has_material_content(
            &json!({ "text": "", "illusts": { "12": { "id": 12 } } }),
            "pixiv",
        ));
    }

    /// 閲覧制限や部分障害は、本文だけが空の綺麗な JSON として返ってくる。
    #[test]
    fn an_empty_pixiv_payload_is_not_material() {
        for payload in [
            json!({ "detail": { "title": "題" }, "text": "" }),
            json!({ "detail": { "title": "題" }, "text": "   
  " }),
            json!({ "detail": { "title": "題" }, "text": "", "illusts": {}, "images": [] }),
            json!({ "detail": { "title": "題" } }),
        ] {
            assert!(
                !fetched_has_material_content(&payload, "pixiv"),
                "{payload}"
            );
        }
    }

    #[test]
    fn fanbox_posts_keep_their_own_judgement() {
        assert!(fetched_has_material_content(
            &json!({ "body": { "text": "制作ノート" } }),
            "fanbox",
        ));
        assert!(!fetched_has_material_content(
            &json!({ "body": null }),
            "fanbox",
        ));
        assert!(!fetched_has_material_content(&json!({}), "fanbox"));
    }
}
