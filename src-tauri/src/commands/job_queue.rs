//! ジョブの正本を一つだけ選ぶ。切り替え後の実行は旧 DB を読まず、書かない。旧キューの引き継ぎだけを起動時に行う。
use crate::database::*;
use crate::library::{Library, LibraryId};
use crate::AppState;
use std::sync::Arc;
#[path = "job_queue_native.rs"]
mod native;
use native::NativeQueue;
pub(crate) struct Jobs<'a> {
    state: &'a AppState,
    native: Option<Arc<Library>>,
}
impl AppState {
    pub(crate) fn jobs(&self) -> Jobs<'_> {
        Jobs {
            state: self,
            native: self.library_v2.switched(),
        }
    }
}
impl Jobs<'_> {
    /// ワーカーを登録する前に、旧キューに残る未対応の記録だけを一度引き継ぐ。
    pub(crate) fn import_previous_queue(&self) -> Result<(), String> {
        let Some(library) = &self.native else {
            return Ok(());
        };
        let known = {
            let conn = library.reader().map_err(|e| e.to_string())?;
            let mut stmt=conn.prepare("SELECT source_key FROM migration_records WHERE source_table='update_jobs' AND status='imported'").map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<std::collections::HashSet<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        self.state.db.visit_previous_queue(&known, |job| {
            NativeQueue {
                library,
                conn: &library.conn,
            }
            .import_previous_job(job)
        })
    }
    pub(crate) fn recover_update_jobs_on_startup(&self) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .recover_update_jobs_on_startup(),
            None => self.state.db.recover_update_jobs_on_startup(),
        }
    }
    pub(crate) fn prune_update_jobs(
        &self,
        keep_count: i64,
        keep_days: i64,
    ) -> Result<usize, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .prune_update_jobs(keep_count, keep_days),
            None => self.state.db.prune_update_jobs(keep_count, keep_days),
        }
    }
    pub(crate) fn trim_update_job_logs(&self, job_id: &str, keep: i64) -> Result<usize, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .trim_update_job_logs(job_id, keep),
            None => self.state.db.trim_update_job_logs(job_id, keep),
        }
    }
    pub(crate) fn clear_finished_update_jobs(&self) -> Result<usize, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .clear_finished_update_jobs(),
            None => self.state.db.clear_finished_update_jobs(),
        }
    }
    pub(crate) fn clear_update_job(&self, job_id: &str) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .clear_update_job(job_id),
            None => self.state.db.clear_update_job(job_id),
        }
    }
    pub(crate) fn create_update_job_if_idle(
        &self,
        job_id: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .create_update_job_if_idle(job_id, request, items),
            None => self
                .state
                .db
                .create_update_job_if_idle(job_id, request, items),
        }
    }
    pub(crate) fn update_job_watch_saved(&self, job_id: &str) -> Result<bool, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_watch_saved(job_id),
            None => self.state.db.update_job_watch_saved(job_id),
        }
    }
    pub(crate) fn update_job_new_works_only(&self, job_id: &str) -> Result<bool, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_new_works_only(job_id),
            None => self.state.db.update_job_new_works_only(job_id),
        }
    }
    pub(crate) fn list_update_jobs(&self) -> Result<Vec<UpdateJobSummary>, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .list_update_jobs(),
            None => self.state.db.list_update_jobs(),
        }
    }
    pub(crate) fn update_job_snapshot(&self, job_id: &str) -> Result<UpdateJobSnapshot, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_snapshot(job_id),
            None => self.state.db.update_job_snapshot(job_id),
        }
    }
    pub(crate) fn update_job_snapshot_page(
        &self,
        job_id: &str,
        candidate_after_id: Option<i64>,
        log_before_id: Option<i64>,
    ) -> Result<UpdateJobSnapshot, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_snapshot_page(job_id, candidate_after_id, log_before_id),
            None => {
                self.state
                    .db
                    .update_job_snapshot_page(job_id, candidate_after_id, log_before_id)
            }
        }
    }
    pub(crate) fn set_update_job_status(
        &self,
        job_id: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .set_update_job_status(job_id, status, active_label),
            None => self
                .state
                .db
                .set_update_job_status(job_id, status, active_label),
        }
    }
    pub(crate) fn transition_update_job_status(
        &self,
        job_id: &str,
        expected_status: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<bool, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .transition_update_job_status(
                job_id,
                expected_status,
                status,
                active_label,
            ),
            None => self.state.db.transition_update_job_status(
                job_id,
                expected_status,
                status,
                active_label,
            ),
        }
    }
    pub(crate) fn mark_update_job_item_known_hold(&self, item_id: i64) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .mark_update_job_item_known_hold(item_id),
            None => self.state.db.mark_update_job_item_known_hold(item_id),
        }
    }
    pub(crate) fn set_update_job_item_payload(
        &self,
        item_id: i64,
        payload: &str,
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .set_update_job_item_payload(item_id, payload),
            None => self.state.db.set_update_job_item_payload(item_id, payload),
        }
    }
    pub(crate) fn prepare_update_job_resume(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .prepare_update_job_resume(job_id, retry_failed),
            None => self
                .state
                .db
                .prepare_update_job_resume(job_id, retry_failed),
        }
    }
    pub(crate) fn resume_update_job_if_idle(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .resume_update_job_if_idle(job_id, retry_failed),
            None => self
                .state
                .db
                .resume_update_job_if_idle(job_id, retry_failed),
        }
    }
    pub(crate) fn append_update_job_log(
        &self,
        job_id: &str,
        log_type: &str,
        message: &str,
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .append_update_job_log(job_id, log_type, message),
            None => self
                .state
                .db
                .append_update_job_log(job_id, log_type, message),
        }
    }
    pub(crate) fn update_job_status_value(&self, job_id: &str) -> Result<String, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_status_value(job_id),
            None => self.state.db.update_job_status_value(job_id),
        }
    }
    pub(crate) fn update_job_mode_value(&self, job_id: &str) -> Result<String, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_mode_value(job_id),
            None => self.state.db.update_job_mode_value(job_id),
        }
    }
    pub(crate) fn list_update_job_item_states(
        &self,
        job_id: &str,
    ) -> Result<Vec<UpdateJobItemState>, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .list_update_job_item_states(job_id),
            None => self.state.db.list_update_job_item_states(job_id),
        }
    }
    pub(crate) fn list_update_job_held_item_states(
        &self,
        job_id: &str,
    ) -> Result<Vec<UpdateJobItemState>, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .list_update_job_held_item_states(job_id),
            None => self.state.db.list_update_job_held_item_states(job_id),
        }
    }
    pub(crate) fn update_job_progress_delta(
        &self,
        job_id: &str,
        changed_item_id: Option<i64>,
    ) -> Result<UpdateJobProgressDelta, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .update_job_progress_delta(job_id, changed_item_id),
            None => self
                .state
                .db
                .update_job_progress_delta(job_id, changed_item_id),
        }
    }
    pub(crate) fn next_update_job_item(
        &self,
        job_id: &str,
    ) -> Result<Option<UpdateJobItem>, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .next_update_job_item(job_id),
            None => self.state.db.next_update_job_item(job_id),
        }
    }
    pub(crate) fn insert_update_job_candidate(
        &self,
        job_id: &str,
        candidate: &UpdateJobItemInput,
    ) -> Result<bool, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .insert_update_job_candidate(job_id, candidate),
            None => self.state.db.insert_update_job_candidate(job_id, candidate),
        }
    }
    pub(crate) fn queue_update_job_candidates(
        &self,
        job_id: &str,
        candidate_ids: &[i64],
    ) -> Result<i64, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .queue_update_job_candidates(job_id, candidate_ids),
            None => self
                .state
                .db
                .queue_update_job_candidates(job_id, candidate_ids),
        }
    }
    pub(crate) fn latest_job_candidate(
        &self,
        source: &str,
        key: &str,
    ) -> Result<(String, String), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .latest_job_candidate(source, key),
            None => self.state.db.latest_job_candidate(source, key),
        }
    }
    pub(crate) fn resolve_unavailable_job_items(
        &self,
        source: &str,
        keys: &[String],
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .resolve_unavailable_job_items(source, keys),
            None => self.state.db.resolve_unavailable_job_items(source, keys),
        }
    }
    pub(crate) fn insert_unavailable_queue_item(
        &self,
        job_id: &str,
        item: &UpdateJobItemInput,
        reason: &str,
        known: bool,
    ) -> Result<bool, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .insert_unavailable_queue_item(job_id, item, reason, known),
            None => self
                .state
                .db
                .insert_unavailable_queue_item(job_id, item, reason, known),
        }
    }
    pub(crate) fn has_active_update_job(&self) -> Result<bool, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .has_active_update_job(),
            None => self.state.db.has_active_update_job(),
        }
    }
    #[cfg(test)]
    pub(crate) fn create_update_job(
        &self,
        job: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        match &self.native {
            Some(library) => NativeQueue {
                library,
                conn: &library.conn,
            }
            .create_update_job(job, request, items),
            None => self.state.db.create_update_job(job, request, items),
        }
    }
    pub(crate) fn complete_update_job_item(
        &self,
        item: i64,
        status: &str,
        error: Option<&str>,
        old: Option<i64>,
    ) -> Result<(), String> {
        self.complete_saved_item(item, status, error, old, None)
    }
    pub(crate) fn complete_saved_item(
        &self,
        item: i64,
        status: &str,
        error: Option<&str>,
        old: Option<i64>,
        work: Option<LibraryId>,
    ) -> Result<(), String> {
        match &self.native {
            Some(library) => {
                if old.is_some() {
                    return Err("旧棚の保存結果を新しいキューへ書けません".into());
                }
                NativeQueue {
                    library,
                    conn: &library.conn,
                }
                .complete_update_job_item(item, status, error, work)
            }
            None => {
                if work.is_some() {
                    return Err("棚の保存結果を旧キューへ書けません".into());
                }
                self.state
                    .db
                    .complete_update_job_item(item, status, error, old)
            }
        }
    }
    pub(crate) fn saved_update_job_download_ids(&self, job: &str) -> Result<Vec<i64>, String> {
        if self.native.is_some() {
            Ok(Vec::new())
        } else {
            self.state.db.saved_update_job_download_ids(job)
        }
    }
}

#[cfg(test)]
#[path = "job_queue_tests.rs"]
mod tests;
