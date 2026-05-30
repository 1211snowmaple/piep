//! データベースCRUD操作。

use rusqlite::{params, Connection, OptionalExtension};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::models::*;
use super::schema;
use super::search::{
    extract_search_body, fts_query, generate_ngrams_limited, make_match_snippet,
    match_fields_and_score, ngram_threshold, normalize_search_text, normalized_levenshtein,
    parse_search_query, parse_tags_for_search, query_ngrams, ParsedSearchQuery, SearchDocument,
};

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

/// スレッドセーフなデータベースハンドル
pub struct Database {
    conn: Mutex<Connection>,
    storage_dir: PathBuf,
}

impl Database {
    /// データベースを開く（存在しなければ作成）
    pub fn open(db_path: &Path, storage_dir: &Path) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| format!("DB open failed: {}", e))?;
        schema::initialize(&conn).map_err(|e| format!("DB init failed: {}", e))?;

        // ストレージディレクトリを作成
        std::fs::create_dir_all(storage_dir)
            .map_err(|e| format!("Storage dir creation failed: {}", e))?;

        Ok(Self {
            conn: Mutex::new(conn),
            storage_dir: storage_dir.to_path_buf(),
        })
    }

    /// ストレージディレクトリのパスを取得
    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    pub fn reindex_download(&self, download_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        reindex_download_locked(&conn, download_id)
    }

    pub fn get_search_index_status(&self) -> Result<SearchIndexStatus, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        search_index_status_locked(&conn)
    }

    pub fn rebuild_search_index_batch(&self, limit: i64) -> Result<SearchIndexStatus, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let limit = limit.clamp(1, 200);
        let ids = stale_search_index_ids_locked(&conn, limit)?;
        for id in ids {
            if let Err(e) = reindex_download_locked(&conn, id) {
                log::warn!("Failed to rebuild search index for download {}: {}", id, e);
            }
        }
        search_index_status_locked(&conn)
    }

    pub fn search_filter_facets(
        &self,
        kind: &str,
        query: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FacetCount>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
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
                content_type, tags, excerpt, cover_path, json_path,
                original_json_path, asset_count, file_size_bytes,
                downloaded_at, source_created_at,
                content_hash, text_length, source_updated_at, watch_updates, current_version, favorite
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
            ON CONFLICT(source, source_id) DO UPDATE SET
                title = excluded.title,
                author_name = excluded.author_name,
                tags = excluded.tags,
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
                dl.tags,
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

        // タグの抽出・パース・正規化インサート
        if let Some(ref tags_str) = dl.tags {
            let tags_str_trimmed = tags_str.trim();
            if !tags_str_trimmed.is_empty() {
                let tags: Vec<String> =
                    if tags_str_trimmed.starts_with('[') && tags_str_trimmed.ends_with(']') {
                        serde_json::from_str(tags_str_trimmed).unwrap_or_else(|_| {
                            tags_str_trimmed
                                .replace(['[', ']', '"', '\''], "")
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                    } else {
                        tags_str_trimmed
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    };

                for tag_name in tags {
                    let clean_tag = tag_name.trim().to_string();
                    if clean_tag.is_empty() {
                        continue;
                    }
                    tx.execute(
                        "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                        params![clean_tag],
                    )
                    .map_err(|e| format!("Failed to insert tag: {}", e))?;

                    let tag_id: i64 = tx
                        .query_row(
                            "SELECT id FROM tags WHERE name = ?1",
                            params![clean_tag],
                            |row| row.get(0),
                        )
                        .map_err(|e| format!("Failed to retrieve tag ID: {}", e))?;

                    tx.execute(
                        "INSERT OR IGNORE INTO download_tags (download_id, tag_id) VALUES (?1, ?2)",
                        params![id, tag_id],
                    )
                    .map_err(|e| format!("Failed to insert download tag relation: {}", e))?;
                }
            }
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
                "UPDATE series SET title = ?1, description = ?2, cover_path = ?3,
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
                    cover_path = excluded.cover_path,
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

    /// ダウンロード一覧を検索
    /// ダウンロード一覧を検索
    pub fn search_downloads(&self, params: &SearchParams) -> Result<Vec<DownloadEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let parsed_query = params
            .query
            .as_ref()
            .map(|query| parse_search_query(query))
            .filter(|parsed| !parsed.include.is_empty() || !parsed.exclude.is_empty());
        let has_positive_query = parsed_query
            .as_ref()
            .map(|parsed| !parsed.include.is_empty())
            .unwrap_or(false);
        let search_score_column = if has_positive_query {
            "COALESCE(search.search_score, 0.0)"
        } else {
            "NULL"
        };

        let mut sql = format!(
            "SELECT d.*,
                (SELECT dp.person_key FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1) AS person_id,
                (SELECT dp.display_name FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1) AS person_name,
                (SELECT ds.series_key FROM download_series ds WHERE ds.download_id = d.id LIMIT 1) AS series_id,
                (SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1) AS series_title,
                {} AS search_score,
                NULL AS match_snippet,
                NULL AS match_fields
             FROM downloads d",
            search_score_column
        );
        let mut joins = Vec::new();
        let mut wheres = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        // 1. Smart Search: FTS5 + n-gram + phrase/exclude conditions
        if let Some(ref parsed) = parsed_query {
            if has_positive_query {
                let mut scoring_subqueries = Vec::new();

                if let Some(search_fts_query) = fts_query(parsed) {
                    scoring_subqueries.push(
                        "SELECT download_id, 25.0 AS score
                         FROM download_search_fts
                         WHERE download_search_fts MATCH ?"
                            .to_string(),
                    );
                    bind_values.push(Box::new(search_fts_query));
                }

                let all_query_grams = parsed
                    .include
                    .iter()
                    .flat_map(|term| query_ngrams(&term.normalized))
                    .collect::<std::collections::BTreeSet<String>>()
                    .into_iter()
                    .collect::<Vec<String>>();

                if !all_query_grams.is_empty() {
                    let placeholders = vec!["?"; all_query_grams.len()].join(", ");
                    scoring_subqueries.push(format!(
                        "SELECT download_id, SUM(weight) AS score
                         FROM download_search_ngrams
                         WHERE token IN ({})
                         GROUP BY download_id",
                        placeholders
                    ));
                    for gram in &all_query_grams {
                        bind_values.push(Box::new(gram.clone()));
                    }
                }

                if !scoring_subqueries.is_empty() {
                    joins.push(format!(
                        "LEFT JOIN (
                            SELECT download_id, SUM(score) AS search_score
                            FROM ({})
                            GROUP BY download_id
                         ) search ON search.download_id = d.id",
                        scoring_subqueries.join(" UNION ALL ")
                    ));
                    wheres.push("search.search_score IS NOT NULL".to_string());
                }

                for term in &parsed.include {
                    let grams = query_ngrams(&term.normalized);
                    if grams.is_empty() {
                        continue;
                    }
                    if term.normalized.chars().count() == 1 {
                        wheres.push(
                            "d.id IN (
                                SELECT download_id FROM download_search_fts
                                WHERE title LIKE ? OR author_name LIKE ? OR tags LIKE ?
                                   OR series_title LIKE ? OR excerpt LIKE ? OR body LIKE ?
                            )"
                            .to_string(),
                        );
                        let like = format!("%{}%", term.normalized);
                        for _ in 0..6 {
                            bind_values.push(Box::new(like.clone()));
                        }
                    } else {
                        let placeholders = vec!["?"; grams.len()].join(", ");
                        wheres.push(format!(
                            "d.id IN (
                                SELECT download_id
                                FROM download_search_ngrams
                                WHERE token IN ({})
                                GROUP BY download_id
                                HAVING COUNT(DISTINCT token) >= ?
                            )",
                            placeholders
                        ));
                        let threshold = ngram_threshold(grams.len());
                        for gram in grams {
                            bind_values.push(Box::new(gram));
                        }
                        bind_values.push(Box::new(threshold));
                    }
                }
            }

            for term in &parsed.exclude {
                wheres.push(
                    "d.id NOT IN (
                        SELECT download_id FROM download_search_fts
                        WHERE title LIKE ? OR author_name LIKE ? OR tags LIKE ?
                           OR series_title LIKE ? OR excerpt LIKE ? OR body LIKE ?
                    )"
                    .to_string(),
                );
                let like = format!("%{}%", term.normalized);
                for _ in 0..6 {
                    bind_values.push(Box::new(like.clone()));
                }
            }
        }

        // 2. ソース絞り込み
        if let Some(ref src) = params.source {
            if !src.is_empty() {
                wheres.push("d.source = ?".to_string());
                bind_values.push(Box::new(src.clone()));
            }
        }

        // 3. コンテンツタイプ絞り込み
        if let Some(ref ct) = params.content_type {
            if !ct.is_empty() {
                wheres.push("d.content_type = ?".to_string());
                bind_values.push(Box::new(ct.clone()));
            }
        }

        // 4. お気に入り絞り込み
        if let Some(fav) = params.favorite {
            if fav {
                wheres.push("d.favorite = 1".to_string());
            }
        }

        // 5. 含むタグの絞り込み (AND/OR条件)
        if let Some(ref tags_inc) = params.tags_include {
            let active_tags: Vec<String> = tags_inc
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !active_tags.is_empty() {
                let placeholders = vec!["?"; active_tags.len()].join(", ");
                if params.tag_filter_mode.as_deref() == Some("or") {
                    let subquery = format!(
                        "d.id IN (
                            SELECT download_id FROM download_tags dt
                            JOIN tags t ON dt.tag_id = t.id
                            WHERE t.name IN ({})
                        )",
                        placeholders
                    );
                    wheres.push(subquery);
                    for tag in active_tags {
                        bind_values.push(Box::new(tag));
                    }
                } else {
                    let subquery = format!(
                        "d.id IN (
                            SELECT download_id FROM download_tags dt
                            JOIN tags t ON dt.tag_id = t.id
                            WHERE t.name IN ({})
                            GROUP BY download_id
                            HAVING COUNT(DISTINCT t.name) = ?
                        )",
                        placeholders
                    );
                    wheres.push(subquery);
                    let count_inc = active_tags.len() as i64;
                    for tag in active_tags {
                        bind_values.push(Box::new(tag));
                    }
                    bind_values.push(Box::new(count_inc));
                }
            }
        }

        // 6. 除外タグの絞り込み (NOT IN)
        if let Some(ref tags_exc) = params.tags_exclude {
            let active_tags: Vec<String> = tags_exc
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !active_tags.is_empty() {
                let placeholders = vec!["?"; active_tags.len()].join(", ");
                let subquery = format!(
                    "d.id NOT IN (
                        SELECT download_id FROM download_tags dt
                        JOIN tags t ON dt.tag_id = t.id
                        WHERE t.name IN ({})
                    )",
                    placeholders
                );
                wheres.push(subquery);
                for tag in active_tags {
                    bind_values.push(Box::new(tag));
                }
            }
        }

        // 7. 含む著者の絞り込み (IN)
        if let Some(ref auths_inc) = params.authors_include {
            let active_auths: Vec<String> = auths_inc
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !active_auths.is_empty() {
                let placeholders = vec!["?"; active_auths.len()].join(", ");
                wheres.push(format!("d.author_name IN ({})", placeholders));
                for auth in active_auths {
                    bind_values.push(Box::new(auth));
                }
            }
        }

        // 8. 除外著者の絞り込み (NOT IN)
        if let Some(ref auths_exc) = params.authors_exclude {
            let active_auths: Vec<String> = auths_exc
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !active_auths.is_empty() {
                let placeholders = vec!["?"; active_auths.len()].join(", ");
                wheres.push(format!("d.author_name NOT IN ({})", placeholders));
                for auth in active_auths {
                    bind_values.push(Box::new(auth));
                }
            }
        }

        // 9. 文字数絞り込み (min_char_count)
        if let Some(min_char) = params.min_char_count {
            wheres.push("d.text_length >= ?".to_string());
            bind_values.push(Box::new(min_char));
        }

        // 10. 文字数絞り込み (max_char_count)
        if let Some(max_char) = params.max_char_count {
            wheres.push("d.text_length <= ?".to_string());
            bind_values.push(Box::new(max_char));
        }

        // 11. アセット有無フィルタ
        if let Some(ref asset_filt) = params.asset_filter {
            match asset_filt.as_str() {
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

        // 12. 更新監視フィルタ
        if let Some(ref watch_filt) = params.watch_filter {
            match watch_filt.as_str() {
                "watched" => wheres.push("d.watch_updates = 1".to_string()),
                "unwatched" => wheres.push("d.watch_updates = 0".to_string()),
                _ => {}
            }
        }

        if let (Some(person_source), Some(person_key)) = (&params.person_source, &params.person_key)
        {
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

        if let (Some(series_source), Some(series_key)) = (&params.series_source, &params.series_key)
        {
            if !series_source.trim().is_empty() && !series_key.trim().is_empty() {
                wheres.push(
                    "d.id IN (
                        SELECT download_id FROM download_series
                        WHERE series_source = ? AND series_key = ?
                    )"
                    .to_string(),
                );
                bind_values.push(Box::new(series_source.clone()));
                bind_values.push(Box::new(series_key.clone()));
            }
        }

        // SQLの組み立て
        if !joins.is_empty() {
            sql.push(' ');
            sql.push_str(&joins.join(" "));
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }

        // 並び替えの設定
        let sort_col = match params.sort_by.as_deref() {
            Some("relevance") if has_positive_query => "search.search_score",
            Some("title") => "d.title",
            Some("author") => "d.author_name",
            Some("date") => "d.downloaded_at",
            Some("published") => "COALESCE(d.source_created_at, d.downloaded_at)",
            Some("series_order") => "(SELECT COALESCE(MIN(ds.content_order), 9223372036854775807) FROM download_series ds WHERE ds.download_id = d.id), COALESCE(d.source_created_at, d.downloaded_at)",
            Some("size") => "d.file_size_bytes",
            _ => {
                if params.search_mode.as_deref() == Some("smart") && has_positive_query {
                    "search.search_score"
                } else {
                    "d.downloaded_at"
                }
            }
        };
        let sort_order = match params.sort_order.as_deref() {
            Some("asc") => "ASC",
            _ => "DESC",
        };
        if sort_col == "search.search_score" {
            sql.push_str(
                " ORDER BY search.search_score DESC,
                    d.favorite DESC,
                    COALESCE(d.source_created_at, d.downloaded_at) DESC",
            );
        } else {
            sql.push_str(&format!(" ORDER BY {} {}", sort_col, sort_order));
        }

        // ページネーション設定
        let limit = params.limit.unwrap_or(50);
        let offset = params.offset.unwrap_or(0);
        sql.push_str(" LIMIT ? OFFSET ?");
        bind_values.push(Box::new(limit));
        bind_values.push(Box::new(offset));

        let refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Query prepare failed: {}\nSQL: {}", e, sql))?;
        let rows = stmt
            .query_map(refs.as_slice(), download_entry_from_row)
            .map_err(|e| format!("Query failed: {}", e))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row read failed: {}", e))?);
        }
        drop(stmt);
        if let Some(ref parsed) = parsed_query {
            decorate_search_results_locked(&conn, &mut results, parsed);
        }
        Ok(results)
    }

    /// 単一ダウンロードの取得
    pub fn get_download(&self, id: i64) -> Result<DownloadEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT d.*,
                (SELECT dp.person_key FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1) AS person_id,
                (SELECT dp.display_name FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1) AS person_name,
                (SELECT ds.series_key FROM download_series ds WHERE ds.download_id = d.id LIMIT 1) AS series_id,
                (SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1) AS series_title
             FROM downloads d WHERE d.id = ?1",
            params![id],
            download_entry_from_row,
        )
        .map_err(|e| format!("Download not found: {}", e))
    }

    /// 特定のソースIDのダウンロードが存在するか確認する
    pub fn check_exists(&self, source: &str, source_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
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
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
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

    /// ライブラリのフィルター候補一覧を取得
    pub fn get_filter_facets(&self) -> Result<FilterFacets, String> {
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
        let mut stmt = conn
            .prepare("SELECT * FROM downloads WHERE watch_updates = 1")
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
        let mut stmt = conn
            .prepare(
                "SELECT d.*,
                    (SELECT dp.person_key FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1) AS person_id,
                    (SELECT dp.display_name FROM download_people dp WHERE dp.download_id = d.id ORDER BY CASE dp.role WHEN 'author' THEN 0 WHEN 'creator' THEN 1 ELSE 2 END LIMIT 1) AS person_name,
                    (SELECT ds.series_key FROM download_series ds WHERE ds.download_id = d.id LIMIT 1) AS series_id,
                    (SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1) AS series_title
                 FROM downloads d WHERE d.source = ?1 AND d.source_id = ?2",
            )
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
}

fn stale_search_index_ids_locked(conn: &Connection, limit: i64) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id
             FROM downloads d
             LEFT JOIN download_search_meta m ON m.download_id = d.id
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

fn search_index_status_locked(conn: &Connection) -> Result<SearchIndexStatus, String> {
    let total_downloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
        .map_err(|e| format!("Search index status total failed: {}", e))?;
    let indexed_downloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM download_search_meta", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("Search index status indexed failed: {}", e))?;
    let pending_downloads: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM downloads d
             LEFT JOIN download_search_meta m ON m.download_id = d.id
             WHERE m.download_id IS NULL
                OR m.current_version != d.current_version
                OR COALESCE(m.content_hash, '') != COALESCE(d.content_hash, '')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Search index status pending failed: {}", e))?;

    Ok(SearchIndexStatus {
        total_downloads,
        indexed_downloads,
        pending_downloads,
        is_complete: pending_downloads == 0,
    })
}

fn reindex_download_locked(conn: &Connection, download_id: i64) -> Result<(), String> {
    let row = conn
        .query_row(
            "SELECT d.source, d.title, d.author_name, d.tags, d.excerpt, d.json_path,
                    d.original_json_path, d.current_version, d.content_hash,
                    (SELECT GROUP_CONCAT(ds.title, ' ') FROM download_series ds WHERE ds.download_id = d.id)
             FROM downloads d
             WHERE d.id = ?1",
            params![download_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Search index download query failed: {}", e))?;

    let Some((
        source,
        title,
        author_name,
        tags_raw,
        excerpt,
        json_path,
        original_json_path,
        current_version,
        content_hash,
        series_title,
    )) = row
    else {
        clear_search_index_locked(conn, download_id)?;
        return Ok(());
    };

    let json_path = original_json_path.unwrap_or(json_path);
    let body = std::fs::read_to_string(&json_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .map(|value| extract_search_body(&value, &source))
        .unwrap_or_default();

    let tags = parse_tags_for_search(tags_raw.as_deref()).join(" ");
    let doc = SearchDocument {
        title: normalize_search_text(&title),
        author_name: normalize_search_text(&author_name),
        tags: normalize_search_text(&tags),
        series_title: normalize_search_text(series_title.as_deref().unwrap_or("")),
        excerpt: normalize_search_text(excerpt.as_deref().unwrap_or("")),
        body: normalize_search_text(&body),
    };
    let doc_for_ngrams = doc.clone();

    clear_search_index_locked(conn, download_id)?;
    conn.execute(
        "INSERT INTO download_search_fts (
            download_id, title, author_name, tags, series_title, excerpt, body
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            download_id,
            doc.title,
            doc.author_name,
            doc.tags,
            doc.series_title,
            doc.excerpt,
            doc.body,
        ],
    )
    .map_err(|e| format!("Search FTS insert failed: {}", e))?;

    conn.execute(
        "INSERT INTO download_search_meta (
            download_id, current_version, content_hash, indexed_at
         ) VALUES (?1, ?2, ?3, ?4)",
        params![
            download_id,
            current_version,
            content_hash,
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .map_err(|e| format!("Search meta insert failed: {}", e))?;

    let mut stmt = conn
        .prepare(
            "INSERT OR IGNORE INTO download_search_ngrams (download_id, token, field, weight)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(|e| format!("Search ngram insert prepare failed: {}", e))?;

    for (field, text, weight) in doc_for_ngrams.fields() {
        let max_terms = if field == "body" { 30_000 } else { 4_000 };
        for gram in generate_ngrams_limited(text, max_terms) {
            stmt.execute(params![download_id, gram, field, weight])
                .map_err(|e| format!("Search ngram insert failed: {}", e))?;
        }
    }

    Ok(())
}

fn clear_search_index_locked(conn: &Connection, download_id: i64) -> Result<(), String> {
    conn.execute(
        "DELETE FROM download_search_fts WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Search FTS clear failed: {}", e))?;
    conn.execute(
        "DELETE FROM download_search_ngrams WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Search ngrams clear failed: {}", e))?;
    conn.execute(
        "DELETE FROM download_search_meta WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Search meta clear failed: {}", e))?;
    Ok(())
}

fn load_search_document_locked(
    conn: &Connection,
    download_id: i64,
) -> Result<Option<SearchDocument>, String> {
    conn.query_row(
        "SELECT title, author_name, tags, series_title, excerpt, body
         FROM download_search_fts
         WHERE download_id = ?1
         LIMIT 1",
        params![download_id],
        |row| {
            Ok(SearchDocument {
                title: row.get(0)?,
                author_name: row.get(1)?,
                tags: row.get(2)?,
                series_title: row.get(3)?,
                excerpt: row.get(4)?,
                body: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("Search document load failed: {}", e))
}

fn decorate_search_results_locked(
    conn: &Connection,
    results: &mut [DownloadEntry],
    parsed: &ParsedSearchQuery,
) {
    if parsed.include.is_empty() && parsed.exclude.is_empty() {
        return;
    }

    for entry in results {
        let Ok(Some(doc)) = load_search_document_locked(conn, entry.id) else {
            continue;
        };
        let (fields, computed_score) = match_fields_and_score(&doc, parsed);
        if !fields.is_empty() {
            entry.match_fields = fields;
        }
        if entry.search_score.unwrap_or(0.0) <= 0.0 && computed_score > 0.0 {
            entry.search_score = Some(computed_score);
        }
        entry.match_snippet = make_match_snippet(&doc, parsed);
    }
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

fn download_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadEntry> {
    Ok(DownloadEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_id: row.get(2)?,
        title: row.get(3)?,
        author_name: row.get(4)?,
        author_id: row.get(5)?,
        content_type: row.get(6)?,
        tags: row.get(7)?,
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
        match_snippet: row.get(27).ok(),
        match_fields: row
            .get::<_, Option<String>>(28)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default(),
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

    fn temp_paths() -> (PathBuf, PathBuf) {
        let rand_val: u32 = rand::random();
        let root = std::env::temp_dir().join(format!("piep_search_test_{}", rand_val));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        (root, storage)
    }

    fn params(query: &str) -> SearchParams {
        SearchParams {
            query: Some(query.to_string()),
            source: None,
            content_type: None,
            sort_by: Some("relevance".to_string()),
            sort_order: Some("desc".to_string()),
            limit: Some(20),
            offset: Some(0),
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
            search_mode: Some("smart".to_string()),
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
            tags: Some(serde_json::to_string(tags).unwrap()),
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
        db.reindex_download(id).unwrap();
        id
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

        let results = db.search_downloads(&params("秘密キーワード")).unwrap();
        assert_eq!(results.first().map(|dl| dl.id), Some(title_hit));
        assert!(results.iter().any(|dl| dl.id == body_only));
        assert!(results
            .iter()
            .find(|dl| dl.id == body_only)
            .and_then(|dl| dl.match_snippet.as_ref())
            .is_some());

        let partial = db.search_downloads(&params("秘密キ")).unwrap();
        assert!(partial.iter().any(|dl| dl.id == body_only));

        let excluded = db.search_downloads(&params("秘密 -タイトル")).unwrap();
        assert!(excluded.iter().any(|dl| dl.id == body_only));
        assert!(!excluded.iter().any(|dl| dl.id == title_hit));

        let _ = fs::remove_dir_all(root);
    }
}
