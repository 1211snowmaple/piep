use super::*;
use crate::library::{
    CaptureSource, EditBlock, EditContent, EditMedia, EditPointerChange, EditRequest,
    ResourceScope, WorkSave,
};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-edit-gaps-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn data() -> Value {
    json!({"detail":{"title":"題", "caption":"説明", "tags":[], "user":{"id":"77","name":"前の名"}}, "text":"[uploadedimage:1][pixivimage:9][pixivimage:id=404]"})
}
fn missing() -> Vec<String> {
    ["[uploadedimage:1]", "[pixivimage:9]", "[pixivimage:id=404]"]
        .map(str::to_owned)
        .into()
}
fn save(library: &Library, key: &str, data: &Value) -> crate::library::CaptureResult {
    library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key,
                route: "save",
                data,
                assets: vec![],
            },
            None,
        )
        .unwrap()
}
fn block(
    ordinal: i64,
    kind: &str,
    text: Option<&str>,
    attrs: Value,
    media_slot: Option<&str>,
) -> EditBlock {
    EditBlock {
        id: LibraryId::now_v7(),
        ordinal,
        kind: kind.into(),
        text: text.map(str::to_owned),
        attrs,
        media_slot: media_slot.map(str::to_owned),
    }
}
fn document(library: &Library, bytes: &[u8]) -> EditContent {
    let file = library
        .store_original(&CaptureSource::Bytes(bytes.into()))
        .unwrap();
    EditContent {
        format: "piep-edit-v1".into(),
        title: None,
        provenance: None,
        blocks: vec![
            block(
                0,
                "image",
                None,
                json!({"sourceRef":"[uploadedimage:1]"}),
                Some("replacement"),
            ),
            block(
                1,
                "image",
                None,
                json!({"sourceRef":"[pixivimage:9-1]","missing":true}),
                None,
            ),
            block(
                2,
                "paragraph",
                Some("[pixivimage:id=404]"),
                Value::Null,
                None,
            ),
        ],
        media: vec![EditMedia {
            slot: "replacement".into(),
            file: file.id,
            original_name: "edit.png".into(),
            mime_type: Some("image/png".into()),
        }],
    }
}

#[test]
fn only_an_active_edit_on_the_same_source_revision_changes_the_effective_gaps() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = save(&library, "1", &data());
    let content = document(&library, b"image");
    let draft = library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: first.entity_id,
            user: library.id,
            base: first.snapshot_id,
            parent: None,
            expected: 0,
            content: &content,
            activate: false,
        })
        .unwrap();
    let check = |snapshot| {
        library
            .pixiv_references_left_by_active_edit(first.entity_id, snapshot, missing())
            .unwrap()
    };
    assert_eq!(
        check(first.snapshot_id),
        missing(),
        "drafts must not settle gaps"
    );
    library
        .change_edit_pointers(
            LibraryId::now_v7(),
            first.entity_id,
            library.id,
            1,
            EditPointerChange::Publish,
        )
        .unwrap();
    assert_eq!(
        check(first.snapshot_id),
        missing()[1..],
        "explicit page 1 must stay unresolved like its shorthand"
    );
    let mut supplemented = data();
    supplemented["detail"]["user"]["name"] = json!("新しい名");
    let next = save(&library, "1", &supplemented);
    assert!(!next.created_revision);
    assert_ne!(next.snapshot_id, first.snapshot_id);
    assert_eq!(
        check(next.snapshot_id),
        missing()[1..],
        "supplemented snapshots keep the same source base"
    );
    supplemented["text"] = json!("別の本文[uploadedimage:1][pixivimage:9][pixivimage:id=404]");
    let revised = save(&library, "1", &supplemented);
    assert!(revised.created_revision);
    assert_eq!(
        check(revised.snapshot_id),
        missing(),
        "an older source edit must not settle a new revision"
    );
    let state = library.edit_state(first.entity_id, library.id).unwrap();
    library
        .change_edit_pointers(
            LibraryId::now_v7(),
            first.entity_id,
            library.id,
            state.generation,
            EditPointerChange::Unpublish,
        )
        .unwrap();
    assert_eq!(check(first.snapshot_id), missing());
    let state = library.edit_state(first.entity_id, library.id).unwrap();
    library
        .change_edit_pointers(
            LibraryId::now_v7(),
            first.entity_id,
            library.id,
            state.generation,
            EditPointerChange::Restore { edit: draft },
        )
        .unwrap();
    assert_eq!(check(first.snapshot_id), missing()[1..]);
    let other = save(&library, "2", &data());
    assert!(matches!(
        library.pixiv_references_left_by_active_edit(other.entity_id, first.snapshot_id, missing()),
        Err(Error::ResourceUnavailable)
    ));
    let original = library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    assert!(
        original.bindings.iter().all(|b| b.resource.is_none()),
        "original gaps stay immutable"
    );
    library.verify().unwrap();
}

#[test]
fn removing_a_reference_settles_it_but_an_empty_edit_image_does_not() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = save(&library, "1", &data());
    let mut content = document(&library, b"");
    content.blocks.remove(1);
    library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: first.entity_id,
            user: library.id,
            base: first.snapshot_id,
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap();
    assert_eq!(
        library
            .pixiv_references_left_by_active_edit(first.entity_id, first.snapshot_id, missing())
            .unwrap(),
        vec!["[uploadedimage:1]", "[pixivimage:id=404]"]
    );
    library.verify().unwrap();
}
