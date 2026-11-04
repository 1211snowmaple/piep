//! 単独サーバーの自己更新。
//!
//! 本体が新しい版を取ってきて検証し、静かなときに終了コードで見張り役へ頼む。
//! 版の切り替えと戻しは見張り役（`watchdog.rs`）が行う。動いている実行ファイルは
//! 自分では置き換えられないからである。

use super::standalone::{ServerPaths, UpdateConfig};
use super::sync_receive::SyncReceiver;
use chrono::{Local, NaiveTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// この版のプログラムの版。更新の試験では組み立てるときに差し替える。
pub const VERSION: &str = match option_env!("PIEP_SERVER_TEST_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// 本体が「更新を当ててほしい」と見張り役へ伝える終了コード。
pub const EXIT_APPLY_UPDATE: i32 = 75;

/// `tauri.conf.json` の `plugins.updater.pubkey` と同じ鍵。デスクトップ版と同じ鍵で署名する。
const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc3N0U3OUI0OUJBQzY1QkYKUldTL1pheWJ0SGwrZHhMa2FaNlc2KzYwZmYyK1BoU01iS2paUzBxTExmMzFDVWRQZkxSVTRuemQK";
const MANIFEST_URL: &str =
    "https://github.com/1211snowmaple/piep/releases/latest/download/latest-server.json";
const DOWNLOAD_PREFIX: &str = "https://github.com/1211snowmaple/piep/releases/download/";

/// `manifest_url|download_prefix|public_key`。試験の組み立てだけで効く。
const TEST_SOURCE: Option<&str> = option_env!("PIEP_SERVER_TEST_SOURCE");

const EXECUTABLE: &str = "piep-server.exe";
/// zip の中で受け入れるファイル。これ以外があれば、その配布物は使わない。
const PACKAGE_FILES: [&str; 6] = [
    EXECUTABLE,
    "msvcp140.dll",
    "msvcp140_1.dll",
    "pdfium.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];
const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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
        if let Some(text) = TEST_SOURCE {
            let mut parts = text.split('|').map(str::to_string);
            if let (Some(manifest_url), Some(download_prefix), Some(public_key)) =
                (parts.next(), parts.next(), parts.next())
            {
                return Self {
                    manifest_url,
                    download_prefix,
                    public_key,
                };
            }
        }
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

pub fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, content)
        .map_err(|error| format!("{} を書けません: {error}", path.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("{} を置けません: {error}", path.display()))
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
    pub last_applied_at: Option<String>,
}

impl UpdateRecord {
    pub fn load(paths: &ServerPaths) -> Self {
        fs::read(paths.update_record())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &ServerPaths) {
        let result = serde_json::to_vec_pretty(self)
            .map_err(|error| error.to_string())
            .and_then(|bytes| write_atomic(&paths.update_record(), &bytes));
        if let Err(error) = result {
            log::warn!("更新の記録を書けません: {error}");
        }
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
    let data = fs::read(package).map_err(|error| format!("配布物を読めません: {error}"))?;
    let signature =
        fs::read_to_string(signature_file).map_err(|error| format!("署名を読めません: {error}"))?;
    verify_signature(&UpdateSource::fixed().public_key, &signature, &data)
}

/// zip を展開する。知らないファイルが 1 つでもあれば、配布物ごと受け入れない。
pub fn extract_package(package: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(package).map_err(|error| format!("配布物を開けません: {error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("配布物を読み取れません: {error}"))?;
    if destination.exists() {
        fs::remove_dir_all(destination)
            .map_err(|error| format!("展開先を空にできません: {error}"))?;
    }
    fs::create_dir_all(destination).map_err(|error| format!("展開先を作れません: {error}"))?;
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
        if !PACKAGE_FILES
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&name))
        {
            return Err(format!("配布物に知らないファイルがあります: {name}"));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err("配布物が大きすぎます".into());
        }
        has_executable |= name.eq_ignore_ascii_case(EXECUTABLE);
        let mut output = File::create(destination.join(name.to_ascii_lowercase()))
            .map_err(|error| format!("展開できません: {error}"))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("展開できません: {error}"))?;
    }
    if has_executable {
        Ok(())
    } else {
        Err(format!("配布物に {EXECUTABLE} がありません"))
    }
}

/// 展開したプログラムに版を尋ねる。署名を確かめたあとにだけ呼ぶ。
fn reported_version(executable: &Path) -> Result<String, String> {
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .map_err(|error| format!("新しい版を動かせません: {error}"))?;
    if !output.status.success() {
        return Err("新しい版が版を答えません".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn download(client: &reqwest::Client, url: &str, destination: &Path) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("新しい版を取ってこられません: {error}"))?;
    let mut output =
        File::create(destination).map_err(|error| format!("新しい版を保存できません: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("新しい版を取ってこられません: {error}"))?;
        total += chunk.len() as u64;
        if total > MAX_PACKAGE_BYTES {
            return Err("新しい版が大きすぎます".into());
        }
        output
            .write_all(&chunk)
            .map_err(|error| format!("新しい版を保存できません: {error}"))?;
    }
    Ok(())
}

/// 新しい版があれば取ってきて検証し、`versions\<版>\` に置く。置いた版を返す。
pub async fn prepare_latest(
    source: &UpdateSource,
    paths: &ServerPaths,
    current: &str,
) -> Result<Option<String>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("piep-server/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("更新を確かめられません: {error}"))?;
    let manifest: Manifest = client
        .get(&source.manifest_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("更新を確かめられません: {error}"))?
        .json()
        .await
        .map_err(|error| format!("更新の案内を読み取れません: {error}"))?;

    let mut record = UpdateRecord::load(paths);
    record.last_checked_at = Some(Utc::now().to_rfc3339());
    record.latest = Some(manifest.version.clone());
    record.save(paths);

    if !is_newer(&manifest.version, current) || record.skipped.contains(&manifest.version) {
        return Ok(None);
    }
    if !manifest.url.starts_with(&source.download_prefix) {
        return Err("更新の案内が決められた場所以外を指しています".into());
    }

    let installed = paths.version_dir(&manifest.version);
    if reported_version(&installed.join(EXECUTABLE))
        .ok()
        .as_deref()
        == Some(&manifest.version)
    {
        return Ok(Some(manifest.version));
    }

    let incoming = paths.versions().join(".incoming");
    if incoming.exists() {
        fs::remove_dir_all(&incoming).map_err(|error| format!("前の取得を消せません: {error}"))?;
    }
    fs::create_dir_all(&incoming).map_err(|error| format!("新しい版を保存できません: {error}"))?;
    let package = incoming.join("package.zip");
    let result = async {
        download(&client, &manifest.url, &package).await?;
        let data = fs::read(&package).map_err(|error| format!("配布物を読めません: {error}"))?;
        verify_signature(&source.public_key, &manifest.signature, &data)?;
        let extracted = incoming.join("extracted");
        extract_package(&package, &extracted)?;
        let reported = reported_version(&extracted.join(EXECUTABLE))?;
        if reported != manifest.version {
            return Err(format!(
                "案内の版 {} と中身の版 {reported} が違います",
                manifest.version
            ));
        }
        if installed.exists() {
            fs::remove_dir_all(&installed)
                .map_err(|error| format!("前の展開を消せません: {error}"))?;
        }
        fs::rename(&extracted, &installed)
            .map_err(|error| format!("新しい版を置けません: {error}"))?;
        Ok::<(), String>(())
    }
    .await;
    let _ = fs::remove_dir_all(&incoming);
    result?;
    Ok(Some(manifest.version))
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
    pub phase: UpdatePhase,
    pub latest: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
    pub last_applied_at: Option<String>,
}

/// 本体の中で動く更新の係。頼めるのは「確かめて当てる」だけで、ファイルや URL は渡せない。
pub struct UpdateController {
    paths: ServerPaths,
    source: UpdateSource,
    config: UpdateConfig,
    window: UpdateWindow,
    phase: Mutex<UpdatePhase>,
    requested: Notify,
    apply: Mutex<Option<String>>,
    stop: Mutex<Option<Box<dyn FnOnce() + Send>>>,
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
            config,
            window,
            phase: Mutex::new(UpdatePhase::Idle),
            requested: Notify::new(),
            apply: Mutex::new(None),
            stop: Mutex::new(Some(stop)),
        }))
    }

    pub fn status(&self) -> UpdateStatus {
        let record = UpdateRecord::load(&self.paths);
        UpdateStatus {
            auto: self.config.auto,
            phase: self
                .phase
                .lock()
                .map(|phase| phase.clone())
                .unwrap_or(UpdatePhase::Idle),
            latest: record.latest,
            last_checked_at: record.last_checked_at,
            last_error: record.last_error,
            last_applied_at: record.last_applied_at,
        }
    }

    /// 今すぐ確かめて当てる。時間帯を待たない。
    pub fn request_now(&self) {
        self.requested.notify_one();
    }

    /// 本体が止まったあとに当てる版。
    pub fn take_apply(&self) -> Option<String> {
        self.apply.lock().ok().and_then(|mut apply| apply.take())
    }

    fn set_phase(&self, phase: UpdatePhase) {
        if let Ok(mut current) = self.phase.lock() {
            *current = phase;
        }
    }

    fn record_error(&self, error: &str) {
        log::warn!("更新: {error}");
        let mut record = UpdateRecord::load(&self.paths);
        record.last_error = Some(error.to_string());
        record.save(&self.paths);
    }

    /// 送信が区切りに来るのを待つ。来なければ false。
    async fn wait_until_quiet(&self, receiver: Option<&SyncReceiver>) -> bool {
        let Some(receiver) = receiver else {
            return true;
        };
        let deadline = tokio::time::Instant::now() + QUIET_LIMIT;
        while receiver.status().session_open {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(QUIET_POLL).await;
        }
        true
    }

    /// 時間帯に入るか、今すぐと頼まれるまで待つ。頼まれたら true。
    async fn wait_for_window(&self) -> bool {
        loop {
            if self.window.contains(Local::now().time()) {
                return false;
            }
            tokio::select! {
                _ = self.requested.notified() => return true,
                _ = tokio::time::sleep(WINDOW_POLL) => {}
            }
        }
    }

    /// 1 回分。当てることになったら true を返し、本体を止める。
    async fn run_once(&self, mut now: bool, receiver: Option<&SyncReceiver>) -> bool {
        self.set_phase(UpdatePhase::Checking);
        let prepared = prepare_latest(&self.source, &self.paths, VERSION).await;
        let version = match prepared {
            Ok(Some(version)) => version,
            Ok(None) => {
                let mut record = UpdateRecord::load(&self.paths);
                record.last_error = None;
                record.save(&self.paths);
                self.set_phase(UpdatePhase::Idle);
                return false;
            }
            Err(error) => {
                self.record_error(&error);
                self.set_phase(UpdatePhase::Idle);
                return false;
            }
        };
        self.set_phase(UpdatePhase::Waiting(version.clone()));
        if !now && !self.config.auto {
            // 自動にしていなければ、頼まれるまで当てない。
            self.requested.notified().await;
            now = true;
        }
        if !now {
            self.wait_for_window().await;
        }
        if !self.wait_until_quiet(receiver).await {
            self.record_error("送信が終わらないので、更新を次に延ばします");
            self.set_phase(UpdatePhase::Idle);
            return false;
        }
        self.set_phase(UpdatePhase::Applying(version.clone()));
        if let Ok(mut apply) = self.apply.lock() {
            *apply = Some(version);
        }
        let stop = self.stop.lock().ok().and_then(|mut stop| stop.take());
        if let Some(stop) = stop {
            stop();
        }
        true
    }

    /// 本体が動いている間、回り続ける。
    pub async fn run(self: Arc<Self>, receiver: Option<Arc<SyncReceiver>>) {
        let mut delay = FIRST_CHECK_DELAY;
        loop {
            let now = if self.config.auto {
                tokio::select! {
                    _ = self.requested.notified() => true,
                    _ = tokio::time::sleep(delay) => false,
                }
            } else {
                self.requested.notified().await;
                true
            };
            if self.run_once(now, receiver.as_deref()).await {
                return;
            }
            delay = CHECK_INTERVAL;
        }
    }
}

/// 当てる前に DB を整合した形で写す。戻すときに使う。
pub fn back_up_databases(paths: &ServerPaths) -> Result<(), String> {
    let backup = paths.update_backup();
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|error| format!("前の写しを消せません: {error}"))?;
    }
    fs::create_dir_all(&backup).map_err(|error| format!("写しの置き場を作れません: {error}"))?;
    for (source, name) in database_pairs(paths) {
        if !source.exists() {
            continue;
        }
        let connection = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|error| format!("{name} を開けません: {error}"))?;
        connection
            .execute(
                "VACUUM INTO ?1",
                [backup.join(name).to_string_lossy().as_ref()],
            )
            .map_err(|error| format!("{name} を写せません: {error}"))?;
    }
    Ok(())
}

/// 写しへ戻す。WAL が残っていると古い変更が混ざるので一緒に消す。
pub fn restore_databases(paths: &ServerPaths) -> Result<(), String> {
    let backup = paths.update_backup();
    for (target, name) in database_pairs(paths) {
        let copy = backup.join(name);
        if !copy.exists() {
            continue;
        }
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = target.clone().into_os_string();
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            if sidecar.exists() {
                fs::remove_file(&sidecar)
                    .map_err(|error| format!("{name} の作業ファイルを消せません: {error}"))?;
            }
        }
        fs::copy(&copy, &target).map_err(|error| format!("{name} を戻せません: {error}"))?;
    }
    Ok(())
}

fn database_pairs(paths: &ServerPaths) -> [(PathBuf, &'static str); 2] {
    [(paths.database(), "piep.db"), (paths.store(), "state.db")]
}

#[cfg(test)]
mod tests {
    use super::*;

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

        zip_with(&package, &["piep-server.exe", "MSVCP140.dll"]);
        extract_package(&package, &target).expect("extract");
        assert!(target.join("piep-server.exe").exists());
        assert!(target.join("msvcp140.dll").exists());

        // パスの外へ出る名前も、知らない名前も受け入れない。
        for bad in ["../piep-server.exe", "sub/piep-server.exe", "evil.dll"] {
            zip_with(&package, &["piep-server.exe", bad]);
            assert!(extract_package(&package, &target).is_err(), "{bad}");
        }
        zip_with(&package, &["msvcp140.dll"]);
        assert!(extract_package(&package, &target).is_err());
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
        let dir = crate::server::tests::TestRoot::created("update");
        let paths = ServerPaths::new(&dir.0).expect("paths");
        fs::create_dir_all(paths.library()).expect("library");
        fs::create_dir_all(paths.state()).expect("state");
        let database = rusqlite::Connection::open(paths.database()).expect("db");
        database
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .expect("seed");
        back_up_databases(&paths).expect("backup");
        database.execute("UPDATE t SET v = 2", []).expect("change");
        drop(database);

        restore_databases(&paths).expect("restore");
        let database = rusqlite::Connection::open(paths.database()).expect("db");
        let value: i64 = database
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("value");
        assert_eq!(value, 1);
    }
}
