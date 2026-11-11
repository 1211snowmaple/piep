use super::{rows::mark_table, LegacyImporter};
use crate::library::{Error, Library, Result};
use rusqlite::params;
use std::path::{Component, Path, PathBuf};

impl LegacyImporter {
    /// パスによる所属判定は移行器に閉じ込める。複数版が同じ場所を使う場合は推測しない。
    pub(super) fn asset_in_version(&self, work: i64, asset: &str, json: &str) -> Result<bool> {
        let absolute = |stored: &str| -> Result<PathBuf> {
            let path = Path::new(stored);
            if path.components().any(|c| matches!(c, Component::ParentDir)) {
                return Err(Error::UnsafePath(path.into()));
            }
            Ok(if path.is_absolute() {
                path.into()
            } else {
                self.root.join(path)
            })
        };
        let asset = absolute(asset)?;
        let selected = absolute(json)?;
        let mut stmt=self.conn.prepare("SELECT coalesce(original_json_path,json_path) FROM download_versions WHERE download_id=?1")?;
        let mut matching = Vec::new();
        for row in stmt.query_map([work], |r| r.get::<_, String>(0))? {
            let json = absolute(&row?)?;
            let dir = json
                .parent()
                .ok_or_else(|| Error::UnsafePath(json.clone()))?;
            let Ok(relative) = asset.strip_prefix(dir) else {
                continue;
            };
            let isolated = |path: &Path| {
                path.file_name().and_then(|n| n.to_str()).is_some_and(|s| {
                    s.strip_prefix('v')
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
            };
            if !isolated(dir)
                && relative
                    .components()
                    .next()
                    .is_some_and(|c| isolated(Path::new(c.as_os_str())))
            {
                continue;
            }
            matching.push(json);
        }
        if !matching.contains(&selected) {
            return Ok(false);
        }
        if matching.len() != 1 {
            return Err(Error::Integrity(format!(
                "作品 {work} の画像・添付の所属版を一意に決められません"
            )));
        }
        Ok(true)
    }

    /// DB に登録された全資産を照合する。どの版・編集にも属さない実体も原本として保管する。
    pub(super) fn normalize_asset_inventory(&self, target: &Library) -> Result<()> {
        let mut unresolved = 0;
        self.each_row("assets",|row| {
            let old=row.integer("id")?;
            let path=self.managed_path(row.text("local_path")?)?;
            let size=row.integer("file_size_bytes")?;
            if size<0 || std::fs::metadata(&path)?.len()!=size as u64 {return Err(Error::Integrity(format!("元ファイル {old} の容量が記録と違います")));}
            let prepared=target.store_path(&path)?;
            let file=prepared.file.clone();
            let mut conn=target.conn.lock().map_err(|_|Error::Busy)?;
            let tx=conn.transaction()?;
            let work=self.work_from_old(&tx,row.integer("download_id")?)?;
            Library::register_file(&tx,&prepared)?;
            let assigned:bool=tx.query_row("SELECT EXISTS(
                SELECT 1 FROM migration_records p JOIN revision_snapshots s ON s.id=p.target_id JOIN snapshot_media m ON m.snapshot_id=s.id JOIN media_renditions r ON r.id=m.rendition_id WHERE p.source_table='asset_placements' AND p.source_key=?3 AND p.status='imported' AND p.evidence_file_id=?2 AND s.entity_id=?1 AND r.file_id=?2 AND s.published=1
                UNION ALL SELECT 1 FROM migration_records p JOIN revision_snapshots s ON s.id=p.target_id JOIN snapshot_files f ON f.snapshot_id=s.id WHERE p.source_table='asset_placements' AND p.source_key=?3 AND p.status='imported' AND p.evidence_file_id=?2 AND s.entity_id=?1 AND f.file_id=?2 AND s.published=1
                UNION ALL SELECT 1 FROM migration_records p JOIN edit_revisions e ON e.id=p.target_id JOIN edit_media m ON m.edit_id=e.id WHERE p.source_table='edit_asset_placements' AND p.source_key=?3 AND p.status='imported' AND p.evidence_file_id=?2 AND e.work_id=?1 AND m.file_id=?2 AND m.slot=('legacy-asset:'||?3) AND e.sealed=1
            )",params![work.as_bytes().as_slice(),file.id.0.as_slice(),old.to_string()],|r|r.get(0))?;
            if assigned { self.mark_used_file(&tx,&path,&file.id.0)?; }
            tx.execute("INSERT INTO migration_records(source_table,source_key,target_id,evidence_file_id,status) VALUES('assets',?1,?2,?3,?4) ON CONFLICT(source_table,source_key) DO UPDATE SET target_id=excluded.target_id,evidence_file_id=excluded.evidence_file_id,status=excluded.status",params![old.to_string(),work.as_bytes().as_slice(),file.id.0.as_slice(),if assigned {"imported"}else{"unresolved"}])?;
            if !assigned {unresolved+=1;}
            tx.commit()?;
            Ok(())
        })?;
        if unresolved != 0 {
            return Err(Error::Integrity(format!(
                "{unresolved} 件の元ファイルは保管しましたが、所属する保存版・編集を特定できません"
            )));
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        mark_table(&tx, "assets")?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Fingerprint, LibraryId};
    use std::{fs, sync::atomic::AtomicBool};

    #[test]
    fn old_save_dates_are_imported_without_changing_the_version_order() {
        let fixture = Fixture::new();
        fs::write(
            fixture
                .0
                .join("old/downloads/pixiv/one/v2/data_assets/cover.png"),
            b"two",
        )
        .unwrap();
        let conn = rusqlite::Connection::open(fixture.0.join("old/piep.db")).unwrap();
        conn.execute_batch("ALTER TABLE download_versions ADD COLUMN created_at TEXT; UPDATE download_versions SET created_at=CASE WHEN version=1 THEN '2025-01-01T00:00:00+09:00' ELSE '2024-01-01T00:00:00+09:00' END").unwrap();
        drop(conn);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(1).unwrap().remove(0);
        let snapshots = library.snapshots(work.id).unwrap();
        assert_eq!(snapshots[0].sequence, 1);
        assert_eq!(snapshots[1].sequence, 2);
        assert!(snapshots[0].saved_at > snapshots[1].saved_at);
        assert_eq!(
            snapshots[0].saved_at,
            crate::library::legacy::rows::parse_time("2025-01-01T00:00:00+09:00").unwrap()
        );
        assert_eq!(snapshots[1].id, work.snapshot_id);
        library.verify().unwrap();
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("piep-version-inventory-{}", LibraryId::now_v7()));
            let old = root.join("old");
            fs::create_dir_all(old.join("downloads/pixiv/one/v2/data_assets")).unwrap();
            fs::create_dir_all(old.join("downloads/pixiv/one/data_assets")).unwrap();
            let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
            conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
INSERT INTO downloads VALUES(1,'pixiv','one','title','author',0);").unwrap();
            for (version, dir) in [(1, "downloads/pixiv/one"), (2, "downloads/pixiv/one/v2")] {
                let path = format!("{dir}/original.json");
                fs::write(old.join(&path),serde_json::json!({"detail":{"title":"title"},"text":format!("version {version}")}).to_string()).unwrap();
                conn.execute(
                    "INSERT INTO download_versions VALUES(?1,1,?1,?2,?2,'source')",
                    params![version, path],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO assets VALUES(?1,1,'cover','cover.png',?2,NULL,'image/png',3)",
                    params![version, format!("{dir}/data_assets/cover.png")],
                )
                .unwrap();
            }
            fs::write(
                old.join("downloads/pixiv/one/data_assets/cover.png"),
                b"one",
            )
            .unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn flat_first_version_never_takes_a_later_image_and_repairing_that_image_resumes() {
        let fixture = Fixture::new();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(report.snapshots, 1);
        assert!(!report.unresolved.is_empty());
        let work = library.works(1).unwrap().remove(0);
        let old_snapshot = work.snapshot_id;
        let cover = library.media_file(work.id, old_snapshot, "cover").unwrap();
        assert_eq!(cover.id, Fingerprint::of(b"one"));
        drop(source);
        fs::write(
            fixture
                .0
                .join("old/downloads/pixiv/one/v2/data_assets/cover.png"),
            b"two",
        )
        .unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert!(library.activation_problems().unwrap().is_empty());
        let current = library.works(1).unwrap().remove(0);
        assert_eq!(library.snapshots(work.id).unwrap().len(), 2);
        assert_ne!(current.snapshot_id, old_snapshot);
        assert_eq!(
            library
                .media_file(work.id, current.snapshot_id, "cover")
                .unwrap()
                .id,
            Fingerprint::of(b"two")
        );
        assert_eq!(
            library
                .media_file(work.id, old_snapshot, "cover")
                .unwrap()
                .id,
            Fingerprint::of(b"one")
        );
    }

    #[test]
    fn unassigned_files_are_preserved_and_prevent_activation() {
        let fixture = Fixture::new();
        fs::write(
            fixture
                .0
                .join("old/downloads/pixiv/one/v2/data_assets/cover.png"),
            b"two",
        )
        .unwrap();
        fs::write(
            fixture.0.join("old/downloads/unassigned.png"),
            b"orphan image\0ICC",
        )
        .unwrap();
        let conn = rusqlite::Connection::open(fixture.0.join("old/piep.db")).unwrap();
        conn.execute("INSERT INTO assets VALUES(99,1,'editor_image','unassigned.png','downloads/unassigned.png',NULL,'image/png',16)",[]).unwrap();
        fs::write(fixture.0.join("old/downloads/duplicate.png"), b"one").unwrap();
        conn.execute("INSERT INTO assets VALUES(100,1,'editor_image','duplicate.png','downloads/duplicate.png',NULL,'image/png',3)",[]).unwrap();
        drop(conn);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(!report.unresolved.is_empty());
        let conn = library.conn.lock().unwrap();
        let (hash,status):(Vec<u8>,String)=conn.query_row("SELECT evidence_file_id,status FROM migration_records WHERE source_table='assets' AND source_key='99'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(hash, Fingerprint::of(b"orphan image\0ICC").0);
        assert_eq!(status, "unresolved");
        assert_eq!(conn.query_row("SELECT status FROM migration_records WHERE source_table='assets' AND source_key='100'",[],|r|r.get::<_,String>(0)).unwrap(),"unresolved");
        drop(conn);
        let file = library.file(Fingerprint(hash.try_into().unwrap())).unwrap();
        let mut bytes = Vec::new();
        library.objects.export(&file, &mut bytes).unwrap();
        assert_eq!(bytes, b"orphan image\0ICC");
        library.verify().unwrap();
    }

    #[test]
    fn rechecking_a_changed_source_file_records_failure_and_can_recover_without_reapplying_history()
    {
        let fixture = Fixture::new();
        fs::write(
            fixture
                .0
                .join("old/downloads/pixiv/one/v2/data_assets/cover.png"),
            b"two",
        )
        .unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        let work = library.works(1).unwrap().remove(0);
        drop(source);
        let path = fixture
            .0
            .join("old/downloads/pixiv/one/data_assets/cover.png");
        fs::write(&path, b"two").unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        assert!(!source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        assert!(!library.activation_problems().unwrap().is_empty());
        assert_eq!(library.current_snapshot(work.id).unwrap(), work.snapshot_id);
        library.verify().unwrap();
        drop(source);
        fs::write(&path, b"one").unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert!(library.activation_problems().unwrap().is_empty());
        assert_eq!(library.snapshots(work.id).unwrap().len(), 2);
        assert_eq!(library.current_snapshot(work.id).unwrap(), work.snapshot_id);
    }
}
