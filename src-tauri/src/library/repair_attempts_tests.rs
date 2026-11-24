use super::*;
use crate::library::{
    asset_repairs::AssetRepairObservation, CaptureResult, Library, StagedAsset, WorkSave,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-repair-attempts-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    (Temp(root), library)
}
fn pixiv(library: &Library, text: &str) -> CaptureResult {
    library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "save",
                data: &json!({"detail":{"title":"題","caption":"","tags":[]},"text":text}),
                assets: vec![],
            },
            None,
        )
        .unwrap()
}
fn fanbox_data() -> Value {
    json!({"id":"1","title":"投稿","creatorId":"creator","type":"file","body":{"files":[{"id":"a","name":"a","extension":"txt","url":"https://example.invalid/a"}]}})
}
fn fanbox(library: &Library, root: &Path, complete: bool) -> CaptureResult {
    let assets = if complete {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join("a.txt");
        std::fs::write(&path, b"a").unwrap();
        vec![StagedAsset {
            role: "file".into(),
            name: "a_a.txt".into(),
            path,
            url: Some("https://example.invalid/a".into()),
            mime_type: Some("text/plain".into()),
        }]
    } else {
        vec![]
    };
    library
        .save_work_with_asset_repair(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &fanbox_data(),
                assets,
            },
            AssetRepairObservation {
                expected_urls: vec!["https://example.invalid/a".into()],
                complete,
            },
        )
        .unwrap()
}
fn rows(library: &Library) -> Vec<(Vec<u8>, u32)> {
    library
        .reader()
        .unwrap()
        .prepare("SELECT snapshot_id,attempts FROM repair_attempts ORDER BY work_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// 取得元によらず同じ規則で数える：同じ版・同じ指紋なら数え上げ、指紋が変われば 1 から、揃えば 0。
/// 新しい版を保存したら前の版の記録は消える。
#[test]
fn the_same_rules_hold_for_every_source() {
    let (temp, library) = fixture();
    let first = pixiv(&library, "本文");
    let fanboxed = fanbox(&library, &temp.0.join("staged"), false);
    for saved in [&first, &fanboxed] {
        let (work, snapshot) = (saved.entity_id, saved.snapshot_id);
        let conn = library.conn.lock().unwrap();
        let a = gap_fingerprint(&conn, snapshot, &["a".into()]).unwrap();
        let b = gap_fingerprint(&conn, snapshot, &["b".into()]).unwrap();
        assert_eq!(record(&conn, work, snapshot, a, false, None, 1).unwrap(), 1);
        assert_eq!(record(&conn, work, snapshot, a, false, None, 2).unwrap(), 2);
        assert_eq!(record(&conn, work, snapshot, b, false, None, 3).unwrap(), 1);
        assert_eq!(attempts(&conn, work, snapshot, a).unwrap(), 0);
        assert_eq!(record(&conn, work, snapshot, b, true, None, 4).unwrap(), 0);
        // 保存の外の取り直しは、欠けた版を作った保存を 1 回目に数える。
        assert_eq!(record_retry(&conn, work, snapshot, a, 5).unwrap(), 2);
        assert_eq!(record_retry(&conn, work, snapshot, a, 6).unwrap(), 3);
    }
    assert_eq!(rows(&library).len(), 2);
    let next = pixiv(&library, "新しい本文");
    assert_ne!(next.snapshot_id, first.snapshot_id);
    let filled = fanbox(&library, &temp.0.join("staged"), true);
    assert_ne!(filled.snapshot_id, fanboxed.snapshot_id);
    // pixiv の記録は消え、FANBOX は揃った新しい版の記録（0）だけが残る。
    assert_eq!(
        rows(&library),
        vec![(filled.snapshot_id.as_bytes().to_vec(), 0)]
    );
    library.verify().unwrap();
}

/// 上限に届いた FANBOX の状態は、もう取りに行かない。
#[test]
fn fanbox_stops_at_the_limit() {
    let (temp, library) = fixture();
    let mut saved = fanbox(&library, &temp.0.join("staged"), false);
    for _ in 1..DEFAULT_REPAIR_ATTEMPTS {
        saved = fanbox(&library, &temp.0.join("staged"), false);
    }
    let plan = library
        .asset_repair_plan(
            saved.entity_id,
            saved.snapshot_id,
            &["https://example.invalid/a".into()],
        )
        .unwrap();
    assert_eq!(plan.attempts, DEFAULT_REPAIR_ATTEMPTS);
    library.verify().unwrap();
}

/// 移行 34：二つの表の今の版の行だけを移し、古い表を落とす。
#[test]
fn migration_moves_the_current_rows_of_both_tables() {
    let (temp, library) = fixture();
    let old = pixiv(&library, "本文");
    let current = pixiv(&library, "新しい本文");
    let fanboxed = fanbox(&library, &temp.0.join("staged"), false);
    let (fingerprint, request): (Vec<u8>, Vec<u8>) = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT gap_fingerprint,request_id FROM repair_attempts WHERE work_id=?1",
            [fanboxed.entity_id.as_bytes().as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute_batch(include_str!("migration_025_incomplete_save_attempts.sql"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE asset_repair_state (
                work_id BLOB PRIMARY KEY REFERENCES works(id),
                snapshot_id BLOB NOT NULL,
                state_fingerprint BLOB NOT NULL CHECK(length(state_fingerprint)=32),
                attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
                request_id BLOB NOT NULL,
                FOREIGN KEY(snapshot_id,work_id) REFERENCES revision_snapshots(id,entity_id),
                FOREIGN KEY(request_id,work_id) REFERENCES acquisition_attempts(id,entity_id)
            ) STRICT;
            DROP TABLE repair_attempts;",
        )
        .unwrap();
        // pixiv：今の版の印と、古い版に残った印（別の作品の行として置けないので、今の版の行を
        // 入れる前の形で確かめる）。
        conn.execute(
            "INSERT INTO incomplete_save_attempts(work_id,snapshot_id,attempted_at) VALUES(?1,?2,7)",
            rusqlite::params![
                current.entity_id.as_bytes().as_slice(),
                old.snapshot_id.as_bytes().as_slice()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO asset_repair_state VALUES(?1,?2,?3,1,?4)",
            rusqlite::params![
                fanboxed.entity_id.as_bytes().as_slice(),
                fanboxed.snapshot_id.as_bytes().as_slice(),
                fingerprint,
                request
            ],
        )
        .unwrap();
        conn.execute_batch(
            "ALTER TABLE library_meta DROP COLUMN repair_attempt_limit;
             DELETE FROM schema_migrations WHERE version>=34; PRAGMA user_version=33;",
        )
        .unwrap();
    }
    drop(library);
    let library = Library::open(&temp.0.join("shelf")).unwrap();
    // 古い版を指した pixiv の印は移さない。
    assert_eq!(
        rows(&library),
        vec![(fanboxed.snapshot_id.as_bytes().to_vec(), 1)]
    );
    library.verify().unwrap();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute_batch(include_str!("migration_025_incomplete_save_attempts.sql"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE asset_repair_state(work_id BLOB PRIMARY KEY, snapshot_id BLOB NOT NULL,
                 state_fingerprint BLOB NOT NULL, attempts INTEGER NOT NULL, request_id BLOB NOT NULL) STRICT;
             DROP TABLE repair_attempts;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO incomplete_save_attempts(work_id,snapshot_id,attempted_at) VALUES(?1,?2,7)",
            rusqlite::params![
                current.entity_id.as_bytes().as_slice(),
                current.snapshot_id.as_bytes().as_slice()
            ],
        )
        .unwrap();
        conn.execute_batch(
            "ALTER TABLE library_meta DROP COLUMN repair_attempt_limit;
             DELETE FROM schema_migrations WHERE version>=34; PRAGMA user_version=33;",
        )
        .unwrap();
    }
    drop(library);
    let library = Library::open(&temp.0.join("shelf")).unwrap();
    // 今の版の pixiv の印は、分からない指紋で上限に届いた記録になる。
    let (snapshot, attempts, fingerprint, request, at): (
        Vec<u8>,
        u32,
        Vec<u8>,
        Option<Vec<u8>>,
        i64,
    ) = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT snapshot_id,attempts,gap_fingerprint,request_id,attempted_at FROM repair_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(snapshot, current.snapshot_id.as_bytes().to_vec());
    assert_eq!(attempts, DEFAULT_REPAIR_ATTEMPTS);
    assert_eq!(fingerprint, vec![0; 32]);
    assert_eq!(request, None);
    assert_eq!(at, 7);
    library.verify().unwrap();
}

/// 上限は棚の設定。既定は 2、範囲の外は断り、開き直しても残る。上げれば、上限に届いていた
/// 状態をまた取りに行く。
#[test]
fn the_limit_is_a_shelf_setting() {
    let (temp, library) = fixture();
    assert_eq!(
        library.repair_attempt_limit().unwrap(),
        DEFAULT_REPAIR_ATTEMPTS
    );
    fanbox(&library, &temp.0.join("staged"), false);
    let saved = fanbox(&library, &temp.0.join("staged"), false);
    let plan = |library: &Library| {
        library
            .asset_repair_plan(
                saved.entity_id,
                saved.snapshot_id,
                &["https://example.invalid/a".into()],
            )
            .unwrap()
    };
    assert_eq!((plan(&library).attempts, plan(&library).limit), (2, 2));
    for wrong in [0, REPAIR_ATTEMPT_LIMITS.end() + 1] {
        assert!(library.set_repair_attempt_limit(wrong).is_err());
    }
    assert_eq!(library.repair_attempt_limit().unwrap(), 2);
    library.set_repair_attempt_limit(3).unwrap();
    assert_eq!(plan(&library).limit, 3);
    drop(library);
    let library = Library::open(&temp.0.join("shelf")).unwrap();
    assert_eq!(library.repair_attempt_limit().unwrap(), 3);
    library.verify().unwrap();
}
