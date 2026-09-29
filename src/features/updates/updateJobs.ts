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

export const MAX_LIVE_UPDATE_LOGS = 2_000;

function mergeSnapshot(
  current: UpdateJobSnapshot | null,
  incoming: UpdateJobSnapshot,
): UpdateJobSnapshot {
  if (!current || current.jobId !== incoming.jobId) return incoming;
  const candidates = new Map(
    current.candidates.map((candidate) => [candidate.id, candidate]),
  );
  const incomingIsNewer = incoming.updatedAt >= current.updatedAt;
  incoming.candidates.forEach((candidate) => {
    if (incomingIsNewer || !candidates.has(candidate.id)) candidates.set(candidate.id, candidate);
  });
  const logs = new Map(current.logs.map((log) => [log.id, log]));
  incoming.logs.forEach((log) => logs.set(log.id, log));
  const summary = incomingIsNewer ? incoming : current;
  const mergedCandidates = [...candidates.values()].sort((a, b) => a.id - b.id);
  return {
    ...summary,
    candidates: mergedCandidates,
    nextCandidateCursor: summary.candidateCount > mergedCandidates.length
      ? mergedCandidates[mergedCandidates.length - 1]?.id ?? null
      : null,
    logs: [...logs.values()].sort((a, b) => a.id - b.id).slice(-MAX_LIVE_UPDATE_LOGS),
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
      ].sort((a, b) => a.id - b.id).slice(-MAX_LIVE_UPDATE_LOGS)
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
  client.invalidateQueries({ queryKey: ["deferred-candidates"] });
  client.invalidateQueries({ queryKey: ["library"] });
  // 監視作者の確認はプロフィール（名前・アイコン・紹介文）も更新する。
  // 見出しだけでなく、開いている履歴・取得データも同じ版へ揃える。
  client.invalidateQueries({ queryKey: ["entity-versions"] });
  client.invalidateQueries({ queryKey: ["entity-json"] });
  invalidateWorkSetViews(client);
}

export function isUpdateJobTerminal(status: UpdateJobStatus): boolean {
  return status === "completed" || status === "failed" || status === "canceled";
}

/**
 * 画面を開いたとき、自動で表に出すジョブ。
 *
 * 中止した回は履歴には残すが、進捗カードへ自動で戻さない。内容を確認したい
 * ときは履歴から明示的に選べる。実行中・停止中の回は、古い完了結果より先に出す。
 */
export function preferredVisibleUpdateJob(
  jobs: UpdateJobSummary[],
): UpdateJobSummary | undefined {
  return (
    jobs.find((job) => !isUpdateJobTerminal(job.status)) ??
    jobs.find((job) => job.status !== "canceled")
  );
}

/** 中止イベントが、いま見ている別のジョブまで追い出さないように畳む。 */
export function mergeVisibleUpdateJobSnapshot(
  current: UpdateJobSnapshot | null,
  incoming: UpdateJobSnapshot,
): UpdateJobSnapshot | null {
  if (incoming.status !== "canceled") return mergeSnapshot(current, incoming);
  return current?.jobId === incoming.jobId ? null : current;
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

export function updateJobStatusMeta(job: UpdateJobSummary): { label: string; color: string } {
  if (job.status === "failed" && job.processed > job.errorCount + (job.heldCount ?? 0)) {
    return { label: "一部失敗", color: "orange" };
  }
  if (job.status === "completed" && (job.heldCount ?? 0) > 0) {
    return { label: "完了（保留あり）", color: "yellow" };
  }
  return UPDATE_JOB_STATUS_META[job.status];
}

// Job summaries are application-level activity, not page-local state. Keeping
// one store prevents the update centre, operation history and sidebar badge
// from loading and interpreting separate copies of the same backend jobs.
const EMPTY_UPDATE_JOB_SUMMARIES: UpdateJobSummary[] = [];
let updateJobSummaries: UpdateJobSummary[] = [];
let updateJobSummaryRevision = 0;
let updateJobSummaryFeedUsers = 0;
let updateJobSummaryRefresh: Promise<UpdateJobSummary[]> | null = null;
const updateJobSummaryEventRevisions = new Map<string, number>();
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
  updateJobSummaryEventRevisions.set(summary.jobId, updateJobSummaryRevision);
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
          // Only events delivered after this read began can add a row that the
          // response did not see. Older local rows may have been deleted.
          if ((updateJobSummaryEventRevisions.get(job.jobId) ?? 0) <= revisionAtStart)
            return;
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
      const visibleIds = new Set(updateJobSummaries.map((job) => job.jobId));
      for (const jobId of updateJobSummaryEventRevisions.keys()) {
        if (!visibleIds.has(jobId)) updateJobSummaryEventRevisions.delete(jobId);
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
  const [syncingCandidatesFor, setSyncingCandidatesFor] = useState<string | null>(null);
  const candidateSyncAttempt = useRef<string | null>(null);
  const candidateSyncRetryTimer = useRef<number | null>(null);
  const [candidateSyncRetry, setCandidateSyncRetry] = useState(0);
  const activeJobId = activeSnapshot?.jobId ?? null;
  const activeJobIdRef = useRef(activeJobId);
  const candidateSyncMounted = useRef(false);
  const selectionRequestId = useRef(0);
  const manuallySelectedJobId = useRef<string | null>(null);

  useEffect(() => {
    candidateSyncMounted.current = true;
    return () => {
      candidateSyncMounted.current = false;
      selectionRequestId.current += 1;
      if (candidateSyncRetryTimer.current !== null)
        window.clearTimeout(candidateSyncRetryTimer.current);
    };
  }, []);

  useEffect(() => {
    activeJobIdRef.current = activeJobId;
    candidateSyncAttempt.current = null;
    if (candidateSyncRetryTimer.current !== null) {
      window.clearTimeout(candidateSyncRetryTimer.current);
      candidateSyncRetryTimer.current = null;
    }
  }, [activeJobId]);

  const loadJobs = useCallback(async (force = true) => {
    const requestId = ++selectionRequestId.current;
    if (!enabled) {
      manuallySelectedJobId.current = null;
      setActiveSnapshot(null);
      return;
    }
    const nextJobs = await refreshUpdateJobSummaries(force);
    if (!candidateSyncMounted.current || selectionRequestId.current !== requestId)
      return;
    const preferred = preferredVisibleUpdateJob(nextJobs);
    if (preferred) {
      const snapshot = await getUpdateJobCommand(preferred.jobId);
      if (!candidateSyncMounted.current || selectionRequestId.current !== requestId)
        return;
      manuallySelectedJobId.current = null;
      setActiveSnapshot((current) => mergeSnapshot(current, snapshot));
      onSnapshot?.(snapshot);
    } else {
      manuallySelectedJobId.current = null;
      setActiveSnapshot(null);
    }
  }, [enabled, onSnapshot]);

  const selectJob = useCallback(
    async (jobId: string) => {
      if (!enabled) return;
      const requestId = ++selectionRequestId.current;
      const snapshot = await getUpdateJobCommand(jobId);
      if (!candidateSyncMounted.current || selectionRequestId.current !== requestId)
        return snapshot;
      manuallySelectedJobId.current = jobId;
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
        const selectedJobId = manuallySelectedJobId.current;
        if (!selectedJobId || selectedJobId === event.payload.jobId) {
          if (event.payload.status === "canceled" && selectedJobId)
            manuallySelectedJobId.current = null;
          setActiveSnapshot((current) =>
            mergeVisibleUpdateJobSnapshot(current, event.payload),
          );
        }
        onSnapshot?.(event.payload);
        if (isUpdateJobTerminal(event.payload.status))
          invalidateAfterUpdateJob(queryClient);
      },
    );
    const disposeDelta = subscribeTauriEvent<UpdateJobProgressDelta>(
      "update-job-progress-delta",
      (event) => {
        lastEventAt.current = Date.now();
        if (event.payload.summary.status === "canceled" &&
            manuallySelectedJobId.current === event.payload.summary.jobId)
          manuallySelectedJobId.current = null;
        setActiveSnapshot((current) => {
          if (!current || current.jobId !== event.payload.summary.jobId)
            return current;
          const next = mergeProgressDelta(current, event.payload);
          onSnapshot?.(next);
          return next.status === "canceled" ? null : next;
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

  // Progress deltas deliberately contain no candidate page. Read only the new
  // local DB rows when a target discovers works, including rows beyond page 1.
  // This makes discovery visible while the remote check is still running.
  useEffect(() => {
    if (!enabled || !activeSnapshot || syncingCandidatesFor === activeSnapshot.jobId ||
        activeSnapshot.candidateCount <= activeSnapshot.candidates.length) return;
    const jobId = activeSnapshot.jobId;
    let cursor = Math.max(0, ...activeSnapshot.candidates.map((item) => item.id));
    const attempt = `${jobId}:${activeSnapshot.candidateCount}:${cursor}`;
    // A snapshot can report rows before a concurrent DB read sees them. Do not
    // spin through IPC on the same count/cursor when that read is still empty.
    if (candidateSyncAttempt.current === attempt) return;
    candidateSyncAttempt.current = attempt;
    if (candidateSyncRetryTimer.current !== null)
      window.clearTimeout(candidateSyncRetryTimer.current);
    setSyncingCandidatesFor(jobId);
    void (async () => {
      try {
        while (candidateSyncMounted.current && activeJobIdRef.current === jobId) {
          const page = await getUpdateJobCommand(jobId, cursor, null);
          if (!candidateSyncMounted.current || activeJobIdRef.current !== jobId) break;
          if (!page.candidates.length) break;
          const nextCursor = page.candidates[page.candidates.length - 1].id;
          if (nextCursor <= cursor) break;
          cursor = nextCursor;
          setActiveSnapshot((current) => current?.jobId === jobId ? mergeSnapshot(current, page) : current);
          if (!page.nextCandidateCursor) break;
        }
      } catch (error) {
        console.warn(`更新ジョブ ${jobId} の新しい候補を読み込めませんでした`, error);
      } finally {
        if (candidateSyncMounted.current)
          setSyncingCandidatesFor((current) => current === jobId ? null : current);
        if (candidateSyncMounted.current && activeJobIdRef.current === jobId) {
          candidateSyncRetryTimer.current = window.setTimeout(() => {
            candidateSyncAttempt.current = null;
            setCandidateSyncRetry((current) => current + 1);
            candidateSyncRetryTimer.current = null;
          }, 5000);
        }
      }
    })();
  }, [activeSnapshot, candidateSyncRetry, enabled, syncingCandidatesFor]);

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
          setActiveSnapshot((current) => current?.jobId === snapshot.jobId ? mergeSnapshot(current, snapshot) : current);
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
    if (!enabled || !activeSnapshot) return;
    const cursor = activeSnapshot.nextCandidateCursor ??
      (activeSnapshot.candidateCount > activeSnapshot.candidates.length
        ? Math.max(0, ...activeSnapshot.candidates.map((candidate) => candidate.id))
        : null);
    if (cursor === null) return;
    const snapshot = await getUpdateJobCommand(
      activeSnapshot.jobId,
      cursor,
      null,
    );
    setActiveSnapshot((current) => current?.jobId === snapshot.jobId ? mergeSnapshot(current, snapshot) : current);
  }, [activeSnapshot, enabled]);

  const loadOlderLogs = useCallback(async () => {
    if (!enabled || !activeSnapshot?.previousLogCursor) return;
    const snapshot = await getUpdateJobCommand(
      activeSnapshot.jobId,
      null,
      activeSnapshot.previousLogCursor,
    );
    setActiveSnapshot((current) => current?.jobId === snapshot.jobId ? mergeSnapshot(current, snapshot) : current);
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
