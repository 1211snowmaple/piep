use super::{
    store::{regular_directory, sync_directory},
    Error, Fingerprint, Library, LibraryId, Result,
};
use std::fs;
use std::path::Path;

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
        let stage = parent.join(format!(".piep-backup-{}.part", LibraryId::now_v7()));
        // 作成した瞬間から、コピー先を通常の棚としては開けない。
        let backup = Library::create_backup_stage(&stage, self.id)?;
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
        drop(backup);
        if destination.try_exists()? {
            return Err(Error::UnsafePath(destination.into()));
        }
        fs::rename(&stage, destination)?;
        sync_directory(parent)?;
        fs::rename(
            destination.join(".backup-marker.json"),
            destination.join("library.json"),
        )?;
        sync_directory(destination)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind};
    use std::collections::BTreeMap;
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
        let stage = fs::read_dir(&root)
            .unwrap()
            .map(|p| p.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".piep-backup-")
            })
            .unwrap();
        assert!(!stage.join("library.json").exists());
        assert!(stage.join(".backup-marker.json").exists());
        assert!(Library::open(&stage).is_err());
        assert!(Library::create(&stage).is_err());
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
