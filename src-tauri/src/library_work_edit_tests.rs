use super::*;
use crate::library::{Capture, CaptureSource, EntityKind};
use serde_json::json;
use std::{collections::BTreeMap, fs};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn input(kind: &str, text: Option<&str>, asset: Option<i64>) -> WorkBlockInput {
    WorkBlockInput {
        block_type: kind.into(),
        text: text.map(str::to_owned),
        asset_id: asset,
        attrs_json: None,
    }
}

#[test]
fn a_published_pixiv_edit_settles_only_the_image_hold_and_unpublishing_restores_it() {
    use crate::library::{DiscoveryInput, WorkSave};
    let temp = Temp(std::env::temp_dir().join(format!("piep-edit-audit-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let data = json!({"detail":{"title":"原題"},"text":"本文[uploadedimage:1]"});
    let saved = library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "w",
                route: "save",
                data: &data,
                assets: vec![],
            },
            None,
        )
        .unwrap();
    let work = saved.entity_id;
    let current = || library.discovery("pixiv", "w").unwrap();
    library.audit_pixiv_edit_images(work).unwrap();
    let hold = current().unwrap();
    let draft = library
        .save_editor(
            LibraryId::now_v7(),
            work,
            EditorSave {
                base: saved.snapshot_id,
                expected: 0,
                title: None,
                blocks: vec![input("paragraph", Some("画像を除いた本文"), None)],
                media: vec![],
                publish: false,
            },
        )
        .unwrap();
    library.audit_pixiv_edit_images(work).unwrap();
    assert_eq!(current().unwrap().generation, hold.generation);
    let edit = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            draft.generation,
            EditPointerChange::Publish,
        )
        .unwrap();
    library
        .reconcile_pixiv_image_hold(work, saved.snapshot_id, draft.generation, Some(&hold), None)
        .unwrap();
    assert_eq!(
        current().unwrap().id,
        hold.id,
        "stale edit must not change the candidate"
    );
    library.audit_pixiv_edit_images(work).unwrap();
    assert!(current().is_none());
    let unpublish = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            edit.generation,
            EditPointerChange::Unpublish,
        )
        .unwrap();
    library.audit_pixiv_edit_images(work).unwrap();
    let restored = current().unwrap();
    assert!(restored.generation > hold.generation);
    assert_eq!(restored.hold_kind.as_deref(), Some("incomplete"));
    assert_eq!(library.current_snapshot(work).unwrap(), saved.snapshot_id);
    assert_eq!(
        library
            .pixiv_source_image_gaps(work, saved.snapshot_id)
            .unwrap(),
        ["[uploadedimage:1]"]
    );
    let mixed = json!({"holdKind":"incomplete","holdReason":"画像と作品情報の未取得",
        "missingReferences":["[uploadedimage:1]"],"missingMetadata":["caption"]})
    .to_string();
    library
        .record_discovery(
            LibraryId::now_v7(),
            DiscoveryInput {
                provider: "pixiv",
                key: "w",
                kind: "revision",
                title: "原題",
                payload: &mixed,
                target_type: Some("work"),
            },
            true,
        )
        .unwrap();
    let metadata_hold = current().unwrap();
    library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            unpublish.generation,
            EditPointerChange::Restore {
                edit: edit.active.unwrap(),
            },
        )
        .unwrap();
    library.audit_pixiv_edit_images(work).unwrap();
    assert_eq!(current().unwrap().payload, metadata_hold.payload);
    library
        .decide_discovery(
            LibraryId::now_v7(),
            "pixiv",
            "w",
            Some(metadata_hold.generation),
            true,
        )
        .unwrap();
    let generation = library.edit_state(work, library.id).unwrap().generation;
    library
        .reconcile_pixiv_image_hold(
            work,
            saved.snapshot_id,
            generation,
            Some(&metadata_hold),
            None,
        )
        .unwrap();
    library.audit_pixiv_edit_images(work).unwrap();
    assert_eq!(current().unwrap().decision, "dismissed");
    library.verify().unwrap();
}

#[test]
fn the_editor_opens_the_source_saves_drafts_once_publishes_and_goes_back() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-edit-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let work = library
        .capture(Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: "w".into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: serde_json::to_vec(&json!({ "key": "w" })).unwrap().into(),
            metadata: BTreeMap::from([("title".into(), json!("元の題"))]),
            field_states: BTreeMap::new(),
            body: Some("一行目\n\n二行目".into()),
            media: vec![],
            attachments: vec![],
        })
        .unwrap()
        .entity_id;

    // 編集が無ければ、今の版の本文から段を作る。
    let view = library.editor_view(work).unwrap();
    assert_eq!(view.generation, 0);
    assert!(view.active.is_none() && view.draft.is_none());
    assert_eq!(view.source_title, "元の題");
    let texts: Vec<_> = view.blocks.iter().filter_map(|b| b.text.clone()).collect();
    assert!(texts.iter().any(|t| t.contains("一行目")), "{texts:?}");

    let (image, selection) = library
        .store_pending_image(&CaptureSource::Bytes(b"edit image".to_vec()))
        .unwrap();
    library.reclaim(Default::default()).unwrap();
    let media = vec![EditorMedia {
        id: 7,
        file: image.id.to_string(),
        name: "絵.png".into(),
        mime_type: Some("image/png".into()),
        selection: Some(selection),
    }];
    let save = |expected, title: Option<&str>, publish| EditorSave {
        base: view.base,
        expected,
        title: title.map(str::to_owned),
        blocks: vec![
            input("paragraph", Some("直した本文"), None),
            input("paragraph", Some("   "), None),
            input("image", None, Some(7)),
        ],
        media: media.clone(),
        publish,
    };
    // 取得元と同じ題は持たない。下書きは同じ中身なら二度書かない。
    let drafted = library
        .save_editor(LibraryId::now_v7(), work, save(0, Some("元の題"), false))
        .unwrap();
    assert_eq!(drafted.generation, 1);
    let pending: bool = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_edit_images WHERE id=?1)",
            [selection.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!pending);
    let again = library
        .save_editor(LibraryId::now_v7(), work, save(1, Some(" 元の題 "), false))
        .unwrap();
    assert_eq!(again.generation, 1);
    assert!(library
        .save_editor(LibraryId::now_v7(), work, save(0, None, false))
        .is_err());

    let opened = library.editor_view(work).unwrap();
    assert!(opened.draft.is_some() && opened.active.is_none());
    assert_eq!(opened.draft.as_ref().unwrap().title, None);
    assert_eq!(opened.blocks.len(), 2);
    assert_eq!(opened.blocks[1].asset_id, Some(1));
    assert_eq!(opened.media[0].file, image.id.to_string());

    // 題を変えて反映する。
    let published = library
        .save_editor(LibraryId::now_v7(), work, save(1, Some("新しい題"), true))
        .unwrap();
    assert!(published.active.is_some() && published.draft.is_none());
    assert_eq!(library.work_card(work).unwrap().title, "新しい題");
    let opened = library.editor_view(work).unwrap();
    assert_eq!(
        opened.active.as_ref().unwrap().title.as_deref(),
        Some("新しい題")
    );

    // 反映をやめると取り込んだ本文へ戻る（版は残る）。
    let request = LibraryId::now_v7();
    let back = library
        .change_edit_pointers(
            request,
            work,
            library.id,
            published.generation,
            EditPointerChange::Unpublish,
        )
        .unwrap();
    assert!(back.active.is_none());
    assert_eq!(
        library
            .change_edit_pointers(
                request,
                work,
                library.id,
                published.generation,
                EditPointerChange::Unpublish
            )
            .unwrap(),
        back
    );
    assert_eq!(library.work_card(work).unwrap().title, "元の題");
    // 開き直すと、取り込んだ本文から始まる。
    let reopened = library.editor_view(work).unwrap();
    assert!(reopened.active.is_none() && reopened.draft.is_none());
    let texts: Vec<_> = reopened
        .blocks
        .iter()
        .filter_map(|b| b.text.clone())
        .collect();
    assert!(texts.iter().any(|t| t.contains("一行目")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("直した本文")), "{texts:?}");
    // 戻した版は以前の編集版に載り、もう一度反映すると一覧から外れる。
    let history = library.edit_history(work).unwrap();
    assert_eq!(history.generation, back.generation);
    assert_eq!(history.archived.len(), 1);
    let old = history.archived[0].id;
    assert_eq!(history.archived[0].title.as_deref(), Some("新しい題"));
    let restored = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            back.generation,
            EditPointerChange::Restore { edit: old },
        )
        .unwrap();
    assert_eq!(restored.active, Some(old));
    assert!(library.edit_history(work).unwrap().archived.is_empty());
    assert_eq!(library.work_card(work).unwrap().title, "新しい題");
    // 一覧に無い版は反映し直せない。
    assert!(library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            restored.generation,
            EditPointerChange::Restore { edit: old },
        )
        .is_err());
    // 別の下書きを反映すると、前の版が一覧に載る。一覧から外しても版は残る。
    let drafted = library
        .save_editor(
            LibraryId::now_v7(),
            work,
            EditorSave {
                expected: restored.generation,
                title: None,
                ..save(0, None, true)
            },
        )
        .unwrap();
    let history = library.edit_history(work).unwrap();
    assert_eq!(
        history.archived.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![old]
    );
    let forgot = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            drafted.generation,
            EditPointerChange::Forget { edit: old },
        )
        .unwrap();
    assert_eq!(forgot.generation, drafted.generation + 1);
    assert!(library.edit_history(work).unwrap().archived.is_empty());
    assert!(library.edit(work, old).is_ok());
    let back = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            forgot.generation,
            EditPointerChange::Unpublish,
        )
        .unwrap();
    assert_eq!(library.edit_history(work).unwrap().archived.len(), 1);
    let cleared = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            back.generation,
            EditPointerChange::ForgetAll,
        )
        .unwrap();
    assert!(library.edit_history(work).unwrap().archived.is_empty());
    // 一覧が空なら、全部外しても書かない。
    let same = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            cleared.generation,
            EditPointerChange::ForgetAll,
        )
        .unwrap();
    assert_eq!(same.generation, cleared.generation);
    let back = cleared;
    // 下書きが無ければ反映できない。捨てるのは変わらなければ書かない。
    assert!(library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            back.generation,
            EditPointerChange::Publish
        )
        .is_err());
    let same = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            back.generation,
            EditPointerChange::Discard,
        )
        .unwrap();
    assert_eq!(same.generation, back.generation);
    library.verify().unwrap();
}

#[test]
fn an_older_shelf_lists_the_edits_nothing_points_to_or_builds_on() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-edit-archive-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let work = library
        .capture(Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: "w".into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: serde_json::to_vec(&json!({ "key": "w" })).unwrap().into(),
            metadata: BTreeMap::from([("title".into(), json!("元の題"))]),
            field_states: BTreeMap::new(),
            body: Some("本文".into()),
            media: vec![],
            attachments: vec![],
        })
        .unwrap()
        .entity_id;
    let base = library.editor_view(work).unwrap().base;
    let save = |expected, text: &str, publish| EditorSave {
        base,
        expected,
        title: None,
        blocks: vec![input("paragraph", Some(text), None)],
        media: vec![],
        publish,
    };
    // 一つ目を反映し、二つ目（一つ目を親にした下書き）を反映すると、一つ目は親なので載らない。
    let first = library
        .save_editor(LibraryId::now_v7(), work, save(0, "一", true))
        .unwrap();
    let second = library
        .save_editor(
            LibraryId::now_v7(),
            work,
            save(first.generation, "二", true),
        )
        .unwrap();
    let back = library
        .change_edit_pointers(
            LibraryId::now_v7(),
            work,
            library.id,
            second.generation,
            EditPointerChange::Unpublish,
        )
        .unwrap();
    assert!(back.active.is_none());
    // スキーマ 17 の棚に戻して開き直す。19 の監視カタログも取り除き、実際の旧 DDL を再現する。
    {
        let conn = library.conn.lock().unwrap();
        conn.execute_batch(
            "ALTER TABLE source_revisions ADD COLUMN classification TEXT NOT NULL DEFAULT 'source'
                 CHECK(classification IN ('source','legacy_unclassified'));
             CREATE TABLE migration_records(source_table TEXT, source_key TEXT);
             CREATE TABLE migration_requests(source_table TEXT, source_key TEXT);
             CREATE TABLE migration_fingerprints(source_table TEXT, source_key TEXT);
             CREATE TABLE repair_attempt_markers(entity_id BLOB, snapshot_id BLOB, attempted_at INTEGER);
             CREATE TABLE asset_audit_markers(entity_id BLOB, snapshot_id BLOB);
             ALTER TABLE library_meta ADD COLUMN switched_at INTEGER CHECK(switched_at IS NULL OR switched_at > 0);
             DROP TABLE edit_archive;
             DROP TABLE discovery_state;
             DROP TABLE discovery_summaries;
             DROP TABLE job_history_visibility;
             DROP TABLE execution_logs;
             DROP TABLE incomplete_save_attempts;
             DROP TABLE ai_notes;
             DROP TABLE asset_repair_state;
             DROP INDEX acquisition_attempt_entity;
             DROP TABLE job_import_evidence;
             DROP TABLE execution_items;
             DROP TABLE execution_jobs;
             ALTER TABLE library_meta DROP COLUMN watch_generation;
             CREATE TABLE outbox_v17 (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 request_id BLOB NOT NULL UNIQUE CHECK(length(request_id)=16),
                 entity_id BLOB REFERENCES entities(id), operation TEXT NOT NULL,
                 payload_json TEXT NOT NULL CHECK(json_valid(payload_json)), created_at INTEGER NOT NULL,
                 saved_search_id BLOB, user_id BLOB,
                 FOREIGN KEY(saved_search_id,user_id) REFERENCES saved_searches(id,user_id),
                 CHECK((entity_id IS NOT NULL AND saved_search_id IS NULL AND user_id IS NULL AND operation<>'saved_search')
                    OR (entity_id IS NULL AND saved_search_id IS NOT NULL AND user_id IS NOT NULL AND operation='saved_search'))
             ) STRICT;
             INSERT INTO outbox_v17 SELECT sequence,request_id,entity_id,operation,payload_json,created_at,saved_search_id,user_id FROM outbox;
             DROP TABLE outbox; ALTER TABLE outbox_v17 RENAME TO outbox;
             DROP TABLE library_tasks;
             DROP TABLE pending_edit_images;
             DELETE FROM schema_migrations WHERE version>=18; PRAGMA user_version=17;",
        )
        .unwrap();
    }
    drop(library);
    let library = Library::open(&temp.0).unwrap();
    let history = library.edit_history(work).unwrap();
    assert_eq!(
        history
            .archived
            .iter()
            .map(|e| Some(e.id))
            .collect::<Vec<_>>(),
        vec![second.active]
    );
    library.verify().unwrap();
}
