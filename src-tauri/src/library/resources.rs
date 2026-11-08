use super::{BodyFormat, ContentFile, Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// 現在版やファイル名から推測せず、読みたい保存状態を明示する。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ResourceScope {
    Snapshot(LibraryId),
    Edit(LibraryId),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceReference {
    Body,
    Original { observation: LibraryId },
    Media { slot: String },
    File { slot: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResourceFormat {
    SourceBody(BodyFormat),
    EditDocumentV1,
    OriginalResponse,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResourceFile {
    pub id: Fingerprint,
    pub length: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResourceInfo {
    pub reference: ResourceReference,
    pub role: String,
    pub original_name: Option<String>,
    pub mime_type: Option<String>,
    pub format: Option<ResourceFormat>,
    pub quality: Option<String>,
    pub source_identity: Option<String>,
    /// 取得元の画像が分かっていても、画像本体が未保存の場合は None。
    pub file: Option<ResourceFile>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResourceManifest {
    pub entity: LibraryId,
    pub scope: ResourceScope,
    /// 編集の表紙等を読む場合も、この基準版を使い、最新の版へ付け替えない。
    pub base_snapshot: Option<LibraryId>,
    pub resources: Vec<ResourceInfo>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceRange {
    pub offset: u64,
    pub length: u64,
}

const SNAPSHOT_RESOURCES: &str = "
SELECT 'body','', 'body',NULL,
    CASE s.body_format WHEN 'fanbox_body_v1' THEN 'application/json' ELSE 'text/plain; charset=utf-8' END,
    s.body_format,0,NULL,NULL,s.body_file_id,f.original_size,f.sealed,1
FROM revision_snapshots s LEFT JOIN content_files f ON f.id=s.body_file_id
WHERE s.id=?1 AND s.body_file_id IS NOT NULL AND (?2 IS NULL OR ?2='body')
UNION ALL
SELECT 'original',lower(hex(o.id)),
    CASE WHEN o.result_snapshot_id=?1 THEN 'source_response' ELSE 'earlier_source_response' END,
    NULL,'application/octet-stream',NULL,0,NULL,NULL,o.raw_file_id,f.original_size,f.sealed,1
FROM snapshot_observations so
JOIN revision_snapshots s ON s.id=so.snapshot_id AND s.entity_id=so.entity_id
JOIN acquisition_observations o ON o.id=so.observation_id AND o.entity_id=so.entity_id
LEFT JOIN content_files f ON f.id=o.raw_file_id
WHERE so.snapshot_id=?1 AND o.raw_file_id IS NOT NULL
    AND (?2 IS NULL OR ?2='original') AND (?3 IS NULL OR lower(hex(o.id))=?3)
UNION ALL
SELECT 'media',m.slot,m.role,m.original_name,r.mime_type,NULL,0,r.quality,sm.source_key,
    r.file_id,f.original_size,f.sealed,m.rendition_id IS NOT NULL
FROM snapshot_media m
LEFT JOIN source_media sm ON sm.id=m.media_id
LEFT JOIN media_renditions r ON r.id=m.rendition_id AND r.media_id=m.media_id
LEFT JOIN content_files f ON f.id=r.file_id
WHERE m.snapshot_id=?1 AND (?2 IS NULL OR ?2='media') AND (?3 IS NULL OR m.slot=?3)
UNION ALL
SELECT 'file',a.slot,a.role,a.original_name,a.mime_type,NULL,0,NULL,NULL,
    a.file_id,f.original_size,f.sealed,1
FROM snapshot_files a LEFT JOIN content_files f ON f.id=a.file_id
WHERE a.snapshot_id=?1 AND (?2 IS NULL OR ?2='file') AND (?3 IS NULL OR a.slot=?3)
";

const EDIT_RESOURCES: &str = "
SELECT 'body','', 'edit_document',NULL,'application/json',NULL,1,NULL,NULL,
    e.content_file_id,f.original_size,f.sealed,1
FROM edit_revisions e LEFT JOIN content_files f ON f.id=e.content_file_id
WHERE e.id=?1 AND (?2 IS NULL OR ?2='body')
UNION ALL
SELECT 'media',m.slot,'editor_asset',m.original_name,m.mime_type,NULL,0,NULL,NULL,
    m.file_id,f.original_size,f.sealed,1
FROM edit_media m LEFT JOIN content_files f ON f.id=m.file_id
WHERE m.edit_id=?1 AND (?2 IS NULL OR ?2='media') AND (?3 IS NULL OR m.slot=?3)
";

struct StoredResource {
    kind: String,
    slot: String,
    role: String,
    name: Option<String>,
    mime: Option<String>,
    body_format: Option<String>,
    edit_document: bool,
    quality: Option<String>,
    source_identity: Option<String>,
    file: Option<Vec<u8>>,
    length: Option<i64>,
    sealed: Option<bool>,
    expects_file: bool,
}

fn scope_id(scope: ResourceScope) -> LibraryId {
    match scope {
        ResourceScope::Snapshot(id) | ResourceScope::Edit(id) => id,
    }
}

fn require_scope(
    conn: &Connection,
    entity: LibraryId,
    scope: ResourceScope,
) -> Result<Option<LibraryId>> {
    let (sql, id) = match scope {
        ResourceScope::Snapshot(id) => (
            "SELECT id FROM revision_snapshots WHERE entity_id=?1 AND id=?2 AND published=1",
            id,
        ),
        ResourceScope::Edit(id) => (
            "SELECT base_snapshot_id FROM edit_revisions WHERE work_id=?1 AND id=?2 AND sealed=1",
            id,
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params![
        entity.as_bytes().as_slice(),
        id.as_bytes().as_slice()
    ])?;
    let row = rows.next()?.ok_or(Error::ResourceUnavailable)?;
    if matches!(scope, ResourceScope::Edit(_)) {
        let bytes: Vec<u8> = row.get(0)?;
        return Ok(Some(
            LibraryId::from_slice(&bytes)
                .map_err(|_| Error::Integrity("編集の基準版が不正です".into()))?,
        ));
    }
    Ok(None)
}

fn decode(row: StoredResource) -> Result<ResourceInfo> {
    let reference = match row.kind.as_str() {
        "body" => ResourceReference::Body,
        "original" => ResourceReference::Original {
            observation: LibraryId::parse_str(&row.slot)
                .map_err(|_| Error::Integrity("原応答の取得記録が不正です".into()))?,
        },
        "media" => ResourceReference::Media { slot: row.slot },
        "file" => ResourceReference::File { slot: row.slot },
        _ => return Err(Error::Integrity("保存資産の種類が不正です".into())),
    };
    let file = match (row.file, row.length, row.sealed) {
        (Some(id), Some(length), Some(true)) if length >= 0 => Some(ResourceFile {
            id: Fingerprint(
                id.try_into()
                    .map_err(|_| Error::Integrity("元ファイルの識別が不正です".into()))?,
            ),
            length: length as u64,
        }),
        (None, None, None) if !row.expects_file => None,
        _ => {
            return Err(Error::Integrity(
                "保存資産に未確定または欠損したファイル参照があります".into(),
            ))
        }
    };
    let format = if row.kind == "original" {
        Some(ResourceFormat::OriginalResponse)
    } else if row.edit_document {
        Some(ResourceFormat::EditDocumentV1)
    } else {
        row.body_format
            .as_deref()
            .map(BodyFormat::from_name)
            .transpose()?
            .map(ResourceFormat::SourceBody)
    };
    Ok(ResourceInfo {
        reference,
        role: row.role,
        original_name: row.name,
        mime_type: row.mime,
        format,
        quality: row.quality,
        source_identity: row.source_identity,
        file,
    })
}

fn resources(
    conn: &Connection,
    scope: ResourceScope,
    filter: Option<&ResourceReference>,
) -> Result<Vec<ResourceInfo>> {
    let (kind, slot) = match filter {
        None => (None, None),
        Some(ResourceReference::Body) => (Some("body"), None),
        Some(ResourceReference::Original { observation }) => {
            (Some("original"), Some(observation.simple().to_string()))
        }
        Some(ResourceReference::Media { slot }) => (Some("media"), Some(slot.clone())),
        Some(ResourceReference::File { slot }) => (Some("file"), Some(slot.clone())),
    };
    let sql = match scope {
        ResourceScope::Snapshot(_) => SNAPSHOT_RESOURCES,
        ResourceScope::Edit(_) => EDIT_RESOURCES,
    };
    let id = scope_id(scope);
    let mut stmt = conn.prepare(sql)?;
    let records = stmt.query_map(params![id.as_bytes().as_slice(), kind, slot], |r| {
        Ok(StoredResource {
            kind: r.get(0)?,
            slot: r.get(1)?,
            role: r.get(2)?,
            name: r.get(3)?,
            mime: r.get(4)?,
            body_format: r.get(5)?,
            edit_document: r.get(6)?,
            quality: r.get(7)?,
            source_identity: r.get(8)?,
            file: r.get(9)?,
            length: r.get(10)?,
            sealed: r.get(11)?,
            expects_file: r.get(12)?,
        })
    })?;
    records.map(|r| decode(r?)).collect()
}

impl Library {
    pub fn resource_manifest(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
    ) -> Result<ResourceManifest> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let base_snapshot = require_scope(&conn, entity, scope)?;
        let mut resources = resources(&conn, scope, None)?;
        resources.sort_by(|a, b| a.reference.cmp(&b.reference));
        Ok(ResourceManifest {
            entity,
            scope,
            base_snapshot,
            resources,
        })
    }

    pub fn resource_info(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
    ) -> Result<ResourceInfo> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        require_scope(&conn, entity, scope)?;
        resources(&conn, scope, Some(reference))?
            .pop()
            .ok_or(Error::ResourceUnavailable)
    }

    pub(super) fn resource_file(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
    ) -> Result<ContentFile> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        require_scope(&conn, entity, scope)?;
        let file = resources(&conn, scope, Some(reference))?
            .pop()
            .and_then(|info| info.file)
            .ok_or(Error::ResourceUnavailable)?;
        Self::file_from_conn(&conn, file.id)
    }

    /// 所属を照合してから読む。ファイルの復元や転送の間は DB の錠を持たない。
    pub fn read_resource(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
        range: Option<ResourceRange>,
        destination: impl Write,
    ) -> Result<()> {
        let file = self.resource_file(entity, scope, reference)?;
        match range {
            None => self.objects.export(&file, destination),
            Some(range) => {
                if range
                    .offset
                    .checked_add(range.length)
                    .is_none_or(|end| end > file.length)
                {
                    return Err(Error::InvalidRange);
                }
                self.objects
                    .read_range(&file, range.offset, range.length, destination)
            }
        }
    }

    /// 元バイト列を検証し、新規の出力先だけに公開する。
    pub fn export_resource(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
        destination: &Path,
    ) -> Result<()> {
        let file = self.resource_file(entity, scope, reference)?;
        self.objects.export_new(&file, destination)
    }
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
