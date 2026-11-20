//! 以前の版が旧キューへ残した記録を、原記録を変えずに一度だけ引き継ぐ。
use super::*;
use crate::database::queries::queue_transfer::QueueExport;

impl NativeQueue<'_> {
    pub(crate) fn import_previous_job(&self, input: QueueExport) -> Result<(), String> {
        let _storage = self
            .library
            .storage_gate
            .lock()
            .map_err(|e| e.to_string())?;
        let old_numbers = self
            .library
            .legacy_work_numbers()
            .map_err(|e| e.to_string())?;
        let mut prepared = Vec::new();
        for row in &input.items {
            known_hold(&row.item.payload_json)?;
            if row.item.payload_json.len() > 16 * 1024 * 1024 {
                return Err("引き継ぐジョブの入力が読み込み上限を超えています".into());
            }
            prepared.push(
                self.library
                    .store_bytes(row.item.payload_json.as_bytes())
                    .map_err(|e| e.to_string())?,
            );
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let j = &input.summary;
        let known:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM migration_records WHERE source_table='update_jobs' AND source_key=?1 AND status='imported')",[&j.job_id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if known {
            return Ok(());
        }
        let uuid = LibraryId::now_v7();
        let job = uuid.to_string();
        tx.execute("INSERT INTO execution_jobs(id,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![job_bytes(&job)?,j.scope,j.mode,j.status,input.watch_saved,j.new_works_only,j.totals,j.processed,j.candidate_count,j.saved_count,j.error_count,j.active_label,j.started_at,j.updated_at,j.finished_at]).map_err(|e|e.to_string())?;
        for (row, file) in input.items.iter().zip(prepared.iter()) {
            let i = &row.item;
            let item = UpdateJobItemInput {
                item_type: i.item_type.clone(),
                source: i.source.clone(),
                source_id: i.source_id.clone(),
                target_type: i.target_type.clone(),
                title: i.title.clone(),
                payload_json: i.payload_json.clone(),
                status: i.status.clone(),
            };
            Library::register_file(&tx, file).map_err(|e| e.to_string())?;
            let id = insert_item(
                &tx,
                &job,
                &item,
                file.file.id,
                known_hold(&item.payload_json)?,
            )?;
            let result = i
                .result_download_id
                .map(|old| {
                    old_numbers
                        .iter()
                        .find_map(|(work, number)| (*number == old).then_some(*work))
                        .ok_or("引き継ぐジョブの保存結果に対応する作品がありません")
                })
                .transpose()?;
            tx.execute("UPDATE execution_items SET error=?2,result_work_id=?3,created_at=?4,updated_at=?5,source_sequence=?6 WHERE id=?1",params![id,i.error,result.map(|v|v.as_bytes().to_vec()),row.created_at,row.updated_at,i.id]).map_err(|e|e.to_string())?;
        }
        for log in input.logs {
            tx.execute("INSERT INTO execution_logs(job_id,source_sequence,log_type,message,created_at) VALUES(?1,?2,?3,?4,?5)",params![job_bytes(&job)?,log.id,log.log_type,log.message,log.created_at]).map_err(|e|e.to_string())?;
        }
        // ジョブを片付けた後も対応を残し、旧行から復活させない。
        tx.execute("INSERT INTO migration_records(source_table,source_key,target_id,status) VALUES('update_jobs',?1,?2,'imported')",params![j.job_id,uuid.as_bytes().as_slice()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}
