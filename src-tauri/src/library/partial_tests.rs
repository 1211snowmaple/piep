use super::*;
use crate::library::{
    Capture, CaptureResult, CapturedMedia, EditBlock, EditContent, EditRequest, EntityKind,
    UserStateChange,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-part-{}", LibraryId::now_v7()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn capture(library: &Library, key: &str, author: &str, series: Option<&str>) -> CaptureResult {
    let mut metadata = BTreeMap::from([
        ("title".into(), json!(format!("作品 {key}"))),
        ("author_key".into(), json!(author)),
        ("author_name".into(), json!(format!("作者 {author}"))),
        ("tags".into(), json!(["共通", format!("タグ {key}")])),
    ]);
    if let Some(series) = series {
        metadata.insert("series_id".into(), json!(series));
        metadata.insert("series_title".into(), json!("シリーズ"));
        metadata.insert("series_position".into(), json!(1));
    }
    library
        .capture(Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: format!("{{\"id\":\"{key}\"}}").into_bytes().into(),
            metadata,
            field_states: BTreeMap::new(),
            body: Some(format!("本文 {key}").into()),
            media: vec![CapturedMedia {
                slot: "cover".into(),
                role: "cover".into(),
                source_key: Some(format!("cover-{key}")),
                source_url: Some(format!("https://example.test/cover-{key}.png")),
                quality: "original".into(),
                original_name: "cover.png".into(),
                mime_type: "image/png".into(),
                bytes: Some(format!("表紙 {key}").into_bytes().into()),
            }],
            attachments: vec![],
        })
        .unwrap()
}

fn edit(library: &Library, saved: &CaptureResult) -> LibraryId {
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: Some("直した題".into()),
        blocks: vec![EditBlock {
            id: LibraryId::now_v7(),
            ordinal: 0,
            kind: "paragraph".into(),
            text: Some("直した本文".into()),
            attrs: json!({}),
            media_slot: None,
        }],
        media: vec![],
        provenance: None,
    };
    library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: saved.entity_id,
            user: library.id,
            base: saved.snapshot_id,
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap()
}

fn count(library: &Library, sql: &str) -> i64 {
    library
        .reader()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

fn edit_of(library: &Library, work: LibraryId) -> Option<LibraryId> {
    library
        .reader()
        .unwrap()
        .query_row(
            "SELECT edit_id FROM work_current_projection WHERE work_id=?1",
            [work.as_bytes().as_slice()],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .unwrap()
        .map(|id| LibraryId::from_slice(&id).unwrap())
}

fn titles(library: &Library) -> Vec<String> {
    let mut titles: Vec<_> = library
        .works(100)
        .unwrap()
        .into_iter()
        .map(|w| w.title)
        .collect();
    titles.sort();
    titles
}

#[test]
fn an_author_archive_holds_only_their_works_and_opens_only_as_a_part() {
    let temp = Temp::new();
    let shelf = Library::create(&temp.0.join("shelf")).unwrap();
    let one = capture(&shelf, "1", "a", Some("s"));
    let two = capture(&shelf, "2", "a", None);
    capture(&shelf, "3", "b", None);
    let edited = edit(&shelf, &two);
    shelf
        .set_user_state(
            LibraryId::now_v7(),
            one.entity_id,
            shelf.id,
            0,
            UserStateChange {
                favorite: Some(true),
                watching: None,
            },
        )
        .unwrap();
    let destination = temp.0.join("archive");
    let outcome = shelf
        .copy_works(&[one.entity_id, two.entity_id], &destination)
        .unwrap();
    assert_eq!(
        outcome,
        PartTransfer {
            added: 2,
            ..PartTransfer::default()
        }
    );
    // 部分写しは棚としては開けない（復元で棚と入れ替える候補にならない）。
    assert!(Library::open(&destination).is_err());
    assert!(Library::is_part(&destination));
    assert!(!Library::is_part(&temp.0.join("shelf")));
    let part = Library::open_part(&destination).unwrap();
    assert!(part.verify().unwrap() > 0);
    assert_eq!(part.id, shelf.id);
    // 選んだ作品と、その作者・シリーズ・タグだけ。ほかの作者の作品や表紙は入らない。
    assert_eq!(titles(&part), ["作品 1", "直した題"]);
    assert_eq!(count(&part, "SELECT count(*) FROM people"), 1);
    assert_eq!(count(&part, "SELECT count(*) FROM series"), 1);
    assert_eq!(count(&part, "SELECT count(*) FROM works"), 2);
    assert_eq!(
        count(&part, "SELECT count(*) FROM tags"),
        3,
        "共通・タグ 1・タグ 2"
    );
    assert_eq!(count(&part, "SELECT count(*) FROM source_media"), 2);
    assert_eq!(
        part.current_snapshot(one.entity_id).unwrap(),
        one.snapshot_id
    );
    // 編集・お気に入りも写る。
    assert_eq!(edit_of(&part, two.entity_id), Some(edited));
    assert_eq!(
        count(
            &part,
            "SELECT count(*) FROM work_user_state WHERE favorite=1"
        ),
        1
    );
    drop(part);
    // 同じ場所にもう一度は作らない。途中の写しも残さない。
    assert!(shelf.copy_works(&[one.entity_id], &destination).is_err());
    let leftovers = fs::read_dir(&temp.0)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".piep-part-")
        })
        .count();
    assert_eq!(leftovers, 0);
    shelf.verify().unwrap();
}

#[test]
fn works_removed_from_the_shelf_come_back_from_the_archive_as_they_were() {
    let temp = Temp::new();
    let shelf = Library::create(&temp.0.join("shelf")).unwrap();
    let one = capture(&shelf, "1", "a", Some("s"));
    let two = capture(&shelf, "2", "a", Some("s"));
    let edited = edit(&shelf, &two);
    let destination = temp.0.join("archive");
    shelf
        .copy_works(&[one.entity_id, two.entity_id], &destination)
        .unwrap();
    shelf
        .remove_work(LibraryId::now_v7(), two.entity_id, 1)
        .unwrap();
    assert_eq!(titles(&shelf), ["作品 1"]);
    let part = Library::open_part(&destination).unwrap();
    let outcome = shelf.merge_part(&part).unwrap();
    assert_eq!(
        outcome,
        PartTransfer {
            restored: 1,
            already: 1,
            ..PartTransfer::default()
        }
    );
    assert_eq!(titles(&shelf), ["作品 1", "直した題"]);
    assert_eq!(edit_of(&shelf, two.entity_id), Some(edited));
    shelf.verify().unwrap();
    // 二度目は何も足さない。
    let again = shelf.merge_part(&part).unwrap();
    assert_eq!(
        again,
        PartTransfer {
            already: 2,
            ..PartTransfer::default()
        }
    );
    shelf.verify().unwrap();
}

#[test]
fn an_older_copy_of_the_shelf_gains_new_works_but_not_ones_saved_again_under_another_id() {
    let temp = Temp::new();
    let shelf = Library::create(&temp.0.join("shelf")).unwrap();
    capture(&shelf, "1", "a", None);
    // 作品を足す前の写し。同じ棚なので、ここへは取り込める。
    shelf.backup(&temp.0.join("older")).unwrap();
    let two = capture(&shelf, "2", "a", Some("s"));
    let three = capture(&shelf, "3", "c", None);
    let destination = temp.0.join("archive");
    shelf
        .copy_works(&[two.entity_id, three.entity_id], &destination)
        .unwrap();
    drop(shelf);
    let older = Library::open(&temp.0.join("older")).unwrap();
    // 写しのあとで、同じ作品 3 を別に保存し直した。識別子が違うので足せない。
    let again = capture(&older, "3", "c", None);
    assert_ne!(again.entity_id, three.entity_id);
    let part = Library::open_part(&destination).unwrap();
    let outcome = older.merge_part(&part).unwrap();
    assert_eq!(
        outcome,
        PartTransfer {
            added: 1,
            conflicts: 1,
            ..PartTransfer::default()
        }
    );
    assert_eq!(titles(&older), ["作品 1", "作品 2", "作品 3"]);
    assert_eq!(
        older.current_snapshot(two.entity_id).unwrap(),
        two.snapshot_id
    );
    // 作者 a は同じ記録を使う。ぶつかった作品の作者 c は足さない。
    assert_eq!(count(&older, "SELECT count(*) FROM people"), 2);
    older.verify().unwrap();
    // 足した作品の本文・表紙は、写しが無くても読める。
    drop(part);
    fs::remove_dir_all(&destination).unwrap();
    older.verify().unwrap();
}

#[test]
fn a_part_from_another_shelf_is_refused_without_touching_this_one() {
    let temp = Temp::new();
    let shelf = Library::create(&temp.0.join("shelf")).unwrap();
    let one = capture(&shelf, "1", "a", None);
    let destination = temp.0.join("archive");
    shelf.copy_works(&[one.entity_id], &destination).unwrap();
    let other = Library::create(&temp.0.join("other")).unwrap();
    let part = Library::open_part(&destination).unwrap();
    assert!(matches!(
        other.merge_part(&part),
        Err(Error::Unsupported(_))
    ));
    assert!(other.works(10).unwrap().is_empty());
    other.verify().unwrap();
}
