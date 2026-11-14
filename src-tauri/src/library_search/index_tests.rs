use super::*;
use crate::library::{Capture, EntityKind, TagProvenance, TagUpdate, UserTag};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn capture(key: &str, title: &str, body: &str, acquired: i64) -> Capture {
    Capture {
        acquired_at: Some(acquired),
        request_id: LibraryId::now_v7(),
        provider: "pixiv".into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"key": key, "body": body}))
            .unwrap()
            .into(),
        metadata: BTreeMap::from([
            ("title".into(), json!(title)),
            ("author_name".into(), json!("作者")),
            ("caption".into(), json!("")),
            ("tags".into(), json!(["物語"])),
        ]),
        field_states: BTreeMap::new(),
        body: Some(body.into()),
        media: vec![],
        attachments: vec![],
    }
}

fn found(library: &Library, query: &str) -> Vec<LibraryId> {
    library
        .search_text(query)
        .unwrap()
        .into_iter()
        .map(|hit| hit.work)
        .collect()
}

#[test]
fn the_index_follows_saves_tags_and_removals_and_rebuilds_when_lost() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-search-index-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let apple = library
        .capture(capture("1", "一", "林檎の話を書く", 1_000))
        .unwrap()
        .entity_id;
    let orange = library
        .capture(capture("2", "二", "蜜柑の話を書く", 2_000))
        .unwrap()
        .entity_id;
    let grape = library
        .capture(capture("3", "三", "葡萄", 3_000))
        .unwrap()
        .entity_id;
    let mut quiet = |_: usize, _: usize| {};

    let first = library.sync_search_index(&mut quiet).unwrap();
    assert!(first.reset);
    assert_eq!((first.indexed, first.removed, first.skipped), (3, 0, 0));
    assert_eq!(found(&library, "林檎"), vec![apple]);
    let mut both = found(&library, "話");
    both.sort();
    let mut expected = vec![apple, orange];
    expected.sort();
    assert_eq!(both, expected);
    assert_eq!(found(&library, "話 -林檎"), vec![orange]);

    // 変わっていなければ何もしない。
    let again = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!(again, SearchIndexReport::default());

    // 利用者のタグは文書に入る。付けた作品だけを作り直す。
    library
        .replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work: grape,
            user: library.id,
            expected_generation: 0,
            tags: vec![UserTag {
                name: "果物".into(),
                provenance: TagProvenance::Manual,
            }],
        })
        .unwrap();
    assert!(
        found(&library, "果物").is_empty(),
        "同期の前は古い文書のまま"
    );
    let tagged = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!(tagged.indexed, 1);
    assert_eq!(found(&library, "果物"), vec![grape]);

    // 改稿した作品は、新しい本文で引ける。古い本文の語ではもう出ない。
    library
        .capture(capture("2", "二", "檸檬の話を書く", 4_000))
        .unwrap();
    let revised = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!(revised.indexed, 1);
    assert!(found(&library, "蜜柑").is_empty());
    assert_eq!(found(&library, "檸檬"), vec![orange]);

    // 棚にない作品の文書は消す。
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO search_documents(work_id,recipe) VALUES(?1,?2)",
            params![LibraryId::now_v7().as_bytes().as_slice(), RECIPE],
        )
        .unwrap();
    let pruned = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!((pruned.removed, pruned.indexed), (1, 0));

    // 索引の場所が消えたら、空から作り直す。
    crate::database::tantivy_index::release_runtime(&temp.0.join("derived"));
    std::fs::remove_dir_all(temp.0.join("derived")).unwrap();
    let rebuilt = library.sync_search_index(&mut quiet).unwrap();
    assert!(rebuilt.reset);
    assert_eq!(rebuilt.indexed, 3);
    assert_eq!(found(&library, "檸檬"), vec![orange]);
    assert_eq!(found(&library, "果物"), vec![grape]);
    crate::database::tantivy_index::release_runtime(&temp.0.join("derived"));
}
