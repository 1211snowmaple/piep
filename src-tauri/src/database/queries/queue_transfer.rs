//! 切り替え後も旧キューへ書いていた版からの引き継ぎ。元の DB は変更しない。
use super::*;

pub(crate) struct QueueItem {
    pub item: UpdateJobItem,
    pub created_at: String,
    pub updated_at: String,
}
pub(crate) struct QueueExport {
    pub summary: UpdateJobSummary,
    pub watch_saved: bool,
    pub items: Vec<QueueItem>,
    pub logs: Vec<UpdateJobLog>,
}
impl Database {
    pub(crate) fn visit_previous_queue(
        &self,
        excluded: &HashSet<String>,
        mut visit: impl FnMut(QueueExport) -> Result<(), String>,
    ) -> Result<(), String> {
        // 起動中、ワーカーを登録する前に呼ぶ。旧 DB の排他を保ち、集計を書き直さない。
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt=conn.prepare("SELECT id,status,scope,mode,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at,0,0,0,0,0,0,new_works_only,watch_saved FROM update_jobs ORDER BY started_at,id").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((update_job_summary_from_row(r)?, r.get::<_, bool>(20)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (summary, watch_saved) = row.map_err(|e| e.to_string())?;
            if excluded.contains(&summary.job_id) {
                continue;
            }
            let mut items=conn.prepare("SELECT id,job_id,item_type,source,source_id,target_type,title,payload_json,status,error,result_download_id,created_at,updated_at FROM update_job_items WHERE job_id=?1 ORDER BY id").map_err(|e|e.to_string())?;
            let items = items
                .query_map([&summary.job_id], |r| {
                    Ok(QueueItem {
                        item: update_job_item_from_row(r)?,
                        created_at: r.get(11)?,
                        updated_at: r.get(12)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let mut logs=conn.prepare("SELECT id,log_type,message,created_at FROM update_job_logs WHERE job_id=?1 ORDER BY id").map_err(|e|e.to_string())?;
            let logs = logs
                .query_map([&summary.job_id], |r| {
                    Ok(UpdateJobLog {
                        id: r.get(0)?,
                        log_type: r.get(1)?,
                        message: r.get(2)?,
                        created_at: r.get(3)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            visit(QueueExport {
                summary,
                watch_saved,
                items,
                logs,
            })?;
        }
        Ok(())
    }
}
