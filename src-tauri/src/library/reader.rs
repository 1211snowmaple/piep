use super::{
    reader_references, BodyAssetReference, BodyFormat, EditContent, Error, Fingerprint, Library,
    LibraryId, ResourceInfo, ResourceManifest, ResourceReference, ResourceScope, Result,
    SourceBody,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReaderBinding {
    pub reference: BodyAssetReference,
    /// None は未取得。別の版や外部 URL へ補完しない。
    pub resource: Option<ResourceReference>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReaderBindingSet {
    pub entity: LibraryId,
    pub snapshot: LibraryId,
    pub recipe: i64,
    pub source_manifest: Fingerprint,
    pub body_file: Fingerprint,
    pub bindings_hash: Fingerprint,
    pub bindings: Vec<ReaderBinding>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ReaderBody {
    PlainText(String),
    PixivText(String),
    Article(Value),
    Edit(EditContent),
}
#[derive(Clone, Debug, Serialize)]
pub struct ResolvedReaderBinding {
    pub reference: BodyAssetReference,
    pub resource: Option<ResourceInfo>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReaderDocument {
    pub format: &'static str,
    pub entity: LibraryId,
    pub scope: ResourceScope,
    pub metadata: BTreeMap<String, Value>,
    pub body: ReaderBody,
    pub resources: ResourceManifest,
    pub base_resources: Option<ResourceManifest>,
    pub bindings: Vec<ResolvedReaderBinding>,
    pub bindings_hash: Option<Fingerprint>,
}

struct Header {
    manifest: Fingerprint,
    body: Fingerprint,
    format: BodyFormat,
    metadata: BTreeMap<String, Value>,
}
struct StoredHeader {
    manifest: Vec<u8>,
    body: Option<Vec<u8>>,
    format: Option<String>,
    metadata: String,
}
struct StoredBindingSet {
    manifest: Vec<u8>,
    body: Vec<u8>,
    hash: Vec<u8>,
    sealed: bool,
}
fn fingerprint(bytes: Vec<u8>) -> Result<Fingerprint> {
    Ok(Fingerprint(bytes.try_into().map_err(|_| {
        Error::Integrity("読書用の指紋が不正です".into())
    })?))
}
fn header(conn: &Connection, entity: LibraryId, snapshot: LibraryId) -> Result<Header> {
    let row=conn.query_row("SELECT s.manifest_hash,s.body_file_id,s.body_format,s.metadata_json FROM revision_snapshots s JOIN works w ON w.id=s.entity_id WHERE s.entity_id=?1 AND s.id=?2 AND s.published=1",params![entity.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|Ok(StoredHeader {manifest:r.get(0)?,body:r.get(1)?,format:r.get(2)?,metadata:r.get(3)?})).optional()?;
    let StoredHeader {
        manifest,
        body,
        format,
        metadata,
    } = row.ok_or(Error::ResourceUnavailable)?;
    Ok(Header {
        manifest: fingerprint(manifest)?,
        body: fingerprint(body.ok_or(Error::ResourceUnavailable)?)?,
        format: BodyFormat::from_name(&format.ok_or(Error::ResourceUnavailable)?)?,
        metadata: serde_json::from_str(&metadata)?,
    })
}
fn validate(body: &SourceBody, bindings: &mut [ReaderBinding]) -> Result<()> {
    normalize_input_bindings(bindings)?;
    let required = reader_references::required(body)?;
    let provided: BTreeSet<_> = bindings.iter().map(|b| b.reference.clone()).collect();
    if provided != required {
        return Err(Error::Integrity(
            "本文の画像・添付指定と読書用の対応表が一致しません".into(),
        ));
    }
    Ok(())
}

pub(super) fn normalize_input_bindings(bindings: &mut [ReaderBinding]) -> Result<()> {
    if bindings.len() > reader_references::MAX_BODY_ASSETS {
        return Err(Error::Unsupported(
            "読書用の対応表が読み込み上限を超えています".into(),
        ));
    }
    bindings.sort_by(|a, b| a.reference.cmp(&b.reference));
    let provided: BTreeSet<_> = bindings.iter().map(|b| b.reference.clone()).collect();
    if provided.len() != bindings.len() {
        return Err(Error::Integrity(
            "本文の画像・添付指定と読書用の対応表が一致しません".into(),
        ));
    }
    for binding in bindings {
        if !match &binding.resource {
            None => true,
            Some(ResourceReference::Media { .. }) => binding.reference.is_image(),
            Some(ResourceReference::File { .. }) => !binding.reference.is_image(),
            _ => false,
        } {
            return Err(Error::Integrity(
                "本文の参照と画像・添付の種類が一致しません".into(),
            ));
        }
    }
    Ok(())
}
fn hash(
    entity: LibraryId,
    snapshot: LibraryId,
    header: &Header,
    bindings: &[ReaderBinding],
) -> Result<Fingerprint> {
    Ok(Fingerprint::of(&serde_json::to_vec(&(
        "reader-bindings-v1",
        entity,
        snapshot,
        header.manifest,
        header.body,
        bindings,
    ))?))
}
fn load(
    conn: &Connection,
    entity: LibraryId,
    snapshot: LibraryId,
    header: &Header,
) -> Result<Option<ReaderBindingSet>> {
    let row=conn.query_row("SELECT source_manifest,body_file_id,bindings_hash,sealed FROM reader_binding_sets WHERE entity_id=?1 AND snapshot_id=?2 AND recipe=1",params![entity.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|Ok(StoredBindingSet {manifest:r.get(0)?,body:r.get(1)?,hash:r.get(2)?,sealed:r.get(3)?})).optional()?;
    let Some(StoredBindingSet {
        manifest,
        body,
        hash: stored_hash,
        sealed,
    }) = row
    else {
        return Ok(None);
    };
    if !sealed || fingerprint(manifest)? != header.manifest || fingerprint(body)? != header.body {
        return Err(Error::Integrity(
            "読書用の対応表が保存版と一致しません".into(),
        ));
    }
    let mut stmt=conn.prepare("SELECT reference_json,media_slot,file_slot,unavailable FROM reader_bindings WHERE snapshot_id=?1 AND recipe=1")?;
    let mut bindings = Vec::new();
    for row in stmt.query_map([snapshot.as_bytes().as_slice()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, bool>(3)?,
        ))
    })? {
        let (reference, media, file, unavailable) = row?;
        let resource = match (media, file, unavailable) {
            (Some(slot), None, false) => Some(ResourceReference::Media { slot }),
            (None, Some(slot), false) => Some(ResourceReference::File { slot }),
            (None, None, true) => None,
            _ => return Err(Error::Integrity("読書用の参照先が不正です".into())),
        };
        bindings.push(ReaderBinding {
            reference: serde_json::from_str(&reference)?,
            resource,
        });
    }
    bindings.sort_by(|a, b| a.reference.cmp(&b.reference));
    let bindings_hash = fingerprint(stored_hash)?;
    if hash(entity, snapshot, header, &bindings)? != bindings_hash {
        return Err(Error::Integrity(
            "読書用の対応表の指紋が一致しません".into(),
        ));
    }
    Ok(Some(ReaderBindingSet {
        entity,
        snapshot,
        recipe: 1,
        source_manifest: header.manifest,
        body_file: header.body,
        bindings_hash,
        bindings,
    }))
}

impl Library {
    /// 対応表は独立した不変の派生物。表を補っても原本・改稿・保存版の指紋を変えない。
    pub fn publish_reader_bindings(
        &self,
        entity: LibraryId,
        snapshot: LibraryId,
        bindings: Vec<ReaderBinding>,
    ) -> Result<ReaderBindingSet> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let plan = self.publish_reader_bindings_in_tx(&tx, entity, snapshot, bindings)?;
        tx.commit()?;
        Ok(plan)
    }

    pub(super) fn publish_reader_bindings_in_tx(
        &self,
        conn: &Connection,
        entity: LibraryId,
        snapshot: LibraryId,
        mut bindings: Vec<ReaderBinding>,
    ) -> Result<ReaderBindingSet> {
        let header = header(conn, entity, snapshot)?;
        let file = Self::file_from_conn(conn, header.body)?;
        if file.length > super::body::MAX_BODY_BYTES {
            return Err(Error::Unsupported(
                "本文が読み込み上限を超えています".into(),
            ));
        }
        let mut bytes = Vec::new();
        self.objects.export(&file, &mut bytes)?;
        let body = SourceBody::decode(header.format, &bytes)?;
        validate(&body, &mut bindings)?;
        let resources = super::resources::resources(conn, ResourceScope::Snapshot(snapshot), None)?;
        let references: BTreeSet<_> = resources.iter().map(|r| &r.reference).collect();
        for binding in &bindings {
            if let Some(resource) = &binding.resource {
                if !references.contains(resource) {
                    return Err(Error::ResourceUnavailable);
                }
            }
        }
        if let Some(old) = load(conn, entity, snapshot, &header)? {
            if old.bindings != bindings {
                return Err(Error::Integrity(
                    "確定済みの読書用の対応表は変更できません".into(),
                ));
            }
            return Ok(old);
        }
        let digest = hash(entity, snapshot, &header, &bindings)?;
        conn.execute("INSERT INTO reader_binding_sets(entity_id,snapshot_id,recipe,source_manifest,body_file_id,bindings_hash) VALUES(?1,?2,1,?3,?4,?5)",params![entity.as_bytes().as_slice(),snapshot.as_bytes().as_slice(),header.manifest.0.as_slice(),header.body.0.as_slice(),digest.0.as_slice()])?;
        for binding in &bindings {
            let (media, file) = match &binding.resource {
                Some(ResourceReference::Media { slot }) => (Some(slot), None),
                Some(ResourceReference::File { slot }) => (None, Some(slot)),
                _ => (None, None),
            };
            conn.execute(
                "INSERT INTO reader_bindings VALUES(?1,1,?2,?3,?4,?5)",
                params![
                    snapshot.as_bytes().as_slice(),
                    serde_json::to_string(&binding.reference)?,
                    media,
                    file,
                    binding.resource.is_none()
                ],
            )?;
        }
        conn.execute(
            "UPDATE reader_binding_sets SET sealed=1 WHERE snapshot_id=?1 AND recipe=1",
            [snapshot.as_bytes().as_slice()],
        )?;
        Ok(ReaderBindingSet {
            entity,
            snapshot,
            recipe: 1,
            source_manifest: header.manifest,
            body_file: header.body,
            bindings_hash: digest,
            bindings,
        })
    }
    pub fn reader_bindings(
        &self,
        entity: LibraryId,
        snapshot: LibraryId,
    ) -> Result<ReaderBindingSet> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        Self::reader_bindings_from_conn(&conn, entity, snapshot)
    }
    pub(super) fn reader_bindings_from_conn(
        conn: &Connection,
        entity: LibraryId,
        snapshot: LibraryId,
    ) -> Result<ReaderBindingSet> {
        let header = header(conn, entity, snapshot)?;
        load(conn, entity, snapshot, &header)?.ok_or(Error::ResourceUnavailable)
    }
    pub fn reader_document(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
    ) -> Result<ReaderDocument> {
        let resources = self.resource_manifest(entity, scope)?;
        if let ResourceScope::Edit(edit) = scope {
            let edit = self.edit(entity, edit)?;
            let metadata = {
                let conn = self.conn.lock().map_err(|_| Error::Busy)?;
                let json:String=conn.query_row("SELECT metadata_json FROM revision_snapshots WHERE entity_id=?1 AND id=?2 AND published=1",params![entity.as_bytes().as_slice(),edit.base.as_bytes().as_slice()],|r|r.get(0))?;
                serde_json::from_str(&json)?
            };
            let base_resources =
                Some(self.resource_manifest(entity, ResourceScope::Snapshot(edit.base))?);
            return Ok(ReaderDocument {
                format: "piep-reader-v1",
                entity,
                scope,
                metadata,
                body: ReaderBody::Edit(edit.content),
                resources,
                base_resources,
                bindings: vec![],
                bindings_hash: None,
            });
        }
        let ResourceScope::Snapshot(snapshot) = scope else {
            unreachable!()
        };
        let body = self
            .source_body(entity, snapshot)?
            .ok_or(Error::ResourceUnavailable)?;
        let (metadata, plan) = {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let header = header(&conn, entity, snapshot)?;
            let plan = load(&conn, entity, snapshot, &header)?;
            (header.metadata, plan)
        };
        let bindings_hash = plan.as_ref().map(|p| p.bindings_hash);
        let mut bindings = match plan {
            Some(plan) => plan.bindings,
            None if reader_references::required(&body)?.is_empty() => vec![],
            None => return Err(Error::ResourceUnavailable),
        };
        validate(&body, &mut bindings)?;
        let selected: BTreeMap<_, _> = resources
            .resources
            .iter()
            .map(|r| (&r.reference, r))
            .collect();
        let bindings = bindings
            .into_iter()
            .map(|binding| {
                Ok(ResolvedReaderBinding {
                    reference: binding.reference,
                    resource: binding
                        .resource
                        .map(|reference| {
                            selected
                                .get(&reference)
                                .map(|r| (*r).clone())
                                .ok_or(Error::ResourceUnavailable)
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let body = match body {
            SourceBody::PlainText(text) => ReaderBody::PlainText(text),
            SourceBody::PixivText(text) => ReaderBody::PixivText(text),
            SourceBody::FanboxBody(body) => ReaderBody::Article(body),
        };
        Ok(ReaderDocument {
            format: "piep-reader-v1",
            entity,
            scope,
            metadata,
            body,
            resources,
            base_resources: None,
            bindings,
            bindings_hash,
        })
    }
    pub(super) fn verify_reader_bindings(&self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("SELECT entity_id,snapshot_id FROM reader_binding_sets")?;
        for row in stmt.query_map([], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })? {
            let (entity, snapshot) = row?;
            let entity = LibraryId::from_slice(&entity)
                .map_err(|_| Error::Integrity("読書用の対象が不正です".into()))?;
            let snapshot = LibraryId::from_slice(&snapshot)
                .map_err(|_| Error::Integrity("読書用の保存版が不正です".into()))?;
            let header = header(conn, entity, snapshot)?;
            let mut plan =
                load(conn, entity, snapshot, &header)?.ok_or(Error::ResourceUnavailable)?;
            let file = Self::file_from_conn(conn, header.body)?;
            if file.length > super::body::MAX_BODY_BYTES {
                return Err(Error::Unsupported(
                    "本文が読み込み上限を超えています".into(),
                ));
            }
            let mut bytes = Vec::new();
            self.objects.export(&file, &mut bytes)?;
            validate(
                &SourceBody::decode(header.format, &bytes)?,
                &mut plan.bindings,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "reader_tests.rs"]
mod tests;
