use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinSet;

pub(crate) const MAX_ASSET_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_PROFILE_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ASSET_IMAGE_PIXELS: u64 = 100_000_000;

/// アセットダウンロード対象
#[derive(Debug, Clone)]
pub struct DownloadTarget {
    pub url: String,
    pub sub_folder: &'static str, // "illustrations", "files", "cover"
    pub filename: String,
}

/// URLからクエリパラメータを排除した上で、安全に拡張子を抽出する
pub fn extract_extension(url: &str, default: &str) -> String {
    let path_part = url.split('?').next().unwrap_or(url);
    let filename = path_part.split('/').next_back().unwrap_or("");
    if let Some(pos) = filename.rfind('.') {
        let ext = &filename[pos + 1..];
        if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_alphanumeric()) {
            return ext.to_lowercase();
        }
    }
    default.to_string()
}

/// FANBOX の添付ファイルの保存名。
///
/// 名前は creator が付けるもので、一つの投稿に同名の添付が二つあることが
/// 実際にある。名前だけで決めると保存先がぶつかり、二つ目が一つ目の中身に
/// すり替わったまま何事もなく保存が終わる - 開くまで誰も気付けない。
/// 投稿内で一意な id を必ず添えて、ぶつかりようのない名前にする。
///
/// 抽出側と JSON への埋め込み側の両方がこの関数を通る。二か所が別々に同じ
/// 規則を書いていたことが、そもそもの穴だった。
fn fanbox_file_filename(file: &Value) -> String {
    let name = file.get("name").and_then(|v| v.as_str()).unwrap_or("file");
    let url = file.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let ext = extract_extension(url, "bin");
    let key = file
        .get("id")
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        // id を持たない形で返ってきたときは URL から作る。名前が同じでも
        // URL が違えば別物になり、同じ添付なら何度呼んでも同じ名前になる。
        .unwrap_or_else(|| short_url_digest(url));
    sanitize_filename(&format!("{name}_{key}.{ext}"))
}

/// FANBOX の画像の保存名。id は投稿内で一意なので、そのまま使える。
fn fanbox_image_filename(image: &Value) -> String {
    let id = image.get("id").and_then(|v| v.as_str()).unwrap_or("image");
    let url = image
        .get("originalUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    sanitize_filename(&format!("{}.{}", id, extract_extension(url, "jpg")))
}

/// URL を短く畳んだ識別子。名前の衝突を解くためだけに使う。
fn short_url_digest(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(url.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// ファイル名サニタイザー
/// OS依存の禁止文字 (< > : " / \ | ? *) や、ディレクトリトラバーサルを示す ".." を安全な文字列に置換する
pub fn sanitize_filename(name: &str) -> String {
    let mut cleaned = name.replace("..", "__");

    // Windowsの不正文字をアンダースコアに置換
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    for &c in &invalid_chars {
        cleaned = cleaned.replace(c, "_");
    }

    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "asset_file".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Authentication is supplied manually rather than through a cookie jar, so
/// it must never follow an arbitrary asset URL from post JSON. Keep this list
/// exact: creator subdomains and lookalike suffixes are not authentication
/// endpoints and do not need the account session.
fn may_send_fanbox_cookie(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.port_or_known_default() == Some(443)
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && matches!(
            parsed.host_str(),
            Some("api.fanbox.cc" | "www.fanbox.cc" | "downloads.fanbox.cc")
        )
}

fn intended_as_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif")
    )
}

fn validate_asset_file(
    path: &Path,
    intended_path: &Path,
    max_bytes: u64,
    require_image: bool,
) -> Result<u64, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to inspect asset file: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("Asset path is not a regular file".to_string());
    }
    let bytes = metadata.len();
    if bytes == 0 {
        return Err("Asset file is empty".to_string());
    }
    if bytes > max_bytes {
        return Err(format!(
            "Asset file is too large: {bytes} bytes (limit {max_bytes})"
        ));
    }

    if require_image || intended_as_image(intended_path) {
        let reader = image::ImageReader::open(path)
            .map_err(|error| format!("Asset image cannot be opened: {error}"))?
            .with_guessed_format()
            .map_err(|error| format!("Asset image format cannot be detected: {error}"))?;
        let (width, height) = reader
            .into_dimensions()
            .map_err(|error| format!("Asset image header is invalid: {error}"))?;
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| "Asset image dimensions overflow".to_string())?;
        if width == 0 || height == 0 || pixels > MAX_ASSET_IMAGE_PIXELS {
            return Err(format!(
                "Asset image dimensions are unsafe: {width}x{height}"
            ));
        }
    }

    Ok(bytes)
}

async fn asset_file_is_valid(
    path: &Path,
    intended_path: &Path,
    max_bytes: u64,
    require_image: bool,
) -> bool {
    let path = path.to_path_buf();
    let intended_path = intended_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        validate_asset_file(&path, &intended_path, max_bytes, require_image).is_ok()
    })
    .await
    .unwrap_or(false)
}

pub(crate) async fn asset_path_is_valid_image(path: &Path, max_bytes: u64) -> bool {
    asset_file_is_valid(path, path, max_bytes, true).await
}

struct PendingAssetFile {
    path: PathBuf,
    file: Option<File>,
    published: bool,
}

impl Drop for PendingAssetFile {
    fn drop(&mut self) {
        // Close the handle before removal so cancellation and panic cleanup also
        // works on Windows, where an open file cannot normally be deleted.
        self.file.take();
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

async fn create_unique_part_file(destination: &Path) -> Result<PendingAssetFile, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "Asset destination has no parent directory".to_string())?;
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    for _ in 0..16 {
        let part_path = parent.join(format!(".{filename}.{:016x}.part", rand::random::<u64>()));
        match File::options()
            .create_new(true)
            .write(true)
            .open(&part_path)
            .await
        {
            Ok(file) => {
                return Ok(PendingAssetFile {
                    path: part_path,
                    file: Some(file),
                    published: false,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Failed to create asset part file: {error}")),
        }
    }
    Err("Failed to allocate a unique asset part file".to_string())
}

/// Streams a response into a unique file beside the destination and publishes
/// it with one rename only after size and format validation succeeds.
pub(crate) async fn save_response_atomically(
    mut response: reqwest::Response,
    destination: &Path,
    max_bytes: u64,
    require_image: bool,
) -> Result<u64, String> {
    let declared_length = response.content_length();
    if let Some(content_length) = declared_length {
        if content_length > max_bytes {
            return Err(format!(
                "Remote asset is too large: {content_length} bytes (limit {max_bytes})"
            ));
        }
    }

    let mut pending = create_unique_part_file(destination).await?;
    let part_path = pending.path.clone();
    let result = async {
        let mut written = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("Streaming chunk error: {error}"))?
        {
            written = written
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| "Remote asset size overflow".to_string())?;
            if written > max_bytes {
                return Err(format!("Remote asset exceeded the {max_bytes} byte limit"));
            }
            pending
                .file
                .as_mut()
                .ok_or_else(|| "Asset part file was closed unexpectedly".to_string())?
                .write_all(&chunk)
                .await
                .map_err(|error| format!("Disk write error: {error}"))?;
        }
        // 相手が申告した長さに、実際に届いた長さが届いているか。
        //
        // HTTP/1.1 の枠組みが崩れた切断は hyper が先に失敗として上げるので、
        // ここは最後の砦にあたる - 間に立つプロキシや CDN が「短いのに
        // 完結した」応答を返す経路が残る。多い側を咎めないのは、将来 gzip の
        // 自動展開を有効にしたときに、展開後の長さと圧縮時の申告を突き合わせて
        // 正しい保存を弾かないため。足りない側だけが、壊れた作品を生む。
        if let Some(expected) = declared_length {
            if written < expected {
                return Err(format!(
                    "Remote asset was truncated: {written} of {expected} bytes arrived"
                ));
            }
        }
        let file = pending
            .file
            .as_mut()
            .ok_or_else(|| "Asset part file was closed unexpectedly".to_string())?;
        file.flush()
            .await
            .map_err(|error| format!("Flush error: {error}"))?;
        file.sync_all()
            .await
            .map_err(|error| format!("File sync error: {error}"))?;
        pending.file.take();

        let validation_path = part_path.clone();
        let intended_path = destination.to_path_buf();
        let validated_bytes = tokio::task::spawn_blocking(move || {
            validate_asset_file(&validation_path, &intended_path, max_bytes, require_image)
        })
        .await
        .map_err(|error| format!("Asset validation task panicked: {error}"))??;

        match fs::rename(&part_path, destination).await {
            Ok(()) => {
                pending.published = true;
                Ok(validated_bytes)
            }
            Err(_)
                if asset_file_is_valid(destination, destination, max_bytes, require_image)
                    .await =>
            {
                // Another concurrent download published a complete copy first.
                let _ = fs::remove_file(&part_path).await;
                pending.published = true;
                Ok(validated_bytes)
            }
            Err(rename_error) => Err(format!("Failed to publish asset file: {rename_error}")),
        }
    }
    .await;

    result
}

/// JSONファイルをパースし、オリジナルのアセットのダウンロードURLを抽出する
pub fn extract_download_targets(data: &Value, is_fanbox: bool, targets: &mut Vec<DownloadTarget>) {
    if is_fanbox {
        let data = crate::fanbox_api::payload::post_or_self(data);
        // 1. Fanboxのカバー画像
        if let Some(cover_url) = data
            .get("coverImageUrl")
            .and_then(|v| v.as_str())
            .or_else(|| {
                data.get("cover")
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
            })
        {
            let filename = sanitize_filename(&extract_filename(cover_url, "cover.jpg"));
            targets.push(DownloadTarget {
                url: cover_url.to_string(),
                sub_folder: "cover",
                filename,
            });
        }

        // 2. Fanbox本文のアセット (imageMap, fileMap)
        if let Some(body) = data.get("body") {
            // articleタイプの画像マップ
            if let Some(image_map) = body.get("imageMap").and_then(|v| v.as_object()) {
                for (_, img) in image_map {
                    if let Some(orig_url) = img.get("originalUrl").and_then(|v| v.as_str()) {
                        targets.push(DownloadTarget {
                            url: orig_url.to_string(),
                            sub_folder: "illustrations",
                            filename: fanbox_image_filename(img),
                        });
                    }
                }
            }

            // articleタイプのファイルマップ (添付ファイル)
            if let Some(file_map) = body.get("fileMap").and_then(|v| v.as_object()) {
                for (_, file) in file_map {
                    if let Some(file_url) = file.get("url").and_then(|v| v.as_str()) {
                        targets.push(DownloadTarget {
                            url: file_url.to_string(),
                            sub_folder: "files",
                            filename: fanbox_file_filename(file),
                        });
                    }
                }
            }

            // fileタイプのfilesリスト
            if let Some(files) = body.get("files").and_then(|v| v.as_array()) {
                for file in files {
                    if let Some(file_url) = file.get("url").and_then(|v| v.as_str()) {
                        targets.push(DownloadTarget {
                            url: file_url.to_string(),
                            sub_folder: "files",
                            filename: fanbox_file_filename(file),
                        });
                    }
                }
            }

            // imageタイプのimagesリスト
            if let Some(images) = body.get("images").and_then(|v| v.as_array()) {
                for img in images {
                    if let Some(orig_url) = img.get("originalUrl").and_then(|v| v.as_str()) {
                        targets.push(DownloadTarget {
                            url: orig_url.to_string(),
                            sub_folder: "illustrations",
                            filename: fanbox_image_filename(img),
                        });
                    }
                }
            }
        }
    } else {
        // Pixiv小説
        // 1. カバー画像
        if let Some(cover_url) = data.get("cover_url").and_then(|v| v.as_str()) {
            let filename = sanitize_filename(&extract_filename(cover_url, "cover.jpg"));
            targets.push(DownloadTarget {
                url: cover_url.to_string(),
                sub_folder: "cover",
                filename,
            });
        } else if let Some(cover_url) = data
            .get("detail")
            .and_then(|d| d.get("cover_url"))
            .and_then(|v| v.as_str())
        {
            let filename = sanitize_filename(&extract_filename(cover_url, "cover.jpg"));
            targets.push(DownloadTarget {
                url: cover_url.to_string(),
                sub_folder: "cover",
                filename,
            });
        }

        // 2. 挿絵 (illusts)
        let mut illust_count = 0;
        if let Some(illusts) = data.get("illusts").and_then(|v| v.as_object()) {
            for (key, illust_entry) in illusts {
                let mut found = false;

                // パターン A: illust_entry.illust.images 構造
                if let Some(illust) = illust_entry.get("illust").and_then(|v| v.as_object()) {
                    if let Some(images) = illust.get("images").and_then(|v| v.as_object()) {
                        if let Some(url) = images
                            .get("original")
                            .and_then(|v| v.as_str())
                            .or_else(|| images.get("medium").and_then(|v| v.as_str()))
                            .or_else(|| images.get("small").and_then(|v| v.as_str()))
                        {
                            let page_num = illust_entry
                                .get("page")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(1);
                            let ext = extract_extension(url, "jpg");
                            let filename =
                                sanitize_filename(&format!("{}_p{}.{}", key, page_num - 1, ext));
                            targets.push(DownloadTarget {
                                url: url.to_string(),
                                sub_folder: "illustrations",
                                filename,
                            });
                            illust_count += 1;
                            found = true;
                        }
                    }
                }

                // パターン B: 従来の pages 配列構造 (フォールバック)
                if !found {
                    if let Some(pages) = illust_entry.get("pages").and_then(|v| v.as_array()) {
                        for (page_idx, page) in pages.iter().enumerate() {
                            if let Some(urls) = page.get("urls") {
                                if let Some(orig_url) = urls
                                    .get("original")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| urls.get("regular").and_then(|v| v.as_str()))
                                {
                                    let illust_id = illust_entry
                                        .get("illustId")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(key);
                                    let ext = extract_extension(orig_url, "jpg");
                                    let filename = sanitize_filename(&format!(
                                        "{}_p{}.{}",
                                        illust_id, page_idx, ext
                                    ));
                                    targets.push(DownloadTarget {
                                        url: orig_url.to_string(),
                                        sub_folder: "illustrations",
                                        filename,
                                    });
                                    illust_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        log::info!(
            "extract_download_targets: Extracted {} Pixiv illustrations (illusts)",
            illust_count
        );

        // 3. インライン画像 (images)
        let mut inline_count = 0;
        if let Some(images) = data.get("images").and_then(|v| v.as_object()) {
            for (img_id, img_val) in images {
                if let Some(urls) = img_val.get("urls") {
                    if let Some(orig_url) = urls
                        .get("original")
                        .and_then(|v| v.as_str())
                        .or_else(|| urls.get("regular").and_then(|v| v.as_str()))
                    {
                        let ext = extract_extension(orig_url, "jpg");
                        let filename = sanitize_filename(&format!("inline_{}.{}", img_id, ext));
                        targets.push(DownloadTarget {
                            url: orig_url.to_string(),
                            sub_folder: "illustrations",
                            filename,
                        });
                        inline_count += 1;
                    }
                }
            }
        }
        log::info!(
            "extract_download_targets: Extracted {} Pixiv inline images (images)",
            inline_count
        );
    }
    log::info!(
        "extract_download_targets: Total assets extracted = {}",
        targets.len()
    );
}

/// ローカルアセットへの相対パスをJSONに埋め込み、オフライン再生に対応させる
pub fn inject_local_paths(data: &mut Value, assets_dir_name: &str, is_fanbox: bool) {
    if is_fanbox {
        let data = crate::fanbox_api::payload::post_mut_or_self(data);
        if let Some(cover_url) = data
            .get("coverImageUrl")
            .and_then(|v| v.as_str())
            .or_else(|| {
                data.get("cover")
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
            })
        {
            let filename = sanitize_filename(&extract_filename(cover_url, "cover.jpg"));
            data["localCoverPath"] =
                Value::String(format!("./{}/cover/{}", assets_dir_name, filename));
        }

        if let Some(body) = data.get_mut("body") {
            // article画像
            if let Some(image_map) = body.get_mut("imageMap").and_then(|v| v.as_object_mut()) {
                for (_, img) in image_map {
                    let filename = fanbox_image_filename(img);
                    img["localPath"] =
                        Value::String(format!("./{assets_dir_name}/illustrations/{filename}"));
                }
            }
            // article添付ファイル
            if let Some(file_map) = body.get_mut("fileMap").and_then(|v| v.as_object_mut()) {
                for (_, file) in file_map {
                    let filename = fanbox_file_filename(file);
                    file["localPath"] =
                        Value::String(format!("./{assets_dir_name}/files/{filename}"));
                }
            }
            // file添付ファイル
            if let Some(files) = body.get_mut("files").and_then(|v| v.as_array_mut()) {
                for file in files {
                    let filename = fanbox_file_filename(file);
                    file["localPath"] =
                        Value::String(format!("./{assets_dir_name}/files/{filename}"));
                }
            }
            // image画像
            if let Some(images) = body.get_mut("images").and_then(|v| v.as_array_mut()) {
                for img in images {
                    let id = img.get("id").and_then(|v| v.as_str()).unwrap_or("image");
                    let orig_url = img
                        .get("originalUrl")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ext = extract_extension(orig_url, "jpg");
                    img["localPath"] = Value::String(format!(
                        "./{}/illustrations/{}",
                        assets_dir_name,
                        sanitize_filename(&format!("{}.{}", id, ext))
                    ));
                }
            }
        }
    } else {
        if let Some(cover_url) = data.get("cover_url").and_then(|v| v.as_str()) {
            let filename = sanitize_filename(&extract_filename(cover_url, "cover.jpg"));
            data["localCoverPath"] =
                Value::String(format!("./{}/cover/{}", assets_dir_name, filename));
        } else if let Some(cover_url) = data
            .get("detail")
            .and_then(|d| d.get("cover_url"))
            .and_then(|v| v.as_str())
        {
            let filename = sanitize_filename(&extract_filename(cover_url, "cover.jpg"));
            data["localCoverPath"] =
                Value::String(format!("./{}/cover/{}", assets_dir_name, filename));
        }

        // 挿絵マップ
        if let Some(illusts) = data.get_mut("illusts").and_then(|v| v.as_object_mut()) {
            for (key, illust_entry) in illusts {
                let mut found = false;
                let page_num = illust_entry
                    .get("page")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1);

                // パターン A: illust_entry.illust.images 構造
                if let Some(illust) = illust_entry
                    .get_mut("illust")
                    .and_then(|v| v.as_object_mut())
                {
                    if let Some(images) = illust.get_mut("images").and_then(|v| v.as_object_mut()) {
                        let url_opt = images
                            .get("original")
                            .and_then(|v| v.as_str())
                            .or_else(|| images.get("medium").and_then(|v| v.as_str()))
                            .or_else(|| images.get("small").and_then(|v| v.as_str()))
                            .map(|s| s.to_string());

                        if let Some(url) = url_opt {
                            let ext = extract_extension(&url, "jpg");
                            let local_path = format!(
                                "./{}/illustrations/{}",
                                assets_dir_name,
                                sanitize_filename(&format!("{}_p{}.{}", key, page_num - 1, ext))
                            );

                            if images.get("original").and_then(|v| v.as_str()).is_some() {
                                images.insert("original".to_string(), Value::String(local_path));
                            } else if images.get("medium").and_then(|v| v.as_str()).is_some() {
                                images.insert("medium".to_string(), Value::String(local_path));
                            }
                            found = true;
                        }
                    }
                }

                // パターン B: 従来の pages 配列構造 (フォールバック)
                if !found {
                    let illust_id = illust_entry
                        .get("illustId")
                        .and_then(|v| v.as_str())
                        .unwrap_or(key)
                        .to_string();
                    if let Some(pages) =
                        illust_entry.get_mut("pages").and_then(|v| v.as_array_mut())
                    {
                        for (page_idx, page) in pages.iter_mut().enumerate() {
                            if let Some(urls) = page.get("urls") {
                                if let Some(orig_url) = urls
                                    .get("original")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| urls.get("regular").and_then(|v| v.as_str()))
                                {
                                    let ext = extract_extension(orig_url, "jpg");
                                    page["localPath"] = Value::String(format!(
                                        "./{}/illustrations/{}",
                                        assets_dir_name,
                                        sanitize_filename(&format!(
                                            "{}_p{}.{}",
                                            illust_id, page_idx, ext
                                        ))
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        // インライン画像 (images)
        if let Some(images) = data.get_mut("images").and_then(|v| v.as_object_mut()) {
            for (img_id, img_val) in images {
                if let Some(urls) = img_val.get("urls") {
                    if let Some(orig_url) = urls
                        .get("original")
                        .and_then(|v| v.as_str())
                        .or_else(|| urls.get("regular").and_then(|v| v.as_str()))
                    {
                        let ext = extract_extension(orig_url, "jpg");
                        img_val["localPath"] = Value::String(format!(
                            "./{}/illustrations/{}",
                            assets_dir_name,
                            sanitize_filename(&format!("inline_{}.{}", img_id, ext))
                        ));
                    }
                }
            }
        }
    }
}

fn extract_filename(url: &str, default: &str) -> String {
    let raw = url
        .split('?')
        .next()
        .unwrap_or(url)
        .split('/')
        .next_back()
        .unwrap_or(default);
    if raw.trim().is_empty() {
        default.to_string()
    } else {
        raw.to_string()
    }
}

fn find_existing_asset_in_other_versions(
    current_version_dir: &Path,
    sub_folder: &str,
    filename: &str,
    url: &str,
    is_fanbox: bool,
) -> Option<PathBuf> {
    let work_root = current_version_dir.parent()?;
    let entries = std::fs::read_dir(work_root).ok()?;
    let normalized_url = url.split('?').next().unwrap_or(url);

    for entry in entries.flatten() {
        let version_dir = entry.path();
        if !version_dir.is_dir() || version_dir == current_version_dir {
            continue;
        }

        let json_path = {
            let original = version_dir.join("original.json");
            if original.is_file() {
                original
            } else {
                version_dir.join("data.json")
            }
        };
        let Ok(json_content) = std::fs::read_to_string(json_path) else {
            continue;
        };
        let Ok(json_data) = serde_json::from_str::<Value>(&json_content) else {
            continue;
        };
        let mut previous_targets = Vec::new();
        extract_download_targets(&json_data, is_fanbox, &mut previous_targets);
        let same_source_asset = previous_targets.iter().any(|target| {
            target.sub_folder == sub_folder
                && target.filename == filename
                && target.url.split('?').next().unwrap_or(&target.url) == normalized_url
        });
        if !same_source_asset {
            continue;
        }

        let candidate = version_dir
            .join("data_assets")
            .join(sub_folder)
            .join(filename);
        if validate_asset_file(&candidate, &candidate, MAX_ASSET_DOWNLOAD_BYTES, false).is_ok() {
            return Some(candidate);
        }
    }

    None
}

/// 全てのアセットを一括並行ダウンロードし、ローカルリンクをバインドする
pub async fn download_and_link_assets(
    _app: &tauri::AppHandle,
    json_data: &mut Value,
    json_path: &Path,
    is_fanbox: bool,
    cookie: Option<String>,
    user_agent: Option<String>,
) -> Result<(), String> {
    let file_stem = json_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("novel");
    let parent_dir = json_path.parent().ok_or("Failed to get parent directory")?;

    let assets_dir_name = format!("{}_assets", file_stem);
    let assets_dir = parent_dir.join(&assets_dir_name);

    fs::create_dir_all(&assets_dir)
        .await
        .map_err(|e| e.to_string())?;
    let canonical_assets_dir = fs::canonicalize(&assets_dir)
        .await
        .map_err(|error| format!("Failed to resolve assets directory: {error}"))?;

    let mut download_targets = Vec::new();
    extract_download_targets(json_data, is_fanbox, &mut download_targets);
    // 同じ保存先に別のURLが来たら、黙って落とさず保存ごと止める。
    //
    // 重複排除は「同じアセットが二度参照されている」ぶんを畳むためのもので、
    // 中身の違う二つを一つに畳んでよいという意味ではない。畳んでしまうと、
    // 二つ目は一つ目の中身を指したまま保存が成功して終わる - 名前の付け方に
    // 抜けが残っていたとき、それを知る手立てがこれしかない。
    let mut destinations: HashMap<(&'static str, String), String> = HashMap::new();
    let mut collision: Option<String> = None;
    download_targets.retain(|target| {
        match destinations.entry((target.sub_folder, target.filename.clone())) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(target.url.clone());
                true
            }
            std::collections::hash_map::Entry::Occupied(slot) => {
                if slot.get() != &target.url {
                    collision.get_or_insert_with(|| {
                        format!(
                            "two different assets both want {}/{}: {} and {}",
                            target.sub_folder,
                            target.filename,
                            slot.get(),
                            target.url
                        )
                    });
                }
                false
            }
        }
    });
    if let Some(reason) = collision {
        return Err(format!(
            "Asset destinations collide, refusing to save: {reason}"
        ));
    }

    let msg = format!(
        "[INFO] アセットの解析中... 合計 {} 件のアセットを検出しました（is_fanbox: {}）",
        download_targets.len(),
        is_fanbox
    );
    log::info!("{}", msg);

    if download_targets.is_empty() {
        let msg = "[INFO] ダウンロード対象のアセットは見つかりませんでした。";
        log::info!("{}", msg);
        return Ok(());
    }

    let referer = if is_fanbox {
        "https://www.fanbox.cc/"
    } else {
        "https://www.pixiv.net/"
    };
    // **総時間ではなく、無音の時間で切る。**
    //
    // `timeout` は接続から完了までの合計に効く。30秒だと、上限の 256MiB を
    // 落とし切るのに 68Mbps 以上の持続速度が要る計算になり、回線が細い人や
    // 大きな添付では**構造的に間に合わない**。しかも再試行は毎回まっさらな
    // 一時ファイルへ最初から落とし直すので、同じ理由で何度でも失敗する。
    //
    // 止まっている接続を捨てたいだけなので、見るのは「次のかたまりが来ない
    // 時間」でよい。落ち切るまでの合計には上限を置かない。
    let client = reqwest::Client::builder()
        .read_timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(4)
        .build()
        .map_err(|error| format!("Asset HTTP client creation failed: {error}"))?;
    let mut join_set = JoinSet::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(4));

    for target in download_targets {
        let dest_folder = assets_dir.join(target.sub_folder);
        let dest_path = dest_folder.join(&target.filename);
        let current_version_dir = parent_dir.to_path_buf();
        let url = target.url.clone();
        let client_clone = client.clone();
        let referer_str = referer.to_string();
        let cookie_clone = cookie.clone();
        let attach_cookie = is_fanbox && may_send_fanbox_cookie(&url);
        let ua_clone = user_agent.clone();
        let sem = semaphore.clone();
        let filename = target.filename.clone();
        let sub_folder = target.sub_folder;
        let canonical_assets_dir = canonical_assets_dir.clone();

        join_set.spawn(async move {
            fs::create_dir_all(&dest_folder)
                .await
                .map_err(|e| format!("Failed to create folder: {}", e))?;
            let canonical_dest_folder = fs::canonicalize(&dest_folder)
                .await
                .map_err(|error| format!("Failed to resolve asset folder: {error}"))?;
            if !canonical_dest_folder.starts_with(&canonical_assets_dir) {
                return Err("Asset folder escapes the version directory".to_string());
            }

            if asset_file_is_valid(
                &dest_path,
                &dest_path,
                MAX_ASSET_DOWNLOAD_BYTES,
                false,
            )
            .await
            {
                let msg = format!("[INFO] すでに保存されています（スキップします）: {}", filename);
                log::info!("{}", msg);
                return Ok::<(), String>(());
            }
            if let Ok(metadata) = fs::symlink_metadata(&dest_path).await {
                if metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                    fs::remove_file(&dest_path)
                        .await
                        .map_err(|error| format!("Failed to remove invalid asset: {error}"))?;
                } else {
                    return Err("Asset destination is not a regular file".to_string());
                }
            }

            if let Some(existing_path) =
                find_existing_asset_in_other_versions(
                    &current_version_dir,
                    sub_folder,
                    &filename,
                    &url,
                    is_fanbox,
                )
            {
                match std::fs::hard_link(&existing_path, &dest_path) {
                    Ok(_) => {
                        let msg = format!(
                            "[INFO] 過去バージョンの同一アセットを再利用しました: {}",
                            filename
                        );
                        log::info!("{}", msg);
                        return Ok::<(), String>(());
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to hard-link existing asset {:?} -> {:?}: {}. Falling back to download.",
                            existing_path,
                            dest_path,
                            e
                        );
                    }
                }
            }

            let _permit = sem.acquire_owned()
                .await
                .map_err(|e| format!("Failed to acquire semaphore permit: {}", e))?;

            let msg = format!("[INFO] アセットのダウンロードを開始: {} -> {}", url, filename);
            log::info!("{}", msg);

            let mut attempts = 4;
            let mut delay = tokio::time::Duration::from_millis(1500);

            loop {
                let res = async {
                    let mut req = client_clone.get(&url)
                        .header("Referer", &referer_str);

                    if attach_cookie {
                        if let Some(ref cookie_str) = cookie_clone {
                            if !cookie_str.is_empty() {
                                req = req.header("Cookie", cookie_str.as_str());
                            }
                        }
                    }

                    if let Some(ref ua_str) = ua_clone {
                        if !ua_str.is_empty() {
                            req = req.header("User-Agent", ua_str);
                        } else {
                            req = req.header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
                        }
                    } else {
                        req = req.header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
                    }

                    let response = req.send()
                        .await
                        .map_err(|e| format!("Network error: {}", e))?;

                    if !response.status().is_success() {
                        return Err(format!("HTTP {} for {}", response.status(), url));
                    }
                    save_response_atomically(
                        response,
                        &dest_path,
                        MAX_ASSET_DOWNLOAD_BYTES,
                        false,
                    )
                    .await
                    .map(|_| ())
                }.await;

                match res {
                    Ok(_) => {
                        let msg = format!("[SUCCESS] 保存完了: {}", filename);
                        log::info!("{}", msg);
                        break;
                    }
                    Err(e) => {
                        attempts -= 1;
                        if attempts == 0 {
                            let msg = format!("[ERROR] アセットのダウンロードに失敗しました: {}. エラー: {}", url, e);
                            log::error!("{}", msg);
                            return Err(e);
                        }
                        let msg = format!(
                            "[WARN] アセット取得に失敗しました。再試行します (残り {} 回) in {:?}. エラー: {}",
                            attempts,
                            delay,
                            e
                        );
                        log::warn!("{}", msg);
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                    }
                }
            }

            Ok::<(), String>(())
        });
    }

    let mut success_count = 0;
    let mut fail_count = 0;
    // 最初の理由だけは持ち帰る。件数しか返さなかったころは、消された画像
    // （やり直しても無駄）と一時的な通信断（次で通る）が同じ一行になり、
    // ジョブの失敗分類も「その他」に落ちて再試行の判断ができなかった。
    let mut first_failure: Option<String> = None;
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(_)) => {
                success_count += 1;
            }
            Ok(Err(e)) => {
                fail_count += 1;
                let msg = format!("[ERROR] アセットダウンロードタスクが失敗しました: {}", e);
                log::error!("{}", msg);
                first_failure.get_or_insert(e);
            }
            Err(e) => {
                fail_count += 1;
                let msg = format!(
                    "[ERROR] アセットダウンロードタスクがパニックしました: {:?}",
                    e
                );
                log::error!("{}", msg);
                first_failure.get_or_insert_with(|| format!("asset task panicked: {e}"));
            }
        }
    }

    let msg = format!(
        "アセット処理完了。成功: {} 件, 失敗: {} 件",
        success_count, fail_count
    );
    log::info!("{}", msg);

    if fail_count > 0 {
        // 1件でも欠けたら保存そのものを取りやめる。中身の足りない版を
        // ライブラリに置くくらいなら、保存されなかったほうが分かりやすい。
        let reason = first_failure.unwrap_or_else(|| "reason unavailable".to_string());
        return Err(format!(
            "{fail_count} asset downloads failed ({success_count} succeeded): {reason}"
        ));
    }
    inject_local_paths(json_data, &assets_dir_name, is_fanbox);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_directory(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("piep_asset_{label}_{}", rand::random::<u64>()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    async fn local_response(headers: &str, body: &[u8]) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers.trim_end_matches("\r\n").to_string();
        let body = body.to_vec();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let response_head =
                format!("HTTP/1.1 200 OK\r\nConnection: close\r\n{headers}\r\n\r\n");
            socket.write_all(response_head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        reqwest::Client::new()
            .get(format!("http://{address}/asset"))
            .send()
            .await
            .unwrap()
    }

    fn assert_no_part_files(directory: &Path) {
        assert!(std::fs::read_dir(directory).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }

    #[test]
    fn test_extract_pixiv_targets() {
        let data = json!({
            "cover_url": "https://example.com/cover.jpg",
            "illusts": {
                "123": {
                    "illustId": "123",
                    "pages": [
                        {
                            "urls": {
                                "original": "https://example.com/123_original.png"
                            }
                        }
                    ]
                }
            }
        });

        let mut targets = Vec::new();
        extract_download_targets(&data, false, &mut targets);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].url, "https://example.com/cover.jpg");
        assert_eq!(targets[0].sub_folder, "cover");
        assert_eq!(targets[0].filename, "cover.jpg");

        assert_eq!(targets[1].url, "https://example.com/123_original.png");
        assert_eq!(targets[1].sub_folder, "illustrations");
        assert_eq!(targets[1].filename, "123_p0.png");
    }

    #[test]
    fn test_inject_pixiv_paths() {
        let mut data = json!({
            "cover_url": "https://example.com/cover.jpg",
            "illusts": {
                "123": {
                    "illustId": "123",
                    "pages": [
                        {
                            "urls": {
                                "original": "https://example.com/123_original.png"
                            }
                        }
                    ]
                }
            }
        });

        inject_local_paths(&mut data, "novel_assets", false);

        assert_eq!(data["localCoverPath"], "./novel_assets/cover/cover.jpg");
        assert_eq!(
            data["illusts"]["123"]["pages"][0]["localPath"],
            "./novel_assets/illustrations/123_p0.png"
        );
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("test..jpg"), "test__jpg");
        assert_eq!(sanitize_filename("illust:123/p0.png"), "illust_123_p0.png");
        assert_eq!(sanitize_filename("  "), "asset_file");
    }

    #[test]
    fn fanbox_cookie_is_limited_to_exact_official_https_hosts() {
        for url in [
            "https://api.fanbox.cc/post.info?postId=1",
            "https://www.fanbox.cc/",
            "https://downloads.fanbox.cc/images/post/1/example.jpg",
        ] {
            assert!(may_send_fanbox_cookie(url), "rejected {url}");
        }
        for url in [
            "http://downloads.fanbox.cc/file",
            "https://downloads.fanbox.cc:444/file",
            "https://creator.fanbox.cc/file",
            "https://fanbox.cc.evil.example/file",
            "https://evil.example/?next=https://api.fanbox.cc",
            "not a url",
        ] {
            assert!(!may_send_fanbox_cookie(url), "accepted {url}");
        }
    }

    #[test]
    fn existing_images_require_a_valid_header_and_safe_dimensions() {
        let directory = test_directory("validation");
        let valid = directory.join("valid.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]))
            .save(&valid)
            .unwrap();
        assert!(validate_asset_file(&valid, &valid, 1024 * 1024, false).is_ok());

        let truncated = directory.join("truncated.png");
        std::fs::write(&truncated, b"\x89PNG\r\n\x1a\npartial").unwrap();
        assert!(validate_asset_file(&truncated, &truncated, 1024 * 1024, false).is_err());

        let empty = directory.join("empty.bin");
        std::fs::write(&empty, []).unwrap();
        assert!(validate_asset_file(&empty, &empty, 1024, false).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    /// 同じ投稿に同名の添付が二つある、は実際に起きる - 名前を付けるのは
    /// creator だからだ。名前だけで保存先を決めていたころは、二つ目が
    /// 一つ目の中身を指したまま、エラーもなく保存が終わっていた。
    #[test]
    fn two_fanbox_attachments_sharing_a_name_get_separate_files() {
        let post = serde_json::json!({
            "body": {
                "files": [
                    { "id": "f1", "name": "資料", "url": "https://downloads.fanbox.cc/a/f1.zip" },
                    { "id": "f2", "name": "資料", "url": "https://downloads.fanbox.cc/b/f2.zip" },
                ]
            }
        });

        let mut targets = Vec::new();
        extract_download_targets(&post, true, &mut targets);
        assert_eq!(targets.len(), 2);
        assert_ne!(
            targets[0].filename, targets[1].filename,
            "同名の添付が同じ保存先を取り合っている: {:?}",
            targets,
        );

        // 埋め込まれる参照も、それぞれ自分のファイルを指すこと。抽出側と
        // 埋め込み側が別々に名前を組み立てていたのが、そもそもの穴だった。
        let mut injected = post.clone();
        inject_local_paths(&mut injected, "data_assets", true);
        let files = injected["body"]["files"].as_array().unwrap();
        let first = files[0]["localPath"].as_str().unwrap();
        let second = files[1]["localPath"].as_str().unwrap();
        assert_ne!(first, second);
        assert!(
            first.ends_with(&targets[0].filename),
            "{first} vs {:?}",
            targets[0]
        );
        assert!(
            second.ends_with(&targets[1].filename),
            "{second} vs {:?}",
            targets[1]
        );
    }

    #[test]
    fn current_fanbox_wrapper_keeps_asset_extraction_and_injection_working() {
        let mut wrapped = serde_json::json!({
            "body": { "post": {
                "id": "1",
                "title": "画像投稿",
                "body": { "images": [{
                    "id": "image-1",
                    "extension": "jpg",
                    "originalUrl": "https://downloads.fanbox.cc/images/image-1.jpg"
                }] }
            }}
        });
        let mut targets = Vec::new();
        extract_download_targets(&wrapped, true, &mut targets);
        assert_eq!(targets.len(), 1);

        inject_local_paths(&mut wrapped, "data_assets", true);
        assert!(wrapped["body"]["post"]["body"]["images"][0]["localPath"]
            .as_str()
            .is_some_and(|path| path.ends_with("image-1.jpg")));
    }

    /// id を返さない形にも備える。同じ添付なら何度呼んでも同じ名前になり、
    /// 違う添付なら必ず違う名前になる、の両方が要る。
    #[test]
    fn attachments_without_an_id_are_still_told_apart_and_stay_stable() {
        let post = serde_json::json!({
            "body": {
                "files": [
                    { "name": "資料", "url": "https://downloads.fanbox.cc/a/x.zip" },
                    { "name": "資料", "url": "https://downloads.fanbox.cc/b/y.zip" },
                ]
            }
        });
        let mut targets = Vec::new();
        extract_download_targets(&post, true, &mut targets);
        assert_ne!(targets[0].filename, targets[1].filename);

        let mut again = Vec::new();
        extract_download_targets(&post, true, &mut again);
        assert_eq!(targets[0].filename, again[0].filename);
    }

    #[tokio::test]
    async fn response_is_published_only_after_complete_atomic_write() {
        let directory = test_directory("atomic_success");
        let destination = directory.join("asset.bin");
        let response = local_response("Content-Length: 4\r\n", b"good").await;

        let written = save_response_atomically(response, &destination, 4, false)
            .await
            .unwrap();

        assert_eq!(written, 4);
        assert_eq!(std::fs::read(&destination).unwrap(), b"good");
        assert_no_part_files(&directory);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn declared_oversize_is_rejected_without_creating_any_file() {
        let directory = test_directory("declared_oversize");
        let destination = directory.join("asset.bin");
        let response = local_response("Content-Length: 5\r\n", b"large").await;

        assert!(save_response_atomically(response, &destination, 4, false)
            .await
            .is_err());
        assert!(!destination.exists());
        assert_no_part_files(&directory);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn streamed_oversize_and_truncation_remove_the_part_file() {
        let directory = test_directory("stream_failures");

        let oversized_destination = directory.join("oversized.bin");
        let oversized =
            local_response("Transfer-Encoding: chunked\r\n", b"5\r\nlarge\r\n0\r\n\r\n").await;
        assert!(
            save_response_atomically(oversized, &oversized_destination, 4, false)
                .await
                .is_err()
        );
        assert!(!oversized_destination.exists());
        assert_no_part_files(&directory);

        let truncated_destination = directory.join("truncated.bin");
        let truncated = local_response("Content-Length: 10\r\n", b"short").await;
        assert!(
            save_response_atomically(truncated, &truncated_destination, 16, false)
                .await
                .is_err()
        );
        assert!(!truncated_destination.exists());
        assert_no_part_files(&directory);
        let _ = std::fs::remove_dir_all(directory);
    }
}
