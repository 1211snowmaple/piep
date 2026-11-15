//! 新しい棚の作者・シリーズを、作者・シリーズの画面が出す形にする（評価文書 6.21）。
//!
//! 画面は旧棚の作者・シリーズの画面をそのまま使い、読む先だけを差し替える。数え方・並べ方は
//! 旧棚の問い合わせ（`get_person`・`get_series`・`list_entity_tags`・`list_entity_series_paged`）
//! に合わせ、`piep-library compare-entities` で全員・全シリーズを突き合わせる。

use crate::library::{
    EntityKind, Fingerprint, Library, LibraryId, ResourceReference, ResourceScope, Result,
    WorkFilter, WorkListQuery,
};
use crate::library_view::VersionView;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::cmp::Ordering;

/// 作者・シリーズの画面の頭。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct EntityHead {
    pub id: LibraryId,
    /// `person` か `series`。
    pub kind: &'static str,
    pub provider: String,
    pub external_key: String,
    /// 作者の名前、またはシリーズの題。
    pub name: String,
    pub description: Option<String>,
    /// 作者のプロフィールのリンク（取得元の書いたまま）。
    pub links: Vec<String>,
    /// 作者のアイコン・ヘッダー画像、シリーズの表紙。ファイルの識別子（16 進）で、
    /// `convertFileSrc(…, "piep-library")` で URL になる。
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub cover: Option<String>,
    /// シリーズだけ。
    pub is_concluded: Option<bool>,
    pub published_content_count: Option<i64>,
    /// 棚にある作品の数（作者は役を問わず載っている作品、シリーズは属する作品）。
    pub work_count: u64,
    /// 今の版と、その番号。作品にだけ名前の載った作者・シリーズは版を持たない。
    pub snapshot: Option<LibraryId>,
    pub version_number: Option<i64>,
    /// ミリ秒。取得元を確かめた・取ってきた最後の時。
    pub last_checked_at: Option<i64>,
    pub last_fetched_at: Option<i64>,
    /// 保存した版（古い順）。
    pub versions: Vec<VersionView>,
}

/// 作者・シリーズの作品に付いたタグと、付いた作品の数。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TagCount {
    pub name: String,
    pub count: u64,
}

/// 作者のシリーズの一件（旧棚の `EntityFacet` の形）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SeriesFacet {
    pub id: LibraryId,
    pub provider: String,
    pub external_key: String,
    pub name: String,
    /// この作者の、このシリーズの作品の数。
    pub work_count: u64,
    /// 表紙のファイルの識別子（16 進）。
    pub cover: Option<String>,
    pub description: Option<String>,
    pub is_concluded: Option<bool>,
    /// シリーズの最初の作品の題。
    pub sample_title: Option<String>,
    /// ミリ秒。この作者の、このシリーズの作品のうち最も新しい保存・公開・更新。
    pub latest_saved_at: i64,
    pub latest_published_at: i64,
    pub latest_updated_at: i64,
    pub total_text_length: u64,
    pub total_size: u64,
    pub total_media: u64,
    /// 作者の改稿を保存した数（修復の版は数えない）。
    pub total_revisions: u64,
}

/// 作者のシリーズを並べる鍵。旧棚の `list_entity_series_paged` と同じ語彙。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesSort {
    WorkCount,
    DownloadedAt,
    SourceCreatedAt,
    SourceUpdatedAt,
    Name,
    TextLength,
    FileSizeBytes,
    AssetCount,
    CurrentVersion,
}

impl SeriesSort {
    /// 向きを指さないときの向き（名前だけ昇順）。
    pub fn ascending_by_default(self) -> bool {
        self == Self::Name
    }
}

/// 作者のシリーズの一ページ。続きは `next` を `offset` に渡す。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SeriesFacetPage {
    pub items: Vec<SeriesFacet>,
    pub next: Option<usize>,
    pub total: usize,
}

const SERIES_PAGE_LIMIT: usize = 200;

fn id(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes)
        .map_err(|_| crate::library::Error::Integrity("識別子の長さが不正です".into()))
}

fn fingerprint(bytes: Option<Vec<u8>>) -> Result<Option<String>> {
    bytes
        .map(|bytes| {
            bytes
                .try_into()
                .map(|bytes| Fingerprint(bytes).to_string())
                .map_err(|_| crate::library::Error::Integrity("画像のハッシュが不正です".into()))
        })
        .transpose()
}

fn text(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

/// 旧棚の `LIKE … COLLATE NOCASE`（ASCII だけ大文字と小文字を区別しない）と同じ一致。
fn contains_nocase(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// SQLite の `COLLATE NOCASE` と同じ比べ方（ASCII だけ大文字を小文字に寄せる）。
fn nocase(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|b| b.to_ascii_lowercase())
        .cmp(right.bytes().map(|b| b.to_ascii_lowercase()))
}

impl Library {
    /// 取得元の ID から作者・シリーズの画面の頭を引く。棚に無ければ `None`。
    pub fn entity_head(
        &self,
        kind: EntityKind,
        provider: &str,
        key: &str,
    ) -> Result<Option<EntityHead>> {
        let Some(entity) = self.entity_by_key(kind, provider, key)? else {
            return Ok(None);
        };
        let snapshots = self.snapshots(entity)?;
        // 作品にだけ名前の載った作者・シリーズは、自分の版を持たない（プロフィールを取っていない）。
        let current: Option<Vec<u8>> = self
            .reader()?
            .query_row(
                "SELECT snapshot_id FROM entity_current WHERE entity_id=?1",
                [entity.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let snapshot = current.as_ref().and_then(|current| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.id.as_bytes().as_slice() == current.as_slice())
        });
        let media = |role: &str| -> Result<Option<String>> {
            let Some(current) = &current else {
                return Ok(None);
            };
            fingerprint(
                self.reader()?
                    .query_row(
                        "SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id
                         WHERE m.snapshot_id=?1 AND m.role=?2 ORDER BY m.slot LIMIT 1",
                        params![current, role],
                        |r| r.get(0),
                    )
                    .optional()?,
            )
        };
        let empty = Default::default();
        let metadata = snapshot.map_or(&empty, |snapshot| &snapshot.metadata);
        let series = kind == EntityKind::Series;
        let filter = if series {
            WorkFilter {
                series: Some(entity),
                ..WorkFilter::default()
            }
        } else {
            WorkFilter {
                person: Some(entity),
                ..WorkFilter::default()
            }
        };
        let work_count = self
            .list_works(&WorkListQuery {
                limit: 1,
                filter,
                count: true,
                ..WorkListQuery::default()
            })?
            .total
            .unwrap_or(0);
        let state = self.source_entity_state(entity)?;
        let mut versions: std::collections::BTreeMap<i64, VersionView> = Default::default();
        for snapshot in &snapshots {
            versions.insert(
                snapshot.revision,
                VersionView {
                    snapshot: snapshot.id,
                    version: snapshot.revision,
                    saved_at: snapshot.saved_at,
                    text_length: None,
                },
            );
        }
        Ok(Some(EntityHead {
            id: entity,
            kind: if series { "series" } else { "person" },
            provider: provider.into(),
            external_key: key.into(),
            name: match text(metadata.get(if series { "title" } else { "display_name" })) {
                Some(name) => name,
                None => self
                    .listed_name(series, entity)?
                    .unwrap_or_else(|| key.into()),
            },
            description: text(metadata.get("description")),
            links: metadata
                .get("links")
                .and_then(serde_json::Value::as_array)
                .map(|links| {
                    links
                        .iter()
                        .filter_map(|link| link.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            icon: if series { None } else { media("icon")? },
            banner: if series { None } else { media("banner")? },
            cover: if series { media("cover")? } else { None },
            is_concluded: series
                .then(|| {
                    metadata
                        .get("is_concluded")
                        .and_then(serde_json::Value::as_bool)
                })
                .flatten(),
            published_content_count: series
                .then(|| {
                    metadata
                        .get("published_content_count")
                        .and_then(serde_json::Value::as_i64)
                })
                .flatten(),
            work_count,
            snapshot: snapshot.map(|snapshot| snapshot.id),
            version_number: snapshot.map(|snapshot| snapshot.revision),
            last_checked_at: state.last_checked_at,
            last_fetched_at: state.last_fetched_at,
            versions: versions.into_values().collect(),
        }))
    }

    /// 画面の「保存データ」の欄に出す、版を作った取得元の応答（最も新しく受け取ったもの）。
    /// 旧棚の行を写した記録（`legacy-current-row`）は取得元の応答ではないので除く。旧棚の
    /// 画面は最新の版の JSON を出しており、その JSON は取り込みの記録の側にある。
    pub fn entity_profile(&self, entity: LibraryId, snapshot: LibraryId) -> Result<Option<String>> {
        let observation: Option<Vec<u8>> = self
            .reader()?
            .query_row(
                "SELECT o.id FROM snapshot_observations so JOIN acquisition_observations o ON o.id=so.observation_id AND o.entity_id=so.entity_id
                 WHERE so.snapshot_id=?1 AND so.entity_id=?2 AND o.raw_file_id IS NOT NULL AND o.route<>'legacy-current-row'
                 ORDER BY o.acquired_at DESC,o.recorded_at DESC,o.id DESC LIMIT 1",
                params![snapshot.as_bytes().as_slice(), entity.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(observation) = observation else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        self.read_resource(
            entity,
            ResourceScope::Snapshot(snapshot),
            &ResourceReference::Original {
                observation: id(observation)?,
            },
            None,
            &mut bytes,
        )?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// 作品に載った作者の名前・シリーズの題（新しい作品のものから）。
    fn listed_name(&self, series: bool, entity: LibraryId) -> Result<Option<String>> {
        let sql = if series {
            "SELECT r.title FROM snapshot_series r JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             WHERE r.series_id=?1 AND r.title<>'' ORDER BY p.saved_at DESC LIMIT 1"
        } else {
            "SELECT r.display_name FROM snapshot_people r JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             WHERE r.person_id=?1 AND r.display_name<>'' ORDER BY p.saved_at DESC LIMIT 1"
        };
        Ok(self
            .reader()?
            .query_row(sql, [entity.as_bytes().as_slice()], |r| r.get(0))
            .optional()?)
    }

    /// 作者・シリーズの作品に付いた取得元のタグ。多い順、同じ数は名前の順（旧棚と同じ）。
    pub fn entity_tags(
        &self,
        kind: EntityKind,
        entity: LibraryId,
        limit: usize,
    ) -> Result<Vec<TagCount>> {
        let relation = match kind {
            EntityKind::Series => "SELECT snapshot_id FROM snapshot_series WHERE series_id=?1",
            _ => "SELECT snapshot_id FROM snapshot_people WHERE person_id=?1",
        };
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(&format!(
            "SELECT t.name,count(DISTINCT p.work_id) AS n FROM work_current_projection p
             JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
             WHERE p.snapshot_id IN ({relation})
             GROUP BY t.id ORDER BY n DESC,t.name ASC LIMIT ?2"
        ))?;
        let rows = statement.query_map(
            params![entity.as_bytes().as_slice(), limit.clamp(1, 200) as i64],
            |r| {
                Ok(TagCount {
                    name: r.get(0)?,
                    count: r.get::<_, i64>(1)? as u64,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 作者の作品が属するシリーズ。旧棚と同じく、名前か説明に語を含むものに絞り、鍵で並べ、
    /// 同じ値は作品の多い順・名前の順に並べる。
    pub fn person_series(
        &self,
        person: LibraryId,
        query: &str,
        sort: SeriesSort,
        ascending: bool,
        offset: usize,
        limit: usize,
    ) -> Result<SeriesFacetPage> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(
            "SELECT ss.series_id,x.provider,x.external_key,
                    max(nullif(ss.title,'')),
                    count(DISTINCT p.work_id),
                    max(p.saved_at),max(coalesce(p.published_at,p.saved_at)),
                    max(coalesce(p.updated_at,p.published_at,p.saved_at)),
                    sum(p.text_length),sum(p.logical_size),sum(p.media_count),
                    sum(p.version_number-1)
             FROM work_current_projection p
             JOIN snapshot_series ss ON ss.snapshot_id=p.snapshot_id
             JOIN external_identities x ON x.entity_id=ss.series_id AND x.kind='series'
             WHERE p.snapshot_id IN (SELECT snapshot_id FROM snapshot_people WHERE person_id=?1)
             GROUP BY ss.series_id",
        )?;
        type Row = (
            Vec<u8>,
            String,
            String,
            Option<String>,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        );
        let rows: Vec<Row> = statement
            .query_map([person.as_bytes().as_slice()], |r| {
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
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(statement);
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let (
                series,
                provider,
                key,
                listed_title,
                count,
                saved,
                published,
                updated,
                length,
                size,
                media,
                revisions,
            ) = row;
            let (metadata, cover): (Option<String>, Option<Vec<u8>>) = conn.query_row(
                "SELECT s.metadata_json,(SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=s.id AND m.role='cover' ORDER BY m.slot LIMIT 1)
                 FROM entity_current c JOIN revision_snapshots s ON s.id=c.snapshot_id WHERE c.entity_id=?1",
                [&series],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?.unwrap_or((None, None));
            let metadata: serde_json::Map<String, serde_json::Value> = metadata
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();
            let sample_title: Option<String> = conn
                .query_row(
                    "SELECT p.title FROM snapshot_series ss JOIN work_current_projection p ON p.snapshot_id=ss.snapshot_id
                     JOIN library_items i ON i.work_id=p.work_id
                     WHERE ss.series_id=?1 ORDER BY coalesce(ss.position,999999),i.added_at,p.work_id LIMIT 1",
                    [&series],
                    |r| r.get(0),
                )
                .optional()?;
            let name = text(metadata.get("title"))
                .or(listed_title.filter(|title| !title.is_empty()))
                .unwrap_or_else(|| key.clone());
            let description = text(metadata.get("description"));
            if !query.is_empty()
                && !contains_nocase(&name, query)
                && !description
                    .as_deref()
                    .is_some_and(|description| contains_nocase(description, query))
            {
                continue;
            }
            items.push(SeriesFacet {
                id: id(series)?,
                provider,
                external_key: key,
                name,
                work_count: count as u64,
                cover: fingerprint(cover)?,
                description,
                is_concluded: metadata
                    .get("is_concluded")
                    .and_then(serde_json::Value::as_bool),
                sample_title,
                latest_saved_at: saved,
                latest_published_at: published,
                latest_updated_at: updated,
                total_text_length: length as u64,
                total_size: size as u64,
                total_media: media as u64,
                total_revisions: revisions as u64,
            });
        }
        let key = |item: &SeriesFacet| -> i64 {
            match sort {
                SeriesSort::WorkCount => item.work_count as i64,
                SeriesSort::DownloadedAt => item.latest_saved_at,
                SeriesSort::SourceCreatedAt => item.latest_published_at,
                SeriesSort::SourceUpdatedAt => item.latest_updated_at,
                SeriesSort::TextLength => item.total_text_length as i64,
                SeriesSort::FileSizeBytes => item.total_size as i64,
                SeriesSort::AssetCount => item.total_media as i64,
                SeriesSort::CurrentVersion => item.total_revisions as i64,
                SeriesSort::Name => 0,
            }
        };
        items.sort_by(|left, right| {
            let primary = if sort == SeriesSort::Name {
                nocase(&left.name, &right.name)
            } else {
                key(left).cmp(&key(right))
            };
            let primary = if ascending {
                primary
            } else {
                primary.reverse()
            };
            primary
                .then_with(|| right.work_count.cmp(&left.work_count))
                .then_with(|| nocase(&left.name, &right.name))
                .then_with(|| left.provider.cmp(&right.provider))
                .then_with(|| left.external_key.cmp(&right.external_key))
        });
        let total = items.len();
        let limit = limit.clamp(1, SERIES_PAGE_LIMIT);
        let end = (offset + limit).min(total);
        let next = (end < total).then_some(end);
        let items = items
            .into_iter()
            .skip(offset)
            .take(end.saturating_sub(offset))
            .collect();
        Ok(SeriesFacetPage { items, next, total })
    }
}

#[cfg(test)]
#[path = "library_entity_tests.rs"]
mod tests;
