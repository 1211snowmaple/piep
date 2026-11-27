use super::{
    publish::{is_stage_name, StagedCopy, BACKUP_STAGE_PREFIX},
    repository::{CREATION_MARKER, LIBRARY_MARKER},
    store::regular_directory,
    Error, Fingerprint, Library, Result, PART_MARKER,
};
use std::fs::{self, OpenOptions};
use std::path::Path;

fn is_backup_stage(name: &str) -> bool {
    is_stage_name(name, BACKUP_STAGE_PREFIX)
}

impl Library {
    /// 自立したバックアップを別ルートへ作る。元の棚を変更せず、未完成の棚を公開しない。
    pub fn backup(&self, destination: &Path) -> Result<()> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::UnsafePath(destination.into()))?;
        regular_directory(parent)?;
        if destination.try_exists()? {
            return Err(Error::UnsafePath(destination.into()));
        }
        // 同じ親に書く別のバックアップが、作業中のフォルダーを消さないようにする。
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(".piep-backup.lock"))?;
        lock.lock()?;
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_name().to_str().is_some_and(is_backup_stage)
                && regular_directory(&path).is_ok()
                && !path.join(LIBRARY_MARKER).try_exists()?
                && !path.join(PART_MARKER).try_exists()?
                && !path.join(CREATION_MARKER).try_exists()?
            {
                fs::remove_dir_all(path)?;
            }
        }
        // 写す途中で、写し元の台帳が参照しなくなった実体を回収されないようにする。
        let _turn = self.storage_gate.lock().map_err(|_| Error::Busy)?;
        let stage = StagedCopy::new(destination, BACKUP_STAGE_PREFIX)?;
        self.write_backup(&stage.path())?;
        stage.publish(self.id, LIBRARY_MARKER)
    }

    fn write_backup(&self, stage: &Path) -> Result<()> {
        let backup = Library::create_staged_copy(stage, self.id)?;
        {
            let source = self.conn.lock().map_err(|_| Error::Busy)?;
            let mut target = backup.conn.lock().map_err(|_| Error::Busy)?;
            let copying = rusqlite::backup::Backup::new(&source, &mut target)?;
            copying.run_to_completion(128, std::time::Duration::from_millis(5), None)?;
        }
        // コピー先 DB の固定された参照を読む。実体は不変なので元の書き手を長時間止めない。
        let files = {
            let conn = backup.conn.lock().map_err(|_| Error::Busy)?;
            let mut stmt = conn.prepare("SELECT id FROM content_files ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
            let mut files = Vec::new();
            for row in rows {
                let bytes: [u8; 32] = row?
                    .try_into()
                    .map_err(|_| Error::Integrity("ファイル識別子が不正です".into()))?;
                files.push(Self::file_from_conn(&conn, Fingerprint(bytes))?);
            }
            files
        };
        for file in &files {
            self.objects.replicate(file, &backup.objects)?;
        }
        backup.verify()?;
        {
            let conn = backup.conn.lock().map_err(|_| Error::Busy)?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind, LibraryId};
    use std::collections::BTreeMap;

    #[test]
    fn interrupted_publish_of_backup_can_be_retried() {
        let root = std::env::temp_dir().join(format!("piep-backup-test-{}", LibraryId::now_v7()));
        fs::create_dir(&root).unwrap();
        let source = Library::create(&root.join("source")).unwrap();
        let destination = root.join("backup");

        crate::library::publish::tests::interrupt_next_publish(LIBRARY_MARKER);
        assert!(matches!(source.backup(&destination), Err(Error::Canceled)));
        assert!(!destination.exists());

        source.backup(&destination).unwrap();
        Library::open_matching(&destination, source.id)
            .unwrap()
            .verify()
            .unwrap();
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failed_backup_never_exposes_a_completed_library() {
        let root = std::env::temp_dir().join(format!("piep-backup-test-{}", LibraryId::now_v7()));
        fs::create_dir(&root).unwrap();
        let original = root.join("source");
        let destination = root.join("copy");
        let source = Library::create(&original).unwrap();
        let file = source
            .store_original(&b"test original".to_vec().into())
            .unwrap();
        let hash = file.chunks[0].id.to_string();
        // テスト専用の棚で実体破損を再現する。稼働棚には触れない。
        fs::write(
            original
                .join("objects/v1")
                .join(&hash[..2])
                .join(format!("{hash}.obj")),
            b"damaged",
        )
        .unwrap();
        assert!(source.backup(&destination).is_err());
        assert!(!destination.exists());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_str()
            .is_some_and(is_backup_stage)));
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_new_backup_removes_only_stale_backup_stage_directories() {
        let root = std::env::temp_dir().join(format!("piep-backup-test-{}", LibraryId::now_v7()));
        fs::create_dir(&root).unwrap();
        let source = Library::create(&root.join("source")).unwrap();
        let stale = root.join(format!(".piep-backup-{}.part", LibraryId::now_v7()));
        let ready = stale.join("copy");
        fs::create_dir_all(&ready).unwrap();
        Library::write_format_marker(&ready.join(LIBRARY_MARKER), source.id).unwrap();
        let unrelated = [
            root.join("other.part"),
            root.join(".piep-backup-not-an-id.part"),
            root.join(".piep-backup-00000000-0000-4000-8000-000000000000.part"),
            root.join(format!(".piep-part-{}.part", LibraryId::now_v7())),
        ];
        for path in &unrelated {
            fs::create_dir(path).unwrap();
        }
        let file = root.join(format!(".piep-backup-{}.part", LibraryId::now_v7()));
        fs::write(&file, b"keep").unwrap();
        let published = root.join(format!(".piep-backup-{}.part", LibraryId::now_v7()));
        source.backup(&published).unwrap();

        let destination = root.join("copy");
        source.backup(&destination).unwrap();
        assert!(!stale.exists());
        assert!(unrelated.iter().all(|path| path.is_dir()));
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        Library::open(&published).unwrap().verify().unwrap();
        Library::open(&destination).unwrap().verify().unwrap();
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_backup_reopens_without_its_source_and_preserves_original_files() {
        let root = std::env::temp_dir().join(format!("piep-backup-test-{}", LibraryId::now_v7()));
        fs::create_dir(&root).unwrap();
        let original = root.join("source");
        let destination = root.join("copy");
        let source = Library::create(&original).unwrap();
        let saved = source
            .capture(Capture {
                acquired_at: None,
                request_id: LibraryId::now_v7(),
                provider: "local".into(),
                external_key: "one".into(),
                kind: EntityKind::Work,
                route: "test".into(),
                raw: b"{ \"unchanged\": true }\r\n".to_vec().into(),
                metadata: BTreeMap::from([("title".into(), serde_json::json!("試験"))]),
                field_states: BTreeMap::new(),
                body: Some("完全に戻る本文\r\n".into()),
                media: vec![],
                attachments: vec![],
            })
            .unwrap();
        source.backup(&destination).unwrap();
        assert!(source.backup(&destination).is_err());
        drop(source);
        fs::rename(&original, root.join("unavailable-source")).unwrap();
        {
            let copy = Library::open(&destination).unwrap();
            assert_eq!(copy.works(10).unwrap()[0].title, "試験");
            let mut bytes = Vec::new();
            copy.read_body(saved.entity_id, saved.snapshot_id, &mut bytes)
                .unwrap();
            assert_eq!(bytes, "完全に戻る本文\r\n".as_bytes());
            copy.verify().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
