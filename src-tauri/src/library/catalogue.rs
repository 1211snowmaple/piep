use super::{Error, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionMember {
    pub work_id: LibraryId,
    pub title_at_add: String,
    pub author_at_add: String,
    pub role: String,
    pub added_by: String,
    pub pinned: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub id: LibraryId,
    pub title: String,
    pub description: Option<String>,
    pub ordered: bool,
    pub generation: i64,
    pub members: Vec<CollectionMember>,
}

pub struct CollectionSave {
    pub request_id: LibraryId,
    pub id: LibraryId,
    pub expected_generation: i64,
    pub title: String,
    pub description: Option<String>,
    pub ordered: bool,
    pub members: Vec<CollectionMember>,
}

fn decode_uuid(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("関係の識別子が不正です".into()))
}

impl Library {
    pub fn collections(&self) -> Result<Vec<CollectionInfo>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare(
            "SELECT id,title,description,ordered,generation FROM collections ORDER BY title,id",
        )?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
            ))
        })?;
        let mut output = Vec::new();
        for row in rows {
            let (id, title, description, ordered, generation) = row?;
            let id = decode_uuid(id)?;
            let mut members=conn.prepare("SELECT work_id,title_at_add,author_at_add,role,added_by,pinned,note FROM collection_members WHERE collection_id=?1 ORDER BY position,work_id")?;
            let rows = members.query_map([id.as_bytes().as_slice()], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?;
            let members = rows
                .map(|row| {
                    let (work, title_at_add, author_at_add, role, added_by, pinned, note) = row?;
                    Ok(CollectionMember {
                        work_id: decode_uuid(work)?,
                        title_at_add,
                        author_at_add,
                        role,
                        added_by,
                        pinned,
                        note,
                    })
                })
                .collect::<Result<_>>()?;
            output.push(CollectionInfo {
                id,
                title,
                description,
                ordered,
                generation,
                members,
            });
        }
        Ok(output)
    }

    /// 所属と順番は一回で確定する。失敗時に途中の並びや outbox を残さない。
    pub fn save_collection(&self, request: CollectionSave) -> Result<i64> {
        if request.title.trim().is_empty() || request.expected_generation < 0 {
            return Err(Error::Integrity(
                "コレクション名または更新世代が不正です".into(),
            ));
        }
        let mut unique = std::collections::HashSet::new();
        if request.members.iter().any(|m| !unique.insert(m.work_id)) {
            return Err(Error::Integrity(
                "同じ作品がコレクション内で重複しています".into(),
            ));
        }
        let payload = serde_json::json!({"id":request.id,"expected":request.expected_generation,"title":request.title,"description":request.description,"ordered":request.ordered,"members":request.members});
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let receipt: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='collection'",
                [request.request_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(receipt) = receipt {
            let receipt: serde_json::Value = serde_json::from_str(&receipt)?;
            if receipt["request"] != payload {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なるコレクションに使われました".into(),
                ));
            }
            return receipt["generation"]
                .as_i64()
                .ok_or_else(|| Error::Integrity("確定記録の世代が不正です".into()));
        }
        let generation: Option<i64> = tx
            .query_row(
                "SELECT generation FROM collections WHERE id=?1",
                [request.id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if generation.unwrap_or(0) != request.expected_generation {
            return Err(Error::Integrity(
                "コレクションが別の操作で変更されました".into(),
            ));
        }
        let next = request
            .expected_generation
            .checked_add(1)
            .ok_or_else(|| Error::Integrity("更新世代の上限を超えました".into()))?;
        let now = chrono::Utc::now().timestamp_millis();
        if generation.is_none() {
            tx.execute(
                "INSERT INTO entities(id,kind,created_at) VALUES(?1,'collection',?2)",
                params![request.id.as_bytes().as_slice(), now],
            )?;
        }
        tx.execute("INSERT INTO collections(id,title,description,ordered,generation) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET title=excluded.title,description=excluded.description,ordered=excluded.ordered,generation=excluded.generation",params![request.id.as_bytes().as_slice(),request.title,request.description,request.ordered,next])?;
        let created = {
            let mut stmt = tx.prepare(
                "SELECT work_id,created_at FROM collection_members WHERE collection_id=?1",
            )?;
            let rows = stmt.query_map([request.id.as_bytes().as_slice()], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
            })?;
            rows.collect::<std::result::Result<std::collections::BTreeMap<_, _>, _>>()?
        };
        tx.execute(
            "DELETE FROM collection_members WHERE collection_id=?1",
            [request.id.as_bytes().as_slice()],
        )?;
        for (position, member) in request.members.iter().enumerate() {
            tx.execute("INSERT INTO collection_members(collection_id,work_id,position,title_at_add,author_at_add,role,added_by,pinned,note,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![request.id.as_bytes().as_slice(),member.work_id.as_bytes().as_slice(),i64::try_from(position).map_err(|_|Error::Integrity("所属数の上限を超えました".into()))?,member.title_at_add,member.author_at_add,member.role,member.added_by,member.pinned,member.note,created.get(member.work_id.as_bytes().as_slice()).copied().unwrap_or(now),now])?;
        }
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'collection',?3,?4)",params![request.request_id.as_bytes().as_slice(),request.id.as_bytes().as_slice(),serde_json::json!({"request":payload,"generation":next}).to_string(),now])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind};
    use std::{collections::BTreeMap, fs};

    #[test]
    fn collection_updates_keep_order_and_retries_and_roll_back_a_failed_replacement() {
        let root =
            std::env::temp_dir().join(format!("piep-collection-state-{}", LibraryId::now_v7()));
        {
            let library = Library::create(&root).unwrap();
            let mut members = Vec::new();
            for key in ["one", "two"] {
                let work = library
                    .capture(Capture {
                        acquired_at: None,
                        request_id: LibraryId::now_v7(),
                        provider: "local".into(),
                        external_key: key.into(),
                        kind: EntityKind::Work,
                        route: "test".into(),
                        raw: b"{}".to_vec().into(),
                        metadata: BTreeMap::new(),
                        field_states: BTreeMap::new(),
                        body: None,
                        media: vec![],
                        attachments: vec![],
                    })
                    .unwrap();
                members.push(CollectionMember {
                    work_id: work.entity_id,
                    title_at_add: key.into(),
                    author_at_add: "作者".into(),
                    role: "main".into(),
                    added_by: "manual".into(),
                    pinned: key == "one",
                    note: Some("メモ".into()),
                });
            }
            let id = LibraryId::now_v7();
            let request = LibraryId::now_v7();
            let save = |request_id, expected_generation, members| {
                library.save_collection(CollectionSave {
                    request_id,
                    id,
                    expected_generation,
                    title: "読み順".into(),
                    description: None,
                    ordered: true,
                    members,
                })
            };
            assert_eq!(save(request, 0, members.clone()).unwrap(), 1);
            assert_eq!(save(request, 0, members.clone()).unwrap(), 1);
            let mut invalid = members.clone();
            invalid[1].work_id = LibraryId::now_v7();
            assert!(save(LibraryId::now_v7(), 1, invalid).is_err());
            let before = library.collections().unwrap().remove(0);
            assert_eq!(before.generation, 1);
            assert_eq!(before.members, members);
            {
                let conn = library.conn.lock().unwrap();
                conn.execute(
                    "UPDATE collection_members SET created_at=42 WHERE collection_id=?1",
                    [id.as_bytes().as_slice()],
                )
                .unwrap();
                assert_eq!(
                    conn.query_row(
                        "SELECT count(*) FROM outbox WHERE operation='collection'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    1
                );
            }
            members.reverse();
            assert_eq!(save(LibraryId::now_v7(), 1, members.clone()).unwrap(), 2);
            assert_eq!(library.collections().unwrap().remove(0).members, members);
            let conn = library.conn.lock().unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM collection_members WHERE created_at=42",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                2
            );
            drop(conn);
            library.verify().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
