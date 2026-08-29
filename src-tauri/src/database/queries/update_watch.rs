//! 更新監視の問い合わせ。
//!
//! `mod.rs` から切り出した。監視の対象（作者・シリーズ・作品）と、確認が
//! 見つけた候補。**監視の単位は作者・シリーズであって作品ではない**という
//! 決めごとがここに現れる（`docs/policy/06-backend.md`）。
//!
//! `impl Database` は複数のファイルに分けて書けるので、公開 API は変えずに
//! 置き場所だけを移している。

use super::{
    update_target_from_row, Database, PendingRevision, UpdateCandidateInput, UpdateCandidateRow,
    UpdateTarget, UpdateTargetInput,
};
use rusqlite::{params, OptionalExtension};

impl Database {
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
        self.find_update_target(target_type, source, source_key)?
            .ok_or_else(|| "Update target not found".to_string())
    }

    /// Looks up one update target by its composite key without requiring the
    /// caller to fetch and deserialize the complete target list.
    pub fn find_update_target(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<Option<UpdateTarget>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM update_targets WHERE target_type = ?1 AND source = ?2 AND source_key = ?3",
            params![target_type, source, source_key],
            update_target_from_row,
        )
        .optional()
        .map_err(|e| format!("Failed to query update target: {e}"))
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

    /// Pages every update target in composite-key order. Archive export uses
    /// this instead of materializing the complete table in one metadata blob.
    pub fn list_update_targets_after(
        &self,
        cursor: Option<(&str, &str, &str)>,
        limit: i64,
    ) -> Result<Vec<UpdateTarget>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 5_000);
        let (sql, bind_values): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match cursor {
            Some((target_type, source, source_key)) => (
                "SELECT * FROM update_targets
                 WHERE target_type > ?1
                    OR (target_type = ?1 AND source > ?2)
                    OR (target_type = ?1 AND source = ?2 AND source_key > ?3)
                 ORDER BY target_type ASC, source ASC, source_key ASC
                 LIMIT ?4",
                vec![
                    Box::new(target_type.to_string()),
                    Box::new(source.to_string()),
                    Box::new(source_key.to_string()),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT * FROM update_targets
                 ORDER BY target_type ASC, source ASC, source_key ASC
                 LIMIT ?1",
                vec![Box::new(limit)],
            ),
        };
        let refs = bind_values
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>();
        let mut statement = conn
            .prepare(sql)
            .map_err(|e| format!("Update target page prepare failed: {e}"))?;
        let targets = statement
            .query_map(refs.as_slice(), update_target_from_row)
            .map_err(|e| format!("Update target page query failed: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Update target page row failed: {e}"))?;
        Ok(targets)
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

    /// 対象を確認できたことを記録する。
    ///
    /// `found` が 0 より大きいときだけ「最後に見つけた時刻」を進める。ここが
    /// 何か月も古いままの対象は、監視をやめるか間隔を空ける判断の材料になる。
    /// 確認できた時点で連続失敗は 0 に戻す。
    pub fn mark_update_target_checked(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        last_seen_source_id: Option<&str>,
        last_seen_source_updated_at: Option<&str>,
        found: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE update_targets SET
                last_checked_at = ?1,
                last_seen_source_id = COALESCE(?2, last_seen_source_id),
                last_seen_source_updated_at = COALESCE(?3, last_seen_source_updated_at),
                last_hit_at = CASE WHEN ?7 > 0 THEN ?1 ELSE last_hit_at END,
                consecutive_errors = 0,
                updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?4 AND source = ?5 AND source_key = ?6",
            params![
                now,
                last_seen_source_id,
                last_seen_source_updated_at,
                target_type,
                source,
                source_key,
                found,
            ],
        )
        .map_err(|e| format!("Failed to mark update target checked: {}", e))?;
        Ok(())
    }

    /// 対象の確認に失敗したことを記録する。連続失敗が積み上がる。
    pub fn mark_update_target_failed(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_targets SET
                last_checked_at = ?1,
                consecutive_errors = consecutive_errors + 1,
                updated_at = CURRENT_TIMESTAMP
             WHERE target_type = ?2 AND source = ?3 AND source_key = ?4",
            params![
                chrono::Utc::now().to_rfc3339(),
                target_type,
                source,
                source_key,
            ],
        )
        .map_err(|e| format!("Failed to mark update target failed: {}", e))?;
        Ok(())
    }

    /// 見つけた候補を、ジョブより長く残す。
    ///
    /// すでに無視すると決めた作品は `pending` に戻さない。同じ作品が
    /// 何度見つかっても、決めた答えの方が新しい。
    pub fn upsert_update_candidate(&self, candidate: &UpdateCandidateInput) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_candidates (
                source, source_id, kind, title, payload_json, target_type, status, first_seen_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)
             ON CONFLICT(source, source_id) DO UPDATE SET
                kind = excluded.kind,
                title = excluded.title,
                payload_json = excluded.payload_json,
                target_type = excluded.target_type,
                updated_at = excluded.updated_at",
            params![
                candidate.source,
                candidate.source_id,
                candidate.kind,
                candidate.title,
                candidate.payload_json,
                candidate.target_type,
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Failed to record update candidate: {}", e))?;
        Ok(())
    }

    /// まだ answer の出ていない候補。新しく見つけた順に返す。
    /// まだ取り込んでいない改稿がある作品の鍵。`{source}:{sourceId}` の並び。
    ///
    /// 「取得元のほうが新しい」という状態は、作品の属性ではなく更新側が
    /// 見つけた事実である。だから `downloads` に列を足さず、ここから引く。
    /// 改稿を見つけたとき基準値をわざと書き換えないのも同じ理由で、
    /// **取り直して初めて追いついたと言える。**
    ///
    /// 返るのは未処理のものだけなので、ふつうは空か、ごく少ない。
    pub fn pending_revision_keys(&self) -> Result<Vec<PendingRevision>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT d.id, c.updated_at
                   FROM update_candidates c
                   JOIN downloads d
                     ON d.source = c.source AND d.source_id = c.source_id
                  WHERE c.status = 'pending' AND c.kind = 'revision'",
            )
            .map_err(|e| format!("Failed to prepare revision keys: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PendingRevision {
                    download_id: row.get(0)?,
                    found_at: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read revision keys: {}", e))
    }

    pub fn list_pending_update_candidates(
        &self,
        limit: i64,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT source, source_id, kind, title, payload_json, target_type, status,
                        first_seen_at, updated_at
                 FROM update_candidates
                 WHERE status = 'pending'
                 ORDER BY updated_at DESC, rowid DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare update candidates: {}", e))?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(UpdateCandidateRow {
                    source: row.get(0)?,
                    source_id: row.get(1)?,
                    kind: row.get(2)?,
                    title: row.get(3)?,
                    payload_json: row.get(4)?,
                    target_type: row.get(5)?,
                    status: row.get(6)?,
                    first_seen_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|e| format!("Failed to read update candidates: {}", e))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("Failed to read update candidate: {}", e))?);
        }
        Ok(items)
    }

    /// Pages every durable candidate in primary-key order for portable
    /// backups. Dismissed rows are included because they record an explicit
    /// user decision and prevent an already-rejected work from reappearing.
    pub fn list_update_candidates_after(
        &self,
        cursor: Option<(&str, &str)>,
        limit: i64,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        let conn = self.read_conn()?;
        let limit = limit.clamp(1, 5_000);
        let (sql, bind_values): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match cursor {
            Some((source, source_id)) => (
                "SELECT source, source_id, kind, title, payload_json, target_type, status,
                        first_seen_at, updated_at
                   FROM update_candidates
                  WHERE source > ?1 OR (source = ?1 AND source_id > ?2)
                  ORDER BY source ASC, source_id ASC
                  LIMIT ?3",
                vec![
                    Box::new(source.to_string()),
                    Box::new(source_id.to_string()),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT source, source_id, kind, title, payload_json, target_type, status,
                        first_seen_at, updated_at
                   FROM update_candidates
                  ORDER BY source ASC, source_id ASC
                  LIMIT ?1",
                vec![Box::new(limit)],
            ),
        };
        let refs = bind_values
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>();
        let mut statement = conn
            .prepare(sql)
            .map_err(|e| format!("Update candidate page prepare failed: {e}"))?;
        let rows = statement
            .query_map(refs.as_slice(), |row| {
                Ok(UpdateCandidateRow {
                    source: row.get(0)?,
                    source_id: row.get(1)?,
                    kind: row.get(2)?,
                    title: row.get(3)?,
                    payload_json: row.get(4)?,
                    target_type: row.get(5)?,
                    status: row.get(6)?,
                    first_seen_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|e| format!("Update candidate page query failed: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Update candidate page row failed: {e}"))
    }

    pub fn restore_update_candidate(&self, candidate: &UpdateCandidateRow) -> Result<(), String> {
        if !matches!(candidate.source.as_str(), "pixiv" | "fanbox")
            || candidate.source_id.trim().is_empty()
            || candidate.source_id.len() > 128
        {
            return Err("Backup update candidate has an invalid source key".to_string());
        }
        if !matches!(candidate.kind.as_str(), "new" | "sequel" | "revision") {
            return Err(format!(
                "Backup update candidate has an unsupported kind: {}",
                candidate.kind
            ));
        }
        if !matches!(candidate.status.as_str(), "pending" | "dismissed") {
            return Err(format!(
                "Backup update candidate has an unsupported status: {}",
                candidate.status
            ));
        }
        if candidate.title.chars().count() > 2_000
            || serde_json::from_str::<serde_json::Value>(&candidate.payload_json).is_err()
        {
            return Err("Backup update candidate has invalid content".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO update_candidates (
                source, source_id, kind, title, payload_json, target_type, status,
                first_seen_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(source, source_id) DO UPDATE SET
                kind = excluded.kind,
                title = excluded.title,
                payload_json = excluded.payload_json,
                target_type = excluded.target_type,
                status = excluded.status,
                first_seen_at = excluded.first_seen_at,
                updated_at = excluded.updated_at",
            params![
                candidate.source,
                candidate.source_id,
                candidate.kind,
                candidate.title,
                candidate.payload_json,
                candidate.target_type,
                candidate.status,
                candidate.first_seen_at,
                candidate.updated_at,
            ],
        )
        .map_err(|e| format!("Failed to restore update candidate: {e}"))?;
        Ok(())
    }

    /// この作品を今後は候補に出さない。決定は取り消せる。
    pub fn set_update_candidate_status(
        &self,
        source: &str,
        source_id: &str,
        status: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_candidates SET status = ?3, updated_at = ?4
             WHERE source = ?1 AND source_id = ?2",
            params![source, source_id, status, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to set update candidate status: {}", e))?;
        Ok(())
    }

    /// 答えの出た候補を片付ける（保存できた、あるいは無視の取り消しで再提示）。
    pub fn clear_update_candidate(&self, source: &str, source_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM update_candidates WHERE source = ?1 AND source_id = ?2",
            params![source, source_id],
        )
        .map_err(|e| format!("Failed to clear update candidate: {}", e))?;
        Ok(())
    }

    pub fn update_candidate_status(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<String>, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT status FROM update_candidates WHERE source = ?1 AND source_id = ?2",
            params![source, source_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read update candidate status: {}", e))
    }

    /// 無視した件数と、いちばん新しいもの。画面の「無視した作品」の表示に使う。
    pub fn count_dismissed_update_candidates(&self) -> Result<i64, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM update_candidates WHERE status = 'dismissed'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count dismissed candidates: {}", e))
    }

    /// 無視の決定をすべて取り消す。次の確認でまた候補に出る。
    pub fn restore_dismissed_update_candidates(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE update_candidates SET status = 'pending', updated_at = ?1
             WHERE status = 'dismissed'",
            params![chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to restore dismissed candidates: {}", e))
    }
}
