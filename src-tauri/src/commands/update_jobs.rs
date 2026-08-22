use crate::database::{
    DownloadEntry, StartUpdateJobRequest, UpdateCandidateInput, UpdateCredentials, UpdateJobItem,
    UpdateJobItemInput, UpdateJobSnapshot, UpdateJobSummary,
};
use crate::pixiv_api::error::PixivError;
use crate::AppState;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, Manager};

const UPDATE_JOB_DELAY_MS: u64 = 800;

/// 取得が制限されたときに間隔を掛ける倍率の上限（800ms → 最大 25.6 秒）。
const MAX_RATE_LIMIT_BACKOFF: u32 = 32;
/// 制限で止まった項目を、同じジョブの中でやり直す回数の上限。
const MAX_RATE_LIMIT_RETRIES: u32 = 2;
/// 1ジョブが持つログの上限。超えた分は古い方から落とす。
const MAX_UPDATE_JOB_LOGS: i64 = 2_000;
/// 何項目ごとにログの上限を確かめるか。
const LOG_TRIM_INTERVAL: u32 = 100;
/// 終わったジョブを、新しい方から何件残すか。
pub const KEEP_UPDATE_JOBS: i64 = 50;
/// それより古いジョブを、何日で消すか。
pub const KEEP_UPDATE_JOB_DAYS: i64 = 30;

/// 1つのジョブに戻す「前回までの候補」の上限。
const PENDING_CANDIDATE_LIMIT: i64 = 2_000;

/// メタデータの指紋が同じでも、この日数を過ぎたら本文まで突き合わせる。
///
/// 文字数の変わらない改稿を取りこぼさないための保険。短くすると取得回数が
/// 増え、長くすると細かい修正に気づくのが遅れる。
const DEEP_CHECK_INTERVAL_DAYS: i64 = 14;
static ACTIVE_UPDATE_JOBS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static PENDING_UPDATE_RESTARTS: OnceLock<Mutex<HashMap<String, UpdateCredentials>>> =
    OnceLock::new();

/// web の一覧から引けたか、引けなかったか。
enum WebLookup {
    /// 一覧に載っていた。
    Found(Box<crate::pixiv_api::web::NovelListEntryWeb>),
    /// 引けなかった。セッションが無い、伏せられている、作者が分からない、など。
    /// **失敗ではない。** 従来どおりアプリAPIで確かめればよい。
    Unknown,
    /// 一覧そのものが当てにならない。再接続が要る。
    NeedsAuth(String),
}

/// ジョブの間だけ生きる、web 一覧の覚え書き。
///
/// pixiv の web は 1 リクエストで 100 件ぶんの `updateDate` を返す。作者ごとに
/// 一度聞いて覚えておけば、作品ごとにアプリAPIを叩かずに「変わっていない」が
/// 分かる。**セッションが無ければ何もしない。** 無い状態は壊れた状態ではなく、
/// 今までどおりの経路になるだけである。
struct WebUpdateIndex {
    api: Option<crate::pixiv_api::web::WebPixivAPI>,
    asked_authors: HashSet<String>,
    /// 作者ID → その作者の作品ID → 一覧が返したもの。
    entries: HashMap<String, HashMap<String, crate::pixiv_api::web::NovelListEntryWeb>>,
    /// 一度でも「数が合わない」を見たら、以降このジョブでは一覧を使わない。
    /// 一件ごとに再接続を促しても、利用者にできることは増えない。
    gave_up: bool,
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
        let api = session.and_then(|session| {
            match crate::pixiv_api::web::WebPixivAPI::new() {
                Ok(api) => Some(api.with_session(session)),
                Err(error) => {
                    log::warn!("pixiv web クライアントを作れません: {error}");
                    None
                }
            }
        });
        Self {
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
    /// セッションが無ければ `None`。**失敗ではない** - 呼ぶ側は従来の経路へ進む。
    async fn author_listing(
        &mut self,
        state: &Arc<AppState>,
        author_id: &str,
    ) -> Result<Option<&HashMap<String, crate::pixiv_api::web::NovelListEntryWeb>>, PixivError> {
        let author_id = author_id.trim();
        if self.api.is_none() || self.gave_up || author_id.is_empty() {
            return Ok(None);
        }
        if self.asked_authors.insert(author_id.to_string()) {
            let ids: Vec<String> = state
                .db
                .pixiv_works_for_author(author_id)
                .unwrap_or_default()
                .into_iter()
                .map(|(source_id, _, _)| source_id)
                .collect();
            if ids.is_empty() {
                return Ok(Some(self.entries.entry(author_id.to_string()).or_default()));
            }
            let api = self.api.as_ref().expect("checked above");
            match api.user_novels_by_ids(author_id, &ids).await {
                Ok(listed) => {
                    let bucket = self.entries.entry(author_id.to_string()).or_default();
                    for entry in listed {
                        bucket.insert(entry.id.clone(), entry);
                    }
                }
                // 数が合わない = R-18 が黙って落ちている = セッションが切れている。
                // これを「更新なし」として通すと、ライブラリ全体が嘘をつく。
                Err(error @ PixivError::PartialListing { .. }) => {
                    self.gave_up = true;
                    return Err(error);
                }
                Err(error) => {
                    // それ以外の失敗は、この作者を諦めるだけ。従来の経路が拾う。
                    log::warn!("pixiv web 一覧を読めません（作者 {author_id}）: {error}");
                    self.entries.entry(author_id.to_string()).or_default();
                }
            }
        }
        Ok(self.entries.get(author_id))
    }

    /// この作品について、取得元での最終更新を引く。
    async fn lookup(&mut self, state: &Arc<AppState>, dl: &DownloadEntry) -> WebLookup {
        match self.author_listing(state, &dl.author_id).await {
            Err(error) => WebLookup::NeedsAuth(format!("pixiv連携の再接続が必要です（{error}）")),
            Ok(None) => WebLookup::Unknown,
            Ok(Some(entries)) => match entries.get(&dl.source_id) {
                // 伏せられた作品はメタデータが当てにならない。判定に使わない。
                Some(entry) if entry.is_masked => WebLookup::Unknown,
                Some(entry) => WebLookup::Found(Box::new(entry.clone())),
                None => WebLookup::Unknown,
            },
        }
    }
}

/// この `updateDate` を基準値として覚えてよいか。
///
/// 覚えるとは「次はこの値と同じなら本文を見ない」と決めることなので、
/// **いま手元にある本文がその更新のあとのものだ、と言えなければ覚えてはいけない。**
/// 言えないまま覚えると、その改稿は二度と拾われなくなる。
///
/// 読めない時刻は「言えない」に倒す。
fn may_remember(listed_update: Option<&str>, verified_at: Option<&str>) -> bool {
    let Some(listed) = listed_update.and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return false;
    };
    verified_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|verified| verified >= listed)
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
    format!("job-{:032x}", rand::random::<u128>())
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

fn pixiv_tags(data: &Value) -> Vec<String> {
    let Some(tags) = value_at(data, &[&["detail", "tags"], &["tags"]]) else {
        return Vec::new();
    };
    if let Some(items) = tags.as_array() {
        return items
            .iter()
            .filter_map(|tag| {
                tag.as_str()
                    .map(str::to_string)
                    .or_else(|| string_at(tag, &[&["name"]]))
            })
            .collect();
    }
    if let Some(items) = tags.get("tags").and_then(|v| v.as_array()) {
        return items
            .iter()
            .filter_map(|tag| {
                tag.as_str()
                    .map(str::to_string)
                    .or_else(|| string_at(tag, &[&["name"]]))
            })
            .collect();
    }
    Vec::new()
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
    updated: &DownloadEntry,
    previous_length: Option<i64>,
    previous_version: Option<i64>,
) -> ItemOutcome {
    match (previous_length, previous_version) {
        (Some(before), Some(version)) if updated.current_version > version => ItemOutcome::Saved(
            updated.id,
            format!(
                "改稿を保存: {}（{}, v{}）",
                updated.title,
                describe_text_change(before, updated.text_length),
                updated.current_version
            ),
        ),
        (Some(_), Some(_)) => {
            ItemOutcome::Skipped(format!("中身は変わっていません: {}", updated.title))
        }
        _ => ItemOutcome::Saved(updated.id, format!("新作を保存: {}", updated.title)),
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
        "subtitle": if author == target_label { date.clone() } else { format!("{} ・ {}", author, date) },
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
    state: &Arc<AppState>,
    target: &crate::database::UpdateTarget,
    watched_authors: &HashSet<(String, String)>,
) -> Result<bool, String> {
    if watched_authors.is_empty() {
        return Ok(false);
    }
    let authors = state
        .db
        .series_author_keys(&target.source, &target.source_key)?;
    Ok(authors
        .iter()
        .any(|author| watched_authors.contains(&(target.source.clone(), author.clone()))))
}

fn build_initial_items(
    state: &Arc<AppState>,
    request: &StartUpdateJobRequest,
) -> Result<Vec<UpdateJobItemInput>, String> {
    let mut items = Vec::new();
    let scope = request.scope.as_str();
    let include_work = matches!(scope, "all" | "work");
    let include_author = matches!(scope, "all" | "author");
    let include_series = matches!(scope, "all" | "series");
    let work_filter = request
        .work_ids
        .as_ref()
        .map(|ids| ids.iter().copied().collect::<HashSet<_>>());
    let target_filter = request
        .target_ids
        .as_ref()
        .map(|ids| ids.iter().copied().collect::<HashSet<_>>());

    if include_work {
        let works = if let Some(filter) = &work_filter {
            let mut entries = Vec::new();
            for id in filter {
                entries.push(state.db.get_download(*id)?);
            }
            entries
        } else {
            state.db.get_watched_downloads()?
        };
        for dl in works {
            if dl.source != "pixiv" && dl.source != "fanbox" {
                continue;
            }
            let title = dl.title.clone();
            let payload = serde_json::to_value(dl).map_err(|e| e.to_string())?;
            items.push(item_from_payload(
                "work",
                "queued",
                payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                payload
                    .get("sourceId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                Some("work".to_string()),
                title,
                payload,
            )?);
        }
    }

    // 一回きりの確認先（作品ページや作者ページの「新作を確認」）。
    // 監視対象には登録しない。登録済みのものを指していれば、その前回位置を
    // 引き継いで無駄に遡らない。
    for adhoc in request.adhoc_targets.iter().flatten() {
        if adhoc.target_type != "author" && adhoc.target_type != "series" {
            continue;
        }
        let registered = state
            .db
            .find_update_target(&adhoc.target_type, &adhoc.source, &adhoc.source_key)?;
        let target = registered.unwrap_or_else(|| crate::database::UpdateTarget {
            id: 0,
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
    if include_author || include_series {
        for candidate in state.db.list_pending_update_candidates(PENDING_CANDIDATE_LIMIT)? {
            // すでに手元にあるものは、改稿として見つけた場合だけ残す。
            let saved = state
                .db
                .get_download_by_source(&candidate.source, &candidate.source_id)?;
            if saved.is_some() && candidate.kind != CandidateKind::Revision.key() {
                state
                    .db
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

    if include_author || include_series {
        let targets = state.db.list_update_targets(None, true)?;
        // 作者を監視しているなら、その人のシリーズはその一覧に出てくる。
        // 両方を走査すると同じものを二度取りに行くので、取得先への負荷になる。
        let watched_authors: HashSet<(String, String)> = targets
            .iter()
            .filter(|target| target.target_type == "author")
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
                && series_covered_by_watched_author(state, &target, &watched_authors)?
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
    if let Ok(snapshot) = state.db.update_job_snapshot(job_id) {
        let _ = app.emit("update-job-progress", snapshot);
    }
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
            run_update_job(worker_app, worker_job_id, credentials).await
        });
        let run_result = match worker.await {
            Ok(result) => result,
            Err(error) => Err(format!("更新ジョブworkerが異常終了しました: {error}")),
        };
        if let Err(error) = run_result {
            let state = app.state::<Arc<AppState>>().inner().clone();
            let _ = state.db.append_update_job_log(&job_id, "error", &error);
            if has_pending_restart(&job_id) {
                // The replacement worker owns the queued state. A panic or
                // late failure from the retired worker must not overwrite it.
                emit_snapshot(&app, &state, &job_id).await;
            } else {
                // A late network/database error must not resurrect a job after the
                // user canceled it while an item was in flight.
                match state.db.update_job_status_value(&job_id).as_deref() {
                    Ok("canceling") => {
                        let _ = finish_update_job_cancellation(&app, &state, &job_id).await;
                    }
                    Ok("canceled") => {}
                    _ => {
                        let _ = state
                            .db
                            .set_update_job_status(&job_id, "failed", Some(&error));
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
    state.db.prepare_update_job_resume(job_id, false)?;
    state
        .db
        .set_update_job_status(job_id, "canceled", Some("キャンセルしました"))?;
    state
        .db
        .append_update_job_log(job_id, "warn", "更新ジョブをキャンセルしました")?;
    emit_snapshot(app, state, job_id).await;
    Ok(())
}

#[tauri::command]
pub async fn start_update_job(
    app: tauri::AppHandle,
    request: StartUpdateJobRequest,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let items = build_initial_items(&state, &request)?;
    if items.is_empty() {
        return Err("更新監視対象がありません".to_string());
    }
    let job_id = make_job_id();
    let snapshot = state.db.create_update_job(&job_id, &request, &items)?;
    spawn_update_job(
        app.clone(),
        job_id.clone(),
        request
            .credentials
            .clone()
            .unwrap_or_else(snapshot_credentials_missing),
    );
    emit_snapshot(&app, &state, &job_id).await;
    Ok(snapshot)
}

#[tauri::command]
pub async fn pause_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.db.prepare_update_job_resume(&job_id, false)?;
    state
        .db
        .set_update_job_status(&job_id, "paused", Some("一時停止しました"))?;
    state
        .db
        .append_update_job_log(&job_id, "warn", "更新ジョブを一時停止しました")?;
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn resume_update_job(
    app: tauri::AppHandle,
    job_id: String,
    credentials: UpdateCredentials,
    retry_failed: Option<bool>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state
        .db
        .prepare_update_job_resume(&job_id, retry_failed.unwrap_or(false))?;
    state
        .db
        .set_update_job_status(&job_id, "queued", Some("再開待ち"))?;
    state
        .db
        .append_update_job_log(&job_id, "info", "更新ジョブを再開しました")?;
    spawn_update_job(app.clone(), job_id.clone(), credentials);
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn cancel_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let current = state.db.update_job_status_value(&job_id)?;
    match canceled_status_for(&current)? {
        Some("canceled") => finish_update_job_cancellation(&app, &state, &job_id).await?,
        Some("canceling") => {
            state
                .db
                .set_update_job_status(&job_id, "canceling", Some("キャンセル中"))?;
            state.db.append_update_job_log(
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
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn get_update_job(
    app: tauri::AppHandle,
    job_id: String,
    candidate_after_id: Option<i64>,
    log_before_id: Option<i64>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .update_job_snapshot_page(&job_id, candidate_after_id, log_before_id)
}

#[tauri::command]
pub async fn list_update_jobs(app: tauri::AppHandle) -> Result<Vec<UpdateJobSummary>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.list_update_jobs()
}

#[tauri::command]
pub async fn save_update_job_candidates(
    app: tauri::AppHandle,
    job_id: String,
    candidate_ids: Vec<i64>,
    credentials: Option<UpdateCredentials>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let changed = state
        .db
        .queue_update_job_candidates(&job_id, &candidate_ids)?;
    if changed > 0 {
        state.db.append_update_job_log(
            &job_id,
            "info",
            &format!("{}件の候補を保存キューに追加しました", changed),
        )?;
        state
            .db
            .set_update_job_status(&job_id, "queued", Some("候補保存待ち"))?;
        spawn_update_job(
            app.clone(),
            job_id.clone(),
            credentials.unwrap_or_else(snapshot_credentials_missing),
        );
    }
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

fn snapshot_credentials_missing() -> UpdateCredentials {
    UpdateCredentials {
        pixiv_refresh_token: None,
        pixiv_cookie: None,
        pixiv_user_agent: None,
        fanbox_cookie: None,
        fanbox_user_agent: None,
    }
}

#[tauri::command]
pub async fn clear_update_job(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.db.clear_update_job(&job_id)
}

/// 終わった更新ジョブをまとめて消す。操作履歴の「完了履歴を消去」から呼ぶ。
#[tauri::command]
pub async fn clear_finished_update_jobs(app: tauri::AppHandle) -> Result<usize, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.clear_finished_update_jobs()
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
    state.db.set_update_candidate_status(
        &source,
        &source_id,
        if dismissed { "dismissed" } else { "pending" },
    )
}

#[tauri::command]
pub async fn count_dismissed_update_candidates(app: tauri::AppHandle) -> Result<i64, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.count_dismissed_update_candidates()
}

/// 無視した作品をすべて戻す。次の確認でまた候補に並ぶ。
#[tauri::command]
pub async fn restore_dismissed_update_candidates(app: tauri::AppHandle) -> Result<usize, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.restore_dismissed_update_candidates()
}

async fn run_update_job(
    app: tauri::AppHandle,
    job_id: String,
    credentials: UpdateCredentials,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    match state.db.update_job_status_value(&job_id)?.as_str() {
        "canceling" => {
            finish_update_job_cancellation(&app, &state, &job_id).await?;
            return Ok(());
        }
        "queued" | "running" => {}
        // A stale scheduled task must not revive a paused or terminal job.
        _ => return Ok(()),
    }
    state
        .db
        .set_update_job_status(&job_id, "running", Some("更新チェックを開始しています"))?;
    emit_snapshot(&app, &state, &job_id).await;

    // 保存した作品をそのまま監視に載せるかは、ジョブを始めたときの依頼が持つ。
    // 再開したワーカーも同じ設定で動く。
    let watch_saved = state.db.update_job_watch_saved(&job_id).unwrap_or(false);
    // web の一覧はジョブの間だけ覚えておく。セッションが無ければ空のまま、
    // 何もしない置物として振る舞う。
    let mut web_index = WebUpdateIndex::new(&credentials);
    // 取得が制限されたときだけ間隔を広げ、うまくいけば元へ戻す。
    let mut backoff: u32 = 1;
    let mut rate_limit_retries: HashMap<i64, u32> = HashMap::new();
    let mut processed_since_trim: u32 = 0;

    loop {
        if has_pending_restart(&job_id) {
            break;
        }
        let status = state.db.update_job_status_value(&job_id)?;
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
                .db
                .set_update_job_status(&job_id, "canceled", Some("キャンセルしました"))?;
            state
                .db
                .append_update_job_log(&job_id, "warn", "更新ジョブをキャンセルしました")?;
            emit_snapshot(&app, &state, &job_id).await;
            break;
        }

        let Some(item) = state.db.next_update_job_item(&job_id)? else {
            let snapshot = state.db.update_job_snapshot(&job_id)?;
            let final_status = if snapshot.error_count > 0 {
                "failed"
            } else {
                "completed"
            };
            let label = if final_status == "failed" {
                "エラーありで終了しました"
            } else {
                "完了しました"
            };
            state
                .db
                .set_update_job_status(&job_id, final_status, Some(label))?;
            state.db.append_update_job_log(
                &job_id,
                if final_status == "failed" {
                    "error"
                } else {
                    "success"
                },
                label,
            )?;
            emit_snapshot(&app, &state, &job_id).await;
            break;
        };

        let outcome =
            process_update_job_item(&app, &state, &job_id, &item, &credentials, &mut web_index)
                .await;
        let restart_pending = has_pending_restart(&job_id);
        match outcome {
            Ok(ItemOutcome::Done(message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "done", None, None)?;
                state
                    .db
                    .append_update_job_log(&job_id, "success", &message)?;
            }
            Ok(ItemOutcome::Saved(download_id, message)) => {
                backoff = 1;
                state
                    .db
                    .complete_update_job_item(item.id, "saved", None, Some(download_id))?;
                // 保存できた候補は、もう「決めていないもの」ではない。
                if let (Some(source), Some(source_id)) =
                    (item.source.as_deref(), item.source_id.as_deref())
                {
                    let _ = state.db.clear_update_candidate(source, source_id);
                }
                state
                    .db
                    .append_update_job_log(&job_id, "success", &message)?;
                // 保存したものを続けて追いたい、という設定のときだけ監視に載せる。
                // 失敗しても保存そのものは済んでいるので、警告に留める。
                if watch_saved {
                    if let Err(error) = state.db.set_watch_updates(download_id, true) {
                        log::warn!("更新監視を有効にできません ({download_id}): {error}");
                    }
                }
            }
            Ok(ItemOutcome::Skipped(message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "skipped", None, None)?;
                state.db.append_update_job_log(&job_id, "info", &message)?;
            }
            Ok(ItemOutcome::AuthRequired(message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "queued", Some(&message), None)?;
                if restart_pending {
                    break;
                } else if state.db.update_job_status_value(&job_id)? == "canceling" {
                    finish_update_job_cancellation(&app, &state, &job_id).await?;
                    break;
                } else {
                    state.db.append_update_job_log(&job_id, "warn", &message)?;
                    state
                        .db
                        .set_update_job_status(&job_id, "auth_required", Some(&message))?;
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
                let kind = classify_failure(&error);
                if kind == FailureKind::Auth {
                    let message = format!("{}の認証を更新してください", item.title);
                    state
                        .db
                        .complete_update_job_item(item.id, "queued", Some(&message), None)?;
                    state.db.append_update_job_log(&job_id, "warn", &message)?;
                    state
                        .db
                        .set_update_job_status(&job_id, "auth_required", Some(&message))?;
                    emit_snapshot(&app, &state, &job_id).await;
                    break;
                }

                // 取得制限は「相手が今は無理と言っている」だけなので、間隔を
                // 広げて同じ項目をやり直す。何度も続くようなら諦めて次へ行く。
                if kind == FailureKind::RateLimited {
                    let attempts = rate_limit_retries.entry(item.id).or_insert(0);
                    *attempts += 1;
                    if *attempts <= MAX_RATE_LIMIT_RETRIES {
                        backoff = (backoff * 2).min(MAX_RATE_LIMIT_BACKOFF);
                        let message = format!(
                            "取得が制限されています。{}秒あけて「{}」をやり直します",
                            UPDATE_JOB_DELAY_MS * u64::from(backoff) / 1000,
                            item.title
                        );
                        state
                            .db
                            .complete_update_job_item(item.id, "queued", Some(&message), None)?;
                        state.db.append_update_job_log(&job_id, "warn", &message)?;
                        emit_snapshot(&app, &state, &job_id).await;
                        tokio::time::sleep(std::time::Duration::from_millis(
                            UPDATE_JOB_DELAY_MS * u64::from(backoff),
                        ))
                        .await;
                        continue;
                    }
                }

                // 失敗の理由を項目とログの両方に残す。あとで「何を再試行すべきか」
                // を選べるようにするための材料になる。
                let reason = format!("[{}] {}", kind.label(), error);
                state
                    .db
                    .complete_update_job_item(item.id, "failed", Some(&reason), None)?;
                state.db.append_update_job_log(
                    &job_id,
                    "error",
                    &format!("{}: {}", item.title, reason),
                )?;
                if item.item_type == "target" {
                    if let (Some(source), Some(source_key), Some(target_type)) = (
                        item.source.as_deref(),
                        item.source_id.as_deref(),
                        item.target_type.as_deref(),
                    ) {
                        let _ = state
                            .db
                            .mark_update_target_failed(target_type, source, source_key);
                    }
                }
            }
        }

        // ログは1ジョブに何万行も溜まりうる。時々古い方から落とす。
        processed_since_trim += 1;
        if processed_since_trim >= LOG_TRIM_INTERVAL {
            processed_since_trim = 0;
            let _ = state.db.trim_update_job_logs(&job_id, MAX_UPDATE_JOB_LOGS);
        }
        if restart_pending {
            break;
        }
        emit_snapshot(&app, &state, &job_id).await;
        if state.db.update_job_status_value(&job_id)? == "canceling" {
            finish_update_job_cancellation(&app, &state, &job_id).await?;
            break;
        }
        // 制限を受けたあとは間隔を広げたままにし、続けて通るようになったら
        // 半分ずつ元へ戻す。取得元への負荷を自分で調整する。
        tokio::time::sleep(std::time::Duration::from_millis(
            UPDATE_JOB_DELAY_MS * u64::from(backoff),
        ))
        .await;
        backoff = (backoff / 2).max(1);
    }

    Ok(())
}

enum ItemOutcome {
    Done(String),
    Saved(i64, String),
    Skipped(String),
    AuthRequired(String),
}

/// 失敗の種類。同じ「エラー1件」でも、次にすべきことが違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    /// 再接続が要る。ジョブを止めて利用者に知らせる。
    Auth,
    /// 取得が制限されている。間隔を空けて、この項目はやり直す。
    RateLimited,
    /// 消された・非公開になった。やり直しても結果は変わらない。
    Missing,
    /// 通信が届かなかった。次の実行で拾えることが多い。
    Network,
    Other,
}

impl FailureKind {
    /// 候補一覧とログに出す短い理由。
    fn label(self) -> &'static str {
        match self {
            FailureKind::Auth => "再接続が必要",
            FailureKind::RateLimited => "取得制限",
            FailureKind::Missing => "見つからない",
            FailureKind::Network => "通信エラー",
            FailureKind::Other => "エラー",
        }
    }
}

fn classify_failure(error: &str) -> FailureKind {
    let normalized = error.to_ascii_lowercase();
    let has = |markers: &[&str]| markers.iter().any(|marker| normalized.contains(marker));

    // 画像や添付の取得で出た 401/403 を、連携切れとして扱わない。
    //
    // 作品本体の API は必ずアセットより先に叩いているので、本当に session が
    // 切れていればそちらが先に失敗する。ここまで来た 403 は i.pximg.net の
    // 直リンク制限のような配信側の都合で、ジョブ全体を止めて「再接続して
    // ください」と言う理由にはならない。取得制限・欠落・通信の判断は残す。
    let from_asset_fetch = normalized.contains("asset downloads failed");

    if !from_asset_fetch
        && has(&[
        "認証が必要",
        "アクセストークンが不正",
        "セッションが無効",
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
    ]) {
        return FailureKind::RateLimited;
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
        "接続できません",
        "タイムアウト",
    ]) {
        return FailureKind::Network;
    }
    FailureKind::Other
}

async fn process_update_job_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    web_index: &mut WebUpdateIndex,
) -> Result<ItemOutcome, String> {
    match item.item_type.as_str() {
        "work" => process_work_item(app, state, item, credentials, web_index).await,
        "target" => process_target_item(state, job_id, item, credentials, web_index).await,
        "candidate" => process_candidate_item(app, state, item, credentials).await,
        _ => Ok(ItemOutcome::Skipped(format!(
            "未対応のジョブ項目をスキップ: {}",
            item.item_type
        ))),
    }
}

/// 本文まで取りに行かずに済むか。
///
/// pixiv の詳細 API は本文以外の材料（題・キャプション・表紙 URL・タグ・
/// シリーズ・文字数）をすべて返すので、保存時に残した指紋と突き合わせれば
/// 変化の有無が分かる。1作品あたりの取得が 2 回から 1 回に減る。
///
/// **取りこぼし**: 文字数が変わらない修正（誤字の置換など）は指紋に出ない。
/// そのため、指紋が同じでも `DEEP_CHECK_INTERVAL_DAYS` を過ぎていれば本文まで
/// 取りに行く。取りこぼしうるのは「その期間内の、文字数の変わらない改稿」だけで、
/// 期間を過ぎれば必ず本文のハッシュで拾われる。
fn is_unchanged_pixiv_work(
    state: &Arc<AppState>,
    dl: &DownloadEntry,
    metadata: &crate::downloader::pixiv::PixivNovelDetail,
) -> bool {
    let Ok((stored_hash, last_deep_checked_at)) = state.db.get_download_meta_state(dl.id) else {
        return false;
    };
    let Some(stored_hash) = stored_hash else {
        return false;
    };
    let tags: Vec<String> = metadata
        .tags
        .iter()
        .map(|tag| tag.name.clone())
        .collect();
    let signature = super::downloader::pixiv_meta_signature(
        &metadata.title,
        &metadata.caption,
        metadata.cover_url.as_deref().unwrap_or(""),
        &tags,
        metadata.series_id.as_deref().unwrap_or(""),
        u64::from(metadata.text_length),
    );
    if signature != stored_hash {
        return false;
    }
    last_deep_checked_at
        .and_then(|checked| chrono::DateTime::parse_from_rfc3339(&checked).ok())
        .is_some_and(|checked| {
            chrono::Utc::now().signed_duration_since(checked.with_timezone(&chrono::Utc))
                < chrono::Duration::days(DEEP_CHECK_INTERVAL_DAYS)
        })
}

/// 本文を取り直さずに終えたとき、その `updateDate` を覚えてよければ覚える。
///
/// 覚えてよいのは「本文まで突き合わせた時刻が、その更新より後」のときだけ
/// （[`may_remember`]）。言い切れないときは何もしない。次の確認で従来どおり
/// 確かめれば済むし、そのほうが取りこぼさない。
fn remember_source_updated_at(state: &Arc<AppState>, dl: &DownloadEntry, listed: Option<&str>) {
    let Some(listed) = listed else { return };
    let verified = state
        .db
        .get_download_meta_state(dl.id)
        .ok()
        .and_then(|(_, checked)| checked);
    if !may_remember(Some(listed), verified.as_deref()) {
        return;
    }
    if let Err(error) = state.db.set_download_source_updated_at(dl.id, listed) {
        log::warn!("pixiv の更新時刻を覚えられません ({}): {error}", dl.id);
    }
}

async fn process_work_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    web_index: &mut WebUpdateIndex,
) -> Result<ItemOutcome, String> {
    let dl: DownloadEntry = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    if dl.source == "pixiv" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };

        // 0段目。web の一覧は、100件まとめて `updateDate` を返す。ここで
        // 「変わっていない」と分かれば、アプリAPIを一度も叩かずに終わる。
        let listed_update = match web_index.lookup(state, &dl).await {
            WebLookup::NeedsAuth(message) => return Ok(ItemOutcome::AuthRequired(message)),
            WebLookup::Found(entry) => entry.update_date.clone(),
            WebLookup::Unknown => None,
        };
        if let (Some(listed), Some(stored)) =
            (listed_update.as_deref(), dl.source_updated_at.as_deref())
        {
            if listed == stored {
                return Ok(ItemOutcome::Skipped(format!("最新: {}", dl.title)));
            }
        }

        // 1段目の取得は本文を含まない詳細だけ。変わっていなければここで終わる。
        let metadata =
            super::downloader::fetch_pixiv_novel_metadata(dl.source_id.clone(), token.clone())
                .await?;
        if is_unchanged_pixiv_work(state, &dl, &metadata) {
            // 指紋は「変わっていない」と言い、一覧は「変わった」と言った。
            // どちらが早く気づくのかを知りたいので、食い違いは記録に残す。
            if listed_update.is_some() {
                log::info!(
                    "pixiv {}: 一覧は更新を示したが、指紋は変化なし（updateDate={:?}）",
                    dl.source_id,
                    listed_update
                );
            }
            remember_source_updated_at(state, &dl, listed_update.as_deref());
            return Ok(ItemOutcome::Skipped(format!("最新: {}", dl.title)));
        }
        let data = super::downloader::fetch_pixiv_novel(dl.source_id.clone(), token).await?;
        let value = serde_json::to_value(&data).map_err(|e| e.to_string())?;
        let title =
            string_at(&value, &[&["detail", "title"], &["title"]]).unwrap_or(dl.title.clone());
        let author_name = string_at(&value, &[&["detail", "user", "name"], &["user", "name"]])
            .unwrap_or(dl.author_name.clone());
        let author_id = string_at(&value, &[&["detail", "user", "id"], &["user", "id"]])
            .unwrap_or(dl.author_id.clone());
        let source_created_at = string_at(
            &value,
            &[
                &["detail", "create_date"],
                &["detail", "createDate"],
                &["create_date"],
                &["createDate"],
            ],
        );
        let excerpt = string_at(&value, &[&["detail", "caption"], &["caption"]]);
        let updated = super::downloader::download_and_save(
            app.clone(),
            value,
            "pixiv".to_string(),
            dl.source_id.clone(),
            title,
            author_name,
            author_id,
            "novel".to_string(),
            Some(pixiv_tags(
                &serde_json::to_value(&data).map_err(|e| e.to_string())?,
            )),
            excerpt,
            source_created_at,
            None,
            None,
        )
        .await?;
        // 本文まで取り直した直後なので、この版が手元にあると言い切れる。
        if let Some(listed) = listed_update.as_deref() {
            if let Err(error) = state.db.set_download_source_updated_at(updated.id, listed) {
                log::warn!("pixiv の更新時刻を覚えられません ({}): {error}", updated.id);
            }
        }
        if updated.current_version > dl.current_version {
            Ok(ItemOutcome::Saved(
                updated.id,
                format!(
                    "更新を保存: {}（{}, v{}）",
                    updated.title,
                    describe_text_change(dl.text_length, updated.text_length),
                    updated.current_version
                ),
            ))
        } else {
            Ok(ItemOutcome::Skipped(format!("最新: {}", updated.title)))
        }
    } else if dl.source == "fanbox" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "FANBOX連携が必要です".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        let post = super::downloader::fetch_fanbox_post(
            dl.source_id.clone(),
            cookie.clone(),
            user_agent.clone(),
        )
        .await?;
        let value = serde_json::to_value(&post).map_err(|e| e.to_string())?;
        let updated_at = string_at(&value, &[&["updatedDatetime"], &["updated_datetime"]]);
        if dl.source_updated_at == updated_at && dl.source_updated_at.is_some() {
            return Ok(ItemOutcome::Skipped(format!("最新: {}", dl.title)));
        }
        let title = string_at(&value, &[&["title"]]).unwrap_or(dl.title.clone());
        let author_name = string_at(&value, &[&["user", "name"]]).unwrap_or(dl.author_name.clone());
        let author_id = string_at(
            &value,
            &[&["creatorId"], &["creator_id"], &["user", "userId"]],
        )
        .unwrap_or(dl.author_id.clone());
        let content_type = string_at(&value, &[&["type"], &["postType"], &["post_type"]])
            .unwrap_or_else(|| "article".to_string());
        let tags = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let updated = super::downloader::download_and_save(
            app.clone(),
            value,
            "fanbox".to_string(),
            dl.source_id.clone(),
            title,
            author_name,
            author_id,
            content_type,
            Some(tags),
            string_at(
                &serde_json::to_value(&post).map_err(|e| e.to_string())?,
                &[&["excerpt"], &["body", "excerpt"]],
            ),
            string_at(
                &serde_json::to_value(&post).map_err(|e| e.to_string())?,
                &[&["publishedDatetime"], &["published_datetime"]],
            ),
            Some(cookie),
            Some(user_agent),
        )
        .await?;
        if updated.current_version > dl.current_version {
            Ok(ItemOutcome::Saved(
                updated.id,
                format!(
                    "更新を保存: {}（{}, v{}）",
                    updated.title,
                    describe_text_change(dl.text_length, updated.text_length),
                    updated.current_version
                ),
            ))
        } else {
            Ok(ItemOutcome::Skipped(format!("最新: {}", updated.title)))
        }
    } else {
        Ok(ItemOutcome::Skipped(format!(
            "未対応ソースをスキップ: {}",
            dl.source
        )))
    }
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
    target: &crate::database::UpdateTarget,
    source_item: &Value,
    kind: CandidateKind,
    auto_save: bool,
) -> Result<bool, String> {
    let Some(payload) = candidate_payload(
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
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("無題")
        .to_string();
    state.db.upsert_update_candidate(&UpdateCandidateInput {
        source: source.clone(),
        source_id: source_id.clone(),
        kind: kind.key().to_string(),
        title: title.clone(),
        payload_json: serde_json::to_string(&payload).map_err(|e| e.to_string())?,
        target_type: Some(target.target_type.clone()),
    })?;
    state.db.insert_update_job_candidate(
        job_id,
        &item_from_payload(
            "candidate",
            if auto_save { "queued" } else { "candidate" },
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
    target: &crate::database::UpdateTarget,
    auto_save: bool,
    web_index: &mut WebUpdateIndex,
) -> Result<i64, String> {
    let Some(entries) = web_index
        .author_listing(state, &target.source_key)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(0);
    };

    let saved = state.db.pixiv_works_for_author(&target.source_key)?;
    let mut found = 0i64;
    for (source_id, stored_update, downloaded_at) in saved {
        let Some(entry) = entries.get(&source_id) else {
            continue;
        };
        // 伏せられた作品はメタデータが当てにならない。判定に使わない。
        if entry.is_masked {
            continue;
        }
        let Some(listed) = entry.update_date.as_deref() else {
            continue;
        };
        match stored_update.as_deref() {
            // 覚えている値と同じなら、取得元では何も起きていない。
            Some(stored) if stored == listed => continue,
            // 覚えている値と違う。改稿である。
            Some(_) => {}
            // 初めて見る作品。保存より前の更新なら、手元の本文はその版のもの。
            None if may_remember(Some(listed), Some(&downloaded_at)) => {
                if let Ok(Some(existing)) = state.db.get_download_by_source("pixiv", &source_id) {
                    let _ = state.db.set_download_source_updated_at(existing.id, listed);
                }
                continue;
            }
            // 保存より後に直されている。古いかもしれないので、候補に出す。
            None => {}
        }
        if state
            .db
            .update_candidate_status("pixiv", &source_id)?
            .as_deref()
            == Some("dismissed")
        {
            continue;
        }
        let item = serde_json::json!({
            "id": source_id,
            "title": entry.title.clone().unwrap_or_else(|| "無題".to_string()),
            "user": { "name": target.display_name },
            "create_date": entry.create_date.clone().unwrap_or_default(),
        });
        if record_candidate(
            state,
            job_id,
            target,
            &item,
            CandidateKind::Revision,
            auto_save,
        )? {
            found += 1;
        }
    }
    Ok(found)
}

async fn process_target_item(
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    web_index: &mut WebUpdateIndex,
) -> Result<ItemOutcome, String> {
    let target: crate::database::UpdateTarget =
        serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    let snapshot = state.db.update_job_snapshot(job_id)?;
    let auto_save = snapshot.mode == "auto_save";

    let items: Vec<Value> = if target.source == "pixiv" && target.target_type == "author" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        super::downloader::fetch_pixiv_user_novels_since(
            target.source_key.clone(),
            token,
            target.last_seen_source_id.as_deref(),
        )
        .await?
        .into_iter()
        .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?
    } else if target.source == "pixiv" && target.target_type == "series" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        super::downloader::fetch_pixiv_series_novels_since(
            target.source_key.clone(),
            token,
            target.last_seen_source_id.as_deref(),
        )
        .await?
        .into_iter()
        .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?
    } else if target.source == "fanbox" && target.target_type == "author" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "FANBOX連携が必要です".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        super::downloader::fetch_fanbox_creator_posts_since(
            target.source_key.clone(),
            cookie,
            user_agent,
            target.last_seen_source_id.as_deref(),
        )
        .await?
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
        let kind = match state.db.get_download_by_source(&target.source, &listed_id)? {
            None => default_kind,
            Some(existing) => {
                // すでに持っている作品。一覧が更新時刻を持っていて、それが
                // 保存済みのものと違うときだけ「改稿」として拾う。
                // pixiv の一覧は投稿日しか持たないので、そちらは監視オンの
                // 作品を辿る経路（work 項目）に任せる。
                let listed_updated = string_at(
                    source_item,
                    &[&["updatedDatetime"], &["updated_datetime"]],
                );
                match (listed_updated, existing.source_updated_at.as_deref()) {
                    (Some(listed), Some(stored)) if listed != stored => CandidateKind::Revision,
                    _ => continue,
                }
            }
        };

        // 一度「今後は出さない」と決めた作品は、見つけても候補にしない。
        if state
            .db
            .update_candidate_status(&target.source, &listed_id)?
            .as_deref()
            == Some("dismissed")
        {
            continue;
        }

        if record_candidate(state, job_id, &target, source_item, kind, auto_save)? {
            found += 1;
        }
    }

    // 作者を監視しているなら、その作者の作品の改稿も同じ確認で分かるべき。
    // FANBOX は取得元の一覧が更新時刻を持つのでずっとそうなっていた。pixiv
    // だけができなかったのは、アプリAPIの一覧に更新時刻が無いからで、
    // web の一覧を通せばこの非対称は消える。
    if target.source == "pixiv" && target.target_type == "author" {
        found += scan_pixiv_revisions(state, job_id, &target, auto_save, web_index).await?;
    }

    let first = items.first();
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
        state.db.mark_update_target_checked(
            &target.target_type,
            &target.source,
            &target.source_key,
            first_id.as_deref(),
            first_updated.as_deref(),
            found,
        )?;
    }

    Ok(ItemOutcome::Done(if found > 0 {
        format!("{}: 候補 {} 件", target.display_name, found)
    } else {
        format!("{}: 新しいものはありません", target.display_name)
    }))
}

async fn process_candidate_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
) -> Result<ItemOutcome, String> {
    let payload: Value = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    let source = item.source.as_deref().unwrap_or("");
    let source_id = item.source_id.as_deref().unwrap_or("");
    // 改稿の候補は「すでに手元にある作品」が対象なので、保存済みでも取りに行く。
    // 中身が本当に変わっていなければ download_and_save 側が版を上げずに終わる。
    let is_revision = payload.get("kind").and_then(|v| v.as_str()) == Some("revision");
    let existing = state.db.get_download_by_source(source, source_id)?;
    if existing.is_some() && !is_revision {
        return Ok(ItemOutcome::Skipped(format!(
            "保存済みのためスキップ: {}",
            item.title
        )));
    }
    let previous_length = existing.as_ref().map(|download| download.text_length);
    let previous_version = existing.as_ref().map(|download| download.current_version);

    if source == "pixiv" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        let data = super::downloader::fetch_pixiv_novel(source_id.to_string(), token).await?;
        let value = serde_json::to_value(&data).map_err(|e| e.to_string())?;
        let title =
            string_at(&value, &[&["detail", "title"], &["title"]]).unwrap_or(item.title.clone());
        let author_name = string_at(&value, &[&["detail", "user", "name"], &["user", "name"]])
            .or_else(|| string_at(&payload, &[&["originalData", "user", "name"]]))
            .unwrap_or_else(|| "unknown".to_string());
        let author_id = string_at(&value, &[&["detail", "user", "id"], &["user", "id"]])
            .or_else(|| string_at(&payload, &[&["originalData", "user", "id"]]))
            .unwrap_or_else(|| "0".to_string());
        let source_created_at = string_at(
            &value,
            &[
                &["detail", "create_date"],
                &["detail", "createDate"],
                &["create_date"],
                &["createDate"],
            ],
        );
        let excerpt = string_at(&value, &[&["detail", "caption"], &["caption"]]);
        let tags = pixiv_tags(&value);
        let updated = super::downloader::download_and_save(
            app.clone(),
            value.clone(),
            "pixiv".to_string(),
            source_id.to_string(),
            title,
            author_name,
            author_id,
            "novel".to_string(),
            Some(tags),
            excerpt,
            source_created_at,
            None,
            None,
        )
        .await?;
        // 保存したついでにシリーズを監視へ入れることはしない。監視対象は
        // 利用者が自分で選ぶもので、勝手に増えると取得先への負荷にもなる。
        Ok(describe_candidate_save(
            &updated,
            previous_length,
            previous_version,
        ))
    } else if source == "fanbox" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "FANBOX連携が必要です".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        let post = super::downloader::fetch_fanbox_post(
            source_id.to_string(),
            cookie.clone(),
            user_agent.clone(),
        )
        .await?;
        let value = serde_json::to_value(&post).map_err(|e| e.to_string())?;
        let title = string_at(&value, &[&["title"]]).unwrap_or(item.title.clone());
        let author_name = string_at(&value, &[&["user", "name"]])
            .or_else(|| string_at(&payload, &[&["originalData", "user", "name"]]))
            .unwrap_or_else(|| "unknown".to_string());
        let author_id = string_at(
            &value,
            &[&["creatorId"], &["creator_id"], &["user", "userId"]],
        )
        .or_else(|| {
            string_at(
                &payload,
                &[
                    &["originalData", "creatorId"],
                    &["originalData", "creator_id"],
                ],
            )
        })
        .unwrap_or_else(|| "0".to_string());
        let tags = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let updated = super::downloader::download_and_save(
            app.clone(),
            value.clone(),
            "fanbox".to_string(),
            source_id.to_string(),
            title,
            author_name,
            author_id,
            string_at(&value, &[&["type"], &["postType"], &["post_type"]])
                .unwrap_or_else(|| "article".to_string()),
            Some(tags),
            string_at(&value, &[&["excerpt"], &["body", "excerpt"]]),
            string_at(&value, &[&["publishedDatetime"], &["published_datetime"]]),
            Some(cookie),
            Some(user_agent),
        )
        .await?;
        Ok(describe_candidate_save(
            &updated,
            previous_length,
            previous_version,
        ))
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
        canceled_status_for, classify_failure, describe_text_change, make_job_id, may_remember,
        FailureKind,
    };
    use std::collections::HashSet;

    /// 基準値を覚えるとは「次はこの値と同じなら本文を見ない」と決めること。
    /// 手元の本文がその更新より古いなら、覚えた瞬間にその改稿は永久に
    /// 拾われなくなる。だから覚えない。
    #[test]
    fn an_update_newer_than_our_copy_is_never_remembered() {
        assert!(!may_remember(
            Some("2026-08-20T10:00:00+09:00"),
            Some("2026-08-19T10:00:00+09:00")
        ));
    }

    /// 本文まで突き合わせたのが更新より後なら、その版を持っていると言える。
    #[test]
    fn an_update_older_than_our_last_deep_check_is_safe_to_remember() {
        assert!(may_remember(
            Some("2026-08-19T10:00:00+09:00"),
            Some("2026-08-20T10:00:00+09:00")
        ));
        assert!(
            may_remember(
                Some("2026-08-20T10:00:00+09:00"),
                Some("2026-08-20T10:00:00+09:00")
            ),
            "同時刻はその版を見たということ"
        );
    }

    /// 時刻の書き方が変わって読めなくなったら、覚えない側に倒す。
    /// 読めないものを「たぶん大丈夫」と扱うと、静かに取りこぼす。
    #[test]
    fn a_timestamp_we_cannot_read_is_treated_as_unverified() {
        assert!(!may_remember(Some("2026-08-20"), Some("2026-08-21T00:00:00Z")));
        assert!(!may_remember(Some("2026-08-20T00:00:00Z"), Some("きのう")));
        assert!(!may_remember(Some("2026-08-20T00:00:00Z"), None));
        assert!(!may_remember(None, Some("2026-08-20T00:00:00Z")));
    }

    /// 手元の保存日時は、小数9桁つきで書かれている。
    ///
    /// これを読み損ねると `may_remember` が常に false を返し、**初めての確認で
    /// ライブラリ全件が「改稿かもしれない」として候補に並ぶ。** 実際に DB に
    /// 入っている書式で固定しておく。
    #[test]
    fn the_shape_our_own_timestamps_are_written_in_is_readable() {
        let downloaded_at = "2026-08-19T14:19:33.782735500+00:00";
        assert!(
            may_remember(Some("2026-07-27T00:02:20+09:00"), Some(downloaded_at)),
            "保存より前の更新は、その版を持っているということ"
        );
        assert!(
            !may_remember(Some("2026-08-20T00:00:00+09:00"), Some(downloaded_at)),
            "保存より後の更新は、持っているとは言えない"
        );
    }

    /// 時差の書き方が違っても、指している瞬間で比べる。
    /// pixiv は詳細で +00:00、一覧で +09:00 を返す。
    #[test]
    fn timestamps_are_compared_as_instants_not_as_text() {
        assert!(may_remember(
            Some("2026-08-20T01:00:00+00:00"),
            Some("2026-08-20T10:00:00+09:00")
        ));
    }

    #[test]
    fn job_ids_are_fixed_width_random_values() {
        let ids = (0..10_000).map(|_| make_job_id()).collect::<HashSet<_>>();
        assert_eq!(ids.len(), 10_000);
        assert!(ids.iter().all(|id| {
            id.len() == 36
                && id.starts_with("job-")
                && id[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
        }));
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
            ("HTTP 404 Not Found", FailureKind::Missing),
            ("この作品は削除されました", FailureKind::Missing),
            ("request timed out", FailureKind::Network),
            ("dns error: failed to lookup address", FailureKind::Network),
            ("HTTP 500 internal server error", FailureKind::Other),
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
            assert_eq!(classify_failure(message), FailureKind::RateLimited, "{message}");
        }
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
            classify_failure("1 asset downloads failed (0 succeeded): Network error: operation timed out"),
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
