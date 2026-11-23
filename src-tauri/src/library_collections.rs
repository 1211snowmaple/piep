//! 棚のコレクションの一覧と中身（評価文書 6.23）。
//!
//! ライブラリの「コレクション」タブと、コレクションの画面が読む。並び・数え方・別版の畳み方は、
//! 切り替えの前に全コレクションを突き合わせて合わせたもの。

use crate::analysis::collection_rules::edition_match_key;
use crate::library::{Error, Library, LibraryId, Result, WorkCard};
use crate::library_entity::fingerprint;
use crate::library_entity_list::{collection_summaries, CollectionSummary};
use crate::search_engine::text::normalize_search_text;
use rusqlite::params;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// 別版を探すときに見る作品の上限。
const EDITION_SCAN_LIMIT: usize = 3_000;

/// コレクションの一回（画面の `WorkCollectionMember`）。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CollectionMember {
    pub provider: String,
    pub external_key: String,
    /// 棚にある作品の、一覧の一件の形。棚に無い回（未保存）は None。
    /// 画面へは命令の側が表紙の識別子を添えて渡すので、空のときは書き出さない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<WorkCard>,
    /// 今の版の題と作者。棚に無い回は入れた時の題と作者。
    pub title: String,
    pub author_name: String,
    /// 表紙のファイルの識別子（16 進）。棚に無い回は None。
    pub cover: Option<String>,
    pub text_length: u64,
    pub position: i64,
    pub role: String,
    pub added_by: String,
    pub pinned: bool,
    pub note: Option<String>,
    /// 棚に無い（未保存の）回。
    pub missing: bool,
    /// ミリ秒。
    pub created_at: i64,
    pub updated_at: i64,
    /// 同じ作品の別版（題の版の呼び名を除いた鍵と作者が同じ、コレクションに入っていない作品）。
    /// 本文の長い順。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub editions: Vec<WorkCard>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CollectionDetail {
    #[serde(flatten)]
    pub summary: CollectionSummary,
    pub members: Vec<CollectionMember>,
}

impl Library {
    /// コレクションの一覧。更新の新しい順。
    pub fn list_collections(&self) -> Result<Vec<CollectionSummary>> {
        let conn = self.reader()?;
        collection_summaries(&conn, "1", &[])
    }

    /// コレクションの要約だけ（回と別版は引かない）。無ければ None。
    pub fn collection_summary(&self, collection: LibraryId) -> Result<Option<CollectionSummary>> {
        let conn = self.reader()?;
        Ok(
            collection_summaries(&conn, "c.id=?1", &[collection.as_bytes().to_vec()])?
                .into_iter()
                .next(),
        )
    }

    /// コレクションとその回。回は並べた順、同じ位置は入れた順。
    pub fn collection_detail(&self, collection: LibraryId) -> Result<CollectionDetail> {
        let conn = self.reader()?;
        let summary = collection_summaries(&conn, "c.id=?1", &[collection.as_bytes().to_vec()])?
            .into_iter()
            .next()
            .ok_or(Error::ResourceUnavailable)?;
        type Row = (
            Vec<u8>,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            Option<Vec<u8>>,
            i64,
            i64,
            String,
            String,
            bool,
            Option<String>,
            bool,
            i64,
            i64,
        );
        let rows: Vec<Row> = conn
            .prepare_cached(
                "SELECT m.work_id,x.provider,x.external_key,p.title,p.author_name,m.title_at_add,m.author_at_add,
                        (SELECT r.file_id FROM snapshot_media sm JOIN media_renditions r ON r.id=sm.rendition_id
                          WHERE sm.snapshot_id=p.snapshot_id AND sm.role='cover' ORDER BY sm.slot LIMIT 1),
                        coalesce(p.text_length,0),m.position,m.role,m.added_by,m.pinned,m.note,
                        p.work_id IS NULL,m.created_at,m.updated_at
                 FROM collection_members m
                 JOIN external_identities x ON x.entity_id=m.work_id AND x.kind='work'
                 LEFT JOIN work_current_projection p ON p.work_id=m.work_id
                 WHERE m.collection_id=?1
                 ORDER BY m.position,m.created_at,x.provider,x.external_key",
            )?
            .query_map(params![collection.as_bytes().as_slice()], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                    r.get(13)?,
                    r.get(14)?,
                    r.get(15)?,
                    r.get(16)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(conn);
        let present: Vec<LibraryId> = rows
            .iter()
            .filter(|row| !row.14)
            .map(|row| crate::library_entity::id(row.0.clone()))
            .collect::<Result<_>>()?;
        let mut cards: HashMap<LibraryId, WorkCard> = self
            .work_cards(&present)?
            .into_iter()
            .flatten()
            .map(|card| (card.id, card))
            .collect();
        let mut editions = self.member_editions(&cards)?;
        let mut members = Vec::with_capacity(rows.len());
        for row in rows {
            let (
                work,
                provider,
                external_key,
                title,
                author,
                title_at_add,
                author_at_add,
                cover,
                text_length,
                position,
                role,
                added_by,
                pinned,
                note,
                missing,
                created_at,
                updated_at,
            ) = row;
            let work = crate::library_entity::id(work)?;
            members.push(CollectionMember {
                provider,
                external_key,
                work: cards.remove(&work),
                title: title.unwrap_or(title_at_add),
                author_name: author.unwrap_or(author_at_add),
                cover: fingerprint(cover)?,
                text_length: text_length.max(0) as u64,
                position,
                role,
                added_by,
                pinned,
                note,
                missing,
                created_at,
                updated_at,
                editions: editions.remove(&work).unwrap_or_default(),
            });
        }
        Ok(CollectionDetail { summary, members })
    }

    /// 回ごとの別版。作者の名前で棚を絞り、正規化した名前と
    /// 題の鍵（版の呼び名を除く）が回と同じ、コレクションに入っていない作品を集める。
    fn member_editions(
        &self,
        members: &HashMap<LibraryId, WorkCard>,
    ) -> Result<HashMap<LibraryId, Vec<WorkCard>>> {
        let mut keys: HashMap<String, Vec<LibraryId>> = HashMap::new();
        let mut authors: HashSet<String> = HashSet::new();
        let mut names: HashSet<String> = HashSet::new();
        for card in members.values() {
            keys.entry(edition_match_key(&card.title))
                .or_default()
                .push(card.id);
            let name = card.author_name.trim();
            if !name.is_empty() {
                authors.insert(normalize_search_text(name));
                names.insert(name.to_owned());
            }
        }
        if authors.is_empty() {
            return Ok(HashMap::new());
        }
        let encoded = serde_json::to_string(&names.iter().collect::<Vec<_>>())?;
        let conn = self.reader()?;
        let mut found: HashMap<LibraryId, Vec<LibraryId>> = HashMap::new();
        let mut statement = conn.prepare_cached(
            "SELECT work_id,title,author_name FROM work_current_projection
             WHERE author_name IN (SELECT value FROM json_each(?1)) LIMIT ?2",
        )?;
        let rows = statement.query_map(params![encoded, EDITION_SCAN_LIMIT as i64 + 1], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for (scanned, row) in rows.enumerate() {
            if scanned >= EDITION_SCAN_LIMIT {
                log::warn!("別版の探索を {EDITION_SCAN_LIMIT} 件で止めました");
                break;
            }
            let (work, title, author) = row?;
            let work = crate::library_entity::id(work)?;
            if members.contains_key(&work) || !authors.contains(&normalize_search_text(&author)) {
                continue;
            }
            if let Some(owners) = keys.get(&edition_match_key(&title)) {
                for owner in owners {
                    found.entry(*owner).or_default().push(work);
                }
            }
        }
        drop(statement);
        drop(conn);
        if found.is_empty() {
            return Ok(HashMap::new());
        }
        let wanted: Vec<LibraryId> = found
            .values()
            .flatten()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let loaded: HashMap<LibraryId, WorkCard> = self
            .work_cards(&wanted)?
            .into_iter()
            .flatten()
            .map(|card| (card.id, card))
            .collect();
        let mut found: Vec<(LibraryId, Vec<WorkCard>)> = found
            .into_iter()
            .map(|(owner, works)| {
                let cards = works
                    .into_iter()
                    .filter_map(|work| loaded.get(&work).cloned())
                    .collect();
                (owner, cards)
            })
            .collect();
        // 長い本文が先。同じ長さは先に保存した作品が先。
        for (_, cards) in &mut found {
            cards.sort_by(|left, right| {
                right
                    .text_length
                    .cmp(&left.text_length)
                    .then_with(|| left.saved_at.cmp(&right.saved_at))
                    .then_with(|| left.id.as_bytes().cmp(right.id.as_bytes()))
            });
        }
        Ok(found.into_iter().collect())
    }
}

#[cfg(test)]
#[path = "library_collections_tests.rs"]
mod tests;
