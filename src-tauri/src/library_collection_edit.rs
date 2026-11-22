//! コレクションの編集（評価文書 4.1 の 4）。画面の一回の操作を一つの変更として受け、所属と並びは
//! `save_collection` で一度に確定する。見た目（表紙・名前の付け方）と削除はここで書く。
//!
//! どれも一つのトランザクションで世代を上げ、outbox に記録する。`request` は画面が一回の操作ごとに
//! 作る ID で、送り直しても二度書かない。

use crate::analysis::collection_rules::episode_order;
use crate::library::{
    CollectionMember, CollectionSave, Error, Fingerprint, Library, LibraryId, Result,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::HashSet;

/// 回を並べ直す決まり。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectionOrder {
    /// 公開日の順（公開日のない回は保存日、棚に無い回は入れた日）。
    Published,
    /// 題の話数の順。話数のない回は末尾、同じ番号なら公開日の順。
    Episode,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CollectionEdit {
    /// 新しいコレクション。回は渡した順。
    Create {
        title: String,
        description: Option<String>,
        ordered: bool,
        works: Vec<LibraryId>,
    },
    /// 名前・説明・並び方。`name_source`（名前を誰が付けたか）は、渡したときだけ変える。
    Describe {
        title: String,
        description: Option<String>,
        ordered: bool,
        name_source: Option<String>,
    },
    /// 表紙。`cover_work` が None なら作品の表紙を使わない。`cover_mode` は渡したときだけ変える。
    /// `cover_image`（表紙に選んだ画像ファイル）は `Some` のときだけ変える（`Some(None)` で外す）。
    Look {
        cover_work: Option<LibraryId>,
        cover_mode: Option<String>,
        cover_image: Option<Option<Fingerprint>>,
    },
    /// 末尾に足す。もう入っている作品は飛ばす。
    Add {
        works: Vec<LibraryId>,
    },
    Remove {
        works: Vec<LibraryId>,
    },
    /// 回の並び。今の回をすべて一度ずつ渡す。
    Reorder {
        works: Vec<LibraryId>,
    },
    Sort {
        by: CollectionOrder,
    },
    Delete,
}

/// 回を変えない編集の中身。None の項目は今のまま。
struct Head {
    describe: Option<(String, Option<String>, bool)>,
    cover: Option<Option<LibraryId>>,
    cover_image: Option<Option<Fingerprint>>,
    cover_mode: Option<String>,
    name_source: Option<String>,
}

struct Member {
    saved: CollectionMember,
    title: String,
    published: i64,
}

struct Current {
    title: String,
    description: Option<String>,
    ordered: bool,
    generation: i64,
    members: Vec<Member>,
}

fn id_of(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("識別子が不正です".into()))
}

impl Library {
    /// 取得元と取得元の ID から作品を引く（棚に載っていない作品も）。
    pub fn work_by_key(&self, provider: &str, key: &str) -> Result<Option<LibraryId>> {
        self.reader()?
            .query_row(
                "SELECT entity_id FROM external_identities WHERE provider=?1 AND kind='work' AND external_key=?2",
                params![provider, key],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(id_of)
            .transpose()
    }

    /// 保存した版のある作品だけを引く。取得元の一覧や関係から識別だけが先にできた作品
    /// （版がまだ無い）は、保存済みとして扱わない。
    pub fn saved_work_by_key(&self, provider: &str, key: &str) -> Result<Option<LibraryId>> {
        self.reader()?
            .query_row(
                "SELECT x.entity_id FROM external_identities x
                 JOIN entity_current c ON c.entity_id=x.entity_id
                 WHERE x.provider=?1 AND x.kind='work' AND x.external_key=?2",
                params![provider, key],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(id_of)
            .transpose()
    }

    fn current_collection(&self, collection: LibraryId) -> Result<Option<Current>> {
        let conn = self.reader()?;
        let head: Option<(String, Option<String>, bool, i64)> = conn
            .query_row(
                "SELECT c.title,c.description,c.ordered,c.generation FROM collections c
                 JOIN entities e ON e.id=c.id AND e.deleted_at IS NULL WHERE c.id=?1",
                [collection.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((title, description, ordered, generation)) = head else {
            return Ok(None);
        };
        // 画面に出る並び（位置・入れた順・取得元の ID）で読み、位置を振り直す。
        let mut statement = conn.prepare(
            "SELECT m.work_id,m.title_at_add,m.author_at_add,m.role,m.added_by,m.pinned,m.note,
                    coalesce(p.title,m.title_at_add),coalesce(p.published_at,p.saved_at,m.created_at)
             FROM collection_members m
             JOIN external_identities x ON x.entity_id=m.work_id AND x.kind='work'
             LEFT JOIN work_current_projection p ON p.work_id=m.work_id
             WHERE m.collection_id=?1
             ORDER BY m.position,m.created_at,x.provider,x.external_key",
        )?;
        let members = statement
            .query_map([collection.as_bytes().as_slice()], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            })?
            .map(|row| {
                let (
                    work,
                    title_at_add,
                    author_at_add,
                    role,
                    added_by,
                    pinned,
                    note,
                    title,
                    published,
                ) = row?;
                Ok(Member {
                    saved: CollectionMember {
                        work_id: id_of(work)?,
                        title_at_add,
                        author_at_add,
                        role,
                        added_by,
                        pinned,
                        note,
                    },
                    title,
                    published,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(Current {
            title,
            description,
            ordered,
            generation,
            members,
        }))
    }

    /// 足す作品の、入れた時の題と作者。棚に載っていない作品は断る。
    fn new_members(&self, works: &[LibraryId]) -> Result<Vec<Member>> {
        let conn = self.reader()?;
        works
            .iter()
            .map(|work| {
                let (title, author, published): (String, String, i64) = conn
                    .query_row(
                        "SELECT title,author_name,coalesce(published_at,saved_at) FROM work_current_projection WHERE work_id=?1",
                        [work.as_bytes().as_slice()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?
                    .ok_or_else(|| Error::Integrity("棚に載っていない作品は入れられません".into()))?;
                Ok(Member {
                    saved: CollectionMember {
                        work_id: *work,
                        title_at_add: title.clone(),
                        author_at_add: author,
                        role: "main".into(),
                        added_by: "manual".into(),
                        pinned: false,
                        note: None,
                    },
                    title,
                    published,
                })
            })
            .collect()
    }

    /// 一回の編集を書く。作ったときは `collection` がその ID になる。
    pub fn edit_collection(
        &self,
        request: LibraryId,
        collection: LibraryId,
        change: CollectionEdit,
    ) -> Result<()> {
        if self.outbox_has(request)? {
            return Ok(());
        }
        let current = self.current_collection(collection)?;
        let (title, description, ordered, generation, mut members) = match (&change, current) {
            (
                CollectionEdit::Create {
                    title,
                    description,
                    ordered,
                    works,
                },
                None,
            ) => {
                let mut seen = HashSet::new();
                let unique: Vec<LibraryId> =
                    works.iter().copied().filter(|w| seen.insert(*w)).collect();
                (
                    title.clone(),
                    description.clone(),
                    *ordered,
                    0,
                    self.new_members(&unique)?,
                )
            }
            (CollectionEdit::Create { .. }, Some(_)) => {
                return Err(Error::Integrity("同じ ID のコレクションがあります".into()))
            }
            (_, None) => return Err(Error::ResourceUnavailable),
            (_, Some(current)) => (
                current.title,
                current.description,
                current.ordered,
                current.generation,
                current.members,
            ),
        };
        match change {
            CollectionEdit::Create { .. } => {}
            CollectionEdit::Describe {
                title,
                description,
                ordered,
                name_source,
            } => {
                if title.trim().is_empty() {
                    return Err(Error::Integrity("コレクション名が空です".into()));
                }
                return self.write_collection_head(
                    request,
                    collection,
                    generation,
                    Head {
                        describe: Some((title, description, ordered)),
                        cover: None,
                        cover_image: None,
                        cover_mode: None,
                        name_source,
                    },
                );
            }
            CollectionEdit::Look {
                cover_work,
                cover_mode,
                cover_image,
            } => {
                return self.write_collection_head(
                    request,
                    collection,
                    generation,
                    Head {
                        describe: None,
                        cover: Some(cover_work),
                        cover_image,
                        cover_mode,
                        name_source: None,
                    },
                )
            }
            CollectionEdit::Delete => {
                return self.write_collection_delete(request, collection, generation)
            }
            CollectionEdit::Add { works } => {
                let present: HashSet<LibraryId> = members.iter().map(|m| m.saved.work_id).collect();
                let mut seen = HashSet::new();
                let fresh: Vec<LibraryId> = works
                    .into_iter()
                    .filter(|w| !present.contains(w) && seen.insert(*w))
                    .collect();
                members.extend(self.new_members(&fresh)?);
            }
            CollectionEdit::Remove { works } => {
                let doomed: HashSet<LibraryId> = works.into_iter().collect();
                members.retain(|m| !doomed.contains(&m.saved.work_id));
            }
            CollectionEdit::Reorder { works } => {
                let mut by_id: std::collections::HashMap<LibraryId, Member> =
                    members.into_iter().map(|m| (m.saved.work_id, m)).collect();
                let reordered = works
                    .iter()
                    .map(|w| by_id.remove(w))
                    .collect::<Option<Vec<_>>>()
                    .filter(|_| by_id.is_empty())
                    .ok_or_else(|| Error::Integrity("並べ替えに今の回がそろっていません".into()))?;
                members = reordered;
            }
            CollectionEdit::Sort { by } => {
                // 安定な並べ替えなので、同じ値は今の並びのまま。
                match by {
                    CollectionOrder::Published => members.sort_by_key(|m| m.published),
                    CollectionOrder::Episode => members.sort_by(|a, b| {
                        match (episode_order(&a.title), episode_order(&b.title)) {
                            (Some(x), Some(y)) => x.cmp(&y),
                            (Some(_), None) => Ordering::Less,
                            (None, Some(_)) => Ordering::Greater,
                            (None, None) => Ordering::Equal,
                        }
                        .then(a.published.cmp(&b.published))
                    }),
                }
            }
        }
        self.save_collection(CollectionSave {
            request_id: request,
            id: collection,
            expected_generation: generation,
            title,
            description,
            ordered,
            members: members.into_iter().map(|m| m.saved).collect(),
        })?;
        Ok(())
    }

    fn outbox_has(&self, request: LibraryId) -> Result<bool> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT 1 FROM outbox WHERE request_id=?1",
                [request.as_bytes().as_slice()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// 世代を確かめて上げ、outbox に記録する（見た目と削除で共通）。
    fn bump_collection(
        tx: &rusqlite::Transaction<'_>,
        request: LibraryId,
        collection: LibraryId,
        expected: i64,
        operation: &str,
        payload: serde_json::Value,
        now: i64,
    ) -> Result<()> {
        let changed = tx.execute(
            "UPDATE collections SET generation=generation+1,updated_at=?3 WHERE id=?1 AND generation=?2",
            params![collection.as_bytes().as_slice(), expected, now],
        )?;
        if changed != 1 {
            return Err(Error::Integrity(
                "コレクションが別の操作で変更されました".into(),
            ));
        }
        tx.execute(
            "INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![
                request.as_bytes().as_slice(),
                collection.as_bytes().as_slice(),
                operation,
                payload.to_string(),
                now
            ],
        )?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        Ok(())
    }

    /// 名前・説明・並び方と見た目を書く（回は変えない）。
    fn write_collection_head(
        &self,
        request: LibraryId,
        collection: LibraryId,
        expected: i64,
        head: Head,
    ) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let key = collection.as_bytes();
        if let Some(Some(work)) = head.cover {
            tx.query_row(
                "SELECT 1 FROM collection_members WHERE collection_id=?1 AND work_id=?2",
                params![key.as_slice(), work.as_bytes().as_slice()],
                |_| Ok(()),
            )
            .optional()?
            .ok_or_else(|| Error::Integrity("表紙はこのコレクションの作品から選びます".into()))?;
        }
        let appearance: String = tx.query_row(
            "SELECT appearance_json FROM collections WHERE id=?1",
            [key.as_slice()],
            |r| r.get(0),
        )?;
        let mut appearance: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&appearance).unwrap_or_default();
        for (name, value) in [
            ("cover_mode", &head.cover_mode),
            ("name_source", &head.name_source),
        ] {
            if let Some(value) = value {
                appearance.insert(name.into(), serde_json::Value::String(value.clone()));
            }
        }
        tx.execute(
            "UPDATE collections SET appearance_json=?2 WHERE id=?1",
            params![
                key.as_slice(),
                serde_json::Value::Object(appearance).to_string()
            ],
        )?;
        if let Some((title, description, ordered)) = &head.describe {
            tx.execute(
                "UPDATE collections SET title=?2,description=?3,ordered=?4 WHERE id=?1",
                params![key.as_slice(), title, description, ordered],
            )?;
        }
        if let Some(cover) = head.cover {
            tx.execute(
                "UPDATE collections SET cover_work_id=?2 WHERE id=?1",
                params![key.as_slice(), cover.map(|w| w.as_bytes().to_vec())],
            )?;
        }
        if let Some(image) = head.cover_image {
            if let Some(file) = image {
                tx.query_row(
                    "SELECT 1 FROM content_files WHERE id=?1",
                    [file.0.as_slice()],
                    |_| Ok(()),
                )
                .optional()?
                .ok_or_else(|| Error::Integrity("表紙の画像が棚にありません".into()))?;
            }
            tx.execute(
                "UPDATE collections SET cover_file_id=?2 WHERE id=?1",
                params![key.as_slice(), image.map(|f| f.0.to_vec())],
            )?;
        }
        let payload = serde_json::json!({
            "describe": head.describe,
            "cover": head.cover,
            "cover_image": head.cover_image.map(|image| image.map(|f| f.to_string())),
            "cover_mode": head.cover_mode,
            "name_source": head.name_source,
        });
        let now = chrono::Utc::now().timestamp_millis();
        Self::bump_collection(
            &tx,
            request,
            collection,
            expected,
            "collection_head",
            payload,
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 消した印を付け、回を外す。作品と、その版・編集は残る。
    fn write_collection_delete(
        &self,
        request: LibraryId,
        collection: LibraryId,
        expected: i64,
    ) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().timestamp_millis();
        Self::bump_collection(
            &tx,
            request,
            collection,
            expected,
            "collection_delete",
            serde_json::json!({}),
            now,
        )?;
        tx.execute(
            "DELETE FROM collection_members WHERE collection_id=?1",
            [collection.as_bytes().as_slice()],
        )?;
        tx.execute(
            "UPDATE entities SET deleted_at=?2 WHERE id=?1 AND deleted_at IS NULL",
            params![collection.as_bytes().as_slice(), now],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "library_collection_edit_tests.rs"]
mod tests;
