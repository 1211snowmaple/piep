//! 一覧の絞り込みの選択肢（タグ・作者・種別・添付）と、その件数。
//!
//! 数え方は一覧の絞り込みに揃える。タグは今の版のタグと自分で付けたタグを名前でまとめ、
//! 作者は今の版の作者名でまとめる（一覧の作者の条件は作者名で当てる）。作者・シリーズの
//! 束ねは一覧の束ねのタブが別に引くので、ここでは持たない。

use crate::analysis::rank_facets;
use crate::library::{Error, Library, LibraryId, Result};
use crate::models::{FacetCount, FilterFacets};
use crate::search_engine::text::normalize_search_text;
use rusqlite::{params_from_iter, types::Value, OptionalExtension};

/// 引き出しに並べる数。多い順に 500 まで（それより先は名前で探す）。
const DRAWER_LIMIT: i64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetKind {
    Tags,
    Authors,
}

impl FacetKind {
    pub fn parse(kind: &str) -> Result<Self> {
        match kind {
            "tags" | "tag" => Ok(Self::Tags),
            "authors" | "author" => Ok(Self::Authors),
            other => Err(Error::Integrity(format!(
                "絞り込みの種類が分かりません: {other}"
            ))),
        }
    }

    /// `pattern` があれば名前に含むものだけ。数は今の版の作品の数。
    fn sql(self, filtered: bool) -> &'static str {
        match (self, filtered) {
            (Self::Tags, false) => {
                "SELECT name,count(*) AS n,NULL FROM (
                   SELECT p.work_id,t.name FROM work_current_projection p
                   JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
                   UNION
                   SELECT wt.work_id,u.name FROM work_user_tags wt
                   JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
                   JOIN work_current_projection p ON p.work_id=wt.work_id
                   WHERE wt.user_id=?1)
                 GROUP BY name ORDER BY n DESC,name ASC LIMIT ?2"
            }
            (Self::Tags, true) => {
                "SELECT name,count(*) AS n,NULL FROM (
                   SELECT p.work_id,t.name FROM work_current_projection p
                   JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
                   WHERE t.name LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                   UNION
                   SELECT wt.work_id,u.name FROM work_user_tags wt
                   JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
                   JOIN work_current_projection p ON p.work_id=wt.work_id
                   WHERE wt.user_id=?1 AND u.name LIKE ?3 ESCAPE '\\' COLLATE NOCASE)
                 GROUP BY name ORDER BY n DESC,name ASC LIMIT ?2"
            }
            (Self::Authors, false) => {
                "SELECT author_name,count(*) AS n,group_concat(DISTINCT provider) FROM work_current_projection
                 WHERE author_name<>'' AND ?1 IS NOT NULL
                 GROUP BY author_name ORDER BY n DESC,author_name ASC LIMIT ?2"
            }
            (Self::Authors, true) => {
                "SELECT author_name,count(*) AS n,group_concat(DISTINCT provider) FROM work_current_projection
                 WHERE author_name<>'' AND ?1 IS NOT NULL AND author_name LIKE ?3 ESCAPE '\\' COLLATE NOCASE
                 GROUP BY author_name ORDER BY n DESC,author_name ASC LIMIT ?2"
            }
        }
    }
}

impl Library {
    fn facet_rows(&self, sql: &str, values: Vec<Value>) -> Result<Vec<FacetCount>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(sql)?;
        let rows = statement.query_map(params_from_iter(values), |r| {
            Ok(FacetCount {
                name: r.get(0)?,
                count: r.get(1)?,
                sources: r
                    .get::<_, Option<String>>(2)?
                    .unwrap_or_default()
                    .split(',')
                    .filter(|source| !source.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn facets_of(
        &self,
        kind: FacetKind,
        pattern: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FacetCount>> {
        let mut values = vec![
            Value::Blob(self.id.as_bytes().to_vec()),
            Value::Integer(limit),
        ];
        if let Some(pattern) = pattern {
            values.push(Value::Text(pattern.to_string()));
        }
        self.facet_rows(kind.sql(pattern.is_some()), values)
    }

    /// 絞り込みの引き出しに並べる選択肢。
    pub fn filter_facets(&self) -> Result<FilterFacets> {
        let content_types = self.facet_rows(
            "SELECT content_type,count(*) AS n,NULL FROM work_current_projection
             WHERE content_type<>'' GROUP BY content_type ORDER BY n DESC,content_type ASC",
            Vec::new(),
        )?;
        let asset_types = self.facet_rows(
            "SELECT kind,n,NULL FROM (
               SELECT 'image' AS kind,count(*) AS n FROM work_current_projection WHERE image_count>0
               UNION ALL
               SELECT 'file',count(*) FROM work_current_projection WHERE file_count>0)
             WHERE n>0 ORDER BY n DESC,kind ASC",
            Vec::new(),
        )?;
        Ok(FilterFacets {
            tags: self.facets_of(FacetKind::Tags, None, DRAWER_LIMIT)?,
            authors: self.facets_of(FacetKind::Authors, None, DRAWER_LIMIT)?,
            author_entities: Vec::new(),
            series: Vec::new(),
            content_types,
            asset_types,
        })
    }

    /// 選択肢を名前で探す。含むものを先に集め、足りなければ多いものから読みの近いものを拾う。
    pub fn search_filter_facets(
        &self,
        kind: FacetKind,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FacetCount>> {
        let limit = limit.clamp(1, 200);
        let normalized = query.map(normalize_search_text).unwrap_or_default();
        if normalized.is_empty() {
            return self.facets_of(kind, None, limit as i64);
        }
        let candidates = limit.saturating_mul(20).clamp(400, 4_000) as i64;
        let raw = query.unwrap_or("").trim();
        let escaped = raw
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let mut facets = self.facets_of(kind, Some(&format!("%{escaped}%")), candidates)?;
        if facets.len() < limit {
            let mut names: std::collections::HashSet<String> =
                facets.iter().map(|facet| facet.name.clone()).collect();
            for facet in self.facets_of(kind, None, candidates)? {
                if names.insert(facet.name.clone()) {
                    facets.push(facet);
                }
            }
        }
        Ok(rank_facets(facets, &normalized, limit))
    }
}

impl Library {
    /// 取得元と作品の番号から、棚に載っている作品を引く（本文のリンクの宛先）。棚から外した
    /// 作品は載っていないので引かない。
    pub fn find_listed_work(
        &self,
        provider: &str,
        key: &str,
    ) -> Result<Option<(LibraryId, String)>> {
        let conn = self.reader()?;
        let row: Option<(Vec<u8>, String)> = conn
            .prepare_cached(
                "SELECT work_id,title FROM work_current_projection WHERE provider=?1 AND external_key=?2",
            )?
            .query_row([provider, key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        row.map(|(id, title)| {
            LibraryId::from_slice(&id)
                .map(|id| (id, title))
                .map_err(|_| Error::Integrity("作品の識別子が不正です".into()))
        })
        .transpose()
    }
}

#[cfg(test)]
#[path = "library_facets_tests.rs"]
mod tests;
