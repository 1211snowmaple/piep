use super::{repository::SOURCE_FIELDS_V1, Error, Fingerprint, LibraryId, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;

struct StoredSnapshot {
    metadata: String,
    states: String,
    body: Option<Vec<u8>>,
    semantic_rule: i64,
    manifest_rule: i64,
    interpretation: i64,
    header: Value,
}

pub(super) fn source_field_v1(name: &str, value: &Value) -> Value {
    let mut value = value.clone();
    if name == "profile_items" {
        if let Some(items) = value.as_array_mut() {
            for item in items.iter_mut().filter_map(Value::as_object_mut) {
                item.remove("thumbnailUrl");
                item.remove("localPath");
            }
        }
    }
    super::body::canonical(value)
}

/// 比較規則は版ごとに固定する。今後の規則変更で既存の指紋を解釈し直さない。
pub(super) fn snapshot_fingerprints(
    conn: &Connection,
    snapshot: LibraryId,
) -> Result<(Fingerprint, Fingerprint)> {
    let StoredSnapshot {metadata,states,body,semantic_rule,manifest_rule,interpretation,header}=conn.query_row("SELECT s.metadata_json,s.field_states_json,s.body_file_id,s.semantic_rule,s.manifest_rule,s.interpretation_version,s.entity_id,s.revision_id,s.sequence,s.created_at,r.ordinal,r.classification,r.discovered_at,s.body_format,s.body_semantic_hash,s.body_semantic_rule FROM revision_snapshots s JOIN source_revisions r ON r.id=s.revision_id WHERE s.id=?1",[snapshot.as_bytes().as_slice()],|r|Ok(StoredSnapshot {
        metadata:r.get(0)?,states:r.get(1)?,body:r.get(14)?,semantic_rule:r.get(3)?,manifest_rule:r.get(4)?,interpretation:r.get(5)?,
        header:json!({"id":snapshot,"entity":r.get::<_,Vec<u8>>(6)?,"revision":r.get::<_,Vec<u8>>(7)?,"sequence":r.get::<_,i64>(8)?,"saved_at":r.get::<_,i64>(9)?,"source_ordinal":r.get::<_,i64>(10)?,"classification":r.get::<_,String>(11)?,"source_discovered_at":r.get::<_,i64>(12)?,"body_file":r.get::<_,Option<Vec<u8>>>(2)?,"body_format":r.get::<_,Option<String>>(13)?,"body_semantic_rule":r.get::<_,i64>(15)?}),
    }))?;
    if semantic_rule != 1 || manifest_rule != 1 {
        return Err(Error::Unsupported(
            "保存状態の指紋の規則に対応していません".into(),
        ));
    }
    let metadata: BTreeMap<String, Value> = serde_json::from_str(&metadata)?;
    let states: BTreeMap<String, String> = serde_json::from_str(&states)?;
    let mut identities = BTreeMap::new();
    let selected_media = {
        let mut stmt=conn.prepare("SELECT m.slot,m.role,s.provider,s.kind,s.source_key,r.file_id,r.quality,m.original_name,r.mime_type,s.identity_rule,r.width,r.height FROM snapshot_media m JOIN source_media s ON s.id=m.media_id LEFT JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 ORDER BY m.slot")?;
        let mut rows = stmt.query([snapshot.as_bytes().as_slice()])?;
        let mut selected = Vec::new();
        while let Some(row) = rows.next()? {
            let slot: String = row.get(0)?;
            let key: Option<String> = row.get(4)?;
            identities.insert(slot.clone(), key.clone());
            selected.push(json!({"slot":slot,"role":row.get::<_,String>(1)?,"provider":row.get::<_,String>(2)?,"kind":row.get::<_,String>(3)?,"source_key":key,"file":row.get::<_,Option<Vec<u8>>>(5)?,"quality":row.get::<_,Option<String>>(6)?,"name":row.get::<_,String>(7)?,"mime":row.get::<_,Option<String>>(8)?,"identity_rule":row.get::<_,i64>(9)?,"width":row.get::<_,Option<i64>>(10)?,"height":row.get::<_,Option<i64>>(11)?}));
        }
        selected
    };
    let selected_files = {
        let mut stmt=conn.prepare("SELECT slot,file_id,original_name,mime_type,role FROM snapshot_files WHERE snapshot_id=?1 ORDER BY slot")?;
        let rows = stmt.query_map([snapshot.as_bytes().as_slice()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let observations = {
        let mut stmt=conn.prepare("SELECT o.id,o.entity_id,o.request_id,o.request_hash,o.acquired_at,o.route,o.status,o.raw_file_id,o.result_snapshot_id,o.created_revision,o.recorded_at,o.body_file_id,o.body_format FROM snapshot_observations s JOIN acquisition_observations o ON o.id=s.observation_id WHERE s.snapshot_id=?1 ORDER BY o.id")?;
        let rows=stmt.query_map([snapshot.as_bytes().as_slice()],|r|Ok(json!({"id":r.get::<_,Vec<u8>>(0)?,"entity":r.get::<_,Vec<u8>>(1)?,"request":r.get::<_,Vec<u8>>(2)?,"request_hash":r.get::<_,Vec<u8>>(3)?,"acquired_at":r.get::<_,i64>(4)?,"route":r.get::<_,String>(5)?,"status":r.get::<_,String>(6)?,"raw":r.get::<_,Option<Vec<u8>>>(7)?,"result_snapshot":r.get::<_,Option<Vec<u8>>>(8)?,"created_revision":r.get::<_,bool>(9)?,"recorded_at":r.get::<_,i64>(10)?,"body_file":r.get::<_,Option<Vec<u8>>>(11)?,"body_format":r.get::<_,Option<String>>(12)?})))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let people = {
        let mut stmt=conn.prepare("SELECT person_id,role,display_name FROM snapshot_people WHERE snapshot_id=?1 ORDER BY person_id,role")?;
        let rows=stmt.query_map([snapshot.as_bytes().as_slice()],|r|Ok(json!({"person":r.get::<_,Vec<u8>>(0)?,"role":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?})))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let series = {
        let mut stmt=conn.prepare("SELECT series_id,title,position FROM snapshot_series WHERE snapshot_id=?1 ORDER BY series_id")?;
        let rows=stmt.query_map([snapshot.as_bytes().as_slice()],|r|Ok(json!({"series":r.get::<_,Vec<u8>>(0)?,"title":r.get::<_,String>(1)?,"position":r.get::<_,Option<i64>>(2)?})))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let tags = {
        let mut stmt=conn.prepare("SELECT t.id,t.provider,t.name FROM snapshot_tags s JOIN tags t ON t.id=s.tag_id WHERE s.snapshot_id=?1 ORDER BY t.id")?;
        let rows=stmt.query_map([snapshot.as_bytes().as_slice()],|r|Ok(json!({"tag":r.get::<_,Vec<u8>>(0)?,"provider":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?})))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let semantic_metadata: BTreeMap<_, _> = metadata
        .iter()
        .filter(|(key, _)| SOURCE_FIELDS_V1.contains(&key.as_str()))
        .map(|(key, value)| (key, source_field_v1(key, value)))
        .collect();
    let attachment_identity: BTreeMap<_, _> = selected_files
        .iter()
        .map(|(slot, file, _, _, _)| (slot, file))
        .collect();
    let semantic = Fingerprint::of(&serde_json::to_vec(&(
        "source-content-v1",
        semantic_metadata,
        body,
        identities,
        attachment_identity,
    ))?);
    let manifest = Fingerprint::of(&serde_json::to_vec(&(
        "snapshot-manifest-v1",
        interpretation,
        header,
        metadata,
        semantic,
        states,
        selected_media,
        selected_files,
        observations,
        (people, series, tags),
    ))?);
    Ok((semantic, manifest))
}

pub(super) fn verify_snapshots(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id,semantic_hash,manifest_hash FROM revision_snapshots WHERE published=1",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: Vec<u8> = row.get(0)?;
        let id = LibraryId::from_slice(&id)
            .map_err(|_| Error::Integrity("保存状態の識別子が不正です".into()))?;
        let (semantic, manifest) = snapshot_fingerprints(conn, id)?;
        if semantic.0.as_slice() != row.get::<_, Vec<u8>>(1)?
            || manifest.0.as_slice() != row.get::<_, Vec<u8>>(2)?
        {
            return Err(Error::Integrity(format!(
                "保存状態 {id} の内容・構成が記録された指紋と一致しません"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, CapturedMedia, EntityKind, Library};
    use rusqlite::params;
    use std::fs;

    struct Fixture {
        root: std::path::PathBuf,
        library: Option<Library>,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-manifest-check-{}", LibraryId::now_v7()));
            let library = Some(Library::create(&root).unwrap());
            Self { root, library }
        }
        fn library(&self) -> &Library {
            self.library.as_ref().unwrap()
        }
        fn capture(&self, name: &str, quality: &str) -> crate::library::CaptureResult {
            self.library()
                .capture(Capture {
                    acquired_at: None,
                    request_id: LibraryId::now_v7(),
                    provider: "local".into(),
                    external_key: "one".into(),
                    kind: EntityKind::Work,
                    route: "test".into(),
                    raw: b"{}".to_vec().into(),
                    metadata: BTreeMap::from([
                        ("title".into(), json!("title")),
                        ("author_key".into(), json!("person")),
                        ("author_name".into(), json!(name)),
                        ("tags".into(), json!(["tag"])),
                    ]),
                    field_states: BTreeMap::new(),
                    body: Some("body".into()),
                    media: vec![CapturedMedia {
                        slot: "cover".into(),
                        role: "cover".into(),
                        source_key: Some("same-source-picture".into()),
                        source_url: Some("https://example.test/picture".into()),
                        quality: quality.into(),
                        original_name: "cover.png".into(),
                        mime_type: "image/png".into(),
                        bytes: Some(quality.as_bytes().to_vec().into()),
                    }],
                    attachments: vec![],
                })
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.library.take());
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn quality_and_author_display_changes_keep_the_source_fingerprint_and_change_the_manifest() {
        let fixture = Fixture::new();
        let library = fixture.library();
        let first = fixture.capture("Old author name", "thumbnail");
        let next = fixture.capture("New author name", "original");
        assert!(!next.created_revision);
        let conn = library.conn.lock().unwrap();
        let fingerprint = |id: LibraryId| {
            conn.query_row(
                "SELECT semantic_hash,manifest_hash FROM revision_snapshots WHERE id=?1",
                [id.as_bytes().as_slice()],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap()
        };
        let (old_source, old_manifest) = fingerprint(first.snapshot_id);
        let (new_source, new_manifest) = fingerprint(next.snapshot_id);
        assert_eq!(old_source, new_source);
        assert_ne!(old_manifest, new_manifest);
        for sql in [
            "UPDATE source_media SET source_key='replacement'",
            "UPDATE media_renditions SET mime_type='image/webp'",
            "UPDATE media_observations SET url='https://example.test/other'",
            "UPDATE tags SET name='renamed'",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
        drop(conn);
        library.verify().unwrap();
    }

    #[test]
    fn valid_file_hashes_do_not_hide_corrupted_metadata_media_observations_or_relations() {
        for (trigger,change) in [
            ("published_snapshot_immutable","UPDATE revision_snapshots SET metadata_json=json_set(metadata_json,'$.author_name','Changed')"),
            ("media_renditions_immutable","UPDATE media_renditions SET mime_type='image/webp'"),
            ("acquisition_observations_immutable","UPDATE acquisition_observations SET route='other'"),
            ("source_tags_immutable","UPDATE tags SET name='Changed'"),
            ("source_revisions_immutable","UPDATE source_revisions SET discovered_at=discovered_at+1"),
            ("published_snapshot_immutable","UPDATE revision_snapshots SET sequence=sequence+1"),
        ] {
            let fixture=Fixture::new();let library=fixture.library();fixture.capture("Name","original");
            {
                let conn=library.conn.lock().unwrap();conn.execute(&format!("DROP TRIGGER {trigger}"),[]).unwrap();conn.execute(change,[]).unwrap();
            }
            assert!(library.verify().is_err(),"{change}");
        }
    }

    #[test]
    fn an_unknown_fingerprint_rule_is_rejected_even_when_the_hashes_look_well_formed() {
        let fixture = Fixture::new();
        let saved = fixture.capture("Name", "original");
        let conn = fixture.library().conn.lock().unwrap();
        conn.execute("DROP TRIGGER published_snapshot_immutable", [])
            .unwrap();
        conn.execute(
            "UPDATE revision_snapshots SET manifest_rule=9 WHERE id=?1",
            params![saved.snapshot_id.as_bytes().as_slice()],
        )
        .unwrap();
        assert!(matches!(
            snapshot_fingerprints(&conn, saved.snapshot_id),
            Err(Error::Unsupported(_))
        ));
    }
}
