import { invoke } from "@tauri-apps/api/core";
import { store } from "@/store";

export type UpdateJobStatus =
  | "queued"
  | "running"
  | "paused"
  | "auth_required"
  | "canceling"
  | "canceled"
  | "completed"
  | "failed";

export type UpdateJobScope = "all" | "work" | "author" | "series";
export type UpdateJobMode = "check_only" | "auto_save";

export interface UpdateJobCredentials {
  pixivRefreshToken?: string | null;
  fanboxCookie?: string | null;
  fanboxUserAgent?: string | null;
}

export interface StartUpdateJobRequest {
  scope: UpdateJobScope;
  mode: UpdateJobMode;
  workIds?: number[] | null;
  targetIds?: number[] | null;
  /** Put every work this job saves under update watching. */
  watchSaved?: boolean | null;
  /** Authors or series to check once, without adding them to the watch list. */
  adhocTargets?: { targetType: "author" | "series"; source: string; sourceKey: string; displayName: string }[] | null;
  credentials?: UpdateJobCredentials | null;
}

export interface UpdateJobSummary {
  jobId: string;
  status: UpdateJobStatus;
  scope: UpdateJobScope;
  mode: UpdateJobMode;
  totals: number;
  processed: number;
  candidateCount: number;
  savedCount: number;
  errorCount: number;
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

export interface UpdateJobCandidate {
  id: number;
  key: string;
  source: "pixiv" | "fanbox";
  sourceId: string;
  title: string;
  subtitle: string;
  targetLabel: string;
  targetType: "work" | "author" | "series";
  selected: boolean;
  status: "candidate" | "queued" | "running" | "saved" | "failed" | "skipped" | "done";
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

export async function getUpdateJobCredentials(): Promise<UpdateJobCredentials> {
  return {
    pixivRefreshToken: await store.get<string>("pixiv_refresh_token") || null,
    fanboxCookie: await store.get<string>("fanbox_session_id") || null,
    fanboxUserAgent: await store.get<string>("fanbox_user_agent") || "Mozilla/5.0",
  };
}

export async function startUpdateJobCommand(
  request: Omit<StartUpdateJobRequest, "credentials"> & { credentials?: UpdateJobCredentials | null },
): Promise<UpdateJobSnapshot> {
  const credentials = request.credentials ?? await getUpdateJobCredentials();
  return invoke<UpdateJobSnapshot>("start_update_job", {
    request: {
      ...request,
      credentials,
    },
  });
}

export async function resumeUpdateJobCommand(jobId: string, retryFailed = false): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("resume_update_job", {
    jobId,
    credentials: await getUpdateJobCredentials(),
    retryFailed,
  });
}

/** Stops a work from being offered again, or takes that decision back. */
/** Removes every finished update job from the history. Running ones stay. */
export async function clearFinishedUpdateJobsCommand(): Promise<number> {
  return invoke<number>("clear_finished_update_jobs");
}

export async function dismissUpdateCandidateCommand(source: string, sourceId: string, dismissed: boolean): Promise<void> {
  return invoke<void>("dismiss_update_candidate", { source, sourceId, dismissed });
}

export async function countDismissedUpdateCandidatesCommand(): Promise<number> {
  return invoke<number>("count_dismissed_update_candidates");
}

export async function restoreDismissedUpdateCandidatesCommand(): Promise<number> {
  return invoke<number>("restore_dismissed_update_candidates");
}

export async function saveUpdateJobCandidatesCommand(jobId: string, candidateIds: number[]): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("save_update_job_candidates", {
    jobId,
    candidateIds,
    credentials: await getUpdateJobCredentials(),
  });
}

export function getUpdateJobCommand(jobId: string, candidateAfterId?: number | null, logBeforeId?: number | null): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("get_update_job", { jobId, candidateAfterId, logBeforeId });
}

export function listUpdateJobsCommand(): Promise<UpdateJobSummary[]> {
  return invoke<UpdateJobSummary[]>("list_update_jobs");
}

export function pauseUpdateJobCommand(jobId: string): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("pause_update_job", { jobId });
}

export function cancelUpdateJobCommand(jobId: string): Promise<UpdateJobSnapshot> {
  return invoke<UpdateJobSnapshot>("cancel_update_job", { jobId });
}

export function clearUpdateJobCommand(jobId: string): Promise<void> {
  return invoke<void>("clear_update_job", { jobId });
}
