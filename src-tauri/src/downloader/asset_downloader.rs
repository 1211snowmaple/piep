use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::Emitter;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinSet;

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

/// JSONファイルをパースし、オリジナルのアセットのダウンロードURLを抽出する
pub fn extract_download_targets(data: &Value, is_fanbox: bool, targets: &mut Vec<DownloadTarget>) {
    if is_fanbox {
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
                        let id = img.get("id").and_then(|v| v.as_str()).unwrap_or("image");
                        let ext = extract_extension(orig_url, "jpg");
                        let filename = sanitize_filename(&format!("{}.{}", id, ext));
                        targets.push(DownloadTarget {
                            url: orig_url.to_string(),
                            sub_folder: "illustrations",
                            filename,
                        });
                    }
                }
            }

            // articleタイプのファイルマップ (添付ファイル)
            if let Some(file_map) = body.get("fileMap").and_then(|v| v.as_object()) {
                for (_, file) in file_map {
                    if let Some(file_url) = file.get("url").and_then(|v| v.as_str()) {
                        let name = file.get("name").and_then(|v| v.as_str()).unwrap_or("file");
                        let ext = extract_extension(file_url, "bin");
                        let filename = sanitize_filename(&format!("{}.{}", name, ext));
                        targets.push(DownloadTarget {
                            url: file_url.to_string(),
                            sub_folder: "files",
                            filename,
                        });
                    }
                }
            }

            // fileタイプのfilesリスト
            if let Some(files) = body.get("files").and_then(|v| v.as_array()) {
                for file in files {
                    if let Some(file_url) = file.get("url").and_then(|v| v.as_str()) {
                        let name = file.get("name").and_then(|v| v.as_str()).unwrap_or("file");
                        let ext = extract_extension(file_url, "bin");
                        let filename = sanitize_filename(&format!("{}.{}", name, ext));
                        targets.push(DownloadTarget {
                            url: file_url.to_string(),
                            sub_folder: "files",
                            filename,
                        });
                    }
                }
            }

            // imageタイプのimagesリスト
            if let Some(images) = body.get("images").and_then(|v| v.as_array()) {
                for img in images {
                    if let Some(orig_url) = img.get("originalUrl").and_then(|v| v.as_str()) {
                        let id = img.get("id").and_then(|v| v.as_str()).unwrap_or("image");
                        let ext = extract_extension(orig_url, "jpg");
                        let filename = sanitize_filename(&format!("{}.{}", id, ext));
                        targets.push(DownloadTarget {
                            url: orig_url.to_string(),
                            sub_folder: "illustrations",
                            filename,
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
            // article添付ファイル
            if let Some(file_map) = body.get_mut("fileMap").and_then(|v| v.as_object_mut()) {
                for (_, file) in file_map {
                    let name = file.get("name").and_then(|v| v.as_str()).unwrap_or("file");
                    let file_url = file.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let ext = extract_extension(file_url, "bin");
                    file["localPath"] = Value::String(format!(
                        "./{}/files/{}",
                        assets_dir_name,
                        sanitize_filename(&format!("{}.{}", name, ext))
                    ));
                }
            }
            // file添付ファイル
            if let Some(files) = body.get_mut("files").and_then(|v| v.as_array_mut()) {
                for file in files {
                    let name = file.get("name").and_then(|v| v.as_str()).unwrap_or("file");
                    let file_url = file.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let ext = extract_extension(file_url, "bin");
                    file["localPath"] = Value::String(format!(
                        "./{}/files/{}",
                        assets_dir_name,
                        sanitize_filename(&format!("{}.{}", name, ext))
                    ));
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
        if candidate.is_file() && candidate.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            return Some(candidate);
        }
    }

    None
}

/// 全てのアセットを一括並行ダウンロードし、ローカルリンクをバインドする
pub async fn download_and_link_assets(
    app: &tauri::AppHandle,
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

    let mut download_targets = Vec::new();
    extract_download_targets(json_data, is_fanbox, &mut download_targets);

    let msg = format!(
        "[INFO] アセットの解析中... 合計 {} 件のアセットを検出しました（is_fanbox: {}）",
        download_targets.len(),
        is_fanbox
    );
    let _ = app.emit("download-log", &msg);
    log::info!("{}", msg);

    if download_targets.is_empty() {
        let msg = "[INFO] ダウンロード対象のアセットは見つかりませんでした。";
        let _ = app.emit("download-log", &msg);
        log::info!("{}", msg);
        return Ok(());
    }

    let referer = if is_fanbox {
        "https://www.fanbox.cc/"
    } else {
        "https://www.pixiv.net/"
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(4)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
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
        let ua_clone = user_agent.clone();
        let sem = semaphore.clone();
        let app_clone = app.clone();
        let filename = target.filename.clone();
        let sub_folder = target.sub_folder;

        join_set.spawn(async move {
            fs::create_dir_all(&dest_folder)
                .await
                .map_err(|e| format!("Failed to create folder: {}", e))?;

            // 修正：ファイルが存在し、かつファイルサイズが0より大きい場合のみスキップ (破損防止)
            let is_valid = if fs::try_exists(&dest_path).await.unwrap_or(false) {
                if let Ok(meta) = fs::metadata(&dest_path).await {
                    meta.len() > 0
                } else {
                    false
                }
            } else {
                false
            };

            if is_valid {
                let msg = format!("[INFO] すでに保存されています（スキップします）: {}", filename);
                let _ = app_clone.emit("download-log", &msg);
                log::info!("{}", msg);
                return Ok::<(), String>(());
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
                        let _ = app_clone.emit("download-log", &msg);
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
            let _ = app_clone.emit("download-log", &msg);
            log::info!("{}", msg);

            let mut attempts = 4;
            let mut delay = tokio::time::Duration::from_millis(1500);

            loop {
                let res = async {
                    let mut req = client_clone.get(&url)
                        .header("Referer", &referer_str);

                    if let Some(ref cookie_str) = cookie_clone {
                        if !cookie_str.is_empty() {
                            req = req.header("Cookie", cookie_str.as_str());
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

                    let mut response = req.send()
                        .await
                        .map_err(|e| format!("Network error: {}", e))?;

                    if !response.status().is_success() {
                        return Err(format!("HTTP {} for {}", response.status(), url));
                    }

                    let mut file = File::create(&dest_path)
                        .await
                        .map_err(|e| format!("Failed to create file: {}", e))?;

                    while let Some(chunk) = response.chunk().await.map_err(|e| format!("Streaming chunk error: {}", e))? {
                        file.write_all(&chunk)
                            .await
                            .map_err(|e| format!("Disk write error: {}", e))?;
                    }

                    file.flush()
                        .await
                        .map_err(|e| format!("Flush error: {}", e))?;

                    Ok::<(), String>(())
                }.await;

                match res {
                    Ok(_) => {
                        let msg = format!("[SUCCESS] 保存完了: {}", filename);
                        let _ = app_clone.emit("download-log", &msg);
                        log::info!("{}", msg);
                        break;
                    }
                    Err(e) => {
                        attempts -= 1;
                        if attempts == 0 {
                            let msg = format!("[ERROR] アセットのダウンロードに失敗しました: {}. エラー: {}", url, e);
                            let _ = app_clone.emit("download-log", &msg);
                            log::error!("{}", msg);
                            return Err(e);
                        }
                        let msg = format!(
                            "[WARN] アセット取得に失敗しました。再試行します (残り {} 回) in {:?}. エラー: {}",
                            attempts,
                            delay,
                            e
                        );
                        let _ = app_clone.emit("download-log", &msg);
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
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(_)) => {
                success_count += 1;
            }
            Ok(Err(e)) => {
                fail_count += 1;
                let msg = format!("[ERROR] アセットダウンロードタスクが失敗しました: {}", e);
                let _ = app.emit("download-log", &msg);
                log::error!("{}", msg);
            }
            Err(e) => {
                fail_count += 1;
                let msg = format!(
                    "[ERROR] アセットダウンロードタスクがパニックしました: {:?}",
                    e
                );
                let _ = app.emit("download-log", &msg);
                log::error!("{}", msg);
            }
        }
    }

    let msg = format!(
        "[SUCCESS] アセット処理完了。成功: {} 件, 失敗: {} 件",
        success_count, fail_count
    );
    let _ = app.emit("download-log", &msg);
    log::info!("{}", msg);

    inject_local_paths(json_data, &assets_dir_name, is_fanbox);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
