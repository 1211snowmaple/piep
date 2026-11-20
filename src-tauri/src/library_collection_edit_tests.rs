use super::*;
use crate::library::{Capture, EntityKind};
use serde_json::json;
use std::{collections::BTreeMap, fs};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn work(library: &Library, key: &str, title: &str, created: &str) -> LibraryId {
    library
        .capture(Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: serde_json::to_vec(&json!({ "key": key })).unwrap().into(),
            metadata: BTreeMap::from([
                ("title".into(), json!(title)),
                ("author_name".into(), json!("作者")),
                ("create_date".into(), json!(created)),
            ]),
            field_states: BTreeMap::new(),
            body: Some("本文".into()),
            media: vec![],
            attachments: vec![],
        })
        .unwrap()
        .entity_id
}

fn order(library: &Library, collection: LibraryId) -> Vec<LibraryId> {
    library
        .current_collection(collection)
        .unwrap()
        .unwrap()
        .members
        .iter()
        .map(|m| m.saved.work_id)
        .collect()
}

#[test]
fn one_edit_at_a_time_creates_reorders_sorts_dresses_and_deletes_a_collection() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-collection-edit-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let third = work(&library, "c", "第3話", "2024-03-01T00:00:00+09:00");
    let first = work(&library, "a", "第1話", "2024-02-01T00:00:00+09:00");
    let second = work(&library, "b", "第2話", "2024-01-01T00:00:00+09:00");
    let id = LibraryId::now_v7();
    let edit = |change| library.edit_collection(LibraryId::now_v7(), id, change);

    let create = LibraryId::now_v7();
    let made = CollectionEdit::Create {
        title: "読み順".into(),
        description: None,
        ordered: true,
        works: vec![third, first, third],
    };
    library.edit_collection(create, id, made.clone()).unwrap();
    // 送り直しても二度作らない。同じ ID でほかの要求は断る。
    library.edit_collection(create, id, made.clone()).unwrap();
    assert!(edit(made).is_err());
    assert_eq!(order(&library, id), vec![third, first]);

    edit(CollectionEdit::Add {
        works: vec![first, second],
    })
    .unwrap();
    assert_eq!(order(&library, id), vec![third, first, second]);
    edit(CollectionEdit::Sort {
        by: CollectionOrder::Episode,
    })
    .unwrap();
    assert_eq!(order(&library, id), vec![first, second, third]);
    edit(CollectionEdit::Sort {
        by: CollectionOrder::Published,
    })
    .unwrap();
    assert_eq!(order(&library, id), vec![second, first, third]);
    // 並べ替えは今の回をそろえて渡す。
    assert!(edit(CollectionEdit::Reorder {
        works: vec![third, first],
    })
    .is_err());
    edit(CollectionEdit::Reorder {
        works: vec![third, first, second],
    })
    .unwrap();
    assert_eq!(order(&library, id), vec![third, first, second]);
    edit(CollectionEdit::Remove { works: vec![first] }).unwrap();
    assert_eq!(order(&library, id), vec![third, second]);

    edit(CollectionEdit::Describe {
        title: "別の名前".into(),
        description: Some("説明".into()),
        ordered: false,
        name_source: Some("manual".into()),
    })
    .unwrap();
    // 表紙は入っている作品からだけ。
    assert!(edit(CollectionEdit::Look {
        cover_work: Some(first),
        cover_mode: Some("single".into()),
        cover_image: None,
    })
    .is_err());
    edit(CollectionEdit::Look {
        cover_work: Some(second),
        cover_mode: Some("single".into()),
        cover_image: None,
    })
    .unwrap();
    let summary = library.list_collections().unwrap().remove(0);
    assert_eq!(summary.name, "別の名前");
    assert!(!summary.ordered);
    assert_eq!(summary.cover_mode, "single");
    assert_eq!(summary.member_count, 2);

    // 表紙の画像は棚に取り込んだファイルだけ。外すと表紙の画像は無くなる。
    let missing = crate::library::Fingerprint::of(b"not stored");
    assert!(edit(CollectionEdit::Look {
        cover_work: None,
        cover_mode: Some("file".into()),
        cover_image: Some(Some(missing)),
    })
    .is_err());
    let image = std::env::temp_dir().join(format!("piep-cover-{}.png", LibraryId::now_v7()));
    std::fs::write(&image, b"PNG cover").unwrap();
    let stored = library
        .store_original(&crate::library::CaptureSource::File(image.clone()))
        .unwrap();
    let _ = std::fs::remove_file(&image);
    edit(CollectionEdit::Look {
        cover_work: None,
        cover_mode: Some("file".into()),
        cover_image: Some(Some(stored.id)),
    })
    .unwrap();
    let summary = library.list_collections().unwrap().remove(0);
    assert_eq!(summary.cover_image, Some(stored.id.to_string()));
    edit(CollectionEdit::Look {
        cover_work: None,
        cover_mode: Some("mosaic".into()),
        cover_image: Some(None),
    })
    .unwrap();
    assert_eq!(
        library.list_collections().unwrap().remove(0).cover_image,
        None
    );

    edit(CollectionEdit::Delete).unwrap();
    assert!(library.list_collections().unwrap().is_empty());
    assert!(matches!(
        edit(CollectionEdit::Add { works: vec![first] }),
        Err(Error::ResourceUnavailable)
    ));
    library.verify().unwrap();
}
