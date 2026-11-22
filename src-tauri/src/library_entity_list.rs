//! 棚の作者・シリーズの一覧と、作者・シリーズに関わるコレクション（評価文書 6.22）。
//!
//! ライブラリの「作者・クリエイター」「シリーズ」タブは旧棚の `search_entity_facets`・
//! `count_entity_facets` を、作者・シリーズの画面のコレクションの欄は旧棚の
//! `list_collections_for_person`・`list_collections_for_series` を、そのまま棚で読む。
//! 数え方・並べ方は、旧棚と突き合わせて合わせたもの。

use crate::library::{EntityKind, Library, LibraryId, Result, WorkFilter};
use crate::library_entity::{contains_nocase, fingerprint, id, nocase, text, SeriesSort};
use rusqlite::{params, params_from_iter, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

/// 更新監視の登録で絞る。旧棚の `EntityFacetScope.watch` と同じ三つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchScope {
    /// 登録して動かしているもの。
    Watched,
    /// 登録して止めているもの。
    Paused,
    /// 登録していないもの。
    Unwatched,
}

/// 作者・シリーズ自身の性質で絞る（作品には無い条件）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct EntityScope {
    pub watch: Option<WatchScope>,
    /// これ以上の作品を持つものだけ。1 以下は条件なしと同じ。
    pub min_work_count: Option<u64>,
    /// 完結しているか。シリーズだけに効き、完結を知らないシリーズはどちらにも入らない。
    pub concluded: Option<bool>,
}

/// 一覧の問い合わせ。並べる鍵は作者のシリーズと同じ語彙（[`SeriesSort`]）。
#[derive(Clone, Debug)]
pub struct EntityListQuery {
    pub kind: EntityKind,
    /// 名前か説明に含む語（ASCII だけ大文字と小文字を区別しない）。空なら絞らない。
    pub query: String,
    /// 作品の絞り込み。当てはまる作品を持つものだけを出し、作品の数もその作品で数える。
    pub filter: WorkFilter,
    pub scope: EntityScope,
    pub sort: SeriesSort,
    pub ascending: bool,
    pub offset: usize,
    pub limit: usize,
}

/// 一覧の一件（旧棚の `EntityFacet` の形）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EntityRow {
    pub id: LibraryId,
    pub provider: String,
    pub external_key: String,
    pub name: String,
    /// 絞り込みに当てはまる作品の数。
    pub work_count: u64,
    pub description: Option<String>,
    /// シリーズだけ。
    pub is_concluded: Option<bool>,
    /// 画像のファイルの識別子（16 進）。作者はアイコンとヘッダー、シリーズは表紙。
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub cover: Option<String>,
    /// 作者は最も新しく公開した作品、シリーズは最初の回の題（絞り込みに依らない、旧棚と同じ）。
    pub sample_title: Option<String>,
    /// ミリ秒。作者・シリーズの今の版を保存した時。版がなければ None。
    pub updated_at: Option<i64>,
    /// ミリ秒。当てはまる作品のうち最も新しい保存・公開・更新。
    pub latest_saved_at: i64,
    pub latest_published_at: i64,
    pub latest_updated_at: i64,
    pub total_text_length: u64,
    pub total_size: u64,
    pub total_media: u64,
    pub total_revisions: u64,
    /// 更新監視の登録。`watching`・`paused`、登録がなければ None。
    pub watch: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EntityListPage {
    pub items: Vec<EntityRow>,
    /// 条件に当てはまる数（ページに依らない）。
    pub total: usize,
}

/// コレクションの表紙のモザイクの一マス（旧棚の `CollectionCoverTile`）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CollectionTile {
    pub provider: String,
    pub external_key: String,
    pub title: String,
    pub author_name: String,
    /// 表紙のファイルの識別子（16 進）。
    pub cover: Option<String>,
}

/// 作者・シリーズに関わるコレクションの一件（旧棚の `WorkCollectionSummary`）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CollectionSummary {
    pub id: LibraryId,
    pub name: String,
    pub description: Option<String>,
    pub ordered: bool,
    /// 表紙に選んだ作品の表紙と、表紙に選んだ画像。ファイルの識別子（16 進）。
    pub cover: Option<String>,
    pub cover_image: Option<String>,
    pub cover_mode: String,
    pub name_source: String,
    pub track: String,
    /// 並びの先頭から 4 件。表紙のない回も席を残す。
    pub tiles: Vec<CollectionTile>,
    pub revision: i64,
    pub member_count: u64,
    /// 棚に保存してある回の数。
    pub available_count: u64,
    pub total_text_length: u64,
    /// ミリ秒。
    pub created_at: i64,
    pub updated_at: i64,
}

const ENTITY_PAGE_LIMIT: usize = 200;
pub(crate) const COLLECTION_TILES: i64 = 4;

/// 集計の一行。並べ替えと絞り込みに要るものだけ。画像と見本の題はページの分だけ後で引く。
struct Aggregate {
    entity: Vec<u8>,
    count: u64,
    saved: i64,
    published: i64,
    updated: i64,
    length: u64,
    size: u64,
    media: u64,
    revisions: u64,
}

fn series_kind(kind: EntityKind) -> Result<bool> {
    match kind {
        EntityKind::Person => Ok(false),
        EntityKind::Series => Ok(true),
        _ => Err(crate::library::Error::Unsupported(
            "作者かシリーズを指定してください".into(),
        )),
    }
}

impl Library {
    /// 棚の世代。保存・編集のたびに進むので、一覧の集計を覚えておく鍵に使う。
    pub fn catalogue_generation(&self) -> Result<i64> {
        Ok(self.reader()?.query_row(
            "SELECT generation FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?)
    }

    /// 作者・シリーズの一覧の一ページ（旧棚の `search_entity_facets`）。
    ///
    /// 作品の絞り込みに当てはまる作品を作者・シリーズごとに集め、名前か説明の語・監視・完結・
    /// 作品の数で絞り、旧棚と同じ鍵で並べる。同じ値は作品の多い順・名前の順・取得元の ID の順。
    /// 作者は名前の載った作品だけを数える。
    pub fn list_entities(&self, request: &EntityListQuery) -> Result<EntityListPage> {
        let rows =
            self.entity_rows(request.kind, &request.query, &request.filter, request.scope)?;
        self.entity_page(
            &rows,
            request.kind,
            request.sort,
            request.ascending,
            request.offset,
            request.limit,
        )
    }

    /// 一覧の集計（並べる前、画像と見本の題を除く）。並び順とページに依らないので、画面が
    /// ページをめくる・並べ替えるあいだは覚えておいて [`Self::entity_page`] に渡せる。
    pub fn entity_rows(
        &self,
        kind: EntityKind,
        query: &str,
        filter: &WorkFilter,
        scope: EntityScope,
    ) -> Result<Vec<EntityRow>> {
        let series = series_kind(kind)?;
        let conn = self.reader()?;
        let (condition, values) = self.work_filter_sql(&conn, filter)?;
        let relation = if series {
            "JOIN snapshot_series r ON r.snapshot_id=p.snapshot_id"
        } else {
            "JOIN snapshot_people r ON r.snapshot_id=p.snapshot_id AND r.display_name<>''"
        };
        let column = if series { "r.series_id" } else { "r.person_id" };
        let mut statement = conn.prepare_cached(&format!(
            "SELECT {column},count(DISTINCT p.work_id),
                    max(p.saved_at),max(coalesce(p.published_at,p.saved_at)),
                    max(coalesce(p.updated_at,p.published_at,p.saved_at)),
                    sum(p.text_length),sum(p.logical_size),sum(p.media_count),
                    sum((SELECT count(*) FROM source_revisions v WHERE v.entity_id=p.work_id AND v.ordinal>1 AND v.repair=0))
             FROM work_current_projection p {relation}
             WHERE {condition}
             GROUP BY {column}"
        ))?;
        let aggregates: Vec<Aggregate> = statement
            .query_map(params_from_iter(values.iter().map(|v| v.as_ref())), |r| {
                Ok(Aggregate {
                    entity: r.get(0)?,
                    count: r.get::<_, i64>(1)? as u64,
                    saved: r.get(2)?,
                    published: r.get(3)?,
                    updated: r.get(4)?,
                    length: r.get::<_, i64>(5)? as u64,
                    size: r.get::<_, i64>(6)? as u64,
                    media: r.get::<_, i64>(7)? as u64,
                    revisions: r.get::<_, i64>(8)?.max(0) as u64,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(statement);
        let kind_name = if series { "series" } else { "person" };

        // 取得元の ID。一つの作者・シリーズに複数あれば、取得元・ID の順で最初のもの。
        let mut identities: HashMap<Vec<u8>, (String, String)> = HashMap::new();
        for row in conn
            .prepare_cached("SELECT entity_id,provider,external_key FROM external_identities WHERE kind=?1 ORDER BY provider DESC,external_key DESC")?
            .query_map([kind_name], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get(1)?, r.get(2)?)))?
        {
            let (entity, provider, key) = row?;
            identities.insert(entity, (provider, key));
        }
        // 今の版の名前・説明・完結と、保存した時。
        let mut profiles: HashMap<Vec<u8>, (serde_json::Map<String, serde_json::Value>, i64)> =
            HashMap::new();
        for row in conn
            .prepare_cached(
                "SELECT c.entity_id,s.metadata_json,s.created_at FROM entity_current c
                 JOIN entities e ON e.id=c.entity_id AND e.kind=?1
                 JOIN revision_snapshots s ON s.id=c.snapshot_id",
            )?
            .query_map([kind_name], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?, r.get(2)?))
            })?
        {
            let (entity, metadata, saved) = row?;
            profiles.insert(
                entity,
                (serde_json::from_str(&metadata).unwrap_or_default(), saved),
            );
        }
        let mut watches: HashMap<Vec<u8>, bool> = HashMap::new();
        for row in conn
            .prepare_cached("SELECT entity_id,enabled FROM watch_targets")?
            .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, bool>(1)?)))?
        {
            let (entity, enabled) = row?;
            watches.insert(entity, enabled);
        }
        // 版のない（プロフィールを取っていない）作者・シリーズは、作品に載った名前・題のうち
        // 最も新しく更新された作品のもの。ほとんどの作者・
        // シリーズは版を持つので、持たないものだけを引く。
        let listed_sql = if series {
            "SELECT r.title FROM snapshot_series r JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             JOIN library_items i ON i.work_id=p.work_id WHERE r.series_id=?1 AND r.title<>''
             ORDER BY coalesce(p.updated_at,p.saved_at) DESC,i.added_at DESC LIMIT 1"
        } else {
            "SELECT r.display_name FROM snapshot_people r JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             JOIN library_items i ON i.work_id=p.work_id WHERE r.person_id=?1 AND r.display_name<>''
             ORDER BY coalesce(p.updated_at,p.saved_at) DESC,i.added_at DESC LIMIT 1"
        };

        let minimum = scope.min_work_count.filter(|n| *n > 1);
        let mut rows = Vec::with_capacity(aggregates.len());
        for aggregate in aggregates {
            let Some((provider, key)) = identities.get(&aggregate.entity).cloned() else {
                continue;
            };
            if minimum.is_some_and(|minimum| aggregate.count < minimum) {
                continue;
            }
            let watch = watches.get(&aggregate.entity).copied();
            let watched = match scope.watch {
                Some(WatchScope::Watched) => watch == Some(true),
                Some(WatchScope::Paused) => watch == Some(false),
                Some(WatchScope::Unwatched) => watch.is_none(),
                None => true,
            };
            if !watched {
                continue;
            }
            let profile = profiles.get(&aggregate.entity);
            let metadata = profile.map(|(metadata, _)| metadata);
            let is_concluded = if series {
                metadata
                    .and_then(|m| m.get("is_concluded"))
                    .and_then(serde_json::Value::as_bool)
            } else {
                None
            };
            if series
                && scope
                    .concluded
                    .is_some_and(|wanted| is_concluded != Some(wanted))
            {
                continue;
            }
            let name = match metadata
                .and_then(|m| text(m.get(if series { "title" } else { "display_name" })))
            {
                Some(name) => name,
                None => conn
                    .prepare_cached(listed_sql)?
                    .query_row([&aggregate.entity], |r| r.get(0))
                    .optional()?
                    .unwrap_or_else(|| key.clone()),
            };
            let description = metadata.and_then(|m| text(m.get("description")));
            if !query.is_empty()
                && !contains_nocase(&name, query)
                && !description
                    .as_deref()
                    .is_some_and(|d| contains_nocase(d, query))
            {
                continue;
            }
            rows.push(EntityRow {
                id: id(aggregate.entity)?,
                provider,
                external_key: key,
                name,
                work_count: aggregate.count,
                description,
                is_concluded,
                icon: None,
                banner: None,
                cover: None,
                sample_title: None,
                updated_at: profile.map(|(_, saved)| *saved),
                latest_saved_at: aggregate.saved,
                latest_published_at: aggregate.published,
                latest_updated_at: aggregate.updated,
                total_text_length: aggregate.length,
                total_size: aggregate.size,
                total_media: aggregate.media,
                total_revisions: aggregate.revisions,
                watch: watch.map(|enabled| if enabled { "watching" } else { "paused" }),
            });
        }
        Ok(rows)
    }

    /// 集計を旧棚と同じ鍵で並べ、一ページ分に画像と見本の題を足す。
    pub fn entity_page(
        &self,
        rows: &[EntityRow],
        kind: EntityKind,
        sort: SeriesSort,
        ascending: bool,
        offset: usize,
        limit: usize,
    ) -> Result<EntityListPage> {
        let series = series_kind(kind)?;
        let key = |row: &EntityRow| -> i64 {
            match sort {
                SeriesSort::WorkCount => row.work_count as i64,
                SeriesSort::DownloadedAt => row.latest_saved_at,
                SeriesSort::SourceCreatedAt => row.latest_published_at,
                SeriesSort::SourceUpdatedAt => row.latest_updated_at,
                SeriesSort::TextLength => row.total_text_length as i64,
                SeriesSort::FileSizeBytes => row.total_size as i64,
                SeriesSort::AssetCount => row.total_media as i64,
                SeriesSort::CurrentVersion => row.total_revisions as i64,
                SeriesSort::Name => 0,
            }
        };
        let direct = |ordering: Ordering| {
            if ascending {
                ordering
            } else {
                ordering.reverse()
            }
        };
        let mut order: Vec<&EntityRow> = rows.iter().collect();
        order.sort_by(|left, right| {
            let ordering = match sort {
                // 旧棚: 名前（大文字小文字を区別しない）→ 作品の多い順。
                SeriesSort::Name => direct(nocase(&left.name, &right.name))
                    .then_with(|| right.work_count.cmp(&left.work_count)),
                // 旧棚: 作品の数 → 名前。
                SeriesSort::WorkCount => direct(left.work_count.cmp(&right.work_count))
                    .then_with(|| left.name.cmp(&right.name)),
                // 旧棚: 鍵 → 作品の多い順 → 名前。
                _ => direct(key(left).cmp(&key(right)))
                    .then_with(|| right.work_count.cmp(&left.work_count))
                    .then_with(|| left.name.cmp(&right.name)),
            };
            ordering
                .then_with(|| left.provider.cmp(&right.provider))
                .then_with(|| left.external_key.cmp(&right.external_key))
        });
        let limit = limit.clamp(1, ENTITY_PAGE_LIMIT);
        let mut items: Vec<EntityRow> = order
            .into_iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        let conn = self.reader()?;
        let media = |entity: &[u8], role: &str| -> Result<Option<String>> {
            fingerprint(
                conn.prepare_cached(
                    "SELECT r.file_id FROM entity_current c JOIN snapshot_media m ON m.snapshot_id=c.snapshot_id
                     JOIN media_renditions r ON r.id=m.rendition_id
                     WHERE c.entity_id=?1 AND m.role=?2 ORDER BY m.slot LIMIT 1",
                )?
                .query_row(params![entity, role], |r| r.get(0))
                .optional()?,
            )
        };
        let sample_sql = if series {
            // 旧棚: 話数の順（話数のない回は後ろ）→ 公開の古い順 → 保存の古い順。
            "SELECT p.title FROM snapshot_series r JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             JOIN library_items i ON i.work_id=p.work_id WHERE r.series_id=?1
             ORDER BY r.position IS NULL,r.position,coalesce(p.published_at,p.saved_at),i.added_at,p.work_id LIMIT 1"
        } else {
            // 旧棚: 公開の新しい順 → 保存の新しい順。
            "SELECT p.title FROM snapshot_people r JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             JOIN library_items i ON i.work_id=p.work_id WHERE r.person_id=?1
             ORDER BY coalesce(p.published_at,p.saved_at) DESC,i.added_at DESC,p.work_id DESC LIMIT 1"
        };
        for item in &mut items {
            let entity = item.id.as_bytes().to_vec();
            if series {
                item.cover = media(&entity, "cover")?;
            } else {
                item.icon = media(&entity, "icon")?;
                item.banner = media(&entity, "banner")?;
            }
            item.sample_title = conn
                .prepare_cached(sample_sql)?
                .query_row([&entity], |r| r.get(0))
                .optional()?;
        }
        Ok(EntityListPage {
            items,
            total: rows.len(),
        })
    }

    /// 作者・シリーズの作品を一つでも含むコレクション（旧棚の `list_collections_for_person`・
    /// `list_collections_for_series`）。最後に変えた順、同じなら名前・ID の順。
    pub fn entity_collections(
        &self,
        kind: EntityKind,
        entity: LibraryId,
    ) -> Result<Vec<CollectionSummary>> {
        let relation = match kind {
            EntityKind::Person => "SELECT snapshot_id FROM snapshot_people WHERE person_id=?1",
            EntityKind::Series => "SELECT snapshot_id FROM snapshot_series WHERE series_id=?1",
            _ => {
                return Err(crate::library::Error::Unsupported(
                    "作者かシリーズを指定してください".into(),
                ))
            }
        };
        let conn = self.reader()?;
        let condition = format!(
            "EXISTS(SELECT 1 FROM collection_members m JOIN work_current_projection p ON p.work_id=m.work_id
                    WHERE m.collection_id=c.id AND p.snapshot_id IN ({relation}))"
        );
        collection_summaries(&conn, &condition, &[entity.as_bytes().to_vec()])
    }

    /// 作品の入ったコレクションと、その中での位置（旧棚の `list_collections_for_work`・
    /// `list_collection_positions_for_work`）。並びはコレクションの一覧と同じ。
    pub fn work_collections(&self, work: LibraryId) -> Result<Vec<(CollectionSummary, i64)>> {
        let conn = self.reader()?;
        let summaries = collection_summaries(
            &conn,
            "EXISTS(SELECT 1 FROM collection_members m WHERE m.collection_id=c.id AND m.work_id=?1)",
            &[work.as_bytes().to_vec()],
        )?;
        let mut statement = conn.prepare_cached(
            "SELECT position FROM collection_members WHERE collection_id=?1 AND work_id=?2",
        )?;
        summaries
            .into_iter()
            .map(|summary| {
                let position: i64 = statement.query_row(
                    params![summary.id.as_bytes().as_slice(), work.as_bytes().as_slice()],
                    |r| r.get(0),
                )?;
                Ok((summary, position))
            })
            .collect()
    }
}

/// コレクションの要約を、条件（`c` が `collections`）に当てはまるものだけ、更新の新しい順で。
/// 値は `?1` から順に束縛する。
pub(crate) fn collection_summaries(
    conn: &rusqlite::Connection,
    condition: &str,
    values: &[Vec<u8>],
) -> Result<Vec<CollectionSummary>> {
    type Row = (
        Vec<u8>,
        String,
        Option<String>,
        String,
        bool,
        i64,
        Option<i64>,
        i64,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        i64,
        i64,
        i64,
    );
    let mut statement = conn.prepare_cached(&format!(
            "SELECT c.id,c.title,c.description,c.appearance_json,c.ordered,c.generation,c.updated_at,e.created_at,
                    (SELECT r.file_id FROM work_current_projection cp JOIN snapshot_media m ON m.snapshot_id=cp.snapshot_id
                       JOIN media_renditions r ON r.id=m.rendition_id
                      WHERE cp.work_id=c.cover_work_id AND m.role='cover' ORDER BY m.slot LIMIT 1),
                    c.cover_file_id,
                    (SELECT count(*) FROM collection_members m WHERE m.collection_id=c.id),
                    (SELECT count(*) FROM collection_members m JOIN work_current_projection p ON p.work_id=m.work_id WHERE m.collection_id=c.id),
                    (SELECT coalesce(sum(p.text_length),0) FROM collection_members m JOIN work_current_projection p ON p.work_id=m.work_id WHERE m.collection_id=c.id)
             FROM collections c JOIN entities e ON e.id=c.id AND e.deleted_at IS NULL
             WHERE {condition}"
        ))?;
    let rows: Vec<Row> = statement
        .query_map(params_from_iter(values.iter()), |r| {
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
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(statement);
    let mut collections = Vec::with_capacity(rows.len());
    for row in rows {
        let (
            collection,
            name,
            description,
            appearance,
            ordered,
            revision,
            updated,
            created,
            cover,
            cover_image,
            members,
            available,
            length,
        ) = row;
        let appearance: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(&appearance).unwrap_or_default();
        let look = |key: &str, default: &str| {
            text(appearance.get(key)).unwrap_or_else(|| default.to_owned())
        };
        let tiles = conn
                .prepare_cached(
                    "SELECT x.provider,x.external_key,coalesce(p.title,m.title_at_add),coalesce(p.author_name,m.author_at_add),
                            (SELECT r.file_id FROM snapshot_media sm JOIN media_renditions r ON r.id=sm.rendition_id
                              WHERE sm.snapshot_id=p.snapshot_id AND sm.role='cover' ORDER BY sm.slot LIMIT 1)
                     FROM collection_members m
                     JOIN external_identities x ON x.entity_id=m.work_id AND x.kind='work'
                     LEFT JOIN work_current_projection p ON p.work_id=m.work_id
                     WHERE m.collection_id=?1
                     ORDER BY m.position,m.created_at,x.provider,x.external_key LIMIT ?2",
                )?
                .query_map(params![collection, COLLECTION_TILES], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<Vec<u8>>>(4)?,
                    ))
                })?
                .map(|row| {
                    let (provider, external_key, title, author_name, cover) = row?;
                    Ok(CollectionTile {
                        provider,
                        external_key,
                        title,
                        author_name,
                        cover: fingerprint(cover)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
        collections.push(CollectionSummary {
            id: id(collection)?,
            name,
            description,
            ordered,
            cover: fingerprint(cover)?,
            cover_image: fingerprint(cover_image)?,
            cover_mode: look("cover_mode", "mosaic"),
            name_source: look("name_source", "manual"),
            track: look("track", "manual"),
            tiles,
            revision,
            member_count: members as u64,
            available_count: available as u64,
            total_text_length: length as u64,
            created_at: created,
            updated_at: updated.unwrap_or(created),
        });
    }
    collections.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| nocase(&left.name, &right.name))
            .then_with(|| left.id.as_bytes().cmp(right.id.as_bytes()))
    });
    Ok(collections)
}

#[cfg(test)]
#[path = "library_entity_list_tests.rs"]
mod tests;
