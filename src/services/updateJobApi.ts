import { invoke } from "@tauri-apps/api/core";
import type { WatchId, WorkId } from "@/types/library";

export type UpdateJobStatus =
  | "queued"
  | "running"
  | "paused"
  | "auth_required"
  | "canceling"
  | "canceled"
  | "completed"
  | "failed";

/** `save` は「選んだ作品をまとめて保存する」ジョブ。監視対象は見に行かない。 */
/** `targets` は監視グループの確認。`targetIds` に挙げた作者・シリーズだけを見る。 */
/** `favorite`・`reading` は棚の範囲（棚が今その範囲にある作品を選ぶ）。 */
export type UpdateJobScope = "all" | "work" | "author" | "series" | "targets" | "save" | "favorite" | "reading";
export type UpdateJobMode = "check_only" | "auto_save" | "save";

export interface StartUpdateJobRequest {
  scope: UpdateJobScope;
  mode: UpdateJobMode;
  workIds?: WorkId[] | null;
  targetIds?: WatchId[] | null;
  /** Put every work this job saves under update watching. */
  watchSaved?: boolean | null;
  /** 作者とシリーズの新作だけを見る軽い確認にする。改稿は見ない。 */
  newWorksOnly?: boolean | null;
  /** Authors or series to check once, without adding them to the watch list. */
  adhocTargets?:
    | {
        targetType: "author" | "series";
        source: string;
        sourceKey: string;
        displayName: string;
      }[]
    | null;
}

export interface UpdateJobSummary {
  jobId: string;
  status: UpdateJobStatus;
  scope: UpdateJobScope;
  mode: UpdateJobMode;
  newWorksOnly?: boolean;
  /** Filled by the history list when one author, series, or work was checked. */
  subjectLabel?: string | null;
  subjectSource?: string | null;
  totals: number;
  processed: number;
  /** Separate local job stages; older snapshots may omit these fields. */
  checkTotal?: number;
  checkProcessed?: number;
  saveTotal?: number;
  saveProcessed?: number;
  candidateCount: number;
  savedCount: number;
  errorCount: number;
  /** この回で新しく保留になった、または保留の種類が変わった件数。 */
  heldCount?: number;
  /** 前の回から同じ理由で保留のままの件数。確かめたが、知らせることはない。 */
  knownHeldCount?: number;
  activeLabel: string | null;
  startedAt: string;
  updatedAt: string;
  finishedAt: string | null;
}

export interface UpdateJobLog {
  id: number;
  logType: "info" | "success" | "warn" | "error";
  message: string;
  createdAt: string;
}

export interface UpdateWorkMetadata { authorName?: string | null; publishedAt?: string | null; updatedAt?: string | null; savedAt?: string | null }

export interface UpdateJobCandidate {
  metadata?: UpdateWorkMetadata;
  id: number;
  workId?: string | null;
  key: string;
  source: "pixiv" | "fanbox";
  sourceId: string;
  title: string;
  subtitle: string;
  targetLabel: string;
  targetType: "work" | "author" | "series";
  selected: boolean;
  status:
    | "candidate"
    | "queued"
    | "running"
    | "saved"
    | "failed"
    | "held"
    | "skipped"
    | "done";
  /** Why this is a candidate: a work we lack, a sequel, or a rewrite of one we have. */
  kind: "new" | "sequel" | "revision";
  /** Set when the candidate failed; carries the classified reason. */
  error?: string | null;
}

export interface UpdateJobSnapshot extends UpdateJobSummary {
  logs: UpdateJobLog[];
  candidates: UpdateJobCandidate[];
  nextCandidateCursor: number | null;
  previousLogCursor: number | null;
}

/**
 * 更新確認を始める。
 *
 * 鍵は渡さない。バックエンドが保存してある接続から自分で読む。
 */
export function startUpdateJobCommand(request: StartUpdateJobRequest): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("start_update_job", { request });
}

/** まとめ保存に渡す1件。取得元とIDが分かれば、本文は向こうが取りに行く。 */
export interface SaveJobWork {
  source: "pixiv" | "fanbox";
  sourceId: string;
  title: string;
  fanboxAccess?: { isRestricted: boolean; accountKey: string; feeRequired?: number; creatorId?: string };
}

export interface DeferredUpdateCandidate {
  workId?: string | null;
  metadata?: UpdateWorkMetadata;
  source: "pixiv" | "fanbox";
  sourceId: string;
  title: string;
  status: "held" | "dismissed";
  payloadJson: string;
}

export function listDeferredUpdateCandidatesCommand(): Promise<DeferredUpdateCandidate[]> {
  return invoke("list_deferred_update_candidates");
}

export function countSavedDeferredUpdateCandidatesCommand(): Promise<number> {
  return invoke("count_saved_deferred_update_candidates");
}

/** 一部保存の補完で取り直す作品。`snapshot` は候補にしたときの版（試した印はこの版に付ける）。 */
export interface IncompleteSaveRepairWork extends SaveJobWork {
  snapshot: string;
}

/** 一部保存のうち、今の版でまだ取り直していない作品。 */
export function listIncompleteSaveRepairWorksCommand(limit: number): Promise<IncompleteSaveRepairWork[]> {
  return invoke("list_incomplete_save_repair_works", { limit });
}

/** 補完の保存ジョブへ渡した作品を、候補にしたときの版で試したと記録する。 */
export function recordIncompleteSaveRepairAttemptsCommand(attempts: Array<{ sourceId: string; snapshot: string }>): Promise<void> {
  return invoke("record_incomplete_save_repair_attempts", { attempts });
}

export function recheckDeferredUpdateCandidatesCommand(keys: string[], refreshPostAccess = false): Promise<UpdateJobSnapshot> {
  return invoke("recheck_deferred_update_candidates", { keys, refreshPostAccess });
}

/**
 * 選んだ作品のまとめ保存を、Rust 側の仕事として預ける。
 *
 * 画面が閉じても、落ちても、保存は走り続ける。返ってくるのは開始直後の様子で、
 * 続きは `getUpdateJobCommand` と `update-job` イベントで追う。
 */
export function startSaveJobCommand(
  works: SaveJobWork[],
  watchSaved: boolean,
): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("start_save_job", { works, watchSaved });
}

/** ジョブの項目ひとつひとつの、いまの状態。行に印を付けるために使う。 */
export interface UpdateJobItemState {
  metadata?: UpdateWorkMetadata;
  source: string | null;
  sourceId: string | null;
  status: string;
  error: string | null;
  /** Included in explicit item lists; omitted from small progress events. */
  itemType?: string | null;
  title?: string | null;
  workId?: string | null;
}

/** Small event emitted for one live job transition. Full pages stay on getUpdateJob. */
export interface UpdateJobProgressDelta {
  summary: UpdateJobSummary;
  changedItem: UpdateJobItemState | null;
  latestLog: UpdateJobLog | null;
}

export async function listUpdateJobItemStatesCommand(
  jobId: string,
): Promise<UpdateJobItemState[]> {
  return invoke<UpdateJobItemState[]>("list_update_job_item_states", { jobId });
}

/** Only held rows, for the live results panel of a large running job. */
export async function listUpdateJobHeldItemStatesCommand(
  jobId: string,
): Promise<UpdateJobItemState[]> {
  return invoke<UpdateJobItemState[]>("list_update_job_held_item_states", { jobId });
}

export async function resumeUpdateJobCommand(
  jobId: string,
  retryFailed = false,
): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("resume_update_job", {
    jobId,
    retryFailed,
  });
}

/** Stops a work from being offered again, or takes that decision back. */
/** Removes every finished update job from the history. Running ones stay. */
export async function clearFinishedUpdateJobsCommand(): Promise<number> {
  return invoke<number>("clear_finished_update_jobs");
}

export async function dismissUpdateCandidateCommand(
  source: string,
  sourceId: string,
  dismissed: boolean,
): Promise<void> {
  return invoke<void>("dismiss_update_candidate", {
    source,
    sourceId,
    dismissed,
  });
}

export async function countDismissedUpdateCandidatesCommand(): Promise<number> {
  return invoke<number>("count_dismissed_update_candidates");
}

export async function restoreDismissedUpdateCandidatesCommand(): Promise<number> {
  return invoke<number>("restore_dismissed_update_candidates");
}

/** まだ取り込んでいない改稿。 */
export interface PendingRevision {
  /** 作品の画面と同じ ID の文字列（作品の UUID）。 */
  work: string;
  /** 更新確認がこの改稿を見つけた時刻。取得元が直した日そのものは持っていない。 */
  foundAt: string;
}

/** A provider revision fetched for comparison but not accepted into history. */
export interface PendingRevisionPreview {
  metadata?: import("./workInspectionApi").RevisionMetadata;
  work: string;
  baseVersion: number;
  text: string;
  textLength: number;
}

/**
 * まだ取り込んでいない改稿の一覧。
 *
 * 作品の一覧に列を足さず、これを一度だけ引いて突き合わせる。
 * 「取得元のほうが新しい」は作品の属性ではなく、更新確認が見つけた事実なので。
 */
export async function listPendingRevisionsCommand(): Promise<
  PendingRevision[]
> {
  return invoke<PendingRevision[]>("list_pending_revisions");
}

/** Fetch the pending provider body without creating a saved version. */
export async function previewPendingRevisionCommand(
  work: string,
): Promise<PendingRevisionPreview> {
  return invoke<PendingRevisionPreview>("preview_pending_revision", { work });
}

export async function saveUpdateJobCandidatesCommand(
  jobId: string,
  candidateIds: number[],
): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("save_update_job_candidates", {
    jobId,
    candidateIds,
  });
}

export function getUpdateJobCommand(
  jobId: string,
  candidateAfterId?: number | null,
  logBeforeId?: number | null,
): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("get_update_job", {
    jobId,
    candidateAfterId,
    logBeforeId,
  });
}

export function listUpdateJobsCommand(): Promise<UpdateJobSummary[]> {
  return invoke<UpdateJobSummary[]>("list_update_jobs");
}

export function pauseUpdateJobCommand(
  jobId: string,
): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("pause_update_job", { jobId });
}

export function cancelUpdateJobCommand(
  jobId: string,
): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("cancel_update_job", { jobId });
}

export function clearUpdateJobCommand(jobId: string): Promise<void> {
  return invoke<void>("clear_update_job", { jobId });
}
