//! モデルに手伝ってもらう仕事の入口。
//!
//! **どれも、利用者が押したときだけ動く。** 裏で走るものは一つも無く、
//! 設定していなければ画面に出ない。返ってくるのはどれも案であって、
//! 保存するかどうかは別のコマンドで、利用者が決めてから呼ぶ。
//!
//! 本文を送る仕事（あらすじ・前回のあらすじ）は、材料を読む前に許可を確かめる。

use std::sync::Arc;

use tauri::Manager;

use crate::assist::{
    self, AssistEngine, AssistNote, AssistRuntimeProfile, BundleSplit, DiscoveredEngine,
    SearchIntent, TagProposal,
};
use crate::database::queries::{AiNote, TaggedName};
use crate::AppState;

/// 材料を読むところと、外へ送るところを分ける。
///
/// 送っているあいだライブラリの錠を握ったままにしない。手元のモデルでも
/// 数秒はかかるので、その間ほかの操作が止まると使い物にならない。
async fn read_blocking<T, F>(app: &tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<Arc<AppState>>().inner().clone();
    tokio::task::spawn_blocking(move || f(state))
        .await
        .map_err(|e| format!("Database task failed: {e}"))?
}

/// Mutations participate in the same library gate as downloads, restores and
/// archives. This keeps a backup snapshot from observing half of an accepted
/// tag/note change while still releasing the gate during model inference.
async fn write_blocking<T, F>(app: &tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<Arc<AppState>>().inner().clone();
    let _library_write_guard = state.library_gate.clone().write_owned().await;
    tokio::task::spawn_blocking(move || f(state))
        .await
        .map_err(|e| format!("Database task failed: {e}"))?
}

/// この端末で動いている推論サーバーを探す。
#[tauri::command]
pub async fn assist_discover_engines() -> Vec<DiscoveredEngine> {
    assist::discover_engines().await
}

/// ホストとロード済みモデルから、入力サイズと安全な並列数を決める。
#[tauri::command]
pub async fn assist_runtime_profile(engine: AssistEngine) -> Result<AssistRuntimeProfile, String> {
    assist::runtime_profile(&engine).await
}

// ---- タグの補完 -----------------------------------------------------------

/// この作品に足りていないタグを、棚の語彙から挙げてもらう。
///
/// **付けはしない。** 案を返すだけで、採るかどうかは利用者が決める。
#[tauri::command]
pub async fn assist_suggest_tags(
    app: tauri::AppHandle,
    engine: AssistEngine,
    download_id: i64,
) -> Result<Vec<TagProposal>, String> {
    let (work, vocabulary) = read_blocking(&app, move |state| {
        Ok((
            state.db.work_facts(download_id)?,
            state.db.tag_vocabulary()?,
        ))
    })
    .await?;
    assist::suggest_tags(&engine, &work, &vocabulary).await
}

/// 案から選んだタグを、`llm` 印で付ける。
#[tauri::command]
pub async fn assist_accept_tags(
    app: tauri::AppHandle,
    download_id: i64,
    tags: Vec<String>,
) -> Result<Vec<TaggedName>, String> {
    write_blocking(&app, move |state| {
        state.db.add_assisted_tags(download_id, &tags)?;
        // Tantivy の文書はタグも持つ。DB だけ更新すると、再起動や全再構築まで
        // 新しいタグで検索できないため、採用と同じ操作で同期する。
        state.db.reindex_download(download_id)?;
        state.db.work_tags_with_source(download_id)
    })
    .await
}

/// 出どころ付きのタグ一覧。
#[tauri::command]
pub async fn assist_work_tags(
    app: tauri::AppHandle,
    download_id: i64,
) -> Result<Vec<TaggedName>, String> {
    read_blocking(&app, move |state| {
        state.db.work_tags_with_source(download_id)
    })
    .await
}

/// モデルの案から採ったタグを外す。取得元のタグは外せない。
#[tauri::command]
pub async fn assist_remove_tag(
    app: tauri::AppHandle,
    download_id: i64,
    tag: String,
) -> Result<Vec<TaggedName>, String> {
    write_blocking(&app, move |state| {
        state.db.remove_assisted_tag(download_id, &tag)?;
        state.db.reindex_download(download_id)?;
        state.db.work_tags_with_source(download_id)
    })
    .await
}

// ---- 言葉で探す -----------------------------------------------------------

/// 「こういうのが読みたい」を、棚のタグと検索語に翻訳する。
///
/// 検索そのものは piep がやる。ここは**言い換えるだけ**で、
/// 何も検索しないし、何も保存しない。
#[tauri::command]
pub async fn assist_interpret_search(
    app: tauri::AppHandle,
    engine: AssistEngine,
    phrase: String,
) -> Result<SearchIntent, String> {
    let vocabulary = read_blocking(&app, move |state| state.db.tag_vocabulary()).await?;
    assist::interpret_search(&engine, &phrase, &vocabulary).await
}

// ---- 作風のメモ -----------------------------------------------------------

/// この作者の作風を、題名とタグからまとめてもらう。本文は送らない。
#[tauri::command]
pub async fn assist_describe_author(
    app: tauri::AppHandle,
    engine: AssistEngine,
    source: String,
    person_key: String,
) -> Result<AssistNote, String> {
    let key = format!("{source}:{person_key}");
    let (author, works) = read_blocking(&app, {
        let source = source.clone();
        let person_key = person_key.clone();
        move |state| state.db.author_facts(&source, &person_key)
    })
    .await?;
    let note = assist::describe_author(&engine, &author, &works).await?;
    let model = engine.model.clone();
    let text = note.text.clone();
    write_blocking(&app, move |state| {
        state
            .db
            .save_ai_note("person", &key, "style", &text, &model)
    })
    .await?;
    Ok(note)
}

// ---- 束を分ける -----------------------------------------------------------

/// この束を分けたほうがよいか、案を出してもらう。分けはしない。
#[tauri::command]
pub async fn assist_propose_splits(
    app: tauri::AppHandle,
    engine: AssistEngine,
    collection_id: String,
) -> Result<Vec<BundleSplit>, String> {
    let works = read_blocking(&app, move |state| state.db.collection_facts(&collection_id)).await?;
    assist::propose_splits(&engine, &works).await
}

// ---- あらすじ -------------------------------------------------------------

/// 本文から、あとで思い出すためのあらすじを作ってもらう。
///
/// **本文を送るので、許可が要る。** 材料を読む前に確かめる。
#[tauri::command]
pub async fn assist_summarize_work(
    app: tauri::AppHandle,
    engine: AssistEngine,
    download_id: i64,
) -> Result<AssistNote, String> {
    assist::ensure_body_allowed(&engine)?;
    let (work, body) = read_blocking(&app, move |state| {
        state.db.work_facts_with_body(download_id)
    })
    .await?;
    let note = assist::summarize_work(&engine, &work, &body).await?;
    let model = engine.model.clone();
    let text = note.text.clone();
    write_blocking(&app, move |state| {
        state
            .db
            .save_ai_note("work", &download_id.to_string(), "synopsis", &text, &model)
    })
    .await?;
    Ok(note)
}

/// 直前の話の要点を出す。連載の続きを、間を空けて読むときのため。
#[tauri::command]
pub async fn assist_recap_previous(
    app: tauri::AppHandle,
    engine: AssistEngine,
    previous_download_id: i64,
    current_download_id: i64,
) -> Result<AssistNote, String> {
    assist::ensure_body_allowed(&engine)?;
    let (work, body) = read_blocking(&app, move |state| {
        state.db.work_facts_with_body(previous_download_id)
    })
    .await?;
    let note = assist::recap_previous(&engine, &work.title, &body).await?;
    // 覚え書きは**読もうとしている話**にぶら下げる。同じ前の話でも、
    // どの続きを読む前かで欲しいものが変わることがある。
    let key = format!("{current_download_id}:{previous_download_id}");
    let model = engine.model.clone();
    let text = note.text.clone();
    write_blocking(&app, move |state| {
        state.db.save_ai_note("work", &key, "recap", &text, &model)
    })
    .await?;
    Ok(note)
}

// ---- 覚え書きの読み書き ---------------------------------------------------

/// 保存してある覚え書きを読む。**モデルを呼ばない** — 無ければ無いと返す。
#[tauri::command]
pub async fn assist_load_note(
    app: tauri::AppHandle,
    subject_type: String,
    subject_key: String,
    note_kind: String,
) -> Result<Option<AiNote>, String> {
    read_blocking(&app, move |state| {
        state
            .db
            .load_ai_note(&subject_type, &subject_key, &note_kind)
    })
    .await
}

/// 覚え書きを消す。作り直したいときと、要らなくなったとき。
#[tauri::command]
pub async fn assist_delete_note(
    app: tauri::AppHandle,
    subject_type: String,
    subject_key: String,
    note_kind: String,
) -> Result<bool, String> {
    write_blocking(&app, move |state| {
        state
            .db
            .delete_ai_note(&subject_type, &subject_key, &note_kind)
    })
    .await
}
