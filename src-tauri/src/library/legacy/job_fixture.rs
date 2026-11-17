//! 旧ジョブを持つ試験用の棚。保管層とアプリの検証で同じ原記録を使う。
use crate::library::{LegacyImporter, Library, LibraryId};
use rusqlite::{params, Connection};
use std::{path::PathBuf, sync::atomic::AtomicBool};
pub(crate) struct Fixture(pub(crate) PathBuf);
impl Fixture {
    pub(crate) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-job-history-{}", LibraryId::now_v7()));
        let version = root.join("old/downloads/pixiv/one/v1");
        std::fs::create_dir_all(&version).unwrap();
        let json = version.join("original.json");
        std::fs::write(
            &json,
            b"{\"detail\":{\"title\":\"book\"},\"text\":\"body\"}",
        )
        .unwrap();
        let db = Connection::open(root.join("old/piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE update_jobs(id TEXT PRIMARY KEY,scope TEXT,mode TEXT,status TEXT,watch_saved INTEGER,new_works_only INTEGER,totals INTEGER,processed INTEGER,candidate_count INTEGER,saved_count INTEGER,error_count INTEGER,active_label TEXT,started_at TEXT,updated_at TEXT,finished_at TEXT);
CREATE TABLE update_job_items(id INTEGER PRIMARY KEY,job_id TEXT,item_type TEXT,source TEXT,source_id TEXT,target_type TEXT,title TEXT,payload_json TEXT,status TEXT,error TEXT,result_download_id INTEGER,created_at TEXT,updated_at TEXT);
CREATE TABLE update_job_logs(id INTEGER PRIMARY KEY,job_id TEXT,log_type TEXT,message TEXT,created_at TEXT);
INSERT INTO downloads VALUES(1,'pixiv','one','book','author',0);
INSERT INTO update_jobs VALUES('job-z','author','auto','completed',1,0,100,7,3,1,2,'当時の表示','2025-01-01 00:00:00','2025-01-02 00:00:00','2025-01-02 00:00:00');
INSERT INTO update_jobs VALUES('job-a','work','manual','completed',0,1,0,0,0,0,0,NULL,'2025-01-02 00:00:00','2025-01-02 00:00:00',NULL);
INSERT INTO update_job_logs VALUES(9,'job-z','error','後の記録','2025-01-02 00:00:00');
INSERT INTO update_job_logs VALUES(2,'job-z','info','先の記録','2025-01-02 00:00:00');
INSERT INTO update_job_logs VALUES(1,'job-a','future-type','原文を保つ','2025-01-02 00:00:00');").unwrap();
        db.execute(
            "INSERT INTO download_versions VALUES(1,1,1,?1,?1,'source')",
            [json.to_str().unwrap()],
        )
        .unwrap();
        for (id, kind, key, target, status, result, error) in [
            (10, "candidate", "one", "author", "saved", Some(1), None),
            (3, "target", "42", "author", "done", None, None),
            (
                20,
                "candidate",
                "held",
                "work",
                "held",
                None,
                Some("制限の理由"),
            ),
        ] {
            db.execute("INSERT INTO update_job_items VALUES(?1,'job-z',?2,'pixiv',?3,?4,?5,?6,?7,?8,?9,'2025-01-01 00:00:00','2025-01-02T00:00:00+09:00')",params![id,kind,key,target,format!("当時の題名 {id}"),payload(),status,error,result]).unwrap();
        }
        Self(root)
    }
    pub(crate) fn change(&self, sql: &str) {
        Connection::open(self.0.join("old/piep.db"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(crate) fn payload() -> &'static str {
    "{ \"future\": {\"decimal\":0.07221960411257732,\"integer\":9007199254740993},\"knownHold\":true }\r\n"
}

pub(crate) fn prepare(fixture: &Fixture, status: &str) {
    fixture.change(&format!(
        "UPDATE update_jobs SET status='{status}',mode='check_only' WHERE id='job-z'"
    ));
}

pub(crate) fn add_pending(fixture: &Fixture, status: &str) {
    Connection::open(fixture.0.join("old/piep.db")).unwrap().execute("INSERT INTO update_job_items VALUES(30,'job-z','candidate','pixiv','new','work','まだの題名',?1,?2,NULL,NULL,'2025-01-01 00:00:00','2025-01-02T00:00:00+09:00')",params![payload(),status]).unwrap();
}

pub(crate) fn moved(library: &Library, key: &str) -> LibraryId {
    LibraryId::from_slice(&library.conn.lock().unwrap().query_row("SELECT target_id FROM migration_records WHERE source_table='update_jobs' AND source_key=?1",[key],|r|r.get::<_,Vec<u8>>(0)).unwrap()).unwrap()
}

pub(crate) fn successful(source: &LegacyImporter, library: &Library) {
    let report = source
        .import(library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert_eq!(report.unresolved_records, 0, "{:?}", report.unresolved);
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
}

pub(crate) fn raw_payload(library: &Library, sequence: i64) -> String {
    let conn = library.conn.lock().unwrap();
    let hash = conn
        .query_row(
            "SELECT payload_file_id FROM execution_items WHERE source_sequence=?1",
            [sequence],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .unwrap();
    library
        .read_job_payload(&conn, crate::library::Fingerprint(hash.try_into().unwrap()))
        .unwrap()
}

pub(crate) fn permission(fixture: &Fixture, decision: &str) -> String {
    prepare(fixture, "paused");
    fixture.change(&format!("CREATE TABLE update_candidates(source TEXT,source_id TEXT,kind TEXT,title TEXT,payload_json TEXT,target_type TEXT,status TEXT,first_seen_at TEXT,updated_at TEXT); INSERT INTO update_candidates VALUES('pixiv','one','revision','保留の題','{{\"holdKind\":\"incomplete\"}}','work','{decision}','2025-01-01 00:00:00','2025-01-02 00:00:00')"));
    let raw=" { \"kind\":\"permission_check\",\"holdKind\":\"incomplete\",\"decimal\":1.2300,\"future\":9007199254740993 } ";
    Connection::open(fixture.0.join("old/piep.db")).unwrap().execute("UPDATE update_job_items SET status='queued',result_download_id=NULL,payload_json=?1 WHERE id=10",[raw]).unwrap();
    raw.into()
}
