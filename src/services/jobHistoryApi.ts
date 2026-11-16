import { invoke } from "@tauri-apps/api/core";

/** 棚に封をした実行当時の記録。ID は棚の UUID、日時はミリ秒。 */
export interface StoredJobHistory {
  id: string;
  origin: string;
  scope: string;
  mode: string;
  status: string;
  watch_saved: boolean;
  new_works_only: boolean;
  totals: number;
  processed: number;
  candidate_count: number;
  saved_count: number;
  error_count: number;
  active_label: string | null;
  started_at: number;
  updated_at: number;
  finished_at: number | null;
}
export interface StoredJobItem {
  id: string;
  sequence: number;
  item_type: string;
  entity: string | null;
  target_type: string | null;
  title: string;
  payload_file: string;
  status: string;
  error: string | null;
  result_work: string | null;
  created_at: number;
  updated_at: number;
}
export interface StoredJobLog {
  id: string;
  sequence: number;
  log_type: string;
  message: string;
  created_at: number;
}
export const storedHistoryKey = ["library-v2-job-history"] as const;
export const HISTORY_PAGE = 20;
export const HISTORY_DETAIL_PAGE = 100;

export function storedJobHistory(before: string | null) {
  return invoke<StoredJobHistory[]>("library_v2_job_history", { before, limit: HISTORY_PAGE });
}
export function storedJobItems(job: string, after: number | null) {
  return invoke<StoredJobItem[]>("library_v2_job_history_items", { job, after, limit: HISTORY_DETAIL_PAGE });
}
export function storedJobLogs(job: string, after: number | null) {
  return invoke<StoredJobLog[]>("library_v2_job_history_logs", { job, after, limit: HISTORY_DETAIL_PAGE });
}
export function setStoredJobHidden(job: string | null, hidden: boolean) {
  return invoke<number>("library_v2_job_history_visibility", { request: crypto.randomUUID(), job, hidden });
}
