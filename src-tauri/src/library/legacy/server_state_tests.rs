use super::*;
use crate::library::{Fingerprint, LegacyImportReport};
use serde_json::json;
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-server-state-{}", LibraryId::now_v7()));
        let old = root.join("old");
        fs::create_dir_all(&old).unwrap();
        let conn = Connection::open(old.join("piep.db")).unwrap();
        conn.execute_batch(r#"CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE work_edit_revisions(id INTEGER,download_id INTEGER,base_version INTEGER,status TEXT,title TEXT,content_hash TEXT,created_at TEXT,updated_at TEXT);
CREATE TABLE work_edit_blocks(id INTEGER,edit_revision_id INTEGER,block_order INTEGER,block_type TEXT,text TEXT,asset_id INTEGER,attrs_json TEXT);
INSERT INTO downloads VALUES(1,'pixiv','123','edited','author',0);
INSERT INTO downloads VALUES(2,'pixiv','456','paged','author',1);
INSERT INTO downloads VALUES(3,'pixiv','777','single','author',0);
INSERT INTO work_edit_revisions VALUES(7,1,1,'active','Edited','edit-hash','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO work_edit_blocks VALUES(11,7,0,'paragraph','one',NULL,NULL);
INSERT INTO work_edit_blocks VALUES(12,7,1,'page_break',NULL,NULL,NULL);
INSERT INTO work_edit_blocks VALUES(13,7,2,'paragraph','two',NULL,NULL);"#).unwrap();
        for (id, key, text) in [
            (1, "123", "body"),
            (2, "456", "first[newpage]second"),
            (3, "777", "only one page"),
        ] {
            let path = old.join(format!("downloads/pixiv/{key}/v1/original.json"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                json!({"detail":{"title":key},"text":text}).to_string(),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO download_versions VALUES(?1,?1,1,?2,?2,'source')",
                params![id, path.to_str().unwrap()],
            )
            .unwrap();
        }
        fs::create_dir_all(old.join("server/outgoing")).unwrap();
        fs::write(old.join("server/settings.json"), b"{}").unwrap();
        let state = Connection::open(old.join("server/state.db")).unwrap();
        state.execute_batch(r#"CREATE TABLE devices(id TEXT PRIMARY KEY,name TEXT NOT NULL,role TEXT NOT NULL,token_hash BLOB NOT NULL,created_at TEXT NOT NULL,last_seen_at TEXT);
CREATE TABLE reading(source TEXT NOT NULL,source_id TEXT NOT NULL,version INTEGER,edit_hash TEXT,page INTEGER NOT NULL,position REAL NOT NULL,finished INTEGER NOT NULL,device_id TEXT NOT NULL,device_time INTEGER NOT NULL,cursor INTEGER NOT NULL,PRIMARY KEY(source,source_id));
CREATE TABLE favorite_changes(source TEXT NOT NULL,source_id TEXT NOT NULL,favorite INTEGER NOT NULL,device_id TEXT NOT NULL,device_time INTEGER NOT NULL,cursor INTEGER NOT NULL,PRIMARY KEY(source,source_id));
INSERT INTO reading VALUES('pixiv','456',1,NULL,1,0.25,0,'phone',1700000000000,1);
INSERT INTO reading VALUES('pixiv','123',1,'edit-hash',1,1.0,1,'phone',1700000000001,2);
INSERT INTO reading VALUES('pixiv','777',1,NULL,3,0.5,0,'phone',1700000000002,3);
INSERT INTO reading VALUES('pixiv','999',1,NULL,0,0.5,0,'phone',1700000000003,4);
INSERT INTO favorite_changes VALUES('pixiv','456',1,'phone',1700000000004,5);
INSERT INTO favorite_changes VALUES('pixiv','123',1,'phone',1700000000005,6);
INSERT INTO favorite_changes VALUES('pixiv','gone',0,'phone',1700000000006,7);"#).unwrap();
        Self(root)
    }

    fn import(&self) -> (Library, LegacyImportReport) {
        let library = Library::create(&self.0.join("new")).unwrap();
        let source = LegacyImporter::open(&self.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        (library, report)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn status(library: &Library, table: &str, key: &str) -> String {
    library
        .reader()
        .unwrap()
        .query_row(
            "SELECT status FROM migration_records WHERE source_table=?1 AND source_key=?2",
            params![table, key],
            |r| r.get(0),
        )
        .unwrap()
}

fn work(library: &Library, key: &str) -> LibraryId {
    let id: Vec<u8> = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT entity_id FROM external_identities WHERE kind='work' AND external_key=?1",
            [key],
            |r| r.get(0),
        )
        .unwrap();
    LibraryId::from_slice(&id).unwrap()
}

#[test]
fn phone_reading_positions_move_as_pages_without_guessing_and_favorite_copies_are_checked() {
    let fixture = Fixture::new();
    let secret = b"device token hash that must stay in the server folder";
    Connection::open(fixture.0.join("old/server/state.db"))
        .unwrap()
        .execute(
            "INSERT INTO devices VALUES('phone','Phone','reader',?1,'2026-01-01',NULL)",
            [secret.as_slice()],
        )
        .unwrap();
    let (library, report) = fixture.import();

    // 本文の [newpage] で数えた 2 ページ目の 4 分の 1。文字の位置へ直さない。
    let paged = library
        .reading_position(work(&library, "456"), library.id)
        .unwrap();
    let Some(ReadingLocation::Source { anchor, .. }) = paged.location else {
        panic!("{paged:?}");
    };
    assert_eq!(
        anchor,
        ReadingAnchor::Page {
            page: 1,
            ratio_ppm: 250_000
        }
    );
    assert_eq!(paged.device, Some(device_id("phone")));
    assert_eq!(paged.updated_at, Some(1_700_000_000_000));
    assert_eq!(paged.generation, 1);

    // 編集の印は、同じ中身の編集版へ結ぶ。page_break で 2 ページになる。
    let edited = library
        .reading_position(work(&library, "123"), library.id)
        .unwrap();
    assert!(edited.finished);
    assert!(matches!(
        edited.location,
        Some(ReadingLocation::Edit {
            anchor: ReadingAnchor::Page {
                page: 1,
                ratio_ppm: PAGE_RATIO_SCALE
            },
            ..
        })
    ));

    // 本文にないページと、棚にない作品は推測せず未解決に残す。
    assert_eq!(status(&library, READING, "pixiv:456"), "imported");
    assert_eq!(status(&library, READING, "pixiv:123"), "imported");
    assert_eq!(status(&library, READING, "pixiv:777"), "unresolved");
    assert_eq!(status(&library, READING, "pixiv:999"), "unresolved");
    assert_eq!(
        library
            .reading_position(work(&library, "777"), library.id)
            .unwrap()
            .location,
        None
    );

    // お気に入りの変更は本棚の値の写し。食い違う行だけが残る。消えた作品の変更は当て先がない。
    assert_eq!(status(&library, FAVORITES, "pixiv:456"), "imported");
    assert_eq!(status(&library, FAVORITES, "pixiv:123"), "unresolved");
    assert_eq!(status(&library, FAVORITES, "pixiv:gone"), "imported");

    // 端末の登録と鍵は読まず、棚へ写さない。置き場そのものは済んだものとする。
    assert_eq!(
        status(&library, "__excluded_entries__", "server"),
        "imported"
    );
    assert!(library.file(Fingerprint::of(secret)).is_err());
    assert_eq!(report.unresolved_records, 3, "{:?}", report.unresolved);
    library.verify().unwrap();
}

#[test]
fn a_server_folder_without_user_rows_needs_nothing_and_unknown_entries_stay_unresolved() {
    let fixture = Fixture::new();
    let state = Connection::open(fixture.0.join("old/server/state.db")).unwrap();
    state
        .execute_batch("DELETE FROM reading; DELETE FROM favorite_changes;")
        .unwrap();
    drop(state);
    let (library, report) = fixture.import();
    assert_eq!(report.unresolved_records, 0, "{:?}", report.unresolved);
    drop(library);
    fs::remove_dir_all(fixture.0.join("new")).unwrap();

    fs::write(fixture.0.join("old/server/private.key"), b"unknown").unwrap();
    let (library, _) = fixture.import();
    assert_eq!(
        status(&library, "__excluded_entries__", "server"),
        "unresolved"
    );
}

#[test]
fn legacy_devices_map_to_the_same_new_device_every_time() {
    assert_eq!(device_id("phone"), device_id("phone"));
    assert_ne!(device_id("phone"), device_id("tablet"));
    assert_eq!(device_id("phone").get_version_num(), 8);
}
