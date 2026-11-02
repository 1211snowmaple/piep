//! デスクトップ版の側の送信。本棚の写しを作り、差分だけを単独サーバーへ上げる。
//!
//! ライブラリの関門を持つのは写しを作る間だけで、転送の間は持たない。
//! 取得元の認証情報はこの経路を通らない（DB にも送るファイルにも入っていない）。

use super::snapshot::{prepare_for_send, PortableReport, FILE_ROOTS};
use super::sync_receive::{
    BindEntry, BlobState, CommitOutcome, CommitRequest, ManifestEntry, OpenedSession,
};
use super::tls::certificate_fingerprint;
use super::{CHUNK_BYTES, MANIFEST_BATCH};
use crate::AppState;
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

const PARALLEL_UPLOADS: usize = 4;
const CHUNK_RETRIES: usize = 3;

/// 管理者としてつないでいる単独サーバー。鍵は別の場所（資格情報マネージャー）に置く。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteServer {
    pub url: String,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendProgress {
    pub running: bool,
    /// snapshot / manifest / upload / database / commit / done / failed / canceled
    pub phase: String,
    pub files_total: u64,
    pub files_done: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub message: Option<String>,
    pub outside_paths: i64,
    pub skipped_names: i64,
}

#[derive(Clone)]
pub struct SendHandle {
    pub progress: Arc<Mutex<SendProgress>>,
    pub cancel: Arc<AtomicBool>,
}

impl SendHandle {
    pub fn new() -> Self {
        Self {
            progress: Arc::new(Mutex::new(SendProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn update(&self, change: impl FnOnce(&mut SendProgress)) {
        if let Ok(mut progress) = self.progress.lock() {
            change(&mut progress);
        }
    }

    fn check_cancel(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(CANCELED.to_string())
        } else {
            Ok(())
        }
    }
}

impl Default for SendHandle {
    fn default() -> Self {
        Self::new()
    }
}

pub const CANCELED: &str = "送信を止めました";

fn normalized_base(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

fn net_error(error: reqwest::Error) -> String {
    // 誤り文に URL の問い合わせや鍵が混ざらないよう、種類だけを言う。
    if error.is_timeout() {
        "サーバーの応答が時間内にありませんでした".into()
    } else if error.is_connect() {
        "サーバーに接続できません。URL と通信状態を確認してください".into()
    } else {
        format!(
            "サーバーとの通信に失敗しました（{}）",
            error.status().map_or("通信".to_string(), |s| s.to_string())
        )
    }
}

/// 接続先を確かめて、HTTP クライアントを作る。
///
/// 指紋があれば、その証明書にだけつなぐ（自己署名）。無ければ OS の信頼する CA と
/// ホスト名で検証する。平文の http はこの機械の中だけ許す。
pub async fn build_client(remote: &RemoteServer) -> Result<reqwest::Client, String> {
    let parsed = url::Url::parse(&remote.url)
        .map_err(|_| "サーバーの URL が正しくありません".to_string())?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("サーバーの URL に利用者名や問い合わせは書けません".into());
    }
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        .no_proxy();
    match parsed.scheme() {
        "https" => {}
        "http" if matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) => {
            return builder.build().map_err(net_error);
        }
        _ => return Err("サーバーの URL は https にしてください".into()),
    }
    let Some(fingerprint) = remote.fingerprint.as_deref() else {
        return builder.https_only(true).build().map_err(net_error);
    };
    // 鍵を送る前に証明書の指紋を照らし合わせる。ここでは hello しか呼ばない。
    let probe = reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(true)
        .tls_info(true)
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(net_error)?;
    let response = probe
        .get(format!("{}/api/v1/hello", normalized_base(&remote.url)))
        .send()
        .await
        .map_err(net_error)?;
    let certificate = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate())
        .ok_or("サーバーの証明書を確かめられません")?
        .to_vec();
    if certificate_fingerprint(&certificate) != fingerprint {
        return Err("サーバーの証明書が登録時と違います。接続先を確かめてください".into());
    }
    let pinned = reqwest::Certificate::from_der(&certificate).map_err(net_error)?;
    builder
        .tls_backend_rustls()
        .tls_certs_only([pinned])
        .tls_danger_accept_invalid_hostnames(true)
        .https_only(true)
        .build()
        .map_err(net_error)
}

/// 応答の状態コードを誤りに直す。サーバーの `message` は決まった文だけなのでそのまま出す。
async fn check_status(response: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("サーバーに登録が残っていません。登録し直してください".into());
    }
    let message = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| {
            body.get("message")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
    Err(message.unwrap_or_else(|| format!("サーバーが要求を断りました（{status}）")))
}

/// 管理者として登録する。一回限りの鍵を、端末の鍵に換える。
pub async fn pair_admin(
    remote: &RemoteServer,
    secret: &str,
    device_name: &str,
) -> Result<String, String> {
    let client = build_client(remote).await?;
    let response = client
        .post(format!("{}/api/v1/pair", normalized_base(&remote.url)))
        .json(&serde_json::json!({ "secret": secret, "deviceName": device_name }))
        .send()
        .await
        .map_err(net_error)?;
    let response = check_status(response).await?;
    let body: serde_json::Value = response.json().await.map_err(net_error)?;
    if body.get("role").and_then(|role| role.as_str()) != Some("admin") {
        return Err("この登録文字列は携帯用です".into());
    }
    body.get("token")
        .and_then(|token| token.as_str())
        .map(str::to_string)
        .ok_or_else(|| "サーバーの応答を読み取れません".to_string())
}

pub async fn admin_get(
    remote: &RemoteServer,
    token: &str,
    path: &str,
) -> Result<serde_json::Value, String> {
    let client = build_client(remote).await?;
    let response = client
        .get(format!(
            "{}/api/v1/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response)
        .await?
        .json()
        .await
        .map_err(net_error)
}

pub async fn admin_post(
    remote: &RemoteServer,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = build_client(remote).await?;
    let response = client
        .post(format!(
            "{}/api/v1/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response)
        .await?
        .json()
        .await
        .map_err(net_error)
}

pub async fn admin_delete(remote: &RemoteServer, token: &str, path: &str) -> Result<(), String> {
    let client = build_client(remote).await?;
    let response = client
        .delete(format!(
            "{}/api/v1/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response).await?;
    Ok(())
}

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

fn file_sha256(path: &Path) -> Result<String, String> {
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
    state: &Arc<AppState>,
    app_data: &Path,
    snapshot: &Path,
) -> Result<(PortableReport, Vec<LocalFile>, i64), String> {
    if let Some(parent) = snapshot.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("写しの置き場を作れません: {error}"))?;
    }
    if snapshot.exists() {
        fs::remove_file(snapshot).map_err(|error| format!("前の写しを消せません: {error}"))?;
    }
    // 保存や編集の途中の状態を写さない。持つのは書き出しと一覧の間だけで、転送の間は持たない。
    let gate = state.library_gate.clone().read_owned().await;
    let db_state = Arc::clone(state);
    let snapshot_path = snapshot.to_path_buf();
    let root = app_data.to_path_buf();
    let (files, skipped) = tokio::task::spawn_blocking(move || {
        let _gate = gate;
        db_state.db.write_vacuumed_copy(&snapshot_path)?;
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
