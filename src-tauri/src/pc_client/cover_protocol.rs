//! 表紙を `<img>` でそのまま読めるようにする。
//!
//! base64 にして IPC で渡すと、転送量が 3 割増え、JavaScript の文字列として抱えるので
//! 小さな上限しか持てず、スクロールで戻るたびに取り直していた。ここではバイト列のまま
//! WebView へ返し、取得したものは端末に残す。次からはサーバーへ行かずに出せる。

use super::{authenticated_get_with_timeout, bounded_response_bytes, PcClientState, ASSET_TIMEOUT, MAX_ASSET_BYTES};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};
use tokio::sync::oneshot;

pub const SCHEME: &str = "pcimg";
const MAX_REMOTE_REQUESTS: usize = 6;
/// 表紙は 1 枚 20KB 前後。一万枚強ぶん残せる。
const MAX_CACHE_BYTES: u64 = 300 * 1024 * 1024;
const TRIMMED_CACHE_BYTES: u64 = 240 * 1024 * 1024;
/// 版を持たない作者・シリーズの画像は、古くなったら裏で取り直す。
const ENTITY_REFRESH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Remote { endpoint: String, refreshes: bool },
    SavedCover { work_id: i64 },
    SavedEntity { work_id: i64, kind: String },
    SavedCollection { work_id: i64, collection_id: String },
}

/// 取り直しを新しい要求から順に回す。速くスクロールすると、画面から消えた行の要求が
/// 溜まる。古いものから処理すると、今見えている表紙がいちばん後回しになる。
#[derive(Default)]
struct Limiter {
    state: Mutex<(usize, Vec<oneshot::Sender<()>>)>,
}

struct Permit<'a>(&'a Limiter);

impl Limiter {
    async fn acquire(&self) -> Permit<'_> {
        let waiting = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.0 < MAX_REMOTE_REQUESTS {
                state.0 += 1;
                None
            } else {
                let (sender, receiver) = oneshot::channel();
                state.1.push(sender);
                Some(receiver)
            }
        };
        if let Some(receiver) = waiting {
            let _ = receiver.await;
        }
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|error| error.into_inner());
        while let Some(next) = state.1.pop() {
            if next.send(()).is_ok() {
                return;
            }
        }
        state.0 -= 1;
    }
}

pub struct CoverCacheState {
    dir: PathBuf,
    limiter: Limiter,
}

impl CoverCacheState {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, limiter: Limiter::default() }
    }

    /// 上限を超えていたら古いものから消す。起動時に裏で一度だけ呼ぶ。
    pub fn trim(&self) {
        trim_directory(&self.dir, MAX_CACHE_BYTES, TRIMMED_CACHE_BYTES);
    }
}

fn trim_directory(dir: &Path, limit: u64, target: u64) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| (metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|file| file.1).sum();
    if total <= limit {
        return;
    }
    files.sort_by_key(|file| file.0);
    for (_, size, path) in files {
        if total <= target {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total -= size;
        }
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = value.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn positive(value: &str) -> Option<i64> {
    value.parse().ok().filter(|id| *id > 0)
}

fn safe_collection_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 受け付ける形を限る。ここで組み立てる以外の経路へは出ていかない。
fn parse_route(path: &str) -> Option<Route> {
    if path.len() > 2048 {
        return None;
    }
    let decoded = percent_decode(path.trim_start_matches('/'))?;
    let parts: Vec<&str> = decoded.split('/').collect();
    match parts.as_slice() {
        ["work", id, _version, _hash] => Some(Route::Remote { endpoint: format!("/api/v1/works/{}/cover", positive(id)?), refreshes: false }),
        ["work", id, _version, _hash, "large"] => Some(Route::Remote { endpoint: format!("/api/v1/works/{}/cover?size=large", positive(id)?), refreshes: false }),
        ["entity", kind @ ("people" | "series"), source @ ("pixiv" | "fanbox"), key, rest @ ..] if rest.is_empty() || rest == ["banner"] => {
            let key = percent_decode(key)?;
            if key.is_empty() || key.chars().count() > 200 {
                return None;
            }
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("source", source);
            query.append_pair("key", &key);
            if !rest.is_empty() {
                query.append_pair("variant", "banner");
            }
            Some(Route::Remote { endpoint: format!("/api/v1/entities/{kind}/cover?{}", query.finish()), refreshes: true })
        }
        ["collection", id, slot @ ("file" | "tile-0" | "tile-1" | "tile-2" | "tile-3"), _revision] if safe_collection_id(id) => {
            Some(Route::Remote { endpoint: format!("/api/v1/collections/{id}/cover/{slot}"), refreshes: false })
        }
        ["saved", id, _generation] => Some(Route::SavedCover { work_id: positive(id)? }),
        ["saved-entity", id, kind @ ("people" | "series"), _generation] => Some(Route::SavedEntity { work_id: positive(id)?, kind: (*kind).to_string() }),
        ["saved-collection", id, collection, _generation] if safe_collection_id(collection) => Some(Route::SavedCollection { work_id: positive(id)?, collection_id: (*collection).to_string() }),
        _ => None,
    }
}

fn respond(status: StatusCode, mime: Option<&str>, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut builder = Response::builder().status(status);
    if let Some(mime) = mime {
        builder = builder.header(header::CONTENT_TYPE, mime).header(header::CACHE_CONTROL, "private, max-age=86400");
    }
    builder.body(body).unwrap_or_else(|_| Response::new(Vec::new()))
}

fn failure(status: StatusCode) -> Response<Vec<u8>> {
    respond(status, None, Vec::new())
}

fn cache_path(dir: &Path, server: &str, path: &str) -> PathBuf {
    // 接続先が変わると同じ番号が別の作品を指す。接続先ごとに分ける。
    let mut hasher = Sha256::new();
    hasher.update(server.as_bytes());
    hasher.update([0]);
    hasher.update(path.as_bytes());
    let name: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    dir.join(name)
}

fn read_cached(file: &Path) -> Option<(String, Vec<u8>)> {
    let data = fs::read(file).ok()?;
    let split = data.iter().position(|byte| *byte == b'\n')?;
    let mime = std::str::from_utf8(&data[..split]).ok()?.to_string();
    allowed_mime(&mime).then(|| (mime, data[split + 1..].to_vec()))
}

fn write_cached(dir: &Path, file: &Path, mime: &str, bytes: &[u8]) {
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let temp = file.with_extension(format!("{:x}.tmp", std::process::id()));
    let mut data = Vec::with_capacity(mime.len() + 1 + bytes.len());
    data.extend_from_slice(mime.as_bytes());
    data.push(b'\n');
    data.extend_from_slice(bytes);
    if fs::write(&temp, data).is_ok() && fs::rename(&temp, file).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

fn allowed_mime(mime: &str) -> bool {
    matches!(mime, "image/jpeg" | "image/png" | "image/webp" | "image/gif" | "image/avif")
}

async fn fetch<R: Runtime>(app: &AppHandle<R>, endpoint: &str) -> Result<(String, Vec<u8>), StatusCode> {
    let state = app.state::<PcClientState>();
    let response = authenticated_get_with_timeout(&state, endpoint, ASSET_TIMEOUT).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| allowed_mime(value))
        .ok_or(StatusCode::BAD_GATEWAY)?
        .to_string();
    let bytes = bounded_response_bytes(response, MAX_ASSET_BYTES).await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok((mime, bytes))
}

async fn serve_remote<R: Runtime>(app: &AppHandle<R>, path: &str, endpoint: &str, refreshes: bool) -> Response<Vec<u8>> {
    let cache = app.state::<CoverCacheState>();
    let Ok(Some(saved)) = app.state::<PcClientState>().load() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let file = cache_path(&cache.dir, &saved.url, path);
    if let Some((mime, bytes)) = read_cached(&file) {
        let stale = refreshes && fs::metadata(&file).and_then(|metadata| metadata.modified()).ok().and_then(|time| time.elapsed().ok()).is_some_and(|age| age > ENTITY_REFRESH_AFTER);
        if stale {
            // 今あるものを先に出し、次に開いたときのために取り直す。
            let (app, endpoint) = (app.clone(), endpoint.to_string());
            tauri::async_runtime::spawn(async move {
                let cache = app.state::<CoverCacheState>();
                let _permit = cache.limiter.acquire().await;
                if let Ok((mime, bytes)) = fetch(&app, &endpoint).await {
                    write_cached(&cache.dir, &file, &mime, &bytes);
                }
            });
        }
        return respond(StatusCode::OK, Some(&mime), bytes);
    }
    let _permit = cache.limiter.acquire().await;
    // 待っているあいだに同じ表紙が届いていることがある。
    if let Some((mime, bytes)) = read_cached(&file) {
        return respond(StatusCode::OK, Some(&mime), bytes);
    }
    match fetch(app, endpoint).await {
        Ok((mime, bytes)) => {
            write_cached(&cache.dir, &file, &mime, &bytes);
            respond(StatusCode::OK, Some(&mime), bytes)
        }
        Err(status) => failure(status),
    }
}

async fn serve<R: Runtime>(app: &AppHandle<R>, path: &str) -> Response<Vec<u8>> {
    use super::offline::{saved_collection_cover, saved_cover, saved_entity_cover, OfflineState};
    let Some(route) = parse_route(path) else {
        return failure(StatusCode::NOT_FOUND);
    };
    let saved = match route {
        Route::Remote { endpoint, refreshes } => return serve_remote(app, path, &endpoint, refreshes).await,
        Route::SavedCover { work_id } => saved_cover(&app.state::<OfflineState>(), work_id).await,
        Route::SavedEntity { work_id, kind } => saved_entity_cover(&app.state::<OfflineState>(), work_id, &kind).await,
        Route::SavedCollection { work_id, collection_id } => saved_collection_cover(&app.state::<OfflineState>(), work_id, &collection_id).await,
    };
    match saved {
        Ok((mime, bytes)) => respond(StatusCode::OK, Some(&mime), bytes),
        Err(_) => failure(StatusCode::NOT_FOUND),
    }
}

pub fn handle<R: Runtime>(context: UriSchemeContext<'_, R>, request: tauri::http::Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = context.app_handle().clone();
    let path = request.uri().path().to_string();
    tauri::async_runtime::spawn(async move {
        responder.respond(serve(&app, &path).await);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_accept_known_shapes() {
        assert_eq!(parse_route("/work%2F12%2F3%2Fabc"), Some(Route::Remote { endpoint: "/api/v1/works/12/cover".into(), refreshes: false }));
        assert_eq!(parse_route("/work%2F12%2F3%2Fabc%2Flarge"), Some(Route::Remote { endpoint: "/api/v1/works/12/cover?size=large".into(), refreshes: false }));
        assert_eq!(parse_route("/entity%2Fpeople%2Fpixiv%2Fa%252Fb"), Some(Route::Remote { endpoint: "/api/v1/entities/people/cover?source=pixiv&key=a%2Fb".into(), refreshes: true }));
        assert_eq!(parse_route("/entity%2Fpeople%2Fpixiv%2Fa%2Fbanner"), Some(Route::Remote { endpoint: "/api/v1/entities/people/cover?source=pixiv&key=a&variant=banner".into(), refreshes: true }));
        assert_eq!(parse_route("/collection%2Fabc-1%2Ftile-2%2F4"), Some(Route::Remote { endpoint: "/api/v1/collections/abc-1/cover/tile-2".into(), refreshes: false }));
        assert_eq!(parse_route("/saved%2F7%2F99"), Some(Route::SavedCover { work_id: 7 }));
        for bad in ["/work%2F0%2F1%2Fa", "/work%2F-1%2F1%2Fa", "/entity%2Fpeople%2Fother%2Fa", "/entity%2Fother%2Fpixiv%2Fa", "/collection%2F..%2Ffile%2F1", "/collection%2Fa%2Ftile-4%2F1", "/etc%2Fpasswd", "/%zz"] {
            assert_eq!(parse_route(bad), None, "{bad}");
        }
    }

    #[test]
    fn cached_files_are_scoped_by_server() {
        let dir = Path::new("covers");
        assert_ne!(cache_path(dir, "https://a/", "/work%2F1"), cache_path(dir, "https://b/", "/work%2F1"));
    }

    #[test]
    fn trimming_removes_the_oldest_first() {
        let dir = std::env::temp_dir().join(format!("piep-cover-trim-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["old", "new"] {
            fs::write(dir.join(name), vec![0u8; 100]).unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        trim_directory(&dir, 150, 100);
        assert!(!dir.join("old").exists());
        assert!(dir.join("new").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn limiter_serves_the_newest_waiter_first() {
        tauri::async_runtime::block_on(async {
            let limiter = Limiter::default();
            let mut held = Vec::new();
            for _ in 0..MAX_REMOTE_REQUESTS {
                held.push(limiter.acquire().await);
            }
            let (old_sender, mut old_receiver) = oneshot::channel();
            let (new_sender, mut new_receiver) = oneshot::channel();
            limiter.state.lock().unwrap().1.push(old_sender);
            limiter.state.lock().unwrap().1.push(new_sender);
            held.pop();
            assert!(new_receiver.try_recv().is_ok());
            assert!(old_receiver.try_recv().is_err());
        });
    }
}
