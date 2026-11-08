use super::{rows::mark_row, LegacyImporter};
use crate::library::{Error, Library, Result};
use rusqlite::params;
use std::sync::atomic::{AtomicBool, Ordering};

impl LegacyImporter {
    pub(super) fn normalize_work_check_state(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if !self.has_column("downloads", "last_deep_checked_at")? {
            return Ok(());
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        self.each_row("downloads",|row| {
            if cancel.load(Ordering::Acquire) { return Err(Error::Canceled) }
            let work=self.work_from_old(&tx,row.integer("id")?)?;
            let date=row.optional_time("last_deep_checked_at")?;
            tx.execute("INSERT INTO source_entity_state(entity_id,last_deep_checked_at) VALUES(?1,?2) ON CONFLICT(entity_id) DO UPDATE SET last_deep_checked_at=excluded.last_deep_checked_at",params![work.as_bytes().as_slice(),date])?;
            mark_row(&tx,"download_source_state",&row.integer("id")?.to_string(),work)
        })?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{EntityKind, LibraryId};
    use rusqlite::Connection;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(second_date: Option<&str>) -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-work-state-{}", LibraryId::now_v7()));
            fs::create_dir_all(root.join("old")).unwrap();
            let db = Connection::open(root.join("old/piep.db")).unwrap();
            db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER,last_deep_checked_at TEXT);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);").unwrap();
            for (id, date) in [(1, Some("2025-01-02T03:04:05.123+09:00")), (2, second_date)] {
                let relative = format!("downloads/pixiv/{id}/v1/original.json");
                let path = root.join("old").join(&relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path,serde_json::to_vec(&serde_json::json!({"detail":{"title":format!("title {id}")},"text":"body"})).unwrap()).unwrap();
                db.execute(
                    "INSERT INTO downloads VALUES(?1,'pixiv',?2,?3,'author',0,?4)",
                    params![id, id.to_string(), format!("title {id}"), date],
                )
                .unwrap();
                db.execute(
                    "INSERT INTO download_versions VALUES(?1,?1,1,?2,?2,'source')",
                    params![id, relative],
                )
                .unwrap();
            }
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn work_check_dates_keep_nulls_and_survive_retry_and_independent_backup_without_changing_history(
    ) {
        let fixture = Fixture::new(None);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let first = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(first.unresolved.is_empty(), "{:?}", first.unresolved);
        let works = library.works(10).unwrap();
        for work in &works {
            let state = library.source_entity_state(work.id).unwrap();
            assert_eq!(state.last_checked_at, None);
            assert_eq!(state.last_fetched_at, None);
            assert_eq!(
                state.last_deep_checked_at,
                if work.title == "title 1" {
                    Some(1735754645123)
                } else {
                    None
                }
            );
        }
        let retry = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(retry.unresolved.is_empty());
        for work in &works {
            assert_eq!(library.current_snapshot(work.id).unwrap(), work.snapshot_id);
            assert_eq!(library.snapshots(work.id).unwrap().len(), 1);
        }
        library.backup(&fixture.0.join("backup")).unwrap();
        let backup = Library::open(&fixture.0.join("backup")).unwrap();
        for work in &works {
            assert_eq!(
                backup.source_entity_state(work.id).unwrap(),
                library.source_entity_state(work.id).unwrap()
            );
        }
        let unknown = library
            .begin_acquisition(
                LibraryId::now_v7(),
                "pixiv",
                EntityKind::Person,
                "unsaved",
                "offline-test",
            )
            .unwrap();
        assert_eq!(
            library
                .source_entity_state(unknown.ticket.entity)
                .unwrap()
                .last_deep_checked_at,
            None
        );
        assert!(library.source_entity_state(LibraryId::now_v7()).is_err());
        library.verify().unwrap();
    }

    #[test]
    fn an_unreadable_check_date_rolls_back_all_work_state_and_remains_a_blocking_migration_problem()
    {
        let fixture = Fixture::new(Some("not a date"));
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(!report.unresolved.is_empty());
        assert!(library
            .activation_problems()
            .unwrap()
            .iter()
            .any(|p| p == "__normalization__: work_check_state"));
        let count: i64 = library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM source_entity_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(library.works(10).unwrap().len(), 2);
        library.verify().unwrap();
    }
}
