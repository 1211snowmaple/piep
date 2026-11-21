//! ジョブの実行キュー。棚の `jobs` 表に持つ。
use crate::database::*;
use crate::library::{Library, LibraryId};
use crate::AppState;
use std::sync::Arc;
#[path = "job_queue_native.rs"]
mod native;
use native::NativeQueue;
pub(crate) struct Jobs {
    library: Option<Arc<Library>>,
}
impl AppState {
    pub(crate) fn jobs(&self) -> Jobs {
        Jobs {
            library: self.library_v2.library().ok(),
        }
    }
}
impl Jobs {
    fn queue(&self) -> Result<NativeQueue<'_>, String> {
        let library = self
            .library
            .as_ref()
            .ok_or_else(|| "棚は開いていません".to_string())?;
        Ok(NativeQueue {
            library,
            conn: &library.conn,
        })
    }
    pub(crate) fn recover_update_jobs_on_startup(&self) -> Result<(), String> {
        self.queue()?.recover_update_jobs_on_startup()
    }
    pub(crate) fn prune_update_jobs(
        &self,
        keep_count: i64,
        keep_days: i64,
    ) -> Result<usize, String> {
        self.queue()?.prune_update_jobs(keep_count, keep_days)
    }
    pub(crate) fn trim_update_job_logs(&self, job_id: &str, keep: i64) -> Result<usize, String> {
        self.queue()?.trim_update_job_logs(job_id, keep)
    }
    pub(crate) fn clear_finished_update_jobs(&self) -> Result<usize, String> {
        self.queue()?.clear_finished_update_jobs()
    }
    pub(crate) fn clear_update_job(&self, job_id: &str) -> Result<(), String> {
        self.queue()?.clear_update_job(job_id)
    }
    pub(crate) fn create_update_job_if_idle(
        &self,
        job_id: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        self.queue()?
            .create_update_job_if_idle(job_id, request, items)
    }
    pub(crate) fn update_job_watch_saved(&self, job_id: &str) -> Result<bool, String> {
        self.queue()?.update_job_watch_saved(job_id)
    }
    pub(crate) fn update_job_new_works_only(&self, job_id: &str) -> Result<bool, String> {
        self.queue()?.update_job_new_works_only(job_id)
    }
    pub(crate) fn list_update_jobs(&self) -> Result<Vec<UpdateJobSummary>, String> {
        self.queue()?.list_update_jobs()
    }
    pub(crate) fn update_job_snapshot(&self, job_id: &str) -> Result<UpdateJobSnapshot, String> {
        self.queue()?.update_job_snapshot(job_id)
    }
    pub(crate) fn update_job_snapshot_page(
        &self,
        job_id: &str,
        candidate_after_id: Option<i64>,
        log_before_id: Option<i64>,
    ) -> Result<UpdateJobSnapshot, String> {
        self.queue()?
            .update_job_snapshot_page(job_id, candidate_after_id, log_before_id)
    }
    pub(crate) fn set_update_job_status(
        &self,
        job_id: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<(), String> {
        self.queue()?
            .set_update_job_status(job_id, status, active_label)
    }
    pub(crate) fn transition_update_job_status(
        &self,
        job_id: &str,
        expected_status: &str,
        status: &str,
        active_label: Option<&str>,
    ) -> Result<bool, String> {
        self.queue()?
            .transition_update_job_status(job_id, expected_status, status, active_label)
    }
    pub(crate) fn mark_update_job_item_known_hold(&self, item_id: i64) -> Result<(), String> {
        self.queue()?.mark_update_job_item_known_hold(item_id)
    }
    pub(crate) fn set_update_job_item_payload(
        &self,
        item_id: i64,
        payload: &str,
    ) -> Result<(), String> {
        self.queue()?.set_update_job_item_payload(item_id, payload)
    }
    pub(crate) fn prepare_update_job_resume(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        self.queue()?
            .prepare_update_job_resume(job_id, retry_failed)
    }
    pub(crate) fn resume_update_job_if_idle(
        &self,
        job_id: &str,
        retry_failed: bool,
    ) -> Result<(), String> {
        self.queue()?
            .resume_update_job_if_idle(job_id, retry_failed)
    }
    pub(crate) fn append_update_job_log(
        &self,
        job_id: &str,
        log_type: &str,
        message: &str,
    ) -> Result<(), String> {
        self.queue()?
            .append_update_job_log(job_id, log_type, message)
    }
    pub(crate) fn update_job_status_value(&self, job_id: &str) -> Result<String, String> {
        self.queue()?.update_job_status_value(job_id)
    }
    pub(crate) fn update_job_mode_value(&self, job_id: &str) -> Result<String, String> {
        self.queue()?.update_job_mode_value(job_id)
    }
    pub(crate) fn list_update_job_item_states(
        &self,
        job_id: &str,
    ) -> Result<Vec<UpdateJobItemState>, String> {
        self.queue()?.list_update_job_item_states(job_id)
    }
    pub(crate) fn list_update_job_held_item_states(
        &self,
        job_id: &str,
    ) -> Result<Vec<UpdateJobItemState>, String> {
        self.queue()?.list_update_job_held_item_states(job_id)
    }
    pub(crate) fn update_job_progress_delta(
        &self,
        job_id: &str,
        changed_item_id: Option<i64>,
    ) -> Result<UpdateJobProgressDelta, String> {
        self.queue()?
            .update_job_progress_delta(job_id, changed_item_id)
    }
    pub(crate) fn next_update_job_item(
        &self,
        job_id: &str,
    ) -> Result<Option<UpdateJobItem>, String> {
        self.queue()?.next_update_job_item(job_id)
    }
    pub(crate) fn insert_update_job_candidate(
        &self,
        job_id: &str,
        candidate: &UpdateJobItemInput,
    ) -> Result<bool, String> {
        self.queue()?.insert_update_job_candidate(job_id, candidate)
    }
    pub(crate) fn queue_update_job_candidates(
        &self,
        job_id: &str,
        candidate_ids: &[i64],
    ) -> Result<i64, String> {
        self.queue()?
            .queue_update_job_candidates(job_id, candidate_ids)
    }
    pub(crate) fn latest_job_candidate(
        &self,
        source: &str,
        key: &str,
    ) -> Result<(String, String), String> {
        self.queue()?.latest_job_candidate(source, key)
    }
    pub(crate) fn resolve_unavailable_job_items(
        &self,
        source: &str,
        keys: &[String],
    ) -> Result<(), String> {
        self.queue()?.resolve_unavailable_job_items(source, keys)
    }
    pub(crate) fn insert_unavailable_queue_item(
        &self,
        job_id: &str,
        item: &UpdateJobItemInput,
        reason: &str,
        known: bool,
    ) -> Result<bool, String> {
        self.queue()?
            .insert_unavailable_queue_item(job_id, item, reason, known)
    }
    pub(crate) fn has_active_update_job(&self) -> Result<bool, String> {
        self.queue()?.has_active_update_job()
    }
    #[cfg(test)]
    pub(crate) fn create_update_job(
        &self,
        job: &str,
        request: &StartUpdateJobRequest,
        items: &[UpdateJobItemInput],
    ) -> Result<UpdateJobSnapshot, String> {
        self.queue()?.create_update_job(job, request, items)
    }
    pub(crate) fn complete_update_job_item(
        &self,
        item: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.complete_saved_item(item, status, error, None)
    }
    pub(crate) fn complete_saved_item(
        &self,
        item: i64,
        status: &str,
        error: Option<&str>,
        work: Option<LibraryId>,
    ) -> Result<(), String> {
        self.queue()?
            .complete_update_job_item(item, status, error, work)
    }
}

#[cfg(test)]
#[path = "job_queue_tests.rs"]
mod tests;
