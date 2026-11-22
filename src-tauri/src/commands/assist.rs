//! モデルに手伝ってもらう仕事の入口。棚の作品（文字列の ID）で行う。
//!
//! **どれも、利用者が押したときだけ動く。** 裏で走るものは一つも無く、
//! モデルを使う仕事は設定が必要である。棚の併用候補と作者メモは通信しない。
//! タグは利用者が選んだときだけ追加する。生成メモは覚え書きとして保存する。
//!
//! 本文を送る仕事（あらすじ・前回のあらすじ・校正）は、材料を読む前に許可を確かめる。
//! 材料を読むところと、モデルへ送るところは分ける（送っているあいだ棚を握らない）。

use std::sync::Arc;

use serde::Serialize;
use tauri::{Emitter, State};

use crate::assist::{
    self, AssistEngine, AssistNote, AssistRuntimeProfile, BundleSplit, DiscoveredEngine,
    GeneratedAssist, ProofreadSegment, ProofreadSuggestion, SearchIntent, TagProposal,
    FEATURE_AUTHOR_STYLE, FEATURE_COLLECTION_SPLIT, FEATURE_EDITOR_PROOFREAD,
    FEATURE_NATURAL_LANGUAGE_SEARCH, FEATURE_READER_RECAP, FEATURE_TAG_SUGGESTION,
    FEATURE_WORK_SYNOPSIS,
};
use crate::commands::shelf::{collection_target, Shelf};
use crate::database::queries::{AiNote, RelatedTagCandidate, TaggedName};
use crate::library::{Library, LibraryId, TagProvenance};

async fn blocking<T, F>(state: &State<'_, Arc<Shelf>>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<Shelf>) -> Result<T, String> + Send + 'static,
{
    let state = Arc::clone(state);
    tauri::async_runtime::spawn_blocking(move || f(state))
        .await
        .map_err(|e| e.to_string())?
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

/// 編集中の文章を校正してもらう。直しはしない。
///
/// 材料は保存した本文ではなく、編集画面がいま持っている文字。保存前の
/// 書きかけも校正できるようにするため。**本文を送るので、許可が要る。**
#[tauri::command]
pub async fn assist_proofread(
    engine: AssistEngine,
    segments: Vec<ProofreadSegment>,
) -> Result<GeneratedAssist<Vec<ProofreadSuggestion>>, String> {
    assist::ensure_body_allowed(&engine)?;
    let provenance = assist::generation_provenance(&engine, FEATURE_EDITOR_PROOFREAD, &segments)?;
    let value = assist::proofread(&engine, &segments).await?;
    Ok(GeneratedAssist { value, provenance })
}

/// 前回のあらすじは**読もうとしている話**にぶら下げる（`読む話:前の話`）。
fn recap_note_key(current: LibraryId, previous: LibraryId) -> String {
    format!("{current}:{previous}")
}

fn compare_note_config(note: &mut AiNote, engine: &AssistEngine) -> Result<(), String> {
    let current = assist::generation_config_fingerprint(engine, &note.feature_id)?;
    note.config_stale = Some(current != note.config_fingerprint);
    Ok(())
}

/// タグを変えたあと、検索の索引を追いつかせて今のタグを返す。棚の変更は済んでいるので、
/// 索引の失敗はその旨を添えて伝える。
fn finish_tag_change(
    state: &Shelf,
    library: &Arc<Library>,
    work: LibraryId,
    changed: bool,
    operation: &str,
) -> Result<Vec<TaggedName>, String> {
    if changed {
        state.forget_derived();
        library.sync_search_index(&mut |_, _| {}).map_err(|error| {
            format!("タグは{operation}済みですが、検索用データの更新に失敗しました。{error}")
        })?;
        super::library_health::schedule_semantic_sync(Arc::clone(library));
    }
    library
        .work_tags_with_source(work)
        .map_err(|e| e.to_string())
}

/// モデルの道具が読む棚。読み取りの接続だけを使う。
struct ShelfAssist(Arc<Library>);

impl assist::ShelfLookup for ShelfAssist {
    fn tags_containing(
        &self,
        text: &str,
        limit: usize,
    ) -> Result<Vec<assist::ShelfTagCount>, String> {
        self.0
            .assist_tags_containing(text, limit)
            .map_err(|e| e.to_string())
    }

    fn existing_tags(&self, names: &[String]) -> Result<Vec<String>, String> {
        self.0
            .assist_existing_tags(names)
            .map_err(|e| e.to_string())
    }

    fn count_works(
        &self,
        include: &[String],
        or_mode: bool,
        exclude: &[String],
    ) -> Result<i64, String> {
        self.0
            .assist_count_works(include, or_mode, exclude)
            .map_err(|e| e.to_string())
    }

    fn tag_profile(&self, name: &str) -> Result<Option<assist::ShelfTagProfile>, String> {
        self.0.assist_tag_profile(name).map_err(|e| e.to_string())
    }

    fn tag_pairs(
        &self,
        name: &str,
        others: &[String],
    ) -> Result<Option<assist::ShelfTagPairs>, String> {
        self.0
            .assist_tag_pairs(name, others)
            .map_err(|e| e.to_string())
    }
}

// ---- タグ ---------------------------------------------------------------------

/// 棚の取得元タグの併用から候補を出す。モデルも通信も使わない。
#[tauri::command]
pub async fn shelf_assist_related_tags(
    state: State<'_, Arc<Shelf>>,
    work: LibraryId,
) -> Result<Vec<RelatedTagCandidate>, String> {
    blocking(&state, move |state| {
        state
            .library()?
            .related_tag_candidates(work)
            .map_err(|e| e.to_string())
    })
    .await
}

/// 選んだ併用候補を、自分で付けたタグとして足す。棚にある名前だけ。
#[tauri::command]
pub async fn shelf_assist_accept_related_tags(
    state: State<'_, Arc<Shelf>>,
    work: LibraryId,
    tags: Vec<String>,
) -> Result<Vec<TaggedName>, String> {
    blocking(&state, move |state| {
        let library = state.library()?;
        let added = library
            .add_user_tags(work, &tags, TagProvenance::Manual, true)
            .map_err(|e| e.to_string())?;
        finish_tag_change(&state, &library, work, !added.is_empty(), "保存")
    })
    .await
}

/// 自分で付けたタグだけを外す。
#[tauri::command]
pub async fn shelf_assist_remove_related_tag(
    state: State<'_, Arc<Shelf>>,
    work: LibraryId,
    tag: String,
) -> Result<Vec<TaggedName>, String> {
    blocking(&state, move |state| {
        let library = state.library()?;
        let removed = library
            .remove_user_tag(work, &tag, TagProvenance::Manual)
            .map_err(|e| e.to_string())?;
        finish_tag_change(&state, &library, work, removed, "削除")
    })
    .await
}

/// この作品に足りていないタグを、棚の語彙から挙げてもらう。**付けはしない。**
#[tauri::command]
pub async fn shelf_assist_suggest_tags(
    state: State<'_, Arc<Shelf>>,
    engine: AssistEngine,
    work: LibraryId,
) -> Result<GeneratedAssist<Vec<TagProposal>>, String> {
    let (library, facts, vocabulary) = blocking(&state, move |state| {
        let library = state.library()?;
        let facts = library.assist_work_facts(work).map_err(|e| e.to_string())?;
        let vocabulary = library.assist_tag_vocabulary().map_err(|e| e.to_string())?;
        Ok((library, facts, vocabulary))
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_TAG_SUGGESTION,
        &serde_json::json!({ "work": &facts, "vocabulary": &vocabulary }),
    )?;
    let shelf: Arc<dyn assist::ShelfLookup> = Arc::new(ShelfAssist(library));
    let value = assist::suggest_tags(&engine, &facts, &vocabulary, Some(shelf)).await?;
    Ok(GeneratedAssist { value, provenance })
}

/// 案から選んだタグを、「モデルの案を採った」印で付ける。
#[tauri::command]
pub async fn shelf_assist_accept_tags(
    state: State<'_, Arc<Shelf>>,
    work: LibraryId,
    tags: Vec<String>,
) -> Result<Vec<TaggedName>, String> {
    blocking(&state, move |state| {
        let library = state.library()?;
        let added = library
            .add_user_tags(work, &tags, TagProvenance::AcceptedModel, false)
            .map_err(|e| e.to_string())?;
        finish_tag_change(&state, &library, work, !added.is_empty(), "保存")
    })
    .await
}

/// 出どころ付きのタグ一覧。
#[tauri::command]
pub async fn shelf_assist_work_tags(
    state: State<'_, Arc<Shelf>>,
    work: LibraryId,
) -> Result<Vec<TaggedName>, String> {
    blocking(&state, move |state| {
        state
            .library()?
            .work_tags_with_source(work)
            .map_err(|e| e.to_string())
    })
    .await
}

/// モデルの案から採ったタグを外す。取得元のタグは外せない。
#[tauri::command]
pub async fn shelf_assist_remove_tag(
    state: State<'_, Arc<Shelf>>,
    work: LibraryId,
    tag: String,
) -> Result<Vec<TaggedName>, String> {
    blocking(&state, move |state| {
        let library = state.library()?;
        let removed = library
            .remove_user_tag(work, &tag, TagProvenance::AcceptedModel)
            .map_err(|e| e.to_string())?;
        finish_tag_change(&state, &library, work, removed, "削除")
    })
    .await
}

// ---- 言葉で探す ---------------------------------------------------------------

/// 「こういうのが読みたい」を、棚のタグと検索語に言い換える。検索も保存もしない。
#[tauri::command]
pub async fn shelf_assist_interpret_search(
    state: State<'_, Arc<Shelf>>,
    engine: AssistEngine,
    phrase: String,
) -> Result<GeneratedAssist<SearchIntent>, String> {
    let (library, vocabulary) = blocking(&state, move |state| {
        let library = state.library()?;
        let vocabulary = library.assist_tag_vocabulary().map_err(|e| e.to_string())?;
        Ok((library, vocabulary))
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_NATURAL_LANGUAGE_SEARCH,
        &serde_json::json!({ "phrase": &phrase, "vocabulary": &vocabulary }),
    )?;
    let shelf: Arc<dyn assist::ShelfLookup> = Arc::new(ShelfAssist(library));
    let value = assist::interpret_search(&engine, &phrase, &vocabulary, Some(shelf)).await?;
    Ok(GeneratedAssist { value, provenance })
}

// ---- 作風のメモ・分け方の案 -----------------------------------------------------

/// この作者の作風を、題名とタグからまとめてもらう。本文は送らない。
/// 覚え書きは棚にだけ残す（読むだけの棚では見せるだけ）。
#[tauri::command]
pub async fn shelf_assist_describe_author(
    state: State<'_, Arc<Shelf>>,
    engine: AssistEngine,
    provider: String,
    person_key: String,
) -> Result<GeneratedAssist<AssistNote>, String> {
    let key = format!("{provider}:{person_key}");
    let (author, works) = blocking(&state, move |state| {
        state
            .library()?
            .assist_author_facts(&provider, &person_key)
            .map_err(|e| e.to_string())
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_AUTHOR_STYLE,
        &serde_json::json!({ "author": &author, "works": &works }),
    )?;
    let value = assist::describe_author(&engine, &author, &works).await?;
    save_note(
        &state,
        "person",
        key,
        "style",
        value.clone(),
        provenance.clone(),
    )
    .await?;
    Ok(GeneratedAssist { value, provenance })
}

/// このコレクションを分けたほうがよいか、案を出してもらう。分けはしない。
#[tauri::command]
pub async fn shelf_assist_propose_splits(
    state: State<'_, Arc<Shelf>>,
    engine: AssistEngine,
    collection: String,
) -> Result<GeneratedAssist<Vec<BundleSplit>>, String> {
    let works = blocking(&state, move |state| {
        let library = state.library()?;
        let id = collection_target(&collection)?;
        library
            .assist_collection_facts(id)
            .map_err(|e| e.to_string())
    })
    .await?;
    let provenance = assist::generation_provenance(&engine, FEATURE_COLLECTION_SPLIT, &works)?;
    let value = assist::propose_splits(&engine, &works).await?;
    Ok(GeneratedAssist { value, provenance })
}

// ---- あらすじ ---------------------------------------------------------------

/// 進み具合のイベント。作品は棚の ID（文字列）で送る。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShelfProgressEvent {
    feature: &'static str,
    download_id: String,
    #[serde(flatten)]
    progress: assist::AssistProgress,
}

fn progress_sink(
    app: &tauri::AppHandle,
    feature: &'static str,
    work: LibraryId,
) -> impl Fn(assist::AssistProgress) + Send + Sync {
    let app = app.clone();
    let download_id = work.to_string();
    move |progress| {
        let _ = app.emit(
            "assist-progress",
            ShelfProgressEvent {
                feature,
                download_id: download_id.clone(),
                progress,
            },
        );
    }
}

/// 覚え書きを残す。
async fn save_note(
    state: &State<'_, Arc<Shelf>>,
    subject_type: &'static str,
    subject_key: String,
    note_kind: &'static str,
    note: AssistNote,
    provenance: assist::AssistProvenance,
) -> Result<(), String> {
    blocking(state, move |state| {
        let library = state.library()?;
        library
            .save_ai_note(subject_type, &subject_key, note_kind, &note, &provenance)
            .map_err(|e| e.to_string())
    })
    .await
}

/// 本文から、あとで思い出すためのあらすじを作ってもらう。**本文を送るので、許可が要る。**
#[tauri::command]
pub async fn shelf_assist_summarize_work(
    app: tauri::AppHandle,
    state: State<'_, Arc<Shelf>>,
    engine: AssistEngine,
    work: LibraryId,
) -> Result<GeneratedAssist<AssistNote>, String> {
    assist::ensure_body_allowed(&engine)?;
    let (facts, body) = blocking(&state, move |state| {
        state
            .library()?
            .assist_work_facts_with_body(work)
            .map_err(|e| e.to_string())
    })
    .await?;
    let mut provenance = assist::generation_provenance(
        &engine,
        FEATURE_WORK_SYNOPSIS,
        &serde_json::json!({ "work": &facts, "body": &body }),
    )?;
    let progress = progress_sink(&app, FEATURE_WORK_SYNOPSIS, work);
    let value = assist::summarize_work(&engine, &facts, &body, &progress).await?;
    provenance.prompt_version =
        assist::prompt_version_for_note(FEATURE_WORK_SYNOPSIS, &value).to_string();
    save_note(
        &state,
        "work",
        work.to_string(),
        "synopsis",
        value.clone(),
        provenance.clone(),
    )
    .await?;
    Ok(GeneratedAssist { value, provenance })
}

/// 直前の話の要点を出す。連載の続きを、間を空けて読むときのため。
#[tauri::command]
pub async fn shelf_assist_recap_previous(
    app: tauri::AppHandle,
    state: State<'_, Arc<Shelf>>,
    engine: AssistEngine,
    previous: LibraryId,
    current: LibraryId,
) -> Result<GeneratedAssist<AssistNote>, String> {
    assist::ensure_body_allowed(&engine)?;
    let (facts, body, memory) = blocking(&state, move |state| {
        let library = state.library()?;
        let (facts, body) = library
            .assist_work_facts_with_body(previous)
            .map_err(|e| e.to_string())?;
        let memory = library
            .recap_memory_before(previous)
            .map_err(|e| e.to_string())?;
        Ok((facts, body, memory))
    })
    .await?;
    // 材料の指紋は、読み出すときに確かめられるもの（前の話の材料）だけで作る。前回までの
    // 覚え書きは文の作り方を助けるだけで、変わっても作り直しを促さない。
    let mut provenance = assist::generation_provenance(
        &engine,
        FEATURE_READER_RECAP,
        &serde_json::json!({
            "previousWork": previous.to_string(),
            "currentWork": current.to_string(),
            "work": &facts,
            "body": &body
        }),
    )?;
    let progress = progress_sink(&app, FEATURE_READER_RECAP, current);
    let value =
        assist::recap_previous(&engine, &facts.title, &body, memory.as_deref(), &progress).await?;
    provenance.prompt_version =
        assist::prompt_version_for_note(FEATURE_READER_RECAP, &value).to_string();
    save_note(
        &state,
        "work",
        recap_note_key(current, previous),
        "recap",
        value.clone(),
        provenance.clone(),
    )
    .await?;
    Ok(GeneratedAssist { value, provenance })
}

// ---- 覚え書きの読み書き ---------------------------------------------------------

/// 保存してある覚え書きを読む。**モデルを呼ばない** — 無ければ無いと返す。
#[tauri::command]
pub async fn shelf_assist_load_note(
    state: State<'_, Arc<Shelf>>,
    subject_type: String,
    subject_key: String,
    note_kind: String,
    engine: Option<AssistEngine>,
) -> Result<Option<AiNote>, String> {
    let mut note = blocking(&state, move |state| {
        state
            .library()?
            .load_ai_note(&subject_type, &subject_key, &note_kind)
            .map_err(|e| e.to_string())
    })
    .await?;
    if let (Some(note), Some(engine)) = (note.as_mut(), engine.as_ref()) {
        compare_note_config(note, engine)?;
    }
    Ok(note)
}

/// 覚え書きを消す。
#[tauri::command]
pub async fn shelf_assist_delete_note(
    state: State<'_, Arc<Shelf>>,
    subject_type: String,
    subject_key: String,
    note_kind: String,
) -> Result<bool, String> {
    blocking(&state, move |state| {
        state
            .library()?
            .delete_ai_note(&subject_type, &subject_key, &note_kind)
            .map_err(|e| e.to_string())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::recap_note_key;
    use crate::library::LibraryId;

    #[test]
    fn recap_is_owned_by_the_chapter_being_opened() {
        let (a, b) = (LibraryId::now_v7(), LibraryId::now_v7());
        assert_eq!(recap_note_key(b, a), format!("{b}:{a}"));
        assert_ne!(recap_note_key(b, a), recap_note_key(a, b));
    }
}
