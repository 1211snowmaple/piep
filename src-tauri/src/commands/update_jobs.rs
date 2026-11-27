//! 更新監視のジョブ。
//!
//! 監視している作品の改稿、作者の新作、シリーズの続編を一つのジョブで確かめ、
//! 選んだものだけを保存する。ジョブは一時停止・再開・中止ができ、失敗した
//! 項目だけを後から追える。
//!
//! **制限に当たった項目は、同じジョブの中で待ち直す。** pixiv の 429 は数秒で
//! 解けないことがある。詳細設定の回数（既定 3 回）まで、待ち時間を
//! `RATE_LIMIT_BACKOFF_BASE_MS` から `MAX_RATE_LIMIT_BACKOFF_MS` へ延ばし
//! ながら試す。短い固定回数で失敗にすると、待てば通るものを落とす。
//!
//! ログは1ジョブ `MAX_UPDATE_JOB_LOGS` 行で頭打ちにし、超えた分は古い方から
//! 落とす。長いジョブのログでライブラリを太らせない。

use super::downloader::shelf::{JOB_ACQUISITION_REQUEST, JOB_ACQUISITION_RETRY};
use crate::fanbox_api::models::FanboxPost;
use crate::library::{AcquisitionState, EntityKind, Library, LibraryId};
use crate::models::{
    StartUpdateJobRequest, UpdateCandidateInput, UpdateCredentials, UpdateJobItem,
    UpdateJobItemInput, UpdateJobItemState, UpdateJobSnapshot, UpdateJobSummary,
};
use crate::pixiv_api::error::PixivError;
use crate::AppState;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

#[path = "update_jobs_items.rs"]
pub(super) mod shelf;

#[cfg(test)]
#[path = "update_jobs_history_tests.rs"]
mod history_tests;

// 項目の間の時間は詳細設定（`advanced_settings`）にある。既定は通常の確認で 2 秒、
// 新作だけの確認で 1.5 秒。新作だけの確認は1人あたりの通信が一覧の1回だけになるので、
// この差で毎秒の通信量が通常の確認と同じ（実測で約 0.6 回/秒）になる。縮めると取得元への
// 圧が増えるだけで、通信の総数が減るわけではない。下限は既定の半分。
const FAST_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

// 制限で止まった項目を同じジョブの中でやり直す回数と、取得エラーが何件続いたら
// 止めるかは、詳細設定にある（どちらも既定 3 回）。pixiv の 429 は数秒では解除
// されないことがあるため、短い固定回数で失敗扱いにせず、十分な時間をかけて
// 段階的に待つ。
const RATE_LIMIT_BACKOFF_BASE_MS: u64 = 60_000;
const MAX_RATE_LIMIT_BACKOFF_MS: u64 = 5 * 60 * 1_000;
const ERROR_BACKOFF_BASE_MS: u64 = 10_000;
const MAX_ERROR_BACKOFF_MS: u64 = 60_000;
/// 1ジョブが持つログの上限。超えた分は古い方から落とす。
const MAX_UPDATE_JOB_LOGS: i64 = 2_000;
/// 何項目ごとにログの上限を確かめるか。
const LOG_TRIM_INTERVAL: u32 = 100;

/// 前回までの候補は全件たどるが、一度に読む本文データはこの件数に収める。
const PENDING_CANDIDATE_PAGE_SIZE: i64 = 500;

/// メタデータの指紋が同じでも、この日数を過ぎたら本文まで突き合わせる（詳細設定）。
///
/// 文字数の変わらない改稿を取りこぼさないための保険。短くすると取得回数が
/// 増え、長くすると細かい修正に気づくのが遅れる。
fn deep_check_window() -> chrono::Duration {
    chrono::Duration::days(crate::advanced_settings::deep_check_interval_days())
}
static ACTIVE_UPDATE_JOBS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static PENDING_UPDATE_RESTARTS: OnceLock<Mutex<HashMap<String, UpdateCredentials>>> =
    OnceLock::new();

/// web の一覧から引けたか、引けなかったか。
enum WebLookup {
    /// 一覧に載っていた。
    Found(Box<crate::pixiv_api::web::NovelListEntryWeb>),
    /// 一覧から欠け、作品詳細も 404。保存済みの内容は残して確認を保留する。
    Unavailable,
    /// 引けなかった。セッションが無い、伏せられている、作者が分からない、など。
    /// **失敗ではない。** 従来どおりアプリAPIで確かめればよい。
    Unknown,
    /// 一覧の確認に失敗した。理由を保ったままジョブの再試行・停止へ渡す。
    Failed(String),
}

#[derive(Clone, Default)]
struct CachedWebListing {
    entries: HashMap<String, crate::pixiv_api::web::NovelListEntryWeb>,
    unavailable_ids: HashSet<String>,
}

/// ジョブの間だけ生きる、web 一覧の覚え書き。
///
/// pixiv の web は 1 リクエストで 100 件ぶんの `updateDate` を返す。作者ごとに
/// 一度聞いて覚えておけば、作品ごとにアプリAPIを叩かずに「変わっていない」が
/// 分かる。セッションが無ければ作品単体は従来の経路へ進めるが、作者全体の
/// 改稿確認を「変更なし」として完了させてはいけない。
struct WebUpdateIndex {
    library: Option<Arc<crate::library::Library>>,
    api: Option<crate::pixiv_api::web::WebPixivAPI>,
    asked_authors: HashSet<String>,
    /// 作者ID → 一覧と、個別取得でも 404 だった保存済み作品。
    entries: HashMap<String, CachedWebListing>,
    /// 欠けた作品の 404 を確認できず一覧が不完全なら、以降このジョブでは使わない。
    /// 一件ごとに再接続を促しても、利用者にできることは増えない。
    gave_up: bool,
}

#[derive(Clone)]
struct FanboxListingEntry {
    updated_datetime: String,
    is_restricted: bool,
}

/// One full creator listing is shared by the saved works in a one-off author
/// check. Missing entries are never treated as unchanged: a removed post still
/// needs its own detail result so the local copy can be marked as preserved.
#[derive(Default)]
struct FanboxUpdateIndex {
    attempted_authors: HashSet<String>,
    entries: HashMap<String, HashMap<String, FanboxListingEntry>>,
    /// 新作の確認でも同じ一覧を使う。同じ作者の全ページを、1回のジョブで二度たどらない。
    posts: HashMap<String, Vec<FanboxPost>>,
}

impl FanboxUpdateIndex {
    /// このジョブで読んだ一覧のうち、`stop_source_id` より新しい投稿。読んでいなければ `None`。
    ///
    /// 一覧は新しい順なので、`get_creator_posts_since` と同じところで止まる。
    fn posts_since(
        &self,
        author_id: &str,
        stop_source_id: Option<&str>,
    ) -> Option<Vec<FanboxPost>> {
        let posts = self.posts.get(author_id)?;
        Some(
            posts
                .iter()
                .take_while(|post| stop_source_id != Some(post.id.as_str()))
                .cloned()
                .collect(),
        )
    }

    async fn lookup(
        &mut self,
        author_id: &str,
        post_id: &str,
        cookie: &str,
        user_agent: &str,
    ) -> Result<Option<&FanboxListingEntry>, String> {
        if !self.attempted_authors.contains(author_id) {
            let api =
                crate::downloader::session::fanbox(cookie.to_string(), user_agent.to_string())
                    .map_err(|error| error.to_string())?;
            match api
                .get_all_creator_posts(author_id)
                .await
                .and_then(|list| list.into_complete().map_err(Into::into))
            {
                Ok(posts) => {
                    self.posts.insert(author_id.to_string(), posts.clone());
                    self.entries.insert(
                        author_id.to_string(),
                        posts
                            .into_iter()
                            .map(|post| {
                                (
                                    post.id,
                                    FanboxListingEntry {
                                        updated_datetime: post.updated_datetime,
                                        is_restricted: post.is_restricted,
                                    },
                                )
                            })
                            .collect(),
                    );
                }
                Err(error) => {
                    if matches!(
                        error,
                        crate::fanbox_api::error::FanboxError::NoAuth
                            | crate::fanbox_api::error::FanboxError::RateLimited
                            | crate::fanbox_api::error::FanboxError::ChallengeRequired
                    ) {
                        return Err(error.to_string());
                    }
                    log::warn!(
                        "FANBOXの作者一覧を使えないため投稿ごとに確認します（作者 {author_id}）: {error}"
                    );
                }
            }
            self.attempted_authors.insert(author_id.to_string());
        }
        Ok(self
            .entries
            .get(author_id)
            .and_then(|items| items.get(post_id)))
    }
}

impl WebUpdateIndex {
    fn new(credentials: &UpdateCredentials) -> Self {
        // 保存済みの古い値にも同じ規則を通す。繋ぎ直していない利用者から、
        // 解析や広告の識別子が出ていくことはない。
        let cookie =
            crate::auth::essential_cookies(credentials.pixiv_cookie.as_deref().unwrap_or_default());
        let session = crate::pixiv_api::web::WebSession::new(
            &cookie,
            credentials.pixiv_user_agent.as_deref().unwrap_or_default(),
        );
        let api = match crate::pixiv_api::web::WebPixivAPI::new() {
            Ok(api) => Some(if let Some(session) = session {
                api.with_session(session)
            } else {
                api
            }),
            Err(error) => {
                log::warn!("pixiv web クライアントを作れません: {error}");
                None
            }
        };
        Self {
            library: None,
            api,
            asked_authors: HashSet::new(),
            entries: HashMap::new(),
            gave_up: false,
        }
    }

    /// その作者の、保存済み作品ぶんの一覧。
    ///
    /// **聞き方はひとつだけ。** 1件を確かめたいときも、作者の改稿をまとめて
    /// 見たいときも、同じ「その作者の保存済み作品を全部」を聞いて覚える。
    /// 用途ごとに違う範囲を聞き分けると、同じ作者を二度聞くことになる。
    ///
    /// セッションが無ければ `None`。作品単体は従来の経路へ進める。
    /// 作者全体の改稿確認では、呼ぶ側が未確認として扱う。
    async fn author_listing(
        &mut self,
        state: &Arc<AppState>,
        author_id: &str,
        job_id: &str,
    ) -> Result<Option<&CachedWebListing>, String> {
        let author_id = author_id.trim();
        if self.api.is_none() || self.gave_up || author_id.is_empty() {
            return Ok(None);
        }
        if !self.asked_authors.contains(author_id) {
            let library = self
                .library
                .as_ref()
                .ok_or_else(|| "棚は開いていません".to_string())?;
            let (library, author) = (Arc::clone(library), author_id.to_string());
            let ids: Vec<String> = tauri::async_runtime::spawn_blocking(move || {
                shelf::author_cards(&library, "pixiv", &author)
            })
            .await
            .map_err(|e| e.to_string())??
            .into_iter()
            .map(|card| card.external_key)
            .collect();
            self.asked_authors.insert(author_id.to_string());
            if ids.is_empty() {
                return Ok(Some(self.entries.entry(author_id.to_string()).or_default()));
            }
            let api = self.api.as_ref().expect("checked above");
            match api
                .user_novels_by_ids_with_control(author_id, &ids, || {
                    state
                        .jobs()
                        .update_job_status_value(job_id)
                        .is_ok_and(|status| status == "running")
                })
                .await
            {
                Ok(listed) => {
                    let bucket = self.entries.entry(author_id.to_string()).or_default();
                    for entry in listed.entries {
                        bucket.entries.insert(entry.id.clone(), entry);
                    }
                    bucket.unavailable_ids = listed.unavailable_ids.into_iter().collect();
                }
                // 個別の 404 で説明できない欠落は、セッション切れなどで
                // R-18 が黙って落ちている可能性がある。「更新なし」にはしない。
                Err(error @ PixivError::PartialListing { .. }) => {
                    self.gave_up = true;
                    return Err(error.to_string());
                }
                Err(error @ PixivError::RateLimited { .. }) => {
                    // 同じジョブ内の再試行でも、一覧を改めて取得する。
                    self.asked_authors.remove(author_id);
                    return Err(error.to_string());
                }
                Err(error @ PixivError::WebSessionExpired) => {
                    // 同じ失効 Cookie で残りの作者をすべて叩かない。再開時は
                    // 新しい資格情報から index を作り直す。
                    self.gave_up = true;
                    return Err(format!("pixivへの再接続が必要です（{error}）"));
                }
                Err(error) => {
                    self.asked_authors.remove(author_id);
                    return Err(error.to_string());
                }
            }
        }
        Ok(self.entries.get(author_id))
    }

    async fn lookup_key(
        &mut self,
        state: &Arc<AppState>,
        author: &str,
        key: &str,
        job_id: &str,
    ) -> WebLookup {
        match self.author_listing(state, author, job_id).await {
            Err(error) => WebLookup::Failed(error),
            Ok(None) => WebLookup::Unknown,
            Ok(Some(listing)) if listing.unavailable_ids.contains(key) => WebLookup::Unavailable,
            Ok(Some(listing)) => match listing.entries.get(key) {
                // 伏せられた作品はメタデータが当てにならない。判定に使わない。
                Some(entry) if entry.is_masked => WebLookup::Unknown,
                Some(entry) => WebLookup::Found(Box::new(entry.clone())),
                None => WebLookup::Unknown,
            },
        }
    }
}

fn active_jobs() -> &'static Mutex<HashSet<String>> {
    ACTIVE_UPDATE_JOBS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn pending_restarts() -> &'static Mutex<HashMap<String, UpdateCredentials>> {
    PENDING_UPDATE_RESTARTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn has_pending_restart(job_id: &str) -> bool {
    pending_restarts()
        .lock()
        .map(|pending| pending.contains_key(job_id))
        .unwrap_or(false)
}

fn make_job_id() -> String {
    crate::library::LibraryId::now_v7().to_string()
}

fn pixiv_token(credentials: &UpdateCredentials) -> Option<String> {
    credentials
        .pixiv_refresh_token
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn fanbox_cookie(credentials: &UpdateCredentials) -> Option<String> {
    credentials
        .fanbox_cookie
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn fanbox_user_agent(credentials: &UpdateCredentials) -> String {
    credentials
        .fanbox_user_agent
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Mozilla/5.0".to_string())
}

/// 更新監視の対象を、同時に確認すべきプロフィールへ写す。
///
/// 作者の新作一覧と作者プロフィールは別の API なので、target 項目を処理した
/// だけではアイコンは新しくならない。シリーズはプロフィール画像を持たないため、
/// ここでは作者だけを対象にする。
fn profile_refresh_params_for_target(
    target: &crate::models::UpdateTarget,
    credentials: &UpdateCredentials,
) -> Option<crate::commands::library::RefreshEntityProfileParams> {
    (target.target_type == "author").then(|| crate::commands::library::RefreshEntityProfileParams {
        entity_type: "person".to_string(),
        source: target.source.clone(),
        source_key: target.source_key.clone(),
        force: Some(false),
        refresh_token: pixiv_token(credentials),
        cookie: fanbox_cookie(credentials),
        user_agent: Some(fanbox_user_agent(credentials)),
    })
}

fn value_at<'a>(value: &'a Value, paths: &[&[&str]]) -> Option<&'a Value> {
    for path in paths {
        let mut cursor = value;
        let mut ok = true;
        for key in *path {
            if let Some(next) = cursor.get(*key) {
                cursor = next;
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(cursor);
        }
    }
    None
}

fn string_at(value: &Value, paths: &[&[&str]]) -> Option<String> {
    value_at(value, paths).and_then(|candidate| {
        if let Some(s) = candidate.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        candidate.as_i64().map(|n| n.to_string())
    })
}

fn normalize_pixiv_novel_id(item: &Value) -> Option<String> {
    string_at(item, &[&["id"], &["detail", "id"]])
}

/// 「何が変わったのか」を一言で。ログと通知に出す。
///
/// 文字数の増減は、改稿が加筆なのか削除なのか差し替えなのかを見分ける
/// いちばん軽い手がかりになる。
fn describe_text_change(before: i64, after: i64) -> String {
    match after - before {
        0 => "文字数は変わらず".to_string(),
        difference if difference > 0 => format!("+{}字", format_thousands(difference)),
        difference => format!("-{}字", format_thousands(-difference)),
    }
}

/// 版が増えた理由の一言。題名だけが直された更新は、文字数だけでは何が変わったのか分からない。
fn describe_source_change(
    before_title: &str,
    after_title: &str,
    before_length: i64,
    after_length: i64,
) -> String {
    let text = describe_text_change(before_length, after_length);
    if before_title == after_title {
        text
    } else {
        format!("題名を変更, {text}")
    }
}

fn format_thousands(value: i64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 候補を保存した結果を、何が起きたのかが分かる一文にする。
///
/// 新規保存と改稿の取り込みは同じ経路を通るので、手元にあった版と比べて
/// 文言を決める。改稿だと思って取りに行ったのに中身が同じだった場合
/// （配信元が更新時刻だけ動かした場合）も、そうと分かるようにする。
fn describe_candidate_save(
    shelf: &super::downloader::ShelfSaved,
    previous: Option<(&str, i64)>,
    masked: &[&str],
) -> ItemOutcome {
    let reference = shelf.work;
    // 挿絵や作品情報が欠けたままなら、本文を保存しても再試行できるよう候補を残す。
    if !shelf.missing_references.is_empty() || !masked.is_empty() {
        let mut losses = Vec::new();
        if !shelf.missing_references.is_empty() {
            losses.push(format!(
                "参照先の挿絵を閲覧できません: {}",
                shelf.missing_references.join(", ")
            ));
        }
        if !masked.is_empty() {
            losses.push(format!(
                "作品情報（{}）が取得元で伏せられています",
                crate::analysis::masked_metadata_label(masked)
            ));
        }
        return ItemOutcome::PartialSaved(
            reference,
            format!(
                "本文は保存しましたが、{}。候補を残して再試行できます",
                losses.join("。")
            ),
        );
    }
    match previous {
        Some((title, before)) if shelf.revised => ItemOutcome::Saved(
            reference,
            format!(
                "改稿を保存: {}（{}, v{}）",
                shelf.title,
                describe_source_change(title, &shelf.title, before, shelf.text_length),
                shelf.version
            ),
        ),
        Some(_) if !shelf.created => {
            ItemOutcome::Skipped(format!("中身は変わっていません: {}", shelf.title))
        }
        _ => ItemOutcome::Saved(reference, format!("新作を保存: {}", shelf.title)),
    }
}

/// 候補の種類。利用者が「何が起きたのか」を見て選べるようにする。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateKind {
    /// 監視している作者の、まだ持っていない作品。
    New,
    /// 監視しているシリーズの続き。
    Sequel,
    /// すでに持っている作品が、配信元で書き換えられている。
    Revision,
}

impl CandidateKind {
    fn key(self) -> &'static str {
        match self {
            CandidateKind::New => "new",
            CandidateKind::Sequel => "sequel",
            CandidateKind::Revision => "revision",
        }
    }
}

fn candidate_payload(
    target_type: &str,
    target_label: &str,
    source: &str,
    item: &Value,
    kind: CandidateKind,
) -> Option<Value> {
    let source_id = if source == "pixiv" {
        normalize_pixiv_novel_id(item)?
    } else {
        string_at(item, &[&["id"]])?
    };
    let title = string_at(item, &[&["title"]]).unwrap_or_else(|| "無題".to_string());
    let author = string_at(item, &[&["user", "name"]]).unwrap_or_else(|| target_label.to_string());
    let date = if source == "pixiv" {
        string_at(item, &[&["create_date"], &["createDate"]]).unwrap_or_default()
    } else {
        string_at(
            item,
            &[
                &["publishedDatetime"],
                &["published_datetime"],
                &["updatedDatetime"],
                &["updated_datetime"],
            ],
        )
        .unwrap_or_default()
    };
    Some(serde_json::json!({
        "source": source,
        "sourceId": source_id,
        "title": title,
        // 種類はバッジ、対象名は行の先頭に出る。副題はその重複を避けて
        // 「（対象と違えば）誰の」と「いつの」だけにする。
        // 改稿は「取得元が新しい」という話なので、公開日を並べても仕方がない。
        // 何が置き換わるのか - いま手元にある版 - を言う。
        "subtitle": match kind {
            CandidateKind::Revision => match (
                item.get("localVersion").and_then(Value::as_i64),
                string_at(item, &[&["localSavedAt"]]),
            ) {
                (Some(version), Some(saved)) => {
                    let title_changed = string_at(item, &[&["localTitle"]])
                        .is_some_and(|local| string_at(item, &[&["title"]]).is_some_and(|title| title != local));
                    format!(
                        "手元は v{version}（{}に保存）{}",
                        saved.chars().take(10).collect::<String>(),
                        if title_changed { "・題名が変わっています" } else { "" }
                    )
                }
                _ => date.clone(),
            },
            _ if author == target_label => date.clone(),
            _ => format!("{} ・ {}", author, date),
        },
        "kind": kind.key(),
        "targetLabel": target_label,
        "targetType": target_type,
        "originalData": item,
    }))
}

fn item_from_payload(
    item_type: &str,
    status: &str,
    source: Option<String>,
    source_id: Option<String>,
    target_type: Option<String>,
    title: String,
    payload: Value,
) -> Result<UpdateJobItemInput, String> {
    Ok(UpdateJobItemInput {
        item_type: item_type.to_string(),
        source,
        source_id,
        target_type,
        title,
        payload_json: serde_json::to_string(&payload).map_err(|e| e.to_string())?,
        status: status.to_string(),
    })
}

/// そのシリーズの作者を、すでに監視しているか。
///
/// 作者の一覧にはシリーズの新作も含まれるので、両方を回すと同じものを二度
/// 取りに行くことになる。手元のライブラリからシリーズの作者が分かるときだけ
/// 判断し、分からなければ従来どおりシリーズも走査する（取りこぼさない側に倒す）。
fn series_covered_by_watched_author(
    library: &crate::library::Library,
    target: &crate::models::UpdateTarget,
    watched_authors: &HashSet<(String, String)>,
) -> Result<bool, String> {
    if watched_authors.is_empty() {
        return Ok(false);
    }
    let authors = super::watching::series_authors(library, &target.source, &target.source_key)?;
    Ok(authors
        .iter()
        .any(|author| watched_authors.contains(&(target.source.clone(), author.clone()))))
}

fn candidate_matches_scope(
    candidate: &crate::models::UpdateCandidateRow,
    works: &HashSet<(String, String)>,
    targets: &HashSet<(String, String, String)>,
) -> bool {
    if candidate.kind == "revision"
        && works.contains(&(candidate.source.clone(), candidate.source_id.clone()))
    {
        return true;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&candidate.payload_json) else {
        return false;
    };
    let origin = string_at(&payload, &[&["originSourceKey"]]);
    if let (Some(kind), Some(key)) = (candidate.target_type.as_deref(), origin) {
        if targets.contains(&(candidate.source.clone(), kind.to_string(), key)) {
            return true;
        }
    }
    // Old candidates may lack originSourceKey. Match real provider IDs, never names.
    let author = string_at(
        &payload,
        &[
            &["originalData", "creatorId"],
            &["originalData", "creator_id"],
            &["originalData", "user", "id"],
        ],
    );
    let series = string_at(&payload, &[&["originalData", "series", "id"]]);
    author.is_some_and(|key| targets.contains(&(candidate.source.clone(), "author".into(), key)))
        || series
            .is_some_and(|key| targets.contains(&(candidate.source.clone(), "series".into(), key)))
}

fn build_initial_items(
    state: &Arc<AppState>,
    request: &StartUpdateJobRequest,
    library: &crate::library::Library,
) -> Result<Vec<UpdateJobItemInput>, String> {
    let mut items = Vec::new();
    let mut included_work_keys = HashSet::new();
    let scope = request.scope.as_str();
    // "targets" は監視グループの確認。`target_ids` に挙げた作者・シリーズだけを
    // 見て、作品ごとの監視は混ぜない。
    // 新作だけの確認は作者とシリーズの一覧だけを見る。
    let new_works_only = request.new_works_only.unwrap_or(false);
    let include_work = !new_works_only && matches!(scope, "all" | "work");
    let include_author = matches!(scope, "all" | "author" | "targets");
    let include_series = matches!(scope, "all" | "series" | "targets");
    let registered_targets =
        super::watching::selected_targets(library, request.target_ids.as_deref())?;
    let target_filter = request.target_ids.as_ref().map(|_| {
        registered_targets
            .iter()
            .map(|t| t.id)
            .collect::<HashSet<_>>()
    });
    // A one-off check from an entity page specifies its target by provider key.
    // Its scope names the kind, not every saved watch target of that kind.
    let include_registered_targets =
        target_filter.is_some() || request.adhoc_targets.as_ref().is_none_or(Vec::is_empty);
    let eligible_targets: HashSet<(String, String, String)> = registered_targets
        .iter()
        .filter(|t| {
            include_registered_targets
                && ((include_author && t.target_type == "author")
                    || (include_series && t.target_type == "series"))
                && target_filter.as_ref().is_none_or(|ids| ids.contains(&t.id))
        })
        .map(|t| {
            (
                t.source.clone(),
                t.target_type.clone(),
                t.source_key.clone(),
            )
        })
        .chain(request.adhoc_targets.iter().flatten().map(|t| {
            (
                t.source.clone(),
                t.target_type.clone(),
                t.source_key.clone(),
            )
        }))
        .collect();

    let held_work_keys = state.candidates().held_update_candidate_keys()?;
    for item in shelf::work_items(library, request, &eligible_targets)? {
        let key = (
            item.source.clone().unwrap(),
            item.source_id.clone().unwrap(),
        );
        if !held_work_keys.contains(&key) && included_work_keys.insert(key) {
            items.push(item);
        }
    }

    // 一回きりの確認先（作品ページや作者ページの「新作を確認」）。
    // 監視対象には登録しない。登録済みのものを指していれば、その前回位置を
    // 引き継いで無駄に遡らない。
    for adhoc in request.adhoc_targets.iter().flatten() {
        if adhoc.target_type != "author" && adhoc.target_type != "series" {
            continue;
        }
        let registered = super::watching::find(
            library,
            &adhoc.target_type,
            &adhoc.source,
            &adhoc.source_key,
        )?;
        let target = registered.unwrap_or_else(|| crate::models::UpdateTarget {
            id: None,
            watch_generation: None,
            target_type: adhoc.target_type.clone(),
            source: adhoc.source.clone(),
            source_key: adhoc.source_key.clone(),
            display_name: adhoc.display_name.clone(),
            enabled: true,
            last_checked_at: None,
            last_seen_source_id: None,
            last_seen_source_updated_at: None,
            metadata_json: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            last_hit_at: None,
            consecutive_errors: 0,
            watch_groups: Vec::new(),
            icon_path: None,
        });
        let title = target.display_name.clone();
        let payload = serde_json::to_value(&target).map_err(|e| e.to_string())?;
        items.push(item_from_payload(
            "target",
            "queued",
            Some(target.source.clone()),
            Some(target.source_key.clone()),
            Some(target.target_type.clone()),
            title,
            payload,
        )?);
    }

    // 前のジョブで見つけたまま、保存も拒否もされていない候補を先に戻す。
    // これをやらないと、取得元の一覧が前回位置から先しか返さないぶん、
    // 「一度出たきり二度と出ない作品」が生まれる。
    if include_work || include_author || include_series {
        let mut cursor: Option<(String, String)> = None;
        loop {
            let page = state.candidates().list_pending_update_candidates_after(
                cursor
                    .as_ref()
                    .map(|(source, source_id)| (source.as_str(), source_id.as_str())),
                PENDING_CANDIDATE_PAGE_SIZE,
            )?;
            let Some(last) = page.last() else {
                break;
            };
            cursor = Some((last.source.clone(), last.source_id.clone()));
            for candidate in page {
                // 前回の改稿候補も「新作だけ」の範囲には含めない。次の全件確認には残す。
                if new_works_only && candidate.kind == CandidateKind::Revision.key() {
                    continue;
                }
                // 作品だけを確認するときは、その確認対象の改稿だけを戻す。
                // 作者・シリーズ由来の新作候補や、別の作品の改稿まで混ぜない。
                if !candidate_matches_scope(&candidate, &included_work_keys, &eligible_targets) {
                    continue;
                }
                // すでに手元にあるものは、改稿として見つけた場合だけ残す。
                let saved =
                    shelf::saved_card(library, &candidate.source, &candidate.source_id)?.is_some();
                if saved && candidate.kind != CandidateKind::Revision.key() {
                    state
                        .candidates()
                        .clear_update_candidate(&candidate.source, &candidate.source_id)?;
                    continue;
                }
                items.push(UpdateJobItemInput {
                    item_type: "candidate".to_string(),
                    source: Some(candidate.source),
                    source_id: Some(candidate.source_id),
                    target_type: candidate.target_type,
                    title: candidate.title,
                    payload_json: candidate.payload_json,
                    // 確認のみのときは選ぶ前の状態で並べ、自動保存なら最初から待機列へ。
                    status: if request.mode == "auto_save" {
                        "queued".to_string()
                    } else {
                        "candidate".to_string()
                    },
                });
            }
        }
    }

    if include_registered_targets && (include_author || include_series) {
        let targets = registered_targets;
        // 作者を監視しているなら、その人のシリーズはその一覧に出てくる。
        // 両方を走査すると同じものを二度取りに行くので、取得先への負荷になる。
        let watched_authors: HashSet<(String, String)> = targets
            .iter()
            .filter(|target| {
                target.target_type == "author"
                    && target_filter
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&target.id))
            })
            .map(|target| (target.source.clone(), target.source_key.clone()))
            .collect();

        for target in targets {
            if target.target_type == "author" && !include_author {
                continue;
            }
            if target.target_type == "series" && !include_series {
                continue;
            }
            if target.target_type != "author" && target.target_type != "series" {
                continue;
            }
            if let Some(filter) = &target_filter {
                if !filter.contains(&target.id) {
                    continue;
                }
            }
            if target.target_type == "series"
                && include_author
                && series_covered_by_watched_author(library, &target, &watched_authors)?
            {
                continue;
            }
            let title = target.display_name.clone();
            let payload = serde_json::to_value(target).map_err(|e| e.to_string())?;
            items.push(item_from_payload(
                "target",
                "queued",
                payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                payload
                    .get("sourceKey")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                payload
                    .get("targetType")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                title,
                payload,
            )?);
        }
    }

    Ok(items)
}

async fn emit_snapshot(app: &tauri::AppHandle, state: &Arc<AppState>, job_id: &str) {
    match state.jobs().update_job_snapshot(job_id) {
        Ok(snapshot) => {
            if let Err(error) = app.emit("update-job-progress", snapshot) {
                log::warn!("更新ジョブ {job_id} の進捗を画面へ送れません: {error}");
            }
        }
        Err(error) => {
            log::error!("更新ジョブ {job_id} の進捗をDBから読めません: {error}");
        }
    }
}

/// Emit the small, per-transition form of progress.
///
/// A full snapshot contains up to 200 candidates and 300 logs. Sending that
/// after every item made a long save repeatedly serialize the same pages and
/// starved the WebView animation thread. Full snapshots are still used for
/// explicit loads and terminal/control transitions.
async fn emit_progress_delta(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    changed_item_id: Option<i64>,
) {
    match state
        .jobs()
        .update_job_progress_delta(job_id, changed_item_id)
    {
        Ok(delta) => {
            if let Err(error) = app.emit("update-job-progress-delta", delta) {
                log::warn!("更新ジョブ {job_id} の差分進捗を画面へ送れません: {error}");
            }
        }
        Err(error) => {
            log::error!("更新ジョブ {job_id} の差分進捗をDBから読めません: {error}");
        }
    }
}

/// 作者の投稿一覧を読み始めたことを画面へ知らせる。
///
/// 一覧を読むあいだ（長い作者で 30 秒ほど）、件数は動かない。先頭の作品の題名を
/// 出したままだと、その作品で止まったように見える。
async fn announce_author_listing(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    author_name: &str,
) -> Result<(), String> {
    let label = format!("{author_name}の投稿一覧を読んでいます");
    state
        .jobs()
        .transition_update_job_status(job_id, "running", "running", Some(&label))?;
    emit_progress_delta(app, state, job_id, None).await;
    Ok(())
}

fn spawn_update_job(app: tauri::AppHandle, job_id: String, credentials: UpdateCredentials) {
    let inserted = active_jobs()
        .lock()
        .map(|mut jobs| jobs.insert(job_id.clone()))
        .unwrap_or(false);
    if !inserted {
        // A paused worker can still be unwinding an in-flight request when the
        // user resumes. Remember the newest credentials and hand them to a new
        // worker after the old one reaches its safe boundary; silently dropping
        // this request used to leave a queued job with no worker.
        if let Ok(mut pending) = pending_restarts().lock() {
            pending.insert(job_id, credentials);
        }
        return;
    }
    tauri::async_runtime::spawn(async move {
        // Keep cleanup outside the worker task so a panic cannot strand the ID
        // in ACTIVE_UPDATE_JOBS forever.
        let worker_app = app.clone();
        let worker_job_id = job_id.clone();
        let worker = tauri::async_runtime::spawn(async move {
            crate::downloader::session::scope(run_update_job(
                worker_app,
                worker_job_id,
                credentials,
            ))
            .await
        });
        let run_result = match worker.await {
            Ok(result) => result,
            Err(error) => Err(format!("更新ジョブの処理が異常終了しました: {error}")),
        };
        if let Err(error) = run_result {
            let state = app.state::<Arc<AppState>>().inner().clone();
            if let Err(log_error) = state.jobs().append_update_job_log(&job_id, "error", &error) {
                log::error!("更新ジョブ {job_id} の失敗をDBへ記録できません: {log_error}");
            }
            if has_pending_restart(&job_id) {
                // The replacement worker owns the queued state. A panic or
                // late failure from the retired worker must not overwrite it.
                emit_snapshot(&app, &state, &job_id).await;
            } else {
                // A late network/database error must not resurrect a job after the
                // user canceled it while an item was in flight.
                match state.jobs().update_job_status_value(&job_id).as_deref() {
                    Ok("canceling") => {
                        if let Err(cancel_error) =
                            finish_update_job_cancellation(&app, &state, &job_id).await
                        {
                            log::error!(
                                "更新ジョブ {job_id} のキャンセルを確定できません: {cancel_error}"
                            );
                        }
                    }
                    Ok("canceled") => {}
                    _ => {
                        if let Err(status_error) =
                            state
                                .jobs()
                                .set_update_job_status(&job_id, "failed", Some(&error))
                        {
                            log::error!(
                                "更新ジョブ {job_id} を失敗状態へ移せません: {status_error}"
                            );
                        }
                    }
                }
                emit_snapshot(&app, &state, &job_id).await;
            }
        }
        if let Ok(mut jobs) = active_jobs().lock() {
            jobs.remove(&job_id);
        }
        let restart = pending_restarts()
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&job_id));
        if let Some(credentials) = restart {
            spawn_update_job(app, job_id, credentials);
        }
    });
}

fn canceled_status_for(current: &str) -> Result<Option<&'static str>, String> {
    match current {
        // These states have no worker that can observe a cancellation request.
        "paused" | "auth_required" | "failed" => Ok(Some("canceled")),
        // A queued/running worker will acknowledge the request at its next safe
        // boundary. Keeping "canceling" visible also tells the UI why it has
        // not stopped in the middle of an HTTP or file operation.
        "queued" | "running" | "canceling" => Ok(Some("canceling")),
        "completed" | "canceled" => Ok(None),
        other => Err(format!("更新ジョブをキャンセルできない状態です: {other}")),
    }
}

async fn finish_update_job_cancellation(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
) -> Result<(), String> {
    state.jobs().prepare_update_job_resume(job_id, false)?;
    state
        .jobs()
        .set_update_job_status(job_id, "canceled", Some("キャンセルしました"))?;
    state
        .jobs()
        .append_update_job_log(job_id, "warn", "更新ジョブをキャンセルしました")?;
    emit_snapshot(app, state, job_id).await;
    Ok(())
}

/// A network failure may return after the user has paused or canceled the job.
/// Preserve that control decision instead of reviving the old worker's status.
async fn transition_running_worker_status(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    next_status: &str,
    label: &str,
) -> Result<bool, String> {
    if state
        .jobs()
        .transition_update_job_status(job_id, "running", next_status, Some(label))?
    {
        return Ok(true);
    }
    if state.jobs().update_job_status_value(job_id)? == "canceling" {
        finish_update_job_cancellation(app, state, job_id).await?;
    }
    Ok(false)
}

/// 更新の確認を始める。返るのは開始直後の姿。
///
/// 確認する対象が一つも無ければ `Err`。**空のジョブを作らない**のは、進みも終わりも
/// 無いものが一覧に並ぶと、失敗したのか終わったのか見分けが付かないためである。
///
/// 作ったあとは背景で走る。進み具合は `update-job-progress` で流れるので、
/// 返り値を最終結果として扱わないこと。
#[tauri::command]
pub async fn start_update_job(
    app: tauri::AppHandle,
    request: StartUpdateJobRequest,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let library = shelf::library(&app)?;
    let (item_state, item_request) = (Arc::clone(&state), request.clone());
    let items = tauri::async_runtime::spawn_blocking(move || {
        build_initial_items(&item_state, &item_request, &library)
    })
    .await
    .map_err(|e| e.to_string())??;
    if items.is_empty() {
        return Err("更新監視対象がありません".to_string());
    }
    let job_id = make_job_id();
    let snapshot = state
        .jobs()
        .create_update_job_if_idle(&job_id, &request, &items)?;
    spawn_update_job(app.clone(), job_id.clone(), saved_credentials());
    emit_snapshot(&app, &state, &job_id).await;
    Ok(snapshot)
}

/// まとめ保存で預かる作品ひとつ。取得元とそのIDが分かれば足りる。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveJobWork {
    pub source: String,
    pub source_id: String,
    pub title: String,
    #[serde(default)]
    pub fanbox_access: Option<FanboxAccess>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanboxAccess {
    pub is_restricted: bool,
    pub account_key: String,
    pub fee_required: Option<u64>,
    pub creator_id: Option<String>,
}

/// 選んだ作品のまとめ保存を、こちら側の仕事として預かる。
///
/// 保存の繰り返しは画面の側にあった。818件を40分かけて取る仕事が画面と同じ
/// 寿命なので、**画面が消えれば仕事も消える**。実際に消えた - 描画側がメモリ
/// 不足で落ち、548件まで進んだ保存がまるごと失われた。再開する手立ても無く、
/// 「済んだものは飛ばす」に頼ってやり直すしかなかった。
///
/// 更新ジョブは同じ仕事を、DBに状態を持ち、起動時に再開し、一時停止も中止も
/// できる形でやっている。まとめ保存もそこへ載せる。項目の種類は候補と同じで
/// よい - `process_candidate_item` が取得元IDから取ってきて保存する。
#[tauri::command]
pub async fn start_save_job(
    app: tauri::AppHandle,
    works: Vec<SaveJobWork>,
    watch_saved: Option<bool>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let items = build_save_items(&works);
    if items.is_empty() {
        return Err("保存する作品が選ばれていません".to_string());
    }
    let request = StartUpdateJobRequest {
        // 走らせるのは候補の保存だけ。監視対象は見に行かない。
        scope: "save".to_string(),
        mode: "save".to_string(),
        work_ids: None,
        target_ids: None,
        watch_saved,
        adhoc_targets: None,
        new_works_only: None,
    };
    let job_id = make_job_id();
    // Saving outlives the page that launched it. After navigating away and
    // back, a fresh SavePage can submit the same candidates again while the
    // first worker is still running. The check and insert must be atomic.
    let snapshot = state
        .jobs()
        .create_update_job_if_idle(&job_id, &request, &items)?;
    state.jobs().append_update_job_log(
        &job_id,
        "info",
        &format!("{}件の保存を受け付けました", items.len()),
    )?;
    spawn_update_job(app.clone(), job_id.clone(), saved_credentials());
    emit_snapshot(&app, &state, &job_id).await;
    Ok(snapshot)
}

/// 取得元とIDが揃っているものだけを、同じ相手先で一度ずつ預かる。
///
/// 同じ作品が二度並ぶと、二度取りに行って二度目は「保存済み」で飛ぶ。
/// 無駄なだけでなく、件数の分母が実際より大きく出る。
fn build_save_items(works: &[SaveJobWork]) -> Vec<UpdateJobItemInput> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for work in works {
        let source = work.source.trim();
        let source_id = work.source_id.trim();
        if (source != "pixiv" && source != "fanbox") || source_id.is_empty() {
            continue;
        }
        if !seen.insert((source.to_string(), source_id.to_string())) {
            continue;
        }
        let title = if work.title.trim().is_empty() {
            source_id.to_string()
        } else {
            work.title.trim().to_string()
        };
        items.push(UpdateJobItemInput {
            item_type: "candidate".to_string(),
            source: Some(source.to_string()),
            source_id: Some(source_id.to_string()),
            // 監視対象を経由せず作品そのものを保存する項目。
            // 候補一覧の公開型は targetType を必須としているので、作品を示す
            // `work` を保存してDBと画面の型を揃える。
            target_type: Some("work".to_string()),
            title,
            // `kind` は保存済みの扱いを分ける。`save` は監視中のものだけ取り直す。
            payload_json: serde_json::json!({ "kind": "save", "originalData": work.fanbox_access })
                .to_string(),
            status: "queued".to_string(),
        });
    }
    items
}

/// 走っているジョブを止める。返るのは止めたあとの姿。
///
/// **今の項目が終わってから止まる。** 相手への問い合わせを途中で切らない。
///
/// 再開には `resume_update_job` を使う。
#[tauri::command]
pub async fn pause_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.jobs().prepare_update_job_resume(&job_id, false)?;
    state
        .jobs()
        .set_update_job_status(&job_id, "paused", Some("一時停止しました"))?;
    state
        .jobs()
        .append_update_job_log(&job_id, "warn", "更新ジョブを一時停止しました")?;
    emit_snapshot(&app, &state, &job_id).await;
    state.jobs().update_job_snapshot(&job_id)
}

/// 止めたジョブを続きから走らせる。
///
/// **鍵は保存してある接続から読み直す。** 止めている間に繋ぎ直されていることが
/// あるので、止めたときのものを持ち回さない。
///
/// `retryFailed` を立てると、失敗した項目もやり直す。既定は false で、まだ
/// 手を付けていないものだけを進める。
#[tauri::command]
pub async fn resume_update_job(
    app: tauri::AppHandle,
    job_id: String,
    retry_failed: Option<bool>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state
        .jobs()
        .resume_update_job_if_idle(&job_id, retry_failed.unwrap_or(false))?;
    state
        .jobs()
        .append_update_job_log(&job_id, "info", "更新ジョブを再開しました")?;
    spawn_update_job(app.clone(), job_id.clone(), saved_credentials());
    emit_snapshot(&app, &state, &job_id).await;
    state.jobs().update_job_snapshot(&job_id)
}

/// ジョブを中止する。返るのは要求したあとの姿。
///
/// **すぐ終わるとは限らない。** 走っている最中なら `canceling` になり、今の項目
/// が切れてから `canceled` へ移る。待っているだけの状態なら、その場で終わる。
///
/// すでに終わっているジョブに呼んでも、何も起きない（誤りではない）。
#[tauri::command]
pub async fn cancel_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let current = state.jobs().update_job_status_value(&job_id)?;
    match canceled_status_for(&current)? {
        Some("canceled") => finish_update_job_cancellation(&app, &state, &job_id).await?,
        Some("canceling") => {
            state
                .jobs()
                .set_update_job_status(&job_id, "canceling", Some("キャンセル中"))?;
            state.jobs().append_update_job_log(
                &job_id,
                "warn",
                "更新ジョブのキャンセルを要求しました",
            )?;
        }
        Some(_) | None => {}
    }
    // Never spawn a new worker to cancel a paused job. The old implementation
    // did so without credentials; that worker first changed the job back to
    // running and then commonly stopped at auth_required instead of canceled.
    emit_snapshot(&app, &state, &job_id).await;
    state.jobs().update_job_snapshot(&job_id)
}

/// ジョブの今の姿を返す。候補とログは頁で切って返す。
///
/// `candidateAfterId` は**その ID より後ろ**の候補、`logBeforeId` は**その ID より
/// 前**のログを返す。候補は増える方向、ログは遡る方向に読むので、向きが逆で
/// ある。取り違えると、いつまでも同じ頁が返る。
///
/// 読むだけ。常に呼ぶ必要はなく、変化は `update-job-progress` で流れてくる。
#[tauri::command]
pub async fn get_update_job(
    app: tauri::AppHandle,
    job_id: String,
    candidate_after_id: Option<i64>,
    log_before_id: Option<i64>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>();
    state
        .jobs()
        .update_job_snapshot_page(&job_id, candidate_after_id, log_before_id)
}

/// ジョブの項目ひとつひとつが、いまどうなっているかを返す。
///
/// 一覧の要約は「何件済んだか」しか持たない。画面が行ごとに印を付けるには、
/// どの作品がどうなったかが要る。
#[tauri::command]
pub async fn list_update_job_item_states(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<Vec<UpdateJobItemState>, String> {
    let state = app.state::<Arc<AppState>>();
    state.jobs().list_update_job_item_states(&job_id)
}

/// 実行中の「結果」に必要な保留項目だけを返す。
/// 候補が何千件あっても、結果タブを開いたときの転送量を保留件数に抑える。
#[tauri::command]
pub async fn list_update_job_held_item_states(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<Vec<UpdateJobItemState>, String> {
    let state = app.state::<Arc<AppState>>();
    state.jobs().list_update_job_held_item_states(&job_id)
}

/// ジョブの一覧を、要約だけで返す。
///
/// 候補もログも含まない。中身は `get_update_job` で取る。
#[tauri::command]
pub async fn list_update_jobs(app: tauri::AppHandle) -> Result<Vec<UpdateJobSummary>, String> {
    let state = app.state::<Arc<AppState>>();
    current_job_summaries(&state)
}

fn current_job_summaries(state: &AppState) -> Result<Vec<UpdateJobSummary>, String> {
    state.jobs().list_update_jobs()
}

fn clear_finished_jobs(state: &AppState, include_shelf: bool) -> Result<usize, String> {
    let library = state.shelf.library()?;
    let hidden = if include_shelf {
        library
            .set_job_history_hidden(crate::library::LibraryId::now_v7(), None, true)
            .map_err(|e| e.to_string())?
    } else {
        0
    };
    state
        .jobs()
        .clear_finished_update_jobs()
        .map(|count| count + hidden)
}

/// 選んだ候補を保存キューへ積み、ジョブを動かし直す。
///
/// **一件も増えなかったときは何も起こさない**（すでに積んである候補を選び直した
/// 場合がこれに当たる）。増えたときだけ状態を `queued` に戻して走らせる。
///
/// 積む間はライブラリの書き込みを止める。候補はバックアップに載るので、書き出し
/// の最中に書き換えると、パートごとに別の世代が写ってしまう。
///
/// 鍵は保存してある接続から読む。繋がっていなければ、保存の段で接続を求めて止まる。
#[tauri::command]
pub async fn save_update_job_candidates(
    app: tauri::AppHandle,
    job_id: String,
    candidate_ids: Vec<i64>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    // `update_candidates` はバックアップに載る。書き出しの最中に書き換えると、
    // パートごとに別の世代が写る。`AppState.library_gate` の約束は
    // 「すべての変更が write を取る」なので、ここも取る。
    let changed = {
        let _library_write_guard = state.library_gate.clone().write_owned().await;
        state
            .jobs()
            .queue_update_job_candidates(&job_id, &candidate_ids)?
    };
    if changed > 0 {
        state.jobs().append_update_job_log(
            &job_id,
            "info",
            &format!("{}件の候補を保存キューに追加しました", changed),
        )?;
        state
            .jobs()
            .set_update_job_status(&job_id, "queued", Some("候補保存待ち"))?;
        spawn_update_job(app.clone(), job_id.clone(), saved_credentials());
    }
    emit_snapshot(&app, &state, &job_id).await;
    state.jobs().update_job_snapshot(&job_id)
}

/// 仕事に渡す鍵。保存してある接続から、その都度読む。
///
/// 止めている間に繋ぎ直されていることがあるので、前に読んだものを持ち回さない。
fn saved_credentials() -> UpdateCredentials {
    crate::auth::saved::Vault::app().update_credentials()
}

#[cfg(test)]
fn snapshot_credentials_missing() -> UpdateCredentials {
    UpdateCredentials {
        pixiv_refresh_token: None,
        pixiv_cookie: None,
        pixiv_user_agent: None,
        fanbox_cookie: None,
        fanbox_user_agent: None,
        fanbox_account_key: None,
    }
}

/// ジョブの記録を消す。
///
/// **候補や取り込んだ作品は消えない。** 消えるのはジョブそのものの記録だけで、
/// 終わった仕事を一覧から片付けるためにある。
#[tauri::command]
pub async fn clear_update_job(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.jobs().clear_update_job(&job_id)
}

/// 終わった更新ジョブをまとめて消す。操作履歴の「完了履歴を消去」から呼ぶ。
#[tauri::command]
pub async fn clear_finished_update_jobs(
    app: tauri::AppHandle,
    include_shelf: Option<bool>,
) -> Result<usize, String> {
    let state = app.state::<Arc<AppState>>();
    clear_finished_jobs(&state, include_shelf.unwrap_or(true))
}

/// この作品を今後の候補から外す（あるいは、その決定を取り消す）。
///
/// チェックを外すだけでは「今は選ばない」の意味しかなく、次の確認でまた出る。
/// 二度と出したくない、を伝える口がこれ。
#[tauri::command]
pub async fn dismiss_update_candidate(
    app: tauri::AppHandle,
    source: String,
    source_id: String,
    dismissed: bool,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    let _library_write_guard = state.library_gate.clone().write_owned().await;
    state
        .candidates()
        .remember_job_candidate(&source, &source_id)?;
    state.candidates().set_update_candidate_status(
        &source,
        &source_id,
        if dismissed { "dismissed" } else { "pending" },
    )
}

/// 保留・非表示の候補の表示情報。本文は運ばず、再確認用の原データはDBに保持する。
#[tauri::command]
pub async fn list_deferred_update_candidates(
    app: tauri::AppHandle,
) -> Result<Vec<crate::models::UpdateCandidateRow>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    tokio::task::spawn_blocking(move || {
        state
            .candidates()
            .list_deferred_update_candidate_summaries(-1)
    })
    .await
    .map_err(|error| format!("保留候補を読み取れません: {error}"))?
}

/// ライブラリに保存済みで、取得状態の確認が必要な作品数を返す。
#[tauri::command]
pub async fn count_saved_deferred_update_candidates(app: tauri::AppHandle) -> Result<i64, String> {
    app.state::<Arc<AppState>>()
        .candidates()
        .count_saved_deferred_update_candidates()
}

/// 一部保存の補完で取り直す作品。`snapshot` は候補にした版で、試した印はこの版に付ける。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncompleteSaveRepairWork {
    pub source: &'static str,
    pub source_id: String,
    pub title: String,
    pub snapshot: String,
}

/// 補完の保存ジョブへ渡した作品と、候補にしたときの版。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncompleteSaveRepairAttempt {
    pub source_id: String,
    pub snapshot: String,
}

/// 一部保存の pixiv 作品のうち、今の版でまだ取り直していないものを `limit` 件まで返す。
#[tauri::command]
pub async fn list_incomplete_save_repair_works(
    app: tauri::AppHandle,
    limit: i64,
) -> Result<Vec<IncompleteSaveRepairWork>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let library = state.shelf.library()?;
    tokio::task::spawn_blocking(move || {
        library
            .incomplete_save_repairs(
                limit.clamp(1, 500) as usize,
                crate::advanced_settings::repair_attempts(),
            )
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("補完する作品を読み取れません: {error}"))?
    .map(|works| {
        works
            .into_iter()
            .map(|(source_id, title, snapshot)| IncompleteSaveRepairWork {
                source: "pixiv",
                source_id,
                title,
                snapshot: snapshot.to_string(),
            })
            .collect()
    })
}

/// 補完の保存ジョブへ渡した作品を、候補にしたときの版で試したと記録する。
#[tauri::command]
pub async fn record_incomplete_save_repair_attempts(
    app: tauri::AppHandle,
    attempts: Vec<IncompleteSaveRepairAttempt>,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let library = state.shelf.library()?;
    let attempts = attempts
        .into_iter()
        .map(|attempt| {
            let snapshot = attempt
                .snapshot
                .parse::<crate::library::LibraryId>()
                .map_err(|_| "補完した版の ID が不正です".to_string())?;
            Ok((attempt.source_id, snapshot))
        })
        .collect::<Result<Vec<_>, String>>()?;
    tokio::task::spawn_blocking(move || {
        library
            .record_incomplete_save_attempts(&attempts, chrono::Utc::now().timestamp_millis())
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("補完の記録を書けません: {error}"))?
}

/// Rechecks an explicit selection after permissions change, without saving or watching.
#[tauri::command]
pub async fn recheck_deferred_update_candidates(
    app: tauri::AppHandle,
    keys: Vec<String>,
    refresh_post_access: Option<bool>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if keys.is_empty() || keys.len() > 2000 {
        return Err("再確認する作品を1〜2000件選んでください".into());
    }
    if refresh_post_access == Some(true) && keys.len() > 20 {
        return Err(
            "投稿側の公開条件の再確認は、取得元への負担を避けるため20件ずつ選んでください".into(),
        );
    }
    let keys: HashSet<_> = keys.into_iter().collect();
    let mut items = Vec::new();
    for candidate in state
        .candidates()
        .list_update_candidates_by_status("deferred", -1)?
    {
        if !keys.contains(&format!("{}:{}", candidate.source, candidate.source_id)) {
            continue;
        }
        let mut payload: Value =
            serde_json::from_str(&candidate.payload_json).map_err(|e| e.to_string())?;
        payload["kind"] = serde_json::json!("permission_check");
        payload["refreshPostAccess"] = serde_json::json!(refresh_post_access.unwrap_or(false));
        if let Some(generation) = state
            .candidates()
            .generation(&candidate.source, &candidate.source_id)?
        {
            payload["discoveryGeneration"] = serde_json::json!(generation);
        }
        items.push(item_from_payload(
            "candidate",
            "queued",
            Some(candidate.source),
            Some(candidate.source_id),
            candidate.target_type,
            candidate.title,
            payload,
        )?);
    }
    if items.is_empty() {
        return Err("対象はすでに保留一覧から移動しています".into());
    }
    static LAST_CHECK: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    {
        let mut last = LAST_CHECK
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|e| e.to_string())?;
        if last.is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(60)) {
            return Err(
                "取得元への連続問い合わせを避けるため、再確認は1分以上あけてください".into(),
            );
        }
        *last = Some(std::time::Instant::now());
    }
    let request = StartUpdateJobRequest {
        scope: "save".into(),
        mode: "check_only".into(),
        work_ids: None,
        target_ids: None,
        watch_saved: Some(false),
        new_works_only: None,
        adhoc_targets: None,
    };
    let job_id = make_job_id();
    let snapshot = state
        .jobs()
        .create_update_job_if_idle(&job_id, &request, &items)?;
    state.jobs().append_update_job_log(
        &job_id,
        "info",
        "保留した作品の閲覧条件を再確認します。保存や監視登録は行いません",
    )?;
    spawn_update_job(app.clone(), job_id.clone(), saved_credentials());
    emit_snapshot(&app, &state, &job_id).await;
    Ok(snapshot)
}

/// 保留の理由から、保留の種類を決める。
fn hold_kind_for(reason: &str) -> &'static str {
    if reason.starts_with("[一部保存]") {
        "incomplete"
    } else if reason.starts_with("[見つからない]") || reason.contains("公開元で投稿が見つからない")
    {
        "unavailable"
    } else if reason.starts_with('[') && !reason.starts_with("[閲覧制限]") {
        "error"
    } else {
        "restricted"
    }
}

/// 保留中・非表示の候補は取りに行かない。その知らせ。
const BLOCKED_CANDIDATE_MESSAGE: &str =
    "候補から外しています。保留・非表示の一覧から再確認してください";

/// 前の回から同じ種類の理由で保留のままの作品か。保留を書き直す前に聞く。
///
/// 確認のたびに同じ作品を「保留」と数えていたので、何も変わっていないのに
/// 毎回「完了（保留あり）」と通知が出ていた。確認そのものは続け、新しく保留に
/// なったものと、保留の種類が変わったものだけを知らせる。
fn is_known_hold(state: &AppState, item: &UpdateJobItem, reason: &str) -> Result<bool, String> {
    let (Some(source), Some(source_id)) = (item.source.as_deref(), item.source_id.as_deref())
    else {
        return Ok(false);
    };
    Ok(
        match state
            .candidates()
            .update_candidate_hold(source, source_id)?
        {
            Some((status, kind)) if status == "held" || status == "dismissed" => {
                reason == BLOCKED_CANDIDATE_MESSAGE
                    || kind.as_deref() == Some(hold_kind_for(reason))
            }
            _ => false,
        },
    )
}

/// 保留を項目に書き、ログに残す。前の回と変わらない保留は数えず、警告にもしない。
fn complete_held_item(
    state: &AppState,
    job_id: &str,
    item: &UpdateJobItem,
    known: bool,
    reason: &str,
    saved: Option<SavedRef>,
    log: &str,
) -> Result<(), String> {
    state
        .jobs()
        .complete_saved_item(item.id, "held", Some(reason), saved)?;
    if known {
        state.jobs().mark_update_job_item_known_hold(item.id)?;
        state.jobs().append_update_job_log(
            job_id,
            "info",
            &format!("{log}（前回から変わらない保留）"),
        )
    } else {
        state.jobs().append_update_job_log(job_id, "warn", log)
    }
}

async fn hold_candidate(
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    post: Option<&Value>,
    reason: &str,
) -> Result<(), String> {
    let _library_write_guard = state.library_gate.write().await;
    let source = item.source.as_deref().ok_or("保留する取得元がありません")?;
    let source_id = item
        .source_id
        .as_deref()
        .ok_or("保留する作品IDがありません")?;
    let mut payload: Value =
        serde_json::from_str(&item.payload_json).unwrap_or_else(|_| serde_json::json!({}));
    let kind = if state.candidates().saved(source, source_id)? {
        "revision"
    } else {
        "new"
    };
    payload["kind"] = serde_json::json!(kind);
    payload["source"] = serde_json::json!(source);
    payload["sourceId"] = serde_json::json!(source_id);
    let hold_kind = hold_kind_for(reason);
    payload["holdKind"] = serde_json::json!(hold_kind);
    payload["holdReason"] = serde_json::json!(reason);
    if let Some(post) = post {
        // Store only listing/permission metadata, never a body or credentials.
        payload["originalData"] = serde_json::json!({
            "id": source_id, "title": item.title,
            "creatorId": post.get("creatorId").or_else(|| post.get("creator_id")),
            "feeRequired": post.get("feeRequired"), "isRestricted": hold_kind == "restricted",
            "user": { "name": string_at(post, &[&["user", "name"]]) }
        });
    }
    if hold_kind != "restricted" {
        payload["originalData"]["isRestricted"] = serde_json::json!(false);
    }
    state.candidates().hold(&UpdateCandidateInput {
        source: source.into(),
        source_id: source_id.into(),
        kind: kind.into(),
        title: item.title.clone(),
        payload_json: payload.to_string(),
        target_type: item.target_type.clone(),
    })?;
    state
        .jobs()
        .set_update_job_item_payload(item.id, &payload.to_string())
}

async fn recheck_deferred_item(
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    mut payload: Value,
) -> Result<ItemOutcome, String> {
    let expected = payload.get("discoveryGeneration").and_then(Value::as_i64);
    let source = item.source.as_deref().ok_or("取得元がありません")?;
    let id = item.source_id.as_deref().ok_or("作品IDがありません")?;
    if source == "fanbox" && payload.get("holdKind").and_then(Value::as_str) == Some("restricted") {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "設定でFANBOXに接続してください".into(),
            ));
        };
        let ua = fanbox_user_agent(credentials);
        let creator = string_at(
            &payload,
            &[
                &["originalData", "creatorId"],
                &["originalData", "creator_id"],
            ],
        );
        let fee = payload
            .pointer("/originalData/feeRequired")
            .and_then(Value::as_u64);
        if let (Some(creator), Some(fee)) = (creator, fee) {
            if fee > 0 && payload.get("refreshPostAccess").and_then(Value::as_bool) != Some(true) {
                let fees =
                    crate::downloader::session::supporting_fees(cookie.clone(), ua.clone()).await?;
                if fees.get(&creator).copied().unwrap_or(0) < fee {
                    return Ok(ItemOutcome::Held(format!("必要な支援額は月額{fee}円以上です。条件が変わっていないため投稿本文は再取得せず、保留を続けます")));
                }
            }
        }
        let post = super::downloader::fanbox_post(id.into(), cookie, ua).await?;
        if post.get("isRestricted").and_then(Value::as_bool) == Some(true)
            || !super::downloader::fetched_has_material_content(&post, "fanbox")
        {
            let reason = super::downloader::fanbox_missing_material_error(&post);
            if post.get("isRestricted").and_then(Value::as_bool) == Some(true) {
                hold_candidate(state, item, Some(&post), &reason).await?;
                return Ok(ItemOutcome::Held(reason));
            }
            return Err(reason);
        }
    }
    let _library_write_guard = state.library_gate.write().await;
    let kind = if state.candidates().saved(source, id)? {
        "revision"
    } else {
        "new"
    };
    payload["kind"] = serde_json::json!(kind);
    if let Some(object) = payload.as_object_mut() {
        object.remove("holdKind");
        object.remove("holdReason");
        object.remove("refreshPostAccess");
        object.remove("discoveryGeneration");
    }
    if let Some(original) = payload
        .get_mut("originalData")
        .and_then(Value::as_object_mut)
    {
        original.insert("isRestricted".into(), serde_json::json!(false));
        if kind == "new" {
            original.remove("localVersion");
            original.remove("localSavedAt");
        }
    }
    if kind == "new" {
        payload["subtitle"] = serde_json::json!("未保存");
    }
    state.candidates().recheck_available(
        &UpdateCandidateInput {
            source: source.into(),
            source_id: id.into(),
            kind: kind.into(),
            title: item.title.clone(),
            payload_json: payload.to_string(),
            target_type: item.target_type.clone(),
        },
        expected,
    )?;
    state
        .jobs()
        .set_update_job_item_payload(item.id, &payload.to_string())?;
    Ok(ItemOutcome::Available(format!(
        "{}: 候補に戻しました。保存するか選んでください",
        item.title
    )))
}

/// 「今は要らない」と断った候補の数を返す。
///
/// 断ったものは次から出てこないので、**この数を見せないと、戻す入口が無いことに
/// 利用者が気づけない**。0 のときは入口ごと隠してよい。
#[tauri::command]
pub async fn count_dismissed_update_candidates(app: tauri::AppHandle) -> Result<i64, String> {
    let state = app.state::<Arc<AppState>>();
    state.candidates().count_dismissed_update_candidates()
}

/// まだ取り込んでいない改稿がある作品。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRevisionWork {
    /// 作品の画面と同じ ID（作品の UUID）。
    pub work: String,
    /// 更新確認がこの改稿を見つけた時刻。取得元が直した日そのものは持っていない。
    pub found_at: String,
}

/// まだ取り込んでいない改稿がある作品。
///
/// 画面はこれを一度だけ引いて、手元にある作品と突き合わせる。作品の一覧に
/// 列を足さないのは、これが作品の属性ではなく更新側が見つけた事実だから。
#[tauri::command]
pub async fn list_pending_revisions(
    app: tauri::AppHandle,
) -> Result<Vec<PendingRevisionWork>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    tokio::task::spawn_blocking(move || {
        let library = state.shelf.library()?;
        library
            .pending_revisions()
            .map_err(|error| error.to_string())
            .map(|rows| {
                rows.into_iter()
                    .map(|(work, found)| PendingRevisionWork {
                        work: work.to_string(),
                        found_at: chrono::DateTime::from_timestamp_millis(found)
                            .unwrap_or_default()
                            .to_rfc3339(),
                    })
                    .collect()
            })
    })
    .await
    .map_err(|error| format!("改稿の一覧を読み取れません: {error}"))?
}

/// A provider revision fetched only for comparison. It is deliberately not
/// written to `downloads` or `download_versions`; opening a diff must not
/// silently accept the revision as a new saved version.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRevisionPreview {
    pub metadata: crate::analysis::RevisionMetadata,
    /// 作品の画面と同じ ID（`list_pending_revisions` の `work`）。
    pub work: String,
    pub base_version: i64,
    pub text: String,
    pub text_length: i64,
}

/// 改稿の中身を、保存する前に取ってきて見せる。
///
/// **相手に問い合わせる。** ライブラリは変えないが、通信は起きるので、一覧の
/// 行ごとに先回りで呼ばないこと。
///
/// 鍵が要る。取得元に繋がっていなければ `Err`。
#[tauri::command]
pub async fn preview_pending_revision(
    app: tauri::AppHandle,
    work: String,
) -> Result<PendingRevisionPreview, String> {
    const NONE: &str = "この作品には未保存の改稿がありません";
    let credentials = saved_credentials();
    let state = app.state::<Arc<AppState>>().inner().clone();
    let lookup_state = state.clone();
    let lookup_work = work.clone();
    let (source, source_id, base_version) = tokio::task::spawn_blocking(move || {
        let library = lookup_state.shelf.library()?;
        let id = crate::library::LibraryId::parse_str(&lookup_work)
            .map_err(|_| "作品の指定が不正です".to_string())?;
        if !library
            .has_pending_revision(id)
            .map_err(|error| error.to_string())?
        {
            return Err(NONE.to_string());
        }
        let card = library.work_card(id).map_err(|error| error.to_string())?;
        Ok((card.provider, card.external_key, card.version_number as i64))
    })
    .await
    .map_err(|error| error.to_string())??;

    let value = match source.as_str() {
        "pixiv" => {
            let token = pixiv_token(&credentials)
                .ok_or_else(|| "差分を取るには、設定でpixivに接続してください".to_string())?;
            let novel = super::downloader::fetch_pixiv_novel(source_id.clone(), token).await?;
            serde_json::to_value(novel).map_err(|error| error.to_string())?
        }
        "fanbox" => {
            let cookie = fanbox_cookie(&credentials)
                .ok_or_else(|| "差分を取るには、設定でFANBOXに接続してください".to_string())?;
            super::downloader::fanbox_post(
                source_id.clone(),
                cookie,
                fanbox_user_agent(&credentials),
            )
            .await?
        }
        _ => return Err("この取得元の改稿比較には対応していません".to_string()),
    };
    let text = super::downloader::fetched_plain_text(&value, &source);
    if !super::downloader::fetched_has_material_content(&value, &source) {
        return Err("取得元から比較できる本文・画像・添付を取得できませんでした".to_string());
    }
    let text_length = text.chars().count() as i64;
    Ok(PendingRevisionPreview {
        metadata: crate::analysis::revision_metadata(&value, &source, None),
        work,
        base_version,
        text,
        text_length,
    })
}

/// 無視した作品をすべて戻す。次の確認でまた候補に並ぶ。
#[tauri::command]
pub async fn restore_dismissed_update_candidates(app: tauri::AppHandle) -> Result<usize, String> {
    let state = app.state::<Arc<AppState>>();
    let _library_write_guard = state.library_gate.clone().write_owned().await;
    state.candidates().restore_dismissed_update_candidates()
}

async fn run_update_job(
    app: tauri::AppHandle,
    job_id: String,
    credentials: UpdateCredentials,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let starting_status = state.jobs().update_job_status_value(&job_id)?;
    match starting_status.as_str() {
        "canceling" => {
            finish_update_job_cancellation(&app, &state, &job_id).await?;
            return Ok(());
        }
        "queued" | "running" => {}
        // A stale scheduled task must not revive a paused or terminal job.
        _ => return Ok(()),
    }
    if !state.jobs().transition_update_job_status(
        &job_id,
        &starting_status,
        "running",
        Some("更新チェックを開始しています"),
    )? {
        if state.jobs().update_job_status_value(&job_id)? == "canceling" {
            finish_update_job_cancellation(&app, &state, &job_id).await?;
        }
        return Ok(());
    }
    emit_snapshot(&app, &state, &job_id).await;

    // 保存した作品をそのまま監視に載せるかは、ジョブを始めたときの依頼が持つ。
    // 再開したワーカーも同じ設定で動く。
    let watch_saved = state
        .jobs()
        .update_job_watch_saved(&job_id)
        .unwrap_or(false);
    let new_works_only = state
        .jobs()
        .update_job_new_works_only(&job_id)
        .unwrap_or(false);
    crate::downloader::session::set_pixiv_web_session(
        credentials.pixiv_cookie.as_deref(),
        credentials.pixiv_user_agent.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    // web の一覧はジョブの間だけ覚えておく。セッションが無ければ空のまま、
    // 何もしない置物として振る舞う。
    let mut web_index = WebUpdateIndex::new(&credentials);
    web_index.library = shelf::library(&app).ok();
    let mut fanbox_index = FanboxUpdateIndex::default();
    // 通常の失敗が続く場合も、取得元を続けて叩かず停止する。
    let mut consecutive_errors: u32 = 0;
    let mut rate_limit_retries: HashMap<i64, u32> = HashMap::new();
    // 詳細設定はジョブの始めに一度読む。途中で変えても、走っているジョブは変わらない。
    let retry_limit = crate::advanced_settings::rate_limit_retries();
    let error_limit = crate::advanced_settings::consecutive_error_limit();
    let mut processed_since_trim: u32 = 0;
    // The save page uses each changed-item event to update its individual
    // rows. Preserve that behavior for explicit save jobs.
    let throttle_fast_progress = state.jobs().update_job_mode_value(&job_id)? != "save";
    let mut last_progress_emit = Instant::now() - FAST_PROGRESS_INTERVAL;

    loop {
        if has_pending_restart(&job_id) {
            break;
        }
        let status = state.jobs().update_job_status_value(&job_id)?;
        if status == "paused"
            || status == "auth_required"
            || status == "completed"
            || status == "failed"
            || status == "canceled"
        {
            break;
        }
        if status == "canceling" {
            state
                .jobs()
                .set_update_job_status(&job_id, "canceled", Some("キャンセルしました"))?;
            state.jobs().append_update_job_log(
                &job_id,
                "warn",
                "更新ジョブをキャンセルしました",
            )?;
            emit_snapshot(&app, &state, &job_id).await;
            // 中止でも、そこまでに保存できた作品は手元に残っている。完了と
            // 同じように索引の遅れを拾う。
            catch_up_semantic_index(&state);
            break;
        }

        let Some(item) = state.jobs().next_update_job_item(&job_id)? else {
            let snapshot = state.jobs().update_job_snapshot(&job_id)?;
            // 仕上げは、完了と印を付ける前に。「完了」が出てからまだ取りに
            // 行っているのでは、画面が閉じられる頃に途中で切れる。
            // まとめ保存だけでなく、更新監視が新作を自動保存した場合にも
            // 作者・シリーズの簡易情報が生まれる。保存済み項目がある全ジョブで
            // 未完了だけを仕上げる。
            // A worker can be replaced after an app restart or a rate-limit pause.
            // Rebuild the complete saved set from durable items so profiles saved
            // by an earlier worker are not omitted.
            if snapshot.saved_count > 0 {
                refresh_profiles_for_saved_works(&app, &state, &job_id, &credentials).await;
                // プロフィール取得中に停止要求が来た場合、仕上げを完了扱いに
                // してしまわない。残りの作品は保存済みなので、次回の保存で
                // プロフィールを再取得できる。
                match state.jobs().update_job_status_value(&job_id)?.as_str() {
                    "canceling" => {
                        finish_update_job_cancellation(&app, &state, &job_id).await?;
                        break;
                    }
                    "paused" => break,
                    _ => {}
                }
            }
            let final_status = if snapshot.error_count > 0 {
                "failed"
            } else {
                "completed"
            };
            let label = if final_status == "failed" {
                "エラーが出たまま終了しました"
            } else {
                "完了しました"
            };
            if !state.jobs().transition_update_job_status(
                &job_id,
                "running",
                final_status,
                Some(label),
            )? {
                if state.jobs().update_job_status_value(&job_id)? == "canceling" {
                    finish_update_job_cancellation(&app, &state, &job_id).await?;
                }
                break;
            }
            state.jobs().append_update_job_log(
                &job_id,
                if final_status == "failed" {
                    "error"
                } else {
                    "success"
                },
                label,
            )?;
            emit_snapshot(&app, &state, &job_id).await;
            // 取り込んだ作品は保存のたびに索引へ入るが、そこで失敗したものは
            // 「古い」印だけが残る。起動時の保守は次にアプリを開くまで走らない
            // ので、それまで**保存できているのに検索に出ない作品**ができる。
            // 取り込みが一区切りしたここで拾っておく。索引に遅れが無ければ
            // 件数を見て何もせずに戻るので、通常の終了は何も変わらない。
            catch_up_semantic_index(&state);
            break;
        };

        // Claiming the row already changed its state and active label. Tell the
        // save screen which single row became active without shipping all rows.
        // Cached author listings can settle many works without a network
        // request. Keep the title responsive without flooding the WebView and
        // repeating the summary query twice for every local-only item.
        if !throttle_fast_progress || last_progress_emit.elapsed() >= FAST_PROGRESS_INTERVAL {
            emit_progress_delta(&app, &state, &job_id, Some(item.id)).await;
            last_progress_emit = Instant::now();
        }

        let outcome = process_update_job_item(
            &app,
            &state,
            &job_id,
            &item,
            &credentials,
            &mut web_index,
            &mut fanbox_index,
        )
        .await;
        let local_only = matches!(&outcome, Ok(ItemOutcome::LocalSkipped(_)));
        let failed = outcome.is_err();
        let restart_pending = has_pending_restart(&job_id);
        let mut next_delay_ms = if new_works_only && item.item_type == "target" {
            crate::advanced_settings::new_works_delay_ms()
        } else {
            crate::advanced_settings::update_check_delay_ms()
        };
        if item.item_type == "work"
            && matches!(
                &outcome,
                Ok(ItemOutcome::LocalSkipped(_)
                    | ItemOutcome::Skipped(_)
                    | ItemOutcome::Done(_)
                    | ItemOutcome::Saved(_, _))
            )
        {
            if let (Some(source), Some(source_id)) =
                (item.source.as_deref(), item.source_id.as_ref())
            {
                let _guard = state.library_gate.write().await;
                state.candidates().clear_resolved_unavailable_candidates(
                    source,
                    std::slice::from_ref(source_id),
                )?;
            }
        }
        match outcome {
            Ok(ItemOutcome::Held(message)) => {
                let known = is_known_hold(&state, &item, &message)?;
                if item.item_type == "work" {
                    hold_candidate(&state, &item, None, &message).await?;
                }
                complete_held_item(
                    &state,
                    &job_id,
                    &item,
                    known,
                    &message,
                    None,
                    &format!("{}: 保留 — {message}", item.title),
                )?;
            }
            Ok(ItemOutcome::Available(message)) => {
                state
                    .jobs()
                    .complete_update_job_item(item.id, "candidate", None)?;
                state
                    .jobs()
                    .append_update_job_log(&job_id, "success", &message)?;
            }
            Ok(ItemOutcome::Done(message)) => {
                state
                    .jobs()
                    .complete_update_job_item(item.id, "done", None)?;
                state
                    .jobs()
                    .append_update_job_log(&job_id, "success", &message)?;
            }
            Ok(ItemOutcome::Saved(saved, message)) => {
                state
                    .jobs()
                    .complete_saved_item(item.id, "saved", None, Some(saved))?;
                // 保存できた候補は、もう「決めていないもの」ではない。
                if let (Some(source), Some(source_id)) =
                    (item.source.as_deref(), item.source_id.as_deref())
                {
                    if let Err(error) = state.candidates().clear_update_candidate(source, source_id)
                    {
                        log::warn!(
                            "保存済み候補 {source}:{source_id} を候補一覧から外せません: {error}"
                        );
                    }
                }
                state
                    .jobs()
                    .append_update_job_log(&job_id, "success", &message)?;
                // 保存したものを続けて追いたい、という設定のときだけ監視に載せる。
                // 失敗しても保存そのものは済んでいるので、警告に留める。
                if watch_saved {
                    watch_saved_work(&app, saved);
                }
            }
            Ok(ItemOutcome::PartialSaved(saved, message)) => {
                // The novel body is usable, but the candidate must remain
                // discoverable because its referenced illustrations are absent.
                let reason = format!("[一部保存] {message}");
                let known = is_known_hold(&state, &item, &reason)?;
                if item.item_type != "target" {
                    hold_candidate(&state, &item, None, &reason).await?;
                }
                complete_held_item(&state, &job_id, &item, known, &reason, Some(saved), &reason)?;
            }
            Ok(ItemOutcome::Skipped(message) | ItemOutcome::LocalSkipped(message)) => {
                state
                    .jobs()
                    .complete_update_job_item(item.id, "skipped", None)?;
                // 候補を実際に取りに行き、正常に「保存不要」と判定できたなら
                // 未処理として残さない。改稿では、取得元の更新時刻だけが動いて
                // 本文は同じだった場合がある。認証待ちと取得失敗は別分岐なので、
                // 再試行が必要な候補までここで消えることはない。
                if item.item_type == "candidate" {
                    if let (Some(source), Some(source_id)) =
                        (item.source.as_deref(), item.source_id.as_deref())
                    {
                        if let Err(error) =
                            state.candidates().clear_update_candidate(source, source_id)
                        {
                            log::warn!(
                                "処理済み候補 {source}:{source_id} を候補一覧から外せません: {error}"
                            );
                        }
                    }
                }
                state
                    .jobs()
                    .append_update_job_log(&job_id, "info", &message)?;
            }
            Ok(ItemOutcome::AuthRequired(message)) => {
                state
                    .jobs()
                    .complete_update_job_item(item.id, "queued", Some(&message))?;
                if restart_pending {
                    break;
                } else {
                    if !transition_running_worker_status(
                        &app,
                        &state,
                        &job_id,
                        "auth_required",
                        &message,
                    )
                    .await?
                    {
                        break;
                    }
                    state
                        .jobs()
                        .append_update_job_log(&job_id, "warn", &message)?;
                    emit_snapshot(&app, &state, &job_id).await;
                    break;
                }
            }
            Err(error) => {
                if restart_pending {
                    // Resume already reset the in-flight row to queued. Leave
                    // it there so the replacement worker can retry with the
                    // newly supplied credentials.
                    break;
                }
                let control_status = state.jobs().update_job_status_value(&job_id)?;
                if matches!(control_status.as_str(), "paused" | "canceling") {
                    state
                        .jobs()
                        .complete_update_job_item(item.id, "queued", None)?;
                    if control_status == "canceling" {
                        finish_update_job_cancellation(&app, &state, &job_id).await?;
                    }
                    break;
                }
                let kind = classify_failure(&error);
                if kind == FailureKind::Auth {
                    let message = format!("{}の認証を更新してください", item.title);
                    state
                        .jobs()
                        .complete_update_job_item(item.id, "queued", Some(&message))?;
                    if !transition_running_worker_status(
                        &app,
                        &state,
                        &job_id,
                        "auth_required",
                        &message,
                    )
                    .await?
                    {
                        break;
                    }
                    state
                        .jobs()
                        .append_update_job_log(&job_id, "warn", &message)?;
                    emit_snapshot(&app, &state, &job_id).await;
                    break;
                }

                if kind == FailureKind::Verification {
                    let message = format!("{}: {error}。未確認のまま一時停止しました", item.title);
                    state
                        .jobs()
                        .complete_update_job_item(item.id, "queued", Some(&message))?;
                    if transition_running_worker_status(&app, &state, &job_id, "paused", &message)
                        .await?
                    {
                        state
                            .jobs()
                            .append_update_job_log(&job_id, "warn", &message)?;
                        emit_snapshot(&app, &state, &job_id).await;
                    }
                    break;
                }

                // 取得制限は「相手が今は無理と言っている」だけなので、間隔を
                // 広げて同じ項目をやり直す。何度も続くようなら諦めて次へ行く。
                if matches!(kind, FailureKind::RateLimited | FailureKind::Network) {
                    if kind == FailureKind::RateLimited
                        && error.to_ascii_lowercase().contains("cloudflare")
                    {
                        let message = "FANBOXのアクセス確認が必要なため一時停止しました。連続再試行はしません。時間をおいて公式サイトと連携状態を確認してください";
                        state
                            .jobs()
                            .complete_update_job_item(item.id, "queued", Some(message))?;
                        if !transition_running_worker_status(
                            &app, &state, &job_id, "paused", message,
                        )
                        .await?
                        {
                            break;
                        }
                        state
                            .jobs()
                            .append_update_job_log(&job_id, "warn", message)?;
                        emit_snapshot(&app, &state, &job_id).await;
                        break;
                    }
                    let attempts = rate_limit_retries.entry(item.id).or_insert(0);
                    *attempts += 1;
                    if *attempts <= retry_limit {
                        let delay_ms = if kind == FailureKind::Network {
                            error_delay_ms(*attempts)
                        } else {
                            rate_limit_delay_ms(*attempts)
                        };
                        let message = format!(
                            "{}。{}秒あけて「{}」をやり直します",
                            if kind == FailureKind::Network {
                                "通信エラー"
                            } else {
                                "取得が制限されています"
                            },
                            delay_ms / 1000,
                            item.title
                        );
                        state
                            .jobs()
                            .complete_update_job_item(item.id, "queued", Some(&message))?;
                        if !transition_running_worker_status(
                            &app, &state, &job_id, "running", &message,
                        )
                        .await?
                        {
                            break;
                        }
                        state
                            .jobs()
                            .append_update_job_log(&job_id, "warn", &message)?;
                        emit_progress_delta(&app, &state, &job_id, Some(item.id)).await;
                        wait_for_retry_or_control(&state, &job_id, delay_ms).await?;
                        continue;
                    }
                    // 制限が長引いた場合も作品を「失敗消費」しない。項目を
                    // queued のまま一時停止し、利用者が時間を置いて再開すれば
                    // 同じ作品から続けられるようにする。
                    let message = format!(
                        "{}が続いているため一時停止しました。「{}」は再開時に再試行します",
                        if kind == FailureKind::Network {
                            "通信エラー"
                        } else {
                            "取得制限"
                        },
                        item.title
                    );
                    state
                        .jobs()
                        .complete_update_job_item(item.id, "queued", Some(&message))?;
                    if !transition_running_worker_status(&app, &state, &job_id, "paused", &message)
                        .await?
                    {
                        break;
                    }
                    state
                        .jobs()
                        .append_update_job_log(&job_id, "warn", &message)?;
                    emit_snapshot(&app, &state, &job_id).await;
                    break;
                }

                // 失敗の理由を項目とログの両方に残す。あとで「何を再試行すべきか」
                // を選べるようにするための材料になる。
                let reason = format!("[{}] {}", kind.label(), error);
                let held = item.item_type != "target";
                if held {
                    let known = is_known_hold(&state, &item, &reason)?;
                    hold_candidate(&state, &item, None, &reason).await?;
                    complete_held_item(
                        &state,
                        &job_id,
                        &item,
                        known,
                        &reason,
                        None,
                        &format!("{}: {}", item.title, reason),
                    )?;
                } else {
                    state
                        .jobs()
                        .complete_update_job_item(item.id, "failed", Some(&reason))?;
                    state.jobs().append_update_job_log(
                        &job_id,
                        "error",
                        &format!("{}: {}", item.title, reason),
                    )?;
                }
                // 消えた・閲覧できない投稿は取得元が答えを返している。続いても
                // 通信の不調ではないので、間隔を空けたり止めたりしない。
                if kind.counts_toward_pause() {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    next_delay_ms = error_delay_ms(consecutive_errors);
                }
                if item.item_type == "target" {
                    if let (Some(source), Some(source_key), Some(target_type)) = (
                        item.source.as_deref(),
                        item.source_id.as_deref(),
                        item.target_type.as_deref(),
                    ) {
                        let result = super::watching::active(&app).and_then(|library| {
                            serde_json::from_str::<crate::models::UpdateTarget>(&item.payload_json)
                                .map_err(|e| e.to_string())
                                .and_then(|target| {
                                    super::watching::record(&library, &target, None, None, 0, true)
                                })
                        });
                        if let Err(error) = result {
                            log::warn!(
                                "更新対象 {target_type}:{source}:{source_key} の失敗回数を記録できません: {error}"
                            );
                        }
                    }
                }
                if too_many_consecutive_errors(consecutive_errors, error_limit) {
                    let message = format!(
                        "取得エラーが{consecutive_errors}件続いたため一時停止しました。エラー内容を確認してから再開してください"
                    );
                    if !transition_running_worker_status(&app, &state, &job_id, "paused", &message)
                        .await?
                    {
                        break;
                    }
                    state
                        .jobs()
                        .append_update_job_log(&job_id, "warn", &message)?;
                    emit_snapshot(&app, &state, &job_id).await;
                    break;
                }
            }
        }

        if !local_only && !failed {
            consecutive_errors = 0;
        }

        // ログは1ジョブに何万行も溜まりうる。時々古い方から落とす。
        processed_since_trim += 1;
        if processed_since_trim >= LOG_TRIM_INTERVAL {
            processed_since_trim = 0;
            if let Err(error) = state
                .jobs()
                .trim_update_job_logs(&job_id, MAX_UPDATE_JOB_LOGS)
            {
                log::warn!("更新ジョブ {job_id} の古いログを整理できません: {error}");
            }
        }
        if restart_pending {
            break;
        }
        if !throttle_fast_progress || last_progress_emit.elapsed() >= FAST_PROGRESS_INTERVAL {
            emit_progress_delta(&app, &state, &job_id, Some(item.id)).await;
            last_progress_emit = Instant::now();
        }
        if state.jobs().update_job_status_value(&job_id)? == "canceling" {
            finish_update_job_cancellation(&app, &state, &job_id).await?;
            break;
        }
        // A cached author listing answered this item without touching the
        // provider. Its successor need not wait for a request that never ran.
        if local_only {
            continue;
        }
        // 待機中も一時停止や中止をすぐに受け付ける。
        wait_for_retry_or_control(&state, &job_id, next_delay_ms).await?;
    }

    Ok(())
}

/// 保存できた作品の、作者とシリーズの横顔を取りに行く。
///
/// 本文が入っても、作者の顔と紹介文は別の取得になる。ここを飛ばすと一覧には
/// 名前だけが並び、次に同じ作者を保存する日まで埋まらない。画面側の保存は
/// 最後にこれを回していた。ジョブに移すなら、これも一緒に連れて来ないと
/// **保存のたびに横顔が欠ける**。
///
/// 取得元は作品と同じ相手。`refresh_entity_profile` が自前で 3 秒の間隔を
/// 持っているので、ここでは待たない。1 件ごとの失敗は記録だけに留める -
/// 本文はもう手元にあり、横顔は次の機会にも埋められる。
async fn refresh_profiles_for_saved_works(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    credentials: &UpdateCredentials,
) {
    // 同じ作者の作品を 10 件保存しても、取りに行くのは 1 回でいい。
    let library = match state.shelf.library() {
        Ok(library) => library,
        Err(error) => {
            log::warn!("保存した作品のプロフィールを照合できません: {error}");
            return;
        }
    };
    let profile_state = state.clone();
    let profile_job = job_id.to_owned();
    let profiles = tauri::async_runtime::spawn_blocking(move || {
        let works = profile_state
            .jobs()
            .list_update_job_item_states(&profile_job)?
            .into_iter()
            .filter(|i| i.status == "saved")
            .filter_map(|i| i.work_id)
            .collect::<Vec<_>>();
        library
            .missing_profiles_for_works(&works)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    let targets: Vec<(&'static str, String, String)> = match profiles {
        Ok(profiles) => profiles
            .into_iter()
            .map(|(kind, provider, key)| {
                (
                    if kind == crate::library::EntityKind::Person {
                        "person"
                    } else {
                        "series"
                    },
                    provider,
                    key,
                )
            })
            .collect(),
        Err(error) => {
            log::warn!("保存した作品のプロフィールを照合できません: {error}");
            return;
        }
    };
    if targets.is_empty() {
        return;
    }
    let total = targets.len();
    let initial_label = format!("作者・シリーズ情報を確認しています（0/{total}件完了）");
    match state.jobs().transition_update_job_status(
        job_id,
        "running",
        "running",
        Some(&initial_label),
    ) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            log::warn!("更新ジョブ {job_id} のプロフィール進捗を記録できません: {error}");
            return;
        }
    }
    if let Err(error) = state.jobs().append_update_job_log(
        job_id,
        "info",
        &format!("作者・シリーズ情報を確認しています（{}件）", targets.len()),
    ) {
        log::warn!("更新ジョブ {job_id} のプロフィール確認ログを記録できません: {error}");
    }
    emit_progress_delta(app, state, job_id, None).await;
    for (index, (entity_type, source, source_key)) in targets.into_iter().enumerate() {
        let current_status = state
            .jobs()
            .update_job_status_value(job_id)
            .unwrap_or_default();
        if current_status != "running" {
            break;
        }
        let params = crate::commands::library::RefreshEntityProfileParams {
            entity_type: entity_type.to_string(),
            source: source.clone(),
            source_key: source_key.clone(),
            force: Some(false),
            refresh_token: pixiv_token(credentials),
            cookie: fanbox_cookie(credentials),
            user_agent: Some(fanbox_user_agent(credentials)),
        };
        if let Err(error) =
            crate::commands::library::refresh_entity_profile_with(app.clone(), params).await
        {
            log::warn!("{entity_type}:{source}:{source_key} の情報を取れません: {error}");
            let _ = state.jobs().append_update_job_log(
                job_id,
                "warn",
                &format!("{entity_type}:{source}:{source_key} の情報を取得できません: {error}"),
            );
        }
        let label = format!(
            "作者・シリーズ情報を確認しています（{}/{total}件完了）",
            index + 1
        );
        match state
            .jobs()
            .transition_update_job_status(job_id, "running", "running", Some(&label))
        {
            Ok(true) => {}
            Ok(false) => break,
            Err(error) => {
                log::warn!("更新ジョブ {job_id} のプロフィール進捗を記録できません: {error}");
                break;
            }
        }
        emit_progress_delta(app, state, job_id, None).await;
    }
}

/// 保存した作品の意味検索の索引を追いつかせる（全文索引は保存のたびに棚が追いつく）。
fn catch_up_semantic_index(state: &AppState) {
    if let Ok(library) = state.shelf.library() {
        super::library_health::schedule_semantic_sync(library);
    }
}

/// 保存した作品の指し先。棚の作品。
type SavedRef = crate::library::LibraryId;

/// 保存した作品を更新の監視に載せる。
fn watch_saved_work(app: &tauri::AppHandle, saved: SavedRef) {
    let work = saved;
    let result = (|| {
        let shelf = app
            .try_state::<Arc<super::shelf::Shelf>>()
            .ok_or("棚は開いていません")?;
        let library = shelf.library()?;
        shelf.forget_derived();
        let current = library
            .user_state(work, library.id)
            .map_err(|e| e.to_string())?;
        library
            .set_user_state(
                crate::library::LibraryId::now_v7(),
                work,
                library.id,
                current.generation,
                crate::library::UserStateChange {
                    favorite: None,
                    watching: Some(true),
                },
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    })();
    if let Err(error) = result {
        log::warn!("更新監視を有効にできません ({saved:?}): {error}");
    }
}

enum ItemOutcome {
    Done(String),
    Saved(SavedRef, String),
    PartialSaved(SavedRef, String),
    Skipped(String),
    LocalSkipped(String),
    AuthRequired(String),
    Held(String),
    Available(String),
}

/// 失敗の種類。同じ「エラー1件」でも、次にすべきことが違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureKind {
    /// 再接続が要る。ジョブを止めて利用者に知らせる。
    Auth,
    /// 取得が制限されている。間隔を空けて、この項目はやり直す。
    RateLimited,
    /// FANBOX の支援額・閲覧権限が現在の連携アカウントでは足りない。
    Restricted,
    /// 消された・非公開になった。やり直しても結果は変わらない。
    Missing,
    /// 通信が届かなかった。次の実行で拾えることが多い。
    Network,
    /// 一覧の完全性を確認できない。未処理のまま停止して誤判定を防ぐ。
    Verification,
    Other,
}

impl FailureKind {
    /// 連続失敗として数え、間隔を広げて一時停止に近づける失敗か。
    /// 消えた・閲覧制限は取得元の答えなので、何件続いても止める理由にならない。
    fn counts_toward_pause(self) -> bool {
        !matches!(self, FailureKind::Missing | FailureKind::Restricted)
    }

    /// 候補一覧とログに出す短い理由。
    fn label(self) -> &'static str {
        match self {
            FailureKind::Auth => "再接続が必要",
            FailureKind::RateLimited => "取得制限",
            FailureKind::Restricted => "閲覧制限",
            FailureKind::Missing => "見つからない",
            FailureKind::Network => "通信エラー",
            FailureKind::Verification => "確認未完了",
            FailureKind::Other => "エラー",
        }
    }
}

pub(super) fn classify_failure(error: &str) -> FailureKind {
    let normalized = error.to_ascii_lowercase();
    let has = |markers: &[&str]| markers.iter().any(|marker| normalized.contains(marker));

    // 画像や添付の取得で出た 401/403 を、連携切れとして扱わない。
    //
    // 作品本体の API は必ずアセットより先に叩いているので、本当に session が
    // 切れていればそちらが先に失敗する。ここまで来た 403 は i.pximg.net の
    // 直リンク制限のような配信側の都合で、ジョブ全体を止めて「再接続して
    // ください」と言う理由にはならない。取得制限・欠落・通信の判断は残す。
    let from_asset_fetch = normalized.contains("asset downloads failed");

    if has(&["一覧が", "改稿一覧を完全に確認できません"]) {
        return FailureKind::Verification;
    }

    if !from_asset_fetch
        && has(&[
            "認証が必要",
            "アクセストークンが不正",
            "セッションが無効",
            "再接続が必要",
            "http 401",
            "http 403",
            "status: 401",
            "status: 403",
            "unauthorized",
            "invalid_grant",
            "invalid token",
        ])
    {
        return FailureKind::Auth;
    }
    if has(&[
        "http 429",
        "status: 429",
        "too many requests",
        "rate limit",
        "http 503",
        "status: 503",
        // 取得元の生の JSON を文面に混ぜるのをやめたので、日本語の言い回しも
        // 拾えないと、間を空ければ通るものを取り違える。
        "レートリミット",
        "アクセス制限",
        "接続の更新に失敗した直後",
        "取得元がエラーを返しました。時間をおいて",
    ]) {
        return FailureKind::RateLimited;
    }
    if has(&[
        "現在のfanbox連携ではこの投稿を閲覧できません",
        "支援プランが足りない",
        "支援中のアカウント",
        "閲覧権限",
    ]) {
        return FailureKind::Restricted;
    }
    if has(&[
        "http 404",
        "status: 404",
        "http 410",
        "status: 410",
        "not found",
        "見つかりません",
        "削除",
        "非公開",
    ]) {
        return FailureKind::Missing;
    }
    if has(&[
        "timed out",
        "timeout",
        "connection",
        "dns",
        "network",
        "http 500",
        "http 502",
        "http 504",
        "status: 500",
        "status: 502",
        "status: 504",
        "取得元の応答を読み取れませんでした",
        "接続できません",
        "タイムアウト",
    ]) {
        return FailureKind::Network;
    }
    FailureKind::Other
}

/// レート制限の再試行間隔（attempt は 1 始まり）。
fn rate_limit_delay_ms(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(6);
    RATE_LIMIT_BACKOFF_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(MAX_RATE_LIMIT_BACKOFF_MS)
}

/// 429 以外の失敗でも次の作品へ急がない。連続失敗は停止で打ち切る。
fn error_delay_ms(consecutive_errors: u32) -> u64 {
    let shift = consecutive_errors.saturating_sub(1).min(6);
    ERROR_BACKOFF_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(MAX_ERROR_BACKOFF_MS)
}

fn too_many_consecutive_errors(consecutive_errors: u32, limit: u32) -> bool {
    consecutive_errors >= limit
}

/// Long rate-limit waits must not make pause/cancel feel ignored. Wake in short
/// intervals and return to the worker loop as soon as a control request lands.
async fn wait_for_retry_or_control(
    state: &Arc<AppState>,
    job_id: &str,
    delay_ms: u64,
) -> Result<(), String> {
    const CONTROL_POLL_MS: u64 = 500;
    let mut remaining_ms = delay_ms;
    while remaining_ms > 0 {
        let step_ms = remaining_ms.min(CONTROL_POLL_MS);
        tokio::time::sleep(std::time::Duration::from_millis(step_ms)).await;
        remaining_ms -= step_ms;
        if !matches!(
            state.jobs().update_job_status_value(job_id)?.as_str(),
            "queued" | "running"
        ) {
            break;
        }
    }
    Ok(())
}

async fn process_update_job_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    web_index: &mut WebUpdateIndex,
    fanbox_index: &mut FanboxUpdateIndex,
) -> Result<ItemOutcome, String> {
    let payload: Value = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    if payload.get("kind").and_then(Value::as_str) == Some("permission_check") {
        return recheck_deferred_item(state, item, credentials, payload).await;
    }
    if item.item_type == "candidate" {
        return process_candidate_item(state, item, credentials).await;
    }
    if item.item_type == "work" && payload.get(JOB_ACQUISITION_REQUEST).is_some() {
        let library = state.shelf.library()?;
        if let Some(outcome) = resume_saved_item(&library, item, &payload).await? {
            return Ok(outcome);
        }
    }
    if item.item_type != "target" {
        if let (Some(source), Some(id)) = (&item.source, &item.source_id) {
            if state
                .candidates()
                .candidate_acquisition_blocked(source, id, false)?
            {
                return Ok(ItemOutcome::Held(BLOCKED_CANDIDATE_MESSAGE.into()));
            }
        }
    }
    match item.item_type.as_str() {
        "work" => {
            process_work_item(
                app,
                state,
                job_id,
                item,
                credentials,
                web_index,
                fanbox_index,
            )
            .await
        }
        "target" => {
            process_target_item(
                app,
                state,
                job_id,
                item,
                credentials,
                web_index,
                fanbox_index,
            )
            .await
        }
        _ => Ok(ItemOutcome::Skipped(format!(
            "未対応のジョブ項目をスキップ: {}",
            item.item_type
        ))),
    }
}

/// A removed or private source post does not invalidate the copy already saved
/// in the library. Other fetch failures must keep their normal retry behavior.
fn unavailable_saved_fanbox_post(
    title: &str,
    error: crate::fanbox_api::error::FanboxError,
) -> Result<ItemOutcome, String> {
    match error {
        crate::fanbox_api::error::FanboxError::NotFound => Ok(ItemOutcome::Held(format!(
            "公開元で投稿が見つからないため確認を保留しました。保存済みの「{title}」は保持しています"
        ))),
        other => Err(other.to_string()),
    }
}

fn unavailable_saved_pixiv_work_message(title: &str, source_id: &str) -> String {
    format!(
        "公開元で投稿が見つからないため確認を保留しました。保存済みの「{title}」（作品ID {source_id}）は保持しています"
    )
}

fn unavailable_saved_pixiv_work(
    title: &str,
    source_id: &str,
    error: String,
) -> Result<ItemOutcome, String> {
    if classify_failure(&error) == FailureKind::Missing {
        Ok(ItemOutcome::Held(unavailable_saved_pixiv_work_message(
            title, source_id,
        )))
    } else {
        Err(error)
    }
}

async fn process_work_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    web_index: &mut WebUpdateIndex,
    fanbox_index: &mut FanboxUpdateIndex,
) -> Result<ItemOutcome, String> {
    let library = shelf::library(app)?;
    shelf::process_work(
        app,
        state,
        &library,
        job_id,
        item,
        credentials,
        (web_index, fanbox_index),
    )
    .await
}

/// 「増えた」「変わった」を候補として残す。
///
/// 候補はジョブとは別に残す。保存も拒否もされないまま次の確認を迎えても、
/// ここから戻ってくる（取得元の一覧は前回位置から先しか返さないため、
/// ジョブ限りにすると一度きりで消えてしまう）。
///
/// 新作も改稿もここを通る。**見つけ方が違うだけで、候補になったあとの扱いは同じ。**
fn record_candidate(
    state: &Arc<AppState>,
    job_id: &str,
    target: &crate::models::UpdateTarget,
    source_item: &Value,
    kind: CandidateKind,
    auto_save: bool,
) -> Result<bool, String> {
    let Some(mut payload) = candidate_payload(
        &target.target_type,
        &target.display_name,
        &target.source,
        source_item,
        kind,
    ) else {
        return Ok(false);
    };
    let source = payload
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or(&target.source)
        .to_string();
    let source_id = payload
        .get("sourceId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if source_id.is_empty() {
        return Ok(false);
    }
    if matches!(
        state
            .candidates()
            .update_candidate_status(&source, &source_id)?
            .as_deref(),
        Some("held" | "dismissed")
    ) {
        return Ok(false);
    }
    payload["originSourceKey"] = serde_json::json!(target.source_key);
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("無題")
        .to_string();
    state
        .candidates()
        .upsert_update_candidate(&UpdateCandidateInput {
            source: source.clone(),
            source_id: source_id.clone(),
            kind: kind.key().to_string(),
            title: title.clone(),
            payload_json: serde_json::to_string(&payload).map_err(|e| e.to_string())?,
            target_type: Some(target.target_type.clone()),
        })?;
    let restricted = source == "fanbox"
        && source_item.get("isRestricted").and_then(Value::as_bool) == Some(true);
    if restricted {
        payload["holdKind"] = serde_json::json!("restricted");
        payload["holdReason"] = serde_json::json!(
            super::downloader::fanbox_missing_material_error(source_item)
        );
        state
            .candidates()
            .upsert_update_candidate(&UpdateCandidateInput {
                source: source.clone(),
                source_id: source_id.clone(),
                kind: kind.key().into(),
                title: title.clone(),
                payload_json: payload.to_string(),
                target_type: Some(target.target_type.clone()),
            })?;
        state
            .candidates()
            .set_update_candidate_status(&source, &source_id, "held")?;
    }
    state.jobs().insert_update_job_candidate(
        job_id,
        &item_from_payload(
            "candidate",
            if restricted {
                "held"
            } else if auto_save {
                "queued"
            } else {
                "candidate"
            },
            Some(source),
            Some(source_id),
            Some(target.target_type.clone()),
            title,
            payload,
        )?,
    )
}

/// 監視している作者の、保存済み作品の改稿を拾う。
///
/// web の一覧は 1 リクエストで 100 件の `updateDate` を返すので、作者の全作品を
/// 並べても往復はアプリAPIの 1% で済む。ここが pixiv で改稿を知る唯一の道である
/// （作品詳細APIには更新時刻に当たるフィールドが無い）。
///
/// # 初めて見る作品
///
/// `source_updated_at` がまだ無い作品は、**保存より後に直されていなければ**
/// その値を基準として覚える。後に直されていれば、手元の本文が古い可能性がある
/// ということなので、覚えずに候補として出す。**分からないものを「最新」に
/// しない。** これを取り違えると、その改稿は二度と拾われない。
async fn scan_pixiv_revisions(
    state: &Arc<AppState>,
    job_id: &str,
    target: &crate::models::UpdateTarget,
    auto_save: bool,
    web_index: &mut WebUpdateIndex,
) -> Result<i64, String> {
    let library = web_index
        .library
        .clone()
        .ok_or_else(|| "棚は開いていません".to_string())?;
    shelf::scan_pixiv_revisions(state, &library, job_id, target, auto_save, web_index).await
}

async fn process_target_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    web_index: &mut WebUpdateIndex,
    fanbox_index: &mut FanboxUpdateIndex,
) -> Result<ItemOutcome, String> {
    let target: crate::models::UpdateTarget =
        serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    let auto_save = state.jobs().update_job_mode_value(job_id)? == "auto_save";
    let new_works_only = state.jobs().update_job_new_works_only(job_id)?;
    let library = shelf::library(app)?;
    let mut fanbox_checkpoint = None;
    let full_fanbox =
        !new_works_only && target.source == "fanbox" && target.target_type == "author";

    let items: Vec<Value> = if target.source == "pixiv" && target.target_type == "author" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "設定でpixivに接続してください".to_string(),
            ));
        };
        super::downloader::fetch_pixiv_user_novels_since(
            target.source_key.clone(),
            token,
            target.last_seen_source_id.as_deref(),
        )
        .await?
        .into_complete()?
        .into_iter()
        .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?
    } else if target.source == "pixiv" && target.target_type == "series" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "設定でpixivに接続してください".to_string(),
            ));
        };
        super::downloader::fetch_pixiv_series_novels_since(
            target.source_key.clone(),
            token,
            target.last_seen_source_id.as_deref(),
        )
        .await?
        .into_complete()?
        .into_iter()
        .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?
    } else if target.source == "fanbox" && target.target_type == "author" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "設定でFANBOXに接続してください".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        if full_fanbox {
            fanbox_index
                .lookup(&target.source_key, "", &cookie, &user_agent)
                .await?;
            if !fanbox_index.posts.contains_key(&target.source_key) {
                return Err("FANBOX の改稿一覧を完全に確認できません".into());
            }
        }
        let posts = match fanbox_index
            .posts_since(&target.source_key, target.last_seen_source_id.as_deref())
        {
            Some(posts) => posts,
            None => super::downloader::fetch_fanbox_creator_posts_since(
                target.source_key.clone(),
                cookie,
                user_agent,
                target.last_seen_source_id.as_deref(),
            )
            .await?
            .into_complete()?,
        };
        if full_fanbox {
            fanbox_checkpoint = posts
                .first()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| e.to_string())?;
        }
        let posts = if full_fanbox {
            shelf::fanbox_scan_posts(&library, &fanbox_index.posts[&target.source_key], &posts)?
        } else {
            posts
        };
        posts
            .into_iter()
            .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    let default_kind = if target.target_type == "series" {
        CandidateKind::Sequel
    } else {
        CandidateKind::New
    };

    let mut found = 0i64;
    for source_item in &items {
        // まず取得元での ID を取り出し、手元にあるかどうかで種類を決める。
        let listed_id = if target.source == "pixiv" {
            normalize_pixiv_novel_id(source_item)
        } else {
            string_at(source_item, &[&["id"]])
        };
        let Some(listed_id) = listed_id.filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some((kind, source_item)) = shelf::listed_candidate(
            &library,
            &target.source,
            &listed_id,
            source_item,
            default_kind,
            new_works_only,
        )?
        else {
            continue;
        };
        let source_item = &source_item;

        // 一度「今後は出さない」と決めた作品は、見つけても候補にしない。
        if state
            .candidates()
            .update_candidate_status(&target.source, &listed_id)?
            .as_deref()
            == Some("dismissed")
        {
            continue;
        }

        let recorded = {
            let _library_write_guard = state.library_gate.clone().write_owned().await;
            record_candidate(state, job_id, &target, source_item, kind, auto_save)?
        };
        if recorded {
            found += 1;
            // A long listing can yield hundreds of candidates. Publish small
            // batches from the local DB without another provider request.
            if found == 1 || found % 10 == 0 {
                emit_progress_delta(app, state, job_id, None).await;
            }
        }
    }

    // 作者を監視しているなら、その作者の作品の改稿も同じ確認で分かるべき。
    // FANBOX は取得元の一覧が更新時刻を持つのでずっとそうなっていた。pixiv
    // だけができなかったのは、アプリAPIの一覧に更新時刻が無いからで、
    // web の一覧を通せばこの非対称は消える。
    if target.source == "pixiv" && target.target_type == "author" && !new_works_only {
        found += scan_pixiv_revisions(state, job_id, &target, auto_save, web_index).await?;
    }

    let first = if full_fanbox {
        fanbox_checkpoint.as_ref()
    } else {
        items.first()
    };
    let first_id = first.and_then(|value| {
        if target.source == "pixiv" {
            normalize_pixiv_novel_id(value)
        } else {
            string_at(value, &[&["id"]])
        }
    });
    let first_updated = first.and_then(|value| {
        if target.source == "pixiv" {
            string_at(value, &[&["create_date"], &["createDate"]])
        } else {
            string_at(value, &[&["updatedDatetime"], &["updated_datetime"]])
        }
    });
    {
        let _library_write_guard = state.library_gate.write().await;
        super::watching::record(
            &library,
            &target,
            first_id.as_deref(),
            first_updated.as_deref(),
            found,
            false,
        )?;
    }

    // 新作一覧とプロフィールは別の取得口を持つ。作者の作品を確認しただけでは、
    // 名前・アイコン・紹介文は更新されない。保存した直後の未完成プロフィールだけを
    // ジョブの仕上げで補っていたため、すでに保存済みの監視作者は、取得元で
    // アイコンを変えても手元が永久に古いままだった。
    //
    // 一覧の確認が成功した作者はプロフィールも確認する。24時間以内に確認済みなら
    // `refresh_entity_profile` 自身が通信せず戻るので、定期確認のたびに同じ API を
    // 叩くことはない。横顔だけの失敗で新作確認まで失敗扱いにはせず、ログに残して
    // 次回また試せるようにする。
    let profile_params = (!new_works_only)
        .then(|| profile_refresh_params_for_target(&target, credentials))
        .flatten();
    if let Some(params) = profile_params {
        if let Err(error) =
            crate::commands::library::refresh_entity_profile_with(app.clone(), params).await
        {
            log::warn!(
                "監視作者 {}:{} のプロフィールを更新できません: {error}",
                target.source,
                target.source_key
            );
            let _ = state.jobs().append_update_job_log(
                job_id,
                "warn",
                &format!(
                    "{}: プロフィールを更新できません: {error}",
                    target.display_name
                ),
            );
        }
    }

    Ok(ItemOutcome::Done(if found > 0 {
        format!("{}: 候補 {} 件", target.display_name, found)
    } else {
        format!("{}: 新しいものはありません", target.display_name)
    }))
}

/// 棚への確定と実行項目を閉じる処理の間で止まった場合、要求に対応する保存結果を使う。
async fn resume_saved_item(
    library: &Arc<Library>,
    item: &UpdateJobItem,
    payload: &Value,
) -> Result<Option<ItemOutcome>, String> {
    let Some(request) = payload.get(JOB_ACQUISITION_REQUEST) else {
        return Ok(None);
    };
    let request: LibraryId = serde_json::from_value(request.clone()).map_err(|e| e.to_string())?;
    let fail = |error: crate::library::Error| error.to_string();
    let Some(info) = library.find_acquisition(request).map_err(fail)? else {
        return Ok(None);
    };
    if info.ticket.kind != EntityKind::Work
        || !matches!(info.ticket.route.as_str(), "save" | "prepare-reader")
        || Some(info.ticket.provider.as_str()) != item.source.as_deref()
        || Some(info.ticket.external_key.as_str()) != item.source_id.as_deref()
    {
        return Err("実行項目と取得要求の作品が一致しません".into());
    }
    if info.state != AcquisitionState::Complete
        || payload[JOB_ACQUISITION_RETRY].as_bool() == Some(true)
    {
        return Ok(None);
    }
    let result = info.result.ok_or("完了した取得の保存結果がありません")?;
    let (title, text_length, version) = library
        .work_snapshot_summary(result.entity_id, result.snapshot_id)
        .map_err(fail)?;
    let previous = info
        .ticket
        .base_snapshot
        .map(|base| library.work_snapshot_summary(result.entity_id, base))
        .transpose()
        .map_err(fail)?;
    let previous = previous
        .as_ref()
        .map(|(title, length, _)| (title.as_str(), *length));
    let mut masked = Vec::new();
    let mut missing = Vec::new();
    if info.ticket.provider == "pixiv" {
        missing = library
            .pixiv_source_image_gaps(result.entity_id, result.snapshot_id)
            .map_err(fail)?;
        missing = super::downloader::shelf::references_left_by_edit(
            Arc::clone(library),
            result.entity_id,
            result.snapshot_id,
            missing,
        )
        .await;
        if item.item_type == "candidate" {
            let input = info
                .payloads
                .iter()
                .find(|payload| payload.slot == "work-save-input")
                .ok_or("完了した取得の原応答がありません")?;
            let file = library.file(input.file).map_err(fail)?;
            let mut raw = Vec::new();
            library
                .objects
                .read_range(&file, 0, file.length, &mut raw)
                .map_err(|e| e.to_string())?;
            masked = crate::analysis::masked_pixiv_metadata(
                &serde_json::from_slice(&raw).map_err(|e| e.to_string())?,
            );
        }
    }
    let saved = super::downloader::ShelfSaved {
        work: result.entity_id,
        title,
        text_length,
        version,
        revised: result.created_revision && previous.is_some(),
        created: previous.is_none(),
        missing_references: missing,
    };
    Ok(Some(if item.item_type == "work" {
        shelf::describe_work_save(
            &saved,
            previous.unwrap_or((&saved.title, saved.text_length)),
        )
    } else {
        describe_candidate_save(&saved, previous, &masked)
    }))
}

async fn process_candidate_item(
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
) -> Result<ItemOutcome, String> {
    let payload: Value = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    let source = item.source.as_deref().unwrap_or("");
    let source_id = item.source_id.as_deref().unwrap_or("");
    let kind = payload.get("kind").and_then(|v| v.as_str());
    let library = state.shelf.library()?;
    if let Some(outcome) = resume_saved_item(&library, item, &payload).await? {
        return Ok(outcome);
    }
    if state
        .candidates()
        .candidate_acquisition_blocked(source, source_id, kind == Some("save"))?
    {
        return Ok(ItemOutcome::Held(BLOCKED_CANDIDATE_MESSAGE.into()));
    }
    // 手元にあるかどうか・監視しているかを棚で見る。
    let card = shelf::saved_card(&library, source, source_id)?;
    // 改稿の候補は「すでに手元にある作品」が対象なので、保存済みでも取りに行く。
    // まとめ保存も、監視に載っている作品は取り直す。
    let refetch = kind == Some("revision")
        || (kind == Some("save")
            && (card.as_ref().is_some_and(|card| card.watching)
                || state
                    .candidates()
                    .has_incomplete_save_candidate(source, source_id)?));
    if card.is_some() && !refetch {
        return Ok(ItemOutcome::Skipped(format!(
            "保存済みのためスキップ: {}",
            item.title
        )));
    }
    let previous = card.map(|card| (card.title, card.text_length as i64));
    save_candidate(state, item, credentials, &payload, previous).await
}

/// まとめ保存では、今の資格情報で観測した制限だけを再利用する。
fn observed_fanbox_restriction<'a>(
    payload: &'a Value,
    credentials: &UpdateCredentials,
) -> Option<&'a Value> {
    let post = payload.get("originalData")?;
    if post.get("isRestricted").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    if payload.get("kind").and_then(Value::as_str) == Some("save") {
        let observed_key = post.get("accountKey").and_then(Value::as_str)?;
        if credentials.fanbox_account_key.as_deref() != Some(observed_key) {
            return None;
        }
    }
    Some(post)
}

/// 候補を取りに行って保存する。`previous` は手元にあった版の題と字数。
async fn save_candidate(
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    payload: &Value,
    previous: Option<(String, i64)>,
) -> Result<ItemOutcome, String> {
    let source = item.source.as_deref().unwrap_or("");
    let source_id = item.source_id.as_deref().unwrap_or("");
    let previous = previous
        .as_ref()
        .map(|(title, length)| (title.as_str(), *length));

    if source == "pixiv" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "設定でpixivに接続してください".to_string(),
            ));
        };
        let data = super::downloader::fetch_pixiv_novel(source_id.to_string(), token).await?;
        let value = serde_json::to_value(&data).map_err(|e| e.to_string())?;
        let masked = crate::analysis::masked_pixiv_metadata(&value);
        let updated = super::downloader::download_and_save(
            state,
            item,
            value,
            "pixiv".to_string(),
            source_id.to_string(),
            None,
            None,
        )
        .await?;
        // 保存したついでにシリーズを監視へ入れることはしない。監視対象は
        // 利用者が自分で選ぶもので、勝手に増えると取得先への負荷にもなる。
        Ok(describe_candidate_save(&updated, previous, &masked))
    } else if source == "fanbox" {
        if let Some(post) = observed_fanbox_restriction(payload, credentials) {
            let reason = super::downloader::fanbox_missing_material_error(post);
            hold_candidate(state, item, Some(post), &reason).await?;
            return Ok(ItemOutcome::Held(reason));
        }
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "設定でFANBOXに接続してください".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        let post = super::downloader::fanbox_post(
            source_id.to_string(),
            cookie.clone(),
            user_agent.clone(),
        )
        .await?;
        let value = serde_json::to_value(&post).map_err(|e| e.to_string())?;
        if value.get("isRestricted").and_then(Value::as_bool) == Some(true) {
            let reason = super::downloader::fanbox_missing_material_error(&value);
            hold_candidate(state, item, Some(&value), &reason).await?;
            return Ok(ItemOutcome::Held(reason));
        }
        let updated = super::downloader::download_and_save(
            state,
            item,
            value,
            "fanbox".to_string(),
            source_id.to_string(),
            Some(cookie),
            Some(user_agent),
        )
        .await?;
        Ok(describe_candidate_save(&updated, previous, &[]))
    } else {
        Ok(ItemOutcome::Skipped(format!(
            "未対応ソースをスキップ: {}",
            source
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_initial_items, build_save_items, canceled_status_for, classify_failure,
        describe_source_change, describe_text_change, error_delay_ms, make_job_id,
        profile_refresh_params_for_target, rate_limit_delay_ms, too_many_consecutive_errors,
        FailureKind, FanboxUpdateIndex, SaveJobWork, MAX_ERROR_BACKOFF_MS,
        MAX_RATE_LIMIT_BACKOFF_MS,
    };
    use crate::fanbox_api::error::FanboxError;
    use crate::models::{AdhocUpdateTarget, StartUpdateJobRequest};
    use std::collections::HashSet;

    #[test]
    fn a_missing_saved_fanbox_post_is_held_without_hiding_other_failures() {
        match super::unavailable_saved_fanbox_post("手元の作品", FanboxError::NotFound) {
            Ok(super::ItemOutcome::Held(message)) => {
                assert!(message.contains("保存済み"));
                assert!(message.contains("保持"));
            }
            _ => panic!("a missing saved post should remain available locally"),
        }
        assert!(matches!(
            super::unavailable_saved_fanbox_post("作品", FanboxError::RateLimited),
            Err(message) if message.contains("アクセス制限")
        ));
    }

    #[test]
    fn a_missing_saved_pixiv_work_is_held_but_other_failures_remain_errors() {
        assert!(matches!(
            super::unavailable_saved_pixiv_work(
                "保存済み",
                "90000008",
                "取得元に見つかりませんでした".into(),
            ),
            Ok(super::ItemOutcome::Held(message))
                if message.contains("90000008") && message.contains("保持しています")
        ));
        assert!(matches!(
            super::unavailable_saved_pixiv_work(
                "保存済み",
                "90000008",
                "接続がタイムアウトしました".into(),
            ),
            Err(message) if message.contains("タイムアウト")
        ));
    }

    #[test]
    fn only_a_new_or_changed_hold_is_reported_again() {
        let (_temp, state, _library) = super::shelf::tests::fixture();
        let item = crate::models::UpdateJobItem {
            id: 1,
            item_type: "work".into(),
            source: Some("pixiv".into()),
            source_id: Some("42".into()),
            target_type: None,
            title: "作品".into(),
            payload_json: "{}".into(),
        };
        let gone = super::unavailable_saved_pixiv_work_message("作品", "42");
        let partial = "[一部保存] 本文は保存しましたが、参照先の挿絵を閲覧できません";
        // 初めての保留は知らせる。
        assert!(!super::is_known_hold(&state, &item, &gone).unwrap());
        state
            .candidates()
            .upsert_update_candidate(&crate::models::UpdateCandidateInput {
                source: "pixiv".into(),
                source_id: "42".into(),
                kind: "revision".into(),
                title: "作品".into(),
                payload_json: serde_json::json!({"holdKind": "unavailable"}).to_string(),
                target_type: Some("work".into()),
            })
            .unwrap();
        // 候補があっても、保留でなければ前回の保留ではない。
        assert!(!super::is_known_hold(&state, &item, &gone).unwrap());
        state
            .candidates()
            .set_update_candidate_status("pixiv", "42", "held")
            .unwrap();
        assert!(super::is_known_hold(&state, &item, &gone).unwrap());
        assert!(super::is_known_hold(&state, &item, super::BLOCKED_CANDIDATE_MESSAGE).unwrap());
        // 保留の種類が変わったら、それは新しい知らせ。
        assert!(!super::is_known_hold(&state, &item, partial).unwrap());
    }

    #[test]
    fn new_works_only_keeps_pending_revisions_out_of_both_check_and_save() {
        let (_temp, state, library) = super::shelf::tests::fixture();
        library
            .edit_watch(
                crate::library::LibraryId::now_v7(),
                library.watch_generation().unwrap(),
                crate::library::WatchEdit::Upsert {
                    target_type: "author".into(),
                    provider: "pixiv".into(),
                    key: "123".into(),
                    name: "作者".into(),
                    enabled: true,
                    options: serde_json::json!({}),
                },
            )
            .unwrap();
        super::shelf::tests::save(
            &library,
            "pixiv",
            "old",
            &super::shelf::tests::data("pixiv", "本文", "123"),
            false,
        );
        for (source_id, kind) in [("old", "revision"), ("new", "new")] {
            state.candidates().upsert_update_candidate(&crate::models::UpdateCandidateInput {
                source: "pixiv".into(), source_id: source_id.into(),
                kind: kind.into(), title: source_id.into(),
                payload_json: serde_json::json!({
                    "kind": kind, "originSourceKey": "123", "originalData": {"user": {"id": "123"}}
                }).to_string(), target_type: Some("author".into()),
            }).unwrap();
        }
        for mode in ["check_only", "auto_save"] {
            let request = StartUpdateJobRequest {
                scope: "all".into(),
                mode: mode.into(),
                work_ids: None,
                target_ids: None,
                watch_saved: None,
                new_works_only: Some(true),
                adhoc_targets: None,
            };
            let items = build_initial_items(&state, &request, &library).unwrap();
            assert!(!items
                .iter()
                .any(|item| item.source_id.as_deref() == Some("old")));
            let new = items
                .iter()
                .find(|item| item.source_id.as_deref() == Some("new"))
                .unwrap();
            assert_eq!(
                new.status,
                if mode == "auto_save" {
                    "queued"
                } else {
                    "candidate"
                }
            );
            let full = build_initial_items(
                &state,
                &StartUpdateJobRequest {
                    new_works_only: Some(false),
                    ..request
                },
                &library,
            )
            .unwrap();
            assert!(full
                .iter()
                .any(|item| item.source_id.as_deref() == Some("old")));
        }
    }

    #[test]
    fn a_target_check_recovers_pending_candidates_beyond_the_global_first_page() {
        let (_temp, state, library) = super::shelf::tests::fixture();
        let candidate = |source_id: String, author: &str| crate::models::UpdateCandidateInput {
            source: "pixiv".into(),
            source_id,
            kind: "new".into(),
            title: "Candidate".into(),
            payload_json: serde_json::json!({"originSourceKey": author}).to_string(),
            target_type: Some("author".into()),
        };
        state
            .candidates()
            .upsert_update_candidate(&candidate("a-requested".into(), "requested"))
            .unwrap();
        // More recent, unrelated candidates used to consume the entire global
        // 2,000-row allowance before the requested author's scope was applied.
        for index in 0..2_001 {
            state
                .candidates()
                .upsert_update_candidate(&candidate(format!("noise-{index:04}"), "another-author"))
                .unwrap();
        }
        state
            .candidates()
            .upsert_update_candidate(&candidate("z-requested".into(), "requested"))
            .unwrap();
        let request = StartUpdateJobRequest {
            scope: "author".into(),
            mode: "check_only".into(),
            work_ids: None,
            target_ids: None,
            watch_saved: None,
            new_works_only: Some(true),
            adhoc_targets: Some(vec![AdhocUpdateTarget {
                target_type: "author".into(),
                source: "pixiv".into(),
                source_key: "requested".into(),
                display_name: "Requested".into(),
            }]),
        };
        let items = build_initial_items(&state, &request, &library).unwrap();
        let keys = items
            .iter()
            .filter(|item| item.item_type == "candidate")
            .filter_map(|item| item.source_id.as_deref())
            .collect::<HashSet<_>>();
        assert_eq!(keys, HashSet::from(["a-requested", "z-requested"]));
    }

    fn update_target(
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> crate::models::UpdateTarget {
        crate::models::UpdateTarget {
            id: None,
            watch_generation: None,
            target_type: target_type.into(),
            source: source.into(),
            source_key: source_key.into(),
            display_name: "作者".into(),
            enabled: true,
            last_checked_at: None,
            last_seen_source_id: None,
            last_seen_source_updated_at: None,
            metadata_json: None,
            created_at: "now".into(),
            updated_at: "now".into(),
            last_hit_at: None,
            consecutive_errors: 0,
            watch_groups: Vec::new(),
            icon_path: None,
        }
    }

    #[test]
    fn a_watched_author_refreshes_the_separate_person_profile() {
        let target = update_target("author", "pixiv", "800000001");
        let credentials = super::snapshot_credentials_missing();
        let params = profile_refresh_params_for_target(&target, &credentials)
            .expect("an author has a person profile");

        assert_eq!(params.entity_type, "person");
        assert_eq!(params.source, "pixiv");
        assert_eq!(params.source_key, "800000001");
        assert_eq!(params.force, Some(false));
    }

    #[test]
    fn a_series_check_does_not_invent_a_person_profile() {
        let target = update_target("series", "pixiv", "42");
        let credentials = super::snapshot_credentials_missing();

        assert!(profile_refresh_params_for_target(&target, &credentials).is_none());
    }

    #[test]
    fn durable_candidates_obey_exact_current_watch_scope() {
        let mut candidate = crate::models::UpdateCandidateRow {
            work_id: None,
            metadata: Default::default(),
            source: "fanbox".into(), source_id: "11".into(), kind: "new".into(), title: "作品".into(),
            payload_json: serde_json::json!({"originSourceKey":"creator", "originalData":{"creatorId":"creator"}}).to_string(),
            target_type: Some("author".into()), status: "pending".into(), first_seen_at: "now".into(), updated_at: "now".into(),
        };
        let works = HashSet::from([("fanbox".into(), "11".into())]);
        assert!(!super::candidate_matches_scope(
            &candidate,
            &works,
            &HashSet::new()
        ));
        assert!(!super::candidate_matches_scope(
            &candidate,
            &HashSet::new(),
            &HashSet::from([("fanbox".into(), "author".into(), "other".into())])
        ));
        assert!(super::candidate_matches_scope(
            &candidate,
            &HashSet::new(),
            &HashSet::from([("fanbox".into(), "author".into(), "creator".into())])
        ));
        candidate.kind = "revision".into();
        assert!(super::candidate_matches_scope(
            &candidate,
            &works,
            &HashSet::new()
        ));
        candidate.kind = "new".into();
        candidate.payload_json = "{}".into();
        candidate.target_type = None;
        assert!(!super::candidate_matches_scope(
            &candidate,
            &works,
            &HashSet::new()
        ));
    }

    #[test]
    fn restricted_listing_is_carried_without_a_post_body() {
        let items = build_save_items(&[SaveJobWork {
            source: "fanbox".into(),
            source_id: "11".into(),
            title: "作品".into(),
            fanbox_access: Some(super::FanboxAccess {
                is_restricted: true,
                account_key: "取得時".into(),
                fee_required: Some(1000),
                creator_id: Some("creator".into()),
            }),
        }]);
        let payload: serde_json::Value = serde_json::from_str(&items[0].payload_json).unwrap();
        assert_eq!(payload["originalData"]["feeRequired"], 1000);
        assert_eq!(payload["originalData"]["isRestricted"], true);
        assert!(payload["originalData"].get("body").is_none());
        let mut credentials = super::snapshot_credentials_missing();
        assert!(super::observed_fanbox_restriction(&payload, &credentials).is_none());
        credentials.fanbox_account_key = Some("取得時".into());
        assert!(super::observed_fanbox_restriction(&payload, &credentials).is_some());
        // ジョブを再開したときは、その時点の資格情報で同じ候補を判定する。
        credentials.fanbox_account_key = Some("切り替え後".into());
        assert!(super::observed_fanbox_restriction(&payload, &credentials).is_none());
    }

    /// 改稿の候補で公開日を並べても仕方がない。置き換わるもの - いま手元に
    /// ある版 - を言う。取得元が直した日は持っていないので、そこは語らない。
    #[test]
    fn a_revision_says_what_is_about_to_be_replaced() {
        let item = serde_json::json!({
            "id": "90000007",
            "title": "作品",
            "user": { "name": "作者" },
            "create_date": "2026-07-26T00:00:01+09:00",
            "localVersion": 1,
            "localSavedAt": "2026-08-19T14:19:33.782735500+00:00",
        });
        let payload = super::candidate_payload(
            "author",
            "作者",
            "pixiv",
            &item,
            super::CandidateKind::Revision,
        )
        .unwrap();
        let subtitle = payload.get("subtitle").and_then(|v| v.as_str()).unwrap();
        assert_eq!(subtitle, "手元は v1（2026-08-19に保存）");
    }

    /// 本文が同じで題名だけ直された更新がある。保存する前に、そうと分かるようにする。
    #[test]
    fn a_revision_says_when_the_title_changed() {
        let item = serde_json::json!({
            "id": "90000007",
            "title": "直した題名",
            "user": { "name": "作者" },
            "localTitle": "元の題名",
            "localVersion": 1,
            "localSavedAt": "2026-08-19T14:19:33.782735500+00:00",
        });
        let payload = super::candidate_payload(
            "author",
            "作者",
            "pixiv",
            &item,
            super::CandidateKind::Revision,
        )
        .unwrap();
        assert_eq!(
            payload.get("subtitle").and_then(|v| v.as_str()).unwrap(),
            "手元は v1（2026-08-19に保存）・題名が変わっています"
        );
        assert_eq!(
            describe_source_change("元の題名", "直した題名", 980, 980),
            "題名を変更, 文字数は変わらず"
        );
        assert_eq!(describe_source_change("題名", "題名", 980, 990), "+10字");
    }

    /// 保存済み作品の確認で読んだ一覧を、新作の確認でも使う。前回見た投稿より
    /// 古いものは、取得元へ聞いたときと同じく候補に混ぜない。
    #[test]
    fn a_listing_read_for_saved_works_also_answers_the_new_work_check() {
        let post = |id: &str| {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "title": id,
                "creatorId": "creator",
                "publishedDatetime": "2026-11-02T00:00:00+09:00",
                "updatedDatetime": "2026-11-02T00:00:00+09:00",
            }))
            .unwrap()
        };
        let mut index = FanboxUpdateIndex::default();
        assert!(index.posts_since("creator", None).is_none());
        index
            .posts
            .insert("creator".to_string(), vec![post("3"), post("2"), post("1")]);

        let ids = |stop| -> Vec<String> {
            index
                .posts_since("creator", stop)
                .unwrap()
                .into_iter()
                .map(|post| post.id)
                .collect()
        };
        assert_eq!(ids(None), ["3", "2", "1"]);
        assert_eq!(ids(Some("2")), ["3"]);
        assert_eq!(ids(Some("3")), Vec::<String>::new());
        assert!(index.posts_since("someone-else", None).is_none());
    }

    /// 手元の版が分からなければ、作り話をせず従来どおりの副題に戻す。
    #[test]
    fn a_revision_without_local_facts_falls_back_rather_than_inventing() {
        let item = serde_json::json!({
            "id": "1", "title": "作品", "user": { "name": "作者" },
            "create_date": "2026-07-26T00:00:01+09:00",
        });
        let payload = super::candidate_payload(
            "author",
            "作者",
            "pixiv",
            &item,
            super::CandidateKind::Revision,
        )
        .unwrap();
        assert_eq!(
            payload.get("subtitle").and_then(|v| v.as_str()),
            Some("2026-07-26T00:00:01+09:00")
        );
    }

    /// 新作の副題は変えていない。
    #[test]
    fn a_new_work_keeps_the_subtitle_it_always_had() {
        let item = serde_json::json!({
            "id": "1", "title": "作品", "user": { "name": "別の人" },
            "create_date": "2026-07-26T00:00:01+09:00",
        });
        let payload =
            super::candidate_payload("author", "作者", "pixiv", &item, super::CandidateKind::New)
                .unwrap();
        assert_eq!(
            payload.get("subtitle").and_then(|v| v.as_str()),
            Some("別の人 ・ 2026-07-26T00:00:01+09:00")
        );
    }

    #[test]
    fn job_ids_are_unique_library_uuids() {
        let ids = (0..10_000).map(|_| make_job_id()).collect::<HashSet<_>>();
        assert_eq!(ids.len(), 10_000);
        assert!(ids
            .iter()
            .all(|id| { id.parse::<crate::library::LibraryId>().is_ok() }));
    }

    /// 預かるのは、取得元とIDが揃っているものだけ。
    ///
    /// 同じ作品が二度並ぶと、二度取りに行って二度目は「保存済み」で飛ぶ。
    /// 無駄なだけでなく、件数の分母が実際より大きく出る。
    #[test]
    fn a_save_job_takes_each_work_once() {
        let works = vec![
            SaveJobWork {
                source: "pixiv".into(),
                source_id: "1".into(),
                title: "一つ目".into(),
                fanbox_access: None,
            },
            // 同じ相手先。二度は預からない。
            SaveJobWork {
                source: "pixiv".into(),
                source_id: "1".into(),
                title: "一つ目（重複）".into(),
                fanbox_access: None,
            },
            // 取得元が違えば別の作品。
            SaveJobWork {
                source: "fanbox".into(),
                source_id: "1".into(),
                title: "別の取得元".into(),
                fanbox_access: None,
            },
            // 知らない取得元は預からない。
            SaveJobWork {
                source: "unknown".into(),
                source_id: "9".into(),
                title: "知らない相手".into(),
                fanbox_access: None,
            },
            // IDが無いものも預からない。
            SaveJobWork {
                source: "pixiv".into(),
                source_id: "   ".into(),
                title: "IDが無い".into(),
                fanbox_access: None,
            },
        ];

        let items = build_save_items(&works);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source.as_deref(), Some("pixiv"));
        assert_eq!(items[0].source_id.as_deref(), Some("1"));
        assert_eq!(items[1].source.as_deref(), Some("fanbox"));
        // 種類は候補と同じ。取得元IDから取ってくる処理をそのまま使う。
        assert!(items.iter().all(|item| item.item_type == "candidate"));
        // 監視対象を経由しない、作品そのものの保存。
        assert!(items
            .iter()
            .all(|item| item.target_type.as_deref() == Some("work")));
        // `kind` で保存済みの扱いを分ける。
        assert!(items
            .iter()
            .all(|item| item.payload_json.contains("\"save\"")));
    }

    /// 題名が空でも預かる。一覧に「（無題）」ではなくIDが出る。
    #[test]
    fn a_work_without_a_title_still_gets_one() {
        let items = build_save_items(&[SaveJobWork {
            source: "pixiv".into(),
            source_id: "12345".into(),
            title: "  ".into(),
            fanbox_access: None,
        }]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "12345");
    }

    #[test]
    fn canceling_a_job_without_a_worker_finishes_immediately() {
        for status in ["paused", "auth_required", "failed"] {
            assert_eq!(canceled_status_for(status).unwrap(), Some("canceled"));
        }
    }

    #[test]
    fn canceling_an_active_job_waits_for_its_safe_boundary() {
        for status in ["queued", "running", "canceling"] {
            assert_eq!(canceled_status_for(status).unwrap(), Some("canceling"));
        }
        assert_eq!(canceled_status_for("completed").unwrap(), None);
        assert_eq!(canceled_status_for("canceled").unwrap(), None);
        assert!(canceled_status_for("unknown").is_err());
    }

    #[test]
    fn authentication_failures_pause_instead_of_consuming_the_item() {
        for error in [
            "FANBOX APIエラー（HTTP 401）",
            "認証が必要ですが、認証情報が提供されていません",
            "oauth invalid_grant",
            "request failed: Unauthorized",
            "pixivのWeb連携が切れています。再接続が必要です",
        ] {
            assert_eq!(classify_failure(error), FailureKind::Auth, "{error}");
        }
    }

    /// 失敗の分類は「次に何をするか」を決める。取り違えると、やり直せば通る
    /// ものを捨てたり、二度と通らないものを延々と叩いたりする。
    #[test]
    fn each_kind_of_failure_is_told_apart() {
        for (error, expected) in [
            ("pixiv APIエラー（HTTP 429）", FailureKind::RateLimited),
            ("Too Many Requests", FailureKind::RateLimited),
            ("FANBOX APIエラー（HTTP 503）", FailureKind::RateLimited),
            ("接続の更新に失敗した直後です。しばらく待ってからやり直してください", FailureKind::RateLimited),
            ("取得元がエラーを返しました。時間をおいてからやり直してください", FailureKind::RateLimited),
            (
                "現在のFANBOX連携ではこの投稿を閲覧できません（必要な支援額: 月額500円以上）",
                FailureKind::Restricted,
            ),
            (
                "この投稿の本文を取得できませんでした。支援プランが足りないか、非公開の可能性があります",
                FailureKind::Restricted,
            ),
            ("HTTP 404 Not Found", FailureKind::Missing),
            ("この作品は削除されました", FailureKind::Missing),
            ("request timed out", FailureKind::Network),
            ("dns error: failed to lookup address", FailureKind::Network),
            ("HTTP 500 internal server error", FailureKind::Network),
            ("HTTP 502 Bad Gateway", FailureKind::Network),
            ("HTTP 504 Gateway Timeout", FailureKind::Network),
            ("取得元の応答を読み取れませんでした。取得元の作りが変わった可能性があります", FailureKind::Network),
            (
                "作品 101 の詳細は取得できましたが、本文の取得先が両方とも 404 を返しました。pixivへの接続を確認して再試行してください",
                FailureKind::Other,
            ),
            (
                "本文は取得できましたが、参照先の挿絵 123 が 404 を返しました",
                FailureKind::Other,
            ),
        ] {
            assert_eq!(classify_failure(error), expected, "{error}");
        }
    }

    /// アセットの取得失敗は、件数ではなく理由まで持ち帰る。それができて
    /// 初めて「やり直せば通るのか」を機械が判断できる。
    /// 取得制限の知らせは、生の JSON を見せずに日本語で届く。文面から
    /// 本文を外した以上、分類も日本語で通らなければ意味が変わる。
    ///
    /// FANBOX 側は元から日本語だけで知らせていたため、英語の目印しか
    /// 見ていなかったころは「その他のエラー」に落ち、間を空ければ通る
    /// はずの対象がそのまま失敗として捨てられていた。
    #[test]
    fn a_japanese_rate_limit_message_is_still_read_as_a_rate_limit() {
        for message in [
            // pixiv
            "アクセス制限（レートリミット）に達しました。時間をおいてからやり直してください",
            // FANBOX
            "FANBOXのアクセス制限に達しました。時間をおいて再試行してください",
        ] {
            assert_eq!(
                classify_failure(message),
                FailureKind::RateLimited,
                "{message}"
            );
        }
    }

    #[test]
    fn rate_limit_retries_use_a_real_exponential_wait() {
        assert_eq!(rate_limit_delay_ms(1), 60_000);
        assert_eq!(rate_limit_delay_ms(2), 120_000);
        assert_eq!(rate_limit_delay_ms(3), 240_000);
        assert!(rate_limit_delay_ms(99) <= MAX_RATE_LIMIT_BACKOFF_MS);
    }

    #[test]
    fn ordinary_errors_back_off_and_stop_before_flooding_the_provider() {
        assert_eq!(error_delay_ms(1), 10_000);
        assert_eq!(error_delay_ms(2), 20_000);
        assert!(error_delay_ms(99) <= MAX_ERROR_BACKOFF_MS);
        let limit = crate::advanced_settings::consecutive_error_limit();
        assert_eq!(limit, 3);
        assert!(!too_many_consecutive_errors(2, limit));
        assert!(too_many_consecutive_errors(3, limit));
        // 詳細設定で回数を増やせば、そこまで止まらない。
        assert!(!too_many_consecutive_errors(3, 5));
        assert!(too_many_consecutive_errors(5, 5));
        assert!(too_many_consecutive_errors(1, 1));
    }

    #[test]
    fn deleted_or_restricted_posts_never_pause_the_job() {
        // 作者が投稿をまとめて消すと、消えた作品が何件も続く。取得元は答えて
        // いるので、それで確認全体が止まってはいけない。
        assert!(!FailureKind::Missing.counts_toward_pause());
        assert!(!FailureKind::Restricted.counts_toward_pause());
        assert!(FailureKind::Network.counts_toward_pause());
        assert!(FailureKind::Other.counts_toward_pause());
    }

    #[test]
    fn asset_failures_are_classified_by_the_reason_they_carry() {
        assert_eq!(
            classify_failure("1 asset downloads failed (3 succeeded): HTTP 404 Not Found for https://i.pximg.net/a.jpg"),
            FailureKind::Missing,
        );
        assert_eq!(
            classify_failure("2 asset downloads failed (0 succeeded): HTTP 429 Too Many Requests for https://i.pximg.net/a.jpg"),
            FailureKind::RateLimited,
        );
        assert_eq!(
            classify_failure(
                "1 asset downloads failed (0 succeeded): Network error: operation timed out"
            ),
            FailureKind::Network,
        );
    }

    /// 配信側の直リンク制限でジョブ全体を止めない。本当に連携が切れて
    /// いれば、アセットより先に叩く作品APIが 401 で落ちている。
    #[test]
    fn an_assets_own_forbidden_response_does_not_demand_reconnection() {
        assert_eq!(
            classify_failure("1 asset downloads failed (5 succeeded): HTTP 403 Forbidden for https://i.pximg.net/a.jpg"),
            FailureKind::Other,
        );
        // 作品APIの 403 はこれまでどおり再接続を求める。
        assert_eq!(
            classify_failure("pixiv APIエラー（HTTP 403）"),
            FailureKind::Auth,
        );
    }

    #[test]
    fn a_saved_update_says_how_much_changed() {
        assert_eq!(describe_text_change(10_000, 11_240), "+1,240字");
        assert_eq!(describe_text_change(11_240, 10_000), "-1,240字");
        assert_eq!(describe_text_change(980, 980), "文字数は変わらず");
        assert_eq!(describe_text_change(0, 12_345_678), "+12,345,678字");
    }
}
