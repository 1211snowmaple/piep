//! アプリ内更新。新しい APK を取ってきて検証し、OS のインストーラーへ渡せる形で置く。
//!
//! 本当の守りは OS の検査（同じ署名鍵で、versionCode が増えていること）である。
//! SHA-256 は壊れた取得を捨てるためにある。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MANIFEST_URL: &str =
    "https://github.com/1211snowmaple/piep-android/releases/latest/download/latest-android.json";
const DOWNLOAD_PREFIX: &str = "https://github.com/1211snowmaple/piep-android/releases/download/";
/// `manifest_url|download_prefix`。試験の組み立てだけで効く。
const TEST_SOURCE: Option<&str> = option_env!("PIEP_ANDROID_TEST_UPDATE_SOURCE");
const MAX_APK_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const UPDATE_FILE: &str = "update.apk";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    manifest_url: String,
    download_prefix: String,
}

fn source() -> Source {
    if let Some((manifest_url, download_prefix)) = TEST_SOURCE.and_then(|text| text.split_once('|'))
    {
        return Source {
            manifest_url: manifest_url.to_string(),
            download_prefix: download_prefix.to_string(),
        };
    }
    Source {
        manifest_url: MANIFEST_URL.to_string(),
        download_prefix: DOWNLOAD_PREFIX.to_string(),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: String,
    url: String,
    sha256: String,
    size: u64,
}

impl Manifest {
    fn validate(&self, source: &Source) -> Result<(), String> {
        if parse_version(&self.version).is_none() {
            return Err("更新の案内を読み取れません".into());
        }
        if !self.url.starts_with(&source.download_prefix) {
            return Err("更新の案内が決められた場所以外を指しています".into());
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("更新の案内を読み取れません".into());
        }
        if self.size == 0 || self.size > MAX_APK_BYTES {
            return Err("更新の案内の大きさが正しくありません".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    current: String,
    latest: String,
    available: bool,
    size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: u64,
    total: u64,
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let parsed = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(parsed)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn updates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|cache| cache.join("updates"))
        .map_err(|_| "更新の置き場が分かりません".to_string())
}

async fn fetch_manifest(client: &reqwest::Client, source: &Source) -> Result<Manifest, String> {
    let response = client
        .get(&source.manifest_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "更新を確かめられません。通信状態を確認してください".to_string())?;
    let bytes = crate::pc_client::bounded_response_bytes(response, MAX_MANIFEST_BYTES)
        .await
        .map_err(|_| "更新の案内を読み取れません".to_string())?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| "更新の案内を読み取れません".to_string())?;
    manifest.validate(source)?;
    Ok(manifest)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn file_sha256(path: &Path) -> Option<String> {
    let mut file = tokio::fs::File::open(path).await.ok()?;
    if file.metadata().await.ok()?.len() > MAX_APK_BYTES {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let size = file.read(&mut buffer).await.ok()?;
        if size == 0 {
            return Some(hex(&hasher.finalize()));
        }
        total += size as u64;
        if total > MAX_APK_BYTES {
            return None;
        }
        hasher.update(&buffer[..size]);
    }
}

#[tauri::command]
pub fn app_update_installed_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn app_update_check(app: AppHandle) -> Result<AppUpdateInfo, String> {
    let source = source();
    let client = crate::pc_client::update_client(TEST_SOURCE.is_some())?;
    let manifest = fetch_manifest(&client, &source).await?;
    let current = app.package_info().version.to_string();
    let available = is_newer(&manifest.version, &current);
    if !available {
        // 当て終わった APK を残さない。
        if let Ok(updates) = updates_dir(&app) {
            let _ = tokio::fs::remove_dir_all(updates).await;
        }
    }
    Ok(AppUpdateInfo {
        current,
        latest: manifest.version,
        available,
        size: manifest.size,
    })
}

/// 新しい APK を取ってきて検証し、キャッシュに置く。インストーラーへ渡すのは画面側の仕事。
#[tauri::command]
pub async fn app_update_download(app: AppHandle) -> Result<(), String> {
    let source = source();
    let client = crate::pc_client::update_client(TEST_SOURCE.is_some())?;
    let manifest = fetch_manifest(&client, &source).await?;
    if !is_newer(&manifest.version, &app.package_info().version.to_string()) {
        return Err("すでに最新の版です".into());
    }
    let updates = updates_dir(&app)?;
    tokio::fs::create_dir_all(&updates)
        .await
        .map_err(|_| "更新の置き場を作れません".to_string())?;
    let target = updates.join(UPDATE_FILE);
    if file_sha256(&target).await.as_deref() == Some(manifest.sha256.to_lowercase().as_str()) {
        return Ok(());
    }

    let part = updates.join("update.part");
    let mut response = client
        .get(&manifest.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "新しい版を取ってこられません。通信状態を確認してください".to_string())?;
    let mut output = tokio::fs::File::create(&part)
        .await
        .map_err(|_| "新しい版を保存できません".to_string())?;
    let mut hasher = Sha256::new();
    let mut done = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "新しい版を取ってこられません。通信状態を確認してください".to_string())?
    {
        done += chunk.len() as u64;
        if done > manifest.size {
            let _ = tokio::fs::remove_file(&part).await;
            return Err("新しい版の大きさが案内と違います".into());
        }
        hasher.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .map_err(|_| "新しい版を保存できません。端末の空きを確認してください".to_string())?;
        let _ = app.emit(
            "app-update-progress",
            Progress {
                done,
                total: manifest.size,
            },
        );
    }
    output
        .flush()
        .await
        .map_err(|_| "新しい版を保存できません".to_string())?;
    drop(output);
    if done != manifest.size || !hex(&hasher.finalize()).eq_ignore_ascii_case(&manifest.sha256) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err("取ってきた版が壊れています。もう一度試してください".into());
    }
    tokio::fs::rename(&part, &target)
        .await
        .map_err(|_| "新しい版を置けません".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_apk_hash_is_checked_in_chunks() {
        tauri::async_runtime::block_on(async {
            let file =
                std::env::temp_dir().join(format!("piep-update-hash-{}", uuid::Uuid::new_v4()));
            let bytes = vec![0x5au8; 64 * 1024 + 19];
            tokio::fs::write(&file, &bytes).await.unwrap();
            assert_eq!(file_sha256(&file).await, Some(hex(&Sha256::digest(&bytes))));
            tokio::fs::remove_file(file).await.unwrap();
        });
    }

    #[test]
    fn oversized_update_manifests_are_rejected_before_parsing() {
        use std::io::{Read, Write};
        tauri::async_runtime::block_on(async {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let received = stream.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..received]).starts_with("GET /manifest ")
                );
                let body = format!(
                    "{}{}",
                    " ".repeat(MAX_MANIFEST_BYTES),
                    serde_json::json!({"version":"1.0.0", "url": format!("{DOWNLOAD_PREFIX}v1/piep.apk"), "sha256":"a".repeat(64), "size":10})
                );
                write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
                // 大きさを見て切ったクライアントには、残りが届かなくてよい。
                let _ = stream.write_all(body.as_bytes());
            });
            let mut source = source();
            source.manifest_url = format!("http://{address}/manifest");
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap();
            assert!(fetch_manifest(&client, &source).await.is_err());
            server.join().unwrap();
        });
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.3", "0.2.0"));
        assert!(!is_newer("0.3.0-beta", "0.2.0"));
    }

    fn manifest(url: &str) -> Manifest {
        Manifest {
            version: "1.0.0".into(),
            url: url.into(),
            sha256: "a".repeat(64),
            size: 10,
        }
    }

    #[test]
    fn the_destination_is_fixed() {
        let source = source();
        assert_eq!(source.manifest_url, MANIFEST_URL);
        assert!(
            manifest(&format!("{DOWNLOAD_PREFIX}v1.0.0/piep.apk"))
                .validate(&source)
                .is_ok()
        );
        assert!(
            manifest("https://example.com/piep.apk")
                .validate(&source)
                .is_err()
        );
        assert!(
            manifest("https://github.com/someone/else/releases/download/v1/piep.apk")
                .validate(&source)
                .is_err()
        );
    }

    #[test]
    fn a_broken_manifest_is_refused() {
        let source = source();
        let url = format!("{DOWNLOAD_PREFIX}v1.0.0/piep.apk");
        let mut bad = manifest(&url);
        bad.version = "new".into();
        assert!(bad.validate(&source).is_err());
        let mut bad = manifest(&url);
        bad.sha256 = "xyz".into();
        assert!(bad.validate(&source).is_err());
        let mut bad = manifest(&url);
        bad.size = 0;
        assert!(bad.validate(&source).is_err());
        let mut bad = manifest(&url);
        bad.size = MAX_APK_BYTES + 1;
        assert!(bad.validate(&source).is_err());
    }
}
