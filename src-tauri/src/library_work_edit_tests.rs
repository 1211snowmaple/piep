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

    let image = library
        .store_original(&CaptureSource::Bytes(b"edit image".to_vec()))
        .unwrap();
    let media = vec![EditorMedia {
        id: 7,
        file: image.id.to_string(),
        name: "絵.png".into(),
        mime_type: Some("image/png".into()),
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
