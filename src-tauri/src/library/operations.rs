use super::{
    editing::EditHeader, repository::Library, BodyFormat, ContentFile, EditContent, Error,
    Fingerprint, LibraryId, Result,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: LibraryId,
    pub revision_id: LibraryId,
    pub revision: i64,
    pub sequence: i64,
    pub saved_at: i64,
    pub classification: String,
    pub metadata: BTreeMap<String, serde_json::Value>,
    pub field_states: BTreeMap<String, String>,
    pub body_file: Option<Fingerprint>,
    pub body_format: Option<BodyFormat>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserState {
    pub favorite: bool,
    /// 作品の更新を見張るか。見張る作者・シリーズ（watch_targets）とは別に、作品ごとに持つ。
    #[serde(default)]
    pub watching: bool,
    pub generation: i64,
}

/// 利用者の状態の変更。`None` の項目は今の値のまま。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserStateChange {
    pub favorite: Option<bool>,
    pub watching: Option<bool>,
}

pub struct EditRequest<'a> {
    pub request_id: LibraryId,
    pub work: LibraryId,
    pub user: LibraryId,
    pub base: LibraryId,
    pub parent: Option<LibraryId>,
    pub expected: i64,
    pub content: &'a EditContent,
    pub activate: bool,
}

fn id(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("識別子の長さが不正です".into()))
}
fn fingerprint(bytes: Vec<u8>) -> Result<Fingerprint> {
    Ok(Fingerprint(bytes.try_into().map_err(|_| {
        Error::Integrity("ハッシュの長さが不正です".into())
    })?))
}

impl Library {
    pub fn current_snapshot(&self, entity: LibraryId) -> Result<LibraryId> {
        let conn = self.reader()?;
        let snapshot: Vec<u8> = conn.query_row(
            "SELECT snapshot_id FROM entity_current WHERE entity_id=?1",
            [entity.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        id(snapshot)
    }
    pub fn snapshots(&self, entity: LibraryId) -> Result<Vec<SnapshotInfo>> {
        let conn = self.reader()?;
        let mut stmt=conn.prepare("SELECT s.id,s.revision_id,r.ordinal,r.classification,s.metadata_json,s.field_states_json,s.body_file_id,s.sequence,s.created_at,s.body_format FROM revision_snapshots s JOIN source_revisions r ON r.id=s.revision_id WHERE s.entity_id=?1 AND s.published=1 ORDER BY s.sequence")?;
        let rows = stmt.query_map([entity.as_bytes().as_slice()], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<Vec<u8>>>(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get::<_, Option<String>>(9)?,
            ))
        })?;
        rows.map(|row| {
            let (
                snapshot,
                revision,
                ordinal,
                classification,
                metadata,
                states,
                body,
                sequence,
                saved_at,
                body_format,
            ) = row?;
            Ok(SnapshotInfo {
                id: id(snapshot)?,
                revision_id: id(revision)?,
                revision: ordinal,
                sequence,
                saved_at,
                classification,
                metadata: serde_json::from_str(&metadata)?,
                field_states: serde_json::from_str(&states)?,
                body_file: body.map(fingerprint).transpose()?,
                body_format: body_format
                    .as_deref()
                    .map(BodyFormat::from_name)
                    .transpose()?,
            })
        })
        .collect()
    }

    pub fn read_body(
        &self,
        entity: LibraryId,
        snapshot: LibraryId,
        destination: impl Write,
    ) -> Result<()> {
        self.read_resource(
            entity,
            super::ResourceScope::Snapshot(snapshot),
            &super::ResourceReference::Body,
            None,
            destination,
        )
    }

    /// 画像は要求した作品・保存状態から到達できるものだけを返す。
    pub fn media_file(
        &self,
        work: LibraryId,
        snapshot: LibraryId,
        slot: &str,
    ) -> Result<ContentFile> {
        self.resource_file(
            work,
            super::ResourceScope::Snapshot(snapshot),
            &super::ResourceReference::Media { slot: slot.into() },
        )
    }

    pub fn user_state(&self, work: LibraryId, user: LibraryId) -> Result<UserState> {
        let conn = self.reader()?;
        // 未保存作品の識別も対象にできるが、存在しない ID は成功にしない。
        conn.query_row(
            "SELECT id FROM works WHERE id=?1",
            [work.as_bytes().as_slice()],
            |_| Ok(()),
        )?;
        Ok(conn
            .query_row(
                "SELECT favorite,watching,generation FROM work_user_state WHERE work_id=?1 AND user_id=?2",
                params![work.as_bytes().as_slice(), user.as_bytes().as_slice()],
                |r| {
                    Ok(UserState {
                        favorite: r.get(0)?,
                        watching: r.get(1)?,
                        generation: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or(UserState {
                favorite: false,
                watching: false,
                generation: 0,
            }))
    }

    pub fn set_favorite(
        &self,
        request: LibraryId,
        work: LibraryId,
        user: LibraryId,
        expected: i64,
        favorite: bool,
    ) -> Result<UserState> {
        self.set_user_state(
            request,
            work,
            user,
            expected,
            UserStateChange {
                favorite: Some(favorite),
                watching: None,
            },
        )
    }

    pub fn set_watching(
        &self,
        request: LibraryId,
        work: LibraryId,
        user: LibraryId,
        expected: i64,
        watching: bool,
    ) -> Result<UserState> {
        self.set_user_state(
            request,
            work,
            user,
            expected,
            UserStateChange {
                favorite: None,
                watching: Some(watching),
            },
        )
    }

    /// お気に入りと更新の監視を一度に変える。世代が `expected` と違えば競合として断る。
    pub fn set_user_state(
        &self,
        request: LibraryId,
        work: LibraryId,
        user: LibraryId,
        expected: i64,
        change: UserStateChange,
    ) -> Result<UserState> {
        if expected < 0 {
            return Err(Error::Integrity("更新世代が不正です".into()));
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let payload = serde_json::json!({ "work":work,"user":user,"expected":expected,"favorite":change.favorite,"watching":change.watching });
        let receipt: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='user_state'",
                [request.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(stored) = receipt {
            let receipt: serde_json::Value = serde_json::from_str(&stored)?;
            if receipt["request"] != payload {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なる更新に使われました".into(),
                ));
            }
            return Ok(serde_json::from_value(receipt["result"].clone())?);
        }
        let (favorite, watching, generation): (bool, bool, i64) = tx
            .query_row(
                "SELECT favorite,watching,generation FROM work_user_state WHERE work_id=?1 AND user_id=?2",
                params![work.as_bytes().as_slice(), user.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .unwrap_or((false, false, 0));
        let favorite = change.favorite.unwrap_or(favorite);
        let watching = change.watching.unwrap_or(watching);
        if generation != expected {
            return Err(Error::Integrity(format!(
                "更新が競合しました。現在の世代は {generation} です"
            )));
        }
        let next = generation
            .checked_add(1)
            .ok_or_else(|| Error::Integrity("更新世代が上限を超えました".into()))?;
        tx.execute("INSERT INTO work_user_state(work_id,user_id,favorite,watching,generation) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(work_id,user_id) DO UPDATE SET favorite=excluded.favorite,watching=excluded.watching,generation=excluded.generation",params![work.as_bytes().as_slice(),user.as_bytes().as_slice(),favorite,watching,next])?;
        if user == self.id {
            self.refresh_work_list(&tx, work)?;
        }
        let result = UserState {
            favorite,
            watching,
            generation: next,
        };
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'user_state',?3,?4)",params![request.as_bytes().as_slice(),work.as_bytes().as_slice(),serde_json::json!({"request":payload,"result":result}).to_string(),chrono::Utc::now().timestamp_millis()])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(result)
    }

    /// 編集は固定した snapshot に対して保存する。別作品の基準は FK が拒否する。
    pub fn save_edit(&self, request: EditRequest<'_>) -> Result<LibraryId> {
        let EditRequest {
            request_id,
            work,
            user,
            base,
            parent,
            expected,
            content,
            activate,
        } = request;
        if expected < 0 {
            return Err(Error::Integrity("更新世代が不正です".into()));
        }
        let prepared = self.store_bytes(&content.encode()?)?;
        let file = prepared.file.clone();
        let media = content
            .media
            .iter()
            .map(|media| self.prepare_file(&self.file(media.file)?))
            .collect::<Result<Vec<_>>>()?;
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let payload = serde_json::json!({"work":work,"user":user,"base":base,"parent":parent,"expected":expected,"file":file.id,"activate":activate});
        let receipt: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='edit'",
                [request_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(receipt) = receipt {
            let receipt: serde_json::Value = serde_json::from_str(&receipt)?;
            if receipt["request"] != payload {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なる編集に使われました".into(),
                ));
            }
            return Ok(serde_json::from_value(receipt["edit"].clone())?);
        }
        let generation: i64 = tx
            .query_row(
                "SELECT generation FROM edit_documents WHERE work_id=?1 AND user_id=?2",
                params![work.as_bytes().as_slice(), user.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if generation != expected {
            return Err(Error::Integrity("編集中に別の変更が保存されました".into()));
        }
        for media in &media {
            Self::register_file(&tx, media)?;
        }
        let edit = LibraryId::now_v7();
        Self::insert_edit(
            &tx,
            EditHeader {
                id: edit,
                work,
                base,
                parent,
                created_at: chrono::Utc::now().timestamp_millis(),
            },
            content,
            &prepared,
        )?;
        tx.execute("INSERT INTO edit_documents(work_id,user_id,generation) VALUES(?1,?2,0) ON CONFLICT(work_id,user_id) DO NOTHING",params![work.as_bytes().as_slice(),user.as_bytes().as_slice()])?;
        let column = if activate { "active_id" } else { "draft_id" };
        tx.execute(&format!("UPDATE edit_documents SET {column}=?3,generation=generation+1 WHERE work_id=?1 AND user_id=?2"),params![work.as_bytes().as_slice(),user.as_bytes().as_slice(),edit.as_bytes().as_slice()])?;
        if activate && user == self.id {
            self.refresh_work_list(&tx, work)?;
        }
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'edit',?3,?4)",params![request_id.as_bytes().as_slice(),work.as_bytes().as_slice(),serde_json::json!({"edit":edit,"request":payload}).to_string(),chrono::Utc::now().timestamp_millis()])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind};
    #[test]
    fn user_state_has_atomic_retries_and_edits_keep_their_base() {
        let root = std::env::temp_dir().join(format!("piep-state-{}", LibraryId::now_v7()));
        {
            let library = Library::create(&root).unwrap();
            let capture = library
                .capture(Capture {
                    acquired_at: None,
                    request_id: LibraryId::now_v7(),
                    provider: "local".into(),
                    external_key: "one".into(),
                    kind: EntityKind::Work,
                    route: "test".into(),
                    raw: b"{}".to_vec().into(),
                    metadata: BTreeMap::new(),
                    field_states: BTreeMap::new(),
                    body: Some("本文".into()),
                    media: vec![],
                    attachments: vec![],
                })
                .unwrap();
            let user = LibraryId::now_v7();
            let request = LibraryId::now_v7();
            assert_eq!(
                library
                    .set_favorite(request, capture.entity_id, user, 0, true)
                    .unwrap()
                    .generation,
                1
            );
            assert_eq!(
                library
                    .set_favorite(request, capture.entity_id, user, 0, true)
                    .unwrap()
                    .generation,
                1
            );
            assert!(library
                .set_favorite(LibraryId::now_v7(), capture.entity_id, user, 0, false)
                .is_err());
            assert!(
                library
                    .user_state(capture.entity_id, user)
                    .unwrap()
                    .favorite
            );
            let request_id = LibraryId::now_v7();
            let document = EditContent {
                format: "piep-edit-v1".into(),
                title: Some("編集した題名".into()),
                blocks: vec![crate::library::EditBlock {
                    id: LibraryId::now_v7(),
                    ordinal: 0,
                    kind: "paragraph".into(),
                    text: Some("編集した本文".into()),
                    attrs: serde_json::Value::Null,
                    media_slot: None,
                }],
                media: vec![],
                provenance: None,
            };
            let edit = library
                .save_edit(EditRequest {
                    request_id,
                    work: capture.entity_id,
                    user,
                    base: capture.snapshot_id,
                    parent: None,
                    expected: 0,
                    content: &document,
                    activate: true,
                })
                .unwrap();
            assert_eq!(
                library
                    .save_edit(EditRequest {
                        request_id,
                        work: capture.entity_id,
                        user,
                        base: capture.snapshot_id,
                        parent: None,
                        expected: 0,
                        content: &document,
                        activate: true
                    })
                    .unwrap(),
                edit
            );
            assert!(library
                .save_edit(EditRequest {
                    request_id: LibraryId::now_v7(),
                    work: capture.entity_id,
                    user,
                    base: LibraryId::now_v7(),
                    parent: Some(edit),
                    expected: 1,
                    content: &document,
                    activate: true
                })
                .is_err());
            assert!(library
                .save_edit(EditRequest {
                    request_id: LibraryId::now_v7(),
                    work: capture.entity_id,
                    user,
                    base: capture.snapshot_id,
                    parent: Some(edit),
                    expected: 0,
                    content: &document,
                    activate: true
                })
                .is_err());
            library.verify().unwrap();
        }
        fs_remove_test_dir(&root);
    }
    fn fs_remove_test_dir(root: &std::path::Path) {
        std::fs::remove_dir_all(root).unwrap();
    }
}
