//! pixiv と FANBOX から取ってくる入口。
//!
//! 五つしかないのは、取得の単位が作品・シリーズ・作者の投稿一覧の三つしか
//! ないからである。取ってきたものを保存する仕事は `downloader/` にある。
//!
//! **同じ作品を同時に二回保存しない。** `WORK_SAVE_LOCKS` が取得元と作品 ID
//! の組ごとに錠を持つ。同じ作品への保存が重なるのは珍しいことではなく、
//! 一覧からの一括保存と更新監視が並んだときに普通に起きる。

use crate::downloader::fanbox::get_post_detail;
use crate::downloader::pixiv::{get_novel_detail, PixivNovelContent};
use crate::fanbox_api::models::FanboxPost;
use crate::models::NewAsset;
use crate::pixiv_api;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tauri::Manager;

#[path = "downloader_save.rs"]
pub(super) mod shelf;
pub use shelf::ShelfSaved;
#[path = "downloader_profiles.rs"]
mod shelf_profiles;

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

/// 作者・シリーズのプロフィール保存を、対象ごとに直列化する。
///
/// 作品の保存は `work_save_mutex` で直列化してあるが、そこから枝分かれする
/// プロフィールの保存には鍵が無かった。`library_gate` は読むときと書き戻すとき
/// だけ短く取り、そのあいだにある**版番号の決定・original.json の書き出し・
/// アイコンの取得（最大30秒の通信）**は誰も守っていない。
///
/// 同じ作者の作品が二つ並行して保存されると（更新ジョブと手動保存など）、
/// 両方が「次は版N」と決め、同じ `vN/original.json` へ書き、両方が upsert する。
/// アイコンの取得は速さがまちまちなので、**遅いほう・失敗したほうが後に着けば
/// 良い icon_path を上書きする**。
///
/// 作品の鍵と同じ表を使う。前置きを付けて、作品の鍵と衝突させない。
pub(super) fn profile_save_mutex(
    kind: &str,
    source: &str,
    entity_id: &str,
) -> Arc<tokio::sync::Mutex<()>> {
    work_save_mutex(
        &format!("profile:{kind}"),
        &format!("{source}\0{entity_id}"),
    )
}

fn is_unpublished_part_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.ends_with(".part"))
}

/// 作品ひとつを本文ごと取り寄せる。
///
/// IPC には出さない。画面から始まる保存は `start_save_job` が、監視から始まる
/// 取り直しは更新ジョブが、どちらも Rust の中でこれを呼ぶ。
pub async fn fetch_pixiv_novel(
    novel_id: String,
    refresh_token: String,
) -> Result<PixivNovelContent, String> {
    crate::downloader::pixiv::get_novel_content(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())
}

/// 本文を伴わない作品詳細。改稿の有無を指紋で見るときに使う。
///
/// IPC には出さない。呼ぶのは更新ジョブだけである。
pub async fn fetch_pixiv_novel_metadata(
    novel_id: String,
    refresh_token: String,
) -> Result<crate::downloader::pixiv::PixivNovelDetail, String> {
    get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())
}

/// FANBOX の投稿を一件、生の JSON のまま返す。
///
/// 型を付けずに返すのは、FANBOX の本文が投稿の種類ごとに別の形をしている
/// ためである。解釈は保存する側で行う。
///
/// 鍵は保存してある FANBOX の接続から読む。
#[tauri::command]
pub async fn fetch_fanbox_post(post_id: String) -> Result<serde_json::Value, String> {
    let (cookie, user_agent) = crate::auth::saved::Vault::app().fanbox_session()?;
    fanbox_post(post_id, cookie, user_agent).await
}

/// FANBOX の投稿を一件。更新ジョブが自分の持つ鍵で呼ぶ。
///
/// **Cookie と UA は対で渡す。** 片方だけでは通らない。
pub(crate) async fn fanbox_post(
    post_id: String,
    cookie: String,
    user_agent: String,
) -> Result<serde_json::Value, String> {
    get_post_detail(&post_id, &cookie, &user_agent)
        .await
        .map_err(|e| e.to_string())
}

/// クリエイターの投稿を、最初から最後まで並べて返す。
///
/// 返るのは一覧の情報だけで、本文は含まない。**一覧に出たことは、中身が読める
/// ことを意味しない** - 支援していない有料の投稿は `isRestricted` が立って返り、
/// 本文を取りに行っても読めない。`feeRequired` にいくら必要かが入る。
///
/// 鍵は保存してある FANBOX の接続から読む。
#[tauri::command]
pub async fn fetch_fanbox_creator_posts(creator_id: String) -> Result<Vec<FanboxPost>, String> {
    let (cookie, user_agent) = crate::auth::saved::Vault::app().fanbox_session()?;
    let api = crate::downloader::session::fanbox(cookie, user_agent)
        .map_err(|error| error.to_string())?;
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
    crate::downloader::session::fanbox(cookie, user_agent)
        .map_err(|error| error.to_string())?
        .get_creator_posts_since(&creator_id, stop_source_id)
        .await
        .map_err(|e| e.to_string())
}

/// 公式シリーズに入っている小説を、最初から最後まで並べて返す。
///
/// 頁をたどるので、長いシリーズでは相応に時間がかかる。**100頁で打ち切って
/// `Err` にする**（相手の返事が終わらないときに無限に叩き続けないための歯止め
/// であって、正常な取得では起きない）。
///
/// 返るのは一覧の情報だけで、本文は含まない。
#[tauri::command]
pub async fn fetch_pixiv_series_novels(
    series_id: String,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let refresh_token = crate::auth::saved::Vault::app().pixiv_token()?;
    fetch_pixiv_series_novels_since(series_id, refresh_token, None).await
}

pub(crate) async fn fetch_pixiv_series_novels_since(
    series_id: String,
    refresh_token: String,
    stop_source_id: Option<&str>,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let api =
        crate::downloader::session::pixiv(refresh_token).map_err(|error| error.to_string())?;
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

/// 作者の小説を、最初から最後まで並べて返す。
///
/// `fetch_pixiv_series_novels` と同じく頁をたどり、**100頁で打ち切って `Err`**
/// にする。作品数の多い作者では時間がかかる。
///
/// 返るのは一覧の情報だけで、本文は含まない。
#[tauri::command]
pub async fn fetch_pixiv_user_novels(
    user_id: String,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let refresh_token = crate::auth::saved::Vault::app().pixiv_token()?;
    fetch_pixiv_user_novels_since(user_id, refresh_token, None).await
}

/// 作者の一覧を遡るのを、ここで止めてよいか。
///
/// 一覧は新しい順で、ID は投稿の順に増える。ID が同じときだけでなく、前回の位置
/// より古くなった時点でも止める。前回の作品が取得元で消えていても、全ページを
/// 読みに行かないで済む。
fn reached_last_seen(novel_id: u64, stop_source_id: Option<&str>) -> bool {
    stop_source_id
        .and_then(|stop| stop.parse::<u64>().ok())
        .is_some_and(|stop| novel_id <= stop)
}

pub(crate) async fn fetch_pixiv_user_novels_since(
    user_id: String,
    refresh_token: String,
    stop_source_id: Option<&str>,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let api =
        crate::downloader::session::pixiv(refresh_token).map_err(|error| error.to_string())?;
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
            if reached_last_seen(novel.id, stop_source_id) {
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

/// pixiv の小説を URL 一本から取ってくる。本文と挿絵の在り処まで含む。
///
/// URL から作品 ID を取り出せなければ `Err`。作品頁でない URL を貼られたとき
/// がこれに当たる。
///
/// 表紙は二か所（web 版と API）から来るので、**良いほうを選んで**一つにして
/// 返す。呼び出し側で選び直さないこと。
///
/// **取ってくるだけで、保存はしない。** ライブラリへ入れるのは別の手順。
#[tauri::command]
pub async fn fetch_pixiv_novel_by_url(url: String) -> Result<PixivNovelContent, String> {
    let novel_id =
        crate::downloader::pixiv::extract_novel_id(&url).ok_or("Invalid Pixiv Novel URL")?;
    let credentials = crate::auth::saved::Vault::app().update_credentials();
    let refresh_token = credentials
        .pixiv_refresh_token
        .ok_or_else(|| "設定でpixivに接続してください".to_string())?;
    crate::downloader::session::scope(async move {
        crate::downloader::session::set_pixiv_web_session(
            credentials.pixiv_cookie.as_deref(),
            credentials.pixiv_user_agent.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        crate::downloader::pixiv::get_novel_content(&novel_id, &refresh_token)
            .await
            .map_err(|error| error.to_string())
    })
    .await
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
            .get("cover_url")
            .or_else(|| data.get("coverUrl"))
            .or_else(|| data.pointer("/detail/cover_url"))
            .or_else(|| data.pointer("/detail/coverUrl"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tags_str = data
            .get("tags")
            .or_else(|| data.get("detail").and_then(|d| d.get("tags")))
            .map(|t| {
                if let Some(arr) = t.as_array() {
                    arr.iter()
                        .filter_map(|v| {
                            v.as_str()
                                .or_else(|| v.get("name").and_then(|name| name.as_str()))
                                .or_else(|| v.get("tag").and_then(|name| name.as_str()))
                        })
                        .collect::<Vec<&str>>()
                        .join(",")
                } else {
                    t.as_str().unwrap_or("").to_string()
                }
            })
            .unwrap_or_default();

        let series_id = data
            .get("series_id")
            .or_else(|| data.get("seriesId"))
            .or_else(|| data.pointer("/detail/series_id"))
            .or_else(|| data.pointer("/detail/seriesId"))
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
        let post = crate::fanbox_api::payload::post_or_self(data);
        if let Some(body) = post.get("body") {
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
        source_updated_at = post
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
            let value = if source == "fanbox" {
                crate::fanbox_api::payload::post_or_self(data)
            } else {
                data
            };
            hasher.update(serde_json::to_string(value).unwrap_or_default().as_bytes());
        }
    }
    let hash_bytes = hasher.finalize();
    let hash = hash_bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    (hash, text_len, source_updated_at)
}

/// Pull the readable prose out of a freshly fetched provider payload without
/// saving it. The revision preview and the save path must compare the same
/// text; keeping this beside `compute_content_details` prevents the two views
/// of a FANBOX block document from drifting apart.
pub(crate) fn fetched_plain_text(data: &serde_json::Value, source: &str) -> String {
    if source == "pixiv" {
        return data
            .get("text")
            .or_else(|| data.get("detail").and_then(|detail| detail.get("text")))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
    }

    if source == "fanbox" {
        let post = crate::fanbox_api::payload::post_or_self(data);
        let Some(body) = post.get("body") else {
            return String::new();
        };
        if let Some(blocks) = body.get("blocks").and_then(|value| value.as_array()) {
            return blocks
                .iter()
                .filter(|block| {
                    matches!(
                        block.get("type").and_then(|value| value.as_str()),
                        Some("p" | "header")
                    )
                })
                .map(|block| {
                    block
                        .get("text")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                })
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        return body
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
    }

    String::new()
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
pub(crate) fn fetched_has_material_content(data: &serde_json::Value, source: &str) -> bool {
    match source {
        "fanbox" => fanbox_has_material_content(data),
        "pixiv" => pixiv_has_material_content(data),
        // 知らない取得元は判断材料が無い。止めるほうに倒すと保存が通らなく
        // なるので、ここは通し、既存の版と衝突しない限り従来どおり扱う。
        _ => true,
    }
}

fn fanbox_has_material_content(data: &serde_json::Value) -> bool {
    let post = crate::fanbox_api::payload::post_or_self(data);
    let Some(body) = post.get("body").filter(|v| !v.is_null()) else {
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

/// 取り寄せた中身を棚へ入れる。版の判断も棚が行い、題名・作者・タグも棚が応答から読む。
///
/// IPC には出さない。**題名・作者・タグをどう読むかは取得元の作法であって
/// 画面の都合ではない**ので、材料を画面から受け取る形にはしない。保存ジョブと
/// 更新ジョブが、取り寄せた応答をそのまま渡して呼ぶ。
pub async fn download_and_save(
    app: tauri::AppHandle,
    data: serde_json::Value,
    source: String,
    source_id: String,
    cookie: Option<String>,
    user_agent: Option<String>,
) -> Result<ShelfSaved, String> {
    let data = if source == "fanbox" {
        crate::fanbox_api::payload::into_post(data).ok_or_else(|| {
            "FANBOX投稿データの形式を解釈できませんでした（応答形式が変わった可能性があります）"
                .to_string()
        })?
    } else {
        data
    };
    let shelf = app
        .try_state::<Arc<crate::commands::shelf::Shelf>>()
        .map(|state| Arc::clone(&state))
        .ok_or_else(|| "棚は開いていません".to_string())?;
    let library = shelf.library()?;
    shelf::save_to_shelf(shelf, library, data, source, source_id, cookie, user_agent).await
}

pub(crate) fn fanbox_missing_material_error(data: &serde_json::Value) -> String {
    let post = crate::fanbox_api::payload::post_or_self(data);
    if post
        .get("isRestricted")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        let fee = post
            .get("feeRequired")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let requirement = if fee > 0 {
            format!("（必要な支援額: 月額{fee}円以上）")
        } else {
            String::new()
        };
        return format!(
            "現在のFANBOX連携ではこの投稿を閲覧できません{requirement}。通常の再試行から外して保留します。支援プランや連携アカウントの変更後に、保留一覧から再確認してください"
        );
    }
    "FANBOXから投稿本文と添付が空の応答を受け取りました。時間をおいて再取得し、続く場合は投稿の公開状態を確認してください"
        .to_string()
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
                asset_type: asset_type.to_string(),
                filename,
                local_path: path.to_string_lossy().to_string(),
                mime_type: mime,
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
        let before =
            pixiv_meta_signature_from_json(&saved_json(&detail, "港の灯が落ちる")).unwrap();
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
    use super::{fetched_has_material_content, fetched_plain_text};
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
        assert!(fetched_has_material_content(
            &json!({ "body": { "post": {
                "id": "1", "title": "投稿", "body": { "text": "制作ノート" }
            }}}),
            "fanbox",
        ));
    }

    #[test]
    fn revision_preview_uses_the_same_readable_provider_text() {
        assert_eq!(
            fetched_plain_text(&json!({ "text": "改稿後の本文" }), "pixiv"),
            "改稿後の本文"
        );
        assert_eq!(
            fetched_plain_text(
                &json!({ "body": { "blocks": [
                    { "type": "header", "text": "見出し" },
                    { "type": "image", "text": "本文ではない" },
                    { "type": "p", "text": "段落" }
                ] } }),
                "fanbox"
            ),
            "見出し\n\n段落"
        );
        assert_eq!(
            fetched_plain_text(
                &json!({ "body": { "post": {
                    "id": "1", "title": "投稿", "body": { "text": "包まれた本文" }
                }}}),
                "fanbox"
            ),
            "包まれた本文"
        );
    }
}

#[cfg(test)]
mod profile_save_lock_tests {
    use super::{profile_save_mutex, work_save_mutex};

    /// 同じ作者は同じ鍵、別の作者は別の鍵。
    ///
    /// 対象ごとに直列化できなければ、並行して走る二つの保存が同じ版番号を
    /// 決め、同じ `vN/original.json` を書き、遅いほうがアイコンを消す。
    #[test]
    fn one_lock_per_entity_and_never_shared_with_another() {
        let left = profile_save_mutex("person", "pixiv", "8000001");
        let same = profile_save_mutex("person", "pixiv", "8000001");
        assert!(std::sync::Arc::ptr_eq(&left, &same));

        let other_author = profile_save_mutex("person", "pixiv", "9999999");
        assert!(!std::sync::Arc::ptr_eq(&left, &other_author));

        // 取得元をまたいだ同じ ID は、別人である。
        let other_source = profile_save_mutex("person", "fanbox", "8000001");
        assert!(!std::sync::Arc::ptr_eq(&left, &other_source));
    }

    /// 作者とシリーズは、ID が同じでも別の鍵。
    #[test]
    fn a_person_and_a_series_with_the_same_id_do_not_block_each_other() {
        let person = profile_save_mutex("person", "pixiv", "1234");
        let series = profile_save_mutex("series", "pixiv", "1234");
        assert!(!std::sync::Arc::ptr_eq(&person, &series));
    }

    /// 作品の鍵とも混ざらない。混ざると、無関係な保存どうしが待ち合う。
    #[test]
    fn a_profile_lock_is_not_the_work_lock() {
        let work = work_save_mutex("pixiv", "1234");
        let profile = profile_save_mutex("person", "pixiv", "1234");
        assert!(!std::sync::Arc::ptr_eq(&work, &profile));
    }
}
