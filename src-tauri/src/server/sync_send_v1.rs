//! 旧 API の本棚の送信（試験だけ）。単独サーバーの旧 API の受け取りを試すために残す。
//!
//! デスクトップ版はもう旧棚を持たないので、この形では送らない。旧 API を消すときに一緒に消す。

use super::snapshot::{prepare_for_send, PortableReport, FILE_ROOTS};
use super::sync_receive::{
    BindEntry, BlobState, CommitOutcome, CommitRequest, ManifestEntry, OpenedSession,
};
use super::sync_send::{
    build_client, check_status, net_error, normalized_base, RemoteServer, SendHandle,
};
use super::{CHUNK_BYTES, MANIFEST_BATCH};
use crate::database::Database;
use futures_util::{stream, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

const PARALLEL_UPLOADS: usize = 4;
const CHUNK_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct LocalFile {
    pub relative: String,
    pub absolute: PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
}

/// 置き場を歩いてファイルの一覧を作る。名前がパスとして送れないものは数だけ返す。
pub fn scan_library_files(app_data: &Path) -> Result<(Vec<LocalFile>, i64), String> {
    let mut files = Vec::new();
    let mut skipped = 0_i64;
    for root in FILE_ROOTS {
        let mut start = app_data.to_path_buf();
        start.extend(root.split('/'));
        if !start.exists() {
            continue;
        }
        let mut stack = vec![(start, (*root).to_string())];
        while let Some((dir, relative)) = stack.pop() {
            let entries = fs::read_dir(&dir)
                .map_err(|error| format!("本棚のフォルダーを読めません: {error}"))?;
            for entry in entries {
                let entry =
                    entry.map_err(|error| format!("本棚のフォルダーを読めません: {error}"))?;
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    skipped += 1;
                    continue;
                };
                let child = format!("{relative}/{name}");
                let kind = entry
                    .file_type()
                    .map_err(|error| format!("本棚のファイルを読めません: {error}"))?;
                if kind.is_dir() {
                    stack.push((entry.path(), child));
                    continue;
                }
                if !kind.is_file() || name.ends_with(".tmp") || name.ends_with(".part") {
                    continue;
                }
                if super::snapshot::validate_relative(&child).is_err() {
                    skipped += 1;
                    continue;
                }
                let meta = entry
                    .metadata()
                    .map_err(|error| format!("本棚のファイルを読めません: {error}"))?;
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |at| at.as_millis() as i64);
                files.push(LocalFile {
                    relative: child,
                    absolute: entry.path(),
                    size: meta.len(),
                    mtime_ms,
                });
            }
        }
    }
    Ok((files, skipped))
}

pub(super) fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("送るファイルを読めません: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("送るファイルを読めません: {error}"))?;
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

struct Uploader {
    client: reqwest::Client,
    base: String,
    token: String,
    session: String,
    handle: SendHandle,
    /// 同じ中身のファイルを同時に上げない。後から来た方は先の完了を待つ。
    blobs: Mutex<HashMap<String, Arc<tokio::sync::OnceCell<()>>>>,
}

impl Uploader {
    fn blob_url(&self, sha: &str) -> String {
        format!(
            "{}/api/v1/admin/sync/sessions/{}/blobs/{sha}",
            self.base, self.session
        )
    }

    async fn upload_blob(&self, path: &Path, sha: &str, total: u64) -> Result<(), String> {
        let state: BlobState = check_status(
            self.client
                .get(self.blob_url(sha))
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(net_error)?,
        )
        .await?
        .json()
        .await
        .map_err(net_error)?;
        if state.complete {
            self.handle.update(|progress| progress.bytes_done += total);
            return Ok(());
        }
        let mut offset = state.received;
        self.handle.update(|progress| progress.bytes_done += offset);
        loop {
            self.handle.check_cancel()?;
            let chunk = read_chunk(path, offset).await?;
            let state = self.put_chunk(sha, offset, total, chunk).await?;
            if state.complete {
                self.handle
                    .update(|progress| progress.bytes_done += total.saturating_sub(offset));
                return Ok(());
            }
            if state.received == offset {
                return Err("サーバーがファイルを受け取れませんでした".into());
            }
            // 受け取った長さが食い違っていれば、サーバーの言う位置から続ける。
            self.handle
                .update(|progress| progress.bytes_done += state.received.saturating_sub(offset));
            offset = state.received;
        }
    }

    async fn put_chunk(
        &self,
        sha: &str,
        offset: u64,
        total: u64,
        chunk: Vec<u8>,
    ) -> Result<BlobState, String> {
        let mut last_error = String::new();
        for attempt in 0..CHUNK_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
            let sent = self
                .client
                .put(self.blob_url(sha))
                .query(&[("offset", offset), ("total", total)])
                .bearer_auth(&self.token)
                .body(chunk.clone())
                .send()
                .await;
            match sent {
                Ok(response) if response.status().is_server_error() => {
                    last_error = format!("サーバーが処理できませんでした（{}）", response.status());
                }
                Ok(response) => {
                    return check_status(response)
                        .await?
                        .json()
                        .await
                        .map_err(net_error)
                }
                Err(error) => last_error = net_error(error),
            }
        }
        Err(last_error)
    }

    async fn upload_file(&self, file: &LocalFile) -> Result<BindEntry, String> {
        self.handle.check_cancel()?;
        let absolute = file.absolute.clone();
        let sha = tokio::task::spawn_blocking(move || file_sha256(&absolute))
            .await
            .map_err(|_| "ファイルのハッシュを計算できません".to_string())??;
        let once = {
            let mut blobs = self
                .blobs
                .lock()
                .map_err(|_| "送信の状態を読めません".to_string())?;
            Arc::clone(blobs.entry(sha.clone()).or_default())
        };
        let mut uploaded_here = false;
        once.get_or_try_init(|| {
            uploaded_here = true;
            self.upload_blob(&file.absolute, &sha, file.size)
        })
        .await?;
        if !uploaded_here {
            self.handle
                .update(|progress| progress.bytes_done += file.size);
        }
        self.handle.update(|progress| progress.files_done += 1);
        Ok(BindEntry {
            path: file.relative.clone(),
            sha256: sha,
        })
    }
}

async fn read_chunk(path: &Path, offset: u64) -> Result<Vec<u8>, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut file =
            File::open(&path).map_err(|error| format!("送るファイルを読めません: {error}"))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("送るファイルを読めません: {error}"))?;
        let mut buffer = Vec::with_capacity(CHUNK_BYTES);
        file.take(CHUNK_BYTES as u64)
            .read_to_end(&mut buffer)
            .map_err(|error| format!("送るファイルを読めません: {error}"))?;
        Ok(buffer)
    })
    .await
    .map_err(|_| "送るファイルを読めません".to_string())?
}

/// 関門の中で DB の写しとファイルの一覧を作り、写しを送る形に直す。
pub async fn prepare_snapshot(
    db: &Arc<Database>,
    app_data: &Path,
    snapshot: &Path,
) -> Result<(PortableReport, Vec<LocalFile>, i64), String> {
    if let Some(parent) = snapshot.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("写しの置き場を作れません: {error}"))?;
    }
    if snapshot.exists() {
        fs::remove_file(snapshot).map_err(|error| format!("前の写しを消せません: {error}"))?;
    }
    let db = Arc::clone(db);
    let snapshot_path = snapshot.to_path_buf();
    let root = app_data.to_path_buf();
    let (files, skipped) = tokio::task::spawn_blocking(move || {
        db.write_vacuumed_copy(&snapshot_path)?;
        scan_library_files(&root)
    })
    .await
    .map_err(|_| "本棚の写しを作れません".to_string())??;
    let snapshot_path = snapshot.to_path_buf();
    let root = app_data.to_path_buf();
    let report = tokio::task::spawn_blocking(move || prepare_for_send(&snapshot_path, &root))
        .await
        .map_err(|_| "本棚の写しを直せません".to_string())??;
    Ok((report, files, skipped))
}

/// 写しとファイルの一覧を、サーバーへ送って確定する。
pub async fn send_prepared(
    remote: &RemoteServer,
    token: &str,
    snapshot: &Path,
    files: Vec<LocalFile>,
    handle: &SendHandle,
) -> Result<CommitOutcome, String> {
    let client = build_client(remote).await?;
    let base = normalized_base(&remote.url);
    let opened: OpenedSession = check_status(
        client
            .post(format!("{base}/api/v1/admin/sync/sessions"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(net_error)?,
    )
    .await?
    .json()
    .await
    .map_err(net_error)?;
    let session = opened.session_id.clone();

    handle.update(|progress| progress.phase = "manifest".into());
    let mut needed_paths = std::collections::HashSet::new();
    for batch in files.chunks(MANIFEST_BATCH) {
        handle.check_cancel()?;
        let entries: Vec<ManifestEntry> = batch
            .iter()
            .map(|file| ManifestEntry {
                path: file.relative.clone(),
                size: file.size,
                mtime_ms: file.mtime_ms,
            })
            .collect();
        let response: serde_json::Value = check_status(
            client
                .post(format!(
                    "{base}/api/v1/admin/sync/sessions/{session}/manifest"
                ))
                .bearer_auth(token)
                .json(&serde_json::json!({ "files": entries }))
                .send()
                .await
                .map_err(net_error)?,
        )
        .await?
        .json()
        .await
        .map_err(net_error)?;
        if let Some(needed) = response.get("needed").and_then(|value| value.as_array()) {
            needed_paths.extend(
                needed
                    .iter()
                    .filter_map(|path| path.as_str().map(str::to_string)),
            );
        }
    }
    let needed: Vec<LocalFile> = files
        .into_iter()
        .filter(|file| needed_paths.contains(&file.relative))
        .collect();
    let database_size = fs::metadata(snapshot)
        .map_err(|error| format!("写しを読めません: {error}"))?
        .len();
    handle.update(|progress| {
        progress.phase = "upload".into();
        progress.files_total = needed.len() as u64;
        progress.files_done = 0;
        progress.bytes_total = needed.iter().map(|file| file.size).sum::<u64>();
        progress.bytes_done = 0;
    });

    let uploader = Arc::new(Uploader {
        client: client.clone(),
        base: base.clone(),
        token: token.to_string(),
        session: session.clone(),
        handle: handle.clone(),
        blobs: Mutex::new(HashMap::new()),
    });
    let bound: Vec<Result<BindEntry, String>> = stream::iter(needed.iter().cloned())
        .map(|file| {
            let uploader = Arc::clone(&uploader);
            async move { uploader.upload_file(&file).await }
        })
        .buffer_unordered(PARALLEL_UPLOADS)
        .collect()
        .await;
    let bound = bound.into_iter().collect::<Result<Vec<_>, _>>()?;
    for batch in bound.chunks(MANIFEST_BATCH) {
        handle.check_cancel()?;
        check_status(
            client
                .post(format!("{base}/api/v1/admin/sync/sessions/{session}/bind"))
                .bearer_auth(token)
                .json(&serde_json::json!({ "files": batch }))
                .send()
                .await
                .map_err(net_error)?,
        )
        .await?;
    }

    handle.update(|progress| {
        progress.phase = "database".into();
        progress.bytes_total += database_size;
    });
    let snapshot_path = snapshot.to_path_buf();
    let database_sha = tokio::task::spawn_blocking(move || file_sha256(&snapshot_path))
        .await
        .map_err(|_| "写しのハッシュを計算できません".to_string())??;
    if opened.database_sha256.as_deref() != Some(database_sha.as_str()) {
        uploader
            .upload_blob(snapshot, &database_sha, database_size)
            .await?;
    }

    handle.check_cancel()?;
    handle.update(|progress| progress.phase = "commit".into());
    let outcome: CommitOutcome = check_status(
        client
            .post(format!(
                "{base}/api/v1/admin/sync/sessions/{session}/commit"
            ))
            .bearer_auth(token)
            .json(&CommitRequest {
                database_sha256: database_sha,
            })
            .send()
            .await
            .map_err(net_error)?,
    )
    .await?
    .json()
    .await
    .map_err(net_error)?;
    Ok(outcome)
}
