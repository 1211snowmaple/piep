import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
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
  UpdateJobItemState,
  UpdateJobProgressDelta,
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
  UpdateJobItemState,
  UpdateJobProgressDelta,
  UpdateJobSnapshot,
} from "@/services/updateJobApi";

function mergeSnapshot(
  current: UpdateJobSnapshot | null,
  incoming: UpdateJobSnapshot,
): UpdateJobSnapshot {
  if (!current || current.jobId !== incoming.jobId) return incoming;
  const candidates = new Map(
    current.candidates.map((candidate) => [candidate.id, candidate]),
  );
  incoming.candidates.forEach((candidate) =>
    candidates.set(candidate.id, candidate),
  );
  const logs = new Map(current.logs.map((log) => [log.id, log]));
  incoming.logs.forEach((log) => logs.set(log.id, log));
  return {
    ...incoming,
    candidates: [...candidates.values()].sort((a, b) => a.id - b.id),
    logs: [...logs.values()].sort((a, b) => a.id - b.id),
  };
}

function mergeProgressDelta(
  current: UpdateJobSnapshot,
  delta: UpdateJobProgressDelta,
): UpdateJobSnapshot {
  const logs = delta.latestLog
    ? [
        ...new Map(
          [...current.logs, delta.latestLog].map((log) => [log.id, log]),
        ).values(),
      ].sort((a, b) => a.id - b.id)
    : current.logs;
  const changed = delta.changedItem;
  const candidates =
    changed?.source && changed.sourceId
      ? current.candidates.map((candidate) =>
          candidate.source === changed.source &&
          candidate.sourceId === changed.sourceId
            ? {
                ...candidate,
                status: changed.status as typeof candidate.status,
                error: changed.error,
              }
            : candidate,
        )
      : current.candidates;
  return {
    ...current,
    ...delta.summary,
    logs,
    candidates,
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

/** One status vocabulary shared by the update centre and operation history. */
export const UPDATE_JOB_STATUS_META: Record<
  UpdateJobStatus,
  { label: string; color: string }
> = {
  queued: { label: "待機中", color: "gray" },
  running: { label: "実行中", color: "piep" },
  paused: { label: "一時停止", color: "yellow" },
  auth_required: { label: "再接続が必要", color: "yellow" },
  canceling: { label: "中止中", color: "gray" },
  canceled: { label: "中止", color: "gray" },
  completed: { label: "完了", color: "green" },
  failed: { label: "失敗", color: "red" },
};

// Job summaries are application-level activity, not page-local state. Keeping
// one store prevents the update centre, operation history and sidebar badge
// from loading and interpreting separate copies of the same backend jobs.
const EMPTY_UPDATE_JOB_SUMMARIES: UpdateJobSummary[] = [];
let updateJobSummaries: UpdateJobSummary[] = [];
let updateJobSummaryRevision = 0;
let updateJobSummaryFeedUsers = 0;
let updateJobSummaryRefresh: Promise<UpdateJobSummary[]> | null = null;
let disposeUpdateJobSummarySnapshot: (() => void) | null = null;
let disposeUpdateJobSummaryDelta: (() => void) | null = null;
const updateJobSummaryListeners = new Set<() => void>();

function getUpdateJobSummaries(): UpdateJobSummary[] {
  return updateJobSummaries;
}

function subscribeUpdateJobSummaries(listener: () => void): () => void {
  updateJobSummaryListeners.add(listener);
  return () => updateJobSummaryListeners.delete(listener);
}

function publishUpdateJobSummaries(next: UpdateJobSummary[]): void {
  updateJobSummaries = next;
  updateJobSummaryRevision += 1;
  updateJobSummaryListeners.forEach((listener) => listener());
}

function applyUpdateJobSummary(summary: UpdateJobSummary): void {
  const rest = updateJobSummaries.filter((job) => job.jobId !== summary.jobId);
  publishUpdateJobSummaries(
    [summary, ...rest].sort((a, b) =>
      b.updatedAt.localeCompare(a.updatedAt),
    ),
  );
}

export function refreshUpdateJobSummaries(
  force = false,
): Promise<UpdateJobSummary[]> {
  if (updateJobSummaryRefresh) {
    if (!force) return updateJobSummaryRefresh;
    // A clear/delete may finish while an older list request is still in
    // flight. Wait for it, then issue a genuinely new read so removed rows do
    // not reappear from that stale response.
    return updateJobSummaryRefresh.then(
      () => refreshUpdateJobSummaries(false),
      () => refreshUpdateJobSummaries(false),
    );
  }
  const revisionAtStart = updateJobSummaryRevision;
  updateJobSummaryRefresh = listUpdateJobsCommand()
    .then((incoming) => {
      // An event may arrive while the list command is in flight. In that case,
      // keep the event's newer row instead of replacing it with the older read.
      if (updateJobSummaryRevision === revisionAtStart) {
        publishUpdateJobSummaries(incoming);
      } else {
        const merged = new Map(
          incoming.map((job) => [job.jobId, job] as const),
        );
        updateJobSummaries.forEach((job) => {
          const listed = merged.get(job.jobId);
          if (!listed || job.updatedAt >= listed.updatedAt)
            merged.set(job.jobId, job);
        });
        publishUpdateJobSummaries(
          [...merged.values()]
            .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
            .slice(0, 30),
        );
      }
      return updateJobSummaries;
    })
    .finally(() => {
      updateJobSummaryRefresh = null;
    });
  return updateJobSummaryRefresh;
}

function acquireUpdateJobSummaryFeed(): () => void {
  updateJobSummaryFeedUsers += 1;
  if (updateJobSummaryFeedUsers === 1) {
    disposeUpdateJobSummarySnapshot = subscribeTauriEvent<UpdateJobSnapshot>(
      "update-job-progress",
      (event) => applyUpdateJobSummary(event.payload),
    );
    disposeUpdateJobSummaryDelta = subscribeTauriEvent<UpdateJobProgressDelta>(
      "update-job-progress-delta",
      (event) => applyUpdateJobSummary(event.payload.summary),
    );
  }
  void refreshUpdateJobSummaries().catch((error) => {
    console.error("更新ジョブ一覧を読み込めませんでした", error);
  });
  return () => {
    updateJobSummaryFeedUsers = Math.max(0, updateJobSummaryFeedUsers - 1);
    if (updateJobSummaryFeedUsers !== 0) return;
    disposeUpdateJobSummarySnapshot?.();
    disposeUpdateJobSummaryDelta?.();
    disposeUpdateJobSummarySnapshot = null;
    disposeUpdateJobSummaryDelta = null;
  };
}

export function useUpdateJobSummaries(enabled = true): UpdateJobSummary[] {
  const summaries = useSyncExternalStore(
    subscribeUpdateJobSummaries,
    getUpdateJobSummaries,
    getUpdateJobSummaries,
  );
  useEffect(() => {
    if (!enabled) return undefined;
    return acquireUpdateJobSummaryFeed();
  }, [enabled]);
  return enabled ? summaries : EMPTY_UPDATE_JOB_SUMMARIES;
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

export async function startUpdateJob(
  request: Omit<StartUpdateJobRequest, "credentials"> & {
    credentials?: UpdateJobCredentials | null;
  },
): Promise<UpdateJobSnapshot> {
  return startUpdateJobCommand(request);
}

export async function resumeUpdateJob(
  jobId: string,
  retryFailed = false,
): Promise<UpdateJobSnapshot> {
  return resumeUpdateJobCommand(jobId, retryFailed);
}

export async function saveUpdateJobCandidates(
  jobId: string,
  candidateIds: number[],
): Promise<UpdateJobSnapshot> {
  return saveUpdateJobCandidatesCommand(jobId, candidateIds);
}

export async function waitForUpdateJob(
  jobId: string,
  onSnapshot?: (snapshot: UpdateJobSnapshot) => void,
  onItemState?: (state: UpdateJobItemState) => void,
): Promise<UpdateJobSnapshot> {
  let snapshot = await getUpdateJobCommand(jobId);
  onSnapshot?.(snapshot);
  if (
    isUpdateJobTerminal(snapshot.status) ||
    snapshot.status === "paused" ||
    snapshot.status === "auth_required"
  )
    return snapshot;

  // Progress events are emitted per item. Reading the complete snapshot on a
  // fixed 900ms timer made a large save do the same expensive IPC/read work a
  // second time, and also caused the page to render from two competing clocks.
  // Listen to the worker as the primary path and retain a slow silence check as
  // a recovery path for a missed listener or a worker that stopped emitting.
  let lastEventAt = Date.now();
  let settled = false;
  let unlistenSnapshot: (() => void) | undefined;
  let unlistenDelta: (() => void) | undefined;
  let checkTimer: number | undefined;
  let resolveFinal!: (result: UpdateJobSnapshot) => void;
  const done = new Promise<UpdateJobSnapshot>((resolve) => {
    resolveFinal = resolve;
  });
  const finish = (result: UpdateJobSnapshot, notify = true) => {
    if (settled || result.jobId !== jobId) return;
    settled = true;
    if (checkTimer !== undefined) window.clearInterval(checkTimer);
    unlistenSnapshot?.();
    unlistenDelta?.();
    snapshot = result;
    if (notify) onSnapshot?.(result);
    resolveFinal(result);
  };
  unlistenSnapshot = subscribeTauriEvent<UpdateJobSnapshot>(
    "update-job-progress",
    (event) => {
      if (event.payload.jobId !== jobId || settled) return;
      lastEventAt = Date.now();
      snapshot = mergeSnapshot(snapshot, event.payload);
      onSnapshot?.(snapshot);
      if (
        isUpdateJobTerminal(event.payload.status) ||
        event.payload.status === "paused" ||
        event.payload.status === "auth_required"
      )
        finish(snapshot, false);
    },
  );
  unlistenDelta = subscribeTauriEvent<UpdateJobProgressDelta>(
    "update-job-progress-delta",
    (event) => {
      if (event.payload.summary.jobId !== jobId || settled) return;
      lastEventAt = Date.now();
      snapshot = mergeProgressDelta(snapshot, event.payload);
      if (event.payload.changedItem) onItemState?.(event.payload.changedItem);
      onSnapshot?.(snapshot);
      if (
        isUpdateJobTerminal(snapshot.status) ||
        snapshot.status === "paused" ||
        snapshot.status === "auth_required"
      )
        finish(snapshot, false);
    },
  );
  checkTimer = window.setInterval(() => {
    if (settled || Date.now() - lastEventAt < EVENT_SILENCE_MS) return;
    getUpdateJobCommand(jobId)
      .then((result) => {
        if (settled) return;
        lastEventAt = Date.now();
        snapshot = mergeSnapshot(snapshot, result);
        onSnapshot?.(snapshot);
        if (
          isUpdateJobTerminal(result.status) ||
          result.status === "paused" ||
          result.status === "auth_required"
        )
          finish(snapshot, false);
      })
      .catch((error) => {
        // A transient IPC failure must not turn a still-running background
        // save into a false failure. Keep the listener alive and try again on
        // the next silence interval.
        console.warn(
          `更新ジョブ ${jobId} の進捗を再取得できませんでした`,
          error,
        );
        lastEventAt = Date.now();
      });
  }, SILENCE_CHECK_MS);
  try {
    return await done;
  } finally {
    if (!settled) {
      settled = true;
      if (checkTimer !== undefined) window.clearInterval(checkTimer);
      unlistenSnapshot?.();
      unlistenDelta?.();
    }
  }
}

/**
 * How long the screen waits for an event before it asks the database itself.
 *
 * The worker emits a small delta after every item, so polling is only a safety
 * net for events that never arrive (a missed listener, a worker that died).
 */
const EVENT_SILENCE_MS = 5_000;
/** How often to notice that the silence has lasted long enough. */
const SILENCE_CHECK_MS = 1_500;

export function useUpdateJobs(
  onSnapshot?: (snapshot: UpdateJobSnapshot) => void,
  enabled = true,
) {
  const queryClient = useQueryClient();
  const jobs = useUpdateJobSummaries(enabled);
  const [activeSnapshot, setActiveSnapshot] =
    useState<UpdateJobSnapshot | null>(null);
  const lastEventAt = useRef(0);

  const loadJobs = useCallback(async (force = true) => {
    if (!enabled) {
      setActiveSnapshot(null);
      return;
    }
    const nextJobs = await refreshUpdateJobSummaries(force);
    const preferred =
      nextJobs.find((job) => !isUpdateJobTerminal(job.status)) ?? nextJobs[0];
    if (preferred) {
      const snapshot = await getUpdateJobCommand(preferred.jobId);
      setActiveSnapshot(snapshot);
      onSnapshot?.(snapshot);
    } else {
      setActiveSnapshot(null);
    }
  }, [enabled, onSnapshot]);

  const selectJob = useCallback(
    async (jobId: string) => {
      if (!enabled) return;
      const snapshot = await getUpdateJobCommand(jobId);
      setActiveSnapshot(snapshot);
      onSnapshot?.(snapshot);
      return snapshot;
    },
    [enabled, onSnapshot],
  );

  useEffect(() => {
    if (!enabled) return undefined;
    loadJobs(false).catch((error) => {
      console.error("更新ジョブ一覧を読み込めませんでした", error);
    });
  }, [enabled, loadJobs]);

  useEffect(() => {
    if (!enabled) return undefined;
    const disposeSnapshot = subscribeTauriEvent<UpdateJobSnapshot>(
      "update-job-progress",
      (event) => {
        lastEventAt.current = Date.now();
        setActiveSnapshot((current) => mergeSnapshot(current, event.payload));
        onSnapshot?.(event.payload);
        if (isUpdateJobTerminal(event.payload.status))
          invalidateAfterUpdateJob(queryClient);
      },
    );
    const disposeDelta = subscribeTauriEvent<UpdateJobProgressDelta>(
      "update-job-progress-delta",
      (event) => {
        lastEventAt.current = Date.now();
        setActiveSnapshot((current) => {
          if (!current || current.jobId !== event.payload.summary.jobId)
            return current;
          const next = mergeProgressDelta(current, event.payload);
          onSnapshot?.(next);
          return next;
        });
        if (isUpdateJobTerminal(event.payload.summary.status))
          invalidateAfterUpdateJob(queryClient);
      },
    );
    return () => {
      disposeSnapshot();
      disposeDelta();
    };
  }, [enabled, onSnapshot, queryClient]);

  // 進捗はイベントで届く。ここはその取りこぼしに備える保険なので、
  // イベントが途切れているときだけ読みに行く。止まっているジョブ
  // （一時停止・再接続待ち）は誰も進めないので、待つ相手がいない。
  useEffect(() => {
    if (
      !enabled ||
      !activeSnapshot ||
      !isUpdateJobActive(activeSnapshot.status)
    )
      return;
    const id = window.setInterval(() => {
      if (Date.now() - lastEventAt.current < EVENT_SILENCE_MS) return;
      getUpdateJobCommand(activeSnapshot.jobId)
        .then((snapshot) => {
          setActiveSnapshot((current) => mergeSnapshot(current, snapshot));
          onSnapshot?.(snapshot);
        })
        .catch((error) => {
          console.warn(
            `更新ジョブ ${activeSnapshot.jobId} の進捗を再取得できませんでした`,
            error,
          );
        });
    }, SILENCE_CHECK_MS);
    return () => window.clearInterval(id);
  }, [activeSnapshot, enabled, onSnapshot]);

  const loadMoreCandidates = useCallback(async () => {
    if (!enabled || !activeSnapshot?.nextCandidateCursor) return;
    const snapshot = await getUpdateJobCommand(
      activeSnapshot.jobId,
      activeSnapshot.nextCandidateCursor,
      null,
    );
    setActiveSnapshot((current) => mergeSnapshot(current, snapshot));
  }, [activeSnapshot, enabled]);

  const loadOlderLogs = useCallback(async () => {
    if (!enabled || !activeSnapshot?.previousLogCursor) return;
    const snapshot = await getUpdateJobCommand(
      activeSnapshot.jobId,
      null,
      activeSnapshot.previousLogCursor,
    );
    setActiveSnapshot((current) => mergeSnapshot(current, snapshot));
    return snapshot;
  }, [activeSnapshot, enabled]);

  return useMemo(
    () => ({
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
    }),
    [
      activeSnapshot,
      jobs,
      loadJobs,
      loadMoreCandidates,
      loadOlderLogs,
      selectJob,
    ],
  );
}
