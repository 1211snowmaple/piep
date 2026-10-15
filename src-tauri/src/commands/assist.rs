//! モデルに手伝ってもらう仕事の入口。
//!
//! **どれも、利用者が押したときだけ動く。** 裏で走るものは一つも無く、
//! モデルを使う仕事は設定が必要である。棚の併用候補と作者メモは通信しない。
//! タグは利用者が選んだときだけ追加する。生成メモは覚え書きとして保存する。
//!
//! 本文を送る仕事（あらすじ・前回のあらすじ）は、材料を読む前に許可を確かめる。

use std::sync::Arc;

use tauri::Manager;

use crate::assist::{
    self, AssistEngine, AssistNote, AssistRuntimeProfile, BundleSplit, DiscoveredEngine,
    GeneratedAssist, SearchIntent, TagProposal, FEATURE_AUTHOR_STYLE, FEATURE_COLLECTION_SPLIT,
    FEATURE_NATURAL_LANGUAGE_SEARCH, FEATURE_READER_RECAP, FEATURE_TAG_SUGGESTION,
    FEATURE_WORK_SYNOPSIS,
};
use crate::database::queries::{AiNote, Database, RelatedTagCandidate, TaggedName};
use crate::AppState;

fn recap_note_key(current_download_id: i64, previous_download_id: i64) -> String {
    format!("{current_download_id}:{previous_download_id}")
}

fn compare_note_config(note: &mut AiNote, engine: &AssistEngine) -> Result<(), String> {
    let current = assist::generation_config_fingerprint(engine, &note.feature_id)?;
    note.config_stale = Some(current != note.config_fingerprint);
    Ok(())
}

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

/// DBへの変更と索引の同期は別段階なので、保存済みの変更を失敗時にも伝える。
fn finish_tag_mutation(
    db: &Database,
    download_id: i64,
    changed: bool,
    operation: &str,
) -> Result<Vec<TaggedName>, String> {
    let state = if changed {
        format!("タグは{operation}済みですが")
    } else {
        "タグの変更はありませんが".to_string()
    };
    // 変更がなくても、前回失敗した索引同期を再試行できるようにする。
    db.reindex_download(download_id)
        .map_err(|error| format!("{state}、検索用データの更新に失敗しました。{error}"))?;
    db.work_tags_with_source(download_id)
        .map_err(|error| format!("{state}、現在のタグ一覧を取得できません。{error}"))
}

/// 棚の取得元タグの併用件数から候補を出す。モデルや通信は使わない。
#[tauri::command]
pub async fn assist_related_tags(
    app: tauri::AppHandle,
    download_id: i64,
) -> Result<Vec<RelatedTagCandidate>, String> {
    read_blocking(&app, move |state| {
        state.db.related_tag_candidates(download_id)
    })
    .await
}

/// 利用者が選んだ併用候補を、手動追加の印で記録する。
#[tauri::command]
pub async fn assist_accept_related_tags(
    app: tauri::AppHandle,
    download_id: i64,
    tags: Vec<String>,
) -> Result<Vec<TaggedName>, String> {
    write_blocking(&app, move |state| {
        let added = state.db.add_related_tags(download_id, &tags)?;
        finish_tag_mutation(&state.db, download_id, !added.is_empty(), "保存")
    })
    .await
}

/// 手動追加されたタグだけを外す。
#[tauri::command]
pub async fn assist_remove_related_tag(
    app: tauri::AppHandle,
    download_id: i64,
    tag: String,
) -> Result<Vec<TaggedName>, String> {
    write_blocking(&app, move |state| {
        let removed = state.db.remove_related_tag(download_id, &tag)?;
        finish_tag_mutation(&state.db, download_id, removed, "削除")
    })
    .await
}

/// この作品に足りていないタグを、棚の語彙から挙げてもらう。
///
/// **付けはしない。** 案を返すだけで、採るかどうかは利用者が決める。
#[tauri::command]
pub async fn assist_suggest_tags(
    app: tauri::AppHandle,
    engine: AssistEngine,
    download_id: i64,
) -> Result<GeneratedAssist<Vec<TagProposal>>, String> {
    let (work, vocabulary) = read_blocking(&app, move |state| {
        Ok((
            state.db.work_facts(download_id)?,
            state.db.tag_vocabulary()?,
        ))
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_TAG_SUGGESTION,
        &serde_json::json!({ "work": &work, "vocabulary": &vocabulary }),
    )?;
    let value = assist::suggest_tags(&engine, &work, &vocabulary).await?;
    Ok(GeneratedAssist { value, provenance })
}

/// 案から選んだタグを、`llm` 印で付ける。
#[tauri::command]
pub async fn assist_accept_tags(
    app: tauri::AppHandle,
    download_id: i64,
    tags: Vec<String>,
) -> Result<Vec<TaggedName>, String> {
    write_blocking(&app, move |state| {
        let added = state.db.add_assisted_tags(download_id, &tags)?;
        // Tantivy の文書はタグも持つ。DB だけ更新すると、再起動や全再構築まで
        // 新しいタグで検索できないため、採用と同じ操作で同期する。
        finish_tag_mutation(&state.db, download_id, !added.is_empty(), "保存")
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
        let removed = state.db.remove_assisted_tag(download_id, &tag)?;
        finish_tag_mutation(&state.db, download_id, removed, "削除")
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
) -> Result<GeneratedAssist<SearchIntent>, String> {
    let vocabulary = read_blocking(&app, move |state| state.db.tag_vocabulary()).await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_NATURAL_LANGUAGE_SEARCH,
        &serde_json::json!({ "phrase": &phrase, "vocabulary": &vocabulary }),
    )?;
    let value = assist::interpret_search(&engine, &phrase, &vocabulary).await?;
    Ok(GeneratedAssist { value, provenance })
}

// ---- 作風のメモ -----------------------------------------------------------

/// この作者の作風を、題名とタグからまとめてもらう。本文は送らない。
#[tauri::command]
pub async fn assist_describe_author(
    app: tauri::AppHandle,
    engine: AssistEngine,
    source: String,
    person_key: String,
) -> Result<GeneratedAssist<AssistNote>, String> {
    let key = format!("{source}:{person_key}");
    let (author, works) = read_blocking(&app, {
        let source = source.clone();
        let person_key = person_key.clone();
        move |state| state.db.author_facts(&source, &person_key)
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_AUTHOR_STYLE,
        &serde_json::json!({ "author": &author, "works": &works }),
    )?;
    let value = assist::describe_author(&engine, &author, &works).await?;
    let saved_provenance = provenance.clone();
    let text = value.text.clone();
    write_blocking(&app, move |state| {
        state
            .db
            .save_ai_note_with_provenance("person", &key, "style", &text, &saved_provenance)
    })
    .await?;
    Ok(GeneratedAssist { value, provenance })
}

// ---- 束を分ける -----------------------------------------------------------

/// この束を分けたほうがよいか、案を出してもらう。分けはしない。
#[tauri::command]
pub async fn assist_propose_splits(
    app: tauri::AppHandle,
    engine: AssistEngine,
    collection_id: String,
) -> Result<GeneratedAssist<Vec<BundleSplit>>, String> {
    let works = read_blocking(&app, move |state| state.db.collection_facts(&collection_id)).await?;
    let provenance = assist::generation_provenance(&engine, FEATURE_COLLECTION_SPLIT, &works)?;
    let value = assist::propose_splits(&engine, &works).await?;
    Ok(GeneratedAssist { value, provenance })
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
) -> Result<GeneratedAssist<AssistNote>, String> {
    assist::ensure_body_allowed(&engine)?;
    let (work, body) = read_blocking(&app, move |state| {
        state.db.work_facts_with_body(download_id)
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_WORK_SYNOPSIS,
        &serde_json::json!({ "work": &work, "body": &body }),
    )?;
    let value = assist::summarize_work(&engine, &work, &body).await?;
    let saved_provenance = provenance.clone();
    let note = value.clone();
    write_blocking(&app, move |state| {
        state.db.save_ai_note_content_with_provenance(
            "work",
            &download_id.to_string(),
            "synopsis",
            &note,
            &saved_provenance,
        )
    })
    .await?;
    Ok(GeneratedAssist { value, provenance })
}

/// 直前の話の要点を出す。連載の続きを、間を空けて読むときのため。
#[tauri::command]
pub async fn assist_recap_previous(
    app: tauri::AppHandle,
    engine: AssistEngine,
    previous_download_id: i64,
    current_download_id: i64,
) -> Result<GeneratedAssist<AssistNote>, String> {
    assist::ensure_body_allowed(&engine)?;
    let (work, body) = read_blocking(&app, move |state| {
        state.db.work_facts_with_body(previous_download_id)
    })
    .await?;
    let provenance = assist::generation_provenance(
        &engine,
        FEATURE_READER_RECAP,
        &serde_json::json!({
            "previousDownloadId": previous_download_id,
            "currentDownloadId": current_download_id,
            "work": &work,
            "body": &body
        }),
    )?;
    let value = assist::recap_previous(&engine, &work.title, &body).await?;
    // 覚え書きは**読もうとしている話**にぶら下げる。同じ前の話でも、
    // どの続きを読む前かで欲しいものが変わることがある。
    let key = recap_note_key(current_download_id, previous_download_id);
    let saved_provenance = provenance.clone();
    let note = value.clone();
    write_blocking(&app, move |state| {
        state.db.save_ai_note_content_with_provenance(
            "work",
            &key,
            "recap",
            &note,
            &saved_provenance,
        )
    })
    .await?;
    Ok(GeneratedAssist { value, provenance })
}

#[cfg(test)]
mod tests {
    use super::{compare_note_config, finish_tag_mutation, recap_note_key};
    use crate::assist::{
        generation_config_fingerprint, AssistEngine, AssistFeatureProfile, AssistInputPolicy,
        FEATURE_WORK_SYNOPSIS,
    };
    use crate::database::queries::{AiNote, Database};

    #[test]
    fn tag_mutations_report_saved_state_and_retry_index_sync() {
        struct TestRoot(std::path::PathBuf);
        impl Drop for TestRoot {
            fn drop(&mut self) {
                let Ok(target) = self.0.canonicalize() else {
                    return;
                };
                let Ok(temp) = std::env::temp_dir().canonicalize() else {
                    return;
                };
                // この試験で新規作成したtemp直下のディレクトリだけを片付ける。
                if target.parent() == Some(temp.as_path())
                    && target.file_name().is_some_and(|name| {
                        name.to_string_lossy().starts_with("piep-assist-tag-sync-")
                    })
                {
                    let _ = std::fs::remove_dir_all(target);
                }
            }
        }
        let root = TestRoot(std::env::temp_dir().join(format!(
            "piep-assist-tag-sync-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        )));
        std::fs::create_dir(&root.0).unwrap();
        let path = root.0.join("piep.db");
        let db = Database::open(&path, &root.0.join("storage")).unwrap();
        db.set_semantic_index_enabled(false).unwrap();
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "INSERT INTO downloads(id, source, source_id, title, author_name, author_id,
                                   content_type, json_path, downloaded_at)
             VALUES (1, 'pixiv', 'synthetic-tag-sync', '灯台', '作者', 'test-author',
                     'synthetic', 'missing-synthetic-body', '2026-01-01');
             INSERT INTO tags(name) VALUES ('推理');
             CREATE TRIGGER reject_test_index_marker BEFORE INSERT ON search_index_state
             BEGIN SELECT RAISE(ABORT, 'injected index-state failure'); END;",
        )
        .unwrap();

        assert_eq!(db.add_related_tags(1, &["推理".into()]).unwrap(), ["推理"]);
        let error = finish_tag_mutation(&db, 1, true, "保存").unwrap_err();
        assert!(error.contains("タグは保存済み"), "{error}");
        assert!(error.contains("injected index-state failure"), "{error}");
        assert_eq!(db.work_tags_with_source(1).unwrap()[0].source, "manual");

        conn.execute_batch("DROP TRIGGER reject_test_index_marker")
            .unwrap();
        // 同じタグの再採用がno-opでも、索引同期をやり直せる。
        assert!(db.add_related_tags(1, &["推理".into()]).unwrap().is_empty());
        assert_eq!(finish_tag_mutation(&db, 1, false, "保存").unwrap().len(), 1);
        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM search_index_state WHERE download_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);

        conn.execute_batch(
            "CREATE TRIGGER reject_test_index_marker BEFORE INSERT ON search_index_state
             BEGIN SELECT RAISE(ABORT, 'injected index-state failure'); END;",
        )
        .unwrap();
        assert!(db.remove_related_tag(1, "推理").unwrap());
        let error = finish_tag_mutation(&db, 1, true, "削除").unwrap_err();
        assert!(error.contains("タグは削除済み"), "{error}");
        assert!(db.work_tags_with_source(1).unwrap().is_empty());
    }

    #[test]
    fn recap_is_owned_by_the_chapter_being_opened() {
        assert_eq!(recap_note_key(20, 19), "20:19");
        assert_ne!(recap_note_key(20, 19), recap_note_key(19, 20));
    }

    #[test]
    fn saved_note_flags_changed_model_settings_without_contacting_the_server() {
        let engine = AssistEngine {
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: "model-a".into(),
            remote_consent_url: None,
            allow_body: false,
            feature_profile: None,
        };
        let mut note = AiNote {
            text: "memo".into(),
            source_tail: None,
            model_id: "model-a".into(),
            feature_id: FEATURE_WORK_SYNOPSIS.into(),
            prompt_version: "work_synopsis/v2".into(),
            input_fingerprint: String::new(),
            config_fingerprint: generation_config_fingerprint(&engine, FEATURE_WORK_SYNOPSIS)
                .unwrap(),
            created_at: String::new(),
            prompt_stale: false,
            input_stale: false,
            config_stale: None,
        };
        compare_note_config(&mut note, &engine).unwrap();
        assert_eq!(note.config_stale, Some(false));
        let changed = AssistEngine {
            model: "model-b".into(),
            ..engine.clone()
        };
        compare_note_config(&mut note, &changed).unwrap();
        assert_eq!(note.config_stale, Some(true));

        let changed_instructions = AssistEngine {
            feature_profile: Some(AssistFeatureProfile {
                profile_id: "default:work_synopsis".into(),
                feature_id: FEATURE_WORK_SYNOPSIS.into(),
                additional_instructions: "短く書く".into(),
                tag_criteria: String::new(),
                input_policy: AssistInputPolicy::default(),
            }),
            ..engine
        };
        compare_note_config(&mut note, &changed_instructions).unwrap();
        assert_eq!(note.config_stale, Some(true));
    }
}

// ---- 覚え書きの読み書き ---------------------------------------------------

/// 保存してある覚え書きを読む。**モデルを呼ばない** — 無ければ無いと返す。
#[tauri::command]
pub async fn assist_load_note(
    app: tauri::AppHandle,
    subject_type: String,
    subject_key: String,
    note_kind: String,
    engine: Option<AssistEngine>,
) -> Result<Option<AiNote>, String> {
    let mut note = read_blocking(&app, move |state| {
        state
            .db
            .load_ai_note(&subject_type, &subject_key, &note_kind)
    })
    .await?;
    if let (Some(note), Some(engine)) = (note.as_mut(), engine.as_ref()) {
        compare_note_config(note, engine)?;
    }
    Ok(note)
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
