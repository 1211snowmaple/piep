use super::{Error, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagProvenance {
    Manual,
    AcceptedModel,
}
impl TagProvenance {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AcceptedModel => "accepted_model",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserTag {
    pub name: String,
    pub provenance: TagProvenance,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserTags {
    pub generation: i64,
    pub tags: Vec<UserTag>,
}

pub struct TagUpdate {
    pub request_id: LibraryId,
    pub work: LibraryId,
    pub user: LibraryId,
    pub expected_generation: i64,
    pub tags: Vec<UserTag>,
}

impl Library {
    pub fn source_tags(&self, work: LibraryId, snapshot: LibraryId) -> Result<Vec<String>> {
        let conn = self.reader()?;
        conn.query_row(
            "SELECT id FROM revision_snapshots WHERE entity_id=?1 AND id=?2 AND published=1",
            params![work.as_bytes().as_slice(), snapshot.as_bytes().as_slice()],
            |_| Ok(()),
        )?;
        let mut stmt=conn.prepare("SELECT t.name FROM snapshot_tags s JOIN tags t ON t.id=s.tag_id WHERE s.snapshot_id=?1 ORDER BY t.name,t.id")?;
        let rows = stmt.query_map([snapshot.as_bytes().as_slice()], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 作者のタグとは別の正本を持つ。作者の保存版を変更しない。
    pub fn user_tags(&self, work: LibraryId, user: LibraryId) -> Result<UserTags> {
        let conn = self.reader()?;
        conn.query_row(
            "SELECT id FROM works WHERE id=?1",
            [work.as_bytes().as_slice()],
            |_| Ok(()),
        )?;
        let generation = conn
            .query_row(
                "SELECT generation FROM work_user_tag_state WHERE work_id=?1 AND user_id=?2",
                params![work.as_bytes().as_slice(), user.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let mut stmt=conn.prepare("SELECT t.name,w.provenance FROM work_user_tags w JOIN user_tags t ON t.id=w.tag_id AND t.user_id=w.user_id WHERE w.work_id=?1 AND w.user_id=?2 ORDER BY t.name,t.id")?;
        let rows = stmt.query_map(
            params![work.as_bytes().as_slice(), user.as_bytes().as_slice()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        let tags = rows
            .map(|row| {
                let (name, provenance) = row?;
                let provenance = match provenance.as_str() {
                    "manual" => TagProvenance::Manual,
                    "accepted_model" => TagProvenance::AcceptedModel,
                    _ => return Err(Error::Integrity("タグの出どころが不明です".into())),
                };
                Ok(UserTag { name, provenance })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(UserTags { generation, tags })
    }

    pub fn replace_user_tags(&self, mut request: TagUpdate) -> Result<UserTags> {
        request.tags.sort_by(|a, b| a.name.cmp(&b.name));
        if request.expected_generation < 0
            || request.tags.iter().any(|t| t.name.trim().is_empty())
            || request.tags.windows(2).any(|p| p[0].name == p[1].name)
        {
            return Err(Error::Integrity("タグ名・重複・更新世代が不正です".into()));
        }
        let payload = serde_json::json!({"work":request.work,"user":request.user,"expected":request.expected_generation,"tags":request.tags});
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let receipt: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='user_tags'",
                [request.request_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(receipt) = receipt {
            let receipt: serde_json::Value = serde_json::from_str(&receipt)?;
            if receipt["request"] != payload {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なるタグの更新に使われました".into(),
                ));
            }
            return Ok(serde_json::from_value(receipt["result"].clone())?);
        }
        let generation: i64 = tx
            .query_row(
                "SELECT generation FROM work_user_tag_state WHERE work_id=?1 AND user_id=?2",
                params![
                    request.work.as_bytes().as_slice(),
                    request.user.as_bytes().as_slice()
                ],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if generation != request.expected_generation {
            return Err(Error::Integrity(
                "タグの編集中に別の変更が保存されました".into(),
            ));
        }
        let next = generation
            .checked_add(1)
            .ok_or_else(|| Error::Integrity("タグの更新世代の上限を超えました".into()))?;
        tx.execute("INSERT INTO work_user_tag_state VALUES(?1,?2,?3) ON CONFLICT(work_id,user_id) DO UPDATE SET generation=excluded.generation",params![request.work.as_bytes().as_slice(),request.user.as_bytes().as_slice(),next])?;
        tx.execute(
            "DELETE FROM work_user_tags WHERE work_id=?1 AND user_id=?2",
            params![
                request.work.as_bytes().as_slice(),
                request.user.as_bytes().as_slice()
            ],
        )?;
        for tag in &request.tags {
            tx.execute(
                "INSERT INTO user_tags VALUES(?1,?2,?3) ON CONFLICT(user_id,name) DO NOTHING",
                params![
                    LibraryId::now_v7().as_bytes().as_slice(),
                    request.user.as_bytes().as_slice(),
                    tag.name
                ],
            )?;
            tx.execute("INSERT INTO work_user_tags(work_id,user_id,tag_id,provenance) SELECT ?1,?2,id,?4 FROM user_tags WHERE user_id=?2 AND name=?3",params![request.work.as_bytes().as_slice(),request.user.as_bytes().as_slice(),tag.name,tag.provenance.name()])?;
        }
        let result = UserTags {
            generation: next,
            tags: request.tags,
        };
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'user_tags',?3,?4)",params![request.request_id.as_bytes().as_slice(),request.work.as_bytes().as_slice(),serde_json::json!({"request":payload,"result":result}).to_string(),chrono::Utc::now().timestamp_millis()])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind};
    use std::{collections::BTreeMap, fs};

    #[test]
    fn user_tags_are_isolated_retryable_and_independent_of_source_revisions() {
        let root = std::env::temp_dir().join(format!("piep-user-tags-{}", LibraryId::now_v7()));
        {
            let library = Library::create(&root).unwrap();
            let capture = |tag: &str| Capture {
                acquired_at: None,
                request_id: LibraryId::now_v7(),
                provider: "local".into(),
                external_key: "work".into(),
                kind: EntityKind::Work,
                route: "test".into(),
                raw: b"{}".to_vec().into(),
                metadata: BTreeMap::from([("tags".into(), serde_json::json!([tag]))]),
                field_states: BTreeMap::new(),
                body: Some("本文".into()),
                media: vec![],
                attachments: vec![],
            };
            let first = library.capture(capture("作者のタグ")).unwrap();
            let user = LibraryId::now_v7();
            let request = LibraryId::now_v7();
            let tags = vec![UserTag {
                name: "選んだタグ".into(),
                provenance: TagProvenance::AcceptedModel,
            }];
            let save = |request_id, expected_generation, tags| {
                library.replace_user_tags(TagUpdate {
                    request_id,
                    work: first.entity_id,
                    user,
                    expected_generation,
                    tags,
                })
            };
            let result = save(request, 0, tags.clone()).unwrap();
            assert_eq!(result.generation, 1);
            assert_eq!(save(request, 0, tags).unwrap(), result);
            assert!(save(LibraryId::now_v7(), 0, vec![]).is_err());
            assert!(save(request, 0, vec![]).is_err());
            assert_eq!(library.user_tags(first.entity_id, user).unwrap(), result);
            assert_eq!(
                library.user_tags(first.entity_id, library.id).unwrap(),
                UserTags::default()
            );
            assert_eq!(
                library
                    .source_tags(first.entity_id, first.snapshot_id)
                    .unwrap(),
                vec!["作者のタグ"]
            );
            assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
            let later = library.capture(capture("新しい作者のタグ")).unwrap();
            assert_eq!(library.user_tags(first.entity_id, user).unwrap(), result);
            assert_eq!(
                library
                    .source_tags(first.entity_id, later.snapshot_id)
                    .unwrap(),
                vec!["新しい作者のタグ"]
            );
            assert_eq!(
                library
                    .source_tags(first.entity_id, first.snapshot_id)
                    .unwrap(),
                vec!["作者のタグ"]
            );
            assert!(library
                .source_tags(LibraryId::now_v7(), first.snapshot_id)
                .is_err());
            {
                let conn = library.conn.lock().unwrap();
                let tag: Vec<u8> = conn
                    .query_row("SELECT id FROM user_tags", [], |r| r.get(0))
                    .unwrap();
                assert!(conn
                    .execute(
                        "INSERT INTO work_user_tags VALUES(?1,?2,?3,'manual')",
                        params![
                            first.entity_id.as_bytes().as_slice(),
                            library.id.as_bytes().as_slice(),
                            tag
                        ]
                    )
                    .is_err());
            }
            assert_eq!(save(LibraryId::now_v7(), 1, vec![]).unwrap().generation, 2);
            assert!(library
                .user_tags(first.entity_id, user)
                .unwrap()
                .tags
                .is_empty());
            library.verify().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
