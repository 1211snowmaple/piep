use super::*;
use crate::library::{Fingerprint, LibraryId};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-filesystem-{}", LibraryId::now_v7()));
        let old = root.join("old");
        let dir = old.join("downloads/pixiv/one/v1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("original.json"),
            br#"{"detail":{"title":"title"},"text":"body"}"#,
        )
        .unwrap();
        fs::write(dir.join("cover.png"), b"exact original image").unwrap();
        let db = Connection::open(old.join("piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
INSERT INTO downloads VALUES(1,'pixiv','one','title','author',0);
INSERT INTO download_versions VALUES(1,1,1,'downloads/pixiv/one/v1/original.json','downloads/pixiv/one/v1/original.json','source');
INSERT INTO assets VALUES(1,1,'cover','cover.png','downloads/pixiv/one/v1/cover.png',NULL,'image/png',20);").unwrap();
        Self(root)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join("old/downloads/pixiv/one/v1").join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn bytes(library: &Library, id: Fingerprint) -> Vec<u8> {
    let mut bytes = Vec::new();
    library
        .objects
        .export(&library.file(id).unwrap(), &mut bytes)
        .unwrap();
    bytes
}
fn record(library: &Library, table: &str, key: &str) -> (String, Option<Fingerprint>) {
    library.conn.lock().unwrap().query_row("SELECT status,evidence_file_id FROM migration_records WHERE source_table=?1 AND source_key=?2",params![table,key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<Vec<u8>>>(1)?))).map(|(status,id)|(status,id.map(|b|Fingerprint(b.try_into().unwrap())))).unwrap()
}

#[test]
fn rebuildable_indexes_models_and_old_backups_are_recorded_but_never_stored_as_originals() {
    let fixture = Fixture::new();
    let old = fixture.0.join("old");
    let index = b"segment bytes that the works can rebuild";
    let model = b"model weights that can be downloaded again";
    for (dir, name, bytes) in [
        (
            "downloads/search-index/default",
            "segment.idx",
            index.as_slice(),
        ),
        ("downloads/search/models", "model.onnx", model.as_slice()),
        (
            "downloads/.search-snapshots/one",
            "meta.json",
            b"{}".as_slice(),
        ),
        (
            "backup-current-20261027",
            "piep.db",
            b"an earlier backup".as_slice(),
        ),
    ] {
        fs::create_dir_all(old.join(dir)).unwrap();
        fs::write(old.join(dir).join(name), bytes).unwrap();
    }
    let db = Connection::open(old.join("piep.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE search_index_state(download_id INTEGER,current_version INTEGER);
INSERT INTO search_index_state VALUES(1,1);
CREATE TABLE search_feature_settings(singleton_id INTEGER,semantic_enabled INTEGER);
INSERT INTO search_feature_settings VALUES(1,1);",
    )
    .unwrap();
    drop(db);
    let source = LegacyImporter::open(&old).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    // 索引・モデル・以前のバックアップは読まず、理由だけを残す。
    for key in [
        "downloads/search-index",
        "downloads/search",
        "downloads/.search-snapshots",
        "backup-current-20261027",
    ] {
        let (status, evidence) = record(&library, "__excluded_entries__", key);
        assert_eq!(status, "imported", "{key}");
        let evidence = String::from_utf8(bytes(&library, evidence.unwrap())).unwrap();
        assert!(evidence.contains("\"contents_read\":false"), "{evidence}");
    }
    assert_eq!(report.unresolved_files, 0, "{:?}", report.unresolved);
    assert_eq!(report.managed_files, 2);
    for content in [index.as_slice(), model.as_slice()] {
        assert!(library.file(Fingerprint::of(content)).is_err());
    }
    // 索引の状態は行を保管しない。利用者が選んだ設定は行を保管するが、棚のデータではないので
    // 有効化を止めない（アプリの設定として引き継ぐ）。
    assert_eq!(
        report.rebuildable_tables.get("search_index_state"),
        Some(&1)
    );
    assert!(!report.preserved_tables.contains_key("search_index_state"));
    assert_eq!(
        record(&library, "search_index_state", "__all_rows__").0,
        "imported"
    );
    assert_eq!(
        report.preserved_tables.get("search_feature_settings"),
        Some(&1)
    );
    assert_eq!(
        record(&library, "search_feature_settings", "__all_rows__").0,
        "imported"
    );
    assert!(library.activation_problems().unwrap().is_empty());
    library.verify().unwrap();
    // 旧アプリを使って索引が進んでも、やり直しを止めない。
    let db = Connection::open(old.join("piep.db")).unwrap();
    db.execute("INSERT INTO search_index_state VALUES(2,1)", [])
        .unwrap();
    drop(db);
    drop(source);
    let source = LegacyImporter::open(&old).unwrap();
    let again = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(again.rebuildable_tables.get("search_index_state"), Some(&2));
    assert_eq!(again.unresolved_files, 0);
}

#[test]
fn a_cover_replaced_in_place_is_tied_to_the_version_that_recorded_it_and_nothing_else() {
    // 初回は未ログイン向けの制限画像を受け取り、後の補完が同じ版の記録を本物の表紙へ
    // 書き換えた旧棚の形。制限画像はフォルダーに残り、どの記録からも指されていない。
    let fixture = Fixture::new();
    let old = fixture.0.join("old");
    let version = old.join("downloads/pixiv/one/v1");
    fs::write(
        version.join("original.json"),
        br#"{"detail":{"title":"title","cover_url":"https://s.pximg.net/common/images/limit_r18_100.png"},"text":"body"}"#,
    )
    .unwrap();
    let cover = version.join("data_assets/cover");
    fs::create_dir_all(&cover).unwrap();
    let placeholder = b"restricted placeholder bytes";
    fs::write(cover.join("limit_r18_100.png"), placeholder).unwrap();
    fs::write(
        cover.join("limit_unknown_360.png"),
        b"similar name, not recorded",
    )
    .unwrap();
    let source = LegacyImporter::open(&old).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    let recorded = "downloads/pixiv/one/v1/data_assets/cover/limit_r18_100.png";
    assert_eq!(record(&library, "__filesystem__", recorded).0, "imported");
    let (status, evidence) = record(&library, "__replaced_in_place__", recorded);
    assert_eq!(status, "imported");
    let evidence = String::from_utf8(bytes(&library, evidence.unwrap())).unwrap();
    assert!(
        evidence.contains("restricted_placeholder_replaced_in_place"),
        "{evidence}"
    );
    assert!(
        evidence.contains("downloads/pixiv/one/v1/original.json"),
        "{evidence}"
    );
    assert_eq!(bytes(&library, Fingerprint::of(placeholder)), placeholder);
    // 原本が記録していない名前は、似ていても結ばない。
    assert_eq!(
        record(
            &library,
            "__filesystem__",
            "downloads/pixiv/one/v1/data_assets/cover/limit_unknown_360.png"
        )
        .0,
        "unresolved"
    );
    assert_eq!(report.unresolved_files, 1, "{:?}", report.unresolved);
    library.verify().unwrap();
}

#[test]
fn unregistered_files_are_preserved_without_mistaking_equal_bytes_for_known_ownership() {
    let fixture = Fixture::new();
    fs::write(fixture.file("unregistered.png"), b"exact original image").unwrap();
    let unknown = b"unregistered attachment\0\r\n";
    fs::write(fixture.file("unregistered.bin"), unknown).unwrap();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(report.managed_files, 4);
    assert_eq!(report.unresolved_files, 2);
    let key = "downloads/pixiv/one/v1/unregistered.bin";
    let (status, file) = record(&library, "__filesystem__", key);
    assert_eq!(status, "unresolved");
    assert_eq!(bytes(&library, file.unwrap()), unknown);
    assert_eq!(
        record(
            &library,
            "__filesystem__",
            "downloads/pixiv/one/v1/unregistered.png"
        )
        .0,
        "unresolved"
    );
    assert_eq!(
        record(
            &library,
            "__filesystem__",
            "downloads/pixiv/one/v1/cover.png"
        )
        .0,
        "imported"
    );
    assert_eq!(
        library
            .snapshots(library.works(1).unwrap()[0].id)
            .unwrap()
            .len(),
        1
    );
    library.backup(&fixture.0.join("backup")).unwrap();
    let backup = Library::open(&fixture.0.join("backup")).unwrap();
    assert_eq!(bytes(&backup, file.unwrap()), unknown);
    assert!(!backup.activation_problems().unwrap().is_empty());
    backup.verify().unwrap();
}

#[test]
fn a_stop_after_archiving_rows_and_before_normalization_never_exposes_a_completed_migration() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    source
        .preserve_tables(&library, &mut Default::default())
        .unwrap();
    assert!(library.works(1).unwrap().is_empty());
    assert!(library
        .activation_problems()
        .unwrap()
        .iter()
        .any(|s| s == "__normalization__: migration"));
    drop(library);
    let library = Library::open(&fixture.0.join("new")).unwrap();
    assert!(!library.activation_problems().unwrap().is_empty());
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    assert_eq!(library.works(1).unwrap().len(), 1);
    library.verify().unwrap();
}

#[test]
fn cancellation_keeps_the_shelf_incomplete_and_resume_does_not_duplicate_source_history() {
    let fixture = Fixture::new();
    let db = Connection::open(fixture.0.join("old/piep.db")).unwrap();
    for n in 2..=3 {
        let name = format!("attachment-{n}.bin");
        fs::write(fixture.file(&name), b"document bytes").unwrap();
        db.execute(
            "INSERT INTO assets VALUES(?1,1,'attachment',?2,?3,NULL,NULL,14)",
            params![n, name, format!("downloads/pixiv/one/v1/{name}")],
        )
        .unwrap();
    }
    drop(db);
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let cancel = AtomicBool::new(false);
    assert!(matches!(
        source.import(&library, &cancel, |report| {
            // 進捗通知から DB を読める。書き込み錠を通知まで持ち続けない。
            library.works(1).unwrap();
            if report.managed_files >= 2 {
                cancel.store(true, Ordering::Release)
            }
        }),
        Err(Error::Canceled)
    ));
    assert!(library
        .activation_problems()
        .unwrap()
        .iter()
        .any(|s| s == "__normalization__: migration"));
    let current = library.works(1).unwrap()[0].clone();
    drop(library);
    let library = Library::open(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    assert_eq!(report.managed_files, 4);
    assert_eq!(
        library.current_snapshot(current.id).unwrap(),
        current.snapshot_id
    );
    assert_eq!(library.snapshots(current.id).unwrap().len(), 1);
    let count = library.verify().unwrap();
    let again = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(again.files, count);
    assert!(again.unresolved.is_empty());
}

#[test]
fn changed_or_disappeared_unregistered_files_remain_unresolved_and_each_received_copy_is_kept() {
    let fixture = Fixture::new();
    let name = "unregistered.bin";
    fs::write(fixture.file(name), b"first bytes").unwrap();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    fs::write(fixture.file(name), b"second bytes").unwrap();
    let changed = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(changed.unresolved_files, 1);
    for raw in [b"first bytes".as_slice(), b"second bytes".as_slice()] {
        let id = Fingerprint::of(raw);
        assert_eq!(bytes(&library, id), raw);
        let key =
            serde_json::to_string(&(format!("downloads/pixiv/one/v1/{name}"), id.to_string()))
                .unwrap();
        assert_eq!(record(&library, "__filesystem_copies__", &key).1, Some(id));
    }
    // 検証用の、明示した単一ファイルだけを外す。
    fs::remove_file(fixture.file(name)).unwrap();
    let missing = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(missing.unresolved_files, 1);
    assert_eq!(
        record(
            &library,
            "__filesystem__",
            "downloads/pixiv/one/v1/unregistered.bin"
        )
        .1,
        Some(Fingerprint::of(b"first bytes"))
    );
    library.verify().unwrap();
}

#[test]
fn the_inventory_records_exclusions_and_does_not_copy_credentials_or_unknown_root_contents() {
    let fixture = Fixture::new();
    let secret = format!("test-only credential {}", LibraryId::now_v7()).into_bytes();
    fs::write(fixture.0.join("old/settings.json"), &secret).unwrap();
    for directory in ["browsing-profile", "server", "unexpected"] {
        fs::create_dir_all(fixture.0.join("old").join(directory)).unwrap();
        fs::write(
            fixture.0.join("old").join(directory).join("private"),
            &secret,
        )
        .unwrap();
    }
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(!report.unresolved.is_empty());
    assert!(library.file(Fingerprint::of(&secret)).is_err());
    assert_eq!(
        record(&library, "__excluded_entries__", "browsing-profile").0,
        "imported"
    );
    // アプリの設定は棚へ写さず、理由を残して済んだものとする。端末の状態と知らない項目は残す。
    for (entry, expected) in [
        ("settings.json", "imported"),
        ("server", "unresolved"),
        ("unexpected", "unresolved"),
    ] {
        let (status, file) = record(&library, "__excluded_entries__", entry);
        assert_eq!(status, expected, "{entry}");
        let evidence: serde_json::Value =
            serde_json::from_slice(&bytes(&library, file.unwrap())).unwrap();
        assert_eq!(evidence["entry"], entry);
        assert_eq!(evidence["contents_read"], false);
    }
    library.verify().unwrap();
}

#[test]
fn user_templates_are_kept_by_path_and_do_not_block_activation() {
    let fixture = Fixture::new();
    let template = b"<html>{{title}}</html>";
    fs::create_dir_all(fixture.0.join("old/templates/mine")).unwrap();
    fs::write(fixture.0.join("old/templates/mine/page.xhtml"), template).unwrap();
    // 作品のフォルダーにある、記録にないファイルは、同じ規則で済ませない。
    fs::write(fixture.file("stray.xhtml"), template).unwrap();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    let key = "templates/mine/page.xhtml";
    assert_eq!(record(&library, "__filesystem__", key).0, "imported");
    let (status, file) = record(&library, "__user_files__", key);
    assert_eq!(status, "imported");
    assert_eq!(bytes(&library, file.unwrap()), template);
    assert_eq!(
        record(
            &library,
            "__filesystem__",
            "downloads/pixiv/one/v1/stray.xhtml"
        )
        .0,
        "unresolved"
    );
    assert_eq!(report.unresolved_files, 1, "{:?}", report.unresolved);
    library.verify().unwrap();
}
