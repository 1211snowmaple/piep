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

/// 編集の指し先の変更（下書き・反映中の版・以前の編集版の一覧）。版そのものは消さない。
/// 反映から外れた版は以前の編集版の一覧へ載り、反映し直した版は一覧から外れる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EditPointerChange {
    /// 下書きを反映する（反映中の版を下書きに替え、下書きを空にする）。
    Publish,
    /// 下書きを捨てる。
    Discard,
    /// 反映をやめ、取り込んだ本文へ戻す。
    Unpublish,
    /// 以前の編集版をもう一度反映する（下書きはそのまま）。
    Restore { edit: LibraryId },
    /// 以前の編集版を一覧から外す。
    Forget { edit: LibraryId },
    /// 以前の編集版を一覧から全部外す。
    ForgetAll,
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

/// 編集の指し先の行（反映中・下書き・世代）。
type Pointers = (Option<Vec<u8>>, Option<Vec<u8>>, i64);

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
        let mut stmt=conn.prepare("SELECT s.id,s.revision_id,r.ordinal,s.metadata_json,s.field_states_json,s.body_file_id,s.sequence,s.created_at,s.body_format FROM revision_snapshots s JOIN source_revisions r ON r.id=s.revision_id WHERE s.entity_id=?1 AND s.published=1 ORDER BY s.sequence")?;
        let rows = stmt.query_map([entity.as_bytes().as_slice()], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<Vec<u8>>>(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                snapshot,
                revision,
                ordinal,
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

    /// 棚に載っている作品の世代（外す操作の `expected` に渡す）。載っていなければ `None`。
    pub fn library_item_generation(&self, work: LibraryId) -> Result<Option<i64>> {
        let conn = self.reader()?;
        Ok(conn
            .query_row(
                "SELECT generation FROM library_items WHERE work_id=?1",
                [work.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 作品を棚から外す。一覧・検索の索引・過去の版の索引から消えるが、作品の識別・保存版・
    /// 編集・利用者の状態・コレクションの行は残す（コレクションでは保存のない作品として出る）。
    /// 実体の回収や完全な削除は別の操作。同じ作品をもう一度保存すれば棚に戻る。
    /// 世代が `expected` と違えば競合として断る。戻り値は外した時点の棚の世代。
    pub fn remove_work(&self, request: LibraryId, work: LibraryId, expected: i64) -> Result<i64> {
        if expected < 1 {
            return Err(Error::Integrity("更新世代が不正です".into()));
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let payload = serde_json::json!({ "work": work, "expected": expected });
        let receipt: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='remove_work'",
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
        let generation: Option<i64> = tx
            .query_row(
                "SELECT generation FROM library_items WHERE work_id=?1",
                [work.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        match generation {
            None => return Err(Error::Integrity("棚に載っていない作品です".into())),
            Some(generation) if generation != expected => {
                return Err(Error::Integrity(format!(
                    "更新が競合しました。現在の世代は {generation} です"
                )))
            }
            Some(_) => {}
        }
        tx.execute(
            "DELETE FROM work_current_projection WHERE work_id=?1",
            [work.as_bytes().as_slice()],
        )?;
        tx.execute(
            "DELETE FROM library_items WHERE work_id=?1",
            [work.as_bytes().as_slice()],
        )?;
        let shelf: i64 = tx.query_row(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1 RETURNING generation",
            [],
            |r| r.get(0),
        )?;
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'remove_work',?3,?4)",params![request.as_bytes().as_slice(),work.as_bytes().as_slice(),serde_json::json!({"request":payload,"result":shelf}).to_string(),chrono::Utc::now().timestamp_millis()])?;
        tx.commit()?;
        Ok(shelf)
    }

    /// 編集の指し先を変える（反映・下書きを捨てる・反映をやめる）。変わらなければ書かない。
    /// `expected` は今の世代。送り直しは同じ `request` で一度きり。
    pub fn change_edit_pointers(
        &self,
        request: LibraryId,
        work: LibraryId,
        user: LibraryId,
        expected: i64,
        change: EditPointerChange,
    ) -> Result<super::EditState> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let payload =
            serde_json::json!({"work":work,"user":user,"expected":expected,"change":change});
        let receipt: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='edit_pointer'",
                [request.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(receipt) = receipt {
            let receipt: serde_json::Value = serde_json::from_str(&receipt)?;
            if receipt["request"] != payload {
                return Err(Error::Integrity(super::repository::REQUEST_REUSED.into()));
            }
            return Ok(serde_json::from_value(receipt["state"].clone())?);
        }
        let key = (work.as_bytes().to_vec(), user.as_bytes().to_vec());
        let row: Option<Pointers> = tx
            .query_row(
                "SELECT active_id,draft_id,generation FROM edit_documents WHERE work_id=?1 AND user_id=?2",
                params![key.0, key.1],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (active, draft, generation) = row.unwrap_or((None, None, 0));
        if generation != expected {
            return Err(Error::Integrity("編集中に別の変更が保存されました".into()));
        }
        let archived: Vec<Vec<u8>> = tx
            .prepare("SELECT edit_id FROM edit_archive WHERE work_id=?1 AND user_id=?2")?
            .query_map(params![key.0, key.1], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let listed = |edit: LibraryId| {
            let edit = edit.as_bytes().to_vec();
            if archived.contains(&edit) {
                Ok(edit)
            } else {
                Err(Error::Integrity("以前の編集版にありません".into()))
            }
        };
        let mut forget: Vec<Vec<u8>> = Vec::new();
        let (next_active, next_draft) = match change {
            EditPointerChange::Publish => match &draft {
                Some(draft) => (Some(draft.clone()), None),
                None => return Err(Error::Integrity("反映する下書きがありません".into())),
            },
            EditPointerChange::Discard => (active.clone(), None),
            EditPointerChange::Unpublish => (None, draft.clone()),
            EditPointerChange::Restore { edit } => (Some(listed(edit)?), draft.clone()),
            EditPointerChange::Forget { edit } => {
                forget.push(listed(edit)?);
                (active.clone(), draft.clone())
            }
            EditPointerChange::ForgetAll => {
                forget.clone_from(&archived);
                (active.clone(), draft.clone())
            }
        };
        let to_id = |value: &Option<Vec<u8>>| {
            value
                .as_deref()
                .map(LibraryId::from_slice)
                .transpose()
                .map_err(|_| Error::Integrity("編集の識別子が不正です".into()))
        };
        if (next_active.as_ref(), next_draft.as_ref()) == (active.as_ref(), draft.as_ref())
            && forget.is_empty()
        {
            return Ok(super::EditState {
                active: to_id(&active)?,
                draft: to_id(&draft)?,
                generation,
            });
        }
        // 一覧にだけある作品（移行で入った以前の編集版）には、まだ編集文書の行が無い。
        tx.execute("INSERT INTO edit_documents(work_id,user_id,generation) VALUES(?1,?2,0) ON CONFLICT(work_id,user_id) DO NOTHING",params![key.0, key.1])?;
        tx.execute(
            "UPDATE edit_documents SET active_id=?3,draft_id=?4,generation=generation+1 WHERE work_id=?1 AND user_id=?2",
            params![key.0, key.1, next_active, next_draft],
        )?;
        if next_active != active {
            if let Some(old) = &active {
                tx.execute(
                    "INSERT OR IGNORE INTO edit_archive(work_id,user_id,edit_id) VALUES(?1,?2,?3)",
                    params![key.0, key.1, old],
                )?;
            }
            if let Some(new) = &next_active {
                forget.push(new.clone());
            }
        }
        for edit in &forget {
            tx.execute(
                "DELETE FROM edit_archive WHERE work_id=?1 AND user_id=?2 AND edit_id=?3",
                params![key.0, key.1, edit],
            )?;
        }
        if next_active != active && user == self.id {
            self.refresh_work_list(&tx, work)?;
        }
        let state = super::EditState {
            active: to_id(&next_active)?,
            draft: to_id(&next_draft)?,
            generation: generation + 1,
        };
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'edit_pointer',?3,?4)",params![request.as_bytes().as_slice(),work.as_bytes().as_slice(),serde_json::json!({"state":state,"request":payload}).to_string(),chrono::Utc::now().timestamp_millis()])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(state)
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
