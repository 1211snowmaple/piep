use super::repair_attempts::DEFAULT_REPAIR_ATTEMPTS;
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
        keys(
            library
                .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
                .unwrap()
        ),
        ["1", "2"]
    );
    assert_eq!(
        library
            .incomplete_save_repairs(0, DEFAULT_REPAIR_ATTEMPTS)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        library
            .incomplete_save_repairs(1, DEFAULT_REPAIR_ATTEMPTS)
            .unwrap()[0]
            .1,
        "題"
    );

    // 棚に無い作品・その作品の版でないものは記録しない。同じ版の記録は重ねても一件。
    let listed = library
        .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
        .unwrap();
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
    assert_eq!(
        keys(
            library
                .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
                .unwrap()
        ),
        ["2"]
    );

    // 版が進めば、もう一度だけ試す。
    save(&library, "1", "直した本文");
    discover(&library, "1", "revision", Some("incomplete"));
    assert_eq!(
        keys(
            library
                .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
                .unwrap()
        ),
        ["1", "2"]
    );
    library.verify().unwrap();
}

/// 補完のジョブが先に新しい版を保存してから数えても、もう今の版でない試した版には記録を
/// 残さず、新しい版は次の候補に残る。
#[test]
fn an_attempt_on_a_version_that_is_no_longer_current_is_not_kept() {
    let (_temp, library) = fixture();
    save(&library, "1", "本文");
    discover(&library, "1", "revision", Some("incomplete"));
    let listed = library
        .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
        .unwrap();
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
    assert_eq!(rows(&library), 0);
    assert_eq!(
        keys(
            library
                .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
                .unwrap()
        ),
        ["1"]
    );
    library.verify().unwrap();
}

fn rows(library: &Library) -> i64 {
    library
        .reader()
        .unwrap()
        .query_row("SELECT count(*) FROM repair_attempts", [], |r| r.get(0))
        .unwrap()
}

/// 試した版の後に新しい版を保存したら、前の版の記録は保存と一緒に消える。
#[test]
fn saving_a_new_version_drops_the_attempts_on_the_previous_one() {
    let (_temp, library) = fixture();
    save(&library, "1", "本文");
    discover(&library, "1", "revision", Some("incomplete"));
    let tried = library
        .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
        .unwrap()[0]
        .2;
    library
        .record_incomplete_save_attempts(&[("1".to_owned(), tried)], 10)
        .unwrap();
    assert_eq!(rows(&library), 1);
    // 同じ内容の保存では版は変わらず、記録も残る。
    save(&library, "1", "本文");
    assert_eq!(rows(&library), 1);
    save(&library, "1", "挿絵まで取れた本文");
    assert_eq!(rows(&library), 0);
    library.verify().unwrap();
}

/// 補完の回数は、欠けた版を作った保存を 1 回目として数え、上限に届いたら候補から外す。
/// 欠けの指紋は、表示できない挿絵の参照と伏せられた作品情報から作る。
#[test]
fn pixiv_attempts_count_the_save_that_left_the_gap() {
    let (_temp, library) = fixture();
    let saved = save(&library, "1", "前[pixivimage:123]後");
    discover(&library, "1", "revision", Some("incomplete"));
    let tried = library
        .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
        .unwrap()[0]
        .2;
    library
        .record_incomplete_save_attempts(&[("1".to_owned(), tried)], 10)
        .unwrap();
    let (attempts, fingerprint, request): (u32, Vec<u8>, Option<Vec<u8>>) = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT attempts,gap_fingerprint,request_id FROM repair_attempts WHERE work_id=?1",
            [saved.entity_id.as_bytes().as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(request, None);
    let expected = repair_attempts::gap_fingerprint(
        &library.reader().unwrap(),
        saved.snapshot_id,
        &["[pixivimage:123]".to_owned()],
    )
    .unwrap();
    assert_eq!(fingerprint, expected.0.to_vec());
    assert!(attempts >= repair_attempts::DEFAULT_REPAIR_ATTEMPTS);
    assert!(library
        .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
        .unwrap()
        .is_empty());
    library.verify().unwrap();
}

/// 上限は呼ぶ側（詳細設定）が渡す。上げれば、上限に届いていた作品もまた候補に戻る。
#[test]
fn the_attempt_limit_comes_from_the_caller() {
    let (_temp, library) = fixture();
    save(&library, "1", "本文");
    discover(&library, "1", "revision", Some("incomplete"));
    let tried = library
        .incomplete_save_repairs(50, DEFAULT_REPAIR_ATTEMPTS)
        .unwrap()[0]
        .2;
    library
        .record_incomplete_save_attempts(&[("1".to_owned(), tried)], 10)
        .unwrap();
    assert!(library.incomplete_save_repairs(50, 2).unwrap().is_empty());
    assert_eq!(keys(library.incomplete_save_repairs(50, 3).unwrap()), ["1"]);
    // 1 は「取り直さない」：最初の保存だけで上限に届く。
    save(&library, "2", "本文");
    discover(&library, "2", "revision", Some("incomplete"));
    assert_eq!(
        keys(library.incomplete_save_repairs(50, 3).unwrap()),
        ["1", "2"]
    );
    let none: Vec<String> = Vec::new();
    assert_eq!(keys(library.incomplete_save_repairs(50, 1).unwrap()), none);
}
