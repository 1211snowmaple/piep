use super::{
    rows::{mark_row, mark_table, marked, LegacyRow},
    LegacyImporter,
};
use crate::library::{
    cover_identity, Capture, CaptureSource, CapturedMedia, EntityKind, Error, Library, LibraryId,
    Result,
};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
};

impl LegacyImporter {
    pub(super) fn normalize_profiles(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<usize> {
        let mut versions = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='entity_versions'",
        )?;
        if !versions.exists([])? {
            return Ok(0);
        }
        let mut count = 0;
        // each_row は表の物理順ではなく、版の順を指定して呼ぶ。
        let mut stmt = self.conn.prepare(
            "SELECT id FROM entity_versions ORDER BY entity_type,source,source_key,version,id",
        )?;
        for old in stmt.query_map([], |r| r.get::<_, i64>(0))? {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            self.each_row_matching("entity_versions", "id", old?, |row| {
                self.import_profile_version(target, row)
            })?;
            count += 1;
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        for (table, kind) in [("people", "person"), ("series", "series")] {
            if marked(&tx, table)? {
                continue;
            }
            self.each_row(table,|row| {
                let (source,key)=(row.text("source")?,row.text("source_key")?);
                let version=row.integer("current_version")?;
                let old:i64=self.conn.query_row("SELECT id FROM entity_versions WHERE entity_type=?1 AND source=?2 AND source_key=?3 AND version=?4",params![kind,source,key,version],|r|r.get(0))?;
                let snapshot:Vec<u8>=tx.query_row("SELECT target_id FROM migration_records WHERE source_table='entity_versions' AND source_key=?1 AND status='imported'",[old.to_string()],|r|r.get(0))?;
                let entity:Vec<u8>=tx.query_row("SELECT entity_id FROM external_identities WHERE provider=?1 AND kind=?2 AND external_key=?3",params![source,kind,key],|r|r.get(0))?;
                let current:Vec<u8>=tx.query_row("SELECT snapshot_id FROM entity_current WHERE entity_id=?1",[&entity],|r|r.get(0))?;
                if current!=snapshot {return Err(Error::Integrity(format!("{kind} {key} の現在版が履歴の末尾と一致しません")));}
                let metadata:String=tx.query_row("SELECT metadata_json FROM revision_snapshots WHERE id=?1",[&snapshot],|r|r.get(0))?;
                let metadata:Value=serde_json::from_str(&metadata)?;
                for (old,new) in [("display_name","display_name"),("title","title"),("description","description"),("is_concluded","is_concluded"),("published_content_count","published_content_count")] {
                    if let Some(value)=row.0.get(old).filter(|v|!v.is_null()) {
                        let equivalent=metadata.get(new)==Some(value) || (new=="is_concluded" && metadata.get(new).and_then(Value::as_bool).map(i64::from)==value.as_i64());
                        if !equivalent {return Err(Error::Integrity(format!("{kind} {key} の現在の {old} が保存された原本と一致しません")));}
                    }
                }
                tx.execute("INSERT INTO source_entity_state(entity_id,last_checked_at,last_fetched_at) VALUES(?1,?2,?3) ON CONFLICT(entity_id) DO UPDATE SET last_checked_at=excluded.last_checked_at,last_fetched_at=excluded.last_fetched_at",params![entity,row.optional_time("last_checked_at")?,row.optional_time("last_fetched_at")?])?;
                let id=LibraryId::from_slice(&entity).map_err(|_|Error::Integrity("プロフィールの識別子が不正です".into()))?;
                mark_row(&tx,table,&row.integer("id")?.to_string(),id)
            })?;
            mark_table(&tx, table)?;
        }
        mark_table(&tx, "entity_versions")?;
        tx.commit()?;
        Ok(count)
    }

    fn import_profile_version(&self, target: &Library, row: LegacyRow) -> Result<()> {
        let old = row.integer("id")?.to_string();
        let provider = row.text("source")?;
        let key = row.text("source_key")?;
        let kind = match row.text("entity_type")? {
            "person" => EntityKind::Person,
            "series" => EntityKind::Series,
            _ => return Err(Error::Unsupported("旧プロフィールの型が不明です".into())),
        };
        let path = self.managed_path(row.text("json_path")?)?;
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(Error::Integrity(
                "プロフィールが読み込み上限を超えています".into(),
            ));
        }
        let data: Value = serde_json::from_slice(&bytes)?;
        if data.get("source").and_then(Value::as_str) != Some(provider)
            || data.get("sourceKey").and_then(Value::as_str) != Some(key)
        {
            return Err(Error::Integrity(format!(
                "プロフィール {old} の取得元が履歴行と違います"
            )));
        }
        let mut metadata = BTreeMap::new();
        let mut states = BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]);
        for (field, name) in [
            ("displayName", "display_name"),
            ("title", "title"),
            ("description", "description"),
            ("links", "links"),
            ("profileItems", "profile_items"),
            ("isConcluded", "is_concluded"),
            ("publishedContentCount", "published_content_count"),
            ("sampleNovelCount", "sample_novel_count"),
        ] {
            match data.get(field) {
                Some(value) if !value.is_null() => {
                    let valid = match name {
                        "display_name" | "title" | "description" => value.is_string(),
                        "links" | "profile_items" => value.is_array(),
                        "is_concluded" => value.is_boolean(),
                        "published_content_count" | "sample_novel_count" => {
                            value.as_u64().is_some()
                        }
                        _ => false,
                    };
                    if !valid {
                        return Err(Error::Integrity(format!(
                            "プロフィール {old} の {field} の型が不正です"
                        )));
                    }
                    metadata.insert(name.into(), value.clone());
                }
                _ => {
                    states.insert(name.into(), "unknown".into());
                }
            }
        }
        let dir = path
            .parent()
            .ok_or_else(|| Error::UnsafePath(path.clone()))?
            .join("assets");
        let mut images = BTreeMap::new();
        let mut total = bytes.len() as u64;
        if dir.try_exists()? {
            super::super::store::regular_directory(&dir)?;
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let file = entry.path();
                super::super::store::regular_file(&file)?;
                let name = entry.file_name().into_string().map_err(|_| {
                    Error::Integrity("プロフィール画像名が UTF-8 ではありません".into())
                })?;
                let stem = std::path::Path::new(&name)
                    .file_stem()
                    .and_then(|s| s.to_str());
                let slot = if name.starts_with("icon_") || stem == Some("icon") {
                    "icon"
                } else if name.starts_with("cover_") || stem == Some("cover") {
                    "cover"
                } else {
                    return Err(Error::Integrity(format!(
                        "プロフィール {old} に用途の不明なファイルがあります"
                    )));
                };
                total = total
                    .checked_add(fs::metadata(&file)?.len())
                    .ok_or_else(|| {
                        Error::Integrity("プロフィールの容量が上限を超えました".into())
                    })?;
                if images.insert(slot, (name, file)).is_some() {
                    return Err(Error::Integrity(format!(
                        "プロフィール {old} の画像が重複しています"
                    )));
                }
            }
        }
        // 初期の人物スナップショット（icon.png 等の固定名）は画像だけを集計していた。
        // その既知の形式でも JSON 自体は必ず保管し、任意の容量差を許容しない。
        let legacy_asset_accounting = kind == EntityKind::Person
            && !images.is_empty()
            && images.values().all(|(name, _)| {
                matches!(
                    std::path::Path::new(name)
                        .file_stem()
                        .and_then(|s| s.to_str()),
                    Some("icon" | "cover")
                )
            });
        if row.optional_integer("asset_count")?.unwrap_or(0) != images.len() as i64
            || row
                .optional_integer("file_size_bytes")?
                .is_some_and(|size| {
                    size < 0
                        || (size as u64 != total
                            && !(legacy_asset_accounting
                                && size as u64 == total - bytes.len() as u64))
                })
        {
            return Err(Error::Integrity(format!(
                "プロフィール {old} のファイル数・容量が記録と違います"
            )));
        }
        let mut media = Vec::new();
        let mut image_paths = Vec::new();
        for (slot, url_field) in [("icon", "iconUrl"), ("cover", "coverUrl")] {
            let saved = images.remove(slot);
            if let Some((_, path)) = &saved {
                image_paths.push((slot, path.clone()));
            }
            let url = data
                .get(url_field)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            if saved.is_none() && url.is_none() {
                continue;
            }
            let (name, file) = saved.map_or_else(
                || (slot.to_owned(), None),
                |(name, file)| (name, Some(CaptureSource::File(file))),
            );
            let mime = match std::path::Path::new(&name)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "application/octet-stream",
            };
            media.push(CapturedMedia {
                slot: slot.into(),
                role: if slot == "cover" && kind == EntityKind::Person {
                    "banner"
                } else {
                    slot
                }
                .into(),
                source_key: url.map(|u| cover_identity(provider, u)),
                source_url: url.map(str::to_owned),
                quality: "unknown".into(),
                original_name: name,
                mime_type: mime.into(),
                bytes: file,
            });
        }
        let request = {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let request:Option<Vec<u8>>=conn.query_row("SELECT request_id FROM migration_requests WHERE source_table='entity_versions' AND source_key=?1",[&old],|r|r.get(0)).optional()?;
            match request {
                Some(id) => LibraryId::from_slice(&id)
                    .map_err(|_| Error::Integrity("プロフィールの移行要求 ID が不正です".into()))?,
                None => {
                    let id = LibraryId::now_v7();
                    conn.execute(
                        "INSERT INTO migration_requests VALUES('entity_versions',?1,?2)",
                        params![old, id.as_bytes().as_slice()],
                    )?;
                    id
                }
            }
        };
        let result = target.capture_with_legacy_classification(
            Capture {
                acquired_at: row.optional_time("created_at")?,
                request_id: request,
                provider: provider.into(),
                external_key: key.into(),
                kind,
                route: "legacy-profile-import".into(),
                raw: CaptureSource::File(path.clone()),
                metadata,
                field_states: states,
                body: None,
                media,
                attachments: vec![],
            },
            Some("legacy_unclassified"),
        )?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        mark_row(&tx, "entity_versions", &old, result.snapshot_id)?;
        let raw: Vec<u8> = tx.query_row(
            "SELECT raw_file_id FROM acquisition_observations WHERE request_id=?1",
            [request.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        self.mark_used_file(&tx, &path, &raw)?;
        for (slot, path) in image_paths {
            let file:Vec<u8>=tx.query_row("SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 AND m.slot=?2",params![result.snapshot_id.as_bytes().as_slice(),slot],|r|r.get(0))?;
            self.mark_used_file(&tx, &path, &file)?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Fingerprint;
    use serde_json::json;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-profile-import-{}", LibraryId::now_v7()));
            let old = root.join("old");
            fs::create_dir_all(&old).unwrap();
            let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
            conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE entity_versions(id INTEGER,entity_type TEXT,source TEXT,source_key TEXT,version INTEGER,json_path TEXT,asset_count INTEGER,file_size_bytes INTEGER);
CREATE TABLE people(id INTEGER,source TEXT,source_key TEXT,display_name TEXT,description TEXT,current_version INTEGER,last_checked_at TEXT,last_fetched_at TEXT);
CREATE TABLE series(id INTEGER,source TEXT,source_key TEXT,title TEXT,current_version INTEGER,last_checked_at TEXT,last_fetched_at TEXT,is_concluded INTEGER,published_content_count INTEGER);
INSERT INTO people VALUES(7,'pixiv','author','New name','Description',2,'2026-01-02 00:00:00','2026-01-01 00:00:00');
INSERT INTO series VALUES(8,'pixiv','series','Series title',1,NULL,NULL,1,3);").unwrap();
            for (id, kind, key, version) in [
                (1, "person", "author", 1),
                (2, "person", "author", 2),
                (3, "series", "series", 1),
            ] {
                let dir = format!(
                    "{}/pixiv/{key}/v{version}",
                    if kind == "person" {
                        "profiles"
                    } else {
                        "series"
                    }
                );
                fs::create_dir_all(old.join(&dir).join("assets")).unwrap();
                let data = if kind == "person" {
                    json!({"source":"pixiv","sourceKey":key,"displayName":if version==1 {"Old name"}else{"New name"},"description":if version==1 {Value::Null}else{json!("Description")},"links":[],"iconUrl":format!("https://i.pximg.net/icon{version}.png")})
                } else {
                    json!({"source":"pixiv","sourceKey":key,"title":"Series title","isConcluded":true,"publishedContentCount":3,"coverUrl":"https://i.pximg.net/series.png"})
                };
                let raw = serde_json::to_vec(&data).unwrap();
                let image = format!("exact image bytes {id}\0ICC").into_bytes();
                let image_name = if kind == "person" {
                    "icon_image.png"
                } else {
                    "cover_image.png"
                };
                fs::write(old.join(&dir).join("original.json"), &raw).unwrap();
                fs::write(old.join(&dir).join("assets").join(image_name), &image).unwrap();
                conn.execute(
                    "INSERT INTO entity_versions VALUES(?1,?2,'pixiv',?3,?4,?5,1,?6)",
                    params![
                        id,
                        kind,
                        key,
                        version,
                        format!("{dir}/original.json"),
                        (raw.len() + image.len()) as i64
                    ],
                )
                .unwrap();
            }
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn profile_history_current_state_and_files_survive_without_the_old_folder() {
        let fixture = Fixture::new();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert_eq!(report.profile_snapshots, 3);
        assert!(library.works(10).unwrap().is_empty());
        let person = {
            let conn = library.conn.lock().unwrap();
            let id: Vec<u8> = conn
                .query_row(
                    "SELECT entity_id FROM external_identities WHERE kind='person'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            LibraryId::from_slice(&id).unwrap()
        };
        let states = library.snapshots(person).unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].metadata["display_name"], json!("Old name"));
        assert_eq!(states[0].field_states["description"], "unknown");
        assert_eq!(states[1].metadata["description"], json!("Description"));
        assert!(states
            .iter()
            .all(|s| s.classification == "legacy_unclassified"));
        assert_eq!(library.current_snapshot(person).unwrap(), states[1].id);
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        assert_eq!(library.snapshots(person).unwrap().len(), 2);
        library.backup(&fixture.0.join("backup")).unwrap();
        drop(source);
        drop(library);
        fs::rename(fixture.0.join("old"), fixture.0.join("old-unavailable")).unwrap();
        let backup = Library::open(&fixture.0.join("backup")).unwrap();
        assert_eq!(backup.current_snapshot(person).unwrap(), states[1].id);
        let file = backup.media_file(person, states[0].id, "icon").unwrap();
        assert_eq!(file.id, Fingerprint::of(b"exact image bytes 1\0ICC"));
        let mut bytes = Vec::new();
        backup.objects.export(&file, &mut bytes).unwrap();
        assert_eq!(bytes, b"exact image bytes 1\0ICC");
        backup.verify().unwrap();
    }

    #[test]
    fn mismatched_profile_counts_and_current_pointers_remain_unresolved() {
        for change in [
            "UPDATE entity_versions SET asset_count=0 WHERE id=2",
            "UPDATE people SET current_version=1",
            "UPDATE people SET display_name='Not in the original'",
        ] {
            let fixture = Fixture::new();
            let conn = rusqlite::Connection::open(fixture.0.join("old/piep.db")).unwrap();
            conn.execute_batch(change).unwrap();
            drop(conn);
            let library = Library::create(&fixture.0.join("new")).unwrap();
            let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
            let report = source
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(!report.unresolved.is_empty(), "{change}");
            assert!(!library.activation_problems().unwrap().is_empty());
            library.verify().unwrap();
        }
    }

    #[test]
    fn the_known_fixed_icon_format_keeps_json_even_when_the_old_total_excluded_it() {
        let fixture = Fixture::new();
        let old = fixture.0.join("old");
        let assets = old.join("profiles/pixiv/author/v1/assets");
        fs::rename(assets.join("icon_image.png"), assets.join("icon.png")).unwrap();
        let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
        conn.execute(
            "UPDATE entity_versions SET file_size_bytes=?1 WHERE id=1",
            [fs::metadata(assets.join("icon.png")).unwrap().len() as i64],
        )
        .unwrap();
        drop(conn);
        let source = LegacyImporter::open(&old).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let raw = fs::read(old.join("profiles/pixiv/author/v1/original.json")).unwrap();
        let file = library.file(Fingerprint::of(&raw)).unwrap();
        let mut bytes = Vec::new();
        library.objects.export(&file, &mut bytes).unwrap();
        assert_eq!(bytes, raw);
        library.verify().unwrap();
    }

    #[test]
    fn a_failed_recheck_stays_unresolved_until_the_original_source_is_available_again() {
        let fixture = Fixture::new();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        drop(source);
        let path = fixture
            .0
            .join("old/profiles/pixiv/author/v2/assets/icon_image.png");
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&path, changed).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        assert!(!source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        assert!(library
            .activation_problems()
            .unwrap()
            .iter()
            .any(|s| s.contains("profiles")));
        library.verify().unwrap();
        drop(source);
        fs::write(&path, original).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert!(library.activation_problems().unwrap().is_empty());
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM revision_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
}
