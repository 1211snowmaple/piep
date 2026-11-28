//! 単独サーバーの自己更新。
//!
//! 本体が新しいバージョンを取ってきて検証し、静かなときに終了コードで見張り役へ頼む。
//! 版の切り替えと戻しは見張り役（`watchdog.rs`）が行う。動いている実行ファイルは
//! 自分では置き換えられないからである。

use super::durable::write_atomic;
use super::standalone::shipment::ShelfShipments;
use super::standalone::{edit_config, load_config, ServerPaths, UpdateConfig};
use chrono::{DateTime, Local, NaiveTime, Utc};
use futures_util::StreamExt;
use piep_storage::{regular_directory, regular_file, sync_directory};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{watch, Mutex as AsyncMutex, Notify};

/// このプログラムの版。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 本体が「更新を当ててほしい」と見張り役へ伝える終了コード。
pub const EXIT_APPLY_UPDATE: i32 = 75;
/// 管理画面から頼まれた再起動。見張り役は待たずに立ち上げ直す。古い見張り役は異常終了として扱い、少し待って立ち上げ直す。
pub const EXIT_RESTART: i32 = 76;

/// `tauri.conf.json` の `plugins.updater.pubkey` と同じ鍵。デスクトップ版と同じ鍵で署名する。
const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc3N0U3OUI0OUJBQzY1QkYKUldTL1pheWJ0SGwrZHhMa2FaNlc2KzYwZmYyK1BoU01iS2paUzBxTExmMzFDVWRQZkxSVTRuemQK";
const MANIFEST_URL: &str =
    "https://github.com/1211snowmaple/piep/releases/latest/download/latest-server.json";
const DOWNLOAD_PREFIX: &str = "https://github.com/1211snowmaple/piep/releases/download/";

const EXECUTABLE: &str = "piep-server.exe";
/// zip の中で受け入れるファイル。これ以外があれば、その配布物は使わない。
const PACKAGE_FILES: [&str; 7] = [
    EXECUTABLE,
    "msvcp140.dll",
    "msvcp140_1.dll",
    "pdfium.dll",
    // pdfium と同梱物の使用許諾。DLL ではないので実行物の検査には関わらない。
    "pdfium-licenses.txt",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const PREPARED_PACKAGE: &str = ".prepared.zip";
const PREVIOUS_DIRECTORY_PREFIX: &str = ".previous-";
const VERSION_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_VERSION_OUTPUT_BYTES: u64 = 1024;
static PACKAGE_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const QUIET_POLL: Duration = Duration::from_secs(30);
const QUIET_LIMIT: Duration = Duration::from_secs(30 * 60);
const WINDOW_POLL: Duration = Duration::from_secs(60);

/// 更新を取りに行く先。設定や API からは変えられない。
#[derive(Debug, Clone)]
pub struct UpdateSource {
    pub manifest_url: String,
    pub download_prefix: String,
    pub public_key: String,
}

impl UpdateSource {
    pub fn fixed() -> Self {
        Self {
            manifest_url: MANIFEST_URL.into(),
            download_prefix: DOWNLOAD_PREFIX.into(),
            public_key: PUBLIC_KEY.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    version: String,
    url: String,
    signature: String,
}

/// `1.2.3` の形だけを版として認める。
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let parsed = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(parsed)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// `install` で置いた形なら、見張り役の下で動いている。
pub fn is_managed(paths: &ServerPaths) -> bool {
    current_version(paths).is_some()
}

pub fn current_version(paths: &ServerPaths) -> Option<String> {
    read_version_file(&paths.current_version())
}

pub fn read_version_file(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let version = text.trim();
    parse_version(version).map(|_| version.to_string())
}

/// 見張り役と本体が読み書きする更新の記録。版をまたいで読むので、知らない項目は無視する。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateRecord {
    pub last_checked_at: Option<String>,
    pub latest: Option<String>,
    /// 当てて起動しなかった版。もう試さない。
    pub skipped: Vec<String>,
    pub last_error: Option<String>,
    /// 起動に失敗して戻した理由は、更新の確認結果より優先する。
    startup_failed: bool,
    pub last_applied_at: Option<String>,
}

impl UpdateRecord {
    pub fn load(paths: &ServerPaths) -> Self {
        Self::read(&paths.update_record()).unwrap_or_default()
    }

    fn read(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("更新の記録を読み取れません: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("更新の記録を開けません: {error}")),
        }
    }

    pub(super) fn edit(paths: &ServerPaths, change: impl FnOnce(&mut Self)) {
        if let Err(error) = Self::try_edit(paths, change) {
            log::warn!("更新の記録を書けません: {error}");
        }
    }

    /// 差し替える記録とは別のファイルをロックし、読み直しから同期まで保持する。
    pub(super) fn try_edit<T>(
        paths: &ServerPaths,
        change: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, String> {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.state().join("update.lock"))
            .map_err(|error| format!("更新の記録の鍵を開けません: {error}"))?;
        lock.lock()
            .map_err(|error| format!("更新の記録をロックできません: {error}"))?;
        let path = paths.update_record();
        let mut record = Self::read(&path)?;
        let result = change(&mut record);
        let bytes = serde_json::to_vec_pretty(&record).map_err(|error| error.to_string())?;
        write_atomic(&path, &bytes)?;
        drop(lock);
        Ok(result)
    }

    pub(super) fn skip(&mut self, version: &str) {
        if !self.skipped.iter().any(|skipped| skipped == version) {
            self.skipped.push(version.to_string());
        }
    }

    pub(super) fn note_failed_update(&mut self, reason: String) {
        self.startup_failed = true;
        self.last_error = Some(reason);
    }

    pub(super) fn clear_transient_error(&mut self) {
        if !self.startup_failed {
            self.last_error = None;
        }
    }

    pub(super) fn clear_error(&mut self) {
        self.startup_failed = false;
        self.last_error = None;
    }

    fn note_check(&mut self, version: &str) {
        self.last_checked_at = Some(Utc::now().to_rfc3339());
        self.latest = Some(version.to_string());
    }
}

/// 更新の履歴の 1 行。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHistoryEntry {
    pub at: String,
    pub from: Option<String>,
    pub to: String,
    /// `applied`（更新した）、`failed`（起動せず戻った）、`rolled_back`（手で戻した）。
    pub result: String,
    /// 止めてから、次の版が動き出すまで。
    pub downtime_seconds: Option<i64>,
    pub reason: Option<String>,
}

const HISTORY_LIMIT: usize = 50;

/// 更新の履歴。書くのは本体だけにする。
///
/// 見張り役は更新のあとも次の起動まで古いまま動き続けるので、`UpdateRecord` に載せると
/// 古い見張り役が知らない項目を落としてしまう。更新の結果や版の変化があった起動で 1 行を足す。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateHistory {
    /// 前回動いていた版。
    pub last_version: Option<String>,
    /// 当てるために止めた時刻と、当てる先。
    pub stopping_at: Option<String>,
    pub stopping_to: Option<String>,
    /// 手で戻すところなら true。
    pub stopping_rollback: bool,
    pub entries: Vec<UpdateHistoryEntry>,
}

impl UpdateHistory {
    pub fn load(paths: &ServerPaths) -> Self {
        fs::read(paths.update_history())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &ServerPaths) -> bool {
        serde_json::to_vec_pretty(self)
            .map_err(|error| error.to_string())
            .and_then(|bytes| write_atomic(&paths.update_history(), &bytes))
            .inspect_err(|error| log::warn!("更新の履歴を書けません: {error}"))
            .is_ok()
    }

    fn push(&mut self, entry: UpdateHistoryEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(HISTORY_LIMIT);
    }

    /// 止める前に、当てる先を書いておく。
    pub fn note_stopping(paths: &ServerPaths, to: &str, rollback: bool) {
        let mut history = Self::load(paths);
        history.stopping_at = Some(Utc::now().to_rfc3339());
        history.stopping_to = Some(to.to_string());
        history.stopping_rollback = rollback;
        history.save(paths);
    }

    /// 本体が動き出したら呼ぶ。前回と比べて、新しい行を履歴に保存できたときだけ true を返す。
    pub fn reconcile(paths: &ServerPaths, version: &str, now: DateTime<Utc>) -> bool {
        let mut history = Self::load(paths);
        let downtime = history
            .stopping_at
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map(|at| (now - at.with_timezone(&Utc)).num_seconds().max(0));
        let from = history.last_version.clone();
        let target = history.stopping_to.take();
        let rollback = std::mem::take(&mut history.stopping_rollback);
        history.stopping_at = None;
        let entry = match target {
            Some(target) if target == version => Some(UpdateHistoryEntry {
                at: now.to_rfc3339(),
                from,
                to: target,
                result: if rollback { "rolled_back" } else { "applied" }.into(),
                downtime_seconds: downtime,
                reason: None,
            }),
            // 当てようとした版とは別の版で動き出した。見張り役が戻した。
            Some(target) => Some(UpdateHistoryEntry {
                at: now.to_rfc3339(),
                from,
                to: target,
                result: "failed".into(),
                downtime_seconds: downtime,
                reason: UpdateRecord::load(paths).last_error,
            }),
            // 更新の外で版が変わった。入れ直したときなど。
            None if from.as_deref().is_some_and(|from| from != version) => {
                Some(UpdateHistoryEntry {
                    at: now.to_rfc3339(),
                    from,
                    to: version.to_string(),
                    result: "applied".into(),
                    downtime_seconds: None,
                    reason: None,
                })
            }
            None => None,
        };
        let added = entry.is_some();
        if let Some(entry) = entry {
            history.push(entry);
        }
        history.last_version = Some(version.to_string());
        let saved = history.save(paths);
        added && saved
    }
}

/// 署名を検証する。Tauri の署名は minisign の文字列を base64 にしたもの。
pub fn verify_signature(public_key: &str, signature: &str, data: &[u8]) -> Result<(), String> {
    let decode = |text: &str| -> Result<String, String> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| "署名を読み取れません".to_string())?;
        String::from_utf8(bytes).map_err(|_| "署名を読み取れません".to_string())
    };
    let key = minisign_verify::PublicKey::decode(&decode(public_key)?)
        .map_err(|_| "公開鍵を読み取れません".to_string())?;
    let signature = minisign_verify::Signature::decode(&decode(signature)?)
        .map_err(|_| "署名を読み取れません".to_string())?;
    key.verify(data, &signature, true)
        .map_err(|_| "署名が公開鍵と一致しません".to_string())
}

/// CI が、できた配布物を組み込みの鍵で確かめる。
pub fn verify_package_file(package: &Path, signature_file: &Path) -> Result<(), String> {
    let data = read_package(package)?;
    let signature =
        fs::read_to_string(signature_file).map_err(|error| format!("署名を読めません: {error}"))?;
    verify_signature(PUBLIC_KEY, &signature, &data)
}

fn read_package(package: &Path) -> Result<Vec<u8>, String> {
    let read_error = |error| format!("配布物を読めません: {error}");
    let mut file = File::open(package).map_err(read_error)?;
    let size = file.metadata().map_err(read_error)?.len();
    if size > MAX_PACKAGE_BYTES {
        return Err("新しいバージョンが大きすぎます".into());
    }
    let mut data = Vec::new();
    data.try_reserve_exact(size as usize)
        .map_err(|error| format!("配布物を読むためのメモリを確保できません: {error}"))?;
    data.resize(size as usize, 0);
    file.read_exact(&mut data).map_err(read_error)?;
    if file.read(&mut [0]).map_err(read_error)? != 0 {
        return Err("読み取り中に配布物の大きさが変わりました".into());
    }
    Ok(data)
}

/// 一時ディレクトリに展開し、版を照合する。失敗した展開は片付ける。
fn extract_package(
    package: impl Read + Seek,
    staging: &Path,
    check: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    fs::create_dir(staging).map_err(|error| format!("展開先を作れません: {error}"))?;
    let result = unpack_package(package, staging).and_then(|()| check(staging));
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

/// 知らないファイルや大小文字だけが違う同名ファイルを受け入れない。
fn unpack_package(package: impl Read + Seek, staging: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(package)
        .map_err(|error| format!("配布物を読み取れません: {error}"))?;
    let mut total = 0_u64;
    let mut has_executable = false;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("配布物を読み取れません: {error}"))?;
        let name = entry
            .name()
            .map_err(|error| format!("配布物を読み取れません: {error}"))?
            .into_owned();
        let canonical = name.to_ascii_lowercase();
        if !PACKAGE_FILES.contains(&canonical.as_str()) {
            return Err(format!("配布物に知らないファイルがあります: {name}"));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err("配布物が大きすぎます".into());
        }
        let mut output = File::create_new(staging.join(&canonical)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("配布物に同じ名前のファイルが複数あります: {name}")
            } else {
                format!("展開できません: {error}")
            }
        })?;
        has_executable |= canonical == EXECUTABLE;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("展開できません: {error}"))?;
    }
    if has_executable {
        Ok(())
    } else {
        Err(format!("配布物に {EXECUTABLE} がありません"))
    }
}

fn recover_retired_directory(previous: &Path, destination: &Path) -> std::io::Result<()> {
    if destination.try_exists()? {
        fs::remove_dir_all(previous)
    } else {
        fs::rename(previous, destination)
    }
}

fn recover_package_directories(versions: &Path) -> std::io::Result<()> {
    let entries = match fs::read_dir(versions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(version) = name
            .to_str()
            .and_then(|name| name.strip_prefix(PREVIOUS_DIRECTORY_PREFIX))
            .filter(|version| parse_version(version).is_some())
        {
            recover_retired_directory(&entry.path(), &versions.join(version))?;
        }
    }
    Ok(())
}

fn replace_package_directory(staging: &Path, destination: &Path) -> Result<(), String> {
    let name = destination.file_name().expect("version directory");
    let previous = destination.with_file_name(format!(
        "{PREVIOUS_DIRECTORY_PREFIX}{}",
        name.to_string_lossy()
    ));
    // 新しい展開が所定の位置に来るまで、元の展開を完全なディレクトリのまま残す。
    let had_previous = destination
        .try_exists()
        .map_err(|error| format!("前の展開を確かめられません: {error}"))?;
    if had_previous {
        fs::rename(destination, &previous)
            .map_err(|error| format!("前の展開を退避できません: {error}"))?;
    }
    if let Err(error) = fs::rename(staging, destination) {
        if had_previous {
            fs::rename(&previous, destination).map_err(|restore_error| {
                format!(
                    "新しいバージョンを置けず、退避した展開も戻せません: {error} / {restore_error}"
                )
            })?;
        }
        return Err(format!("新しいバージョンを置けません: {error}"));
    }
    if had_previous {
        if let Err(error) = fs::remove_dir_all(&previous) {
            log::warn!("退避した展開を消せません: {error}");
        }
    }
    Ok(())
}

/// 展開したプログラムに版を尋ねる。署名を確かめたあとにだけ呼ぶ。
fn reported_version(executable: &Path) -> Result<String, String> {
    crate::server::watchdog::launch_failures_without_dialogs();
    let mut command = Command::new(executable);
    command.arg("--version");
    tokio::runtime::Handle::current().block_on(query_version(command, VERSION_QUERY_TIMEOUT))
}

async fn query_version(mut command: Command, timeout: Duration) -> Result<String, String> {
    let run_error = |error| format!("新しいバージョンを動かせません: {error}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(run_error)?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let result = tokio::time::timeout(timeout, async {
        let mut output = Vec::new();
        (&mut stdout)
            .take(MAX_VERSION_OUTPUT_BYTES + 1)
            .read_to_end(&mut output)
            .await
            .map_err(run_error)?;
        if output.len() as u64 > MAX_VERSION_OUTPUT_BYTES {
            return Err("新しいバージョンの照会結果が長すぎます".into());
        }
        if !child.wait().await.map_err(run_error)?.success() {
            return Err("新しいバージョンが自分のバージョンを答えません".into());
        }
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            if let Err(error) = child.kill().await {
                log::warn!("バージョンの照会を終了できません: {error}");
            }
            Err("新しいバージョンの照会が時間内に終わりませんでした".into())
        }
    }
}

async fn download(client: &reqwest::Client, url: &str, destination: &Path) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("新しいバージョンを取ってこられません: {error}"))?;
    let mut output = tokio::fs::File::create(destination)
        .await
        .map_err(|error| format!("新しいバージョンを保存できません: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| format!("新しいバージョンを取ってこられません: {error}"))?;
        total += chunk.len() as u64;
        if total > MAX_PACKAGE_BYTES {
            return Err("新しいバージョンが大きすぎます".into());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|error| format!("新しいバージョンを保存できません: {error}"))?;
    }
    output
        .flush()
        .await
        .map_err(|error| format!("新しいバージョンを保存できません: {error}"))
}

/// 公開されている最新の版を確かめて記録する。取ってはこない。
pub async fn check_latest(source: &UpdateSource, paths: &ServerPaths) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("piep-server/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("更新を確かめられません: {error}"))?;
    let manifest = fetch_manifest(&client, source, Duration::from_secs(30)).await?;
    UpdateRecord::try_edit(paths, |record| {
        record.note_check(&manifest.version);
        record.clear_transient_error();
    })?;
    Ok(manifest.version)
}

async fn fetch_manifest(
    client: &reqwest::Client,
    source: &UpdateSource,
    timeout: Duration,
) -> Result<Manifest, String> {
    let response = client
        .get(&source.manifest_url)
        .timeout(timeout)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("更新を確かめられません: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("更新の案内を読み取れません: {error}"))?;
        if chunk.len() > MAX_MANIFEST_BYTES.saturating_sub(body.len()) {
            return Err("更新の案内が大きすぎます".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|error| format!("更新の案内を読み取れません: {error}"))
}

struct PackageWorkspace {
    extracted: PathBuf,
    package: PathBuf,
    // 呼び出しが取り消されても、検証が終わるまでは次の更新が作業場所を消せない。
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl PackageWorkspace {
    fn new(versions: &Path, lock: tokio::sync::MutexGuard<'static, ()>) -> Result<Self, String> {
        let extracted = versions.join(".incoming");
        let package = versions.join(".incoming.zip");
        fs::create_dir_all(versions)
            .map_err(|error| format!("新しいバージョンを保存できません: {error}"))?;
        if extracted.exists() {
            fs::remove_dir_all(&extracted)
                .map_err(|error| format!("前の展開を消せません: {error}"))?;
        }
        if package.exists() {
            fs::remove_file(&package).map_err(|error| format!("前の取得を消せません: {error}"))?;
        }
        Ok(Self {
            extracted,
            package,
            _lock: lock,
        })
    }
}

impl Drop for PackageWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.extracted);
        let _ = fs::remove_file(&self.package);
    }
}

/// 新しいバージョンがあれば取ってきて検証し、`versions\<版>\` に置く。置いた版を返す。
pub async fn prepare_latest(
    source: &UpdateSource,
    paths: &ServerPaths,
    current: &str,
) -> Result<Option<String>, String> {
    prepare_latest_with_timeouts(
        source,
        paths,
        current,
        Duration::from_secs(30),
        Duration::from_secs(30 * 60),
        reported_version,
    )
    .await
}

async fn prepare_latest_with_timeouts(
    source: &UpdateSource,
    paths: &ServerPaths,
    current: &str,
    manifest_timeout: Duration,
    package_timeout: Duration,
    report: impl Fn(&Path) -> Result<String, String> + Clone + Send + 'static,
) -> Result<Option<String>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("piep-server/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(package_timeout)
        .build()
        .map_err(|error| format!("更新を確かめられません: {error}"))?;
    let lock = PACKAGE_LOCK.lock().await;
    let versions = paths.versions();
    recover_package_directories(&versions)
        .map_err(|error| format!("中断した展開を片付けられません: {error}"))?;
    let manifest = fetch_manifest(&client, source, manifest_timeout).await?;

    let has_update = UpdateRecord::try_edit(paths, |record| {
        record.note_check(&manifest.version);
        let has_update =
            is_newer(&manifest.version, current) && !record.skipped.contains(&manifest.version);
        if !has_update {
            record.clear_transient_error();
        }
        has_update
    })?;
    let cached = versions.join(PREPARED_PACKAGE);
    if !has_update {
        let _ = fs::remove_file(&cached);
        return Ok(None);
    }
    if !manifest.url.starts_with(&source.download_prefix) {
        return Err("更新の案内が決められた場所以外を指しています".into());
    }

    let workspace = Arc::new(PackageWorkspace::new(&versions, lock)?);
    let verify = |package: PathBuf| {
        let workspace = Arc::clone(&workspace);
        let public_key = source.public_key.clone();
        let manifest = manifest.clone();
        let report = report.clone();
        tokio::task::spawn_blocking(move || {
            verify_and_extract(
                &package,
                &workspace.extracted,
                &public_key,
                &manifest,
                report,
            )
        })
    };
    let task_error = |error| format!("配布物の検証を終えられません: {error}");
    // 延期した版も、最新の案内に対して署名と版を確かめ、検証した内容から展開し直す。
    let reused = if cached.is_file() {
        verify(cached.clone())
            .await
            .map_err(task_error)?
            .inspect_err(|error| log::warn!("保存済みの配布物を取り直します: {error}"))
            .is_ok()
    } else {
        false
    };
    if !reused {
        download(&client, &manifest.url, &workspace.package).await?;
        verify(workspace.package.clone())
            .await
            .map_err(task_error)??;
        if let Err(error) = fs::rename(&workspace.package, &cached) {
            log::warn!("延期に備えて配布物を保存できません: {error}");
        }
    }
    let installed = paths.version_dir(&manifest.version);
    tokio::task::spawn_blocking(move || {
        replace_package_directory(&workspace.extracted, &installed)
    })
    .await
    .map_err(task_error)??;
    Ok(Some(manifest.version))
}

fn verify_and_extract(
    package: &Path,
    staging: &Path,
    public_key: &str,
    manifest: &Manifest,
    report: impl Fn(&Path) -> Result<String, String>,
) -> Result<(), String> {
    let data = read_package(package)?;
    verify_signature(public_key, &manifest.signature, &data)?;
    extract_package(Cursor::new(data), staging, |extracted| {
        let reported = report(&extracted.join(EXECUTABLE))?;
        if reported != manifest.version {
            return Err(format!(
                "案内のバージョン {} と中身のバージョン {reported} が違います",
                manifest.version
            ));
        }
        Ok(())
    })
}

/// `03:00-06:00` の形の時間帯。日をまたいでもよい。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateWindow {
    start: NaiveTime,
    end: NaiveTime,
}

impl UpdateWindow {
    pub fn parse(text: &str) -> Result<Self, String> {
        let invalid = || format!("更新の時間帯 {text} は 03:00-06:00 の形で書いてください");
        let (start, end) = text.split_once('-').ok_or_else(invalid)?;
        let time =
            |part: &str| NaiveTime::parse_from_str(part.trim(), "%H:%M").map_err(|_| invalid());
        Ok(Self {
            start: time(start)?,
            end: time(end)?,
        })
    }

    pub fn contains(&self, time: NaiveTime) -> bool {
        if self.start <= self.end {
            self.start <= time && time < self.end
        } else {
            time >= self.start || time < self.end
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "state", content = "version")]
pub enum UpdatePhase {
    Idle,
    Checking,
    /// 取ってきて検証した。時間帯か、送信の区切りを待っている。
    Waiting(String),
    Applying(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub auto: bool,
    /// 自動で当てる時間帯。`03:00-06:00`
    pub window: String,
    pub phase: UpdatePhase,
    pub latest: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
    pub last_applied_at: Option<String>,
    /// 起動せずに戻ったなどで、飛ばしている版。
    pub skipped: Vec<String>,
    /// 戻せる版。無ければ戻せない。
    pub previous: Option<String>,
}

/// 動いている間に変えられる設定。
#[derive(Debug, Clone)]
struct LiveSettings {
    auto: bool,
    window_text: String,
    window: UpdateWindow,
}

/// 口を閉じたあとに行うこと。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterStop {
    Apply(String),
    RollBack(String),
}

const UPDATE_BUSY: &str = "更新の処理が終わってから、もう一度お試しください";

struct UpdateOperation {
    phase: UpdatePhase,
    immediate: bool,
    after_stop: Option<AfterStop>,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

struct ShipmentAccess {
    shipments: Arc<ShelfShipments>,
    opening: Arc<AsyncMutex<()>>,
}

/// 本体の中で動く更新の係。頼めるのは「確かめて当てる」だけで、ファイルや URL は渡せない。
pub struct UpdateController {
    paths: ServerPaths,
    source: UpdateSource,
    settings: Mutex<watch::Sender<LiveSettings>>,
    operation: Mutex<UpdateOperation>,
    requested: Notify,
    shipment_access: OnceLock<ShipmentAccess>,
}

impl UpdateController {
    pub fn new(
        paths: ServerPaths,
        source: UpdateSource,
        config: UpdateConfig,
        stop: Box<dyn FnOnce() + Send>,
    ) -> Result<Arc<Self>, String> {
        let window = UpdateWindow::parse(&config.window)?;
        Ok(Arc::new(Self {
            paths,
            source,
            settings: Mutex::new(watch::Sender::new(LiveSettings {
                auto: config.auto,
                window_text: config.window,
                window,
            })),
            operation: Mutex::new(UpdateOperation {
                phase: UpdatePhase::Idle,
                immediate: false,
                after_stop: None,
                stop: Some(stop),
            }),
            requested: Notify::new(),
            shipment_access: OnceLock::new(),
        }))
    }

    fn live(&self) -> LiveSettings {
        self.subscribe_settings().borrow().clone()
    }

    fn subscribe_settings(&self) -> watch::Receiver<LiveSettings> {
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .subscribe()
    }

    fn operation(&self) -> MutexGuard<'_, UpdateOperation> {
        self.operation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn set_shipment_access(
        &self,
        shipments: Arc<ShelfShipments>,
        opening: Arc<AsyncMutex<()>>,
    ) {
        let _ = self
            .shipment_access
            .set(ShipmentAccess { shipments, opening });
    }

    /// 呼ぶ側は発送開始の排他を持つ。停止を予約済みなら HTTP の口を閉じる途中でも開始を断る。
    pub(super) fn allows_shipment_start(&self) -> bool {
        !matches!(self.operation().phase, UpdatePhase::Applying(_))
    }

    pub fn status(&self) -> UpdateStatus {
        let record = UpdateRecord::load(&self.paths);
        let settings = self.live();
        UpdateStatus {
            auto: settings.auto,
            window: settings.window_text,
            phase: self.operation().phase.clone(),
            latest: record.latest,
            last_checked_at: record.last_checked_at,
            last_error: record.last_error,
            last_applied_at: record.last_applied_at,
            skipped: record.skipped,
            previous: self.rollback_target(),
        }
    }

    pub fn history(&self) -> Vec<UpdateHistoryEntry> {
        UpdateHistory::load(&self.paths).entries
    }

    /// 今すぐ確かめて当てる。時間帯を待たない。
    pub fn request_now(&self) -> Result<(), &'static str> {
        self.reserve_check(true)?;
        self.requested.notify_one();
        Ok(())
    }

    /// 次の周期を待たずに確かめ、取ってきた版は時間帯に当てる。デスクトップ版が自分の更新に
    /// 合わせて頼む。確かめや待機がもう進んでいれば、それに任せる。
    pub fn request_in_window(&self) {
        if self.reserve_check(false).is_ok() {
            self.requested.notify_one();
        }
    }

    fn reserve_check(&self, immediate: bool) -> Result<(), &'static str> {
        let mut operation = self.operation();
        match operation.phase {
            UpdatePhase::Idle => operation.phase = UpdatePhase::Checking,
            // 時間帯を待つ自動更新があれば、その処理を一度だけ早められる。
            UpdatePhase::Waiting(_) if immediate && !operation.immediate => {}
            _ => return Err(UPDATE_BUSY),
        }
        operation.immediate = immediate;
        Ok(())
    }

    /// 確かめるだけ。取ってこないので、結果はすぐ返る。
    pub async fn check_now(&self) -> Result<String, String> {
        match check_latest(&self.source, &self.paths).await {
            Ok(version) => Ok(version),
            Err(error) => {
                self.record_error(&error);
                Err(error)
            }
        }
    }

    /// 自動更新と時間帯を変える。`server.toml` に書き戻し、再起動せずに効かせる。
    pub fn set_settings(&self, auto: bool, window_text: &str) -> Result<(), String> {
        let window = UpdateWindow::parse(window_text)?;
        // 保存と通知を同じ順にする。設定変更が重なっても、ファイルと待つ側に同じ値が残る。
        let settings = self
            .settings
            .lock()
            .map_err(|_| "設定を変えられません".to_string())?;
        write_update_settings(&self.paths, auto, window_text)?;
        settings.send_replace(LiveSettings {
            auto,
            window_text: window_text.to_string(),
            window,
        });
        Ok(())
    }

    /// 飛ばしている版をもう一度試す。
    pub fn retry_skipped(&self) {
        UpdateRecord::edit(&self.paths, |record| {
            record.skipped.clear();
            record.clear_error();
        });
        let _ = self.request_now();
    }

    /// 戻せる版。版の置き場と DB の写しの両方があるときだけ。
    fn rollback_target(&self) -> Option<String> {
        let previous = read_version_file(&self.paths.previous_version())?;
        (is_newer(VERSION, &previous)
            && self.paths.version_dir(&previous).join(EXECUTABLE).exists()
            && read_database_backup(&self.paths).is_ok_and(|backup| backup.is_some()))
        .then_some(previous)
    }

    /// 前の版へ戻す。戻した版は飛ばすので、自動で当て直さない。
    pub fn roll_back(&self) -> Result<String, String> {
        // 自己更新直後には、以前の見張り役がまだ動いていることがある。
        // HTTPを止める前に確認し、旧版へ新しい復旧手順を渡さない。
        super::watchdog::check_rollback_support()?;
        let previous = self
            .rollback_target()
            .ok_or("戻せるバージョンがありません".to_string())?;
        let after = AfterStop::RollBack(previous.clone());
        let shipments = self
            .shipment_access
            .get()
            .map(|access| access.shipments.as_ref());
        if !self.stop_when_quiet(shipments, &after)? {
            return Err("送信が区切りに来てから、もう一度お試しください".into());
        }
        Ok(previous)
    }

    /// 本体が止まったあとに行うこと。
    pub fn take_after_stop(&self) -> Option<AfterStop> {
        self.operation().after_stop.take()
    }

    fn stop_for(&self, after: AfterStop) -> Result<(), String> {
        let mut operation = self.operation();
        let (version, rollback) = match (&after, &operation.phase) {
            (AfterStop::Apply(version), UpdatePhase::Waiting(waiting)) if version == waiting => {
                (version, false)
            }
            (AfterStop::RollBack(version), UpdatePhase::Idle) => (version, true),
            _ => return Err(UPDATE_BUSY.into()),
        };
        if rollback {
            UpdateRecord::try_edit(&self.paths, |record| record.skip(VERSION))?;
        }
        UpdateHistory::note_stopping(&self.paths, version, rollback);
        operation.phase = UpdatePhase::Applying(version.clone());
        operation.after_stop = Some(after);
        let stop = operation.stop.take();
        drop(operation);
        if !rollback {
            let _ = fs::remove_file(self.paths.versions().join(PREPARED_PACKAGE));
        }
        if let Some(stop) = stop {
            stop();
        }
        Ok(())
    }

    fn stop_when_quiet(
        &self,
        shipments: Option<&ShelfShipments>,
        after: &AfterStop,
    ) -> Result<bool, String> {
        // 開始処理の途中も含めて送信がないことを確かめ、同じ排他の中で停止を予約する。
        // 予約後は新しい開始を断り、先に始まった送信はこの予約の前に区切りまで進める。
        // 公開を待つ棚は現在の版で先に配る。試行中に棚が切り替わると台帳の写しと合わなくなる。
        let _opening = if let Some(access) = self.shipment_access.get() {
            let Ok(opening) = access.opening.try_lock() else {
                return Ok(false);
            };
            Some(opening)
        } else {
            None
        };
        if shipments.is_some_and(|shipments| shipments.busy() || shipments.pending()) {
            return Ok(false);
        }
        self.stop_for(after.clone())?;
        Ok(true)
    }

    fn record_error(&self, error: &str) {
        log::warn!("更新: {error}");
        UpdateRecord::edit(&self.paths, |record| {
            if !record.startup_failed {
                record.last_error = Some(error.to_string());
            }
        });
    }

    /// 棚の受け取りが区切りに来たら適用を予約する。来なければ次の回へ延ばす。
    async fn apply_when_quiet(
        &self,
        shipments: Option<&ShelfShipments>,
        version: String,
    ) -> Result<(), String> {
        let after = AfterStop::Apply(version);
        let deadline = tokio::time::Instant::now() + QUIET_LIMIT;
        while !self.stop_when_quiet(shipments, &after)? {
            if tokio::time::Instant::now() >= deadline {
                return Err("送信が終わらないので、更新を次に延ばします".into());
            }
            tokio::time::sleep(QUIET_POLL).await;
        }
        Ok(())
    }

    /// 時間帯に入るか、今すぐと頼まれるまで待つ。自動を切られたら false。
    async fn wait_for_window(&self, local_time: impl Fn() -> NaiveTime) -> bool {
        let mut changed = self.subscribe_settings();
        loop {
            if self.operation().immediate {
                return true;
            }
            let settings = changed.borrow_and_update().clone();
            if !settings.auto {
                return false;
            }
            if settings.window.contains(local_time()) {
                return true;
            }
            tokio::select! {
                _ = self.requested.notified() => {}
                _ = changed.changed() => {}
                _ = tokio::time::sleep(WINDOW_POLL) => {}
            }
        }
    }

    /// 1 回分。当てることになったら true を返し、本体を止める。
    async fn run_once(&self, now: bool, shipments: Option<&ShelfShipments>) -> bool {
        if !now && !self.live().auto {
            // 自動を切っていても、新しいバージョンがあることは知らせる。取ってはこない。
            if let Err(error) = check_latest(&self.source, &self.paths).await {
                self.record_error(&error);
            }
            self.operation().phase = UpdatePhase::Idle;
            return false;
        }
        let prepared = prepare_latest(&self.source, &self.paths, VERSION).await;
        let version = match prepared {
            Ok(Some(version)) => version,
            Ok(None) => {
                self.operation().phase = UpdatePhase::Idle;
                return false;
            }
            Err(error) => {
                self.record_error(&error);
                self.operation().phase = UpdatePhase::Idle;
                return false;
            }
        };
        self.operation().phase = UpdatePhase::Waiting(version.clone());
        if !now && !self.wait_for_window(|| Local::now().time()).await {
            let mut operation = self.operation();
            // 自動を切って待機を終える直前に「今すぐ」が届いたら、その依頼を処理する。
            if !operation.immediate {
                operation.phase = UpdatePhase::Idle;
                return false;
            }
        }
        if let Err(error) = self.apply_when_quiet(shipments, version).await {
            self.record_error(&error);
            self.operation().phase = UpdatePhase::Idle;
            return false;
        }
        true
    }

    /// 本体が動いている間、回り続ける。自動を切っていても、確認だけは続ける。
    pub async fn run(self: Arc<Self>, shipments: Option<Arc<ShelfShipments>>) {
        let mut delay = FIRST_CHECK_DELAY;
        loop {
            let now = tokio::select! {
                _ = self.requested.notified() => {
                    let operation = self.operation();
                    if !matches!(operation.phase, UpdatePhase::Checking) {
                        continue;
                    }
                    operation.immediate
                }
                _ = tokio::time::sleep(delay) => {
                    if self.reserve_check(false).is_err() {
                        continue;
                    }
                    false
                }
            };
            if self.run_once(now, shipments.as_deref()).await {
                return;
            }
            delay = CHECK_INTERVAL;
        }
    }
}

/// `[update]` の値だけを書き換える。コメントと他の設定はそのまま残す。
pub(super) fn write_update_settings(
    paths: &ServerPaths,
    auto: bool,
    window: &str,
) -> Result<(), String> {
    edit_config(paths, |path| {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("設定を読めません: {error}")),
        };
        let mut document: toml_edit::DocumentMut = text
            .parse()
            .map_err(|error| format!("設定を読み取れません: {error}"))?;
        // 読み込み側は `update = { ... }` の書き方も受け取るので、どちらの表の形でも書き換える。
        let update = document
            .entry("update")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_like_mut()
            .ok_or("設定の update が表ではありません")?;
        set_value_kept(update, "auto", auto.into());
        set_value_kept(update, "window", window.into());
        write_atomic(path, document.to_string().as_bytes())
    })
}

/// 値だけを差し替え、行末のコメントなど前後の装飾は残す。
/// 鍵（とその上の説明の行）は差し替えない。
fn set_value_kept(table: &mut dyn toml_edit::TableLike, key: &str, mut value: toml_edit::Value) {
    match table.get_mut(key) {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *value.decor_mut() = old.decor().clone();
            }
            *item = toml_edit::Item::Value(value);
        }
        None => {
            table.insert(key, toml_edit::Item::Value(value));
        }
    }
}

const DATABASE_MANIFEST: &str = "databases.json";
const PREVIOUS_DATABASE_BACKUP: &str = ".previous-update-backup";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DatabaseBackup {
    state: bool,
    shelf: Option<PathBuf>,
}

/// 当てる前に DB を整合した形で写す。戻すときに使う。
pub fn back_up_databases(paths: &ServerPaths) -> Result<(), String> {
    let manifest = DatabaseBackup {
        state: paths
            .store()
            .try_exists()
            .map_err(|error| error.to_string())?,
        shelf: shelf_database(paths)?,
    };
    let backup = paths.update_backup();
    let previous = paths.state().join(PREVIOUS_DATABASE_BACKUP);
    if previous.try_exists().map_err(|error| error.to_string())? {
        // 差し替えの途中で止まっていたら、完成した写しを所定の位置に戻す。
        recover_retired_directory(&previous, &backup)
            .map_err(|error| format!("前の写しを整理できません: {error}"))?;
        sync_directory(&paths.state()).map_err(|error| error.to_string())?;
    }
    let staging = paths.state().join(".incoming-update-backup");
    match fs::remove_dir_all(&staging) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(format!("作業中の写しを片付けられません: {error}")),
    }
    fs::create_dir(&staging).map_err(|error| format!("写しの置き場を作れません: {error}"))?;
    // 棚を開くと台帳は移行されるが、library.json は読み取るだけで、
    // objects/ の既存の実体は内容で識別され、起動時の移行で上書きされない。
    for (source, name, was_present) in database_pairs(paths, &manifest) {
        if !was_present {
            continue;
        }
        regular_file(&source).map_err(|error| format!("{name} を開けません: {error}"))?;
        let connection = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|error| format!("{name} を開けません: {error}"))?;
        let copy = staging.join(name);
        let destination = copy.to_str().ok_or("写しのパスを読み取れません")?;
        connection
            .execute("VACUUM INTO ?1", [destination])
            .map_err(|error| format!("{name} を写せません: {error}"))?;
        sync_database(&copy)?;
    }
    let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    write_atomic(&staging.join(DATABASE_MANIFEST), &bytes)?;

    // 新しい写しが完成するまで前の写しを残し、差し替えの途中でも退避先から戻せるようにする。
    let had_previous = backup.try_exists().map_err(|error| error.to_string())?;
    if had_previous {
        fs::rename(&backup, &previous)
            .map_err(|error| format!("前の写しを退避できません: {error}"))?;
        sync_directory(&paths.state()).map_err(|error| error.to_string())?;
    }
    fs::rename(&staging, &backup).map_err(|error| format!("新しい写しを置けません: {error}"))?;
    sync_directory(&paths.state()).map_err(|error| error.to_string())?;
    if had_previous {
        if let Err(error) = fs::remove_dir_all(&previous) {
            log::warn!("前の写しを片付けられません: {error}");
        }
    }
    Ok(())
}

fn read_database_backup(paths: &ServerPaths) -> Result<Option<(PathBuf, DatabaseBackup)>, String> {
    let mut backup = paths.update_backup();
    if !backup.try_exists().map_err(|error| error.to_string())? {
        backup = paths.state().join(PREVIOUS_DATABASE_BACKUP);
        if !backup.try_exists().map_err(|error| error.to_string())? {
            return Ok(None);
        }
    }
    let bytes = fs::read(backup.join(DATABASE_MANIFEST))
        .map_err(|error| format!("写しの目録を読めません: {error}"))?;
    let manifest: DatabaseBackup = serde_json::from_slice(&bytes)
        .map_err(|error| format!("写しの目録を読めません: {error}"))?;
    if shelf_database(paths)? != manifest.shelf {
        return Err("配信中の棚が更新前の写しと一致しません".into());
    }
    // どれかの写しが欠けていれば、いずれの DB にも手を付けない。
    for (_, name, was_present) in database_pairs(paths, &manifest) {
        if was_present {
            regular_file(&backup.join(name))
                .map_err(|error| format!("{name} の写しを読めません: {error}"))?;
        }
    }
    Ok(Some((backup, manifest)))
}

/// 写しへ戻す。WAL が残っていると新しい版での変更が混ざるので一緒に消す。
pub fn restore_databases(paths: &ServerPaths) -> Result<(), String> {
    let Some((backup, manifest)) = read_database_backup(paths)? else {
        return Ok(());
    };
    // 一時ファイルをすべて作ってから、巻き戻し先に手を付ける。
    // 置き換えの途中で中断しても、戻しの印から同じ写しでやり直せる。
    for (target, name, was_present) in database_pairs(paths, &manifest) {
        if was_present {
            let temporary = target.with_extension("restore");
            fs::copy(backup.join(name), &temporary)
                .map_err(|error| format!("{name} を戻せません: {error}"))?;
            sync_database(&temporary)?;
        }
    }
    for (target, name, _) in database_pairs(paths, &manifest) {
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = target.as_os_str().to_os_string();
            sidecar.push(suffix);
            remove_if_present(Path::new(&sidecar))
                .map_err(|error| format!("{name} の作業ファイルを消せません: {error}"))?;
        }
    }
    for (target, name, was_present) in database_pairs(paths, &manifest) {
        if was_present {
            fs::rename(target.with_extension("restore"), &target)
                .map_err(|error| format!("{name} を戻せません: {error}"))?;
        } else {
            // 更新した版が新しく作った状態 DB は、旧版へ戻すときは取り除く。
            remove_if_present(&target).map_err(|error| format!("{name} を消せません: {error}"))?;
        }
        sync_directory(target.parent().expect("DB の親"))
            .map_err(|error| format!("{name} の置き場を同期できません: {error}"))?;
    }
    Ok(())
}

fn shelf_database(paths: &ServerPaths) -> Result<Option<PathBuf>, String> {
    load_config(paths)?
        .shelf
        .map(|settings| {
            let root = settings.root(paths)?;
            regular_directory(&root).map_err(|error| error.to_string())?;
            let root = root
                .canonicalize()
                .map_err(|error| format!("配信中の棚を確かめられません: {error}"))?;
            Ok(root.join("library.db"))
        })
        .transpose()
}

fn database_pairs(
    paths: &ServerPaths,
    manifest: &DatabaseBackup,
) -> impl Iterator<Item = (PathBuf, &'static str, bool)> {
    let shelf = manifest
        .shelf
        .clone()
        .map(|path| (path, "library.db", true));
    std::iter::once((paths.store(), "state.db", manifest.state)).chain(shelf)
}

fn sync_database(path: &Path) -> Result<(), String> {
    File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("DB の写しを同期できません: {error}"))
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn history_paths() -> (crate::server::tests::TestRoot, ServerPaths) {
        let root = crate::server::tests::TestRoot::created("history");
        let paths = ServerPaths::new(&root.0).unwrap();
        fs::create_dir_all(paths.state()).unwrap();
        (root, paths)
    }

    #[test]
    fn stopping_keeps_the_first_reserved_action() {
        let (_root, paths) = history_paths();
        let controller = UpdateController::new(
            paths,
            UpdateSource::fixed(),
            UpdateConfig::default(),
            Box::new(|| {}),
        )
        .unwrap();
        controller
            .stop_for(AfterStop::RollBack("1.0.0".into()))
            .unwrap();
        assert_eq!(
            controller.stop_for(AfterStop::Apply("2.0.0".into())),
            Err(UPDATE_BUSY.into())
        );
        assert_eq!(
            controller.take_after_stop(),
            Some(AfterStop::RollBack("1.0.0".into()))
        );
    }

    #[test]
    fn manual_apply_reserves_before_the_worker_wakes() {
        let (_root, paths) = history_paths();
        let controller = UpdateController::new(
            paths,
            UpdateSource::fixed(),
            UpdateConfig::default(),
            Box::new(|| {}),
        )
        .unwrap();
        controller.request_now().unwrap();
        assert!(matches!(controller.status().phase, UpdatePhase::Checking));
        assert_eq!(controller.request_now(), Err(UPDATE_BUSY));
        assert_eq!(
            controller.stop_for(AfterStop::RollBack("1.0.0".into())),
            Err(UPDATE_BUSY.into())
        );
    }

    #[test]
    fn a_scheduled_request_checks_now_but_leaves_applying_to_the_window() {
        let (_root, paths) = history_paths();
        let controller = UpdateController::new(
            paths,
            UpdateSource::fixed(),
            UpdateConfig::default(),
            Box::new(|| {}),
        )
        .unwrap();
        controller.request_in_window();
        assert!(matches!(controller.status().phase, UpdatePhase::Checking));
        assert!(!controller.operation().immediate);

        // 時間帯を待っている更新に重ねて頼んでも、今すぐには変わらない。
        controller.operation().phase = UpdatePhase::Waiting("2.0.0".into());
        controller.request_in_window();
        assert!(!controller.operation().immediate);
        // 手で「今すぐ」と頼めば、待っている更新を早められる。
        controller.request_now().unwrap();
        assert!(controller.operation().immediate);
    }

    #[tokio::test]
    async fn shipment_opening_and_update_stop_exclude_each_other() {
        use crate::server::http::{combined_router, ServerContext};
        use crate::server::store::{Role, ServerStore};
        use reqwest::StatusCode;

        let (_root, paths) = history_paths();
        fs::write(paths.config(), "").unwrap();
        let store = Arc::new(ServerStore::open(&paths.store()).unwrap());
        let secret = store
            .issue_pairing(Role::Admin, Duration::from_secs(60))
            .unwrap();
        let admin = store.consume_pairing(&secret, "管理者").unwrap();
        let context = ServerContext::new(store, paths.root.clone(), None, None);
        let shipments = Arc::new(ShelfShipments::new(paths.clone(), None));
        context.set_shipments(paths.clone(), Arc::clone(&shipments));
        let controller = UpdateController::new(
            paths,
            UpdateSource::fixed(),
            UpdateConfig::default(),
            Box::new(|| {}),
        )
        .unwrap();
        context.set_update(Arc::clone(&controller));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/admin/shelf/shipments",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, combined_router(context))
                .await
                .unwrap()
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let open = || {
            client
                .post(&url)
                .bearer_auth(&admin.token)
                .json(&serde_json::json!({
                    "libraryId": uuid::Uuid::now_v7().to_string()
                }))
                .send()
        };
        let response = open().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();

        let version = "1.0.1";
        controller.reserve_check(false).unwrap();
        controller.operation().phase = UpdatePhase::Waiting(version.into());
        let after = AfterStop::Apply(version.into());
        assert!(!controller
            .stop_when_quiet(Some(&shipments), &after)
            .unwrap());
        shipments
            .abort(body["shipmentId"].as_str().unwrap())
            .unwrap();
        let opening = controller
            .shipment_access
            .get()
            .unwrap()
            .opening
            .lock()
            .await;
        assert!(!controller
            .stop_when_quiet(Some(&shipments), &after)
            .unwrap());
        drop(opening);
        let pending_shelf = controller.paths.root.join("shelf-pending.json");
        fs::write(&pending_shelf, "{}").unwrap();
        let stopped_before_publication = controller.stop_when_quiet(Some(&shipments), &after);
        fs::remove_file(pending_shelf).unwrap();
        assert!(!stopped_before_publication.unwrap());
        assert!(controller
            .stop_when_quiet(Some(&shipments), &after)
            .unwrap());

        let response = open().await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!shipments.busy());
        assert_eq!(controller.take_after_stop(), Some(after));
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn window_wait_observes_settings_changed_before_awaiting() {
        let (_root, paths) = history_paths();
        let controller = UpdateController::new(
            paths,
            UpdateSource::fixed(),
            UpdateConfig {
                auto: true,
                window: "03:00-06:00".into(),
            },
            Box::new(|| {}),
        )
        .unwrap();
        let wait = controller.wait_for_window(|| {
            // 待つ側が設定を読んでから、変更を待ち始めるまでの間に書き換える。
            controller.set_settings(false, "03:00-06:00").unwrap();
            NaiveTime::from_hms_opt(12, 0, 0).unwrap()
        });
        tokio::pin!(wait);
        assert_eq!(
            futures_util::poll!(&mut wait),
            std::task::Poll::Ready(false)
        );
    }

    #[test]
    fn update_record_writer_waits_then_keeps_concurrent_skip() {
        use std::sync::mpsc::{channel, RecvTimeoutError};

        let (_root, paths) = history_paths();
        let controller = UpdateController::new(
            paths.clone(),
            UpdateSource::fixed(),
            UpdateConfig::default(),
            Box::new(|| {}),
        )
        .unwrap();
        let lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.state().join("update.lock"))
            .unwrap();
        lock.lock().unwrap();

        let (started, starting) = channel();
        let (finished, finishing) = channel();
        let writer = std::thread::spawn(move || {
            started.send(()).unwrap();
            controller.record_error("更新の確認に失敗しました");
            finished.send(()).unwrap();
        });
        starting.recv().unwrap();
        let before_unlock = finishing.recv_timeout(Duration::from_millis(200));
        // 見張り役が、鍵を持っている間に飛ばす版を保存する。
        write_atomic(&paths.update_record(), br#"{"skipped":["1.1.0"]}"#).unwrap();
        drop(lock);
        writer.join().unwrap();

        assert_eq!(before_unlock, Err(RecvTimeoutError::Timeout));
        let record = UpdateRecord::load(&paths);
        assert_eq!(record.skipped, ["1.1.0"]);
        assert_eq!(
            record.last_error.as_deref(),
            Some("更新の確認に失敗しました")
        );
    }

    #[tokio::test]
    async fn update_record_keeps_watchdog_failure_when_no_update_is_found() {
        use axum::{routing::get, Json, Router};

        let (_root, paths) = history_paths();
        let reason = "版 1.1.0 に更新できなかったので、前の版に戻しました";
        UpdateRecord::try_edit(&paths, |record| {
            record.skip("1.1.0");
            record.note_failed_update(reason.into());
        })
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/manifest",
            get(|| async {
                Json(serde_json::json!({"version": VERSION, "url": "", "signature": ""}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let source = UpdateSource {
            manifest_url: format!("{base}/manifest"),
            download_prefix: base,
            public_key: String::new(),
        };
        let controller =
            UpdateController::new(paths, source, UpdateConfig::default(), Box::new(|| {})).unwrap();

        assert_eq!(controller.check_now().await.unwrap(), VERSION);
        let after_manual = controller.status().last_error;
        controller.request_now().unwrap();
        assert!(!controller.run_once(true, None).await);
        let after_automatic = controller.status().last_error;
        server.abort();

        assert_eq!(
            (after_manual.as_deref(), after_automatic.as_deref()),
            (Some(reason), Some(reason))
        );
        controller.record_error("更新の案内を取得できません");
        assert_eq!(controller.status().last_error.as_deref(), Some(reason));
        controller.retry_skipped();
        let status = controller.status();
        assert!(status.last_error.is_none());
        assert!(status.skipped.is_empty());
    }

    #[test]
    fn starting_records_what_happened_since_the_last_start() {
        let (_root, paths) = history_paths();
        let now = Utc::now();
        // 初めて動いたときと、同じ版で動き直したときは何も足さない。
        UpdateHistory::reconcile(&paths, "0.36.0", now);
        UpdateHistory::reconcile(&paths, "0.36.0", now);
        assert!(UpdateHistory::load(&paths).entries.is_empty());

        // 当てて、その版で動き出した。
        UpdateHistory::note_stopping(&paths, "0.37.0", false);
        UpdateHistory::reconcile(&paths, "0.37.0", now + chrono::Duration::seconds(30));
        let entry = UpdateHistory::load(&paths).entries[0].clone();
        assert_eq!(entry.result, "applied");
        assert_eq!(entry.from.as_deref(), Some("0.36.0"));
        assert_eq!(entry.to, "0.37.0");
        assert!(entry.downtime_seconds.is_some());

        // 当てたが、見張り役が戻した。
        UpdateHistory::note_stopping(&paths, "0.38.0", false);
        UpdateRecord::try_edit(&paths, |record| {
            record.note_failed_update("起動しませんでした".into());
        })
        .unwrap();
        UpdateHistory::reconcile(&paths, "0.37.0", now);
        let entry = UpdateHistory::load(&paths).entries[0].clone();
        assert_eq!(entry.result, "failed");
        assert_eq!(entry.to, "0.38.0");
        assert_eq!(entry.reason.as_deref(), Some("起動しませんでした"));

        // 手で戻した。
        UpdateHistory::note_stopping(&paths, "0.36.0", true);
        UpdateHistory::reconcile(&paths, "0.36.0", now);
        assert_eq!(UpdateHistory::load(&paths).entries[0].result, "rolled_back");
        assert_eq!(UpdateHistory::load(&paths).entries.len(), 3);
    }

    #[test]
    fn changing_the_settings_keeps_the_comments_and_other_values() {
        let (_root, paths) = history_paths();
        let text = "# 上のコメント\nlisten = \"127.0.0.1:1234\"\n\n[update]\n# 時間帯の説明\nauto = true\nwindow = \"03:00-06:00\"\n";
        fs::write(paths.config(), text).unwrap();
        write_update_settings(&paths, false, "01:30-02:30").unwrap();
        let written = fs::read_to_string(paths.config()).unwrap();
        assert!(written.contains("# 上のコメント"));
        assert!(written.contains("# 時間帯の説明"));
        assert!(written.contains("listen = \"127.0.0.1:1234\""));
        let config: crate::server::standalone::ServerConfig = toml::from_str(&written).unwrap();
        assert!(!config.update.auto);
        assert_eq!(config.update.window, "01:30-02:30");
    }

    #[test]
    fn changing_the_settings_keeps_comments_after_the_values() {
        let (_root, paths) = history_paths();
        let text = "[update]\nauto = true # 自動で当てる\nwindow = \"03:00-06:00\" # 夜中だけ\n";
        fs::write(paths.config(), text).unwrap();
        write_update_settings(&paths, false, "01:30-02:30").unwrap();
        let written = fs::read_to_string(paths.config()).unwrap();
        assert!(written.contains("auto = false # 自動で当てる"), "{written}");
        assert!(
            written.contains("window = \"01:30-02:30\" # 夜中だけ"),
            "{written}"
        );
    }

    #[test]
    fn settings_written_as_an_inline_table_can_be_changed() {
        let (_root, paths) = history_paths();
        let text =
            "listen = \"127.0.0.1:1234\"\nupdate = { auto = true, window = \"03:00-06:00\" }\n";
        fs::write(paths.config(), text).unwrap();
        // 読み込み側が受け取る書き方であることを先に確かめる。
        toml::from_str::<crate::server::standalone::ServerConfig>(text).unwrap();
        write_update_settings(&paths, false, "01:30-02:30").unwrap();
        let written = fs::read_to_string(paths.config()).unwrap();
        let config: crate::server::standalone::ServerConfig = toml::from_str(&written).unwrap();
        assert!(!config.update.auto);
        assert_eq!(config.update.window, "01:30-02:30");
    }

    #[test]
    fn settings_can_be_written_when_the_section_is_missing() {
        let (_root, paths) = history_paths();
        fs::write(paths.config(), "listen = \"127.0.0.1:1234\"\n").unwrap();
        write_update_settings(&paths, true, "02:00-04:00").unwrap();
        let written = fs::read_to_string(paths.config()).unwrap();
        let config: crate::server::standalone::ServerConfig = toml::from_str(&written).unwrap();
        assert_eq!(config.update.window, "02:00-04:00");
    }

    #[tokio::test]
    async fn deferred_package_is_reused_without_another_download() {
        check_deferred_package(false).await;
    }

    #[tokio::test]
    async fn deferred_package_with_invalid_signature_is_downloaded_again() {
        check_deferred_package(true).await;
    }

    async fn check_deferred_package(tamper: bool) {
        use axum::{routing::get, Json, Router};
        use base64::{engine::general_purpose::STANDARD, Engine};
        use std::sync::atomic::{AtomicUsize, Ordering};

        // 試験専用の鍵で署名した ZIP。中の実行ファイルは版番号を書いたテキスト。
        const ZIP: &str = "UEsDBBQAAAAIAMtmfF2gUjxkBwAAAAUAAAAPAAAAcGllcC1zZXJ2ZXIuZXhlM9Qz0DMEAFBLAQIUABQAAAAIAMtmfF2gUjxkBwAAAAUAAAAPAAAAAAAAAAAAAAAAAAAAAABwaWVwLXNlcnZlci5leGVQSwUGAAAAAAEAAQA9AAAANAAAAAAA";
        const KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Qga2V5ClJXUjBaWE4wTFhwcGNKSFFqRm5zeE0rbmxROWFMbXVHRU9pMlhrUlJTUnFhSkViNmVQNHBJa3huCg==";
        const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Qgc2lnbmF0dXJlClJVUjBaWE4wTFhwcGNLRU93MkxWWDZTbHRRWmlSaXNLRC9zaUh0MmJMY0NtaXdTbko0cHRSMkdLd1YxMVlibHN5TEdLaEFjTy8yNkdCM2VZZUUyTGltejJ0bmJJN3hTQjBBZz0KdHJ1c3RlZCBjb21tZW50OiBmaWxlOnBhY2thZ2UuemlwCktIbitzNU1vMSsvU2k3bS9HdjdiRTFEMllnMnpqY09EamVxNlRpV1BPemJGV01xZUFmcEs4RmxPcmRrZ09jZVlYWHhCSjZLVUR3R21aYkRpM0pGYUJnPT0K";
        let (_root, paths) = history_paths();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let url = format!("{base}/package");
        let downloads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&downloads);
        let app = Router::new()
            .route(
                "/manifest",
                get(move || {
                    let url = url.clone();
                    async move {
                        Json(serde_json::json!({
                            "version": "1.0.1", "url": url, "signature": SIGNATURE
                        }))
                    }
                }),
            )
            .route(
                "/package",
                get(move || {
                    let counted = Arc::clone(&counted);
                    async move {
                        counted.fetch_add(1, Ordering::Relaxed);
                        STANDARD.decode(ZIP).unwrap()
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let source = UpdateSource {
            manifest_url: format!("{base}/manifest"),
            download_prefix: base,
            public_key: KEY.into(),
        };
        let prepare = || {
            prepare_latest_with_timeouts(
                &source,
                &paths,
                "1.0.0",
                Duration::from_secs(3),
                Duration::from_secs(3),
                |path| fs::read_to_string(path).map_err(|error| error.to_string()),
            )
        };
        assert_eq!(prepare().await.unwrap().as_deref(), Some("1.0.1"));
        let cache = paths.versions().join(PREPARED_PACKAGE);
        if tamper {
            fs::write(&cache, b"changed").unwrap();
        }
        assert_eq!(prepare().await.unwrap().as_deref(), Some("1.0.1"));
        assert_eq!(
            downloads.load(Ordering::Relaxed),
            if tamper { 2 } else { 1 }
        );
        assert_eq!(fs::read(cache).unwrap(), STANDARD.decode(ZIP).unwrap());
        server.abort();
    }

    #[tokio::test]
    async fn stalled_update_requests_end_and_can_be_retried() {
        use axum::{body::Body, response::Response, routing::get, Json, Router};

        for phase in ["headers", "manifest", "oversized_manifest", "package"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let package_url = format!("{base}/package");
            let app = Router::new()
                .route(
                    "/manifest",
                    get(move || {
                        let package_url = package_url.clone();
                        async move {
                            if phase == "headers" {
                                return std::future::pending::<Response>().await;
                            }
                            if phase == "manifest" {
                                return Response::new(Body::from_stream(
                                    futures_util::stream::pending::<
                                        Result<axum::body::Bytes, std::io::Error>,
                                    >(),
                                ));
                            }
                            if phase == "oversized_manifest" {
                                return Response::new(Body::from(vec![
                                    b' ';
                                    MAX_MANIFEST_BYTES + 1
                                ]));
                            }
                            axum::response::IntoResponse::into_response(Json(serde_json::json!({
                                "version": "1.0.1", "url": package_url, "signature": "unused"
                            })))
                        }
                    }),
                )
                .route(
                    "/package",
                    get(|| async {
                        Response::new(Body::from_stream(futures_util::stream::pending::<
                            Result<axum::body::Bytes, std::io::Error>,
                        >()))
                    }),
                );
            let server = tokio::spawn(async move { axum::serve(listener, app).await });
            let dir = crate::server::tests::TestRoot::created("update_timeout");
            let paths = ServerPaths::new(&dir.0).unwrap();
            fs::create_dir_all(paths.state()).unwrap();
            let source = UpdateSource {
                manifest_url: format!("{base}/manifest"),
                download_prefix: base,
                public_key: PUBLIC_KEY.into(),
            };
            for _ in 0..2 {
                let result = tokio::time::timeout(
                    Duration::from_secs(3),
                    prepare_latest_with_timeouts(
                        &source,
                        &paths,
                        "1.0.0",
                        Duration::from_millis(100),
                        Duration::from_millis(100),
                        reported_version,
                    ),
                )
                .await;
                assert!(result.is_ok(), "更新が止まったままにならない: {phase}");
                assert!(
                    result.unwrap().is_err(),
                    "通信が止まった理由を返す: {phase}"
                );
                assert!(!paths.versions().join(".incoming").exists());
                assert!(!paths.versions().join(".incoming.zip").exists());
            }
            server.abort();
        }
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.31.0"));
        assert!(!is_newer("0.31.0", "0.31.0"));
        assert!(!is_newer("0.30.9", "0.31.0"));
        assert!(!is_newer("0.32", "0.31.0"));
        assert!(!is_newer("0.32.0-beta", "0.31.0"));
        assert!(!is_newer("../../evil", "0.31.0"));
    }

    #[test]
    fn the_window_may_cross_midnight() {
        let at = |text| NaiveTime::parse_from_str(text, "%H:%M").expect("time");
        let night = UpdateWindow::parse("03:00-06:00").expect("window");
        assert!(night.contains(at("03:00")));
        assert!(night.contains(at("05:59")));
        assert!(!night.contains(at("06:00")));
        assert!(!night.contains(at("02:59")));
        let crossing = UpdateWindow::parse("23:00-02:00").expect("window");
        assert!(crossing.contains(at("23:30")));
        assert!(crossing.contains(at("01:00")));
        assert!(!crossing.contains(at("02:00")));
        assert!(UpdateWindow::parse("3時から").is_err());
        assert!(UpdateWindow::parse("03:00").is_err());
    }

    #[test]
    fn the_embedded_key_matches_the_desktop_updater() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(
            config["plugins"]["updater"]["pubkey"].as_str(),
            Some(PUBLIC_KEY)
        );
    }

    #[test]
    fn the_source_is_fixed_to_this_repository() {
        assert_eq!(UpdateSource::fixed().manifest_url, MANIFEST_URL);
        assert_eq!(UpdateSource::fixed().download_prefix, DOWNLOAD_PREFIX);
        assert!(MANIFEST_URL.starts_with("https://github.com/1211snowmaple/piep/"));
    }

    fn zip_with(path: &Path, names: &[&str]) {
        let mut writer = zip::ZipWriter::new(File::create(path).expect("zip"));
        for name in names {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("start");
            writer.write_all(b"content").expect("write");
        }
        writer.finish().expect("finish");
    }

    #[test]
    fn only_known_files_are_extracted() {
        let dir = crate::server::tests::TestRoot::created("update");
        let package = dir.0.join("p.zip");
        let target = dir.0.join("out");
        let staging = dir.0.join(".incoming");
        let extract = || {
            extract_package(File::open(&package).unwrap(), &staging, |_| Ok(()))?;
            replace_package_directory(&staging, &target)
        };

        zip_with(
            &package,
            &["piep-server.exe", "MSVCP140.dll", "pdfium-licenses.txt"],
        );
        extract().expect("extract");
        assert!(target.join("piep-server.exe").exists());
        assert!(target.join("msvcp140.dll").exists());
        assert!(target.join("pdfium-licenses.txt").exists());

        // パスの外へ出る名前も、知らない名前も受け入れない。
        for bad in ["../piep-server.exe", "sub/piep-server.exe", "evil.dll"] {
            zip_with(&package, &["piep-server.exe", bad]);
            assert!(extract().is_err(), "{bad}");
        }
        zip_with(&package, &["msvcp140.dll"]);
        assert!(extract().is_err());
    }

    #[test]
    fn case_only_duplicates_are_rejected() {
        let dir = crate::server::tests::TestRoot::created("update_duplicate");
        let package = dir.0.join("p.zip");
        zip_with(&package, &["piep-server.exe", "PIEP-SERVER.EXE"]);
        let staging = dir.0.join(".incoming");
        let error =
            extract_package(File::open(package).unwrap(), &staging, |_| Ok(())).unwrap_err();
        assert!(error.contains("同じ名前"));
        assert!(!staging.exists());
    }

    #[test]
    fn failed_extraction_preserves_the_previous_directory() {
        let dir = crate::server::tests::TestRoot::created("update_extraction_failure");
        let package = dir.0.join("p.zip");
        let target = dir.0.join("out");
        let staging = dir.0.join(".incoming");
        fs::create_dir(&target).unwrap();
        fs::write(target.join(EXECUTABLE), b"previous executable").unwrap();
        fs::write(target.join("pdfium.dll"), b"previous library").unwrap();
        zip_with(&package, &[EXECUTABLE, "unknown.dll"]);

        assert!(
            extract_package(File::open(package).unwrap(), &staging, |_| {
                panic!("展開に失敗した配布物は版を照合しない");
            })
            .is_err()
        );
        assert_eq!(
            fs::read(target.join(EXECUTABLE)).unwrap(),
            b"previous executable"
        );
        assert_eq!(
            fs::read(target.join("pdfium.dll")).unwrap(),
            b"previous library"
        );
        assert!(!staging.exists());
    }

    #[test]
    fn failed_version_check_preserves_the_previous_directory() {
        let dir = crate::server::tests::TestRoot::created("update_version_failure");
        let package = dir.0.join("p.zip");
        let target = dir.0.join("1.0.1");
        let staging = dir.0.join(".incoming");
        fs::create_dir(&target).unwrap();
        fs::write(target.join(EXECUTABLE), b"previous executable").unwrap();
        zip_with(&package, &[EXECUTABLE, "pdfium.dll"]);

        let result = extract_package(File::open(package).unwrap(), &staging, |extracted| {
            assert!(extracted.join(EXECUTABLE).is_file());
            assert!(extracted.join("pdfium.dll").is_file());
            Err("版が違います".into())
        });
        assert_eq!(result.unwrap_err(), "版が違います");
        assert_eq!(
            fs::read(target.join(EXECUTABLE)).unwrap(),
            b"previous executable"
        );
        assert!(!target.join("pdfium.dll").exists());
        assert!(!staging.exists());
    }

    #[test]
    fn a_failed_rename_restores_the_previous_directory() {
        let dir = crate::server::tests::TestRoot::created("update_rename_failure");
        let target = dir.0.join("1.0.1");
        fs::create_dir(&target).unwrap();
        fs::write(target.join(EXECUTABLE), b"previous executable").unwrap();

        assert!(replace_package_directory(&dir.0.join("missing"), &target).is_err());
        assert_eq!(
            fs::read(target.join(EXECUTABLE)).unwrap(),
            b"previous executable"
        );
        assert!(!dir.0.join(".previous-1.0.1").exists());
    }

    #[test]
    fn an_interrupted_replacement_recovers_a_complete_directory() {
        let dir = crate::server::tests::TestRoot::created("update_interrupted");
        let previous = dir.0.join(".previous-1.0.1");
        let target = dir.0.join("1.0.1");
        for installed in [false, true] {
            fs::create_dir(&previous).unwrap();
            fs::write(previous.join(EXECUTABLE), b"previous executable").unwrap();
            fs::write(previous.join("pdfium.dll"), b"previous library").unwrap();
            if installed {
                fs::write(target.join(EXECUTABLE), b"new executable").unwrap();
                fs::write(target.join("pdfium.dll"), b"new library").unwrap();
            }

            recover_package_directories(&dir.0).unwrap();
            let name = if installed { "new" } else { "previous" };
            assert_eq!(
                fs::read(target.join(EXECUTABLE)).unwrap(),
                format!("{name} executable").as_bytes()
            );
            assert_eq!(
                fs::read(target.join("pdfium.dll")).unwrap(),
                format!("{name} library").as_bytes()
            );
            assert!(!previous.exists());
        }
    }

    #[test]
    fn version_query_fixture() {
        if std::env::var_os("PIEP_VERSION_QUERY_FIXTURE").is_some() {
            std::thread::sleep(Duration::from_millis(250));
            println!("1.0.1");
        }
    }

    #[tokio::test]
    async fn a_stalled_version_query_is_stopped() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "server::update::tests::version_query_fixture",
                "--nocapture",
            ])
            .env("PIEP_VERSION_QUERY_FIXTURE", "1");
        let error = query_version(command, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(error, "新しいバージョンの照会が時間内に終わりませんでした");
    }

    /// 試験専用の鍵で署名した例。配布には使っていない。
    const FIXTURE_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE4NTcyNzk5OTQ0MjQ2OTEKUldTUlJrS1VtU2RYcUxFd3N0bTcxVm9qUkN1L2MvL0RuVW5RZTJZdkI1SE80VDdWeWk5bnZrWjEK";
    const FIXTURE_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTUlJrS1VtU2RYcU9TYjRqdGgySnYyclBlRGRCTS9hNTcyUFoxOHlxb0FPZ29uYzZrWVdaT1lwQ3BWYjJiRCsvV1Bwc041NGY4dFY5eG94a3dMVmxqdlhVQ1JzWTVSV3dzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzNjExMjAyCWZpbGU6Zml4dHVyZS50eHQKMWdDYU04WWVsVTdCaXhwWWI1allOZko3UjVMaTBXb2ZQd3N2QlNjOUt5RnF4QjVUdE1NVUQ3UjdSRm1wTFRHazkreXJUTFJsN1dqd3pFZzU4QXc3QlE9PQo=";
    const FIXTURE_DATA: &[u8] = b"piep-server signature fixture
";

    #[test]
    fn only_the_signed_bytes_and_key_pass() {
        verify_signature(FIXTURE_KEY, FIXTURE_SIGNATURE, FIXTURE_DATA).expect("valid");
        assert!(verify_signature(FIXTURE_KEY, FIXTURE_SIGNATURE, b"changed").is_err());
        assert!(verify_signature(PUBLIC_KEY, FIXTURE_SIGNATURE, FIXTURE_DATA).is_err());
    }

    #[test]
    fn malformed_signatures_are_rejected() {
        assert!(verify_signature(PUBLIC_KEY, "not base64 ***", b"data").is_err());
        use base64::Engine;
        let garbage =
            base64::engine::general_purpose::STANDARD.encode("untrusted comment: x\nabc\n");
        assert!(verify_signature(PUBLIC_KEY, &garbage, b"data").is_err());
    }

    #[test]
    fn the_record_survives_unknown_fields() {
        let parsed: UpdateRecord =
            serde_json::from_str(r#"{"skipped":["1.0.0"],"fromTheFuture":1}"#).expect("record");
        assert_eq!(parsed.skipped, vec!["1.0.0".to_string()]);
    }

    #[test]
    fn databases_are_backed_up_and_restored() {
        let (_dir, paths) = database_paths("update");
        let database = rusqlite::Connection::open(paths.store()).expect("db");
        database
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .expect("seed");
        back_up_databases(&paths).expect("backup");
        database.execute("UPDATE t SET v = 2", []).expect("change");
        drop(database);

        restore_databases(&paths).expect("restore");
        let database = rusqlite::Connection::open(paths.store()).expect("db");
        let value: i64 = database
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("value");
        assert_eq!(value, 1);
    }
    #[test]
    fn rollback_removes_databases_that_did_not_exist_before_the_update() {
        let (_dir, paths) = database_paths("update_absent_db");
        back_up_databases(&paths).unwrap();

        let path = paths.store();
        fs::write(&path, b"created by the new version").unwrap();
        for suffix in ["state.db-wal", "state.db-shm"] {
            fs::write(paths.state().join(suffix), b"new journal").unwrap();
        }
        restore_databases(&paths).unwrap();
        restore_databases(&paths).unwrap();
        assert!(!path.exists());
        for sidecar in ["state.db-wal", "state.db-shm"] {
            assert!(!paths.state().join(sidecar).exists());
        }
    }

    #[test]
    fn a_missing_snapshot_is_refused_before_any_database_is_restored() {
        let (_dir, paths) = database_paths("update_missing_snapshot");
        rusqlite::Connection::open(paths.store())
            .unwrap()
            .execute_batch("CREATE TABLE t(v);")
            .unwrap();
        back_up_databases(&paths).unwrap();
        fs::write(paths.store(), b"current database").unwrap();
        fs::remove_file(paths.update_backup().join("state.db")).unwrap();

        assert!(restore_databases(&paths).is_err());
        assert_eq!(fs::read(paths.store()).unwrap(), b"current database");
    }

    fn database_paths(name: &str) -> (crate::server::tests::TestRoot, ServerPaths) {
        let root = crate::server::tests::TestRoot::created(name);
        let paths = ServerPaths::new(&root.0).unwrap();
        fs::create_dir_all(paths.state()).unwrap();
        fs::write(paths.config(), "").unwrap();
        (root, paths)
    }

    fn configure_shelf(paths: &ServerPaths, generation: Option<uuid::Uuid>) -> PathBuf {
        let library = uuid::Uuid::now_v7();
        let epoch = uuid::Uuid::now_v7();
        let (root, setting) = match generation {
            Some(id) => (
                paths.root.join("shelf-generations").join(id.to_string()),
                format!("generation = \"{id}\"\n"),
            ),
            None => (paths.shelf(), String::new()),
        };
        fs::write(
            paths.config(),
            format!("[shelf]\nlibrary_id = \"{library}\"\nsync_epoch = \"{epoch}\"\n{setting}"),
        )
        .unwrap();
        fs::create_dir_all(&root).unwrap();
        root.join("library.db")
    }

    #[test]
    fn a_failed_trial_rolls_back_the_served_shelf_migration() {
        let (_dir, paths) = database_paths("update_shelf_migration");
        for generation in [None, Some(uuid::Uuid::now_v7())] {
            let shelf = configure_shelf(&paths, generation);
            let database = rusqlite::Connection::open(&shelf).unwrap();
            database
                .execute_batch(
                    "PRAGMA journal_mode=WAL; PRAGMA user_version=1;
                    CREATE TABLE works(id); INSERT INTO works VALUES (42);",
                )
                .unwrap();
            back_up_databases(&paths).unwrap();
            write_atomic(&paths.previous_version(), b"1.0.0").unwrap();
            write_atomic(&paths.current_version(), b"1.1.0").unwrap();
            write_atomic(&paths.pending_version(), b"1.1.0").unwrap();

            database
                .execute_batch(
                    "PRAGMA user_version=2; ALTER TABLE works ADD COLUMN title;
                    DELETE FROM works;",
                )
                .unwrap();
            drop(database);
            super::super::watchdog::roll_back(&paths, "1.1.0", Some("準備確認に失敗")).unwrap();

            assert_eq!(current_version(&paths).as_deref(), Some("1.0.0"));
            assert!(!paths.pending_version().exists());
            let database = rusqlite::Connection::open(&shelf).unwrap();
            let version: i64 = database
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 1);
            let id: i64 = database
                .query_row("SELECT id FROM works", [], |row| row.get(0))
                .unwrap();
            assert_eq!(id, 42);
        }
    }

    #[test]
    fn a_changed_shelf_generation_refuses_restore_before_touching_either_database() {
        let (_dir, paths) = database_paths("update_shelf_changed");
        let previous = configure_shelf(&paths, Some(uuid::Uuid::now_v7()));
        for path in [paths.store(), previous.clone()] {
            rusqlite::Connection::open(path)
                .unwrap()
                .execute_batch("CREATE TABLE t(v);")
                .unwrap();
        }
        back_up_databases(&paths).unwrap();
        let current = configure_shelf(&paths, Some(uuid::Uuid::now_v7()));
        fs::write(paths.store(), b"current state").unwrap();
        fs::write(&previous, b"previous shelf").unwrap();
        fs::write(&current, b"current shelf").unwrap();

        assert!(restore_databases(&paths).is_err());
        assert_eq!(fs::read(paths.store()).unwrap(), b"current state");
        assert_eq!(fs::read(previous).unwrap(), b"previous shelf");
        assert_eq!(fs::read(current).unwrap(), b"current shelf");
    }

    #[test]
    fn a_failed_backup_keeps_the_previous_complete_backup() {
        let (_dir, paths) = database_paths("update_backup_interrupted");
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        database
            .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .unwrap();
        back_up_databases(&paths).unwrap();
        drop(database);
        fs::write(paths.store(), b"not a database").unwrap();

        assert!(back_up_databases(&paths).is_err());
        restore_databases(&paths).unwrap();
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        let value: i64 = database
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn an_interrupted_backup_swap_can_restore_the_retired_backup() {
        let (_dir, paths) = database_paths("update_backup_swap");
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        database
            .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .unwrap();
        back_up_databases(&paths).unwrap();
        database.execute("UPDATE t SET v = 2", []).unwrap();
        drop(database);
        // 前の写しを退けた直後に止まり、新しい写しはまだ公開されていない。
        fs::rename(
            paths.update_backup(),
            paths.state().join(".previous-update-backup"),
        )
        .unwrap();

        restore_databases(&paths).unwrap();
        let database = rusqlite::Connection::open(paths.store()).unwrap();
        let value: i64 = database
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn a_failed_shelf_restore_stage_leaves_state_database_untouched() {
        let (_dir, paths) = database_paths("update_restore_stage");
        let shelf = configure_shelf(&paths, None);
        for path in [paths.store(), shelf.clone()] {
            rusqlite::Connection::open(path)
                .unwrap()
                .execute_batch("CREATE TABLE t(v);")
                .unwrap();
        }
        back_up_databases(&paths).unwrap();
        fs::write(paths.store(), b"current state").unwrap();
        fs::create_dir(shelf.with_extension("restore")).unwrap();

        assert!(restore_databases(&paths).is_err());
        assert_eq!(fs::read(paths.store()).unwrap(), b"current state");

        fs::remove_dir(shelf.with_extension("restore")).unwrap();
        restore_databases(&paths).unwrap();
        restore_databases(&paths).unwrap();
        assert!(rusqlite::Connection::open(paths.store())
            .unwrap()
            .prepare("SELECT v FROM t")
            .is_ok());
    }
}
