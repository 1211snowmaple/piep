import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { subscribeTauriEvent } from "@/services/eventBus";
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
  /** Put every work this job saves under update watching. */
  watchSaved?: boolean | null;
  /** Authors or series to check once, without adding them to the watch list. */
  adhocTargets?: { targetType: "author" | "series"; source: string; sourceKey: string; displayName: string }[] | null;
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

function mergeSnapshot(current: UpdateJobSnapshot | null, incoming: UpdateJobSnapshot): UpdateJobSnapshot {
  if (!current || current.jobId !== incoming.jobId) return incoming;
  const candidates = new Map(current.candidates.map(candidate => [candidate.id, candidate]));
  incoming.candidates.forEach(candidate => candidates.set(candidate.id, candidate));
  const logs = new Map(current.logs.map(log => [log.id, log]));
  incoming.logs.forEach(log => logs.set(log.id, log));
  return {
    ...incoming,
    candidates: [...candidates.values()].sort((a, b) => a.id - b.id),
    logs: [...logs.values()].sort((a, b) => a.id - b.id),
  };
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

/**
 * How long the screen waits for an event before it asks the database itself.
 *
 * The worker emits a snapshot after every item, so polling is only a safety
 * net for events that never arrive (a missed listener, a worker that died).
 * Polling on a fixed timer as well meant reading the whole snapshot twice a
 * second for the entire length of a job.
 */
const EVENT_SILENCE_MS = 5_000;
/** How often to notice that the silence has lasted long enough. */
const SILENCE_CHECK_MS = 1_500;

export function useUpdateJobs(onSnapshot?: (snapshot: UpdateJobSnapshot) => void, enabled = true) {
  const [jobs, setJobs] = useState<UpdateJobSummary[]>([]);
  const [activeSnapshot, setActiveSnapshot] = useState<UpdateJobSnapshot | null>(null);
  const lastEventAt = useRef(0);

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
    return subscribeTauriEvent<UpdateJobSnapshot>("update-job-progress", event => {
      lastEventAt.current = Date.now();
      setActiveSnapshot(current => mergeSnapshot(current, event.payload));
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
    });
  }, [enabled, onSnapshot]);

  // 進捗はイベントで届く。ここはその取りこぼしに備える保険なので、
  // イベントが途切れているときだけ読みに行く。
  useEffect(() => {
    if (!enabled || !activeSnapshot || isUpdateJobTerminal(activeSnapshot.status)) return;
    const id = window.setInterval(() => {
      if (Date.now() - lastEventAt.current < EVENT_SILENCE_MS) return;
      getUpdateJobCommand(activeSnapshot.jobId)
        .then(snapshot => {
          setActiveSnapshot(current => mergeSnapshot(current, snapshot));
          onSnapshot?.(snapshot);
        })
        .catch(() => undefined);
    }, SILENCE_CHECK_MS);
    return () => window.clearInterval(id);
  }, [activeSnapshot, enabled, onSnapshot]);

  const loadMoreCandidates = useCallback(async () => {
    if (!enabled || !activeSnapshot?.nextCandidateCursor) return;
    const snapshot = await getUpdateJobCommand(activeSnapshot.jobId, activeSnapshot.nextCandidateCursor, null);
    setActiveSnapshot(current => mergeSnapshot(current, snapshot));
  }, [activeSnapshot, enabled]);

  const loadOlderLogs = useCallback(async () => {
    if (!enabled || !activeSnapshot?.previousLogCursor) return;
    const snapshot = await getUpdateJobCommand(activeSnapshot.jobId, null, activeSnapshot.previousLogCursor);
    setActiveSnapshot(current => mergeSnapshot(current, snapshot));
  }, [activeSnapshot, enabled]);

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
    loadMoreCandidates,
    loadOlderLogs,
  }), [activeSnapshot, jobs, loadJobs, loadMoreCandidates, loadOlderLogs, selectJob]);
}
