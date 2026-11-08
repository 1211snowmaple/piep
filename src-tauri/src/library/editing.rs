use super::{
    repository::PreparedFile, ContentFile, Error, Fingerprint, Library, LibraryId, Result,
};
use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

const MAX_EDIT_BYTES: u64 = 16 * 1024 * 1024;
type StoredEditState = (Option<Vec<u8>>, Option<Vec<u8>>, i64);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditBlock {
    pub id: LibraryId,
    pub ordinal: i64,
    pub kind: String,
    pub text: Option<String>,
    pub attrs: Value,
    pub media_slot: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditMedia {
    pub slot: String,
    pub file: Fingerprint,
    pub original_name: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditContent {
    pub format: String,
    pub title: Option<String>,
    pub blocks: Vec<EditBlock>,
    pub media: Vec<EditMedia>,
    pub provenance: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditInfo {
    pub id: LibraryId,
    pub work: LibraryId,
    pub base: LibraryId,
    pub parent: Option<LibraryId>,
    pub created_at: i64,
    pub content: EditContent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditState {
    pub active: Option<LibraryId>,
    pub draft: Option<LibraryId>,
    pub generation: i64,
}

pub(crate) struct EditHeader {
    pub id: LibraryId,
    pub work: LibraryId,
    pub base: LibraryId,
    pub parent: Option<LibraryId>,
    pub created_at: i64,
}

impl EditContent {
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_EDIT_BYTES {
            return Err(Error::Integrity(
                "編集文書が読み込み上限を超えています".into(),
            ));
        }
        Ok(bytes)
    }

    fn validate(&self) -> Result<()> {
        if self.format != "piep-edit-v1" {
            return Err(Error::Unsupported("編集文書の形式が異なります".into()));
        }
        let mut slots = BTreeSet::new();
        for media in &self.media {
            if media.slot.is_empty() || !slots.insert(media.slot.as_str()) {
                return Err(Error::Integrity(
                    "編集画像の識別が空または重複しています".into(),
                ));
            }
        }
        let mut ids = BTreeSet::new();
        let mut previous = -1;
        for block in &self.blocks {
            if block.kind.is_empty() || block.ordinal <= previous || !ids.insert(block.id) {
                return Err(Error::Integrity(
                    "編集ブロックの識別・順序が不正です".into(),
                ));
            }
            previous = block.ordinal;
            if block
                .media_slot
                .as_deref()
                .is_some_and(|s| !slots.contains(s))
                || (block.kind == "image" && block.media_slot.is_none())
            {
                return Err(Error::Integrity("編集画像の参照先がありません".into()));
            }
        }
        Ok(())
    }
}

impl Library {
    pub fn edit_state(&self, work: LibraryId, user: LibraryId) -> Result<EditState> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        conn.query_row(
            "SELECT id FROM works WHERE id=?1",
            [work.as_bytes().as_slice()],
            |_| Ok(()),
        )?;
        let row:Option<StoredEditState>=conn.query_row("SELECT active_id,draft_id,generation FROM edit_documents WHERE work_id=?1 AND user_id=?2",params![work.as_bytes().as_slice(),user.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let id = |id: Vec<u8>| {
            LibraryId::from_slice(&id)
                .map_err(|_| Error::Integrity("編集の識別子が不正です".into()))
        };
        row.map(|(active, draft, generation)| {
            Ok(EditState {
                active: active.map(id).transpose()?,
                draft: draft.map(id).transpose()?,
                generation,
            })
        })
        .unwrap_or_else(|| Ok(EditState::default()))
    }

    /// 本文とその関係を一度に公開する。公開した後の追加・差し替えは許さない。
    pub(crate) fn insert_edit(
        tx: &Transaction<'_>,
        header: EditHeader,
        document: &EditContent,
        prepared: &PreparedFile,
    ) -> Result<()> {
        if Fingerprint::of(&document.encode()?) != prepared.file.id {
            return Err(Error::Integrity(
                "編集文書と保存ファイルが一致しません".into(),
            ));
        }
        Self::register_file(tx, prepared)?;
        tx.execute("INSERT INTO edit_revisions(id,work_id,base_snapshot_id,parent_id,content_file_id,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![header.id.as_bytes().as_slice(),header.work.as_bytes().as_slice(),header.base.as_bytes().as_slice(),header.parent.map(|id|id.as_bytes().to_vec()),prepared.file.id.0.as_slice(),header.created_at])?;
        for media in &document.media {
            tx.execute(
                "INSERT INTO edit_media VALUES(?1,?2,?3,?4,?5)",
                params![
                    header.id.as_bytes().as_slice(),
                    media.slot,
                    media.file.0.as_slice(),
                    media.original_name,
                    media.mime_type
                ],
            )?;
        }
        for block in &document.blocks {
            tx.execute(
                "INSERT INTO edit_blocks VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    header.id.as_bytes().as_slice(),
                    block.id.as_bytes().as_slice(),
                    block.ordinal,
                    block.kind,
                    block.text,
                    block.attrs.to_string(),
                    block.media_slot
                ],
            )?;
        }
        tx.execute(
            "UPDATE edit_revisions SET sealed=1 WHERE id=?1",
            [header.id.as_bytes().as_slice()],
        )?;
        Ok(())
    }

    pub fn edit(&self, work: LibraryId, edit: LibraryId) -> Result<EditInfo> {
        let (base, parent, file, created_at) = {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let (base,parent,file,created_at):(Vec<u8>,Option<Vec<u8>>,Vec<u8>,i64)=conn.query_row("SELECT base_snapshot_id,parent_id,content_file_id,created_at FROM edit_revisions WHERE id=?1 AND work_id=?2 AND sealed=1",params![edit.as_bytes().as_slice(),work.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            let file = Self::file_from_conn(
                &conn,
                Fingerprint(
                    file.try_into()
                        .map_err(|_| Error::Integrity("編集ファイルのハッシュが不正です".into()))?,
                ),
            )?;
            (base, parent, file, created_at)
        };
        if file.length > MAX_EDIT_BYTES {
            return Err(Error::Integrity(
                "編集文書が読み込み上限を超えています".into(),
            ));
        }
        let mut bytes = Vec::new();
        self.objects.export(&file, &mut bytes)?;
        let content: EditContent = serde_json::from_slice(&bytes)?;
        content.validate()?;
        {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            self.check_edit_relations(&conn, edit, &content)?;
        }
        let id = |bytes: Vec<u8>| {
            LibraryId::from_slice(&bytes)
                .map_err(|_| Error::Integrity("編集の識別子が不正です".into()))
        };
        Ok(EditInfo {
            id: edit,
            work,
            base: id(base)?,
            parent: parent.map(id).transpose()?,
            created_at,
            content,
        })
    }

    pub fn edit_media_file(
        &self,
        work: LibraryId,
        edit: LibraryId,
        slot: &str,
    ) -> Result<ContentFile> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let file:Vec<u8>=conn.query_row("SELECT m.file_id FROM edit_media m JOIN edit_revisions e ON e.id=m.edit_id WHERE e.work_id=?1 AND e.id=?2 AND e.sealed=1 AND m.slot=?3",params![work.as_bytes().as_slice(),edit.as_bytes().as_slice(),slot],|r|r.get(0))?;
        Self::file_from_conn(
            &conn,
            Fingerprint(
                file.try_into()
                    .map_err(|_| Error::Integrity("編集画像のハッシュが不正です".into()))?,
            ),
        )
    }

    fn check_edit_relations(
        &self,
        conn: &Connection,
        edit: LibraryId,
        content: &EditContent,
    ) -> Result<()> {
        let mut stmt=conn.prepare("SELECT block_id,ordinal,kind,text,attrs_json,media_slot FROM edit_blocks WHERE edit_id=?1 ORDER BY ordinal")?;
        let mut rows = stmt.query([edit.as_bytes().as_slice()])?;
        for expected in &content.blocks {
            let row = rows
                .next()?
                .ok_or_else(|| Error::Integrity("編集文書のブロックが不足しています".into()))?;
            let id: Vec<u8> = row.get(0)?;
            let actual = EditBlock {
                id: LibraryId::from_slice(&id)
                    .map_err(|_| Error::Integrity("ブロックの ID が不正です".into()))?,
                ordinal: row.get(1)?,
                kind: row.get(2)?,
                text: row.get(3)?,
                attrs: serde_json::from_str(&row.get::<_, String>(4)?)?,
                media_slot: row.get(5)?,
            };
            if &actual != expected {
                return Err(Error::Integrity(
                    "編集文書とブロックの記録が一致しません".into(),
                ));
            }
        }
        if rows.next()?.is_some() {
            return Err(Error::Integrity("編集文書にないブロックがあります".into()));
        }
        let mut stmt=conn.prepare("SELECT slot,file_id,original_name,mime_type FROM edit_media WHERE edit_id=?1 ORDER BY slot")?;
        let mut rows = stmt.query([edit.as_bytes().as_slice()])?;
        let mut media = content.media.iter().collect::<Vec<_>>();
        media.sort_by(|a, b| a.slot.cmp(&b.slot));
        for expected in media {
            let row = rows
                .next()?
                .ok_or_else(|| Error::Integrity("編集文書の画像が不足しています".into()))?;
            let file: Vec<u8> = row.get(1)?;
            let actual = EditMedia {
                slot: row.get(0)?,
                file: Fingerprint(
                    file.try_into()
                        .map_err(|_| Error::Integrity("画像のハッシュが不正です".into()))?,
                ),
                original_name: row.get(2)?,
                mime_type: row.get(3)?,
            };
            if &actual != expected {
                return Err(Error::Integrity(
                    "編集文書と画像の記録が一致しません".into(),
                ));
            }
        }
        if rows.next()?.is_some() {
            return Err(Error::Integrity("編集文書にない画像があります".into()));
        }
        Ok(())
    }

    pub(crate) fn verify_edit_documents(&self, conn: &Connection) -> Result<()> {
        let mut stmt =
            conn.prepare("SELECT id,content_file_id FROM edit_revisions WHERE sealed=1")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: Vec<u8> = row.get(0)?;
            let file: Vec<u8> = row.get(1)?;
            let edit = LibraryId::from_slice(&id)
                .map_err(|_| Error::Integrity("編集の ID が不正です".into()))?;
            let file = Self::file_from_conn(
                conn,
                Fingerprint(
                    file.try_into()
                        .map_err(|_| Error::Integrity("編集のハッシュが不正です".into()))?,
                ),
            )?;
            if file.length > MAX_EDIT_BYTES {
                return Err(Error::Integrity(
                    "編集文書が読み込み上限を超えています".into(),
                ));
            }
            let mut bytes = Vec::new();
            self.objects.export(&file, &mut bytes)?;
            let content: EditContent = serde_json::from_slice(&bytes)?;
            content.validate()?;
            self.check_edit_relations(conn, edit, &content)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, CaptureSource, EditRequest, EntityKind};
    use std::{collections::BTreeMap, fs};

    #[test]
    fn editing_keeps_its_base_and_media_across_later_source_changes_and_retries() {
        let root = std::env::temp_dir().join(format!("piep-editing-{}", LibraryId::now_v7()));
        {
            let library = Library::create(&root).unwrap();
            let capture = |body: &str| Capture {
                acquired_at: None,
                request_id: LibraryId::now_v7(),
                provider: "local".into(),
                external_key: "work".into(),
                kind: EntityKind::Work,
                route: "test".into(),
                raw: b"{}".to_vec().into(),
                metadata: BTreeMap::new(),
                field_states: BTreeMap::new(),
                body: Some(body.into()),
                media: vec![],
                attachments: vec![],
            };
            let old = library.capture(capture("元の本文")).unwrap();
            let image = library
                .store_original(&CaptureSource::Bytes(
                    b"original editor image\0ICC".to_vec(),
                ))
                .unwrap();
            let document = EditContent {
                format: "piep-edit-v1".into(),
                title: None,
                blocks: vec![EditBlock {
                    id: LibraryId::now_v7(),
                    ordinal: 3,
                    kind: "image".into(),
                    text: None,
                    attrs: serde_json::json!({"gapBefore":2}),
                    media_slot: Some("added".into()),
                }],
                media: vec![EditMedia {
                    slot: "added".into(),
                    file: image.id,
                    original_name: "元画像.png".into(),
                    mime_type: Some("image/png".into()),
                }],
                provenance: None,
            };
            let request = LibraryId::now_v7();
            let save = |generation| {
                library.save_edit(EditRequest {
                    request_id: request,
                    work: old.entity_id,
                    user: library.id,
                    base: old.snapshot_id,
                    parent: None,
                    expected: generation,
                    content: &document,
                    activate: true,
                })
            };
            let edit = save(0).unwrap();
            let later = library.capture(capture("作者が変えた本文")).unwrap();
            assert_ne!(old.snapshot_id, later.snapshot_id);
            assert_eq!(save(0).unwrap(), edit);
            assert!(save(1).is_err());
            let info = library.edit(old.entity_id, edit).unwrap();
            assert_eq!(info.base, old.snapshot_id);
            assert_eq!(info.content, document);
            assert_eq!(
                library.edit_state(old.entity_id, library.id).unwrap(),
                EditState {
                    active: Some(edit),
                    draft: None,
                    generation: 1
                }
            );
            let media = library
                .edit_media_file(old.entity_id, edit, "added")
                .unwrap();
            let mut bytes = Vec::new();
            library.objects.export(&media, &mut bytes).unwrap();
            assert_eq!(bytes, b"original editor image\0ICC");
            assert!(library.edit(LibraryId::now_v7(), edit).is_err());
            assert!(library
                .edit_media_file(LibraryId::now_v7(), edit, "added")
                .is_err());
            library.verify().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mismatched_edit_relations_are_detected_even_if_the_content_hash_is_valid() {
        let root =
            std::env::temp_dir().join(format!("piep-editing-corrupt-{}", LibraryId::now_v7()));
        {
            let library = Library::create(&root).unwrap();
            let work = library
                .capture(Capture {
                    acquired_at: None,
                    request_id: LibraryId::now_v7(),
                    provider: "local".into(),
                    external_key: "work".into(),
                    kind: EntityKind::Work,
                    route: "test".into(),
                    raw: b"{}".to_vec().into(),
                    metadata: BTreeMap::new(),
                    field_states: BTreeMap::new(),
                    body: Some("body".into()),
                    media: vec![],
                    attachments: vec![],
                })
                .unwrap();
            let document = EditContent {
                format: "piep-edit-v1".into(),
                title: None,
                blocks: vec![EditBlock {
                    id: LibraryId::now_v7(),
                    ordinal: 0,
                    kind: "paragraph".into(),
                    text: Some("original".into()),
                    attrs: Value::Null,
                    media_slot: None,
                }],
                media: vec![],
                provenance: None,
            };
            let edit = library
                .save_edit(EditRequest {
                    request_id: LibraryId::now_v7(),
                    work: work.entity_id,
                    user: library.id,
                    base: work.snapshot_id,
                    parent: None,
                    expected: 0,
                    content: &document,
                    activate: false,
                })
                .unwrap();
            {
                let conn = library.conn.lock().unwrap();
                conn.execute_batch("DROP TRIGGER edit_blocks_sealed_update; UPDATE edit_blocks SET text='corruption';").unwrap();
            }
            assert!(library.edit(work.entity_id, edit).is_err());
            assert!(library.verify().is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
