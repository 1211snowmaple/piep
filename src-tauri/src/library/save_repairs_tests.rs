use super::*;
use crate::library::{DiscoveryInput, WorkSave};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-save-repairs-{}", LibraryId::now_v7()));
    let library = Library::create(&root).unwrap();
    (Temp(root), library)
}
fn data(text: &str) -> Value {
    json!({"detail":{"title":"題", "caption":"", "tags":[], "user":{"id":"77","name":"作者"}}, "text":text})
}
fn save(library: &Library, key: &str, text: &str) -> crate::library::CaptureResult {
    library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key,
                route: "save",
                data: &data(text),
                assets: vec![],
            },
            None,
        )
        .unwrap()
}
fn discover(library: &Library, key: &str, kind: &str, hold: Option<&str>) {
    let payload = match hold {
        Some(hold) => json!({"holdKind":hold,"holdReason":"理由"}).to_string(),
        None => json!({"kind":kind}).to_string(),
    };
    library
        .record_discovery(
            LibraryId::now_v7(),
            DiscoveryInput {
                provider: "pixiv",
                key,
                kind,
                title: "候補",
                payload: &payload,
                target_type: Some("work"),
            },
            hold.is_some(),
        )
        .unwrap();
}
fn keys(rows: Vec<(String, String, LibraryId)>) -> Vec<String> {
    let mut keys: Vec<String> = rows.into_iter().map(|(key, ..)| key).collect();
    keys.sort();
    keys
}

#[test]
fn pending_revisions_are_the_undecided_revisions_of_works_on_the_shelf() {
    let (_temp, library) = fixture();
    let saved = save(&library, "1", "本文");
    let other = save(&library, "2", "本文");
    discover(&library, "1", "revision", None);
    discover(&library, "2", "revision", None);
    // 保存していない作品の候補は、棚の作品の印にならない。
    discover(&library, "3", "revision", None);
    discover(&library, "4", "new", None);
    let mut works: Vec<_> = library
        .pending_revisions()
        .unwrap()
        .into_iter()
        .map(|(work, found)| {
            assert!(found > 0);
            work
        })
        .collect();
    works.sort();
    let mut expected = vec![saved.entity_id, other.entity_id];
    expected.sort();
    assert_eq!(works, expected);
    assert!(library.has_pending_revision(saved.entity_id).unwrap());

    library
        .decide_discovery(LibraryId::now_v7(), "pixiv", "1", None, true)
        .unwrap();
    assert!(!library.has_pending_revision(saved.entity_id).unwrap());
    let generation = library
        .library_item_generation(other.entity_id)
        .unwrap()
        .unwrap();
    library
        .remove_work(LibraryId::now_v7(), other.entity_id, generation)
        .unwrap();
    assert!(library.pending_revisions().unwrap().is_empty());
    assert!(!library.has_pending_revision(other.entity_id).unwrap());
}

#[test]
fn incomplete_saves_are_tried_once_per_snapshot() {
    let (_temp, library) = fixture();
    for key in ["1", "2", "3"] {
        save(&library, key, "本文");
    }
    discover(&library, "1", "revision", Some("incomplete"));
    discover(&library, "2", "revision", Some("incomplete"));
    discover(&library, "3", "revision", Some("unavailable"));
    // 保存していない作品は取り直しの対象にしない。
    discover(&library, "9", "new", Some("incomplete"));
    assert_eq!(
        keys(library.incomplete_save_repairs(50).unwrap()),
        ["1", "2"]
    );
    assert_eq!(library.incomplete_save_repairs(0).unwrap().len(), 1);
    assert_eq!(library.incomplete_save_repairs(1).unwrap()[0].1, "題");

    // 棚に無い作品・その作品の版でないものは記録しない。同じ版の記録は重ねても一件。
    let listed = library.incomplete_save_repairs(50).unwrap();
    let first = listed.iter().find(|(key, ..)| key == "1").unwrap().2;
    let second = listed.iter().find(|(key, ..)| key == "2").unwrap().2;
    let keys_tried = [
        ("1".to_owned(), first),
        ("9".to_owned(), first),
        ("missing".to_owned(), first),
        ("2".to_owned(), first),
    ];
    assert_ne!(first, second);
    assert_eq!(
        library
            .record_incomplete_save_attempts(&keys_tried, 10)
            .unwrap(),
        1
    );
    assert_eq!(
        library
            .record_incomplete_save_attempts(&keys_tried, 11)
            .unwrap(),
        1
    );
    assert_eq!(keys(library.incomplete_save_repairs(50).unwrap()), ["2"]);

    // 版が進めば、もう一度だけ試す。
    save(&library, "1", "直した本文");
    discover(&library, "1", "revision", Some("incomplete"));
    assert_eq!(
        keys(library.incomplete_save_repairs(50).unwrap()),
        ["1", "2"]
    );
    library.verify().unwrap();
}

/// 補完のジョブが先に新しい版を保存してから印を付けても、もう今の版でない試した版には印を
/// 残さず、新しい版は次の候補に残る。
#[test]
fn a_mark_for_a_version_that_is_no_longer_current_is_not_kept() {
    let (_temp, library) = fixture();
    save(&library, "1", "本文");
    discover(&library, "1", "revision", Some("incomplete"));
    let listed = library.incomplete_save_repairs(50).unwrap();
    assert_eq!(keys(listed.clone()), ["1"]);
    let tried = listed[0].2;
    // ジョブが先に保存を確定した（まだ一部保存のまま）。
    save(&library, "1", "直した本文");
    discover(&library, "1", "revision", Some("incomplete"));
    assert_eq!(
        library
            .record_incomplete_save_attempts(&[("1".to_owned(), tried)], 10)
            .unwrap(),
        0
    );
    assert_eq!(marks(&library), 0);
    assert_eq!(keys(library.incomplete_save_repairs(50).unwrap()), ["1"]);
}

fn marks(library: &Library) -> i64 {
    library
        .reader()
        .unwrap()
        .query_row("SELECT count(*) FROM incomplete_save_attempts", [], |r| {
            r.get(0)
        })
        .unwrap()
}

/// 試した版の後に新しい版を保存したら、前の版の印は保存と一緒に消える。
#[test]
fn saving_a_new_version_drops_the_mark_of_the_previous_one() {
    let (_temp, library) = fixture();
    save(&library, "1", "本文");
    discover(&library, "1", "revision", Some("incomplete"));
    let tried = library.incomplete_save_repairs(50).unwrap()[0].2;
    library
        .record_incomplete_save_attempts(&[("1".to_owned(), tried)], 10)
        .unwrap();
    assert_eq!(marks(&library), 1);
    // 同じ内容の保存では版は変わらず、印も残る。
    save(&library, "1", "本文");
    assert_eq!(marks(&library), 1);
    save(&library, "1", "挿絵まで取れた本文");
    assert_eq!(marks(&library), 0);
    library.verify().unwrap();
}

/// 前の仕組みで残った、今の版を指さない印は、次に印を付けるときに消える。
#[test]
fn recording_attempts_sweeps_marks_left_on_old_versions() {
    let (_temp, library) = fixture();
    save(&library, "1", "本文");
    save(&library, "2", "本文");
    let first = save(&library, "1", "本文").snapshot_id;
    save(&library, "1", "新しい本文");
    // 古い仕組みが残した印を直に書く。
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO incomplete_save_attempts(work_id,snapshot_id,attempted_at)
             SELECT entity_id,id,1 FROM revision_snapshots WHERE id=?1",
            [first.as_bytes().as_slice()],
        )
        .unwrap();
    assert_eq!(marks(&library), 1);
    discover(&library, "2", "revision", Some("incomplete"));
    let tried = library.incomplete_save_repairs(50).unwrap()[0].2;
    assert_eq!(
        library
            .record_incomplete_save_attempts(&[("2".to_owned(), tried)], 10)
            .unwrap(),
        1
    );
    assert_eq!(marks(&library), 1);
    assert_eq!(
        keys(library.incomplete_save_repairs(50).unwrap()),
        Vec::<String>::new()
    );
}
