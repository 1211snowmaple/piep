//! 表紙を `<img>` でそのまま読めるようにする。
//!
//! base64 にして IPC で渡すと、転送量が 3 割増え、JavaScript の文字列として抱えるので
//! 小さな上限しか持てず、スクロールで戻るたびに取り直していた。ここではバイト列のまま
//! WebView へ返し、取得したものは端末に残す。次からはサーバーへ行かずに出せる。

use super::image_validation::validate_display_image;
use super::{
    ASSET_TIMEOUT, MAX_ASSET_BYTES, PcClientState, SavedConnection, authenticated_get_with_saved,
    bounded_response_bytes,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};
use tokio::sync::oneshot;

pub const SCHEME: &str = "pcimg";
const MAX_REMOTE_REQUESTS: usize = 6;
/// 表紙は 1 枚 20KB 前後。挿絵は端末の幅に縮めて数百 KB。合わせて読み返しの分を残せる。
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
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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
    index: Mutex<Option<CacheIndex>>,
}

#[derive(Default)]
struct CacheIndex {
    files: HashMap<PathBuf, (SystemTime, u64)>,
    oldest: BTreeSet<(SystemTime, PathBuf)>,
    bytes: u64,
}

impl CacheIndex {
    fn load(dir: &Path) -> std::io::Result<Self> {
        let mut index = Self::default();
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(index),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                index.insert(
                    entry.path(),
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                );
            }
        }
        Ok(index)
    }

    fn insert(&mut self, path: PathBuf, modified: SystemTime, size: u64) {
        if let Some((old_time, old_size)) = self.files.insert(path.clone(), (modified, size)) {
            self.oldest.remove(&(old_time, path.clone()));
            self.bytes -= old_size;
        }
        self.bytes += size;
        self.oldest.insert((modified, path));
    }

    fn remove(&mut self, path: &Path) {
        if let Some((time, size)) = self.files.remove(path) {
            self.oldest.remove(&(time, path.to_path_buf()));
            self.bytes -= size;
        }
    }

    fn trim_to(&mut self, target: u64) {
        let mut failed = Vec::new();
        while self.bytes > target {
            let Some((time, path)) = self.oldest.pop_first() else {
                break;
            };
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    failed.push((time, path));
                    continue;
                }
            }
            if let Some((_, size)) = self.files.remove(&path) {
                self.bytes -= size;
            }
        }
        self.oldest.extend(failed);
    }
}

impl CoverCacheState {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            limiter: Limiter::default(),
            index: Mutex::new(None),
        }
    }

    /// ディレクトリを一度だけ索引化し、以後は書き込み時に増分で管理する。
    pub fn trim(&self) {
        let mut index = self.index.lock().unwrap_or_else(|error| error.into_inner());
        if index.is_none() {
            *index = CacheIndex::load(&self.dir).ok();
        }
        let Some(index) = index.as_mut() else {
            return;
        };
        if index.bytes > MAX_CACHE_BYTES {
            index.trim_to(TRIMMED_CACHE_BYTES);
        }
    }

    fn write(&self, file: &Path, mime: &str, bytes: &[u8]) {
        self.write_with_limits(file, mime, bytes, MAX_CACHE_BYTES, TRIMMED_CACHE_BYTES);
    }

    fn write_with_limits(&self, file: &Path, mime: &str, bytes: &[u8], limit: u64, target: u64) {
        let size = (mime.len() + 1 + bytes.len()) as u64;
        if size > limit || fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        // 同じ画像の同時取得や裏の更新も、一時ファイルを含めて容量を予約する。
        let mut guard = self.index.lock().unwrap_or_else(|error| error.into_inner());
        if guard.is_none() {
            *guard = CacheIndex::load(&self.dir).ok();
        }
        let Some(index) = guard.as_mut() else {
            return;
        };
        if index.bytes.saturating_add(size) > limit {
            index.trim_to(target.min(limit - size));
        }
        if index.bytes.saturating_add(size) > limit {
            return;
        }
        let temp = file.with_extension("tmp");
        let written = (|| -> std::io::Result<()> {
            let mut target = fs::File::create(&temp)?;
            target.write_all(mime.as_bytes())?;
            target.write_all(b"\n")?;
            target.write_all(bytes)
        })();
        if written.is_ok() && fs::rename(&temp, file).is_ok() {
            index.remove(&temp);
            index.insert(file.to_path_buf(), SystemTime::now(), size);
        } else {
            let _ = fs::remove_file(&temp);
            index.remove(&temp);
            if let Ok(metadata) = fs::metadata(&temp) {
                index.insert(
                    temp,
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                );
            }
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
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// 受け付ける形を限る。ここで組み立てる以外の経路へは出ていかない。
fn parse_route(path: &str) -> Option<Route> {
    if path.len() > 2048 {
        return None;
    }
    let decoded = percent_decode(path.trim_start_matches('/'))?;
    let parts: Vec<&str> = decoded.split('/').collect();
    match parts.as_slice() {
        ["work", id, _version, _hash] => Some(Route::Remote {
            endpoint: format!("/api/v1/works/{}/cover", positive(id)?),
            refreshes: false,
        }),
        // 本文の挿絵。端末の幅を添える。版と内容の印は、改稿後に古い絵を出さないためのキャッシュ鍵。
        ["asset", id, asset, width, _version, _hash] => {
            let width: u32 = width
                .parse()
                .ok()
                .filter(|width| (1..=4096).contains(width))?;
            Some(Route::Remote {
                endpoint: format!(
                    "/api/v1/works/{}/assets/{}?width={width}",
                    positive(id)?,
                    positive(asset)?
                ),
                refreshes: false,
            })
        }
        ["work", id, _version, _hash, "large"] => Some(Route::Remote {
            endpoint: format!("/api/v1/works/{}/cover?size=large", positive(id)?),
            refreshes: false,
        }),
        [
            "entity",
            kind @ ("people" | "series"),
            source @ ("pixiv" | "fanbox"),
            key,
            rest @ ..,
        ] if rest.is_empty() || rest == ["banner"] => {
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
            Some(Route::Remote {
                endpoint: format!("/api/v1/entities/{kind}/cover?{}", query.finish()),
                refreshes: true,
            })
        }
        [
            "collection",
            id,
            slot @ ("file" | "tile-0" | "tile-1" | "tile-2" | "tile-3"),
            _revision,
        ] if safe_collection_id(id) => Some(Route::Remote {
            endpoint: format!("/api/v1/collections/{id}/cover/{slot}"),
            refreshes: false,
        }),
        ["saved", id, _generation] => Some(Route::SavedCover {
            work_id: positive(id)?,
        }),
        [
            "saved-entity",
            id,
            kind @ ("people" | "series"),
            _generation,
        ] => Some(Route::SavedEntity {
            work_id: positive(id)?,
            kind: (*kind).to_string(),
        }),
        ["saved-collection", id, collection, _generation] if safe_collection_id(collection) => {
            Some(Route::SavedCollection {
                work_id: positive(id)?,
                collection_id: (*collection).to_string(),
            })
        }
        _ => None,
    }
}

fn respond(status: StatusCode, mime: Option<&str>, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut builder = Response::builder().status(status);
    if let Some(mime) = mime {
        builder = builder
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "private, max-age=86400");
    }
    builder
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn failure(status: StatusCode) -> Response<Vec<u8>> {
    respond(status, None, Vec::new())
}

fn cache_path(dir: &Path, server: &str, path: &str) -> PathBuf {
    // 接続先が変わると同じ番号が別の作品を指す。接続先ごとに分ける。
    let mut hasher = Sha256::new();
    // APNG 検査のない旧版が保存した画像を、検査済みとして使い回さない。
    hasher.update(b"display-image-validation-v2\0");
    // 作者の丸い枠に背景画像を流用していた旧サーバーの返却を使い回さない。
    let person = percent_decode(path.trim_start_matches('/'))
        .is_some_and(|decoded| decoded.starts_with("entity/people/"));
    if person {
        hasher.update(b"profile-icon-only-v1\0");
    }
    hasher.update(server.as_bytes());
    hasher.update([0]);
    hasher.update(path.as_bytes());
    let name: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    dir.join(name)
}

fn read_cached(file: &Path) -> Option<(String, Vec<u8>)> {
    let data = fs::read(file).ok()?;
    let split = data.iter().position(|byte| *byte == b'\n')?;
    let mime = std::str::from_utf8(&data[..split]).ok()?.to_string();
    allowed_mime(&mime).then(|| (mime, data[split + 1..].to_vec()))
}

fn allowed_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" | "image/avif"
    )
}

async fn fetch_and_cache(
    state: &PcClientState,
    cache: &CoverCacheState,
    saved: &SavedConnection,
    generation: u64,
    file: &Path,
    endpoint: &str,
) -> Result<(String, Vec<u8>), StatusCode> {
    // 待ち行列の間に登録を変えた要求は送らない。送信先は、キャッシュの
    // パスを決めたときの接続そのものを使う。
    state
        .with_connection(generation, || Ok(()))
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let response = authenticated_get_with_saved(state, saved, endpoint, ASSET_TIMEOUT)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| allowed_mime(value))
        .ok_or(StatusCode::BAD_GATEWAY)?
        .to_string();
    let bytes = bounded_response_bytes(response, MAX_ASSET_BYTES)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    // 圧縮後は小さくても、展開すると端末のメモリを食い尽くす画像がある。
    validate_display_image(&bytes, &mime).map_err(|_| StatusCode::BAD_GATEWAY)?;
    // 通信中の A→B→A も世代で見分ける。照合から書き込みまで登録変更を
    // 挟ませず、古い返事を別の接続の画像として残さない。
    state
        .with_connection(generation, || {
            cache.write(file, &mime, &bytes);
            Ok(())
        })
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok((mime, bytes))
}

async fn serve_remote<R: Runtime>(
    app: &AppHandle<R>,
    path: &str,
    endpoint: &str,
    refreshes: bool,
) -> Response<Vec<u8>> {
    let cache = app.state::<CoverCacheState>();
    let state = app.state::<PcClientState>();
    let Ok((generation, Some(saved))) = state.connection_snapshot() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let file = cache_path(&cache.dir, &saved.url, path);
    let Ok(cached) = state.with_connection(generation, || Ok(read_cached(&file))) else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    if let Some((mime, bytes)) = cached {
        let stale = refreshes
            && fs::metadata(&file)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > ENTITY_REFRESH_AFTER);
        if stale {
            // 今あるものを先に出し、次に開いたときのために取り直す。
            let (app, endpoint) = (app.clone(), endpoint.to_string());
            tauri::async_runtime::spawn(async move {
                let cache = app.state::<CoverCacheState>();
                let _permit = cache.limiter.acquire().await;
                let state = app.state::<PcClientState>();
                let _ = fetch_and_cache(&state, &cache, &saved, generation, &file, &endpoint).await;
            });
        }
        return respond(StatusCode::OK, Some(&mime), bytes);
    }
    let _permit = cache.limiter.acquire().await;
    // 待っているあいだに同じ表紙が届いていることがある。
    let Ok(cached) = state.with_connection(generation, || Ok(read_cached(&file))) else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    if let Some((mime, bytes)) = cached {
        return respond(StatusCode::OK, Some(&mime), bytes);
    }
    match fetch_and_cache(&state, &cache, &saved, generation, &file, endpoint).await {
        Ok((mime, bytes)) => respond(StatusCode::OK, Some(&mime), bytes),
        Err(status) => failure(status),
    }
}

async fn serve<R: Runtime>(app: &AppHandle<R>, path: &str) -> Response<Vec<u8>> {
    use super::offline::{OfflineState, saved_collection_cover, saved_cover, saved_entity_cover};
    let Some(route) = parse_route(path) else {
        return failure(StatusCode::NOT_FOUND);
    };
    let saved = match route {
        Route::Remote {
            endpoint,
            refreshes,
        } => return serve_remote(app, path, &endpoint, refreshes).await,
        Route::SavedCover { work_id } => saved_cover(&app.state::<OfflineState>(), work_id).await,
        Route::SavedEntity { work_id, kind } => {
            saved_entity_cover(&app.state::<OfflineState>(), work_id, &kind).await
        }
        Route::SavedCollection {
            work_id,
            collection_id,
        } => saved_collection_cover(&app.state::<OfflineState>(), work_id, &collection_id).await,
    };
    match saved {
        Ok((mime, bytes)) => respond(StatusCode::OK, Some(&mime), bytes),
        Err(_) => failure(StatusCode::NOT_FOUND),
    }
}

pub fn handle<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    let path = request.uri().path().to_string();
    tauri::async_runtime::spawn(async move {
        responder.respond(serve(&app, &path).await);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCipher;
    impl crate::secret_cipher::SecretCipher for TestCipher {
        fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, ()> {
            Ok(plain.to_vec())
        }
        fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, ()> {
            Ok(sealed.to_vec())
        }
    }

    fn connection(url: String) -> SavedConnection {
        SavedConnection {
            url,
            fingerprint: None,
            certificate_der: None,
            token: "dummy".into(),
            device_id: "dummy".into(),
            device_name: "dummy".into(),
            capabilities: vec![],
        }
    }

    #[test]
    fn queued_image_requests_do_not_follow_a_new_connection() {
        tauri::async_runtime::block_on(async {
            let root =
                std::env::temp_dir().join(format!("piep-cover-queue-{}", uuid::Uuid::new_v4()));
            let state =
                PcClientState::new(root.join("connection"), std::sync::Arc::new(TestCipher));
            let saved = connection("https://old.invalid".into());
            *state.saved.lock().unwrap() = Some(connection("https://new.invalid".into()));
            *state.connection_generation.lock().unwrap() = 1;
            let cache = CoverCacheState::new(root.join("covers"));
            let file = cache_path(&cache.dir, &saved.url, "/work/1");
            assert_eq!(
                fetch_and_cache(&state, &cache, &saved, 0, &file, "/api/v1/works/1/cover")
                    .await
                    .unwrap_err(),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert!(!root.exists());
        });
    }

    #[test]
    fn an_image_received_after_registration_changes_is_not_cached() {
        use std::io::{Read, Write};
        tauri::async_runtime::block_on(async {
            for change in [false, true] {
                let root = std::env::temp_dir()
                    .join(format!("piep-cover-flight-{}", uuid::Uuid::new_v4()));
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let saved = connection(format!("http://{}", listener.local_addr().unwrap()));
                let state = std::sync::Arc::new(PcClientState::new(
                    root.join("connection"),
                    std::sync::Arc::new(TestCipher),
                ));
                *state.saved.lock().unwrap() = Some(saved.clone());
                *state.client.lock().unwrap() = Some((
                    format!("{}|", saved.url),
                    reqwest::Client::builder()
                        .no_proxy()
                        .timeout(Duration::from_secs(3))
                        .build()
                        .unwrap(),
                ));
                let changed_state = state.clone();
                let server = std::thread::spawn(move || {
                    let (mut socket, _) = listener.accept().unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = [0; 4096];
                    let length = socket.read(&mut request).unwrap();
                    assert!(
                        String::from_utf8_lossy(&request[..length])
                            .starts_with("GET /api/v1/works/1/cover ")
                    );
                    if change {
                        *changed_state.connection_generation.lock().unwrap() += 1;
                    }
                    let image = include_bytes!("fixtures/apng-2-frames.png");
                    write!(socket, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n", image.len()).unwrap();
                    socket.write_all(image).unwrap();
                });
                let cache = CoverCacheState::new(root.join("covers"));
                let file = cache_path(&cache.dir, &saved.url, "/work/1");
                let result =
                    fetch_and_cache(&state, &cache, &saved, 0, &file, "/api/v1/works/1/cover")
                        .await;
                server.join().unwrap();
                if change {
                    assert_eq!(result.unwrap_err(), StatusCode::SERVICE_UNAVAILABLE);
                    assert!(!file.exists());
                } else {
                    let (mime, bytes) = result.unwrap();
                    assert_eq!(read_cached(&file).unwrap(), (mime, bytes));
                    fs::remove_dir_all(root).unwrap();
                }
            }
        });
    }

    #[test]
    fn routes_only_accept_known_shapes() {
        assert_eq!(
            parse_route("/work%2F12%2F3%2Fabc"),
            Some(Route::Remote {
                endpoint: "/api/v1/works/12/cover".into(),
                refreshes: false
            })
        );
        assert_eq!(
            parse_route("/work%2F12%2F3%2Fabc%2Flarge"),
            Some(Route::Remote {
                endpoint: "/api/v1/works/12/cover?size=large".into(),
                refreshes: false
            })
        );
        assert_eq!(
            parse_route("/entity%2Fpeople%2Fpixiv%2Fa%252Fb"),
            Some(Route::Remote {
                endpoint: "/api/v1/entities/people/cover?source=pixiv&key=a%2Fb".into(),
                refreshes: true
            })
        );
        assert_eq!(
            parse_route("/entity%2Fpeople%2Fpixiv%2Fa%2Fbanner"),
            Some(Route::Remote {
                endpoint: "/api/v1/entities/people/cover?source=pixiv&key=a&variant=banner".into(),
                refreshes: true
            })
        );
        assert_eq!(
            parse_route("/collection%2Fabc-1%2Ftile-2%2F4"),
            Some(Route::Remote {
                endpoint: "/api/v1/collections/abc-1/cover/tile-2".into(),
                refreshes: false
            })
        );
        assert_eq!(
            parse_route("/asset%2F12%2F5%2F1080%2F3%2Fabc"),
            Some(Route::Remote {
                endpoint: "/api/v1/works/12/assets/5?width=1080".into(),
                refreshes: false
            })
        );
        assert_eq!(parse_route("/asset%2F12%2F5%2F99999%2F3%2Fabc"), None);
        assert_eq!(parse_route("/asset%2F12%2F-5%2F1080%2F3%2Fabc"), None);
        assert_eq!(
            parse_route("/saved%2F7%2F99"),
            Some(Route::SavedCover { work_id: 7 })
        );
        for bad in [
            "/work%2F0%2F1%2Fa",
            "/work%2F-1%2F1%2Fa",
            "/entity%2Fpeople%2Fother%2Fa",
            "/entity%2Fother%2Fpixiv%2Fa",
            "/collection%2F..%2Ffile%2F1",
            "/collection%2Fa%2Ftile-4%2F1",
            "/etc%2Fpasswd",
            "/%zz",
        ] {
            assert_eq!(parse_route(bad), None, "{bad}");
        }
    }

    #[test]
    fn cached_files_are_scoped_by_server() {
        let dir = Path::new("covers");
        assert_ne!(
            cache_path(dir, "https://a/", "/work%2F1"),
            cache_path(dir, "https://b/", "/work%2F1")
        );
    }

    #[test]
    fn only_person_images_skip_the_old_cache() {
        // 旧版の鍵を再現して、作者の画像だけが別の鍵になることを確かめる。
        let old_key = |path: &str| {
            let mut hasher = Sha256::new();
            hasher.update(b"display-image-validation-v2\0");
            hasher.update(b"https://a/");
            hasher.update([0]);
            hasher.update(path.as_bytes());
            let name: String = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            Path::new("covers").join(name)
        };
        let dir = Path::new("covers");
        let person = "/entity%2Fpeople%2Fpixiv%2Fa";
        assert_ne!(cache_path(dir, "https://a/", person), old_key(person));
        for path in ["/entity%2Fseries%2Fpixiv%2Fpeople", "/work%2F1"] {
            assert_eq!(cache_path(dir, "https://a/", path), old_key(path), "{path}");
        }
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
        CacheIndex::load(&dir).unwrap().trim_to(100);
        assert!(!dir.join("old").exists());
        assert!(dir.join("new").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_writes_and_refreshes_stay_within_the_budget() {
        let dir = std::env::temp_dir().join(format!("piep-cover-budget-{}", uuid::Uuid::new_v4()));
        let cache = std::sync::Arc::new(CoverCacheState::new(dir.clone()));
        cache.trim();
        for number in 0..30 {
            let file = dir.join(format!("image-{}", number % 8));
            cache.write_with_limits(&file, "image/png", &[number; 100], 512, 384);
            let actual: u64 = fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().metadata().unwrap().len())
                .sum();
            assert!(actual <= 512);
            assert_eq!(cache.index.lock().unwrap().as_ref().unwrap().bytes, actual);
        }
        let threads: Vec<_> = (0..8)
            .map(|number| {
                let cache = cache.clone();
                std::thread::spawn(move || {
                    cache.write_with_limits(
                        &cache.dir.join("shared"),
                        "image/png",
                        &[number; 100],
                        512,
                        384,
                    );
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let blocked = dir.join("blocked");
        fs::create_dir(&blocked).unwrap();
        cache.write_with_limits(&blocked, "image/png", &[0; 100], 512, 384);
        assert!(!blocked.with_extension("tmp").exists());
        let actual: u64 = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let metadata = entry.unwrap().metadata().unwrap();
                metadata.is_file().then_some(metadata.len())
            })
            .sum();
        assert!(actual <= 512);
        assert_eq!(cache.index.lock().unwrap().as_ref().unwrap().bytes, actual);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_indexing_is_retried_and_orphan_temporary_bytes_are_accounted_for() {
        let root = std::env::temp_dir().join(format!("piep-cover-index-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let dir = root.join("covers");
        fs::write(&dir, b"a file cannot be scanned as a cache directory").unwrap();
        let cache = CoverCacheState::new(dir.clone());
        cache.trim();
        assert!(cache.index.lock().unwrap().is_none());
        cache.write_with_limits(&dir.join("image"), "image/png", &[1; 100], 512, 384);
        assert!(cache.index.lock().unwrap().is_none());
        fs::remove_file(&dir).unwrap();
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("image.tmp"), [0; 64]).unwrap();
        cache.trim();
        assert_eq!(cache.index.lock().unwrap().as_ref().unwrap().bytes, 64);
        cache.write_with_limits(&dir.join("image"), "image/png", &[1; 100], 512, 384);
        assert!(!dir.join("image.tmp").exists());
        assert_eq!(
            cache.index.lock().unwrap().as_ref().unwrap().bytes,
            fs::metadata(dir.join("image")).unwrap().len()
        );
        fs::remove_dir_all(root).unwrap();
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
