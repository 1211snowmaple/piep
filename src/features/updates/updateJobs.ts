import { useCallback, useEffect, useMemo, useState } from "react";
import { onTauriEvent } from "@/services/eventBus";
import {
  cancelUpdateJobCommand,
  clearUpdateJobCommand,
  getUpdateJobCommand,
  getUpdateJobCredentials,
  listUpdateJobsCommand,
  pauseUpdateJobCommand,
  resumeUpdateJobCommand,
  saveUpdateJobCandidatesCommand,
  startUpdateJobCommand,
} from "@/services/updateJobApi";

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
  credentials?: UpdateJobCredentials | null;
  concurrency?: {
    fetch?: number | null;
    save?: number | null;
    collection?: number | null;
  } | null;
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
}

export interface UpdateJobSnapshot extends UpdateJobSummary {
  logs: UpdateJobLog[];
  candidates: UpdateJobCandidate[];
}

export function isUpdateJobTerminal(status: UpdateJobStatus): boolean {
  return status === "completed" || status === "failed" || status === "canceled";
}

export { getUpdateJobCredentials };

export async function startUpdateJob(request: Omit<StartUpdateJobRequest, "credentials"> & { credentials?: UpdateJobCredentials | null }): Promise<UpdateJobSnapshot> {
  return startUpdateJobCommand(request);
}

export async function resumeUpdateJob(jobId: string, retryFailed = false): Promise<UpdateJobSnapshot> {
  return resumeUpdateJobCommand(jobId, retryFailed);
}

export async function saveUpdateJobCandidates(jobId: string, candidateIds: number[]): Promise<UpdateJobSnapshot> {
  return saveUpdateJobCandidatesCommand(jobId, candidateIds);
}

export async function waitForUpdateJob(jobId: string, onSnapshot?: (snapshot: UpdateJobSnapshot) => void): Promise<UpdateJobSnapshot> {
  let snapshot = await getUpdateJobCommand(jobId);
  onSnapshot?.(snapshot);
  while (!isUpdateJobTerminal(snapshot.status) && snapshot.status !== "paused" && snapshot.status !== "auth_required") {
    await new Promise(resolve => window.setTimeout(resolve, 900));
    snapshot = await getUpdateJobCommand(jobId);
    onSnapshot?.(snapshot);
  }
  return snapshot;
}

export function useUpdateJobs(onSnapshot?: (snapshot: UpdateJobSnapshot) => void, enabled = true) {
  const [jobs, setJobs] = useState<UpdateJobSummary[]>([]);
  const [activeSnapshot, setActiveSnapshot] = useState<UpdateJobSnapshot | null>(null);

  const loadJobs = useCallback(async () => {
    if (!enabled) {
      setJobs([]);
      setActiveSnapshot(null);
      return;
    }
    const nextJobs = await listUpdateJobsCommand();
    setJobs(nextJobs);
    const preferred = nextJobs.find(job => !isUpdateJobTerminal(job.status)) ?? nextJobs[0];
    if (preferred) {
      const snapshot = await getUpdateJobCommand(preferred.jobId);
      setActiveSnapshot(snapshot);
      onSnapshot?.(snapshot);
    } else {
      setActiveSnapshot(null);
    }
  }, [enabled, onSnapshot]);

  const selectJob = useCallback(async (jobId: string) => {
    if (!enabled) return;
    const snapshot = await getUpdateJobCommand(jobId);
    setActiveSnapshot(snapshot);
    onSnapshot?.(snapshot);
  }, [enabled, onSnapshot]);

  useEffect(() => {
    if (!enabled) return undefined;
    loadJobs().catch(() => undefined);
  }, [loadJobs]);

  useEffect(() => {
    if (!enabled) return undefined;
    let unlisten: (() => void) | undefined;
    onTauriEvent<UpdateJobSnapshot>("update-job-progress", event => {
      setActiveSnapshot(event.payload);
      onSnapshot?.(event.payload);
      setJobs(prev => {
        const summary: UpdateJobSummary = {
          jobId: event.payload.jobId,
          status: event.payload.status,
          scope: event.payload.scope,
          mode: event.payload.mode,
          totals: event.payload.totals,
          processed: event.payload.processed,
          candidateCount: event.payload.candidateCount,
          savedCount: event.payload.savedCount,
          errorCount: event.payload.errorCount,
          activeLabel: event.payload.activeLabel,
          startedAt: event.payload.startedAt,
          updatedAt: event.payload.updatedAt,
          finishedAt: event.payload.finishedAt,
        };
        const rest = prev.filter(job => job.jobId !== summary.jobId);
        return [summary, ...rest].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
      });
    }).then(dispose => {
      unlisten = dispose;
    }).catch(() => undefined);
    return () => unlisten?.();
  }, [enabled, onSnapshot]);

  useEffect(() => {
    if (!enabled || !activeSnapshot || isUpdateJobTerminal(activeSnapshot.status)) return;
    const id = window.setInterval(() => {
      getUpdateJobCommand(activeSnapshot.jobId)
        .then(snapshot => {
          setActiveSnapshot(snapshot);
          onSnapshot?.(snapshot);
        })
        .catch(() => undefined);
    }, 1500);
    return () => window.clearInterval(id);
  }, [activeSnapshot, enabled, onSnapshot]);

  return useMemo(() => ({
    jobs,
    activeSnapshot,
    loadJobs,
    selectJob,
    start: startUpdateJob,
    pause: pauseUpdateJobCommand,
    resume: resumeUpdateJob,
    cancel: cancelUpdateJobCommand,
    clear: clearUpdateJobCommand,
    saveCandidates: saveUpdateJobCandidates,
  }), [activeSnapshot, jobs, loadJobs, selectJob]);
}
