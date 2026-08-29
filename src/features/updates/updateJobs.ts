import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { subscribeTauriEvent } from "@/services/eventBus";
import { invalidateWorkSetViews } from "@/features/library/workSetInvalidation";
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

import type {
  StartUpdateJobRequest,
  UpdateJobCredentials,
  UpdateJobSnapshot,
  UpdateJobStatus,
  UpdateJobSummary,
} from "@/services/updateJobApi";

// 形は IPC の側が持つ。ここで同じものを書き写していたころ、`updateJobApi` に
// 種類をひとつ足すたびに、こちらでも同じ手を入れないと型が食い違った。
// **同じ形をふたつ置かない。**
export type {
  UpdateJobStatus,
  UpdateJobScope,
  UpdateJobMode,
  UpdateJobCredentials,
  StartUpdateJobRequest,
  UpdateJobSummary,
  UpdateJobLog,
  UpdateJobCandidate,
  UpdateJobSnapshot,
} from "@/services/updateJobApi";

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

/**
 * 更新確認が終わったあと、古くなるもの。
 *
 * 改稿の印（カード・作品ページ）と「改稿あり」棚の件数は、確認が見つけた事実
 * から引いている。読む側はどれも別の画面にいるので、**変えた側から知らせる**。
 * これを忘れると、確認が終わっても画面が前の答えを出しつづける。
 *
 * 「自動保存」を選んだジョブは、確認するだけでなく**作品そのものを増やす**。
 * 増えた分が古くするのは、消したときに古くなるものと同じ場所である。
 * 一覧の定義は `deletedWorkCleanup.ts` にあるので、そちらに合わせる。
 */
export function invalidateAfterUpdateJob(client: QueryClient): void {
  client.invalidateQueries({ queryKey: ["pending-revisions"] });
  client.invalidateQueries({ queryKey: ["library"] });
  invalidateWorkSetViews(client);
}

export function isUpdateJobTerminal(status: UpdateJobStatus): boolean {
  return status === "completed" || status === "failed" || status === "canceled";
}

/**
 * 動かしている worker が付いているか。
 *
 * `paused` と `auth_required` は終わってもいないが、誰も進めていない。
 * 「終わっていない＝任せておけばよい」で判断すると、止まったままの1件が
 * 以後の自動確認を永久に塞ぐ。待つべき相手はこちらで数える。
 */
export function isUpdateJobActive(status: UpdateJobStatus): boolean {
  return status === "queued" || status === "running" || status === "canceling";
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
  const queryClient = useQueryClient();
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
    loadJobs().catch((error) => {
      console.error("更新ジョブ一覧を読み込めませんでした", error);
    });
  }, [enabled, loadJobs]);

  useEffect(() => {
    if (!enabled) return undefined;
    return subscribeTauriEvent<UpdateJobSnapshot>("update-job-progress", event => {
      lastEventAt.current = Date.now();
      setActiveSnapshot(current => mergeSnapshot(current, event.payload));
      onSnapshot?.(event.payload);
      if (isUpdateJobTerminal(event.payload.status)) invalidateAfterUpdateJob(queryClient);
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
  }, [enabled, onSnapshot, queryClient]);

  // 進捗はイベントで届く。ここはその取りこぼしに備える保険なので、
  // イベントが途切れているときだけ読みに行く。止まっているジョブ
  // （一時停止・再接続待ち）は誰も進めないので、待つ相手がいない。
  useEffect(() => {
    if (!enabled || !activeSnapshot || !isUpdateJobActive(activeSnapshot.status)) return;
    const id = window.setInterval(() => {
      if (Date.now() - lastEventAt.current < EVENT_SILENCE_MS) return;
      getUpdateJobCommand(activeSnapshot.jobId)
        .then(snapshot => {
          setActiveSnapshot(current => mergeSnapshot(current, snapshot));
          onSnapshot?.(snapshot);
        })
        .catch((error) => {
          console.warn(`更新ジョブ ${activeSnapshot.jobId} の進捗を再取得できませんでした`, error);
        });
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
