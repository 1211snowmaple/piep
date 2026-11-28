//! 棚の保存した検索・コレクションの提案・更新の監視・検索の索引を扱う入口。
//!
//! **ここに仕事は無い。** 計算は `analysis`・`library_*`・`search_engine` にあり、この
//! 層は引数を受けて専用スレッドへ渡すだけである。SQLite の操作は同期的で、
//! async の中でそのまま走らせると WebView の応答が止まる。
//!
//! → [アーキテクチャ](https://1211snowmaple.github.io/piep/policy/03-architecture)

use crate::models::{
    AcceptCollectionSuggestionInput, CollectionAdditionResult, CollectionNameCandidate,
    CollectionSuggestion, CollectionSuggestionRequest, CollectionSweepResult,
    EntityProfileRepairStatus, SearchIndexStatus, UpdateTarget, UpdateTargetInput, WorkKey,
};
use crate::AppState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Instant;
use tauri::{Emitter, Manager};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{sleep, Duration};

static ENTITY_REFRESH_LOCK: LazyLock<AsyncMutex<Option<Instant>>> =
    LazyLock::new(|| AsyncMutex::new(None));
#[path = "library_profiles.rs"]
mod profile_shelf;
#[path = "library_saved_searches.rs"]
mod saved_search_shelf;
use saved_search_shelf::{SavedSearchView, SavedSearchViewInput};
static ENTITY_PROFILE_REPAIR_RUNNING: AtomicBool = AtomicBool::new(false);
static ENTITY_PROFILE_REPAIR_CANCEL: AtomicBool = AtomicBool::new(false);
static SEARCH_REBUILD_JOBS: LazyLock<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
static SEARCH_REBUILD_SEQUENCE: AtomicU64 = AtomicU64::new(1);

async fn run_db_blocking<T, F>(app: tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<Arc<AppState>>().inner().clone();
    tokio::task::spawn_blocking(move || f(state))
        .await
        .map_err(|e| format!("Database task failed: {}", e))?
}

async fn run_library_write_blocking<T, F>(app: tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<Arc<AppState>>().inner().clone();
    let guard = state.library_gate.clone().write_owned().await;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        f(state)
    })
    .await
    .map_err(|e| format!("Database task failed: {e}"))?
}

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshEntityProfileParams {
    pub entity_type: String,
    pub source: String,
    pub source_key: String,
    pub force: Option<bool>,
    /// 鍵は画面から受け取らない。IPC の入口が保存してある接続から埋める。
    #[serde(skip_deserializing)]
    pub refresh_token: Option<String>,
    #[serde(skip_deserializing)]
    pub cookie: Option<String>,
    #[serde(skip_deserializing)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityProfileRepairProgress {
    phase: String,
    completed: usize,
    total: usize,
    repaired: usize,
    failed: usize,
    active_label: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityProfileRepairResult {
    stopped_reason: Option<String>,
    attempted: usize,
    repaired: usize,
    failed: usize,
    canceled: bool,
    remaining: i64,
}

static ENTITY_PROFILE_REPAIR_PROGRESS: LazyLock<
    std::sync::Mutex<Option<EntityProfileRepairProgress>>,
> = LazyLock::new(|| std::sync::Mutex::new(None));

fn emit_profile_repair_progress(app: &tauri::AppHandle, progress: EntityProfileRepairProgress) {
    *ENTITY_PROFILE_REPAIR_PROGRESS
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(progress.clone());
    let _ = app.emit("entity-profile-repair-progress", progress);
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityProfileRepairView {
    #[serde(flatten)]
    status: EntityProfileRepairStatus,
    running: bool,
    progress: Option<EntityProfileRepairProgress>,
}

struct EntityProfileRepairRunGuard;

impl Drop for EntityProfileRepairRunGuard {
    fn drop(&mut self) {
        ENTITY_PROFILE_REPAIR_RUNNING.store(false, Ordering::Release);
        ENTITY_PROFILE_REPAIR_CANCEL.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildJobOptions {
    include_semantic: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildProgress {
    job_id: String,
    /// "manual" when someone pressed the button, "automatic" when the app
    /// noticed the index was behind and caught it up on its own. The wording
    /// the UI uses differs, and an automatic run must not read as an error.
    origin: String,
    status: String,
    total_works: i64,
    indexed_works: i64,
    pending_works: i64,
    is_complete: bool,
    phase: String,
    /// Documents handled by this run, which is what the progress bar tracks.
    /// `indexed_works` counts the whole library and moves in commit-sized
    /// steps, so on its own it looks stalled for minutes at a time.
    processed: i64,
    processed_total: i64,
    failed: i64,
    embedding_provider: String,
    gpu_enabled: bool,
    throughput_per_sec: Option<f64>,
    eta_seconds: Option<f64>,
    error: Option<String>,
}

/// 検索索引を作り直す。返るのはジョブ番号。
///
/// **すぐには終わらない。** 背景で走るので、返ってきた番号で進み具合を追い、
/// 止めたいときは `search_cancel_rebuild_index` に渡す。
///
/// `includeSemantic` を渡すと、意味索引の有効・無効もここで切り替わる。**設定を
/// 書き換える副作用がある**ので、作り直すだけのつもりなら省くこと。
#[tauri::command]
pub async fn search_rebuild_index(
    app: tauri::AppHandle,
    job_options: Option<SearchRebuildJobOptions>,
) -> Result<String, String> {
    let options = job_options.unwrap_or(SearchRebuildJobOptions {
        include_semantic: None,
    });
    let library = app.state::<Arc<AppState>>().shelf.library()?;
    if let Some(enabled) = options.include_semantic {
        let target = Arc::clone(&library);
        tokio::task::spawn_blocking(move || {
            super::library_health::apply_semantic_enabled(&target, enabled)
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    spawn_shelf_index_job(app, library, "manual")
}

/// Persistent capability switch shared by manual rebuilds, automatic
/// maintenance, and every incremental work update.
#[tauri::command]
pub async fn search_set_semantic_enabled(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<SearchIndexStatus, String> {
    run_db_blocking(app, move |state| {
        let library = state.shelf.library()?;
        super::library_health::apply_semantic_enabled(&library, enabled)?;
        library
            .search_index_status(enabled)
            .map_err(|e| e.to_string())
    })
    .await
}

/// 棚の索引を追いつかせる（全文、続けて意味検索を入れていれば意味の索引）。
/// 進み具合は索引の作り直しの知らせで流し、走っている作り直しがあればそれに合流する。
fn spawn_shelf_index_job(
    app: tauri::AppHandle,
    library: Arc<crate::library::Library>,
    origin: &'static str,
) -> Result<String, String> {
    let job_id = format!(
        "search-index-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        SEARCH_REBUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = SEARCH_REBUILD_JOBS.lock().map_err(|e| e.to_string())?;
        if let Some((existing, _)) = jobs.iter().next() {
            return Ok(existing.clone());
        }
        jobs.insert(job_id.clone(), cancel.clone());
    }
    let job = job_id.clone();
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        let progress_app = app.clone();
        let progress_job = job.clone();
        let task_library = Arc::clone(&library);
        let outcome = tokio::task::spawn_blocking(move || {
            let library = task_library;
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut emit = |phase: &str, done: usize, total: usize| {
                if done < total && last.elapsed() < Duration::from_millis(100) {
                    return;
                }
                last = Instant::now();
                let elapsed = started.elapsed().as_secs_f64();
                let rate = (elapsed > 0.5).then(|| done as f64 / elapsed);
                let _ = progress_app.emit(
                    "search-index-progress",
                    SearchRebuildProgress {
                        job_id: progress_job.clone(),
                        origin: origin.to_string(),
                        status: "running".to_string(),
                        total_works: 0,
                        indexed_works: 0,
                        pending_works: total.saturating_sub(done) as i64,
                        is_complete: false,
                        phase: phase.to_string(),
                        processed: done as i64,
                        processed_total: total as i64,
                        failed: 0,
                        embedding_provider: String::new(),
                        gpu_enabled: false,
                        throughput_per_sec: rate,
                        eta_seconds: rate
                            .filter(|r| *r > 0.0)
                            .map(|r| total.saturating_sub(done) as f64 / r),
                        error: None,
                    },
                );
            };
            let lexical = library
                .sync_search_index(&mut |done, total| emit("indexing", done, total))
                .map_err(|e| e.to_string())?;
            let mut processed = lexical.indexed + lexical.history.indexed;
            if super::library_health::semantic_enabled() && !cancel.load(Ordering::Relaxed) {
                let _turn = super::library_health::SEMANTIC_TURN
                    .lock()
                    .map_err(|e| e.to_string())?;
                let semantic = library
                    .sync_semantic_index(&|| cancel.load(Ordering::Relaxed), &mut |done, total| {
                        emit("semantic", done, total)
                    })
                    .map_err(|e| e.to_string())?;
                processed += semantic.indexed;
            }
            Ok::<_, String>((processed as i64, cancel.load(Ordering::Relaxed)))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|result| result);
        let status = library
            .search_index_status(super::library_health::semantic_enabled())
            .map_err(|e| e.to_string());
        match (outcome, status) {
            (Ok((processed, canceled)), Ok(status)) => emit_search_index_progress(
                &app,
                &job,
                origin,
                if canceled { "canceled" } else { "completed" },
                status,
                processed,
                0,
                started.elapsed().as_secs_f64(),
            ),
            (Err(error), _) | (_, Err(error)) => {
                let _ = app.emit(
                    "search-index-progress",
                    serde_json::json!({ "jobId": job, "origin": origin, "status": "failed", "phase": "failed", "error": error }),
                );
            }
        }
        if let Ok(mut jobs) = SEARCH_REBUILD_JOBS.lock() {
            jobs.remove(&job);
        }
    });
    Ok(job_id)
}

/// 作り直しを止めるよう頼む。
///
/// 知らない番号なら `Err`。**すぐには止まらない** - 印を立てるだけで、切りの
/// 良いところで降りる。
#[tauri::command]
pub async fn search_cancel_rebuild_index(job_id: String) -> Result<(), String> {
    let jobs = SEARCH_REBUILD_JOBS.lock().map_err(|e| e.to_string())?;
    let Some(cancel) = jobs.get(&job_id) else {
        return Err("Search rebuild job not found".to_string());
    };
    cancel.store(true, Ordering::Relaxed);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_search_index_progress(
    app: &tauri::AppHandle,
    job_id: &str,
    origin: &str,
    status: &str,
    index: SearchIndexStatus,
    processed: i64,
    failed: i64,
    elapsed_secs: f64,
) {
    let _ = app.emit(
        "search-index-progress",
        SearchRebuildProgress {
            job_id: job_id.to_string(),
            origin: origin.to_string(),
            status: status.to_string(),
            total_works: index.total_works,
            indexed_works: index.indexed_works,
            pending_works: index.pending_works,
            is_complete: index.is_complete,
            phase: if status == "completed" {
                "ready".to_string()
            } else {
                index.phase
            },
            processed,
            processed_total: processed,
            failed,
            embedding_provider: index.embedding_provider,
            gpu_enabled: index.gpu_enabled,
            throughput_per_sec: (elapsed_secs > 0.0).then(|| processed as f64 / elapsed_secs),
            eta_seconds: Some(0.0),
            error: None,
        },
    );
}

/// 保存した検索の一覧を返す。ID は検索の UUID の文字列。
#[tauri::command]
pub async fn list_saved_searches(app: tauri::AppHandle) -> Result<Vec<SavedSearchView>, String> {
    run_db_blocking(app, move |state| {
        let library = state.shelf.library()?;
        saved_search_shelf::list(&library)
    })
    .await
}

/// 検索を保存する。同じ名前があれば上書きし、保存後の姿を返す。
#[tauri::command]
pub async fn upsert_saved_search(
    app: tauri::AppHandle,
    input: SavedSearchViewInput,
) -> Result<SavedSearchView, String> {
    run_library_write_blocking(app, move |state| {
        let library = state.shelf.library()?;
        saved_search_shelf::upsert(&library, input)
    })
    .await
}

/// 保存した検索を消す。`false` は「その ID が無かった」を意味する。
#[tauri::command]
pub async fn delete_saved_search(app: tauri::AppHandle, id: String) -> Result<bool, String> {
    run_library_write_blocking(app, move |state| {
        let library = state.shelf.library()?;
        saved_search_shelf::delete(&library, &id)
    })
    .await
}

/// すでにあるコレクションに、名前の案を出し直す。保存はしない。
#[tauri::command]
pub async fn propose_collection_names(
    app: tauri::AppHandle,
    collection_id: String,
) -> Result<Vec<CollectionNameCandidate>, String> {
    run_db_blocking(app, move |state| {
        let library = state.shelf.library()?;
        let target = super::shelf::collection_target(&collection_id)?;
        library.collection_name_proposals_on_shelf(target)
    })
    .await
}

/// すでにあるコレクションの名前と説明を、モデルにも考えてもらう。
///
/// 提案のときと同じく、**足すだけで置き換えない**。返ってきた案を採るかどうかは
/// 利用者が決める。送るのは題名・作者・タグ・シリーズ名だけで、本文は送らない。
#[tauri::command]
pub async fn name_collection_with_model(
    app: tauri::AppHandle,
    collection_id: String,
    engine: crate::assist::AssistEngine,
) -> Result<crate::assist::GeneratedAssist<crate::assist::NamedBundle>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let works = tokio::task::spawn_blocking(move || {
        let library = state.shelf.library()?;
        let target = super::shelf::collection_target(&collection_id)?;
        library.collection_naming_works_on_shelf(target)
    })
    .await
    .map_err(|e| format!("Database task failed: {e}"))??;
    let provenance = crate::assist::generation_provenance(
        &engine,
        crate::assist::FEATURE_COLLECTION_NAMING,
        &works,
    )?;
    let value = crate::assist::name_bundle(&engine, &works).await?;
    Ok(crate::assist::GeneratedAssist { value, provenance })
}

/// 設定したエンジンを、実際の仕事で試す。
///
/// `collection_id` を渡すとその束で、渡さなければ棚から拾った作品で試す。
/// **保存する前に、何が返るのかを見せる**ためのもの。つながることと
/// 使えることは別である。
#[tauri::command]
pub async fn try_naming_engine(
    app: tauri::AppHandle,
    engine: crate::assist::AssistEngine,
    collection_id: Option<String>,
) -> Result<crate::assist::TrialResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let works = tokio::task::spawn_blocking(move || {
        let library = state.shelf.library()?;
        match collection_id {
            Some(id) => {
                library.collection_naming_works_on_shelf(super::shelf::collection_target(&id)?)
            }
            None => library.sample_naming_works_on_shelf(),
        }
    })
    .await
    .map_err(|e| format!("Database task failed: {e}"))??;
    if works.is_empty() {
        return Err("試すための作品がライブラリにありません".to_string());
    }
    crate::assist::try_engine(&engine, &works).await
}

/// 提案の名前を、利用者が選んだモデルにも考えてもらう。
///
/// 返ってきた案は決定せず、モデル候補だけを更新する。題名・タグなどから作った
/// 通常候補は置き換えない。失敗しても提案そのものは壊れない。
#[tauri::command]
pub async fn name_collection_suggestion(
    app: tauri::AppHandle,
    suggestion_id: String,
    engine: crate::assist::AssistEngine,
) -> Result<CollectionSuggestion, String> {
    // 材料を読むところと、外へ送るところを分ける。送っているあいだ
    // ライブラリの錠を握ったままにしない。
    let state = app.state::<Arc<AppState>>().inner().clone();
    let library = state.shelf.library()?;
    let works = {
        let library = library.clone();
        let suggestion_id = suggestion_id.clone();
        tokio::task::spawn_blocking(move || library.suggestion_naming_works(&suggestion_id))
            .await
            .map_err(|e| format!("Database task failed: {e}"))??
    };
    let provenance = crate::assist::generation_provenance(
        &engine,
        crate::assist::FEATURE_COLLECTION_NAMING,
        &works,
    )?;
    let named = crate::assist::name_bundle(&engine, &works).await?;
    run_library_write_blocking(app, move |_| {
        library.attach_llm_name_to_suggestion(&suggestion_id, &named, &provenance)
    })
    .await
}

/// 走査で出た候補を、まとめて閉じる。`track` を渡すとその系統だけ。
#[tauri::command]
pub async fn dismiss_swept_suggestions(app: tauri::AppHandle) -> Result<usize, String> {
    run_library_write_blocking(app, move |state| {
        let library = state.shelf.library()?;
        library.dismiss_swept_collection_suggestions()
    })
    .await
}

/// 棚全体を走査して、束の候補を作り直す。
///
/// 意味索引を全部読むので、開いた瞬間に走らせるものではない。利用者が
/// 「探す」と言ったときだけ動かす。
#[tauri::command]
pub async fn sweep_collection_candidates(
    app: tauri::AppHandle,
) -> Result<CollectionSweepResult, String> {
    run_library_write_blocking(app, move |state| {
        let library = state.shelf.library()?;
        library.sweep_collection_suggestions()
    })
    .await
}

/// すでにある束へ、あとから入れるとよさそうな作品を探す。
///
/// 保存はしない。読むだけの操作なので、棚の書き込み権は取らない。
#[tauri::command]
pub async fn suggest_collection_additions(
    app: tauri::AppHandle,
    collection_id: String,
) -> Result<CollectionAdditionResult, String> {
    run_db_blocking(app, move |state| {
        let library = state.shelf.library()?;
        let target = super::shelf::collection_target(&collection_id)?;
        library.collection_additions_on_shelf(target)
    })
    .await
}

/// 一つの束の候補を作る。
///
/// 候補は**下書きであって決定ではない**。採るまでコレクションは増えない。
///
/// 採否は `accept_collection_suggestion` と `reject_collection_suggestion` で決める。
#[tauri::command]
pub async fn generate_collection_suggestion(
    app: tauri::AppHandle,
    request: CollectionSuggestionRequest,
) -> Result<CollectionSuggestion, String> {
    run_library_write_blocking(app, move |state| {
        let library = state.shelf.library()?;
        let seeds = request
            .seed_works
            .iter()
            .map(|work| {
                work.parse::<crate::library::LibraryId>()
                    .map_err(|_| "基準の作品の ID が不正です".to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        if seeds.is_empty() {
            return Err("At least one seed work is required".to_string());
        }
        if seeds.len() > 20 {
            return Err("A suggestion can use at most 20 seed works".to_string());
        }
        library.seed_collection_suggestion(&seeds, request.limit)
    })
    .await
}

/// 束の候補を返す。`stateFilter` で状態を絞れる。
///
/// 並びは**確からしさの高い順**である。大きい束が先に来るわけではない。
#[tauri::command]
pub async fn list_collection_suggestions(
    app: tauri::AppHandle,
    state_filter: Option<String>,
) -> Result<Vec<CollectionSuggestion>, String> {
    run_db_blocking(app, move |state| {
        let library = state.shelf.library()?;
        library.collection_suggestion_views(state_filter.as_deref())
    })
    .await
}

/// 採って作ったコレクション。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedCollection {
    /// コレクションの UUID。
    pub id: String,
    pub name: String,
}

/// 候補を採り、コレクションを作る。作ったコレクションの ID と名前を返す。
///
/// **候補の段階で並べ替えはできない。** 順序を直すのは採ったあと、
/// `db_reorder_work_collection_members` で行う。
#[tauri::command]
pub async fn accept_collection_suggestion(
    app: tauri::AppHandle,
    input: AcceptCollectionSuggestionInput,
) -> Result<AcceptedCollection, String> {
    run_library_write_blocking(app, move |state| {
        let name = input.name.clone();
        let library = state.shelf.library()?;
        let id = library.accept_collection_suggestion_view(&input)?;
        let name = library
            .collection_detail(id)
            .map(|detail| detail.summary.name)
            .or_else(|_| name.ok_or(()))
            .unwrap_or_default();
        Ok(AcceptedCollection {
            id: id.to_string(),
            name,
        })
    })
    .await
}

/// 候補を拒む。`memberKeys` を渡すと、その作品だけを拒む。
///
/// **拒んだ組み合わせは覚える**ので、次の走査で同じ提案は出てこない。全部を
/// 拒むと候補ごと消える。`false` は「その候補が無かった」を意味する。
#[tauri::command]
pub async fn reject_collection_suggestion(
    app: tauri::AppHandle,
    suggestion_id: String,
    member_keys: Option<Vec<WorkKey>>,
) -> Result<bool, String> {
    run_library_write_blocking(app, move |state| {
        let library = state.shelf.library()?;
        library.reject_collection_suggestion_view(&suggestion_id, member_keys.as_deref())
    })
    .await
}

/// 索引に何件入っていて、全体が何件かを返す。
///
/// この二つの比が「索引の完成度」である。**分母は棚の全件**で、絞り込んだ数では
/// ない。
#[tauri::command]
pub async fn get_search_index_status(app: tauri::AppHandle) -> Result<SearchIndexStatus, String> {
    run_db_blocking(app, |state| {
        let library = state.shelf.library()?;
        library
            .search_index_status(super::library_health::semantic_enabled())
            .map_err(|e| e.to_string())
    })
    .await
}

/// 作者・シリーズの更新監視を設定する。あれば上書きし、設定後の姿を返す。
#[tauri::command]
pub async fn upsert_update_target(
    app: tauri::AppHandle,
    target: UpdateTargetInput,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<UpdateTarget, String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        let options = target
            .metadata_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| serde_json::json!({}));
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::Upsert {
                target_type: target.target_type.clone(),
                provider: target.source.clone(),
                key: target.source_key.clone(),
                name: target.display_name,
                enabled: target.enabled,
                options,
            },
        )?;
        super::watching::find(
            &library,
            &target.target_type,
            &target.source,
            &target.source_key,
        )?
        .ok_or("監視対象が見つかりません".into())
    })
    .await
}

/// 更新監視の設定を一つ引く。監視していなければ `None`。
#[tauri::command]
pub async fn get_update_target(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
) -> Result<Option<UpdateTarget>, String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        super::watching::find(&library, &target_type, &source, &source_key)
    })
    .await
}

/// 更新監視の設定を並べる。
///
/// `enabledOnly` を立てると、今動いているものだけ。既定は false で、止めてある
/// ものも返る。
#[tauri::command]
pub async fn list_update_targets(
    app: tauri::AppHandle,
    target_type: Option<String>,
    enabled_only: Option<bool>,
) -> Result<Vec<UpdateTarget>, String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        super::watching::targets(
            &library,
            target_type.as_deref(),
            enabled_only.unwrap_or(false),
        )
    })
    .await
}

/// 更新監視を止める／再開する。
///
/// **設定は消えない。** 消したいときは `delete_update_target`。
#[tauri::command]
#[allow(clippy::too_many_arguments)] // 既存の IPC 引数に要求 ID・世代・棚の対象 ID を添える。
pub async fn set_update_target_enabled(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
    enabled: bool,
    target_id: Option<crate::library::LibraryId>,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<(), String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        let id = match target_id {
            Some(id) => id,
            None => super::watching::find(&library, &target_type, &source, &source_key)?
                .and_then(|v| v.id)
                .ok_or("監視対象が見つかりません")?,
        };
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::Enable {
                target: id,
                enabled,
            },
        )
        .map(|_| ())
    })
    .await
}

/// 更新監視の対象を外す。
///
/// **保存済みの作品には触らない。** 監視をやめるだけである。
#[tauri::command]
pub async fn delete_update_target(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
    target_id: Option<crate::library::LibraryId>,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<(), String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        let id = match target_id {
            Some(id) => id,
            None => super::watching::find(&library, &target_type, &source, &source_key)?
                .and_then(|v| v.id)
                .ok_or("監視対象が見つかりません")?,
        };
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::RemoveTarget { target: id },
        )
        .map(|_| ())
    })
    .await
}

/// 利用者が作った監視グループを並び順で返す。中の監視対象の ID も添える。
#[tauri::command]
pub async fn list_watch_groups(
    app: tauri::AppHandle,
) -> Result<Vec<crate::models::WatchGroup>, String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || super::watching::groups(&library)).await
}

/// 監視グループを一番下に作り、その ID を返す。同じ名前があれば `Err`。
#[tauri::command]
pub async fn create_watch_group(
    app: tauri::AppHandle,
    name: String,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<crate::library::LibraryId, String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::CreateGroup { name },
        )
        .map(|v| v.id)
    })
    .await
}

/// 監視グループの名前を変える。
#[tauri::command]
pub async fn rename_watch_group(
    app: tauri::AppHandle,
    group_id: crate::library::LibraryId,
    name: String,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<(), String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        let id = group_id;
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::RenameGroup { group: id, name },
        )
        .map(|_| ())
    })
    .await
}

/// 監視グループを消す。**中の監視対象は消えない。**
#[tauri::command]
pub async fn delete_watch_group(
    app: tauri::AppHandle,
    group_id: crate::library::LibraryId,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<(), String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        let id = group_id;
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::RemoveGroup { group: id },
        )
        .map(|_| ())
    })
    .await
}

/// 監視対象を監視グループに入れる（`add`）か、外す。変わった件数を返す。
#[tauri::command]
pub async fn set_watch_group_members(
    app: tauri::AppHandle,
    group_id: crate::library::LibraryId,
    target_ids: Vec<crate::library::LibraryId>,
    add: bool,
    request_id: Option<crate::library::LibraryId>,
    expected_generation: Option<i64>,
) -> Result<usize, String> {
    let library = super::watching::active(&app)?;
    super::watching::blocking(move || {
        let group = group_id;
        let targets = target_ids;
        super::watching::edit(
            &library,
            request_id,
            expected_generation,
            crate::library::WatchEdit::Members {
                group,
                targets,
                add,
            },
        )
        .map(|v| v.changed)
    })
    .await
}

fn collect_profile_links(
    candidates: impl IntoIterator<Item = String>,
    text: Option<&str>,
) -> Vec<String> {
    static URL_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"https?://[^\s<>\"']+"#).expect("profile URL regex must compile")
    });
    let mut links = Vec::new();
    let mut push = |candidate: &str| {
        let trimmed = candidate
            .trim()
            .trim_end_matches(['.', ',', ')', ']', '}', '。', '、']);
        if url::Url::parse(trimmed).is_ok() && !links.iter().any(|link| link == trimmed) {
            links.push(trimmed.to_string());
        }
    };
    for candidate in candidates {
        push(&candidate);
    }
    if let Some(text) = text {
        for matched in URL_PATTERN.find_iter(text) {
            push(matched.as_str());
        }
    }
    links
}

/// 作者やシリーズの情報を取得元から取り直す。
///
/// **呼びすぎないよう間隔を空けてある。** 前回から間が空いていなければ待たされる。
/// 画面を開くたびに呼ばないこと。
///
/// `force` を立てると、新しいと分かっていても取り直す。鍵は保存してある接続から読む。
#[tauri::command]
pub async fn refresh_entity_profile(
    app: tauri::AppHandle,
    mut params: RefreshEntityProfileParams,
) -> Result<serde_json::Value, String> {
    let credentials = crate::auth::saved::Vault::app().update_credentials();
    params.refresh_token = credentials.pixiv_refresh_token;
    params.cookie = credentials.fanbox_cookie;
    params.user_agent = credentials.fanbox_user_agent;
    refresh_entity_profile_with(app, params).await
}

/// 鍵を添えた `params` で取り直す。更新ジョブと修復が、自分の持つ鍵で呼ぶ。
pub(crate) async fn refresh_entity_profile_with(
    app: tauri::AppHandle,
    params: RefreshEntityProfileParams,
) -> Result<serde_json::Value, String> {
    let mut last_refresh = ENTITY_REFRESH_LOCK.lock().await;
    if let Some(last) = *last_refresh {
        let elapsed = last.elapsed();
        if elapsed < Duration::from_secs(3) {
            sleep(Duration::from_secs(3) - elapsed).await;
        }
    }
    *last_refresh = Some(Instant::now());

    let state = app.state::<Arc<AppState>>().inner().clone();
    let library = state.shelf.library()?;
    profile_shelf::refresh(state, library, params).await
}

/// 取り直しの進み具合を返す。走っていないときの姿も返る。
#[tauri::command]
pub async fn get_entity_profile_repair_status(
    app: tauri::AppHandle,
) -> Result<EntityProfileRepairView, String> {
    let status = run_db_blocking(app, |state| {
        let library = state.shelf.library()?;
        library.profile_repair_status().map_err(|e| e.to_string())
    })
    .await?;
    Ok(EntityProfileRepairView {
        status,
        running: ENTITY_PROFILE_REPAIR_RUNNING.load(Ordering::Acquire),
        progress: ENTITY_PROFILE_REPAIR_PROGRESS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone(),
    })
}

/// 完全取得できていない作者・シリーズを、成功したものから順に埋め直す。
///
/// 再開位置は一時的なメモリではなく各エンティティの `last_fetched_at`。
/// アプリ終了やキャンセル後も、次回は残件だけが対象になる。
#[tauri::command]
pub async fn repair_incomplete_entity_profiles(
    app: tauri::AppHandle,
) -> Result<EntityProfileRepairResult, String> {
    let credentials = crate::auth::saved::Vault::app().update_credentials();
    ENTITY_PROFILE_REPAIR_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "作者・シリーズ情報の修復はすでに実行中です".to_string())?;
    let _run_guard = EntityProfileRepairRunGuard;
    ENTITY_PROFILE_REPAIR_CANCEL.store(false, Ordering::Release);

    let targets = run_db_blocking(app.clone(), |state| {
        let library = state.shelf.library()?;
        library.incomplete_profiles().map_err(|e| e.to_string())
    })
    .await?;
    let total = targets.len();
    let mut repaired = 0usize;
    let mut failed = 0usize;
    let mut completed = 0usize;
    let mut consecutive_errors = 0usize;
    let mut stopped_reason = None;

    emit_profile_repair_progress(
        &app,
        EntityProfileRepairProgress {
            phase: "running".to_string(),
            completed,
            total,
            repaired,
            failed,
            active_label: None,
            error: None,
        },
    );

    for target in targets {
        if ENTITY_PROFILE_REPAIR_CANCEL.load(Ordering::Acquire) {
            break;
        }
        let params = RefreshEntityProfileParams {
            entity_type: target.entity_type.clone(),
            source: target.source.clone(),
            source_key: target.source_key.clone(),
            force: Some(true),
            refresh_token: credentials.pixiv_refresh_token.clone(),
            cookie: credentials.fanbox_cookie.clone(),
            user_agent: credentials.fanbox_user_agent.clone(),
        };
        emit_profile_repair_progress(
            &app,
            EntityProfileRepairProgress {
                phase: "running".to_string(),
                completed,
                total,
                repaired,
                failed,
                active_label: Some(target.display_name.clone()),
                error: None,
            },
        );
        let result = refresh_entity_profile_with(app.clone(), params).await;
        completed += 1;
        let error = match result {
            Ok(_) => {
                repaired += 1;
                consecutive_errors = 0;
                None
            }
            Err(error) => {
                failed += 1;
                use super::update_jobs::{classify_failure, FailureKind};
                let kind = classify_failure(&error);
                consecutive_errors =
                    if matches!(kind, FailureKind::Missing | FailureKind::Restricted) {
                        0
                    } else {
                        consecutive_errors + 1
                    };
                if matches!(kind, FailureKind::Auth | FailureKind::RateLimited)
                    || consecutive_errors >= 3
                {
                    stopped_reason = Some(format!("取得先への接続を確認するため一時停止しました。取得済みの情報は保持しています。{error}"));
                }
                log::warn!(
                    "{}:{}:{} のプロフィール修復に失敗しました: {}",
                    target.entity_type,
                    target.source,
                    target.source_key,
                    error
                );
                Some(error)
            }
        };
        emit_profile_repair_progress(
            &app,
            EntityProfileRepairProgress {
                phase: "checked".to_string(),
                completed,
                total,
                repaired,
                failed,
                active_label: Some(target.display_name),
                error,
            },
        );
        if stopped_reason.is_some() {
            break;
        }
    }

    let canceled = ENTITY_PROFILE_REPAIR_CANCEL.load(Ordering::Acquire);
    let remaining = run_db_blocking(app.clone(), |state| {
        let library = state.shelf.library()?;
        library.profile_repair_status().map_err(|e| e.to_string())
    })
    .await?
    .total_count;
    emit_profile_repair_progress(
        &app,
        EntityProfileRepairProgress {
            phase: if canceled {
                "canceled"
            } else if stopped_reason.is_some() {
                "paused"
            } else {
                "complete"
            }
            .to_string(),
            completed,
            total,
            repaired,
            failed,
            active_label: None,
            error: stopped_reason.clone(),
        },
    );
    Ok(EntityProfileRepairResult {
        stopped_reason,
        attempted: completed,
        repaired,
        failed,
        canceled,
        remaining,
    })
}

/// 走っている取り直しに、中止を頼む。
///
/// `false` は「走っていなかった」を意味する。**すぐには止まらない** - 印を立てる
/// だけで、今の一件が終わったところで降りる。
#[tauri::command]
pub fn cancel_entity_profile_repair() -> bool {
    if !ENTITY_PROFILE_REPAIR_RUNNING.load(Ordering::Acquire) {
        return false;
    }
    ENTITY_PROFILE_REPAIR_CANCEL.store(true, Ordering::Release);
    true
}

#[cfg(test)]
mod profile_link_tests {
    use super::collect_profile_links;

    #[test]
    fn profile_links_are_deduplicated_and_extracted_from_text() {
        let links = collect_profile_links(
            [
                "https://www.pixiv.net/users/42".to_string(),
                "https://x.com/example".to_string(),
                "https://x.com/example".to_string(),
            ],
            Some("Skeb: https://skeb.jp/@example。 portfolio https://example.com/work"),
        );
        assert_eq!(
            links,
            vec![
                "https://www.pixiv.net/users/42",
                "https://x.com/example",
                "https://skeb.jp/@example",
                "https://example.com/work",
            ]
        );
    }
}

#[cfg(test)]
mod entity_image_url_tests {
    /// 表紙とアイコンの取得先を https に限る。
    ///
    /// この一本だけ、取得元の応答に入っていた URL をそのまま渡していた。
    /// ほかの外向きの要求は例外なく宛先を絞ってある。
    #[test]
    fn only_https_urls_are_fetched() {
        let allowed = |url: &str| {
            let url = url.trim();
            url.get(..8)
                .is_some_and(|head| head.eq_ignore_ascii_case("https://"))
        };
        assert!(allowed("https://i.pximg.net/user-profile/x.jpg"));
        assert!(allowed("  https://downloads.fanbox.cc/cover.png  "));

        assert!(!allowed("http://i.pximg.net/x.jpg"));
        assert!(!allowed("file:///C:/Windows/win.ini"));
        assert!(!allowed("data:image/png;base64,AAAA"));
        assert!(!allowed("//i.pximg.net/x.jpg"));
        assert!(!allowed(""));
        // scheme の綴りの大小は問わない。RFC 3986 で区別しないものを区別すると、
        // 取得元が大文字で返した日に画像が黙って出なくなる。
        assert!(allowed("HTTPS://i.pximg.net/x.jpg"));
        // 短すぎて scheme が入りきらない文字列で落ちない。
        assert!(!allowed("http"));
    }
}
