//! データベースCRUD操作。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use regex::Regex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::models::*;
use super::schema;
use super::search::{
    extract_search_body, generate_ngrams_limited, make_match_highlights, match_fields_and_score,
    normalize_search_text, normalized_levenshtein, parse_search_query, query_ngrams,
    ParsedSearchQuery, SearchDocument,
};

#[derive(Debug, Clone)]
struct RankedSearchHit {
    download_id: i64,
    score: f64,
    semantic: Option<super::semantic_index::SemanticSearchHit>,
}

#[derive(Debug, Clone)]
struct SearchIndexBuildDocument {
    download_id: i64,
    current_version: i64,
    content_hash: Option<String>,
    tantivy: super::tantivy_index::TantivyIndexDocument,
    semantic: super::semantic_index::SemanticIndexDocument,
}

/// Indicates whether an entity row came from a real profile fetch or a lightweight work snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityProfileFreshness {
    SnapshotOnly,
    RemoteChecked,
}

impl EntityProfileFreshness {
    fn checked_at(self, now: &str) -> Option<&str> {
        match self {
            EntityProfileFreshness::SnapshotOnly => None,
            EntityProfileFreshness::RemoteChecked => Some(now),
        }
    }
}

fn build_read_pool(db_path: &Path) -> Result<Pool<SqliteConnectionManager>, String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let manager = SqliteConnectionManager::file(db_path)
        .with_flags(flags)
        .with_init(|conn| {
            conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
            conn.execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;
                PRAGMA temp_store = MEMORY;
                PRAGMA mmap_size = 268435456;
                PRAGMA cache_size = -64000;
                ",
            )
        });
    let parallelism = std::thread::available_parallelism()
        .map(|value| value.get() as u32)
        .unwrap_or(4)
        .clamp(4, 16);
    Pool::builder()
        .max_size(parallelism)
        .min_idle(Some(1))
        .build(manager)
        .map_err(|e| format!("DB read pool creation failed: {}", e))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchCursor {
    kind: String,
    sort_by: Option<String>,
    sort_order: Option<String>,
    value: Option<String>,
    id: Option<i64>,
    score: Option<f64>,
    downloaded_at: Option<String>,
}

/// スレッドセーフなデータベースハンドル
pub struct Database {
    conn: Mutex<Connection>,
    read_pool: Pool<SqliteConnectionManager>,
    storage_dir: PathBuf,
}

impl Database {
    /// データベースを開く（存在しなければ作成）
    pub fn open(db_path: &Path, storage_dir: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| format!("DB open failed: {}", e))?;
        schema::initialize(&conn).map_err(|e| format!("DB init failed: {}", e))?;
        let read_pool = build_read_pool(db_path)?;

        // ストレージディレクトリを作成
        std::fs::create_dir_all(storage_dir)
            .map_err(|e| format!("Storage dir creation failed: {}", e))?;

        Ok(Self {
            conn: Mutex::new(conn),
            read_pool,
            storage_dir: storage_dir.to_path_buf(),
        })
    }

    /// ストレージディレクトリのパスを取得
    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    fn read_conn(&self) -> Result<PooledConnection<SqliteConnectionManager>, String> {
        self.read_pool
            .get()
            .map_err(|e| format!("DB read pool checkout failed: {}", e))
    }

    pub fn reindex_download(&self, download_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        reindex_download_locked(&conn, &self.storage_dir, download_id)
    }

    pub fn get_search_index_status(&self) -> Result<SearchIndexStatus, String> {
        let conn = self.read_conn()?;
        search_index_status_locked(&conn, &self.storage_dir)
    }

    pub fn rebuild_search_index_batch(&self, limit: i64) -> Result<SearchIndexStatus, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let limit = limit.clamp(1, 200);
        let ids = stale_search_index_ids_locked(&conn, limit)?;
        let mut docs = Vec::with_capacity(ids.len());
        for id in &ids {
            match search_index_document_locked(&conn, &self.storage_dir, *id) {
                Ok(Some(doc)) => docs.push(doc),
                Ok(None) => {
                    if let Err(e) = clear_search_index_locked(&conn, &self.storage_dir, *id) {
                        log::warn!("Failed to clear missing search index row {}: {}", id, e);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to prepare search index document {}: {}", id, e);
                }
            }
        }
        if let Err(e) = index_search_documents_locked(&conn, &self.storage_dir, &docs, false) {
            log::warn!("Failed to rebuild search index batch: {}", e);
        }
        search_index_status_locked(&conn, &self.storage_dir)
    }

    pub fn search_filter_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FacetCount>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 200) as usize;
        let normalized_query = query.map(normalize_search_text).unwrap_or_default();

        let sql = match kind {
            "tags" | "tag" => {
                "SELECT t.name, COUNT(dt.download_id) AS count
                 FROM tags t
                 JOIN download_tags dt ON dt.tag_id = t.id
                 GROUP BY t.id, t.name"
            }
            "authors" | "author" => {
                "SELECT author_name, COUNT(*) AS count
                 FROM downloads
                 WHERE author_name IS NOT NULL AND author_name != ''
                 GROUP BY author_name"
            }
            _ => return Err(format!("Unsupported facet kind: {}", kind)),
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Facet search prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FacetCount {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            })
            .map_err(|e| format!("Facet search failed: {}", e))?;

        let mut facets = Vec::new();
        for row in rows {
            facets.push(row.map_err(|e| format!("Facet search row read failed: {}", e))?);
        }

        if normalized_query.is_empty() {
            facets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
            facets.truncate(limit);
            return Ok(facets);
        }

        let query_grams = query_ngrams(&normalized_query);
        let mut scored = facets
            .into_iter()
            .filter_map(|facet| {
                let normalized_name = normalize_search_text(&facet.name);
                let mut score = if normalized_name == normalized_query {
                    1000.0
                } else if normalized_name.starts_with(&normalized_query) {
                    800.0
                } else if normalized_name.contains(&normalized_query) {
                    650.0
                } else {
                    let name_grams = generate_ngrams_limited(&normalized_name, 512);
                    let overlap = query_grams
                        .iter()
                        .filter(|gram| name_grams.contains(*gram))
                        .count();
                    let ratio = if query_grams.is_empty() {
                        0.0
                    } else {
                        overlap as f64 / query_grams.len() as f64
                    };
                    let fuzzy = normalized_levenshtein(&normalized_name, &normalized_query);
                    (ratio * 430.0).max(if fuzzy >= 0.72 { fuzzy * 360.0 } else { 0.0 })
                };

                if score <= 0.0 {
                    return None;
                }
                score += (facet.count as f64 + 1.0).ln() * 12.0;
                Some((score, facet))
            })
            .collect::<Vec<(f64, FacetCount)>>();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.1.count.cmp(&a.1.count))
                .then_with(|| a.1.name.cmp(&b.1.name))
        });
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, facet)| facet).collect())
    }

    pub fn search_suggest(
        &self,
        params: &SearchSuggestParams,
    ) -> Result<SearchSuggestResult, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let limit = params.limit.unwrap_or(12).clamp(1, 50);
        let text = params.text.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return Ok(SearchSuggestResult { items: Vec::new() });
        }
        let like = format!("%{}%", text.replace('%', "\\%").replace('_', "\\_"));
        let mut items = Vec::new();

        collect_suggestions(
            &conn,
            "tag",
            "SELECT t.name, t.name, COUNT(dt.download_id)
             FROM tags t
             JOIN download_tags dt ON dt.tag_id = t.id
             WHERE t.name LIKE ?1 ESCAPE '\\'
             GROUP BY t.id, t.name
             ORDER BY COUNT(dt.download_id) DESC, t.name ASC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;
        collect_suggestions(
            &conn,
            "author",
            "SELECT d.author_name, d.author_name, COUNT(*)
             FROM downloads d
             WHERE d.author_name LIKE ?1 ESCAPE '\\'
             GROUP BY d.author_name
             ORDER BY COUNT(*) DESC, d.author_name ASC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;
        collect_suggestions(
            &conn,
            "series",
            "SELECT s.title, s.source || ':' || s.source_key, COUNT(ds.download_id)
             FROM series s
             LEFT JOIN download_series ds ON ds.series_source = s.source AND ds.series_key = s.source_key
             WHERE s.title LIKE ?1 ESCAPE '\\'
             GROUP BY s.source, s.source_key, s.title
             ORDER BY COUNT(ds.download_id) DESC, s.title ASC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;
        collect_suggestions(
            &conn,
            "title",
            "SELECT d.title, d.source_id, 1
             FROM downloads d
             WHERE d.title LIKE ?1 ESCAPE '\\'
             ORDER BY d.downloaded_at DESC
             LIMIT ?2",
            &like,
            limit,
            &mut items,
        )?;

        items.truncate(limit as usize * 4);
        Ok(SearchSuggestResult { items })
    }

    /// ダウンロードを挿入（UPSERT: 既に存在する場合は更新）
    pub fn upsert_download(&self, dl: &NewDownload) -> Result<i64, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;

        // 正規化インサートをアトミックに行うためのトランザクション開始
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction begin failed: {}", e))?;

        tx.execute(
            "INSERT INTO downloads (
                source, source_id, title, author_name, author_id,
                content_type, excerpt, cover_path, json_path,
                original_json_path, asset_count, file_size_bytes,
                downloaded_at, source_created_at,
                content_hash, text_length, source_updated_at, watch_updates, current_version, favorite
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            ON CONFLICT(source, source_id) DO UPDATE SET
                title = excluded.title,
                author_name = excluded.author_name,
                excerpt = excluded.excerpt,
                cover_path = excluded.cover_path,
                json_path = excluded.json_path,
                original_json_path = excluded.original_json_path,
                asset_count = excluded.asset_count,
                file_size_bytes = excluded.file_size_bytes,
                downloaded_at = excluded.downloaded_at,
                content_hash = excluded.content_hash,
                text_length = excluded.text_length,
                source_updated_at = excluded.source_updated_at,
                watch_updates = excluded.watch_updates,
                current_version = excluded.current_version",
            params![
                dl.source,
                dl.source_id,
                dl.title,
                dl.author_name,
                dl.author_id,
                dl.content_type,
                dl.excerpt,
                dl.cover_path,
                dl.json_path,
                dl.original_json_path,
                dl.asset_count,
                dl.file_size_bytes,
                dl.downloaded_at,
                dl.source_created_at,
                dl.content_hash,
                dl.text_length,
                dl.source_updated_at,
                if dl.watch_updates { 1i64 } else { 0i64 },
                dl.current_version,
                if dl.favorite { 1i64 } else { 0i64 },
            ],
        )
        .map_err(|e| format!("Insert download failed: {}", e))?;

        // 外部キー参照整合性を担保するため、主キー id を確実にクエリ
        let id: i64 = tx
            .query_row(
                "SELECT id FROM downloads WHERE source = ?1 AND source_id = ?2",
                params![dl.source, dl.source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query upserted download ID: {}", e))?;

        // 更新時の既存タグ関係を一旦クローン削除
        tx.execute(
            "DELETE FROM download_tags WHERE download_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to clear old tags: {}", e))?;

        // タグは download_tags を唯一の真実として同期する
        for tag_name in normalized_tag_list(&dl.tags) {
            tx.execute(
                "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                params![tag_name],
            )
            .map_err(|e| format!("Failed to insert tag: {}", e))?;

            let tag_id: i64 = tx
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    params![tag_name],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to retrieve tag ID: {}", e))?;

            tx.execute(
                "INSERT OR IGNORE INTO download_tags (download_id, tag_id) VALUES (?1, ?2)",
                params![id, tag_id],
            )
            .map_err(|e| format!("Failed to insert download tag relation: {}", e))?;
        }

        tx.commit()
            .map_err(|e| format!("Transaction commit failed: {}", e))?;
        Ok(id)
    }

    pub fn upsert_update_target(&self, target: &UpdateTargetInput) -> Result<UpdateTarget, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_targets (
                target_type, source, source_key, display_name, enabled, metadata_json, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                display_name = excluded.display_name,
                enabled = excluded.enabled,
                metadata_json = excluded.metadata_json,
                updated_at = CURRENT_TIMESTAMP",
            params![
                target.target_type,
                target.source,
                target.source_key,
                target.display_name,
                if target.enabled { 1i64 } else { 0i64 },
                target.metadata_json,
            ],
        )
        .map_err(|e| format!("Failed to upsert update target: {}", e))?;
        drop(conn);
        self.get_update_target(&target.target_type, &target.source, &target.source_key)
    }

    pub fn get_update_target(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<UpdateTarget, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM update_targets WHERE target_type = ?1 AND source = ?2 AND source_key = ?3",
            params![target_type, source, source_key],
            update_target_from_row,
        )
        .map_err(|e| format!("Update target not found: {}", e))
    }

    pub fn list_update_targets(
        &self,
        target_type: Option<&str>,
        enabled_only: bool,
    ) -> Result<Vec<UpdateTarget>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from("SELECT * FROM update_targets");
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut wheres = Vec::new();
        if let Some(t) = target_type {
            if !t.is_empty() && t != "all" {
                wheres.push("target_type = ?".to_string());
                bind_values.push(Box::new(t.to_string()));
            }
        }
        if enabled_only {
            wheres.push("enabled = 1".to_string());
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" ORDER BY target_type ASC, source ASC, display_name COLLATE NOCASE ASC");
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(refs.as_slice(), update_target_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn set_update_target_enabled(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_targets SET enabled = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?2 AND source = ?3 AND source_key = ?4",
            params![
                if enabled { 1i64 } else { 0i64 },
                target_type,
                source,
                source_key
            ],
        )
        .map_err(|e| format!("Failed to update target enabled state: {}", e))?;
        Ok(())
    }

    pub fn delete_update_target(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_targets WHERE target_type = ?1 AND source = ?2 AND source_key = ?3",
            params![target_type, source, source_key],
        )
        .map_err(|e| format!("Failed to delete update target: {}", e))?;
        Ok(())
    }

    pub fn mark_update_target_checked(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        last_seen_source_id: Option<&str>,
        last_seen_source_updated_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_targets SET
                last_checked_at = ?1,
                last_seen_source_id = COALESCE(?2, last_seen_source_id),
                last_seen_source_updated_at = COALESCE(?3, last_seen_source_updated_at),
                updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?4 AND source = ?5 AND source_key = ?6",
            params![
                chrono::Utc::now().to_rfc3339(),
                last_seen_source_id,
                last_seen_source_updated_at,
                target_type,
                source,
                source_key,
            ],
        )
        .map_err(|e| format!("Failed to mark update target checked: {}", e))?;
        Ok(())
    }

    pub fn upsert_download_relation(
        &self,
        download_id: i64,
        relation_type: &str,
        source: &str,
        relation_id: &str,
        relation_name: &str,
    ) -> Result<(), String> {
        if relation_id.trim().is_empty() || relation_name.trim().is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO download_relations (
                download_id, relation_type, source, relation_id, relation_name
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(download_id, relation_type, source, relation_id) DO UPDATE SET
                relation_name = excluded.relation_name",
            params![
                download_id,
                relation_type,
                source,
                relation_id,
                relation_name
            ],
        )
        .map_err(|e| format!("Failed to upsert download relation: {}", e))?;
        Ok(())
    }

    pub fn upsert_download_person(
        &self,
        download_id: i64,
        source: &str,
        person_key: &str,
        role: &str,
        display_name: &str,
    ) -> Result<(), String> {
        if person_key.trim().is_empty() || display_name.trim().is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO people (source, source_key, display_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![source, person_key, display_name],
        )
        .map_err(|e| format!("Failed to upsert person shell: {}", e))?;
        conn.execute(
            "INSERT INTO download_people (download_id, person_source, person_key, role, display_name)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(download_id, person_source, person_key, role) DO UPDATE SET
                display_name = excluded.display_name",
            params![download_id, source, person_key, role, display_name],
        )
        .map_err(|e| format!("Failed to upsert download person: {}", e))?;
        Ok(())
    }

    pub fn upsert_download_series(
        &self,
        download_id: i64,
        source: &str,
        series_key: &str,
        title: &str,
        content_order: Option<i64>,
    ) -> Result<(), String> {
        if series_key.trim().is_empty() || title.trim().is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO series (source, source_key, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            params![source, series_key, title],
        )
        .map_err(|e| format!("Failed to upsert series shell: {}", e))?;
        conn.execute(
            "INSERT INTO download_series (download_id, series_source, series_key, title, content_order)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(download_id, series_source, series_key) DO UPDATE SET
                title = excluded.title,
                content_order = excluded.content_order",
            params![download_id, source, series_key, title, content_order],
        )
        .map_err(|e| format!("Failed to upsert download series: {}", e))?;
        Ok(())
    }

    pub fn get_download_relations_for_download(
        &self,
        download_id: i64,
    ) -> Result<Vec<DownloadRelation>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT download_id, relation_type, source, relation_id, relation_name, NULL
                 FROM download_relations
                 WHERE download_id = ?1
                 ORDER BY relation_type, source, relation_id",
            )
            .map_err(|e| format!("Download relation query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadRelation {
                    download_id: row.get(0)?,
                    relation_type: row.get(1)?,
                    source: row.get(2)?,
                    relation_id: row.get(3)?,
                    relation_name: row.get(4)?,
                    work_count: row.get(5)?,
                })
            })
            .map_err(|e| format!("Download relation query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Download relation row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_download_people(&self, download_id: i64) -> Result<Vec<DownloadPerson>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT download_id, person_source, person_key, role, display_name
                 FROM download_people
                 WHERE download_id = ?1
                 ORDER BY person_source, person_key, role",
            )
            .map_err(|e| format!("Download people query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadPerson {
                    download_id: row.get(0)?,
                    person_source: row.get(1)?,
                    person_key: row.get(2)?,
                    role: row.get(3)?,
                    display_name: row.get(4)?,
                })
            })
            .map_err(|e| format!("Download people query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Download people row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_download_series_list(
        &self,
        download_id: i64,
    ) -> Result<Vec<DownloadSeries>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT download_id, series_source, series_key, title, content_order
                 FROM download_series
                 WHERE download_id = ?1
                 ORDER BY series_source, series_key",
            )
            .map_err(|e| format!("Download series query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadSeries {
                    download_id: row.get(0)?,
                    series_source: row.get(1)?,
                    series_key: row.get(2)?,
                    title: row.get(3)?,
                    content_order: row.get(4)?,
                })
            })
            .map_err(|e| format!("Download series query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Download series row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_person(&self, source: &str, source_key: &str) -> Result<PersonEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT p.*,
                (SELECT COUNT(DISTINCT download_id) FROM download_people dp
                 WHERE dp.person_source = p.source AND dp.person_key = p.source_key) AS work_count
             FROM people p WHERE p.source = ?1 AND p.source_key = ?2",
            params![source, source_key],
            person_entry_from_row,
        )
        .map_err(|e| format!("Person not found: {}", e))
    }

    pub fn list_people(&self) -> Result<Vec<PersonEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT p.*,
                    (SELECT COUNT(DISTINCT download_id) FROM download_people dp
                     WHERE dp.person_source = p.source AND dp.person_key = p.source_key) AS work_count
                 FROM people p
                 ORDER BY p.source ASC, p.source_key ASC",
            )
            .map_err(|e| format!("People query prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], person_entry_from_row)
            .map_err(|e| format!("People query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Person row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn restore_update_target(&self, target: &UpdateTarget) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_targets (
                target_type, source, source_key, display_name, enabled,
                last_checked_at, last_seen_source_id, last_seen_source_updated_at,
                metadata_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                display_name = excluded.display_name,
                enabled = excluded.enabled,
                last_checked_at = excluded.last_checked_at,
                last_seen_source_id = excluded.last_seen_source_id,
                last_seen_source_updated_at = excluded.last_seen_source_updated_at,
                metadata_json = excluded.metadata_json,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                target.target_type,
                target.source,
                target.source_key,
                target.display_name,
                if target.enabled { 1i64 } else { 0i64 },
                target.last_checked_at,
                target.last_seen_source_id,
                target.last_seen_source_updated_at,
                target.metadata_json,
                target.created_at,
                target.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore update target: {}", e))?;
        Ok(())
    }

    pub fn get_series(&self, source: &str, source_key: &str) -> Result<SeriesEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT s.*,
                (SELECT COUNT(DISTINCT download_id) FROM download_series ds
                 WHERE ds.series_source = s.source AND ds.series_key = s.source_key) AS work_count
             FROM series s WHERE s.source = ?1 AND s.source_key = ?2",
            params![source, source_key],
            series_entry_from_row,
        )
        .map_err(|e| format!("Series not found: {}", e))
    }

    pub fn list_series(&self) -> Result<Vec<SeriesEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT s.*,
                    (SELECT COUNT(DISTINCT download_id) FROM download_series ds
                     WHERE ds.series_source = s.source AND ds.series_key = s.source_key) AS work_count
                 FROM series s
                 ORDER BY s.source ASC, s.source_key ASC",
            )
            .map_err(|e| format!("Series query prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], series_entry_from_row)
            .map_err(|e| format!("Series query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Series row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn list_entity_versions(
        &self,
        entity_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<Vec<EntityVersion>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM entity_versions
                 WHERE entity_type = ?1 AND source = ?2 AND source_key = ?3
                 ORDER BY version DESC",
            )
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(
                params![entity_type, source, source_key],
                entity_version_from_row,
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn restore_person(&self, person: &PersonEntry) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO people (
                source, source_key, display_name, icon_path, cover_path, description, links_json,
                content_hash, current_version, last_checked_at, last_fetched_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(source, source_key) DO UPDATE SET
                display_name = excluded.display_name,
                icon_path = excluded.icon_path,
                cover_path = excluded.cover_path,
                description = excluded.description,
                links_json = excluded.links_json,
                content_hash = excluded.content_hash,
                current_version = excluded.current_version,
                last_checked_at = excluded.last_checked_at,
                last_fetched_at = excluded.last_fetched_at,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                person.source,
                person.source_key,
                person.display_name,
                person.icon_path,
                person.cover_path,
                person.description,
                person.links_json,
                person.content_hash,
                person.current_version,
                person.last_checked_at,
                person.last_fetched_at,
                person.created_at,
                person.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore person: {}", e))?;
        Ok(())
    }

    pub fn restore_series(&self, series: &SeriesEntry) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO series (
                source, source_key, title, description, cover_path, content_hash,
                current_version, last_checked_at, last_fetched_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(source, source_key) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                cover_path = excluded.cover_path,
                content_hash = excluded.content_hash,
                current_version = excluded.current_version,
                last_checked_at = excluded.last_checked_at,
                last_fetched_at = excluded.last_fetched_at,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at",
            params![
                series.source,
                series.source_key,
                series.title,
                series.description,
                series.cover_path,
                series.content_hash,
                series.current_version,
                series.last_checked_at,
                series.last_fetched_at,
                series.created_at,
                series.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore series: {}", e))?;
        Ok(())
    }

    pub fn restore_entity_version(&self, version: &EntityVersion) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO entity_versions (
                entity_type, source, source_key, version, content_hash, json_path,
                asset_count, file_size_bytes, created_at, change_summary
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(entity_type, source, source_key, version) DO UPDATE SET
                content_hash = excluded.content_hash,
                json_path = excluded.json_path,
                asset_count = excluded.asset_count,
                file_size_bytes = excluded.file_size_bytes,
                created_at = excluded.created_at,
                change_summary = excluded.change_summary",
            params![
                version.entity_type,
                version.source,
                version.source_key,
                version.version,
                version.content_hash,
                version.json_path,
                version.asset_count,
                version.file_size_bytes,
                version.created_at,
                version.change_summary,
            ],
        )
        .map_err(|e| format!("Failed to restore entity version: {}", e))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_person_profile(
        &self,
        source: &str,
        source_key: &str,
        display_name: &str,
        icon_path: Option<&str>,
        cover_path: Option<&str>,
        description: Option<&str>,
        links_json: Option<&str>,
        content_hash: &str,
        json_path: &str,
        asset_count: i64,
        file_size_bytes: i64,
        freshness: EntityProfileFreshness,
    ) -> Result<PersonEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (current_hash, current_version): (Option<String>, i64) = conn
            .query_row(
                "SELECT content_hash, current_version FROM people WHERE source = ?1 AND source_key = ?2",
                params![source, source_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, 0));
        let now = chrono::Utc::now().to_rfc3339();
        let checked_at = freshness.checked_at(&now);
        if current_hash.as_deref() == Some(content_hash) {
            conn.execute(
                "UPDATE people SET display_name = ?1, icon_path = ?2, cover_path = ?3,
                    description = ?4, links_json = ?5,
                    last_checked_at = COALESCE(?6, last_checked_at),
                    updated_at = CURRENT_TIMESTAMP
                 WHERE source = ?7 AND source_key = ?8",
                params![
                    display_name,
                    icon_path,
                    cover_path,
                    description,
                    links_json,
                    checked_at,
                    source,
                    source_key
                ],
            )
            .map_err(|e| format!("Failed to mark person checked: {}", e))?;
        } else {
            let next_version = current_version + 1;
            conn.execute(
                "INSERT INTO people (
                    source, source_key, display_name, icon_path, cover_path, description, links_json,
                    content_hash, current_version, last_checked_at, last_fetched_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                 ON CONFLICT(source, source_key) DO UPDATE SET
                    display_name = excluded.display_name,
                    icon_path = excluded.icon_path,
                    cover_path = excluded.cover_path,
                    description = excluded.description,
                    links_json = excluded.links_json,
                    content_hash = excluded.content_hash,
                    current_version = excluded.current_version,
                    last_checked_at = COALESCE(excluded.last_checked_at, people.last_checked_at),
                    last_fetched_at = COALESCE(excluded.last_fetched_at, people.last_fetched_at),
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    source, source_key, display_name, icon_path, cover_path, description,
                    links_json, content_hash, next_version, checked_at
                ],
            )
            .map_err(|e| format!("Failed to upsert person: {}", e))?;
            conn.execute(
                "INSERT OR IGNORE INTO entity_versions (
                    entity_type, source, source_key, version, content_hash, json_path,
                    asset_count, file_size_bytes, created_at, change_summary
                 ) VALUES ('person', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    source,
                    source_key,
                    next_version,
                    content_hash,
                    json_path,
                    asset_count,
                    file_size_bytes,
                    now,
                    if current_version == 0 {
                        "初回プロフィール保存"
                    } else {
                        "プロフィール更新"
                    },
                ],
            )
            .map_err(|e| format!("Failed to insert person version: {}", e))?;
        }
        drop(conn);
        self.get_person(source, source_key)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_series_profile(
        &self,
        source: &str,
        source_key: &str,
        title: &str,
        description: Option<&str>,
        cover_path: Option<&str>,
        content_hash: &str,
        json_path: &str,
        asset_count: i64,
        file_size_bytes: i64,
        freshness: EntityProfileFreshness,
    ) -> Result<SeriesEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (current_hash, current_version): (Option<String>, i64) = conn
            .query_row(
                "SELECT content_hash, current_version FROM series WHERE source = ?1 AND source_key = ?2",
                params![source, source_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, 0));
        let now = chrono::Utc::now().to_rfc3339();
        let checked_at = freshness.checked_at(&now);
        if current_hash.as_deref() == Some(content_hash) {
            conn.execute(
                "UPDATE series SET title = ?1, description = ?2, cover_path = COALESCE(?3, cover_path),
                    last_checked_at = COALESCE(?4, last_checked_at),
                    updated_at = CURRENT_TIMESTAMP
                 WHERE source = ?5 AND source_key = ?6",
                params![
                    title,
                    description,
                    cover_path,
                    checked_at,
                    source,
                    source_key
                ],
            )
            .map_err(|e| format!("Failed to mark series checked: {}", e))?;
        } else {
            let next_version = current_version + 1;
            conn.execute(
                "INSERT INTO series (
                    source, source_key, title, description, cover_path, content_hash,
                    current_version, last_checked_at, last_fetched_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                 ON CONFLICT(source, source_key) DO UPDATE SET
                    title = excluded.title,
                    description = excluded.description,
                    cover_path = COALESCE(excluded.cover_path, series.cover_path),
                    content_hash = excluded.content_hash,
                    current_version = excluded.current_version,
                    last_checked_at = COALESCE(excluded.last_checked_at, series.last_checked_at),
                    last_fetched_at = COALESCE(excluded.last_fetched_at, series.last_fetched_at),
                    updated_at = CURRENT_TIMESTAMP",
                params![
                    source,
                    source_key,
                    title,
                    description,
                    cover_path,
                    content_hash,
                    next_version,
                    checked_at
                ],
            )
            .map_err(|e| format!("Failed to upsert series: {}", e))?;
            conn.execute(
                "INSERT OR IGNORE INTO entity_versions (
                    entity_type, source, source_key, version, content_hash, json_path,
                    asset_count, file_size_bytes, created_at, change_summary
                 ) VALUES ('series', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    source,
                    source_key,
                    next_version,
                    content_hash,
                    json_path,
                    asset_count,
                    file_size_bytes,
                    now,
                    if current_version == 0 {
                        "初回シリーズ保存"
                    } else {
                        "シリーズ情報更新"
                    },
                ],
            )
            .map_err(|e| format!("Failed to insert series version: {}", e))?;
        }
        drop(conn);
        self.get_series(source, source_key)
    }

    pub fn list_download_relations(
        &self,
        relation_type: Option<&str>,
    ) -> Result<Vec<DownloadRelation>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from(
            "SELECT
                MIN(download_id) AS download_id,
                relation_type,
                source,
                relation_id,
                relation_name,
                COUNT(DISTINCT download_id) AS work_count
             FROM download_relations",
        );
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(t) = relation_type {
            if !t.is_empty() && t != "all" {
                sql.push_str(" WHERE relation_type = ?");
                bind_values.push(Box::new(t.to_string()));
            }
        }
        sql.push_str(" GROUP BY relation_type, source, relation_id, relation_name ORDER BY work_count DESC, relation_name COLLATE NOCASE ASC");
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok(DownloadRelation {
                    download_id: row.get(0)?,
                    relation_type: row.get(1)?,
                    source: row.get(2)?,
                    relation_id: row.get(3)?,
                    relation_name: row.get(4)?,
                    work_count: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query failed: {}", e))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        if relation_type
            .map(|t| t == "author" || t == "all")
            .unwrap_or(true)
        {
            let mut existing = std::collections::HashSet::new();
            for rel in &results {
                existing.insert(format!(
                    "{}:{}:{}",
                    rel.relation_type, rel.source, rel.relation_id
                ));
            }
            let mut stmt = conn
                .prepare(
                    "SELECT
                        MIN(id) AS download_id,
                        'author' AS relation_type,
                        source,
                        author_id AS relation_id,
                        author_name AS relation_name,
                        COUNT(*) AS work_count
                     FROM downloads
                     WHERE author_id IS NOT NULL AND author_id != ''
                     GROUP BY source, author_id, author_name
                     ORDER BY work_count DESC, relation_name COLLATE NOCASE ASC",
                )
                .map_err(|e| format!("Author relation query prepare failed: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(DownloadRelation {
                        download_id: row.get(0)?,
                        relation_type: row.get(1)?,
                        source: row.get(2)?,
                        relation_id: row.get(3)?,
                        relation_name: row.get(4)?,
                        work_count: row.get(5)?,
                    })
                })
                .map_err(|e| format!("Author relation query failed: {}", e))?;
            for row in rows {
                let rel = row.map_err(|e| format!("Author relation row read failed: {}", e))?;
                let key = format!("{}:{}:{}", rel.relation_type, rel.source, rel.relation_id);
                if existing.insert(key) {
                    results.push(rel);
                }
            }
        }
        Ok(results)
    }

    /// アセットを挿入
    pub fn insert_asset(&self, asset: &NewAsset) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO assets (
                download_id, asset_type, filename, local_path,
                original_url, mime_type, file_size_bytes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                asset.download_id,
                asset.asset_type,
                asset.filename,
                asset.local_path,
                asset.original_url,
                asset.mime_type,
                asset.file_size_bytes,
            ],
        )
        .map_err(|e| format!("Insert asset failed: {}", e))?;
        Ok(conn.last_insert_rowid())
    }

    /// Cursor-based library/search entrypoint backed by Tantivy for full text
    /// and SQLite for metadata filters.
    pub fn search_downloads_v2(&self, params: &SearchV2Params) -> Result<SearchV2Result, String> {
        self.search_downloads_v2_inner(params, 1_000)
    }

    pub fn search_downloads_v2_internal(
        &self,
        params: &SearchV2Params,
        max_limit: i64,
    ) -> Result<SearchV2Result, String> {
        self.search_downloads_v2_inner(params, max_limit)
    }

    fn search_downloads_v2_inner(
        &self,
        params: &SearchV2Params,
        max_limit: i64,
    ) -> Result<SearchV2Result, String> {
        let mut effective_params = normalize_search_params(params);
        let limit = effective_params.limit.unwrap_or(80).clamp(1, max_limit);
        effective_params.limit = Some(limit);
        let query = query_text(&effective_params);
        let status = self.get_search_index_status()?;
        let semantic_ready = status.semantic_model_ready;
        let semantic_complete = status.semantic_indexed_chunks > 0 || status.total_downloads == 0;

        if query.is_empty() {
            let cursor = decode_cursor(effective_params.cursor.as_deref()).filter(|cursor| {
                cursor.kind == "sql"
                    && cursor.sort_by == effective_sort_by(&effective_params)
                    && cursor.sort_order == effective_sort_order(&effective_params)
            });
            let mut items = self.search_sql_page(&effective_params, limit + 1, cursor.as_ref())?;
            let has_more = items.len() as i64 > limit;
            if has_more {
                items.truncate(limit as usize);
            }
            let total_estimate = self.count_sql_matches(&effective_params).ok();
            let next_cursor = if has_more {
                items
                    .last()
                    .and_then(|item| encode_sql_cursor(&effective_params, item))
            } else {
                None
            };
            return Ok(SearchV2Result {
                items,
                next_cursor,
                total_estimate,
                search_meta: SearchMeta {
                    engine: "sqlite-metadata".to_string(),
                    query: effective_params
                        .query
                        .clone()
                        .or(effective_params.text.clone()),
                    total_estimate,
                    index_complete: status.is_complete,
                    semantic_index_complete: Some(semantic_complete),
                    semantic_model_ready: Some(semantic_ready),
                },
                facets_version: status.indexed_downloads,
            });
        }

        let search_mode = effective_search_mode(&effective_params);
        let search_limit = search_candidate_limit(&effective_params, limit);
        let parsed_query = parse_search_query(query);
        let lexical_hits = if search_mode != "semantic" {
            super::tantivy_index::search(&self.storage_dir, query, search_limit)?
        } else {
            Vec::new()
        };
        let semantic_hits = if search_mode == "semantic" {
            match super::semantic_index::search(&self.storage_dir, query, search_limit) {
                Ok(hits) => hits,
                Err(error) => {
                    log::warn!("Semantic search unavailable: {}", error);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let hits = blend_search_hits(&lexical_hits, &semantic_hits, &search_mode);
        let semantic_map = hits
            .iter()
            .filter_map(|hit| {
                hit.semantic
                    .clone()
                    .map(|semantic| (hit.download_id, semantic))
            })
            .collect::<HashMap<_, _>>();
        let mut ranked_items = self.fetch_ranked_sql_matches(&effective_params, &hits)?;
        if !parsed_query.exclude.is_empty() {
            ranked_items =
                filter_excluded_search_results(&self.storage_dir, ranked_items, &parsed_query);
        }
        ranked_items.sort_by(|a, b| {
            b.search_score
                .unwrap_or(0.0)
                .partial_cmp(&a.search_score.unwrap_or(0.0))
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.downloaded_at.cmp(&a.downloaded_at))
                .then_with(|| b.id.cmp(&a.id))
        });
        if let Some(cursor) =
            decode_cursor(effective_params.cursor.as_deref()).filter(|c| c.kind == "search")
        {
            ranked_items.retain(|item| search_item_is_after_cursor(item, &cursor));
        }
        let total_estimate = Some(ranked_items.len() as i64);
        let has_more = ranked_items.len() as i64 > limit;
        let page_items = ranked_items
            .drain(..)
            .take(limit as usize)
            .collect::<Vec<DownloadEntry>>();
        let next_cursor = if has_more {
            page_items.last().and_then(encode_search_cursor)
        } else {
            None
        };
        let items =
            decorate_search_results(&self.storage_dir, page_items, &parsed_query, &semantic_map);

        Ok(SearchV2Result {
            items,
            next_cursor,
            total_estimate,
            search_meta: SearchMeta {
                engine: if search_mode == "exact" {
                    "tantivy-exact".to_string()
                } else if search_mode == "semantic" {
                    "semantic-local".to_string()
                } else {
                    "hybrid-local".to_string()
                },
                query: effective_params
                    .query
                    .clone()
                    .or(effective_params.text.clone()),
                total_estimate,
                index_complete: status.is_complete,
                semantic_index_complete: Some(semantic_complete),
                semantic_model_ready: Some(semantic_ready),
            },
            facets_version: status.indexed_downloads,
        })
    }

    fn search_sql_page(
        &self,
        params: &SearchV2Params,
        limit: i64,
        cursor: Option<&SearchCursor>,
    ) -> Result<Vec<DownloadEntry>, String> {
        let conn = self.read_conn()?;
        let mut sql = download_select_sql_for_projection(
            params.projection.as_deref(),
            "NULL",
            &sort_key_select_expr(params),
        );
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_library_filters(params, &mut wheres, &mut bind_values);
        append_keyset_filter(params, cursor, &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(&sort_clause(params));
        sql.push_str(" LIMIT ?");
        bind_values.push(Box::new(limit));
        query_download_entries(&conn, &sql, &bind_values)
    }

    fn count_sql_matches(&self, params: &SearchV2Params) -> Result<i64, String> {
        let conn = self.read_conn()?;
        let mut sql = "SELECT COUNT(*) FROM downloads d".to_string();
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        append_library_filters(params, &mut wheres, &mut bind_values);
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();
        conn.query_row(&sql, refs.as_slice(), |row| row.get(0))
            .map_err(|e| format!("Count query failed: {}", e))
    }

    fn fetch_ranked_sql_matches(
        &self,
        params: &SearchV2Params,
        hits: &[RankedSearchHit],
    ) -> Result<Vec<DownloadEntry>, String> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        let rank_map = hits
            .iter()
            .enumerate()
            .map(|(idx, hit)| (hit.download_id, (idx, hit.score)))
            .collect::<HashMap<i64, (usize, f64)>>();
        let conn = self.read_conn()?;
        let mut items = Vec::new();

        for chunk in hits.chunks(500) {
            let mut sql =
                download_select_sql_for_projection(params.projection.as_deref(), "NULL", "NULL");
            let mut wheres = Vec::new();
            let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            append_library_filters(params, &mut wheres, &mut bind_values);
            let placeholders = vec!["?"; chunk.len()].join(", ");
            wheres.push(format!("d.id IN ({})", placeholders));
            for hit in chunk {
                bind_values.push(Box::new(hit.download_id));
            }
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
            let mut chunk_items = query_download_entries(&conn, &sql, &bind_values)?;
            for item in &mut chunk_items {
                if let Some((_, score)) = rank_map.get(&item.id) {
                    item.search_score = Some(*score);
                }
            }
            items.extend(chunk_items);
        }

        items.sort_by_key(|item| {
            rank_map
                .get(&item.id)
                .map(|(idx, _)| *idx)
                .unwrap_or(usize::MAX)
        });
        Ok(items)
    }

    /// 単一ダウンロードの取得
    pub fn get_download(&self, id: i64) -> Result<DownloadEntry, String> {
        let conn = self.read_conn()?;
        let sql = format!(
            "{} WHERE d.id = ?1",
            download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL")
        );
        conn.query_row(&sql, params![id], download_entry_from_row)
            .map_err(|e| format!("Download not found: {}", e))
    }

    /// 複数の作品をまとめて取得する。
    ///
    /// EPUBキューのように多数のIDを扱う画面で1件ずつ問い合わせると、
    /// 件数分のIPCとクエリが発生する。存在しないIDは結果から除かれるので、
    /// 呼び出し側は削除済みの項目を検出できる。
    pub fn get_downloads(&self, ids: &[i64]) -> Result<Vec<DownloadEntry>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let unique: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            ids.iter().copied().filter(|id| seen.insert(*id)).collect()
        };
        let conn = self.read_conn()?;
        let mut entries = Vec::with_capacity(unique.len());
        // SQLite caps host parameters, so long queues are read in chunks.
        for chunk in unique.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "{} WHERE d.id IN ({})",
                download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL"),
                placeholders
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    download_entry_from_row(row)
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                entries.push(row.map_err(|e| e.to_string())?);
            }
        }
        // Preserve the caller's ordering, which is the queue order on screen.
        let position: HashMap<i64, usize> = unique
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
        entries.sort_by_key(|entry| position.get(&entry.id).copied().unwrap_or(usize::MAX));
        Ok(entries)
    }

    /// 特定のソースIDのダウンロードが存在するか確認する
    pub fn check_exists(&self, source: &str, source_id: &str) -> Result<bool, String> {
        let conn = self.read_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query failed: {}", e))?;
        Ok(count > 0)
    }

    /// ダウンロードのアセット一覧取得
    pub fn get_assets(&self, download_id: i64) -> Result<Vec<AssetEntry>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM assets WHERE download_id = ?1")
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(AssetEntry {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    asset_type: row.get(2)?,
                    filename: row.get(3)?,
                    local_path: row.get(4)?,
                    original_url: row.get(5)?,
                    mime_type: row.get(6)?,
                    file_size_bytes: row.get(7)?,
                })
            })
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_reader_document(
        &self,
        download_id: i64,
        version: Option<i64>,
    ) -> Result<ReaderDocument, String> {
        let download = self.get_download(download_id)?;
        let assets = self.get_assets(download_id)?;
        let versions = self.get_versions(download_id)?;

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let active_edit = active_edit_revision_locked(&conn, download_id)?;
        if version.is_none() {
            if let Some(edit) = active_edit.clone() {
                let blocks = blocks_for_revision_locked(&conn, edit.id)?;
                return Ok(ReaderDocument {
                    download,
                    assets: assets.clone(),
                    versions,
                    html: blocks_to_html(&blocks, &assets),
                    plain_text: blocks_to_plain_text(&blocks),
                    is_edited: true,
                    active_edit_revision: Some(edit),
                });
            }
        }
        drop(conn);

        let target_version = version.unwrap_or(download.current_version);
        let raw_json = self.read_download_json_for_version(&download, &versions, target_version)?;
        let html = if download.source == "pixiv" {
            super::parser::parse_pixiv_to_html(&raw_json, &assets)
        } else if download.source == "fanbox" {
            super::parser::parse_fanbox_to_html(&raw_json, &assets)
        } else {
            String::new()
        };
        let plain_text = serde_json::from_str::<serde_json::Value>(&raw_json)
            .ok()
            .map(|value| extract_search_body(&value, &download.source))
            .unwrap_or_default();

        Ok(ReaderDocument {
            download,
            assets,
            versions,
            html,
            plain_text,
            is_edited: false,
            active_edit_revision: active_edit,
        })
    }

    pub fn get_editor_document(&self, download_id: i64) -> Result<EditorDocument, String> {
        let download = self.get_download(download_id)?;
        let assets = self.get_assets(download_id)?;
        let versions = self.get_versions(download_id)?;
        let base_version = versions
            .iter()
            .map(|v| v.version)
            .max()
            .unwrap_or(download.current_version);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let active_revision = active_edit_revision_locked(&conn, download_id)?;
        let draft_revision = draft_edit_revision_locked(&conn, download_id)?;

        if let Some(draft) = draft_revision.clone() {
            let blocks = blocks_for_revision_locked(&conn, draft.id)?;
            return Ok(EditorDocument {
                download,
                assets,
                active_revision,
                draft_revision: Some(draft),
                base_version,
                blocks,
            });
        }

        if let Some(active) = active_revision.clone() {
            let blocks = blocks_for_revision_locked(&conn, active.id)?;
            return Ok(EditorDocument {
                download,
                assets,
                active_revision: Some(active),
                draft_revision: None,
                base_version,
                blocks,
            });
        }
        drop(conn);

        let raw_json =
            self.read_download_json_for_version(&download, &versions, download.current_version)?;
        let source_html = if download.source == "pixiv" {
            super::parser::parse_pixiv_to_html(&raw_json, &assets)
        } else if download.source == "fanbox" {
            super::parser::parse_fanbox_to_html(&raw_json, &assets)
        } else {
            String::new()
        };
        let blocks = html_to_editor_blocks(&source_html, &assets);

        Ok(EditorDocument {
            download,
            assets,
            active_revision,
            draft_revision: None,
            base_version,
            blocks,
        })
    }

    pub fn save_work_draft(
        &self,
        download_id: i64,
        base_version: i64,
        blocks: &[WorkBlockInput],
    ) -> Result<WorkEditRevision, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let normalized_blocks = normalize_block_inputs(blocks);
        let content_hash = hash_blocks(&normalized_blocks);
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;

        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'archived', updated_at = ?2
             WHERE download_id = ?1 AND status = 'draft'",
            params![download_id, now],
        )
        .map_err(|e| format!("Failed to archive previous draft: {}", e))?;

        tx.execute(
            "INSERT INTO work_edit_revisions (
                download_id, base_version, status, title, content_hash, created_at, updated_at
             ) VALUES (?1, ?2, 'draft', NULL, ?3, ?4, ?4)",
            params![download_id, base_version, content_hash, now],
        )
        .map_err(|e| format!("Failed to insert draft revision: {}", e))?;
        let revision_id = tx.last_insert_rowid();

        insert_work_blocks_locked(&tx, revision_id, &normalized_blocks)?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        get_work_edit_revision_locked(&conn, revision_id)
    }

    pub fn activate_work_edit(&self, edit_revision_id: i64) -> Result<WorkEditRevision, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let revision = get_work_edit_revision_locked(&conn, edit_revision_id)?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;
        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'archived', updated_at = ?2
             WHERE download_id = ?1 AND status = 'active'",
            params![revision.download_id, now],
        )
        .map_err(|e| format!("Failed to archive active revision: {}", e))?;
        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'active', updated_at = ?2
             WHERE id = ?1",
            params![edit_revision_id, now],
        )
        .map_err(|e| format!("Failed to activate revision: {}", e))?;
        tx.execute(
            "DELETE FROM search_index_state WHERE download_id = ?1",
            params![revision.download_id],
        )
        .map_err(|e| format!("Failed to invalidate search index: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        reindex_download_locked(&conn, &self.storage_dir, revision.download_id)?;
        get_work_edit_revision_locked(&conn, edit_revision_id)
    }

    fn read_download_json_for_version(
        &self,
        download: &DownloadEntry,
        versions: &[DownloadVersion],
        version: i64,
    ) -> Result<String, String> {
        let mut target_path = download
            .original_json_path
            .clone()
            .filter(|p| Path::new(p).exists())
            .unwrap_or_else(|| download.json_path.clone());
        if version != download.current_version {
            if let Some(target_version) = versions.iter().find(|v| v.version == version) {
                target_path = target_version
                    .original_json_path
                    .clone()
                    .filter(|p| Path::new(p).exists())
                    .unwrap_or_else(|| target_version.json_path.clone());
            }
        }
        std::fs::read_to_string(&target_path)
            .map_err(|e| format!("Failed to read download JSON: {}", e))
    }

    /// ダウンロードを削除（カスケードでアセットも削除）
    pub fn delete_download(&self, id: i64) -> Result<(), String> {
        let json_path = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;

            let (jp, _ojp): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT json_path, original_json_path FROM downloads WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| format!("Download not found: {}", e))?;

            // DBから削除
            conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
                .map_err(|e| format!("Delete failed: {}", e))?;

            jp
        }; // ここで conn (MutexGuard) がスコープ外になり、ロックが即座に解放される！

        // データベースのロックが解放された安全な状態で、重いフォルダ削除（リトライ待ちが発生し得る）を実行
        if let Some(jp) = json_path {
            let jp_path = std::path::Path::new(&jp);
            // jp_path は downloads/{source}/{source_id}/v{version}/data.json
            // その parent() である version_dir は downloads/{source}/{source_id}/v{version}
            if let Some(version_dir) = jp_path.parent() {
                // その parent() である work_root_dir は downloads/{source}/{source_id}
                if let Some(work_root_dir) = version_dir.parent() {
                    if work_root_dir.exists() {
                        if let Err(e) = remove_dir_all_resilient(work_root_dir) {
                            log::warn!(
                                "Failed to remove work root directory {:?}: {}",
                                work_root_dir,
                                e
                            );
                        }
                        // その親フォルダ（ソースフォルダ: e.g. downloads/pixiv）が空ならクリーンアップ
                        if let Some(source_dir) = work_root_dir.parent() {
                            let _ = remove_dir_resilient(source_dir); // 空のときのみ成功
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn recover_update_jobs_on_startup(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_jobs
             SET status = 'paused',
                 active_label = '前回の起動中に中断されました',
                 updated_at = ?1,
                 finished_at = NULL
             WHERE status IN ('running', 'queued', 'canceling')",
            params![chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to recover update jobs: {}", e))?;
        conn.execute(
            "UPDATE update_job_items
             SET status = 'queued', updated_at = CURRENT_TIMESTAMP
             WHERE status = 'running'",
            [],
        )
        .map_err(|e| format!("Failed to recover update job items: {}", e))?;
        Ok(())
    }

    pub fn create_update_job(
        &self,
        job_id: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start update job transaction: {}", e))?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut persisted_request = request.clone();
        persisted_request.credentials = None;
        let request_json = serde_json::to_string(&persisted_request).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO update_jobs (
                id, scope, mode, status, request_json, totals, processed,
                candidate_count, saved_count, error_count, active_label,
                started_at, updated_at, finished_at
             ) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, 0, 0, 0, 0, NULL, ?6, ?6, NULL)",
            params![
                job_id,
                request.scope,
                request.mode,
                request_json,
                items.len() as i64,
                now,
            ],
        )
        .map_err(|e| format!("Failed to insert update job: {}", e))?;
        for item in items {
            tx.execute(
                "INSERT INTO update_job_items (
                    job_id, item_type, source, source_id, target_type, title, payload_json, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    job_id,
                    item.item_type,
                    item.source,
                    item.source_id,
                    item.target_type,
                    item.title,
                    item.payload_json,
                    item.status,
                ],
            )
            .map_err(|e| format!("Failed to insert update job item: {}", e))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit update job: {}", e))?;
        drop(conn);
        self.append_update_job_log(job_id, "info", "更新ジョブを作成しました")?;
        self.update_job_snapshot(job_id)
    }

    pub fn list_update_jobs(&self) -> Result<Vec<UpdateJobSummary>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at
                 FROM update_jobs
                 ORDER BY updated_at DESC, started_at DESC
                 LIMIT 30",
            )
            .map_err(|e| format!("Failed to prepare update job list: {}", e))?;
        let rows = stmt
            .query_map([], update_job_summary_from_row)
            .map_err(|e| format!("Failed to list update jobs: {}", e))?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|e| format!("Failed to read update job: {}", e))?);
        }
        Ok(jobs)
    }

    pub fn update_job_snapshot(&self, job_id: &str) -> Result<UpdateJobSnapshot, String> {
        self.sync_update_job_counters(job_id)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let summary = conn
            .query_row(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at
                 FROM update_jobs WHERE id = ?1",
                params![job_id],
                update_job_summary_from_row,
            )
            .map_err(|e| format!("Update job not found: {}", e))?;

        let mut log_stmt = conn
            .prepare(
                "SELECT id, log_type, message, created_at
                 FROM (
                    SELECT id, log_type, message, created_at
                    FROM update_job_logs
                    WHERE job_id = ?1
                    ORDER BY id DESC
                    LIMIT 300
                 ) ORDER BY id ASC",
            )
            .map_err(|e| format!("Failed to prepare update job logs: {}", e))?;
        let log_rows = log_stmt
            .query_map(params![job_id], |row| {
                Ok(UpdateJobLog {
                    id: row.get(0)?,
                    log_type: row.get(1)?,
                    message: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("Failed to read update job logs: {}", e))?;
        let mut logs = Vec::new();
        for row in log_rows {
            logs.push(row.map_err(|e| format!("Failed to read update job log: {}", e))?);
        }

        let mut candidate_stmt = conn
            .prepare(
                "SELECT id, source, source_id, title, target_type, payload_json, status
                 FROM update_job_items
                 WHERE job_id = ?1 AND item_type = 'candidate'
                 ORDER BY id ASC",
            )
            .map_err(|e| format!("Failed to prepare update job candidates: {}", e))?;
        let candidate_rows = candidate_stmt
            .query_map(params![job_id], |row| {
                let id: i64 = row.get(0)?;
                let source: String = row.get(1)?;
                let source_id: String = row.get(2)?;
                let title: String = row.get(3)?;
                let target_type: String = row.get(4)?;
                let payload_json: String = row.get(5)?;
                let status: String = row.get(6)?;
                let payload: serde_json::Value =
                    serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
                let target_label = payload
                    .get("targetLabel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let subtitle = payload
                    .get("subtitle")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(UpdateJobCandidate {
                    id,
                    key: format!("candidate:{}:{}:{}", source, source_id, id),
                    source,
                    source_id,
                    title,
                    subtitle,
                    target_label,
                    target_type,
                    selected: matches!(status.as_str(), "candidate" | "queued"),
                    status,
                })
            })
            .map_err(|e| format!("Failed to read update job candidates: {}", e))?;
        let mut candidates = Vec::new();
        for row in candidate_rows {
            candidates
                .push(row.map_err(|e| format!("Failed to read update job candidate: {}", e))?);
        }

        Ok(UpdateJobSnapshot {
            job_id: summary.job_id,
            status: summary.status,
            scope: summary.scope,
            mode: summary.mode,
            totals: summary.totals,
            processed: summary.processed,
            candidate_count: summary.candidate_count,
            saved_count: summary.saved_count,
            error_count: summary.error_count,
            active_label: summary.active_label,
            logs,
            candidates,
            started_at: summary.started_at,
            updated_at: summary.updated_at,
            finished_at: summary.finished_at,
        })
    }

    pub fn set_update_job_status(
        &self,
        job_id: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let terminal = matches!(
            status,
            "completed" | "failed" | "canceled" | "auth_required"
        );
        conn.execute(
            "UPDATE update_jobs
             SET status = ?1,
                 active_label = ?2,
                 updated_at = ?3,
                 finished_at = CASE WHEN ?4 THEN COALESCE(finished_at, ?3) ELSE NULL END
             WHERE id = ?5",
            params![status, active_label, now, terminal, job_id],
        )
        .map_err(|e| format!("Failed to update job status: {}", e))?;
        Ok(())
    }

    pub fn prepare_update_job_resume(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_job_items
             SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE job_id = ?1 AND status = 'running'",
            params![job_id],
        )
        .map_err(|e| format!("Failed to reset running update items: {}", e))?;
        if retry_failed {
            conn.execute(
                "UPDATE update_job_items
                 SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE job_id = ?1 AND status = 'failed'",
                params![job_id],
            )
            .map_err(|e| format!("Failed to reset failed update items: {}", e))?;
        }
        Ok(())
    }

    pub fn append_update_job_log(
        &self,
        job_id: &str,
        log_type: &str,
        message: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_job_logs (job_id, log_type, message)
             VALUES (?1, ?2, ?3)",
            params![job_id, log_type, message],
        )
        .map_err(|e| format!("Failed to append update job log: {}", e))?;
        conn.execute(
            "UPDATE update_jobs SET updated_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), job_id],
        )
        .map_err(|e| format!("Failed to touch update job: {}", e))?;
        Ok(())
    }

    pub fn update_job_status_value(&self, job_id: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT status FROM update_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Update job not found: {}", e))
    }

    pub fn next_update_job_item(&self, job_id: &str) -> Result<Option<UpdateJobItem>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let item = conn
            .query_row(
                "SELECT id, job_id, item_type, source, source_id, target_type, title,
                        payload_json, status, error, result_download_id
                 FROM update_job_items
                 WHERE job_id = ?1 AND status = 'queued'
                 ORDER BY CASE item_type WHEN 'work' THEN 0 WHEN 'target' THEN 1 ELSE 2 END, id ASC
                 LIMIT 1",
                params![job_id],
                update_job_item_from_row,
            )
            .optional()
            .map_err(|e| format!("Failed to fetch next update item: {}", e))?;
        if let Some(item) = item {
            conn.execute(
                "UPDATE update_job_items SET status = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![item.id],
            )
            .map_err(|e| format!("Failed to mark update item running: {}", e))?;
            conn.execute(
                "UPDATE update_jobs SET active_label = ?1, status = 'running', updated_at = ?2 WHERE id = ?3",
                params![item.title, chrono::Utc::now().to_rfc3339(), job_id],
            )
            .map_err(|e| format!("Failed to mark update job running: {}", e))?;
            return Ok(Some(UpdateJobItem {
                status: "running".to_string(),
                ..item
            }));
        }
        Ok(None)
    }

    pub fn complete_update_job_item(
        &self,
        item_id: i64,
        status: &str,
        error: Option<&str>,
        result_download_id: Option<i64>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_job_items
             SET status = ?1, error = ?2, result_download_id = ?3, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4",
            params![status, error, result_download_id, item_id],
        )
        .map_err(|e| format!("Failed to update job item: {}", e))?;
        Ok(())
    }

    pub fn insert_update_job_candidate(
        &self,
        job_id: &str,
        candidate: &UpdateJobItemInput,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM update_job_items
                 WHERE job_id = ?1 AND item_type = 'candidate' AND source = ?2 AND source_id = ?3",
                params![job_id, candidate.source, candidate.source_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check candidate: {}", e))?;
        if exists > 0 {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO update_job_items (
                job_id, item_type, source, source_id, target_type, title, payload_json, status
             ) VALUES (?1, 'candidate', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                job_id,
                candidate.source,
                candidate.source_id,
                candidate.target_type,
                candidate.title,
                candidate.payload_json,
                candidate.status,
            ],
        )
        .map_err(|e| format!("Failed to insert update candidate: {}", e))?;
        Ok(())
    }

    pub fn queue_update_job_candidates(
        &self,
        job_id: &str,
        candidate_ids: &[i64],
    ) -> Result<i64, String> {
        if candidate_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut changed = 0i64;
        for id in candidate_ids {
            changed += conn
                .execute(
                    "UPDATE update_job_items
                     SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
                     WHERE id = ?1 AND job_id = ?2 AND item_type = 'candidate' AND status IN ('candidate', 'failed')",
                    params![id, job_id],
                )
                .map_err(|e| format!("Failed to queue update candidate: {}", e))?
                as i64;
        }
        Ok(changed)
    }

    pub fn clear_update_job(&self, job_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_jobs WHERE id = ?1 AND status NOT IN ('queued', 'running', 'canceling')",
            params![job_id],
        )
        .map_err(|e| format!("Failed to clear update job: {}", e))?;
        Ok(())
    }

    fn sync_update_job_counters(&self, job_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_jobs
             SET totals = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND item_type != 'candidate'),
                 processed = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND item_type != 'candidate' AND status IN ('done', 'saved', 'skipped', 'failed')),
                 candidate_count = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND item_type = 'candidate'),
                 saved_count = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND status = 'saved'),
                 error_count = (SELECT COUNT(*) FROM update_job_items WHERE job_id = ?1 AND status = 'failed')
             WHERE id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Failed to sync update job counters: {}", e))?;
        Ok(())
    }

    pub fn delete_downloads(&self, ids: &[i64]) -> Result<BulkMutationResult, String> {
        let mut seen = std::collections::HashSet::new();
        let mut changed_count = 0i64;

        for id in ids.iter().copied().filter(|id| seen.insert(*id)) {
            self.delete_download(id)?;
            changed_count += 1;
        }

        Ok(BulkMutationResult {
            matched_count: seen.len() as i64,
            changed_count,
        })
    }

    /// 選択した作品のお気に入り・更新監視をまとめて更新する。
    ///
    /// 1件ずつコマンドを呼ぶと選択数だけIPCとトランザクションが走るため、
    /// 大量選択時に極端に遅くなる。単一トランザクションで処理する。
    pub fn set_flags_for_ids(
        &self,
        ids: &[i64],
        favorite: Option<bool>,
        watch: Option<bool>,
    ) -> Result<BulkMutationResult, String> {
        let unique: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            ids.iter().copied().filter(|id| seen.insert(*id)).collect()
        };
        if unique.is_empty() || (favorite.is_none() && watch.is_none()) {
            return Ok(BulkMutationResult {
                matched_count: unique.len() as i64,
                changed_count: 0,
            });
        }

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut changed_count = 0i64;
        {
            let mut assignments = Vec::new();
            if favorite.is_some() {
                assignments.push("favorite = ?1");
            }
            if watch.is_some() {
                assignments.push(if favorite.is_some() {
                    "watch_updates = ?2"
                } else {
                    "watch_updates = ?1"
                });
            }
            let sql = format!(
                "UPDATE downloads SET {} WHERE id = ?{}",
                assignments.join(", "),
                assignments.len() + 1
            );
            let mut stmt = tx.prepare(&sql).map_err(|e| e.to_string())?;
            for id in &unique {
                let affected = match (favorite, watch) {
                    (Some(fav), Some(wat)) => stmt.execute(rusqlite::params![fav, wat, id]),
                    (Some(fav), None) => stmt.execute(rusqlite::params![fav, id]),
                    (None, Some(wat)) => stmt.execute(rusqlite::params![wat, id]),
                    (None, None) => Ok(0),
                }
                .map_err(|e| e.to_string())?;
                changed_count += affected as i64;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;

        Ok(BulkMutationResult {
            matched_count: unique.len() as i64,
            changed_count,
        })
    }

    /// 統計情報の取得
    pub fn get_stats(&self) -> Result<DbStats, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let total_downloads: i64 = conn
            .query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))
            .unwrap_or(0);
        let pixiv_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE source = 'pixiv'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let fanbox_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE source = 'fanbox'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let total_assets: i64 = conn
            .query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap_or(0);
        let total_size: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(file_size_bytes), 0) FROM downloads",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        Ok(DbStats {
            total_downloads,
            pixiv_count,
            fanbox_count,
            total_assets,
            total_size_bytes: total_size,
        })
    }

    pub fn get_dashboard_summary(&self) -> Result<DashboardSummary, String> {
        let stats = self.get_stats()?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let favorite_count = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE favorite = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let watched_count = conn
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE watch_updates = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let update_target_count = conn
            .query_row(
                "SELECT COUNT(*) FROM update_targets WHERE enabled = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let indexed_count = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM downloads d
                 JOIN search_index_state m ON m.download_id = d.id
                 WHERE m.current_version = d.current_version
                   AND COALESCE(m.content_hash, '') = COALESCE(d.content_hash, '')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let pending_index_count = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM downloads d
                 LEFT JOIN search_index_state m ON m.download_id = d.id
                 WHERE m.download_id IS NULL
                    OR m.current_version != d.current_version
                    OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let collect_facets = |sql: &str, limit: i64| -> Result<Vec<FacetCount>, String> {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("Dashboard facet prepare failed: {}", e))?;
            let rows = stmt
                .query_map(params![limit], |row| {
                    Ok(FacetCount {
                        name: row.get(0)?,
                        count: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Dashboard facet query failed: {}", e))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| format!("Dashboard facet row failed: {}", e))?);
            }
            Ok(out)
        };

        let top_tags = collect_facets(
            "SELECT t.name, COUNT(dt.download_id) AS count
             FROM tags t
             JOIN download_tags dt ON dt.tag_id = t.id
             GROUP BY t.id, t.name
             ORDER BY count DESC, t.name ASC
             LIMIT ?1",
            12,
        )?;
        let top_authors = collect_facets(
            "SELECT author_name, COUNT(*) AS count
             FROM downloads
             WHERE author_name IS NOT NULL AND author_name != ''
             GROUP BY author_name
             ORDER BY count DESC, author_name ASC
             LIMIT ?1",
            12,
        )?;

        let mut source_stmt = conn
            .prepare(
                "SELECT source, COUNT(*) AS count, COALESCE(SUM(file_size_bytes), 0)
                 FROM downloads
                 GROUP BY source
                 ORDER BY count DESC",
            )
            .map_err(|e| format!("Dashboard source prepare failed: {}", e))?;
        let source_rows = source_stmt
            .query_map([], |row| {
                Ok(SourceBreakdown {
                    source: row.get(0)?,
                    count: row.get(1)?,
                    total_size_bytes: row.get(2)?,
                })
            })
            .map_err(|e| format!("Dashboard source query failed: {}", e))?;
        let mut source_breakdown = Vec::new();
        for row in source_rows {
            source_breakdown.push(row.map_err(|e| format!("Dashboard source row failed: {}", e))?);
        }

        let mut trend_stmt = conn
            .prepare(
                "SELECT substr(downloaded_at, 1, 7) AS bucket,
                        COUNT(*) AS count,
                        SUM(CASE WHEN source = 'pixiv' THEN 1 ELSE 0 END) AS pixiv_count,
                        SUM(CASE WHEN source = 'fanbox' THEN 1 ELSE 0 END) AS fanbox_count,
                        COALESCE(SUM(file_size_bytes), 0) AS total_size
                 FROM downloads
                 GROUP BY bucket
                 ORDER BY bucket DESC
                 LIMIT 12",
            )
            .map_err(|e| format!("Dashboard trend prepare failed: {}", e))?;
        let trend_rows = trend_stmt
            .query_map([], |row| {
                Ok(DashboardTrendPoint {
                    bucket: row.get(0)?,
                    count: row.get(1)?,
                    pixiv_count: row.get(2)?,
                    fanbox_count: row.get(3)?,
                    total_size_bytes: row.get(4)?,
                })
            })
            .map_err(|e| format!("Dashboard trend query failed: {}", e))?;
        let mut monthly_downloads = Vec::new();
        for row in trend_rows {
            monthly_downloads.push(row.map_err(|e| format!("Dashboard trend row failed: {}", e))?);
        }
        monthly_downloads.reverse();

        drop(trend_stmt);
        drop(source_stmt);
        drop(conn);

        let recent_downloads = self
            .search_downloads_v2(&SearchV2Params {
                text: None,
                query: None,
                source: None,
                content_type: None,
                sort_by: Some("date".to_string()),
                sort_order: Some("desc".to_string()),
                limit: Some(8),
                cursor: None,
                favorite: None,
                tags_include: None,
                tags_exclude: None,
                tag_filter_mode: None,
                authors_include: None,
                authors_exclude: None,
                min_char_count: None,
                max_char_count: None,
                asset_filter: None,
                watch_filter: None,
                person_source: None,
                person_key: None,
                series_source: None,
                series_key: None,
                view_mode: Some("compact".to_string()),
                projection: Some("libraryCompact".to_string()),
                search_mode: None,
            })?
            .items;

        Ok(DashboardSummary {
            stats,
            favorite_count,
            watched_count,
            update_target_count,
            indexed_count,
            pending_index_count,
            top_tags,
            top_authors,
            recent_downloads,
            source_breakdown,
            monthly_downloads,
        })
    }

    /// ライブラリのフィルター候補一覧を取得
    /// 作者・シリーズの一覧を検索・ページングして返す。
    ///
    /// `get_filter_facets` は上位60件しか返さないため、ライブラリの
    /// 作者/シリーズタブでは大半のエンティティに到達できなかった。絞り込みと
    /// ページングをSQL側で行い、件数に依存せず一覧できるようにする。
    pub fn search_entity_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EntityFacet>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        let filter = query.map(str::trim).unwrap_or("");
        let has_filter = !filter.is_empty();
        let like = format!("%{}%", filter.replace('%', "\\%").replace('_', "\\_"));

        let sql = match kind {
            "person" | "people" | "author" | "authors" => {
                let having = if has_filter {
                    "HAVING COALESCE(p.display_name, d.author_name) LIKE ?1 ESCAPE '\\'
                         OR COALESCE(p.description, '') LIKE ?1 ESCAPE '\\'"
                } else {
                    ""
                };
                format!(
                    "SELECT
                        d.source,
                        d.author_id,
                        COALESCE(p.display_name, d.author_name) AS display_name,
                        COUNT(DISTINCT d.id) AS count,
                        COALESCE(p.icon_path, p.cover_path) AS cover_path,
                        p.description,
                        p.updated_at,
                        MAX(d.downloaded_at) AS latest_downloaded_at,
                        (
                            SELECT d2.title
                            FROM downloads d2
                            WHERE d2.source = d.source AND d2.author_id = d.author_id
                            ORDER BY COALESCE(d2.source_created_at, d2.downloaded_at) DESC, d2.id DESC
                            LIMIT 1
                        ) AS sample_title,
                        p.icon_path,
                        p.cover_path AS banner_path
                     FROM downloads d
                     LEFT JOIN people p ON p.source = d.source AND p.source_key = d.author_id
                     WHERE d.author_id IS NOT NULL AND d.author_id != ''
                       AND d.author_name IS NOT NULL AND d.author_name != ''
                     GROUP BY d.source, d.author_id, COALESCE(p.display_name, d.author_name), p.icon_path, p.cover_path, p.description, p.updated_at
                     {having}
                     ORDER BY count DESC, display_name ASC
                     LIMIT ?{limit_index} OFFSET ?{offset_index}",
                    having = having,
                    limit_index = if has_filter { 2 } else { 1 },
                    offset_index = if has_filter { 3 } else { 2 },
                )
            }
            "series" => {
                let having = if has_filter {
                    "HAVING COALESCE(s.title, ds.title) LIKE ?1 ESCAPE '\\'
                         OR COALESCE(s.description, '') LIKE ?1 ESCAPE '\\'"
                } else {
                    ""
                };
                format!(
                    "SELECT
                        ds.series_source,
                        ds.series_key,
                        COALESCE(s.title, ds.title) AS title,
                        COUNT(DISTINCT ds.download_id) AS count,
                        s.cover_path,
                        s.description,
                        s.updated_at,
                        MAX(d.downloaded_at) AS latest_downloaded_at,
                        (
                            SELECT d2.title
                            FROM downloads d2
                            JOIN download_series ds2 ON ds2.download_id = d2.id
                            WHERE ds2.series_source = ds.series_source AND ds2.series_key = ds.series_key
                            ORDER BY COALESCE(ds2.content_order, 9223372036854775807) ASC,
                                     COALESCE(d2.source_created_at, d2.downloaded_at) ASC,
                                     d2.id ASC
                            LIMIT 1
                        ) AS sample_title,
                        NULL AS icon_path,
                        s.cover_path AS banner_path
                     FROM download_series ds
                     LEFT JOIN series s ON s.source = ds.series_source AND s.source_key = ds.series_key
                     JOIN downloads d ON d.id = ds.download_id
                     GROUP BY ds.series_source, ds.series_key, COALESCE(s.title, ds.title), s.cover_path, s.description, s.updated_at
                     {having}
                     ORDER BY count DESC, title ASC
                     LIMIT ?{limit_index} OFFSET ?{offset_index}",
                    having = having,
                    limit_index = if has_filter { 2 } else { 1 },
                    offset_index = if has_filter { 3 } else { 2 },
                )
            }
            other => return Err(format!("Unsupported entity facet kind: {}", other)),
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Entity facet search prepare failed: {}", e))?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(EntityFacet {
                source: row.get(0)?,
                source_key: row.get(1)?,
                display_name: row.get(2)?,
                count: row.get(3)?,
                cover_path: row.get(4)?,
                description: row.get(5)?,
                updated_at: row.get(6)?,
                latest_downloaded_at: row.get(7)?,
                sample_title: row.get(8)?,
                icon_path: row.get(9)?,
                banner_path: row.get(10)?,
            })
        };
        let rows = if has_filter {
            stmt.query_map(rusqlite::params![like, limit, offset], map_row)
        } else {
            stmt.query_map(rusqlite::params![limit, offset], map_row)
        }
        .map_err(|e| format!("Entity facet search failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Entity facet row read failed: {}", e))?);
        }
        Ok(results)
    }

    pub fn get_filter_facets(&self) -> Result<FilterFacets, String> {
        self.get_filter_facets_with(true)
    }

    /// `include_entities` を落とすと、作者・シリーズの重い集計を省略する。
    ///
    /// ライブラリの絞り込みUIはタグと種別しか使わないのに、開くたびに相関
    /// サブクエリを含む集計が2本走っていた。大規模ライブラリでは、この2本が
    /// 画面表示の待ち時間の大半を占める。
    pub fn get_filter_facets_with(&self, include_entities: bool) -> Result<FilterFacets, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let collect = |sql: &str| -> Result<Vec<FacetCount>, String> {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("Facet query prepare failed: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(FacetCount {
                        name: row.get(0)?,
                        count: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Facet query failed: {}", e))?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(|e| format!("Facet row read failed: {}", e))?);
            }
            Ok(results)
        };

        let collect_entities = |sql: &str| -> Result<Vec<EntityFacet>, String> {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| format!("Entity facet query prepare failed: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(EntityFacet {
                        source: row.get(0)?,
                        source_key: row.get(1)?,
                        display_name: row.get(2)?,
                        count: row.get(3)?,
                        cover_path: row.get(4)?,
                        description: row.get(5)?,
                        updated_at: row.get(6)?,
                        latest_downloaded_at: row.get(7)?,
                        sample_title: row.get(8)?,
                        icon_path: row.get(9)?,
                        banner_path: row.get(10)?,
                    })
                })
                .map_err(|e| format!("Entity facet query failed: {}", e))?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(|e| format!("Entity facet row read failed: {}", e))?);
            }
            Ok(results)
        };

        Ok(FilterFacets {
            tags: collect(
                "SELECT t.name, COUNT(dt.download_id) AS count
                 FROM tags t
                 JOIN download_tags dt ON dt.tag_id = t.id
                 GROUP BY t.id, t.name
                 ORDER BY count DESC, t.name ASC
                 LIMIT 500",
            )?,
            authors: collect(
                "SELECT author_name, COUNT(*) AS count
                 FROM downloads
                 WHERE author_name IS NOT NULL AND author_name != ''
                 GROUP BY author_name
                 ORDER BY count DESC, author_name ASC
                 LIMIT 500",
            )?,
            author_entities: if !include_entities { Vec::new() } else { collect_entities(
                "SELECT
                    d.source,
                    d.author_id,
                    COALESCE(p.display_name, d.author_name) AS display_name,
                    COUNT(DISTINCT d.id) AS count,
                    COALESCE(p.icon_path, p.cover_path) AS cover_path,
                    p.description,
                    p.updated_at,
                    MAX(d.downloaded_at) AS latest_downloaded_at,
                    (
                        SELECT d2.title
                        FROM downloads d2
                        WHERE d2.source = d.source AND d2.author_id = d.author_id
                        ORDER BY COALESCE(d2.source_created_at, d2.downloaded_at) DESC, d2.id DESC
                        LIMIT 1
                    ) AS sample_title,
                    p.icon_path,
                    p.cover_path AS banner_path
                 FROM downloads d
                 LEFT JOIN people p ON p.source = d.source AND p.source_key = d.author_id
                 WHERE d.author_id IS NOT NULL AND d.author_id != ''
                   AND d.author_name IS NOT NULL AND d.author_name != ''
                 GROUP BY d.source, d.author_id, COALESCE(p.display_name, d.author_name), p.icon_path, p.cover_path, p.description, p.updated_at
                 ORDER BY count DESC, display_name ASC
                 LIMIT 60",
            )? },
            series: if !include_entities { Vec::new() } else { collect_entities(
                "SELECT
                    ds.series_source,
                    ds.series_key,
                    COALESCE(s.title, ds.title) AS title,
                    COUNT(DISTINCT ds.download_id) AS count,
                    s.cover_path,
                    s.description,
                    s.updated_at,
                    MAX(d.downloaded_at) AS latest_downloaded_at,
                    (
                        SELECT d2.title
                        FROM downloads d2
                        JOIN download_series ds2 ON ds2.download_id = d2.id
                        WHERE ds2.series_source = ds.series_source AND ds2.series_key = ds.series_key
                        ORDER BY COALESCE(ds2.content_order, 9223372036854775807) ASC,
                                 COALESCE(d2.source_created_at, d2.downloaded_at) ASC,
                                 d2.id ASC
                        LIMIT 1
                    ) AS sample_title,
                    NULL AS icon_path,
                    s.cover_path AS banner_path
                 FROM download_series ds
                 LEFT JOIN series s ON s.source = ds.series_source AND s.source_key = ds.series_key
                 JOIN downloads d ON d.id = ds.download_id
                 GROUP BY ds.series_source, ds.series_key, COALESCE(s.title, ds.title), s.cover_path, s.description, s.updated_at
                 ORDER BY count DESC, title ASC
                 LIMIT 60",
            )? },
            content_types: collect(
                "SELECT content_type, COUNT(*) AS count
                 FROM downloads
                 WHERE content_type IS NOT NULL AND content_type != ''
                 GROUP BY content_type
                 ORDER BY count DESC, content_type ASC",
            )?,
            asset_types: collect(
                "SELECT asset_type, COUNT(*) AS count
                 FROM assets
                 WHERE asset_type IS NOT NULL AND asset_type != ''
                 GROUP BY asset_type
                 ORDER BY count DESC, asset_type ASC",
            )?,
        })
    }

    /// バージョン履歴を挿入
    pub fn insert_version(&self, ver: &NewVersion) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO download_versions (
                download_id, version, content_hash, text_length, json_path,
                original_json_path, asset_count, file_size_bytes, created_at, change_summary
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                ver.download_id,
                ver.version,
                ver.content_hash,
                ver.text_length,
                ver.json_path,
                ver.original_json_path,
                ver.asset_count,
                ver.file_size_bytes,
                ver.created_at,
                ver.change_summary,
            ],
        )
        .map_err(|e| format!("Insert version failed: {}", e))?;
        Ok(conn.last_insert_rowid())
    }

    /// ダウンロードの全バージョン履歴取得
    pub fn get_versions(&self, download_id: i64) -> Result<Vec<DownloadVersion>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM download_versions WHERE download_id = ?1 ORDER BY version DESC")
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(DownloadVersion {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    version: row.get(2)?,
                    content_hash: row.get(3)?,
                    text_length: row.get(4)?,
                    json_path: row.get(5)?,
                    original_json_path: row.get(6)?,
                    asset_count: row.get(7)?,
                    file_size_bytes: row.get(8)?,
                    created_at: row.get(9)?,
                    change_summary: row.get(10)?,
                })
            })
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    /// 特定のバージョン履歴取得
    pub fn get_version(&self, download_id: i64, version: i64) -> Result<DownloadVersion, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM download_versions WHERE download_id = ?1 AND version = ?2",
            params![download_id, version],
            |row| {
                Ok(DownloadVersion {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    version: row.get(2)?,
                    content_hash: row.get(3)?,
                    text_length: row.get(4)?,
                    json_path: row.get(5)?,
                    original_json_path: row.get(6)?,
                    asset_count: row.get(7)?,
                    file_size_bytes: row.get(8)?,
                    created_at: row.get(9)?,
                    change_summary: row.get(10)?,
                })
            },
        )
        .map_err(|e| format!("Version not found: {}", e))
    }

    /// 指定バージョンを削除する。最新バージョンを削除した場合は、直前のバージョンを現行に戻す。
    pub fn delete_version(&self, download_id: i64, version: i64) -> Result<(), String> {
        let (version_dir, source_dir_to_cleanup) = {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction begin failed: {}", e))?;

            let version_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM download_versions WHERE download_id = ?1",
                    params![download_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to count versions: {}", e))?;
            if version_count <= 1 {
                return Err(
                    "最後のバージョンは削除できません。作品全体の削除を使用してください。"
                        .to_string(),
                );
            }

            let current_version: i64 = tx
                .query_row(
                    "SELECT current_version FROM downloads WHERE id = ?1",
                    params![download_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Download not found: {}", e))?;

            let target: DownloadVersion = tx
                .query_row(
                    "SELECT * FROM download_versions WHERE download_id = ?1 AND version = ?2",
                    params![download_id, version],
                    |row| {
                        Ok(DownloadVersion {
                            id: row.get(0)?,
                            download_id: row.get(1)?,
                            version: row.get(2)?,
                            content_hash: row.get(3)?,
                            text_length: row.get(4)?,
                            json_path: row.get(5)?,
                            original_json_path: row.get(6)?,
                            asset_count: row.get(7)?,
                            file_size_bytes: row.get(8)?,
                            created_at: row.get(9)?,
                            change_summary: row.get(10)?,
                        })
                    },
                )
                .map_err(|e| format!("Version not found: {}", e))?;

            let replacement = if version == current_version {
                Some(
                    tx.query_row(
                        "SELECT * FROM download_versions
                         WHERE download_id = ?1 AND version != ?2
                         ORDER BY version DESC
                         LIMIT 1",
                        params![download_id, version],
                        |row| {
                            Ok(DownloadVersion {
                                id: row.get(0)?,
                                download_id: row.get(1)?,
                                version: row.get(2)?,
                                content_hash: row.get(3)?,
                                text_length: row.get(4)?,
                                json_path: row.get(5)?,
                                original_json_path: row.get(6)?,
                                asset_count: row.get(7)?,
                                file_size_bytes: row.get(8)?,
                                created_at: row.get(9)?,
                                change_summary: row.get(10)?,
                            })
                        },
                    )
                    .map_err(|e| format!("Replacement version not found: {}", e))?,
                )
            } else {
                None
            };

            let version_dir = Path::new(&target.json_path)
                .parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| "Version directory could not be resolved".to_string())?;
            let deleted_prefix = format!("{}%", version_dir.to_string_lossy());

            tx.execute(
                "DELETE FROM assets WHERE download_id = ?1 AND local_path LIKE ?2",
                params![download_id, deleted_prefix],
            )
            .map_err(|e| format!("Failed to delete version assets: {}", e))?;

            tx.execute(
                "DELETE FROM download_versions WHERE download_id = ?1 AND version = ?2",
                params![download_id, version],
            )
            .map_err(|e| format!("Failed to delete version: {}", e))?;

            if let Some(repl) = replacement {
                let repl_dir = Path::new(&repl.json_path)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .ok_or_else(|| {
                        "Replacement version directory could not be resolved".to_string()
                    })?;
                let repl_prefix = format!("{}%", repl_dir.to_string_lossy());
                let cover_path: Option<String> = tx
                    .query_row(
                        "SELECT local_path FROM assets
                         WHERE download_id = ?1 AND local_path LIKE ?2 AND mime_type LIKE 'image/%'
                         ORDER BY id ASC LIMIT 1",
                        params![download_id, repl_prefix],
                        |row| row.get(0),
                    )
                    .ok();

                tx.execute(
                    "UPDATE downloads SET
                        json_path = ?1,
                        original_json_path = ?2,
                        cover_path = ?3,
                        asset_count = ?4,
                        file_size_bytes = ?5,
                        downloaded_at = ?6,
                        content_hash = ?7,
                        text_length = ?8,
                        current_version = ?9
                     WHERE id = ?10",
                    params![
                        repl.json_path,
                        repl.original_json_path,
                        cover_path,
                        repl.asset_count,
                        repl.file_size_bytes,
                        repl.created_at,
                        repl.content_hash,
                        repl.text_length,
                        repl.version,
                        download_id,
                    ],
                )
                .map_err(|e| format!("Failed to promote replacement version: {}", e))?;
            }

            tx.commit()
                .map_err(|e| format!("Transaction commit failed: {}", e))?;

            let source_dir_to_cleanup = version_dir
                .parent()
                .and_then(|work_dir| work_dir.parent())
                .map(|p| p.to_path_buf());
            (version_dir, source_dir_to_cleanup)
        };

        if version_dir.exists() {
            let canon_storage = self
                .storage_dir
                .canonicalize()
                .map_err(|e| format!("Storage path resolution failed: {}", e))?;
            let canon_version_dir = version_dir
                .canonicalize()
                .map_err(|e| format!("Version path resolution failed: {}", e))?;
            if !canon_version_dir.starts_with(&canon_storage) {
                return Err(
                    "Access Denied: Version path is outside of storage directory".to_string(),
                );
            }
            if let Err(e) = remove_dir_all_resilient(&canon_version_dir) {
                log::warn!(
                    "Failed to remove version directory {:?}: {}",
                    canon_version_dir,
                    e
                );
            }
        }

        if let Some(source_dir) = source_dir_to_cleanup {
            let _ = remove_dir_resilient(&source_dir);
        }

        if let Err(e) = self.reindex_download(download_id) {
            log::warn!(
                "Failed to refresh search index after deleting version {} of download {}: {}",
                version,
                download_id,
                e
            );
        }

        Ok(())
    }

    /// 更新監視（トグル）を設定する
    pub fn set_watch_updates(&self, download_id: i64, watch: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET watch_updates = ?1 WHERE id = ?2",
            params![if watch { 1i64 } else { 0i64 }, download_id],
        )
        .map_err(|e| format!("Failed to set watch_updates: {}", e))?;
        if let Ok((source, source_id, title)) = conn.query_row(
            "SELECT source, source_id, title FROM downloads WHERE id = ?1",
            params![download_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        ) {
            conn.execute(
                "INSERT INTO update_targets (
                    target_type, source, source_key, display_name, enabled, created_at, updated_at
                ) VALUES ('work', ?1, ?2, ?3, ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                    display_name = excluded.display_name,
                    enabled = excluded.enabled,
                    updated_at = CURRENT_TIMESTAMP",
                params![source, source_id, title, if watch { 1i64 } else { 0i64 }],
            )
            .map_err(|e| format!("Failed to sync work update target: {}", e))?;
        }
        Ok(())
    }

    pub fn set_watch_updates_for_search(
        &self,
        params: &SearchV2Params,
        watch: bool,
    ) -> Result<BulkMutationResult, String> {
        let mut bulk_params = params.clone();
        bulk_params.cursor = None;
        bulk_params.limit = Some(200_000);
        bulk_params.projection = Some("bulk".to_string());
        let result = self.search_downloads_v2_inner(&bulk_params, 200_000)?;
        let ids = result.items.iter().map(|item| item.id).collect::<Vec<_>>();
        let changed_count = self.set_watch_updates_for_ids(&ids, watch)?;

        Ok(BulkMutationResult {
            matched_count: ids.len() as i64,
            changed_count,
        })
    }

    pub fn delete_downloads_for_search(
        &self,
        params: &SearchV2Params,
    ) -> Result<BulkMutationResult, String> {
        let mut bulk_params = params.clone();
        bulk_params.cursor = None;
        bulk_params.limit = Some(200_000);
        bulk_params.projection = Some("bulk".to_string());
        let result = self.search_downloads_v2_inner(&bulk_params, 200_000)?;
        let ids = result.items.iter().map(|item| item.id).collect::<Vec<_>>();
        self.delete_downloads(&ids)
    }

    fn set_watch_updates_for_ids(&self, ids: &[i64], watch: bool) -> Result<i64, String> {
        if ids.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Bulk watch update transaction failed: {}", e))?;
        let mut changed_count = 0i64;
        let watch_value = if watch { 1i64 } else { 0i64 };

        for id in ids {
            let updated = tx
                .execute(
                    "UPDATE downloads SET watch_updates = ?1 WHERE id = ?2",
                    params![watch_value, id],
                )
                .map_err(|e| format!("Failed to set watch_updates: {}", e))?;
            if updated == 0 {
                continue;
            }
            changed_count += updated as i64;

            if let Ok((source, source_id, title)) = tx.query_row(
                "SELECT source, source_id, title FROM downloads WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            ) {
                tx.execute(
                    "INSERT INTO update_targets (
                        target_type, source, source_key, display_name, enabled, created_at, updated_at
                    ) VALUES ('work', ?1, ?2, ?3, ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                    ON CONFLICT(target_type, source, source_key) DO UPDATE SET
                        display_name = excluded.display_name,
                        enabled = excluded.enabled,
                        updated_at = CURRENT_TIMESTAMP",
                    params![source, source_id, title, watch_value],
                )
                .map_err(|e| format!("Failed to sync work update target: {}", e))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Bulk watch update commit failed: {}", e))?;
        Ok(changed_count)
    }

    /// お気に入り（トグル）を設定する
    pub fn set_favorite(&self, download_id: i64, favorite: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE downloads SET favorite = ?1 WHERE id = ?2",
            params![if favorite { 1i64 } else { 0i64 }, download_id],
        )
        .map_err(|e| format!("Failed to set favorite: {}", e))?;
        Ok(())
    }

    /// 監視対象（watch_updates = 1）のダウンロード作品一覧を取得
    pub fn get_watched_downloads(&self) -> Result<Vec<DownloadEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sql = format!(
            "{} WHERE d.watch_updates = 1 ORDER BY d.downloaded_at DESC",
            download_select_sql_for_projection(Some("libraryCompact"), "NULL", "NULL")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;
        let rows = stmt
            .query_map([], download_entry_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        Ok(results)
    }

    /// ソースとソースIDからダウンロードエントリを取得（存在しない場合はOk(None)）
    pub fn get_download_by_source(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<DownloadEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sql = format!(
            "{} WHERE d.source = ?1 AND d.source_id = ?2",
            download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}", e))?;

        let mut rows = stmt
            .query_map(params![source, source_id], download_entry_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;

        if let Some(row) = rows.next() {
            let entry = row.map_err(|e| format!("Row read failed: {}", e))?;
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    pub fn reconstruct_entities_after_import(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "
            DELETE FROM download_people;
            DELETE FROM download_series;
            DELETE FROM people;
            DELETE FROM series;

            INSERT OR IGNORE INTO people (
                source, source_key, display_name, current_version, created_at, updated_at
            )
            SELECT DISTINCT source, author_id, author_name, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
            FROM downloads
            WHERE author_id IS NOT NULL AND author_id != '';

            INSERT OR IGNORE INTO download_people (
                download_id, person_source, person_key, role, display_name
            )
            SELECT id, source, author_id, CASE WHEN source = 'fanbox' THEN 'creator' ELSE 'author' END, author_name
            FROM downloads
            WHERE author_id IS NOT NULL AND author_id != '';

            INSERT OR IGNORE INTO series (
                source, source_key, title, current_version, created_at, updated_at
            )
            SELECT DISTINCT source, relation_id, relation_name, 0, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
            FROM download_relations
            WHERE relation_type = 'series' AND relation_id IS NOT NULL AND relation_id != '';

            INSERT OR IGNORE INTO download_series (
                download_id, series_source, series_key, title
            )
            SELECT download_id, source, relation_id, relation_name
            FROM download_relations
            WHERE relation_type = 'series' AND relation_id IS NOT NULL AND relation_id != '';
            "
        ).map_err(|e| format!("Reconstructing entities failed: {}", e))?;

        Ok(())
    }
}

fn query_text(params: &SearchV2Params) -> &str {
    params
        .query
        .as_deref()
        .or(params.text.as_deref())
        .unwrap_or("")
        .trim()
}

fn effective_search_mode(params: &SearchV2Params) -> String {
    match params.search_mode.as_deref() {
        Some("exact") => "exact",
        Some("semantic") => "semantic",
        _ => "smart",
    }
    .to_string()
}

fn search_candidate_limit(params: &SearchV2Params, page_limit: i64) -> usize {
    let page_limit = page_limit.max(1) as usize;
    let (multiplier, floor) = if params_have_library_filters(params) {
        (4usize, 400usize)
    } else {
        (2usize, 160usize)
    };
    page_limit.saturating_mul(multiplier).max(floor).min(1_000)
}

fn params_have_library_filters(params: &SearchV2Params) -> bool {
    params
        .source
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || params
            .content_type
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params.favorite == Some(true)
        || params
            .tags_include
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params
            .tags_exclude
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params
            .authors_include
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params
            .authors_exclude
            .as_ref()
            .map(|values| values.iter().any(|value| !value.trim().is_empty()))
            .unwrap_or(false)
        || params.min_char_count.is_some()
        || params.max_char_count.is_some()
        || params
            .asset_filter
            .as_deref()
            .map(|value| !value.trim().is_empty() && value != "all")
            .unwrap_or(false)
        || params
            .watch_filter
            .as_deref()
            .map(|value| !value.trim().is_empty() && value != "all")
            .unwrap_or(false)
        || params
            .person_source
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params
            .person_key
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params
            .series_source
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || params
            .series_key
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
}

fn blend_search_hits(
    lexical_hits: &[super::tantivy_index::TantivySearchHit],
    semantic_hits: &[super::semantic_index::SemanticSearchHit],
    search_mode: &str,
) -> Vec<RankedSearchHit> {
    let mut merged: HashMap<i64, RankedSearchHit> = HashMap::new();
    if search_mode != "semantic" {
        for (idx, hit) in lexical_hits.iter().enumerate() {
            let rrf = 1.0 / (60.0 + (idx + 1) as f64);
            let score = (hit.score as f64).min(80.0) + rrf * 900.0;
            let entry = merged.entry(hit.download_id).or_insert(RankedSearchHit {
                download_id: hit.download_id,
                score: 0.0,
                semantic: None,
            });
            entry.score += score;
        }
    }
    if search_mode != "exact" {
        for (idx, hit) in semantic_hits.iter().enumerate() {
            let rrf = 1.0 / (60.0 + (idx + 1) as f64);
            let score = hit.score.max(0.0) * 120.0 + rrf * 900.0;
            let entry = merged.entry(hit.download_id).or_insert(RankedSearchHit {
                download_id: hit.download_id,
                score: 0.0,
                semantic: None,
            });
            entry.score += score;
            if entry
                .semantic
                .as_ref()
                .map(|existing| existing.score < hit.score)
                .unwrap_or(true)
            {
                entry.semantic = Some(hit.clone());
            }
        }
    }
    let mut hits = merged.into_values().collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.download_id.cmp(&b.download_id))
    });
    hits
}

fn normalize_search_params(params: &SearchV2Params) -> SearchV2Params {
    let mut normalized = params.clone();
    normalized.projection = Some(normalized_projection(
        normalized.projection.as_deref(),
        normalized.view_mode.as_deref(),
    ));
    let query = query_text(&normalized).to_string();
    if query.is_empty() {
        return normalized;
    }

    let (clean_query, source_id_terms) = extract_structured_search_filters(&query, &mut normalized);
    let mut query_parts = Vec::new();
    if !clean_query.trim().is_empty() {
        query_parts.push(clean_query.trim().to_string());
    }
    query_parts.extend(source_id_terms);
    let next_query = query_parts.join(" ");
    normalized.query = if next_query.is_empty() {
        None
    } else {
        Some(next_query)
    };
    normalized.text = None;
    normalized
}

fn normalized_projection(projection: Option<&str>, view_mode: Option<&str>) -> String {
    match projection {
        Some("libraryGallery") | Some("library") => "libraryGallery".to_string(),
        Some("libraryCompact") => "libraryCompact".to_string(),
        Some("bulk") => "bulk".to_string(),
        Some("entityFacet") => "entityFacet".to_string(),
        Some("minimal") => "bulk".to_string(),
        _ => match view_mode {
            Some("compact") | Some("epubSelection") | Some("updateReview") => {
                "libraryCompact".to_string()
            }
            _ => "libraryGallery".to_string(),
        },
    }
}

fn extract_structured_search_filters(
    query: &str,
    params: &mut SearchV2Params,
) -> (String, Vec<String>) {
    let mut clean_terms = Vec::new();
    let mut source_id_terms = Vec::new();

    for raw_term in split_query_terms(query) {
        let excluded = raw_term.starts_with('-');
        let term = raw_term.trim_start_matches('-');
        let Some((field, raw_value)) = term.split_once(':') else {
            if let Some(id) = source_id_from_url(term) {
                source_id_terms.push(id);
            } else {
                clean_terms.push(raw_term);
            }
            continue;
        };

        let field = field.to_ascii_lowercase();
        let value = raw_value.trim_matches('"').trim();
        if value.is_empty() {
            continue;
        }

        match field.as_str() {
            "tag" | "tags" => {
                if excluded {
                    push_unique(&mut params.tags_exclude, value.to_string());
                } else {
                    push_unique(&mut params.tags_include, value.to_string());
                }
            }
            "author" | "creator" => {
                if excluded {
                    push_unique(&mut params.authors_exclude, value.to_string());
                } else {
                    push_unique(&mut params.authors_include, value.to_string());
                }
            }
            "series" | "series_key" | "series_id" if !excluded => {
                if let Some((source, key)) = value.split_once(':') {
                    let source = source.trim();
                    let key = key.trim();
                    if !source.is_empty() && !key.is_empty() {
                        params.series_source = Some(source.to_ascii_lowercase());
                        params.series_key = Some(key.to_string());
                    }
                } else {
                    params.series_source = None;
                    params.series_key = Some(value.to_string());
                }
            }
            "series_title" | "title" => {
                clean_terms.push(if excluded {
                    format!("-{}", value)
                } else {
                    value.to_string()
                });
            }
            "source" if !excluded => {
                params.source = Some(value.to_ascii_lowercase());
            }
            "id" | "source_id" | "sourceid" | "url" if !excluded => {
                if let Some(id) = source_id_from_url(value) {
                    source_id_terms.push(id);
                } else {
                    source_id_terms.push(value.to_string());
                }
            }
            _ => clean_terms.push(raw_term),
        }
    }

    (clean_terms.join(" "), source_id_terms)
}

fn split_query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut chars = query.chars().peekable();
    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut term = String::new();
        let mut in_quote = false;
        for c in chars.by_ref() {
            if c == '"' {
                in_quote = !in_quote;
                term.push(c);
                continue;
            }
            if !in_quote && c.is_whitespace() {
                break;
            }
            term.push(c);
        }
        if !term.trim().is_empty() {
            terms.push(term);
        }
    }
    terms
}

fn source_id_from_url(value: &str) -> Option<String> {
    let pixiv = regex::Regex::new(r"(?:novel/show\.php\?id=|/novels/|/artworks/)(\d+)").ok()?;
    if let Some(caps) = pixiv.captures(value) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    let fanbox = regex::Regex::new(r"fanbox\.cc/(?:@[^/]+/)?posts/(\d+)").ok()?;
    fanbox
        .captures(value)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
}

fn source_url_for_download(source: &str, source_id: &str, author_id: &str) -> String {
    match source {
        "pixiv" => format!("https://www.pixiv.net/novel/show.php?id={}", source_id),
        "fanbox" if !author_id.is_empty() => {
            format!("https://{}.fanbox.cc/posts/{}", author_id, source_id)
        }
        "fanbox" => format!("https://www.fanbox.cc/posts/{}", source_id),
        _ => source_id.to_string(),
    }
}

fn push_unique(slot: &mut Option<Vec<String>>, value: String) {
    let values = slot.get_or_insert_with(Vec::new);
    if !values.iter().any(|item| item == &value) {
        values.push(value);
    }
}

fn encode_cursor(cursor: &SearchCursor) -> Option<String> {
    serde_json::to_vec(cursor)
        .ok()
        .map(|bytes| format!("k:{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn decode_cursor(raw: Option<&str>) -> Option<SearchCursor> {
    let raw = raw?;
    let encoded = raw.strip_prefix("k:")?;
    let bytes = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn effective_sort_by(params: &SearchV2Params) -> Option<String> {
    Some(
        match params.sort_by.as_deref() {
            Some("title") => "title",
            Some("author") => "author",
            Some("date") => "date",
            Some("published") => "published",
            Some("series_order") => "series_order",
            Some("size") => "size",
            _ => "date",
        }
        .to_string(),
    )
}

fn effective_sort_order(params: &SearchV2Params) -> Option<String> {
    Some(
        match params.sort_order.as_deref() {
            Some("asc") => "asc",
            _ => "desc",
        }
        .to_string(),
    )
}

fn encode_sql_cursor(params: &SearchV2Params, item: &DownloadEntry) -> Option<String> {
    encode_cursor(&SearchCursor {
        kind: "sql".to_string(),
        sort_by: effective_sort_by(params),
        sort_order: effective_sort_order(params),
        value: item
            .sort_key
            .clone()
            .or_else(|| fallback_sort_value(params, item)),
        id: Some(item.id),
        score: None,
        downloaded_at: None,
    })
}

fn encode_search_cursor(item: &DownloadEntry) -> Option<String> {
    encode_cursor(&SearchCursor {
        kind: "search".to_string(),
        sort_by: Some("relevance".to_string()),
        sort_order: Some("desc".to_string()),
        value: None,
        id: Some(item.id),
        score: item.search_score,
        downloaded_at: Some(item.downloaded_at.clone()),
    })
}

fn search_item_is_after_cursor(item: &DownloadEntry, cursor: &SearchCursor) -> bool {
    let cursor_score = cursor.score.unwrap_or(0.0);
    let item_score = item.search_score.unwrap_or(0.0);
    if item_score < cursor_score {
        return true;
    }
    if item_score > cursor_score {
        return false;
    }
    let cursor_date = cursor.downloaded_at.as_deref().unwrap_or("");
    if item.downloaded_at.as_str() < cursor_date {
        return true;
    }
    if item.downloaded_at.as_str() > cursor_date {
        return false;
    }
    item.id < cursor.id.unwrap_or(i64::MIN)
}

fn fallback_sort_value(params: &SearchV2Params, item: &DownloadEntry) -> Option<String> {
    Some(match effective_sort_by(params).as_deref() {
        Some("title") => item.title.clone(),
        Some("author") => item.author_name.clone(),
        Some("published") => item
            .source_created_at
            .clone()
            .unwrap_or_else(|| item.downloaded_at.clone()),
        Some("size") => item.file_size_bytes.to_string(),
        _ => item.downloaded_at.clone(),
    })
}

fn download_select_sql_for_projection(
    projection: Option<&str>,
    search_score_expr: &str,
    sort_key_expr: &str,
) -> String {
    let projection = normalized_projection(projection, None);
    let tags_expr = "COALESCE((
        SELECT json_group_array(name)
        FROM (
            SELECT t.name AS name
            FROM download_tags dt
            JOIN tags t ON t.id = dt.tag_id
            WHERE dt.download_id = d.id
            ORDER BY t.name
        )
    ), '[]')";
    let person_id_expr =
        "(SELECT dp.person_key FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1)";
    let person_name_expr =
        "(SELECT dp.display_name FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1)";
    let series_id_expr =
        "(SELECT ds.series_key FROM download_series ds WHERE ds.download_id = d.id LIMIT 1)";
    let series_title_expr =
        "(SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1)";
    // Cards show the creator's avatar beside the author name. Keyed on
    // author_id, the same join the author listing uses.
    let person_icon_expr =
        "(SELECT p.icon_path FROM people p WHERE p.source = d.source AND p.source_key = d.author_id)";

    // Only the card projections need the avatar; bulk reads skip the lookup.
    let person_icon = match projection.as_str() {
        "libraryGallery" | "libraryCompact" => person_icon_expr,
        _ => "NULL",
    };
    let (core_columns, person_id, person_name, series_id, series_title) = match projection.as_str()
    {
        "bulk" => (
            "d.id,
                d.source,
                d.source_id,
                d.title,
                '' AS author_name,
                '' AS author_id,
                d.content_type,
                '[]' AS tags,
                NULL AS excerpt,
                NULL AS cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
                .to_string(),
            "NULL",
            "NULL",
            "NULL",
            "NULL",
        ),
        "entityFacet" => (
            "d.id,
                d.source,
                d.source_id,
                d.title,
                d.author_name,
                d.author_id,
                d.content_type,
                '[]' AS tags,
                NULL AS excerpt,
                d.cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
                .to_string(),
            "NULL",
            "NULL",
            "NULL",
            "NULL",
        ),
        // The list view shows tags too; only the excerpt is surplus there.
        "libraryCompact" => (
            format!(
                "d.id,
                d.source,
                d.source_id,
                d.title,
                d.author_name,
                d.author_id,
                d.content_type,
                {tags_expr} AS tags,
                NULL AS excerpt,
                d.cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
            ),
            person_id_expr,
            person_name_expr,
            series_id_expr,
            series_title_expr,
        ),
        _ => (
            format!(
                "d.id,
                d.source,
                d.source_id,
                d.title,
                d.author_name,
                d.author_id,
                d.content_type,
                {tags_expr} AS tags,
                d.excerpt,
                d.cover_path,
                d.json_path,
                d.original_json_path,
                d.asset_count,
                d.file_size_bytes,
                d.downloaded_at,
                d.source_created_at,
                d.content_hash,
                d.text_length,
                d.source_updated_at,
                d.watch_updates,
                d.current_version,
                d.favorite"
            ),
            person_id_expr,
            person_name_expr,
            series_id_expr,
            series_title_expr,
        ),
    };
    format!(
        "SELECT {core_columns},
            {} AS person_id,
            {} AS person_name,
            {} AS series_id,
            {} AS series_title,
            {} AS search_score,
            NULL AS match_fields,
            NULL AS score_reasons,
            NULL AS match_highlights,
            {} AS sort_key,
            {} AS person_icon_path
         FROM downloads d",
        person_id,
        person_name,
        series_id,
        series_title,
        search_score_expr,
        sort_key_expr,
        person_icon
    )
}

fn query_download_entries(
    conn: &Connection,
    sql: &str,
    bind_values: &[Box<dyn rusqlite::types::ToSql>],
) -> Result<Vec<DownloadEntry>, String> {
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Query prepare failed: {}\nSQL: {}", e, sql))?;
    let rows = stmt
        .query_map(refs.as_slice(), download_entry_from_row)
        .map_err(|e| format!("Query failed: {}", e))?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
    }
    Ok(results)
}

fn collect_suggestions(
    conn: &Connection,
    kind: &str,
    sql: &str,
    like: &str,
    limit: i64,
    items: &mut Vec<SearchSuggestion>,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Suggest query prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![like, limit], |row| {
            Ok(SearchSuggestion {
                kind: kind.to_string(),
                label: row.get(0)?,
                value: row.get(1)?,
                count: row.get(2)?,
            })
        })
        .map_err(|e| format!("Suggest query failed: {}", e))?;
    for row in rows {
        items.push(row.map_err(|e| format!("Suggest row read failed: {}", e))?);
    }
    Ok(())
}

fn sort_key_select_expr(params: &SearchV2Params) -> String {
    match effective_sort_by(params).as_deref() {
        Some("title") => "CAST(d.title AS TEXT)".to_string(),
        Some("author") => "CAST(d.author_name AS TEXT)".to_string(),
        Some("published") => {
            "CAST(COALESCE(d.source_created_at, d.downloaded_at) AS TEXT)".to_string()
        }
        Some("size") => "CAST(d.file_size_bytes AS TEXT)".to_string(),
        Some("series_order") => format!("CAST({} AS TEXT)", series_order_sort_expr()),
        _ => "CAST(d.downloaded_at AS TEXT)".to_string(),
    }
}

fn sort_compare_expr(params: &SearchV2Params) -> String {
    match effective_sort_by(params).as_deref() {
        Some("title") => "d.title COLLATE NOCASE".to_string(),
        Some("author") => "d.author_name COLLATE NOCASE".to_string(),
        Some("published") => "COALESCE(d.source_created_at, d.downloaded_at)".to_string(),
        Some("size") => "d.file_size_bytes".to_string(),
        Some("series_order") => series_order_sort_expr(),
        _ => "d.downloaded_at".to_string(),
    }
}

fn append_keyset_filter(
    params: &SearchV2Params,
    cursor: Option<&SearchCursor>,
    wheres: &mut Vec<String>,
    bind_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
) {
    let Some(cursor) = cursor else { return };
    let Some(value) = cursor.value.as_deref() else {
        return;
    };
    let Some(id) = cursor.id else { return };
    let expr = sort_compare_expr(params);
    let asc = effective_sort_order(params).as_deref() == Some("asc");
    let cmp = if asc { ">" } else { "<" };
    let id_cmp = if asc { ">" } else { "<" };
    wheres.push(format!(
        "({expr} {cmp} ? OR ({expr} = ? AND d.id {id_cmp} ?))"
    ));
    if effective_sort_by(params).as_deref() == Some("size") {
        let parsed = value.parse::<i64>().unwrap_or(0);
        bind_values.push(Box::new(parsed));
        bind_values.push(Box::new(parsed));
    } else {
        bind_values.push(Box::new(value.to_string()));
        bind_values.push(Box::new(value.to_string()));
    }
    bind_values.push(Box::new(id));
}

fn append_library_filters(
    params: &SearchV2Params,
    wheres: &mut Vec<String>,
    bind_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
) {
    if let Some(ref src) = params.source {
        if !src.is_empty() {
            wheres.push("d.source = ?".to_string());
            bind_values.push(Box::new(src.clone()));
        }
    }

    if let Some(ref ct) = params.content_type {
        if !ct.is_empty() {
            wheres.push("d.content_type = ?".to_string());
            bind_values.push(Box::new(ct.clone()));
        }
    }

    if params.favorite == Some(true) {
        wheres.push("d.favorite = 1".to_string());
    }

    if let Some(ref tags_inc) = params.tags_include {
        let active_tags = active_strings(tags_inc);
        if !active_tags.is_empty() {
            let placeholders = vec!["?"; active_tags.len()].join(", ");
            if params.tag_filter_mode.as_deref() == Some("or") {
                wheres.push(format!(
                    "d.id IN (
                        SELECT download_id FROM download_tags dt
                        JOIN tags t ON dt.tag_id = t.id
                        WHERE t.name IN ({})
                    )",
                    placeholders
                ));
                for tag in active_tags {
                    bind_values.push(Box::new(tag));
                }
            } else {
                wheres.push(format!(
                    "d.id IN (
                        SELECT download_id FROM download_tags dt
                        JOIN tags t ON dt.tag_id = t.id
                        WHERE t.name IN ({})
                        GROUP BY download_id
                        HAVING COUNT(DISTINCT t.name) = ?
                    )",
                    placeholders
                ));
                let count_inc = active_tags.len() as i64;
                for tag in active_tags {
                    bind_values.push(Box::new(tag));
                }
                bind_values.push(Box::new(count_inc));
            }
        }
    }

    if let Some(ref tags_exc) = params.tags_exclude {
        let active_tags = active_strings(tags_exc);
        if !active_tags.is_empty() {
            let placeholders = vec!["?"; active_tags.len()].join(", ");
            wheres.push(format!(
                "d.id NOT IN (
                    SELECT download_id FROM download_tags dt
                    JOIN tags t ON dt.tag_id = t.id
                    WHERE t.name IN ({})
                )",
                placeholders
            ));
            for tag in active_tags {
                bind_values.push(Box::new(tag));
            }
        }
    }

    if let Some(ref authors_inc) = params.authors_include {
        let active_authors = active_strings(authors_inc);
        if !active_authors.is_empty() {
            let placeholders = vec!["?"; active_authors.len()].join(", ");
            wheres.push(format!("d.author_name IN ({})", placeholders));
            for author in active_authors {
                bind_values.push(Box::new(author));
            }
        }
    }

    if let Some(ref authors_exc) = params.authors_exclude {
        let active_authors = active_strings(authors_exc);
        if !active_authors.is_empty() {
            let placeholders = vec!["?"; active_authors.len()].join(", ");
            wheres.push(format!("d.author_name NOT IN ({})", placeholders));
            for author in active_authors {
                bind_values.push(Box::new(author));
            }
        }
    }

    if let Some(min_char) = params.min_char_count {
        wheres.push("d.text_length >= ?".to_string());
        bind_values.push(Box::new(min_char));
    }

    if let Some(max_char) = params.max_char_count {
        wheres.push("d.text_length <= ?".to_string());
        bind_values.push(Box::new(max_char));
    }

    if let Some(ref asset_filter) = params.asset_filter {
        match asset_filter.as_str() {
            "has_assets" => wheres.push("d.asset_count > 0".to_string()),
            "no_assets" => wheres.push("d.asset_count = 0".to_string()),
            "has_images" => wheres.push(
                "d.id IN (
                    SELECT download_id FROM assets
                    WHERE mime_type LIKE 'image/%'
                )"
                .to_string(),
            ),
            "has_files" => wheres.push(
                "d.id IN (
                    SELECT download_id FROM assets
                    WHERE mime_type IS NULL OR mime_type NOT LIKE 'image/%'
                )"
                .to_string(),
            ),
            "has_images_and_files" => {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM assets
                        WHERE mime_type LIKE 'image/%'
                    )"
                    .to_string(),
                );
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM assets
                        WHERE mime_type IS NULL OR mime_type NOT LIKE 'image/%'
                    )"
                    .to_string(),
                );
            }
            _ => {}
        }
    }

    if let Some(ref watch_filter) = params.watch_filter {
        match watch_filter.as_str() {
            "watched" => wheres.push("d.watch_updates = 1".to_string()),
            "unwatched" => wheres.push("d.watch_updates = 0".to_string()),
            _ => {}
        }
    }

    if let (Some(person_source), Some(person_key)) = (&params.person_source, &params.person_key) {
        if !person_source.trim().is_empty() && !person_key.trim().is_empty() {
            wheres.push(
                "d.id IN (
                    SELECT download_id FROM download_people
                    WHERE person_source = ? AND person_key = ?
                )"
                .to_string(),
            );
            bind_values.push(Box::new(person_source.clone()));
            bind_values.push(Box::new(person_key.clone()));
        }
    }

    if let Some(series_key) = &params.series_key {
        let series_key = series_key.trim();
        if !series_key.is_empty() {
            if let Some(series_source) = params
                .series_source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM download_series
                        WHERE series_source = ? AND series_key = ?
                    )"
                    .to_string(),
                );
                bind_values.push(Box::new(series_source.to_string()));
                bind_values.push(Box::new(series_key.to_string()));
            } else {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM download_series
                        WHERE series_key = ? OR title = ?
                    )"
                    .to_string(),
                );
                bind_values.push(Box::new(series_key.to_string()));
                bind_values.push(Box::new(series_key.to_string()));
            }
        }
    }
}

fn active_strings(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn normalized_tag_list(values: &[String]) -> Vec<String> {
    let mut tags = active_strings(values);
    tags.sort();
    tags.dedup();
    tags
}

fn sort_clause(params: &SearchV2Params) -> String {
    let sort_col = match params.sort_by.as_deref() {
        Some("title") => "d.title COLLATE NOCASE",
        Some("author") => "d.author_name COLLATE NOCASE",
        Some("date") => "d.downloaded_at",
        Some("published") => "COALESCE(d.source_created_at, d.downloaded_at)",
        Some("series_order") => {
            return format!(
                " ORDER BY {} {}, d.id {}",
                series_order_sort_expr(),
                sort_order(params),
                sort_order(params)
            )
        }
        Some("size") => "d.file_size_bytes",
        _ => "d.downloaded_at",
    };
    let sort_order = sort_order(params);
    format!(" ORDER BY {} {}, d.id {}", sort_col, sort_order, sort_order)
}

fn sort_order(params: &SearchV2Params) -> &'static str {
    match params.sort_order.as_deref() {
        Some("asc") => "ASC",
        _ => "DESC",
    }
}

fn series_order_sort_expr() -> String {
    "printf('%020lld|%s',
        (SELECT COALESCE(MIN(ds.content_order), 9223372036854775807) FROM download_series ds WHERE ds.download_id = d.id),
        COALESCE(d.source_created_at, d.downloaded_at)
    )"
    .to_string()
}

fn get_work_edit_revision_locked(
    conn: &Connection,
    revision_id: i64,
) -> Result<WorkEditRevision, String> {
    conn.query_row(
        "SELECT id, download_id, base_version, status, title, content_hash, created_at, updated_at
         FROM work_edit_revisions
         WHERE id = ?1",
        params![revision_id],
        work_edit_revision_from_row,
    )
    .map_err(|e| format!("Work edit revision not found: {}", e))
}

fn active_edit_revision_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<WorkEditRevision>, String> {
    conn.query_row(
        "SELECT id, download_id, base_version, status, title, content_hash, created_at, updated_at
         FROM work_edit_revisions
         WHERE download_id = ?1 AND status = 'active'
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![download_id],
        work_edit_revision_from_row,
    )
    .optional()
    .map_err(|e| format!("Failed to load active edit revision: {}", e))
}

fn draft_edit_revision_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<WorkEditRevision>, String> {
    conn.query_row(
        "SELECT id, download_id, base_version, status, title, content_hash, created_at, updated_at
         FROM work_edit_revisions
         WHERE download_id = ?1 AND status = 'draft'
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![download_id],
        work_edit_revision_from_row,
    )
    .optional()
    .map_err(|e| format!("Failed to load draft edit revision: {}", e))
}

fn blocks_for_revision_locked(
    conn: &Connection,
    revision_id: i64,
) -> Result<Vec<WorkBlock>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, edit_revision_id, block_order, block_type, text, asset_id, attrs_json
             FROM work_edit_blocks
             WHERE edit_revision_id = ?1
             ORDER BY block_order ASC",
        )
        .map_err(|e| format!("Work block query prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![revision_id], work_block_from_row)
        .map_err(|e| format!("Work block query failed: {}", e))?;
    let mut blocks = Vec::new();
    for row in rows {
        blocks.push(row.map_err(|e| format!("Work block row failed: {}", e))?);
    }
    Ok(blocks)
}

fn insert_work_blocks_locked(
    tx: &Transaction<'_>,
    revision_id: i64,
    blocks: &[WorkBlockInput],
) -> Result<(), String> {
    for (idx, block) in blocks.iter().enumerate() {
        tx.execute(
            "INSERT INTO work_edit_blocks (
                edit_revision_id, block_order, block_type, text, asset_id, attrs_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                revision_id,
                idx as i64,
                block.block_type,
                block.text,
                block.asset_id,
                block.attrs_json
            ],
        )
        .map_err(|e| format!("Failed to insert work block: {}", e))?;
    }
    Ok(())
}

fn normalize_block_inputs(blocks: &[WorkBlockInput]) -> Vec<WorkBlockInput> {
    let mut normalized = Vec::new();
    for block in blocks {
        let block_type = match block.block_type.as_str() {
            "heading" | "image" | "separator" | "page_break" | "quote" | "link" => {
                block.block_type.clone()
            }
            _ => "paragraph".to_string(),
        };
        let text = block
            .text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if block_type != "image"
            && text.is_none()
            && block_type != "separator"
            && block_type != "page_break"
        {
            continue;
        }
        normalized.push(WorkBlockInput {
            block_type,
            text,
            asset_id: block.asset_id,
            attrs_json: block.attrs_json.clone(),
        });
    }
    if normalized.is_empty() {
        normalized.push(WorkBlockInput {
            block_type: "paragraph".to_string(),
            text: Some(String::new()),
            asset_id: None,
            attrs_json: None,
        });
    }
    normalized
}

fn plain_text_to_editor_blocks(text: &str) -> Vec<WorkBlock> {
    let chunks = text
        .split("\n\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>();
    let chunks = if chunks.is_empty() {
        vec![text.trim()]
    } else {
        chunks
    };

    chunks
        .into_iter()
        .enumerate()
        .map(|(idx, text)| WorkBlock {
            id: 0,
            edit_revision_id: 0,
            order: idx as i64,
            block_type: if text.starts_with('#') {
                "heading".to_string()
            } else {
                "paragraph".to_string()
            },
            text: Some(text.trim_start_matches('#').trim().to_string()),
            asset_id: None,
            attrs_json: None,
        })
        .collect()
}

fn html_to_editor_blocks(html: &str, assets: &[AssetEntry]) -> Vec<WorkBlock> {
    if html.trim().is_empty() {
        return plain_text_to_editor_blocks("");
    }
    let token_re = Regex::new(
        r#"(?is)(<!--\s*newpage\s*-->|<h2\b[^>]*>.*?</h2>|<img\b[^>]*>|<hr\s*/?>|<a\b[^>]*class=\"[^\"]*novel-link-card[^\"]*\"[^>]*>.*?</a>)"#,
    )
    .expect("valid editor HTML token regex");
    let attr_re =
        Regex::new(r#"(?i)([a-z0-9_-]+)=\"([^\"]*)\""#).expect("valid editor attribute regex");
    let mut blocks = Vec::new();
    let mut cursor = 0;

    let push_text = |fragment: &str, blocks: &mut Vec<WorkBlock>| {
        let text = html_fragment_to_text(fragment);
        for chunk in text
            .split("\n\n")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            blocks.push(editor_block(
                blocks.len(),
                "paragraph",
                Some(chunk.to_string()),
                None,
                None,
            ));
        }
    };

    for matched in token_re.find_iter(html) {
        push_text(&html[cursor..matched.start()], &mut blocks);
        let token = matched.as_str();
        let lower = token.to_ascii_lowercase();
        if lower.starts_with("<!--") {
            blocks.push(editor_block(blocks.len(), "page_break", None, None, None));
        } else if lower.starts_with("<h2") {
            blocks.push(editor_block(
                blocks.len(),
                "heading",
                Some(html_fragment_to_text(token).trim().to_string()),
                None,
                None,
            ));
        } else if lower.starts_with("<img") {
            let local_path = attr_re
                .captures_iter(token)
                .find(|capture| {
                    capture
                        .get(1)
                        .map(|v| v.as_str().eq_ignore_ascii_case("data-local-path"))
                        .unwrap_or(false)
                })
                .and_then(|capture| {
                    capture
                        .get(2)
                        .map(|value| decode_editor_entities(value.as_str()))
                });
            let asset_id = local_path.as_deref().and_then(|path| {
                assets
                    .iter()
                    .find(|asset| asset.local_path == path || asset.local_path.ends_with(path))
                    .map(|asset| asset.id)
            });
            let alt = attr_re
                .captures_iter(token)
                .find(|capture| {
                    capture
                        .get(1)
                        .map(|v| v.as_str().eq_ignore_ascii_case("alt"))
                        .unwrap_or(false)
                })
                .and_then(|capture| {
                    capture
                        .get(2)
                        .map(|value| decode_editor_entities(value.as_str()))
                });
            blocks.push(editor_block(blocks.len(), "image", alt, asset_id, None));
        } else if lower.starts_with("<hr") {
            blocks.push(editor_block(blocks.len(), "separator", None, None, None));
        } else {
            let href = attr_re
                .captures_iter(token)
                .find(|capture| {
                    capture
                        .get(1)
                        .map(|v| v.as_str().eq_ignore_ascii_case("href"))
                        .unwrap_or(false)
                })
                .and_then(|capture| {
                    capture
                        .get(2)
                        .map(|value| decode_editor_entities(value.as_str()))
                })
                .unwrap_or_default();
            let label = html_fragment_to_text(token)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let attrs = serde_json::json!({ "label": label }).to_string();
            blocks.push(editor_block(
                blocks.len(),
                "link",
                Some(href),
                None,
                Some(attrs),
            ));
        }
        cursor = matched.end();
    }
    push_text(&html[cursor..], &mut blocks);
    if blocks.is_empty() {
        plain_text_to_editor_blocks("")
    } else {
        blocks
    }
}

fn editor_block(
    order: usize,
    block_type: &str,
    text: Option<String>,
    asset_id: Option<i64>,
    attrs_json: Option<String>,
) -> WorkBlock {
    WorkBlock {
        id: 0,
        edit_revision_id: 0,
        order: order as i64,
        block_type: block_type.to_string(),
        text,
        asset_id,
        attrs_json,
    }
}

fn html_fragment_to_text(fragment: &str) -> String {
    let breaks = Regex::new(r"(?i)<br\s*/?>|</(?:p|div|h[1-6]|blockquote)>")
        .expect("valid HTML break regex");
    let tags = Regex::new(r"(?is)<[^>]+>").expect("valid HTML tag regex");
    let with_breaks = breaks.replace_all(fragment, "\n");
    decode_editor_entities(tags.replace_all(&with_breaks, "").as_ref())
        .replace("\r\n", "\n")
        .replace("\n\n\n", "\n\n")
}

fn decode_editor_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

fn hash_blocks(blocks: &[WorkBlockInput]) -> String {
    let mut hasher = Sha256::new();
    for block in blocks {
        hasher.update(block.block_type.as_bytes());
        hasher.update([0]);
        if let Some(text) = &block.text {
            hasher.update(text.as_bytes());
        }
        hasher.update([0]);
        if let Some(asset_id) = block.asset_id {
            hasher.update(asset_id.to_le_bytes());
        }
        hasher.update([0]);
        if let Some(attrs) = &block.attrs_json {
            hasher.update(attrs.as_bytes());
        }
        hasher.update([0xff]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

fn blocks_to_plain_text(blocks: &[WorkBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block.block_type.as_str() {
            "image" | "separator" | "page_break" => None,
            _ => block.text.as_deref(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn blocks_to_html(blocks: &[WorkBlock], assets: &[AssetEntry]) -> String {
    blocks
        .iter()
        .map(|block| match block.block_type.as_str() {
            "heading" => format!(
                "<h2>{}</h2>",
                escape_editor_html(block.text.as_deref().unwrap_or(""))
            ),
            "image" => {
                let asset = block
                    .asset_id
                    .and_then(|id| assets.iter().find(|asset| asset.id == id));
                if let Some(asset) = asset {
                    format!(
                        r#"<img class="novel-image" data-local-path="{}" alt="{}" />"#,
                        escape_editor_html(&asset.local_path),
                        escape_editor_html(&asset.filename)
                    )
                } else {
                    "<div class=\"missing-image-placeholder\">画像が見つかりません</div>"
                        .to_string()
                }
            }
            "separator" => "<hr />".to_string(),
            "page_break" => "<!-- newpage -->".to_string(),
            "quote" => format!(
                "<blockquote>{}</blockquote>",
                escape_editor_html(block.text.as_deref().unwrap_or("")).replace('\n', "<br />\n")
            ),
            "link" => {
                let url = block.text.as_deref().unwrap_or("");
                let label = block
                    .attrs_json
                    .as_deref()
                    .and_then(|attrs| serde_json::from_str::<serde_json::Value>(attrs).ok())
                    .and_then(|attrs| attrs.get("label").and_then(|value| value.as_str()).map(str::to_string))
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or_else(|| url.to_string());
                format!(
                    r#"<a href="{}" target="_blank" rel="noopener noreferrer" class="novel-link-card"><span class="link-card-icon">🔗</span><span class="link-card-info"><span class="link-card-title">{}</span><span class="link-card-host">{}</span></span></a>"#,
                    escape_editor_html(url),
                    escape_editor_html(&label),
                    escape_editor_html(url)
                )
            }
            _ => escape_editor_html(block.text.as_deref().unwrap_or("")).replace('\n', "<br />\n"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn active_edit_plain_text_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<String>, String> {
    let Some(revision) = active_edit_revision_locked(conn, download_id)? else {
        return Ok(None);
    };
    let blocks = blocks_for_revision_locked(conn, revision.id)?;
    Ok(Some(blocks_to_plain_text(&blocks)))
}

fn escape_editor_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn stale_search_index_ids_locked(conn: &Connection, limit: i64) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id
             FROM downloads d
             LEFT JOIN search_index_state m ON m.download_id = d.id
             WHERE m.download_id IS NULL
                OR m.current_version != d.current_version
                OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')
             ORDER BY d.id ASC
             LIMIT ?1",
        )
        .map_err(|e| format!("Search index stale query prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![limit], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("Search index stale query failed: {}", e))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| format!("Search index stale row read failed: {}", e))?);
    }
    Ok(ids)
}

fn search_index_status_locked(
    conn: &Connection,
    storage_dir: &Path,
) -> Result<SearchIndexStatus, String> {
    let total_downloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .map_err(|e| format!("Search index status total failed: {}", e))?;
    let indexed_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM downloads d
             JOIN search_index_state m ON m.download_id = d.id
             WHERE m.current_version = d.current_version
               AND COALESCE(m.content_hash, '') = COALESCE(d.content_hash, '')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Search index status indexed failed: {}", e))?;
    let pending_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM downloads d
             LEFT JOIN search_index_state m ON m.download_id = d.id
             WHERE m.download_id IS NULL
                OR m.current_version != d.current_version
                OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Search index status pending failed: {}", e))?;

    let semantic = super::semantic_index::status(storage_dir);

    Ok(SearchIndexStatus {
        total_downloads,
        indexed_downloads,
        pending_downloads,
        is_complete: pending_downloads == 0,
        phase: if pending_downloads == 0 {
            "ready".to_string()
        } else {
            "indexing".to_string()
        },
        indexed_chunks: semantic.indexed_chunks,
        semantic_indexed_chunks: semantic.indexed_chunks,
        semantic_model_ready: semantic.model_ready,
        embedding_provider: semantic.provider,
        gpu_enabled: semantic.gpu_enabled,
        throughput_per_sec: None,
    })
}

fn reindex_download_locked(
    conn: &Connection,
    storage_dir: &Path,
    download_id: i64,
) -> Result<(), String> {
    let Some(doc) = search_index_document_locked(conn, storage_dir, download_id)? else {
        clear_search_index_locked(conn, storage_dir, download_id)?;
        return Ok(());
    };
    index_search_documents_locked(conn, storage_dir, &[doc], true)
}

fn search_index_document_locked(
    conn: &Connection,
    _storage_dir: &Path,
    download_id: i64,
) -> Result<Option<SearchIndexBuildDocument>, String> {
    let row = conn
        .query_row(
            "SELECT d.source, d.source_id, d.title, d.author_name, d.author_id,
                    (SELECT GROUP_CONCAT(t.name, ' ')
                     FROM download_tags dt
                     JOIN tags t ON t.id = dt.tag_id
                     WHERE dt.download_id = d.id),
                    d.excerpt, d.json_path,
                    d.original_json_path, d.current_version, d.content_hash,
                    (SELECT GROUP_CONCAT(ds.title, ' ') FROM download_series ds WHERE ds.download_id = d.id),
                    COALESCE(d.source_created_at, ''),
                    d.downloaded_at,
                    d.favorite,
                    d.watch_updates,
                    d.text_length,
                    (SELECT GROUP_CONCAT(DISTINCT a.asset_type) FROM assets a WHERE a.download_id = d.id)
             FROM downloads d
             WHERE d.id = ?1",
            params![download_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, i64>(14)? != 0,
                    row.get::<_, i64>(15)? != 0,
                    row.get::<_, i64>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Search index download query failed: {}", e))?;

    let Some((
        source,
        source_id,
        title,
        author_name,
        author_id,
        tags_raw,
        excerpt,
        json_path,
        original_json_path,
        current_version,
        content_hash,
        series_title,
        published_at,
        downloaded_at,
        favorite,
        watch_updates,
        text_length,
        asset_kinds,
    )) = row
    else {
        return Ok(None);
    };

    let json_path = original_json_path.unwrap_or(json_path);
    let body = active_edit_plain_text_locked(conn, download_id)?.unwrap_or_else(|| {
        std::fs::read_to_string(&json_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|value| extract_search_body(&value, &source))
            .unwrap_or_default()
    });

    let tags = tags_raw.unwrap_or_default();
    let series_title_raw = series_title.unwrap_or_default();
    let excerpt_raw = excerpt.unwrap_or_default();
    let doc = SearchDocument {
        title: title.clone(),
        author_name: author_name.clone(),
        tags: tags.clone(),
        series_title: series_title_raw.clone(),
        excerpt: excerpt_raw.clone(),
        body: body.clone(),
    };
    Ok(Some(SearchIndexBuildDocument {
        download_id,
        current_version,
        content_hash,
        tantivy: super::tantivy_index::TantivyIndexDocument {
            download_id,
            source: source.clone(),
            source_id: source_id.clone(),
            source_url: source_url_for_download(&source, &source_id, &author_id),
            title: doc.title,
            author_name: doc.author_name,
            author_id: author_id.clone(),
            tags: doc.tags,
            series_title: doc.series_title,
            excerpt: doc.excerpt,
            body: doc.body,
            published_at,
            downloaded_at,
            favorite,
            watch_updates,
            asset_kinds: normalize_search_text(asset_kinds.as_deref().unwrap_or("")),
            text_length,
        },
        semantic: super::semantic_index::SemanticIndexDocument {
            download_id,
            title,
            author_name,
            tags,
            series_title: series_title_raw,
            excerpt: excerpt_raw,
            body,
        },
    }))
}

fn index_search_documents_locked(
    conn: &Connection,
    storage_dir: &Path,
    docs: &[SearchIndexBuildDocument],
    include_semantic: bool,
) -> Result<(), String> {
    if docs.is_empty() {
        return Ok(());
    }
    let tantivy_docs = docs
        .iter()
        .map(|doc| doc.tantivy.clone())
        .collect::<Vec<_>>();
    super::tantivy_index::upsert_documents(storage_dir, &tantivy_docs)?;

    let now = chrono::Utc::now().to_rfc3339();
    for doc in docs {
        conn.execute(
            "INSERT OR REPLACE INTO search_index_state (
                download_id, current_version, content_hash, indexed_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![doc.download_id, doc.current_version, doc.content_hash, now],
        )
        .map_err(|e| format!("Search meta insert failed: {}", e))?;
    }

    if !include_semantic {
        return Ok(());
    }

    let semantic_docs = docs
        .iter()
        .map(|doc| doc.semantic.clone())
        .collect::<Vec<_>>();
    if let Err(error) = super::semantic_index::upsert_documents(storage_dir, &semantic_docs) {
        log::warn!(
            "Semantic index batch update skipped for {} documents: {}",
            docs.len(),
            error
        );
    }

    Ok(())
}

fn clear_search_index_locked(
    conn: &Connection,
    storage_dir: &Path,
    download_id: i64,
) -> Result<(), String> {
    super::tantivy_index::delete_document(storage_dir, download_id)?;
    if let Err(error) = super::semantic_index::clear_document(storage_dir, download_id) {
        log::warn!(
            "Semantic index clear skipped for {}: {}",
            download_id,
            error
        );
    }
    conn.execute(
        "DELETE FROM search_index_state WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Search meta clear failed: {}", e))?;
    Ok(())
}

fn filter_excluded_search_results(
    storage_dir: &Path,
    results: Vec<DownloadEntry>,
    parsed: &ParsedSearchQuery,
) -> Vec<DownloadEntry> {
    if parsed.exclude.is_empty() {
        return results;
    }

    let mut filtered = Vec::with_capacity(results.len());
    for entry in results {
        let Ok(Some(doc)) = super::tantivy_index::load_document(storage_dir, entry.id) else {
            continue;
        };
        if document_matches_excluded_term(&doc, parsed) {
            continue;
        }
        filtered.push(entry);
    }
    filtered
}

fn decorate_search_results(
    storage_dir: &Path,
    results: Vec<DownloadEntry>,
    parsed: &ParsedSearchQuery,
    semantic_hits: &HashMap<i64, super::semantic_index::SemanticSearchHit>,
) -> Vec<DownloadEntry> {
    if parsed.include.is_empty() && semantic_hits.is_empty() {
        return results;
    }

    let mut decorated = Vec::with_capacity(results.len());
    for mut entry in results {
        let Ok(Some(doc)) = super::tantivy_index::load_document(storage_dir, entry.id) else {
            decorated.push(entry);
            continue;
        };
        let (fields, reasons, computed_score) = match_fields_and_score(&doc, parsed);
        if !fields.is_empty() {
            entry.match_fields = fields;
        }
        if !reasons.is_empty() {
            entry.score_reasons = reasons
                .into_iter()
                .map(|reason| ScoreReason {
                    field: reason.field,
                    match_type: reason.match_type,
                    term: reason.term,
                    contribution: reason.contribution,
                    detail: reason.detail,
                })
                .collect();
        }
        if let Some(semantic) = semantic_hits.get(&entry.id) {
            entry.score_reasons.push(ScoreReason {
                field: semantic.field.clone(),
                match_type: "semantic".to_string(),
                term: parsed
                    .include
                    .first()
                    .map(|term| term.raw.clone())
                    .unwrap_or_default(),
                contribution: semantic.score * 120.0,
                detail: Some(format!("semantic chunk score {:.3}", semantic.score)),
            });
            let semantic_highlight = semantic_chunk_highlight(semantic);
            let mut highlights = make_match_highlights(&doc, parsed);
            highlights.insert(0, semantic_highlight);
            entry.match_highlights = highlights.into_iter().take(4).collect();
        } else {
            entry.match_highlights = make_match_highlights(&doc, parsed);
        }
        if computed_score > 0.0 {
            entry.search_score = Some(entry.search_score.unwrap_or(0.0) + computed_score);
        }
        decorated.push(entry);
    }
    decorated
}

fn document_matches_excluded_term(doc: &SearchDocument, parsed: &ParsedSearchQuery) -> bool {
    parsed.exclude.iter().any(|term| {
        doc.fields().iter().any(|(_, text, _)| {
            let normalized = normalize_search_text(text);
            !text.is_empty()
                && term
                    .variants
                    .iter()
                    .any(|variant| !variant.is_empty() && normalized.contains(variant))
        })
    })
}

fn semantic_chunk_highlight(
    semantic: &super::semantic_index::SemanticSearchHit,
) -> SearchHighlight {
    SearchHighlight {
        field: semantic.field.clone(),
        text: semantic.text.clone(),
        segments: vec![SearchHighlightSegment {
            text: truncate_highlight_text(&semantic.text, 220),
            matched: true,
        }],
        source_chunk_id: Some(semantic.chunk_id.clone()),
        match_type: Some("semantic".to_string()),
    }
}

fn truncate_highlight_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn work_edit_revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkEditRevision> {
    Ok(WorkEditRevision {
        id: row.get(0)?,
        download_id: row.get(1)?,
        base_version: row.get(2)?,
        status: row.get(3)?,
        title: row.get(4)?,
        content_hash: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn work_block_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkBlock> {
    Ok(WorkBlock {
        id: row.get(0)?,
        edit_revision_id: row.get(1)?,
        order: row.get(2)?,
        block_type: row.get(3)?,
        text: row.get(4)?,
        asset_id: row.get(5)?,
        attrs_json: row.get(6)?,
    })
}

fn update_target_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateTarget> {
    Ok(UpdateTarget {
        id: row.get(0)?,
        target_type: row.get(1)?,
        source: row.get(2)?,
        source_key: row.get(3)?,
        display_name: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        last_checked_at: row.get(6)?,
        last_seen_source_id: row.get(7)?,
        last_seen_source_updated_at: row.get(8)?,
        metadata_json: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn update_job_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateJobSummary> {
    Ok(UpdateJobSummary {
        job_id: row.get(0)?,
        status: row.get(1)?,
        scope: row.get(2)?,
        mode: row.get(3)?,
        totals: row.get(4)?,
        processed: row.get(5)?,
        candidate_count: row.get(6)?,
        saved_count: row.get(7)?,
        error_count: row.get(8)?,
        active_label: row.get(9)?,
        started_at: row.get(10)?,
        updated_at: row.get(11)?,
        finished_at: row.get(12)?,
    })
}

fn update_job_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateJobItem> {
    Ok(UpdateJobItem {
        id: row.get(0)?,
        job_id: row.get(1)?,
        item_type: row.get(2)?,
        source: row.get(3)?,
        source_id: row.get(4)?,
        target_type: row.get(5)?,
        title: row.get(6)?,
        payload_json: row.get(7)?,
        status: row.get(8)?,
        error: row.get(9)?,
        result_download_id: row.get(10)?,
    })
}

fn download_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadEntry> {
    let tags = row
        .get::<_, Option<String>>(7)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    let match_fields = row
        .get::<_, Option<String>>(27)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let score_reasons = row
        .get::<_, Option<String>>(28)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let match_highlights = row
        .get::<_, Option<String>>(29)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    Ok(DownloadEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_id: row.get(2)?,
        title: row.get(3)?,
        author_name: row.get(4)?,
        author_id: row.get(5)?,
        content_type: row.get(6)?,
        tags,
        excerpt: row.get(8)?,
        cover_path: row.get(9)?,
        json_path: row.get(10)?,
        original_json_path: row.get(11)?,
        asset_count: row.get(12)?,
        file_size_bytes: row.get(13)?,
        downloaded_at: row.get(14)?,
        source_created_at: row.get(15)?,
        content_hash: row.get(16)?,
        text_length: row.get(17)?,
        source_updated_at: row.get(18)?,
        watch_updates: row.get::<_, i64>(19)? != 0,
        current_version: row.get(20)?,
        favorite: row.get::<_, i64>(21)? != 0,
        person_id: row.get(22).ok(),
        person_name: row.get(23).ok(),
        series_id: row.get(24).ok(),
        series_title: row.get(25).ok(),
        search_score: row.get(26).ok(),
        match_fields,
        score_reasons,
        match_highlights,
        sort_key: row.get(30).ok(),
        // Appended last so the existing positional reads keep their indexes.
        person_icon_path: row.get(31).ok(),
    })
}

fn person_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonEntry> {
    Ok(PersonEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_key: row.get(2)?,
        display_name: row.get(3)?,
        icon_path: row.get(4)?,
        cover_path: row.get(5)?,
        description: row.get(6)?,
        links_json: row.get(7)?,
        content_hash: row.get(8)?,
        current_version: row.get(9)?,
        last_checked_at: row.get(10)?,
        last_fetched_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        work_count: row.get(14).ok(),
    })
}

fn series_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SeriesEntry> {
    Ok(SeriesEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_key: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        cover_path: row.get(5)?,
        content_hash: row.get(6)?,
        current_version: row.get(7)?,
        last_checked_at: row.get(8)?,
        last_fetched_at: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        work_count: row.get(12).ok(),
    })
}

fn entity_version_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityVersion> {
    Ok(EntityVersion {
        id: row.get(0)?,
        entity_type: row.get(1)?,
        source: row.get(2)?,
        source_key: row.get(3)?,
        version: row.get(4)?,
        content_hash: row.get(5)?,
        json_path: row.get(6)?,
        asset_count: row.get(7)?,
        file_size_bytes: row.get(8)?,
        created_at: row.get(9)?,
        change_summary: row.get(10)?,
    })
}

/// Windowsなどでファイルが掴まれているため一時的に削除に失敗するケースに備えた、
/// リトライ機能付きのディレクトリ再帰削除ヘルパー
fn remove_dir_all_resilient(path: &std::path::Path) -> std::io::Result<()> {
    let mut attempts = 5;
    loop {
        match std::fs::remove_dir_all(path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                attempts -= 1;
                if attempts == 0 {
                    return Err(e);
                }
                log::warn!(
                    "Failed to remove directory {:?}, retrying in 150ms... (Attempts left: {}) Error: {}",
                    path,
                    attempts,
                    e
                );
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
}

/// ディレクトリが空の場合にのみ削除する、リトライ機能付きの削除ヘルパー
/// (空ではない場合のエラーは即座に返す)
fn remove_dir_resilient(path: &std::path::Path) -> std::io::Result<()> {
    let mut attempts = 5;
    loop {
        match std::fs::remove_dir(path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                // ディレクトリが空ではない場合はリトライしても無駄なので即座に返す
                // Windows: ERROR_DIR_NOT_EMPTY (145), Unix: ENOTEMPTY (39)
                if let Some(code) = e.raw_os_error() {
                    if code == 145 || code == 39 {
                        return Err(e);
                    }
                }
                attempts -= 1;
                if attempts == 0 {
                    return Err(e);
                }
                log::warn!(
                    "Failed to remove empty directory {:?}, retrying in 150ms... (Attempts left: {}) Error: {}",
                    path,
                    attempts,
                    e
                );
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
}

#[cfg(test)]
mod search_integration_tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn editor_blocks_preserve_rich_content_order() {
        let assets = vec![AssetEntry {
            id: 42,
            download_id: 7,
            asset_type: "image".to_string(),
            filename: "scene.webp".to_string(),
            local_path: "assets/scene.webp".to_string(),
            original_url: None,
            mime_type: Some("image/webp".to_string()),
            file_size_bytes: 128,
        }];
        let html = concat!(
            "<p>導入<br>二行目</p>",
            "<!-- newpage -->",
            "<h2>章題</h2>",
            "<img data-local-path=\"assets/scene.webp\" alt=\"挿絵\">",
            "<a class=\"novel-link-card\" href=\"https://example.com/story\">続きはこちら</a>",
            "<hr>",
            "<p>結び</p>"
        );

        let blocks = html_to_editor_blocks(html, &assets);

        assert_eq!(
            blocks
                .iter()
                .map(|block| block.block_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "paragraph",
                "page_break",
                "heading",
                "image",
                "link",
                "separator",
                "paragraph"
            ]
        );
        assert_eq!(blocks[0].text.as_deref(), Some("導入\n二行目"));
        assert_eq!(blocks[3].asset_id, Some(42));
        assert_eq!(blocks[3].text.as_deref(), Some("挿絵"));
        assert_eq!(blocks[4].text.as_deref(), Some("https://example.com/story"));
        assert!(blocks[4]
            .attrs_json
            .as_deref()
            .is_some_and(|attrs| attrs.contains("続きはこちら")));
        assert!(blocks_to_html(&blocks, &assets).contains("<!-- newpage -->"));
    }

    fn temp_paths() -> (PathBuf, PathBuf) {
        let rand_val: u32 = rand::random();
        let root = std::env::temp_dir().join(format!("piep_search_test_{}", rand_val));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        (root, storage)
    }

    fn params(query: &str) -> SearchV2Params {
        SearchV2Params {
            text: None,
            query: Some(query.to_string()),
            source: None,
            content_type: None,
            sort_by: Some("relevance".to_string()),
            sort_order: Some("desc".to_string()),
            limit: Some(20),
            cursor: None,
            favorite: None,
            tags_include: None,
            tags_exclude: None,
            tag_filter_mode: None,
            authors_include: None,
            authors_exclude: None,
            min_char_count: None,
            max_char_count: None,
            asset_filter: None,
            watch_filter: None,
            person_source: None,
            person_key: None,
            series_source: None,
            series_key: None,
            view_mode: None,
            projection: None,
            search_mode: None,
        }
    }

    fn v2_params(query: Option<&str>, limit: i64, cursor: Option<String>) -> SearchV2Params {
        SearchV2Params {
            text: None,
            query: query.map(str::to_string),
            source: None,
            content_type: None,
            sort_by: Some(if query.is_some() { "relevance" } else { "date" }.to_string()),
            sort_order: Some("desc".to_string()),
            limit: Some(limit),
            cursor,
            favorite: None,
            tags_include: None,
            tags_exclude: None,
            tag_filter_mode: None,
            authors_include: None,
            authors_exclude: None,
            min_char_count: None,
            max_char_count: None,
            asset_filter: None,
            watch_filter: None,
            person_source: None,
            person_key: None,
            series_source: None,
            series_key: None,
            view_mode: None,
            projection: None,
            search_mode: None,
        }
    }

    fn insert_download(
        db: &Database,
        storage: &Path,
        source_id: &str,
        title: &str,
        author: &str,
        tags: &[&str],
        body: &str,
    ) -> i64 {
        insert_download_with_reindex(
            db,
            storage,
            TestDownloadInput {
                source_id,
                title,
                author,
                tags,
                body,
                reindex: true,
            },
        )
    }

    fn insert_download_unindexed(
        db: &Database,
        storage: &Path,
        source_id: &str,
        title: &str,
        author: &str,
        tags: &[&str],
        body: &str,
    ) -> i64 {
        insert_download_with_reindex(
            db,
            storage,
            TestDownloadInput {
                source_id,
                title,
                author,
                tags,
                body,
                reindex: false,
            },
        )
    }

    struct TestDownloadInput<'a> {
        source_id: &'a str,
        title: &'a str,
        author: &'a str,
        tags: &'a [&'a str],
        body: &'a str,
        reindex: bool,
    }

    fn insert_download_with_reindex(
        db: &Database,
        storage: &Path,
        input: TestDownloadInput<'_>,
    ) -> i64 {
        let TestDownloadInput {
            source_id,
            title,
            author,
            tags,
            body,
            reindex,
        } = input;
        let dir = storage.join("pixiv").join(source_id).join("v1");
        fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("original.json");
        fs::write(
            &json_path,
            serde_json::json!({ "text": body }).to_string().as_bytes(),
        )
        .unwrap();
        let dl = NewDownload {
            source: "pixiv".to_string(),
            source_id: source_id.to_string(),
            title: title.to_string(),
            author_name: author.to_string(),
            author_id: format!("author-{}", source_id),
            content_type: "novel".to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            excerpt: Some("短い概要".to_string()),
            cover_path: None,
            json_path: json_path.to_string_lossy().to_string(),
            original_json_path: Some(json_path.to_string_lossy().to_string()),
            asset_count: 0,
            file_size_bytes: 0,
            downloaded_at: "2026-01-01T00:00:00Z".to_string(),
            source_created_at: Some("2026-01-01T00:00:00Z".to_string()),
            content_hash: Some(format!("hash-{}", source_id)),
            text_length: body.chars().count() as i64,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        };
        let id = db.upsert_download(&dl).unwrap();
        if reindex {
            db.reindex_download(id).unwrap();
        }
        id
    }

    #[test]
    fn entity_facets_search_and_page_beyond_the_dashboard_cap() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        // get_filter_facets only ever returns the top 60 authors, so the
        // library tab needs a query that can reach past that.
        for index in 0..70 {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("{}", 100 + index),
                &format!("作品{}", index),
                &format!("作者{:02}", index),
                &["日常"],
                "本文",
            );
        }

        let capped = db.get_filter_facets().unwrap();
        assert_eq!(capped.author_entities.len(), 60);

        let second_page = db.search_entity_facets("person", None, 60, 60).unwrap();
        assert_eq!(second_page.len(), 10, "authors past the cap stay reachable");

        let filtered = db.search_entity_facets("person", Some("作者69"), 60, 0).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].display_name, "作者69");
        assert_eq!(filtered[0].count, 1);

        let missing = db.search_entity_facets("person", Some("存在しない"), 60, 0).unwrap();
        assert!(missing.is_empty());

        assert!(db.search_entity_facets("unknown", None, 10, 0).is_err());
    }

    #[test]
    fn smart_search_ranks_metadata_over_body_and_supports_body_search() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let body_only = insert_download(
            &db,
            &storage,
            "1",
            "静かな本文一致",
            "作者A",
            &["日常"],
            "ここだけに秘密キーワードが出てくる長い本文です。",
        );
        let title_hit = insert_download(
            &db,
            &storage,
            "2",
            "秘密キーワードのタイトル",
            "作者B",
            &["冒険"],
            "本文には別の内容を書いておく。",
        );

        let results = db
            .search_downloads_v2(&params("秘密キーワード"))
            .unwrap()
            .items;
        assert_eq!(results.first().map(|dl| dl.id), Some(title_hit));
        assert!(results.iter().any(|dl| dl.id == body_only));
        assert!(results
            .iter()
            .find(|dl| dl.id == body_only)
            .map(|dl| !dl.match_highlights.is_empty())
            .unwrap_or(false));

        let partial = db.search_downloads_v2(&params("秘密キ")).unwrap().items;
        assert!(partial.iter().any(|dl| dl.id == body_only));

        let excluded = db
            .search_downloads_v2(&params("秘密 -タイトル"))
            .unwrap()
            .items;
        assert!(excluded.iter().any(|dl| dl.id == body_only));
        assert!(!excluded.iter().any(|dl| dl.id == title_hit));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_v2_uses_cursor_without_duplicate_pages() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let first = insert_download(&db, &storage, "1", "一番目", "作者A", &["日常"], "本文A");
        let second = insert_download(&db, &storage, "2", "二番目", "作者B", &["日常"], "本文B");
        let third = insert_download(&db, &storage, "3", "三番目", "作者C", &["日常"], "本文C");

        let page1 = db.search_downloads_v2(&v2_params(None, 2, None)).unwrap();
        assert_eq!(page1.items.len(), 2);
        assert!(page1.next_cursor.is_some());
        assert!(page1.next_cursor.as_deref().unwrap().starts_with("k:"));

        let page2 = db
            .search_downloads_v2(&v2_params(None, 2, page1.next_cursor.clone()))
            .unwrap();
        let mut seen = page1.items.iter().map(|dl| dl.id).collect::<Vec<_>>();
        seen.extend(page2.items.iter().map(|dl| dl.id));
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3);
        assert!(seen.contains(&first));
        assert!(seen.contains(&second));
        assert!(seen.contains(&third));
        assert!(page2.next_cursor.is_none());

        let query = db
            .search_downloads_v2(&v2_params(Some("番目"), 10, None))
            .unwrap();
        assert_eq!(query.search_meta.engine, "hybrid-local");
        assert_eq!(query.items.len(), 3);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_suggest_returns_metadata_candidates() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        insert_download(
            &db,
            &storage,
            "suggest-1",
            "候補タイトル",
            "候補作者",
            &["候補タグ"],
            "本文",
        );

        let suggestions = db
            .search_suggest(&SearchSuggestParams {
                text: Some("候補".to_string()),
                limit: Some(10),
            })
            .unwrap();
        assert!(suggestions
            .items
            .iter()
            .any(|item| item.kind == "tag" && item.label == "候補タグ"));
        assert!(suggestions
            .items
            .iter()
            .any(|item| item.kind == "author" && item.label == "候補作者"));
        assert!(suggestions
            .items
            .iter()
            .any(|item| item.kind == "title" && item.label == "候補タイトル"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn series_token_filters_by_series_relation() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let in_series = insert_download(
            &db,
            &storage,
            "series-1",
            "シリーズ内作品",
            "作者A",
            &["連載"],
            "本文",
        );
        let outside = insert_download(
            &db,
            &storage,
            "series-2",
            "シリーズ外作品",
            "作者B",
            &["読切"],
            "本文",
        );
        db.upsert_download_series(in_series, "pixiv", "s-100", "構造化シリーズ", Some(1))
            .unwrap();

        let result = db
            .search_downloads_v2(&params("series:pixiv:s-100"))
            .unwrap();
        assert_eq!(
            result.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
            vec![in_series]
        );
        assert!(!result.items.iter().any(|dl| dl.id == outside));

        let by_title = db
            .search_downloads_v2(&params("series:\"構造化シリーズ\""))
            .unwrap();
        assert_eq!(
            by_title.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
            vec![in_series]
        );

        let suggestions = db
            .search_suggest(&SearchSuggestParams {
                text: Some("構造化".to_string()),
                limit: Some(10),
            })
            .unwrap();
        assert!(suggestions.items.iter().any(|item| {
            item.kind == "series" && item.label == "構造化シリーズ" && item.value == "pixiv:s-100"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn japanese_reading_kana_and_romaji_match_same_work() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "jp-reading-1",
            "小説テスト作品",
            "作者A",
            &["物語"],
            "本文にも小説という語を含みます。",
        );

        for query in ["てすと", "テスト", "tesuto", "しょうせつ", "shousetsu"] {
            let result = db.search_downloads_v2(&params(query)).unwrap();
            assert!(
                result.items.iter().any(|dl| dl.id == target),
                "query {query} should match the target"
            );
        }

        let romaji = db.search_downloads_v2(&params("shousetsu")).unwrap();
        let target_row = romaji.items.iter().find(|dl| dl.id == target).unwrap();
        assert!(target_row.score_reasons.iter().any(|reason| {
            matches!(
                reason.match_type.as_str(),
                "exact" | "reading" | "romaji" | "synonym"
            )
        }));
        assert!(!target_row
            .score_reasons
            .iter()
            .any(|reason| reason.match_type == "semantic"));
        assert!(!target_row.match_highlights.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn smart_search_does_not_add_semantic_reasons() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "smart-no-semantic-1",
            "小説テスト作品",
            "作者A",
            &["物語"],
            "本文にも小説という語を含みます。",
        );
        let result = db.search_downloads_v2(&params("novel")).unwrap();
        let target_row = result.items.iter().find(|dl| dl.id == target).unwrap();

        assert_eq!(result.search_meta.engine, "hybrid-local");
        assert!(!target_row
            .score_reasons
            .iter()
            .any(|reason| reason.match_type == "semantic"));
        assert!(!target_row
            .match_highlights
            .iter()
            .any(|highlight| highlight.match_type.as_deref() == Some("semantic")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn multi_term_search_requires_each_term() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "multi-term-1",
            "alpha beta target",
            "作者A",
            &["mixed"],
            "本文",
        );
        let alpha_only = insert_download(
            &db,
            &storage,
            "multi-term-2",
            "alpha only",
            "作者B",
            &["mixed"],
            "本文",
        );
        let beta_only = insert_download(
            &db,
            &storage,
            "multi-term-3",
            "beta only",
            "作者C",
            &["mixed"],
            "本文",
        );

        let result = db.search_downloads_v2(&params("alpha beta")).unwrap();
        assert!(result.items.iter().any(|item| item.id == target));
        assert!(!result.items.iter().any(|item| item.id == alpha_only));
        assert!(!result.items.iter().any(|item| item.id == beta_only));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn semantic_mode_returns_body_chunk_highlight_and_reason() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        let target = insert_download(
            &db,
            &storage,
            "semantic-1",
            "静かな作品",
            "作者A",
            &["日常"],
            "これは長い小説本文です。読者が物語を探すときに本文チャンクで見つかります。",
        );
        let mut semantic_params = params("novel");
        semantic_params.search_mode = Some("semantic".to_string());

        let result = db.search_downloads_v2(&semantic_params).unwrap();
        let target_row = result.items.iter().find(|dl| dl.id == target).unwrap();
        assert!(target_row
            .score_reasons
            .iter()
            .any(|reason| reason.match_type == "semantic"));
        assert!(target_row.match_highlights.iter().any(|highlight| {
            highlight.match_type.as_deref() == Some("semantic")
                && highlight
                    .segments
                    .iter()
                    .any(|segment| segment.matched && segment.text.contains("小説本文"))
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "performance smoke test for large-library browsing"]
    fn library_browsing_stays_fast_on_a_large_library() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        const SEEDED: usize = 20_000;
        for index in 0..SEEDED {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("scale-{}", index),
                &format!("蔵書 {:05}", index),
                &format!("作者 {:03}", index % 400),
                &[&format!("tag{}", index % 30)],
                "大規模ライブラリの一覧性能を確認するための本文です。",
            );
        }

        // The library browses with an empty query, which is the keyset path.
        let mut first = v2_params(None, 60, None);
        first.projection = Some("libraryGallery".to_string());
        let started = Instant::now();
        let page1 = db.search_downloads_v2(&first).unwrap();
        let first_elapsed = started.elapsed();
        assert_eq!(page1.items.len(), 60);
        assert_eq!(page1.total_estimate, Some(SEEDED as i64));
        assert!(
            page1.next_cursor.is_some(),
            "a large library must expose more pages"
        );

        // Walk deep into the list: keyset paging must not degrade with depth.
        let mut cursor = page1.next_cursor.clone();
        let mut deepest = Duration::ZERO;
        for _ in 0..40 {
            let mut page_params = v2_params(None, 60, cursor.clone());
            page_params.projection = Some("libraryGallery".to_string());
            let started = Instant::now();
            let page = db.search_downloads_v2(&page_params).unwrap();
            deepest = deepest.max(started.elapsed());
            cursor = page.next_cursor.clone();
            assert!(cursor.is_some(), "paging ended earlier than expected");
        }

        let started = Instant::now();
        let authors = db.search_entity_facets("person", None, 60, 300).unwrap();
        let entity_elapsed = started.elapsed();
        assert_eq!(authors.len(), 60);

        let started = Instant::now();
        let facets = db.get_filter_facets_with(false).unwrap();
        let facet_elapsed = started.elapsed();
        assert!(!facets.tags.is_empty());
        assert!(
            facets.author_entities.is_empty(),
            "the light variant must skip the entity aggregates"
        );

        eprintln!(
            "{} works: first page {:?}, deepest page {:?}, authors {:?}, filter options {:?}",
            SEEDED, first_elapsed, deepest, entity_elapsed, facet_elapsed
        );
        assert!(
            first_elapsed < Duration::from_millis(400),
            "first page took {:?}",
            first_elapsed
        );
        assert!(
            deepest < Duration::from_millis(400),
            "deep page took {:?}",
            deepest
        );
        // Without idx_downloads_author_recent this listing takes ~1.9s here.
        assert!(
            entity_elapsed < Duration::from_millis(300),
            "author listing took {:?}",
            entity_elapsed
        );
        assert!(
            facet_elapsed < Duration::from_millis(500),
            "filter options took {:?}",
            facet_elapsed
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "performance smoke test for local search tuning"]
    fn smart_search_handles_5000_seed_items_under_target() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();

        for index in 0..5_000 {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("perf-{}", index),
                &format!("Seed Work {:04}", index),
                &format!("Seed Author {:02}", index % 50),
                &[&format!("seed{}", index % 25)],
                &format!(
                    "検索性能検証用の本文です。番号 {}、タグ seed{}。日本語とASCII mixed text for search.",
                    index,
                    index % 25
                ),
            );
        }
        loop {
            let status = db.rebuild_search_index_batch(200).unwrap();
            if status.pending_downloads == 0 {
                break;
            }
        }

        let mut search_params = params("検索性能 seed7");
        search_params.limit = Some(120);
        let started = Instant::now();
        let result = db.search_downloads_v2(&search_params).unwrap();
        let elapsed = started.elapsed();

        assert!(!result.items.is_empty());
        assert!(
            elapsed < Duration::from_secs(1),
            "smart search took {:?}",
            elapsed
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn projection_selects_do_not_include_unneeded_subqueries() {
        let bulk = download_select_sql_for_projection(Some("bulk"), "NULL", "NULL");
        assert!(!bulk.contains("download_tags"));
        assert!(!bulk.contains("download_people"));
        assert!(!bulk.contains("download_series"));

        let entity = download_select_sql_for_projection(Some("entityFacet"), "NULL", "NULL");
        assert!(!entity.contains("download_tags"));
        assert!(!entity.contains("download_people"));
        assert!(!entity.contains("download_series"));
        assert!(entity.contains("d.cover_path"));

        // The list view shows a tag column, so compact now pays for that
        // lookup; the excerpt is still the one thing it does not read.
        let compact = download_select_sql_for_projection(Some("libraryCompact"), "NULL", "NULL");
        assert!(compact.contains("download_tags"));
        assert!(compact.contains("download_people"));
        assert!(compact.contains("download_series"));
        assert!(compact.contains("NULL AS excerpt"));

        let gallery = download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL");
        assert!(gallery.contains("download_tags"));
        assert!(gallery.contains("download_people"));
        assert!(gallery.contains("download_series"));
    }

    #[test]
    fn active_edit_revision_drives_reader_and_search_body() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let download_id = insert_download(
            &db,
            &storage,
            "edit-1",
            "編集対象",
            "作者A",
            &["編集"],
            "原本だけにある本文です。",
        );

        let initial_reader = db.get_reader_document(download_id, None).unwrap();
        assert!(!initial_reader.is_edited);
        assert!(initial_reader.plain_text.contains("原本だけ"));

        let draft = db
            .save_work_draft(
                download_id,
                1,
                &[
                    WorkBlockInput {
                        block_type: "heading".to_string(),
                        text: Some("編集版見出し".to_string()),
                        asset_id: None,
                        attrs_json: None,
                    },
                    WorkBlockInput {
                        block_type: "paragraph".to_string(),
                        text: Some("編集固有キーワードを含む本文です。".to_string()),
                        asset_id: None,
                        attrs_json: None,
                    },
                ],
            )
            .unwrap();
        db.activate_work_edit(draft.id).unwrap();

        let edited_reader = db.get_reader_document(download_id, None).unwrap();
        assert!(edited_reader.is_edited);
        assert!(edited_reader.html.contains("編集版見出し"));
        assert!(edited_reader.plain_text.contains("編集固有キーワード"));

        let search = db
            .search_downloads_v2(&v2_params(Some("編集固有キーワード"), 10, None))
            .unwrap();
        assert_eq!(search.items.first().map(|item| item.id), Some(download_id));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_job_schema_recovers_interrupted_jobs() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "work".to_string(),
            mode: "auto_save".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            concurrency: None,
        };
        db.create_update_job(
            "job-test",
            &request,
            &[UpdateJobItemInput {
                item_type: "work".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("1".to_string()),
                target_type: Some("work".to_string()),
                title: "Test".to_string(),
                payload_json: "{}".to_string(),
                status: "queued".to_string(),
            }],
        )
        .unwrap();
        db.set_update_job_status("job-test", "running", Some("running"))
            .unwrap();
        db.recover_update_jobs_on_startup().unwrap();
        let snapshot = db.update_job_snapshot("job-test").unwrap();
        assert_eq!(snapshot.status, "paused");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_job_candidates_can_be_queued_for_saving() {
        let (root, storage) = temp_paths();
        let db = Database::open(&root.join("piep.db"), &storage).unwrap();
        let request = StartUpdateJobRequest {
            scope: "author".to_string(),
            mode: "check_only".to_string(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            concurrency: None,
        };
        db.create_update_job(
            "job-candidates",
            &request,
            &[UpdateJobItemInput {
                item_type: "target".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("user-1".to_string()),
                target_type: Some("author".to_string()),
                title: "Author".to_string(),
                payload_json: "{}".to_string(),
                status: "done".to_string(),
            }],
        )
        .unwrap();
        db.insert_update_job_candidate(
            "job-candidates",
            &UpdateJobItemInput {
                item_type: "candidate".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some("novel-1".to_string()),
                target_type: Some("author".to_string()),
                title: "Novel".to_string(),
                payload_json: serde_json::json!({
                    "targetLabel": "Author",
                    "subtitle": "Author / now"
                })
                .to_string(),
                status: "candidate".to_string(),
            },
        )
        .unwrap();
        let snapshot = db.update_job_snapshot("job-candidates").unwrap();
        let candidate_id = snapshot.candidates[0].id;
        let changed = db
            .queue_update_job_candidates("job-candidates", &[candidate_id])
            .unwrap();
        assert_eq!(changed, 1);
        let snapshot = db.update_job_snapshot("job-candidates").unwrap();
        assert_eq!(snapshot.candidates[0].status, "queued");

        let _ = fs::remove_dir_all(root);
    }
}
