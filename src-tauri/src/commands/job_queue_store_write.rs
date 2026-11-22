//! 実行項目の入力・表示用の写し・集計を、一つの確定で更新する。
use super::*;
fn known_hold(payload: &str) -> Result<bool, String> {
    Ok(
        serde_json::from_str::<serde_json::Value>(payload).map_err(|e| e.to_string())?["knownHold"]
            .as_bool()
            .unwrap_or(false),
    )
}
fn display(payload: &str) -> Result<String, String> {
    UpdateWorkMetadata::execution_display(payload).map_err(|e| e.to_string())
}
fn insert_item(
    tx: &Transaction<'_>,
    job: &str,
    item: &UpdateJobItemInput,
    hash: Fingerprint,
    known: bool,
) -> Result<i64, String> {
    let kind = match (item.item_type.as_str(), item.target_type.as_deref()) {
        ("work" | "candidate", _) | ("target", Some("work")) => EntityKind::Work,
        ("target", Some("author" | "creator")) => EntityKind::Person,
        ("target", Some("series")) => EntityKind::Series,
        _ => return Err("実行項目の種類が不正です".into()),
    };
    let entity = match (item.source.as_deref(), item.source_id.as_deref()) {
        (Some(source), Some(key)) if !source.is_empty() && !key.is_empty() => Some(
            Library::relation_entity(tx, source, kind, key, chrono::Utc::now().timestamp_millis())
                .map_err(|e| e.to_string())?,
        ),
        (None, None) => None,
        _ => return Err("実行項目の取得元の識別が不完全です".into()),
    };
    tx.execute("INSERT INTO execution_items(job_id,item_type,entity_id,source,source_id,target_type,title,payload_file_id,known_hold,status,display_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![job_bytes(job)?,item.item_type,entity.map(|v|v.as_bytes().to_vec()),item.source,item.source_id,item.target_type,item.title,hash.0.as_slice(),known,item.status,display(&item.payload_json)?]).map_err(|e|e.to_string())?;
    Ok(tx.last_insert_rowid())
}
const COUNTERS:&str="UPDATE execution_jobs SET
 totals=(SELECT count(*) FROM execution_items WHERE job_id=?1 AND (item_type!='candidate' OR status!='candidate')),
 processed=(SELECT count(*) FROM execution_items WHERE job_id=?1 AND (item_type!='candidate' OR status!='candidate') AND status IN ('done','saved','skipped','failed','held')),
 candidate_count=(SELECT count(*) FROM execution_items WHERE job_id=?1 AND item_type='candidate'),
 saved_count=(SELECT count(*) FROM execution_items WHERE job_id=?1 AND status='saved'),
 error_count=(SELECT count(*) FROM execution_items WHERE job_id=?1 AND status='failed') WHERE id=?1";
impl Queue<'_> {
    pub(crate) fn create_update_job_if_idle(
        &self,
        job: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        self.create_checked(job, request, items, true)
    }
    #[cfg(test)]
    pub(crate) fn create_update_job(
        &self,
        job: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        self.create_checked(job, request, items, false)
    }
    fn create_checked(
        &self,
        job: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
        idle: bool,
    ) -> Result<UpdateJobSnapshot, String> {
        job.parse::<LibraryId>().map_err(|e| e.to_string())?;
        let _storage = self
            .library
            .storage_gate
            .lock()
            .map_err(|e| e.to_string())?;
        let prepared = items
            .iter()
            .map(|item| {
                if item.payload_json.len() > 16 * 1024 * 1024 {
                    return Err("実行項目の入力が読み込み上限を超えています".to_string());
                }
                known_hold(&item.payload_json)?;
                self.library
                    .store_bytes(item.payload_json.as_bytes())
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if idle && tx.query_row("SELECT EXISTS(SELECT 1 FROM execution_jobs WHERE status IN ('queued','running','canceling'))",[],|r|r.get::<_,bool>(0)).map_err(|e|e.to_string())? {return Err("別の更新または保存ジョブが進行中です".into());}
        let now = chrono::Utc::now().to_rfc3339();
        tx.execute("INSERT INTO execution_jobs(id,scope,mode,status,watch_saved,new_works_only,started_at,updated_at) VALUES(?1,?2,?3,'queued',?4,?5,?6,?6)",params![job_bytes(job)?,request.scope,request.mode,request.watch_saved.unwrap_or(false),request.new_works_only.unwrap_or(false),now]).map_err(|e|e.to_string())?;
        for (item, file) in items.iter().zip(prepared.iter()) {
            Library::register_file(&tx, file).map_err(|e| e.to_string())?;
            insert_item(
                &tx,
                job,
                item,
                file.file.id,
                known_hold(&item.payload_json)?,
            )?;
        }
        tx.execute(COUNTERS, [job_bytes(job)?])
            .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO execution_logs(job_id,log_type,message) VALUES(?1,'info','更新ジョブを作成しました')",[job_bytes(job)?]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn);
        self.update_job_snapshot(job)
    }
    pub(crate) fn has_active_update_job(&self) -> Result<bool, String> {
        self.read_conn()?.query_row("SELECT EXISTS(SELECT 1 FROM execution_jobs WHERE status IN ('queued','running','canceling'))",[],|r|r.get(0)).map_err(|e|e.to_string())
    }
    pub(crate) fn set_update_job_item_payload(
        &self,
        item: i64,
        payload: &str,
    ) -> Result<(), String> {
        let _storage = self
            .library
            .storage_gate
            .lock()
            .map_err(|e| e.to_string())?;
        let known = known_hold(payload)?;
        if payload.len() > 16 * 1024 * 1024 {
            return Err("実行項目の入力が読み込み上限を超えています".into());
        }
        let file = self
            .library
            .store_bytes(payload.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        Library::register_file(&tx, &file).map_err(|e| e.to_string())?;
        if tx.execute("UPDATE execution_items SET payload_file_id=?2,known_hold=?3,display_json=?4,resume_generation=NULL WHERE id=?1",params![item,file.file.id.0.as_slice(),known,display(payload)?]).map_err(|e|e.to_string())?!=1 {return Err("実行項目が見つかりません".into());}
        tx.commit().map_err(|e| e.to_string())
    }
    pub(crate) fn mark_update_job_item_known_hold(&self, item: i64) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE execution_items SET known_hold=1 WHERE id=?1",
                [item],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn insert_update_job_candidate(
        &self,
        job: &str,
        item: &UpdateJobItemInput,
    ) -> Result<bool, String> {
        self.insert_extra_item(job, item, None, false)
    }
    pub(crate) fn insert_unavailable_queue_item(
        &self,
        job: &str,
        item: &UpdateJobItemInput,
        reason: &str,
        known: bool,
    ) -> Result<bool, String> {
        let mut item = item.clone();
        item.item_type = "work".into();
        item.status = "held".into();
        self.insert_extra_item(job, &item, Some(reason), known)
    }
    fn insert_extra_item(
        &self,
        job: &str,
        item: &UpdateJobItemInput,
        error: Option<&str>,
        known: bool,
    ) -> Result<bool, String> {
        let _storage = self
            .library
            .storage_gate
            .lock()
            .map_err(|e| e.to_string())?;
        known_hold(&item.payload_json)?;
        if item.payload_json.len() > 16 * 1024 * 1024 {
            return Err("実行項目の入力が読み込み上限を超えています".into());
        }
        let file = self
            .library
            .store_bytes(item.payload_json.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM execution_items WHERE job_id=?1 AND item_type=?2 AND source IS ?3 AND source_id IS ?4)",params![job_bytes(job)?,item.item_type,item.source,item.source_id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if exists {
            return Ok(false);
        }
        Library::register_file(&tx, &file).map_err(|e| e.to_string())?;
        let id = insert_item(
            &tx,
            job,
            item,
            file.file.id,
            known || known_hold(&item.payload_json)?,
        )?;
        tx.execute(
            "UPDATE execution_items SET error=?2 WHERE id=?1",
            params![id, error],
        )
        .map_err(|e| e.to_string())?;
        let c = update_job_item_counter_contribution(&item.item_type, &item.status);
        tx.execute("UPDATE execution_jobs SET totals=totals+?2,processed=processed+?3,candidate_count=candidate_count+?4,saved_count=saved_count+?5,error_count=error_count+?6,updated_at=?7 WHERE id=?1",params![job_bytes(job)?,c[0],c[1],c[2],c[3],c[4],chrono::Utc::now().to_rfc3339()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
    pub(crate) fn latest_job_candidate(
        &self,
        source: &str,
        key: &str,
    ) -> Result<(String, String), String> {
        let conn = self.read_conn()?;
        conn.query_row("SELECT title,payload_file_id FROM execution_items WHERE source=?1 AND source_id=?2 AND item_type='candidate' ORDER BY id DESC LIMIT 1",params![source,key],|r|Ok((r.get(0)?,self.payload(&conn,r.get(1)?)?))).map_err(|e|e.to_string())
    }
    pub(crate) fn resolve_unavailable_job_items(
        &self,
        source: &str,
        keys: &[String],
    ) -> Result<(), String> {
        let conn = self.read_conn()?;
        let ids=conn.prepare("SELECT id,payload_file_id FROM execution_items WHERE source=?1 AND source_id IN (SELECT value FROM json_each(?2)) AND status='held' AND (error LIKE '%[見つからない]%' OR error LIKE '%公開元で投稿が見つかりません%' OR error LIKE '%公開元で投稿が見つからない%')").map_err(|e|e.to_string())?.query_map(params![source,serde_json::to_string(keys).map_err(|e|e.to_string())?],|r|Ok((r.get::<_,i64>(0)?,self.payload(&conn,r.get(1)?)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
        drop(conn);
        for (id, text) in ids {
            let mut value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if value["acquisitionResolved"] != true {
                value["acquisitionResolved"] = true.into();
                self.set_update_job_item_payload(id, &value.to_string())?;
            }
        }
        Ok(())
    }
    pub(crate) fn list_update_job_item_states(
        &self,
        job: &str,
    ) -> Result<Vec<UpdateJobItemState>, String> {
        self.item_states(job, false)
    }
    pub(crate) fn list_update_job_held_item_states(
        &self,
        job: &str,
    ) -> Result<Vec<UpdateJobItemState>, String> {
        self.item_states(job, true)
    }
    fn item_states(&self, job: &str, held: bool) -> Result<Vec<UpdateJobItemState>, String> {
        let conn = self.read_conn()?;
        let mut stmt=conn.prepare("SELECT source,source_id,status,error,item_type,title,display_json FROM execution_items WHERE job_id=?1 AND (?2=0 OR (status='held' AND known_hold=0)) ORDER BY id").map_err(|e|e.to_string())?;
        let result = stmt
            .query_map(params![job_bytes(job)?, held], |r| {
                let payload: String = r.get(6)?;
                let value: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Blob,
                        Box::new(e),
                    )
                })?;
                let source: Option<String> = r.get(0)?;
                let key: Option<String> = r.get(1)?;
                let mut metadata =
                    serde_json::from_value(value["metadata"].clone()).unwrap_or_default();
                let kind: String = r.get(4)?;
                let work_id = if kind == "target" {
                    None
                } else {
                    self.saved_metadata(source.as_deref(), key.as_deref(), &mut metadata)?
                };
                Ok(UpdateJobItemState {
                    work_id,
                    source,
                    source_id: key,
                    status: r.get(2)?,
                    error: r.get(3)?,
                    item_type: r.get(4)?,
                    title: r.get(5)?,
                    metadata,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string());
        result
    }
    pub fn complete_update_job_item(
        &self,
        item_id: i64,
        status: &str,
        error: Option<&str>,
        result_work_id: Option<LibraryId>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin update item completion: {e}"))?;
        let (job_id, item_type, previous_status): (String, String, String) = tx
            .query_row(
                "SELECT job_id, item_type, status FROM execution_items WHERE id = ?1",
                params![item_id],
                |row| Ok((uuid_text(row, 0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Failed to read update job item before completion: {e}"))?;
        let changed = tx
            .execute(
                "UPDATE execution_items
             SET status = ?1, error = ?2, result_work_id = ?3, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4",
                params![
                    status,
                    error,
                    result_work_id.map(|v| v.as_bytes().to_vec()),
                    item_id
                ],
            )
            .map_err(|e| format!("Failed to update job item: {}", e))?;
        if changed == 1 && previous_status != status {
            let previous = update_job_item_counter_contribution(&item_type, &previous_status);
            let next = update_job_item_counter_contribution(&item_type, status);
            let delta = [
                next[0] - previous[0],
                next[1] - previous[1],
                next[2] - previous[2],
                next[3] - previous[3],
                next[4] - previous[4],
            ];
            tx.execute(
                "UPDATE execution_jobs
                    SET totals = MAX(0, totals + ?1),
                        processed = MAX(0, processed + ?2),
                        candidate_count = MAX(0, candidate_count + ?3),
                        saved_count = MAX(0, saved_count + ?4),
                        error_count = MAX(0, error_count + ?5),
                        updated_at = ?6
                  WHERE id = ?7",
                params![
                    delta[0],
                    delta[1],
                    delta[2],
                    delta[3],
                    delta[4],
                    chrono::Utc::now().to_rfc3339(),
                    job_id,
                ],
            )
            .map_err(|e| format!("Failed to advance update job counters: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit update item completion: {e}"))?;
        Ok(())
    }
}
fn update_job_item_counter_contribution(item_type: &str, status: &str) -> [i64; 5] {
    let counts_as_work = item_type != "candidate" || status != "candidate";
    let terminal = matches!(status, "done" | "saved" | "skipped" | "failed" | "held");
    [
        i64::from(counts_as_work),
        i64::from(counts_as_work && terminal),
        i64::from(item_type == "candidate"),
        i64::from(status == "saved"),
        i64::from(status == "failed"),
    ]
}
