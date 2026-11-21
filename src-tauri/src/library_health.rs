//! 棚の数と手入れ（ホーム・横の欄・設定・診断の画面）。
//!
//! 旧棚の同じ画面と同じ形で返す。数え方は棚の今の版（`work_current_projection`）に揃え、
//! 旧棚にしかない数（保存フォルダーの照合・未登録のファイル）は持たない。棚のファイルは
//! 中身の指紋で台帳と結ばれているので、照合は `verify` が、未参照の片付けは `reclaim` が担う。

use crate::database::models::{
    DashboardSummary, DashboardTrendPoint, DbStats, EntityProfileRepairStatus, FacetCount,
    IncompleteEntityProfile, LibraryMaintenanceResult, LibraryShelfCounts,
    SearchIndexOptimizationResult, SearchIndexStatus, SourceBreakdown,
};
use crate::library::{
    Error, Library, ReclaimOptions, ReclaimReport, Result, RevisionFilter, WorkFilter,
    WorkListQuery,
};
use serde::Serialize;
use std::path::Path;

/// 診断の画面の数。旧棚の診断のうち、棚にも意味のあるものだけを持つ。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfDiagnostics {
    pub measured_at: String,
    pub works: u64,
    /// 作品の保存した版の数（作者・シリーズの記録は含めない）。
    pub versions: u64,
    pub text_length: u64,
    pub media: u64,
    /// 台帳のファイルの数と、その元の大きさ・保存している大きさ（同じ中身は一度だけ）。
    pub files: u64,
    pub logical_bytes: u64,
    pub stored_bytes: u64,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub live_database_bytes: u64,
    pub fragmentation_percent: f64,
    pub lexical_index_bytes: u64,
    pub lexical_index_segments: u64,
    pub history_index_bytes: u64,
    pub semantic_index_bytes: u64,
    pub process_memory_bytes: Option<u64>,
    pub process_private_memory_bytes: Option<u64>,
    pub gpu_dedicated_memory_bytes: Option<u64>,
    pub gpu_shared_memory_bytes: Option<u64>,
    pub search_index: SearchIndexStatus,
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// 月の区切りは日本時間（旧棚の保存日時の文字列の先頭 7 文字と同じ月）。
const MONTH_SQL: &str = "strftime('%Y-%m',p.saved_at/1000,'unixepoch','+9 hours')";

impl Library {
    /// 絞り込みに当てはまる作品の数。
    pub fn count_works(&self, filter: WorkFilter) -> Result<u64> {
        let page = self.list_works(&WorkListQuery {
            limit: 1,
            filter,
            count: true,
            ..WorkListQuery::default()
        })?;
        Ok(page.total.unwrap_or(0))
    }

    /// 横の欄の件数（すべて・お気に入り・読みかけ・更新監視・改稿あり）。
    pub fn shelf_counts(&self) -> Result<LibraryShelfCounts> {
        let (total, favorite, watched): (i64, i64, i64) = self.reader()?.query_row(
            "SELECT count(*),coalesce(sum(favorite),0),coalesce(sum(watching),0) FROM work_current_projection",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let reading = self.count_works(WorkFilter {
            reading: Some(true),
            ..WorkFilter::default()
        })?;
        let revised = self.count_works(WorkFilter {
            revision: Some(RevisionFilter::Revised),
            ..WorkFilter::default()
        })?;
        Ok(LibraryShelfCounts {
            total,
            favorite,
            watched,
            reading: reading as i64,
            revised: revised as i64,
        })
    }

    /// 保存の数（ホーム・設定の統計）。
    pub fn shelf_stats(&self) -> Result<DbStats> {
        let conn = self.reader()?;
        let (total, pixiv, fanbox, media, size): (i64, i64, i64, i64, i64) = conn.query_row(
            "SELECT count(*),coalesce(sum(provider='pixiv'),0),coalesce(sum(provider='fanbox'),0),
                    coalesce(sum(media_count),0),coalesce(sum(logical_size),0)
             FROM work_current_projection p",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        Ok(DbStats {
            total_downloads: total,
            pixiv_count: pixiv,
            fanbox_count: fanbox,
            total_assets: media,
            total_size_bytes: size,
        })
    }

    /// ホームの集計。最近の保存は画面が一覧の命令で引く（作品の形が棚の一覧と同じなので）。
    pub fn dashboard(&self, index: &SearchIndexStatus) -> Result<DashboardSummary> {
        let stats = self.shelf_stats()?;
        let conn = self.reader()?;
        let (favorite, watched): (i64, i64) = conn.query_row(
            "SELECT coalesce(sum(favorite),0),coalesce(sum(watching),0) FROM work_current_projection",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let targets: i64 = conn.query_row(
            "SELECT count(*) FROM watch_targets WHERE enabled=1",
            [],
            |r| r.get(0),
        )?;
        let facets = |sql: &str, limit: i64| -> Result<Vec<FacetCount>> {
            let mut statement = conn.prepare(sql)?;
            let rows = statement.query_map([limit], |r| {
                Ok(FacetCount {
                    name: r.get(0)?,
                    count: r.get(1)?,
                    sources: Vec::new(),
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        // 旧棚と同じく名前でまとめる（取得元が違っても同じ名前のタグは一つ）。
        let top_tags = facets(
            "SELECT t.name,count(*) AS n FROM work_current_projection p
             JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
             GROUP BY t.name ORDER BY n DESC,t.name ASC LIMIT ?1",
            24,
        )?;
        let top_authors = facets(
            "SELECT author_name,count(*) AS n FROM work_current_projection
             WHERE author_name<>'' GROUP BY author_name ORDER BY n DESC,author_name ASC LIMIT ?1",
            12,
        )?;
        let source_breakdown = {
            let mut statement = conn.prepare(
                "SELECT provider,count(*) AS n,coalesce(sum(logical_size),0) FROM work_current_projection
                 GROUP BY provider ORDER BY n DESC,provider",
            )?;
            let rows = statement.query_map([], |r| {
                Ok(SourceBreakdown {
                    source: r.get(0)?,
                    count: r.get(1)?,
                    total_size_bytes: r.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut monthly_downloads = {
            let mut statement = conn.prepare(&format!(
                "SELECT {MONTH_SQL} AS bucket,count(*),coalesce(sum(p.provider='pixiv'),0),
                        coalesce(sum(p.provider='fanbox'),0),coalesce(sum(p.logical_size),0)
                 FROM work_current_projection p GROUP BY bucket ORDER BY bucket DESC LIMIT 12"
            ))?;
            let rows = statement.query_map([], |r| {
                Ok(DashboardTrendPoint {
                    bucket: r.get(0)?,
                    count: r.get(1)?,
                    pixiv_count: r.get(2)?,
                    fanbox_count: r.get(3)?,
                    total_size_bytes: r.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        monthly_downloads.reverse();
        Ok(DashboardSummary {
            stats,
            favorite_count: favorite,
            watched_count: watched,
            update_target_count: targets,
            indexed_count: index.indexed_downloads,
            pending_index_count: index.pending_downloads,
            top_tags,
            top_authors,
            recent_downloads: Vec::new(),
            source_breakdown,
            monthly_downloads,
        })
    }

    /// 検索の索引の状態。意味検索を使うかはアプリの設定なので、呼ぶ側が渡す。
    pub fn search_index_status(&self, semantic_enabled: bool) -> Result<SearchIndexStatus> {
        let total = count(self.reader()?.query_row(
            "SELECT count(*) FROM library_items",
            [],
            |r| r.get(0),
        )?);
        let pending = self.search_index_pending()? as u64;
        let semantic = if semantic_enabled {
            Some(self.semantic_index_state()?)
        } else {
            None
        };
        let model = crate::database::semantic_index::status(&self.semantic_storage());
        let indexed = total.saturating_sub(pending);
        let semantic_indexed = semantic.map_or(0, |s| s.indexed);
        Ok(SearchIndexStatus {
            total_downloads: total as i64,
            indexed_downloads: indexed as i64,
            pending_downloads: pending as i64,
            is_complete: pending == 0,
            phase: if pending == 0 { "ready" } else { "pending" }.into(),
            // 画面は「意味ベクトルが一つもない」を塊の数で見る。棚は作品ごとに数える。
            semantic_indexed_chunks: semantic_indexed as i64,
            semantic_indexed_downloads: semantic_indexed as i64,
            semantic_pending_downloads: semantic.map_or(0, |s| s.works.saturating_sub(s.indexed))
                as i64,
            semantic_enabled,
            semantic_model_ready: model.model_ready,
            embedding_provider: model.provider,
            gpu_enabled: model.gpu_enabled,
            throughput_per_sec: None,
        })
    }

    /// 診断の数。台帳と索引の大きさを数えるだけで、ファイルの中身は読まない（照合は `verify`）。
    pub fn diagnostics(&self, index: SearchIndexStatus) -> Result<ShelfDiagnostics> {
        let conn = self.reader()?;
        let (works, text_length, media): (i64, i64, i64) = conn.query_row(
            "SELECT count(*),coalesce(sum(text_length),0),coalesce(sum(media_count),0) FROM work_current_projection",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let versions: i64 = conn.query_row(
            "SELECT count(*) FROM revision_snapshots s JOIN works w ON w.id=s.entity_id WHERE s.published=1",
            [],
            |r| r.get(0),
        )?;
        let (files, logical): (i64, i64) = conn.query_row(
            "SELECT count(*),coalesce(sum(original_size),0) FROM content_files",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let stored: i64 = conn.query_row(
            "SELECT coalesce(sum(stored_size),0) FROM chunk_storage",
            [],
            |r| r.get(0),
        )?;
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        drop(conn);
        let database = self.root().join("library.db");
        let derived = self.search_storage();
        let semantic_bytes = crate::database::queries::recursive_file_size(&derived.join("search"));
        let history_bytes = crate::database::queries::recursive_file_size(&derived.join("history"));
        let lexical_bytes =
            crate::database::queries::recursive_file_size(&derived.join("search-index"));
        let segments =
            crate::database::tantivy_index::searchable_segment_count(&derived).unwrap_or(0);
        let process = crate::database::queries::process_resource_snapshot();
        Ok(ShelfDiagnostics {
            measured_at: chrono::Utc::now().to_rfc3339(),
            works: count(works),
            versions: count(versions),
            text_length: count(text_length),
            media: count(media),
            files: count(files),
            logical_bytes: count(logical),
            stored_bytes: count(stored),
            database_bytes: file_size(&database),
            wal_bytes: file_size(&self.root().join("library.db-wal")),
            live_database_bytes: count((pages - free).max(0) * page_size),
            fragmentation_percent: if pages > 0 {
                free.max(0) as f64 / pages as f64 * 100.0
            } else {
                0.0
            },
            lexical_index_bytes: lexical_bytes,
            lexical_index_segments: segments as u64,
            history_index_bytes: history_bytes,
            semantic_index_bytes: semantic_bytes,
            process_memory_bytes: process.working_set_bytes,
            process_private_memory_bytes: process.private_bytes,
            gpu_dedicated_memory_bytes: process.gpu_dedicated_bytes,
            gpu_shared_memory_bytes: process.gpu_shared_bytes,
            search_index: index,
        })
    }

    /// 台帳の統計を整え、`compact` なら空き領域を回収する。空きが足りなければ何もしない。
    pub fn maintain_database(&self, compact: bool) -> Result<LibraryMaintenanceResult> {
        let database = self.root().join("library.db");
        let before = file_size(&database);
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE); PRAGMA optimize;")?;
        if compact {
            let required = before.saturating_add(before / 4).saturating_add(256 << 20);
            let available = crate::database::queries::available_space_bytes(self.root())
                .ok_or_else(|| {
                    Error::Integrity("空き容量を確かめられないので圧縮しません".into())
                })?;
            if available < required {
                return Err(Error::Integrity(format!(
                    "空き容量が足りないので圧縮しません（必要 {required} バイト、空き {available} バイト）"
                )));
            }
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA optimize;")?;
        }
        drop(conn);
        let after = file_size(&database);
        Ok(LibraryMaintenanceResult {
            compacted: compact,
            before_bytes: before,
            after_bytes: after,
            reclaimed_bytes: before.saturating_sub(after),
        })
    }

    /// 全文検索の索引（いまの版と過去の版）の断片を一つにまとめる。
    pub fn optimize_search_index(&self) -> Result<SearchIndexOptimizationResult> {
        use crate::database::tantivy_index::{optimize_segments, searchable_segment_count};
        let started = std::time::Instant::now();
        let current = self.search_storage();
        let history = current.join("history");
        let size = || {
            crate::database::queries::recursive_file_size(&current.join("search-index"))
                + crate::database::queries::recursive_file_size(&history.join("search-index"))
        };
        let before_bytes = size();
        let required = before_bytes
            .saturating_add(before_bytes / 10)
            .saturating_add(512 << 20);
        let available = crate::database::queries::available_space_bytes(&current)
            .ok_or_else(|| Error::Integrity("空き容量を確かめられないので統合しません".into()))?;
        if available < required {
            return Err(Error::Integrity(format!(
                "空き容量が足りないので統合しません（必要 {required} バイト、空き {available} バイト）"
            )));
        }
        let mut before = 0;
        let mut after = 0;
        for storage in [&current, &history] {
            if searchable_segment_count(storage).is_err() {
                continue;
            }
            let (b, a) = optimize_segments(storage).map_err(Error::Integrity)?;
            before += b;
            after += a;
        }
        let after_bytes = size();
        Ok(SearchIndexOptimizationResult {
            optimized: after < before,
            before_segments: before as u64,
            after_segments: after as u64,
            before_bytes,
            after_bytes,
            reclaimed_bytes: before_bytes.saturating_sub(after_bytes),
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        })
    }

    /// 棚の作品の今の版に載る作者・シリーズのうち、プロフィールをまだ一度も取得していないもの
    /// （旧棚の「作者・シリーズ情報の修復」の対象と同じ。作者が先、名前の順）。
    pub fn incomplete_profiles(&self) -> Result<Vec<IncompleteEntityProfile>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare(
            "SELECT kind,provider,external_key,max(name) FROM (
               SELECT 'person' AS kind,x.provider,x.external_key,r.display_name AS name,0 AS ord
               FROM work_current_projection p JOIN snapshot_people r ON r.snapshot_id=p.snapshot_id
               JOIN external_identities x ON x.entity_id=r.person_id
               LEFT JOIN source_entity_state st ON st.entity_id=r.person_id
               WHERE r.role IN ('author','creator') AND x.kind='person' AND x.provider IN ('pixiv','fanbox')
                 AND st.last_fetched_at IS NULL
               UNION ALL
               SELECT 'series',x.provider,x.external_key,r.title,1
               FROM work_current_projection p JOIN snapshot_series r ON r.snapshot_id=p.snapshot_id
               JOIN external_identities x ON x.entity_id=r.series_id
               LEFT JOIN source_entity_state st ON st.entity_id=r.series_id
               WHERE x.kind='series' AND x.provider='pixiv' AND st.last_fetched_at IS NULL)
             GROUP BY kind,provider,external_key
             ORDER BY min(ord),max(name) COLLATE NOCASE,external_key",
        )?;
        let rows = statement.query_map([], |r| {
            Ok(IncompleteEntityProfile {
                entity_type: r.get(0)?,
                source: r.get(1)?,
                source_key: r.get(2)?,
                display_name: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `incomplete_profiles` の数（作者・シリーズ別）。
    pub fn profile_repair_status(&self) -> Result<EntityProfileRepairStatus> {
        let rows = self.incomplete_profiles()?;
        let people = rows.iter().filter(|r| r.entity_type == "person").count() as i64;
        let total = rows.len() as i64;
        Ok(EntityProfileRepairStatus {
            person_count: people,
            series_count: total - people,
            total_count: total,
        })
    }

    /// 診断の画面からの回収（既定の猶予）。
    pub fn reclaim_now(&self) -> Result<ReclaimReport> {
        self.reclaim(ReclaimOptions::default())
    }
}

#[cfg(test)]
#[path = "library_health_tests.rs"]
mod tests;
