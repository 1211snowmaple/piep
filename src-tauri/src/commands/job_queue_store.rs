//! 棚の実行キューの読み書き。項目の原入力は棚の圧縮ファイル、保存結果は作品 UUID に結ぶ。
//! ワーカーと画面は同じ台帳を使う。
use crate::library::{EntityKind, Fingerprint, Library, LibraryId};
use crate::models::{
    StartUpdateJobRequest, UpdateJobCandidate, UpdateJobItem, UpdateJobItemInput,
    UpdateJobItemState, UpdateJobLog, UpdateJobProgressDelta, UpdateJobSnapshot, UpdateJobSummary,
    UpdateWorkMetadata,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::Mutex;
const UPDATE_SNAPSHOT_CANDIDATE_PAGE_SIZE: i64 = 200;
const UPDATE_SNAPSHOT_LOG_PAGE_SIZE: i64 = 300;
/// 選んだ候補の保存に移ったため、確かめずに残した作者・作品の印（項目の `error` に入れる）。
/// 探す段階の進み具合はこれを数えない（要約の SQL に同じ文字列がある。試験で突き合わせる）。
const UNCHECKED_BEFORE_SAVE: &str = "選んだ候補の保存に移ったため、確かめていません";
fn job_bytes(value: impl AsRef<str>) -> Result<Vec<u8>, String> {
    value
        .as_ref()
        .parse::<LibraryId>()
        .map(|v| v.as_bytes().to_vec())
        .map_err(|e| e.to_string())
}
fn uuid_text(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    let bytes: Vec<u8> = row.get(index)?;
    LibraryId::from_slice(&bytes)
        .map(|v| v.to_string())
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Blob,
                Box::new(e),
            )
        })
}
pub(crate) struct Queue<'a> {
    pub(crate) library: &'a Library,
    pub(crate) conn: &'a Mutex<Connection>,
}
impl Queue<'_> {
    pub(super) fn saved_metadata(
        &self,
        source: Option<&str>,
        key: Option<&str>,
        metadata: &mut UpdateWorkMetadata,
    ) -> rusqlite::Result<Option<LibraryId>> {
        let (Some(source), Some(key)) = (source, key) else {
            return Ok(None);
        };
        let result = super::super::update_jobs::shelf::saved_card(self.library, source, key)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Blob,
                    Box::new(std::io::Error::other(e)),
                )
            })?;
        if let Some(card) = result {
            metadata.author_name = metadata.author_name.take().or(Some(card.author_name));
            let date = |v: i64| chrono::DateTime::from_timestamp_millis(v).map(|d| d.to_rfc3339());
            metadata.published_at = metadata
                .published_at
                .take()
                .or(card.published_at.and_then(date));
            metadata.updated_at = metadata
                .updated_at
                .take()
                .or(card.updated_at.and_then(date));
            metadata.saved_at = date(card.saved_at);
            return Ok(Some(card.id));
        }
        Ok(None)
    }
    fn read_conn(&self) -> Result<impl std::ops::Deref<Target = Connection> + '_, String> {
        self.library.reader().map_err(|e| e.to_string())
    }
    fn payload(&self, conn: &Connection, bytes: Vec<u8>) -> rusqlite::Result<String> {
        let hash = Fingerprint(
            bytes
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        );
        self.library.read_job_payload(conn, hash).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(e))
        })
    }
    fn item_from_row(
        &self,
        conn: &Connection,
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<UpdateJobItem> {
        let mut payload = self.payload(conn, row.get(7)?)?;
        if let Some(generation) = row.get::<_, Option<i64>>(11)? {
            let mut value: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })?;
            if value.get("kind").and_then(serde_json::Value::as_str) != Some("permission_check") {
                return Err(rusqlite::Error::InvalidQuery);
            }
            value
                .as_object_mut()
                .ok_or(rusqlite::Error::InvalidQuery)?
                .insert("discoveryGeneration".into(), serde_json::json!(generation));
            payload = value.to_string();
        }
        Ok(UpdateJobItem {
            id: row.get(0)?,
            item_type: row.get(2)?,
            source: row.get(3)?,
            source_id: row.get(4)?,
            target_type: row.get(5)?,
            title: row.get(6)?,
            payload_json: payload,
        })
    }
    pub fn recover_update_jobs_on_startup(&self) -> Result<(), String> {
        let mut guard = self.conn.lock().map_err(|e| e.to_string())?;
        let conn = guard.transaction().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE execution_jobs
             SET status = 'paused',
                 active_label = '前回の起動中に中断されました',
                 updated_at = ?1,
                 finished_at = NULL
             WHERE status IN ('running', 'queued', 'canceling')",
            params![chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("Failed to recover update jobs: {}", e))?;
        conn.execute(
            "UPDATE execution_items
             SET status = 'queued', updated_at = CURRENT_TIMESTAMP
             WHERE status = 'running'",
            [],
        )
        .map_err(|e| format!("Failed to recover update job items: {}", e))?;
        conn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn update_job_watch_saved(&self, job_id: &str) -> Result<bool, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT watch_saved FROM execution_jobs WHERE id = ?1",
            params![job_bytes(job_id)?],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|e| format!("Update job not found: {}", e))
    }

    pub fn update_job_new_works_only(&self, job_id: &str) -> Result<bool, String> {
        let conn = self.read_conn()?;
        conn.query_row(
            "SELECT new_works_only FROM execution_jobs WHERE id = ?1",
            params![job_bytes(job_id)?],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|e| format!("Update job not found: {}", e))
    }

    pub fn prune_update_jobs(&self, keep_count: i64, keep_days: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(keep_days)).to_rfc3339();
        let removed = conn
            .execute(
                "DELETE FROM execution_jobs
                 WHERE status IN ('completed', 'failed', 'canceled')
                   AND COALESCE(finished_at, updated_at) < ?1
                   AND id NOT IN (
                       SELECT id FROM execution_jobs ORDER BY updated_at DESC LIMIT ?2
                   )",
                params![cutoff, keep_count],
            )
            .map_err(|e| format!("Failed to prune update jobs: {}", e))?;
        Ok(removed)
    }

    pub fn clear_finished_update_jobs(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM execution_jobs WHERE status IN ('completed', 'failed', 'canceled')",
            [],
        )
        .map_err(|e| format!("Failed to clear finished update jobs: {}", e))
    }

    pub fn trim_update_job_logs(&self, job_id: &str, keep: i64) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute(
                "DELETE FROM execution_logs
                 WHERE job_id = ?1 AND id NOT IN (
                     SELECT id FROM execution_logs WHERE job_id = ?1 ORDER BY id DESC LIMIT ?2
                 )",
                params![job_bytes(job_id)?, keep],
            )
            .map_err(|e| format!("Failed to trim update job logs: {}", e))?;
        Ok(removed)
    }

    pub fn list_update_jobs(&self) -> Result<Vec<UpdateJobSummary>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at,
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.status='held' AND i.known_hold=0),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type IN ('work','target') AND i.error IS NOT '選んだ候補の保存に移ったため、確かめていません'),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type IN ('work','target') AND i.error IS NOT '選んだ候補の保存に移ったため、確かめていません' AND i.status NOT IN ('queued','running')),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status != 'candidate'),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status NOT IN ('candidate','queued','running')),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.status='held' AND i.known_hold=1),
                        new_works_only
                 FROM execution_jobs
                 ORDER BY updated_at DESC, started_at DESC
                 LIMIT 30",
            )
            .map_err(|e| format!("Failed to prepare update job list: {}", e))?;
        let rows = stmt
            .query_map([], update_job_summary_from_row)
            .map_err(|e| format!("Failed to list update jobs: {}", e))?;
        let mut jobs = Vec::new();
        for row in rows {
            let mut job = row.map_err(|e| format!("Failed to read update job: {}", e))?;
            // History needs a subject, while live progress events must stay cheap.
            // An ad hoc author check can contain many works but one target.
            let subject =
                |item_type: &str| -> Result<(Option<String>, Option<String>, i64), String> {
                    conn.query_row(
                        "SELECT CASE WHEN COUNT(*) = 1 THEN MIN(title) END,
                            CASE WHEN COUNT(*) = 1 THEN MIN(source) END, COUNT(*)
                     FROM execution_items WHERE job_id = ?1 AND item_type = ?2",
                        params![job_bytes(&job.job_id)?, item_type],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(|e| format!("Failed to read update job subject: {e}"))
                };
            let (label, source, count) = subject("target")?;
            let (label, source) = if count == 0 {
                let (label, source, _) = subject("work")?;
                (label, source)
            } else {
                (label, source)
            };
            job.subject_label = label;
            job.subject_source = source;
            jobs.push(job);
        }
        Ok(jobs)
    }

    pub fn update_job_snapshot(&self, job_id: &str) -> Result<UpdateJobSnapshot, String> {
        self.update_job_snapshot_page(job_id, None, None)
    }

    pub fn update_job_snapshot_page(
        &self,
        job_id: &str,
        candidate_after_id: Option<i64>,
        log_before_id: Option<i64>,
    ) -> Result<UpdateJobSnapshot, String> {
        self.sync_update_job_counters(job_id)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let summary = conn
            .query_row(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at,
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.status='held' AND i.known_hold=0),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type IN ('work','target') AND i.error IS NOT '選んだ候補の保存に移ったため、確かめていません'),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type IN ('work','target') AND i.error IS NOT '選んだ候補の保存に移ったため、確かめていません' AND i.status NOT IN ('queued','running')),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status != 'candidate'),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status NOT IN ('candidate','queued','running')),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.status='held' AND i.known_hold=1),
                        new_works_only
                 FROM execution_jobs WHERE id = ?1",
                params![job_bytes(job_id)?],
                update_job_summary_from_row,
            )
            .map_err(|e| format!("Update job not found: {}", e))?;

        let mut log_stmt = conn
            .prepare(
                "SELECT id, log_type, message, created_at
                 FROM (
                    SELECT id, log_type, message, created_at
                    FROM execution_logs
                    WHERE job_id = ?1 AND (?2 IS NULL OR id < ?2)
                    ORDER BY id DESC
                    LIMIT ?3
                 ) ORDER BY id ASC",
            )
            .map_err(|e| format!("Failed to prepare update job logs: {}", e))?;
        let log_rows = log_stmt
            .query_map(
                params![
                    job_bytes(job_id)?,
                    log_before_id,
                    UPDATE_SNAPSHOT_LOG_PAGE_SIZE + 1
                ],
                |row| {
                    Ok(UpdateJobLog {
                        id: row.get(0)?,
                        log_type: row.get(1)?,
                        message: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("Failed to read update job logs: {}", e))?;
        let mut logs = Vec::new();
        for row in log_rows {
            logs.push(row.map_err(|e| format!("Failed to read update job log: {}", e))?);
        }
        let previous_log_cursor = if logs.len() as i64 > UPDATE_SNAPSHOT_LOG_PAGE_SIZE {
            logs.remove(0);
            logs.first().map(|log| log.id)
        } else {
            None
        };

        let mut candidate_stmt = conn
            .prepare(
                "SELECT i.id, i.source, i.source_id, i.title, i.target_type, i.display_json, i.status, i.error, NULL, NULL, NULL, NULL, NULL
                 FROM execution_items i
                 WHERE i.job_id = ?1 AND i.item_type = 'candidate' AND i.id > ?2
                 ORDER BY i.id ASC
                 LIMIT ?3",
            )
            .map_err(|e| format!("Failed to prepare update job candidates: {}", e))?;
        let candidate_rows = candidate_stmt
            .query_map(
                params![
                    job_bytes(job_id)?,
                    candidate_after_id.unwrap_or(0),
                    UPDATE_SNAPSHOT_CANDIDATE_PAGE_SIZE + 1
                ],
                |row| {
                    let id: i64 = row.get(0)?;
                    let source: String = row.get(1)?;
                    let source_id: String = row.get(2)?;
                    let title: String = row.get(3)?;
                    // v0.11.0 で作られたまとめ保存ジョブは target_type を NULL で
                    // 記録していた。作品を直接保存する候補なので `work` として読み、
                    // 既存ジョブも更新後すぐ再開・表示できるようにする。
                    let target_type: Option<String> = row.get(4)?;
                    let payload_json: String = row.get(5)?;
                    let status: String = row.get(6)?;
                    let error: Option<String> = row.get(7)?;
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
                    // 古いジョブの候補には kind が無い。既定は「新作」。
                    let kind = payload
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("new")
                        .to_string();
                    let mut metadata =
                        serde_json::from_value(payload["metadata"].clone()).unwrap_or_default();
                    let work_id =
                        self.saved_metadata(Some(&source), Some(&source_id), &mut metadata)?;
                    Ok(UpdateJobCandidate {
                        metadata,
                        work_id,
                        id,
                        key: format!("candidate:{}:{}:{}", source, source_id, id),
                        source,
                        source_id,
                        title,
                        subtitle,
                        target_label,
                        target_type: target_type.unwrap_or_else(|| "work".to_string()),
                        selected: matches!(status.as_str(), "candidate" | "queued"),
                        status,
                        kind,
                        error: error.or_else(|| {
                            payload
                                .get("holdReason")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                        }),
                    })
                },
            )
            .map_err(|e| format!("Failed to read update job candidates: {}", e))?;
        let mut candidates = Vec::new();
        for row in candidate_rows {
            candidates
                .push(row.map_err(|e| format!("Failed to read update job candidate: {}", e))?);
        }
        let next_candidate_cursor = if candidates.len() as i64 > UPDATE_SNAPSHOT_CANDIDATE_PAGE_SIZE
        {
            candidates.pop();
            candidates.last().map(|candidate| candidate.id)
        } else {
            None
        };

        Ok(UpdateJobSnapshot {
            job_id: summary.job_id,
            status: summary.status,
            scope: summary.scope,
            mode: summary.mode,
            totals: summary.totals,
            processed: summary.processed,
            check_total: summary.check_total,
            check_processed: summary.check_processed,
            save_total: summary.save_total,
            save_processed: summary.save_processed,
            candidate_count: summary.candidate_count,
            saved_count: summary.saved_count,
            error_count: summary.error_count,
            held_count: summary.held_count,
            known_held_count: summary.known_held_count,
            new_works_only: summary.new_works_only,
            active_label: summary.active_label,
            logs,
            candidates,
            next_candidate_cursor,
            previous_log_cursor,
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
            "UPDATE execution_jobs
             SET status = ?1,
                 active_label = ?2,
                 updated_at = ?3,
                 finished_at = CASE WHEN ?4 THEN COALESCE(finished_at, ?3) ELSE NULL END
             WHERE id = ?5",
            params![status, active_label, now, terminal, job_bytes(job_id)?],
        )
        .map_err(|e| format!("Failed to update job status: {}", e))?;
        Ok(())
    }

    pub fn transition_update_job_status(
        &self,
        job_id: &str,
        expected_status: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let terminal = matches!(
            status,
            "completed" | "failed" | "canceled" | "auth_required"
        );
        let changed = conn
            .execute(
                "UPDATE execution_jobs
                 SET status = ?1,
                     active_label = ?2,
                     updated_at = ?3,
                     finished_at = CASE WHEN ?4 THEN COALESCE(finished_at, ?3) ELSE NULL END
                 WHERE id = ?5 AND status = ?6",
                params![
                    status,
                    active_label,
                    now,
                    terminal,
                    job_bytes(job_id)?,
                    expected_status
                ],
            )
            .map_err(|e| format!("Failed to transition job status: {e}"))?;
        Ok(changed > 0)
    }

    pub fn prepare_update_job_resume(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        self.prepare_update_job_resume_checked(job_id, retry_failed, false)
    }

    pub fn resume_update_job_if_idle(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        self.prepare_update_job_resume_checked(job_id, retry_failed, true)
    }

    fn prepare_update_job_resume_checked(
        &self,
        job_id: &str,
        retry_failed: bool,
        activate: bool,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start update job resume transaction: {e}"))?;
        if activate {
            let current: String = tx
                .query_row(
                    "SELECT status FROM execution_jobs WHERE id = ?1",
                    params![job_bytes(job_id)?],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Update job not found: {e}"))?;
            if !matches!(
                current.as_str(),
                "paused" | "auth_required" | "failed" | "canceled"
            ) {
                return Err("この更新ジョブは再開できません".to_string());
            }
            let other_active: i64 = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM execution_jobs
                     WHERE id != ?1 AND status IN ('queued', 'running', 'canceling'))",
                    params![job_bytes(job_id)?],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to check active update jobs: {e}"))?;
            if other_active != 0 {
                return Err("別の更新または保存ジョブが進行中です".to_string());
            }
        }
        tx.execute(
            "UPDATE execution_items
             SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE job_id = ?1 AND status = 'running'",
            params![job_bytes(job_id)?],
        )
        .map_err(|e| format!("Failed to reset running update items: {}", e))?;
        if retry_failed {
            let reset_count = tx
                .execute(
                    "UPDATE execution_items
                 SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE job_id = ?1 AND status = 'failed'",
                    params![job_bytes(job_id)?],
                )
                .map_err(|e| format!("Failed to reset failed update items: {}", e))?;
            if reset_count > 0 {
                tx.execute(
                    "UPDATE execution_jobs
                     SET processed = MAX(0, processed - ?2),
                         error_count = MAX(0, error_count - ?2),
                         updated_at = ?3
                     WHERE id = ?1",
                    params![
                        job_bytes(job_id)?,
                        reset_count as i64,
                        chrono::Utc::now().to_rfc3339()
                    ],
                )
                .map_err(|e| format!("Failed to reset retry counters: {e}"))?;
            }
        }
        if activate {
            tx.execute(
                "UPDATE execution_jobs
                 SET status = 'queued', active_label = '再開待ち',
                     updated_at = ?2, finished_at = NULL
                 WHERE id = ?1",
                params![job_bytes(job_id)?, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(|e| format!("Failed to queue resumed update job: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit update job resume: {e}"))?;
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
            "INSERT INTO execution_logs (job_id, log_type, message)
             VALUES (?1, ?2, ?3)",
            params![job_bytes(job_id)?, log_type, message],
        )
        .map_err(|e| format!("Failed to append update job log: {}", e))?;
        // Snapshots expose only the latest 300 records. Keeping a bounded
        // diagnostic tail prevents a long-running monitor from growing this
        // derived log table without limit.
        conn.execute(
            "DELETE FROM execution_logs
             WHERE job_id = ?1 AND id NOT IN (
                SELECT id FROM execution_logs
                WHERE job_id = ?1 ORDER BY id DESC LIMIT 2000
             )",
            params![job_bytes(job_id)?],
        )
        .map_err(|e| format!("Failed to trim update job logs: {}", e))?;
        conn.execute(
            "UPDATE execution_jobs SET updated_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), job_bytes(job_id)?],
        )
        .map_err(|e| format!("Failed to touch update job: {}", e))?;
        Ok(())
    }

    pub fn update_job_status_value(&self, job_id: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT status FROM execution_jobs WHERE id = ?1",
            params![job_bytes(job_id)?],
            |row| row.get(0),
        )
        .map_err(|e| format!("Update job not found: {}", e))
    }

    pub fn update_job_mode_value(&self, job_id: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT mode FROM execution_jobs WHERE id = ?1",
            params![job_bytes(job_id)?],
            |row| row.get(0),
        )
        .map_err(|e| format!("Update job not found: {}", e))
    }

    pub fn update_job_progress_delta(
        &self,
        job_id: &str,
        changed_item_id: Option<i64>,
    ) -> Result<UpdateJobProgressDelta, String> {
        let conn = self.read_conn()?;
        let summary = conn
            .query_row(
                "SELECT id, status, scope, mode, totals, processed, candidate_count,
                        saved_count, error_count, active_label, started_at, updated_at, finished_at,
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.status='held' AND i.known_hold=0),
                        totals - (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status != 'candidate')
                               - (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type != 'candidate' AND i.error IS '選んだ候補の保存に移ったため、確かめていません'),
                        processed - (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status NOT IN ('candidate','queued','running'))
                                  - (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type != 'candidate' AND i.error IS '選んだ候補の保存に移ったため、確かめていません'),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status != 'candidate'),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.item_type='candidate' AND i.status NOT IN ('candidate','queued','running')),
                        (SELECT COUNT(*) FROM execution_items i WHERE i.job_id=execution_jobs.id AND i.status='held' AND i.known_hold=1),
                        new_works_only
                 FROM execution_jobs WHERE id = ?1",
                params![job_bytes(job_id)?],
                update_job_summary_from_row,
            )
            .map_err(|e| format!("Update job not found: {e}"))?;
        let changed_item = match changed_item_id {
            Some(item_id) => conn
                .query_row(
                    "SELECT source, source_id, status, error
                     FROM execution_items WHERE id = ?1 AND job_id = ?2",
                    params![item_id, job_bytes(job_id)?],
                    |row| {
                        Ok(UpdateJobItemState {
                            work_id: None,
                            metadata: Default::default(),
                            source: row.get(0)?,
                            source_id: row.get(1)?,
                            status: row.get(2)?,
                            error: row.get(3)?,
                            item_type: None,
                            title: None,
                        })
                    },
                )
                .optional()
                .map_err(|e| format!("Failed to read changed update item: {e}"))?,
            None => None,
        };
        let latest_log = conn
            .query_row(
                "SELECT id, log_type, message, created_at
                 FROM execution_logs WHERE job_id = ?1 ORDER BY id DESC LIMIT 1",
                params![job_bytes(job_id)?],
                |row| {
                    Ok(UpdateJobLog {
                        id: row.get(0)?,
                        log_type: row.get(1)?,
                        message: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("Failed to read latest update log: {e}"))?;
        Ok(UpdateJobProgressDelta {
            summary,
            changed_item,
            latest_log,
        })
    }

    pub fn next_update_job_item(&self, job_id: &str) -> Result<Option<UpdateJobItem>, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        // Claiming an item and deciding whether the worker may still run are
        // one operation. A pause/cancel can arrive after the worker's outer
        // status check, so checking only there would let one fresh network or
        // save operation start after the stop request was already recorded.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("Failed to begin update item claim: {e}"))?;
        let item = tx
            .query_row(
                "SELECT id, job_id, item_type, source, source_id, target_type, title,
                        payload_file_id, status, error, NULL, resume_generation
                 FROM execution_items i
                 WHERE i.job_id = ?1 AND i.status = 'queued'
                   AND EXISTS (
                       SELECT 1 FROM execution_jobs j
                       WHERE j.id = i.job_id AND j.status IN ('queued', 'running')
                   )
                 ORDER BY CASE item_type WHEN 'work' THEN 0 WHEN 'target' THEN 1 ELSE 2 END,
                          i.id ASC
                 LIMIT 1",
                params![job_bytes(job_id)?],
                |row| self.item_from_row(&tx, row),
            )
            .optional()
            .map_err(|e| format!("Failed to fetch next update item: {}", e))?;
        if let Some(item) = item {
            let claimed = tx
                .execute(
                    "UPDATE execution_items
                     SET status = 'running', updated_at = CURRENT_TIMESTAMP
                     WHERE id = ?1 AND status = 'queued'
                       AND EXISTS (
                           SELECT 1 FROM execution_jobs
                           WHERE id = ?2 AND status IN ('queued', 'running')
                       )",
                    params![item.id, job_bytes(job_id)?],
                )
                .map_err(|e| format!("Failed to mark update item running: {}", e))?;
            if claimed == 0 {
                tx.commit()
                    .map_err(|e| format!("Failed to finish empty update item claim: {e}"))?;
                return Ok(None);
            }
            let job_claimed = tx
                .execute(
                    "UPDATE execution_jobs
                  SET active_label = ?1, status = 'running', updated_at = ?2
                  WHERE id = ?3 AND status IN ('queued', 'running')",
                    params![
                        item.title,
                        chrono::Utc::now().to_rfc3339(),
                        job_bytes(job_id)?
                    ],
                )
                .map_err(|e| format!("Failed to mark update job running: {}", e))?;
            if job_claimed != 1 {
                return Err("Update job stopped while its next item was being claimed".to_string());
            }
            tx.commit()
                .map_err(|e| format!("Failed to commit update item claim: {e}"))?;
            return Ok(Some(item));
        }
        tx.commit()
            .map_err(|e| format!("Failed to finish update item claim: {e}"))?;
        Ok(None)
    }

    pub fn queue_update_job_candidates(
        &self,
        job_id: &str,
        candidate_ids: &[i64],
    ) -> Result<i64, String> {
        if candidate_ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin update candidate queue: {e}"))?;
        let job_status: String = tx
            .query_row(
                "SELECT status FROM execution_jobs WHERE id = ?1",
                params![job_bytes(job_id)?],
                |row| row.get(0),
            )
            .map_err(|e| format!("Update job not found: {e}"))?;

        // A paused or canceled worker leaves unfinished items queued so an
        // explicit resume can continue where it stopped. Saving selected
        // candidates instead replaces that queue with only this selection.
        // Unselected candidates remain available for a later choice.
        if matches!(job_status.as_str(), "paused" | "canceled") {
            let mut has_selected_candidate = false;
            for id in candidate_ids {
                has_selected_candidate |= tx
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM execution_items
                            WHERE id = ?1 AND job_id = ?2 AND item_type = 'candidate'
                              AND status IN ('candidate', 'failed', 'queued', 'running')
                         )",
                        params![id, job_bytes(job_id)?],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|e| format!("Failed to validate update candidate: {e}"))?;
            }
            // Preserve the existing no-op guarantee for stale or foreign IDs.
            // In particular, an invalid save request must not discard the
            // queue which an explicit resume could still continue.
            if !has_selected_candidate {
                tx.commit()
                    .map_err(|e| format!("Failed to finish empty candidate queue: {e}"))?;
                return Ok(0);
            }
            // 確かめずに残した作者は「確認済み」ではない。印を付けて、探す段階の進み具合から外し
            // （探すバーが満ちたように見せない）、記録にも残す。
            let unchecked = tx
                .execute(
                    "UPDATE execution_items
                     SET status = 'skipped', error = ?2, updated_at = CURRENT_TIMESTAMP
                     WHERE job_id = ?1 AND item_type != 'candidate' AND status IN ('queued', 'running')",
                    params![job_bytes(job_id)?, UNCHECKED_BEFORE_SAVE],
                )
                .map_err(|e| format!("Failed to abandon canceled update items: {e}"))?;
            tx.execute(
                "UPDATE execution_items
                 SET status = 'candidate', error = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE job_id = ?1 AND item_type = 'candidate' AND status IN ('queued', 'running')",
                params![job_bytes(job_id)?],
            )
            .map_err(|e| format!("Failed to abandon canceled update items: {e}"))?;
            if unchecked > 0 {
                tx.execute(
                    "INSERT INTO execution_logs (job_id, log_type, message) VALUES (?1, 'warn', ?2)",
                    params![
                        job_bytes(job_id)?,
                        format!("確かめていない{unchecked}件を残して、選んだ候補の保存に移りました。残りは次の確認で調べます")
                    ],
                )
                .map_err(|e| format!("Failed to append update job log: {e}"))?;
            }
        }

        let mut changed = 0i64;
        for id in candidate_ids {
            changed += tx
                .execute(
                    "UPDATE execution_items
                     SET status = 'queued', error = NULL, updated_at = CURRENT_TIMESTAMP
                     WHERE id = ?1 AND job_id = ?2 AND item_type = 'candidate' AND status IN ('candidate', 'failed')",
                    params![id, job_bytes(job_id)?],
                )
                .map_err(|e| format!("Failed to queue update candidate: {}", e))?
                as i64;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit update candidate queue: {e}"))?;
        drop(conn);
        // 待機列に入れた候補と確かめずに残した項目の数を、画面が読む数え直しにすぐ映す
        // （次の一件が終わるまで、前の数のまま止まって見えないように）。
        self.sync_update_job_counters(job_id)?;
        Ok(changed)
    }

    pub fn clear_update_job(&self, job_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM execution_jobs WHERE id = ?1 AND status NOT IN ('queued', 'running', 'canceling')",
            params![job_bytes(job_id)?],
        )
        .map_err(|e| format!("Failed to clear update job: {}", e))?;
        Ok(())
    }

    fn sync_update_job_counters(&self, job_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            // 候補は「見つけた結果」であって作業ではない。ただし保存するために
            // 待機列へ入れた候補は作業なので、そのぶん総数と処理済みに数える。
            // これを外すと、自動保存で 101 件を保存している最中に進捗が
            // 10/10（＝完了）と出てしまう。
            "UPDATE execution_jobs
             SET totals = (SELECT COUNT(*) FROM execution_items
                           WHERE job_id = ?1 AND (item_type != 'candidate' OR status != 'candidate')),
                 processed = (SELECT COUNT(*) FROM execution_items
                              WHERE job_id = ?1 AND (item_type != 'candidate' OR status != 'candidate')
                                AND status IN ('done', 'saved', 'skipped', 'failed', 'held')),
                 candidate_count = (SELECT COUNT(*) FROM execution_items WHERE job_id = ?1 AND item_type = 'candidate'),
                 saved_count = (SELECT COUNT(*) FROM execution_items WHERE job_id = ?1 AND status = 'saved'),
                 error_count = (SELECT COUNT(*) FROM execution_items WHERE job_id = ?1 AND status = 'failed')
             WHERE id = ?1",
            params![job_bytes(job_id)?],
        )
        .map_err(|e| format!("Failed to sync update job counters: {}", e))?;
        Ok(())
    }
}
fn update_job_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UpdateJobSummary> {
    Ok(UpdateJobSummary {
        job_id: uuid_text(row, 0)?,
        status: row.get(1)?,
        scope: row.get(2)?,
        mode: row.get(3)?,
        new_works_only: row.get::<_, i64>(19)? != 0,
        subject_label: None,
        subject_source: None,
        totals: row.get(4)?,
        processed: row.get(5)?,
        check_total: row.get(14)?,
        check_processed: row.get(15)?,
        save_total: row.get(16)?,
        save_processed: row.get(17)?,
        candidate_count: row.get(6)?,
        saved_count: row.get(7)?,
        error_count: row.get(8)?,
        held_count: row.get(13)?,
        known_held_count: row.get(18)?,
        active_label: row.get(9)?,
        started_at: row.get(10)?,
        updated_at: row.get(11)?,
        finished_at: row.get(12)?,
    })
}

#[path = "job_queue_store_write.rs"]
mod write;
