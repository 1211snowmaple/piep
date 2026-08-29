import { useSyncExternalStore } from "react";

export type OperationKind =
  "save" | "update" | "epub" | "backup" | "restore" | "search" | "maintenance";
export type OperationStatus =
  | "queued"
  | "running"
  | "canceling"
  | "canceled"
  | "completed"
  | "failed"
  | "interrupted";
export type OperationLogLevel = "info" | "success" | "warn" | "error";

export interface OperationLog {
  id: string;
  level: OperationLogLevel;
  message: string;
  createdAt: string;
}

export interface OperationJob {
  id: string;
  kind: OperationKind;
  label: string;
  detail: string | null;
  status: OperationStatus;
  current: number;
  total: number | null;
  startedAt: string;
  updatedAt: string;
  finishedAt: string | null;
  canCancel: boolean;
  canRetry: boolean;
  logs: OperationLog[];
  /** Corresponding persistent/backend job, when this UI operation mirrors one. */
  externalJobId?: string | null;
}

export interface StartOperationOptions {
  kind: OperationKind;
  label: string;
  detail?: string | null;
  total?: number | null;
  onCancel?: () => void | Promise<void>;
  onRetry?: () => void | Promise<void>;
}

export interface OperationController {
  id: string;
  progress: (current: number, total?: number | null, message?: string) => void;
  log: (message: string, level?: OperationLogLevel) => void;
  complete: (message?: string) => void;
  fail: (error: unknown) => void;
  cancel: (message?: string) => void;
  isCancelRequested: () => boolean;
  linkExternalJob: (jobId: string) => void;
}

const STORAGE_KEY = "piep.operation-history.v1";
const MAX_JOBS = 100;
// 画面中は全件を保持するが、localStorage は容量が有限なので、再起動後に
// 復元する履歴だけはDBの更新ジョブと同じ2000件までに抑える。
const MAX_PERSISTED_LOGS = 2_000;
export const OPERATION_HISTORY_PERSIST_DELAY_MS = 250;
const listeners = new Set<() => void>();
const cancelHandlers = new Map<string, () => void | Promise<void>>();
const retryHandlers = new Map<string, () => void | Promise<void>>();
const cancelRequests = new Set<string>();

function pruneDetachedHandlers() {
  const retained = new Set(jobs.map((job) => job.id));
  for (const jobId of cancelHandlers.keys())
    if (!retained.has(jobId)) cancelHandlers.delete(jobId);
  for (const jobId of retryHandlers.keys())
    if (!retained.has(jobId)) retryHandlers.delete(jobId);
  for (const jobId of cancelRequests)
    if (!retained.has(jobId)) cancelRequests.delete(jobId);
}

function now() {
  return new Date().toISOString();
}
function id() {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `op-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

function loadJobs(): OperationJob[] {
  try {
    const raw = storage()?.getItem(STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    if (!Array.isArray(parsed)) return [];
    const loaded = parsed.filter((job): job is OperationJob =>
      Boolean(
        job && typeof job.id === "string" && typeof job.label === "string",
      ),
    );
    const loadedAt = now();
    return loaded.slice(0, MAX_JOBS).map((job) => {
      if (
        job.status !== "queued" &&
        job.status !== "running" &&
        job.status !== "canceling"
      ) {
        return { ...job, canCancel: false, canRetry: false };
      }
      return {
        ...job,
        status: "interrupted" as const,
        canCancel: false,
        canRetry: false,
        finishedAt: loadedAt,
        updatedAt: loadedAt,
        // 何が起きたかだけでなく、**次に何をすればいいか**まで書く。
        // ここに残るのは、走っている最中に画面かアプリが消えたジョブである。
        // 保存も取り込みも、済んだものは相手先IDで見分けて飛ばすので、
        // 同じ操作をもう一度始めれば続きから進む。それを知らないと、
        // 800件のうち500件まで進んだ人が最初からやり直すことになる。
        logs: [
          ...(job.logs ?? []),
          {
            id: id(),
            level: "warn" as const,
            message:
              "処理の途中で画面が閉じられたため、状態を引き継げませんでした。同じ操作をやり直すと、済んでいるものは飛ばして続きから進みます",
            createdAt: loadedAt,
          },
        ],
      };
    });
  } catch {
    return [];
  }
}

let jobs = loadJobs();
let persistTimer: ReturnType<typeof setTimeout> | null = null;

function writePersistedJobs() {
  try {
    const persisted = jobs.slice(0, MAX_JOBS).map((job) => ({
      ...job,
      logs: job.logs.slice(-MAX_PERSISTED_LOGS),
    }));
    storage()?.setItem(STORAGE_KEY, JSON.stringify(persisted));
  } catch {
    /* History is a convenience; an unavailable/quota-full store must not break work. */
  }
}

function persistNow() {
  if (persistTimer !== null) {
    clearTimeout(persistTimer);
    persistTimer = null;
  }
  writePersistedJobs();
}

function schedulePersist() {
  if (persistTimer !== null) return;
  persistTimer = setTimeout(() => {
    persistTimer = null;
    writePersistedJobs();
  }, OPERATION_HISTORY_PERSIST_DELAY_MS);
}

function emit(persistence: "debounced" | "immediate" = "debounced") {
  if (persistence === "immediate") persistNow();
  else schedulePersist();
  listeners.forEach((listener) => listener());
}

function updateJob(
  jobId: string,
  updater: (job: OperationJob) => OperationJob,
  persistence: "debounced" | "immediate" = "debounced",
) {
  jobs = jobs
    .map((job) => (job.id === jobId ? updater(job) : job))
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
    .slice(0, MAX_JOBS);
  pruneDetachedHandlers();
  emit(persistence);
}

function appendLog(
  job: OperationJob,
  message: string,
  level: OperationLogLevel,
): OperationJob {
  if (!message.trim()) return job;
  return {
    ...job,
    logs: [
      ...job.logs,
      { id: id(), level, message: message.trim(), createdAt: now() },
    ],
  };
}

export function startOperation(
  options: StartOperationOptions,
): OperationController {
  const createdAt = now();
  const jobId = id();
  const job: OperationJob = {
    id: jobId,
    kind: options.kind,
    label: options.label,
    detail: options.detail ?? null,
    status: "running",
    current: 0,
    total: options.total ?? null,
    startedAt: createdAt,
    updatedAt: createdAt,
    finishedAt: null,
    canCancel: Boolean(options.onCancel),
    canRetry: Boolean(options.onRetry),
    logs: [
      { id: id(), level: "info", message: "処理を開始しました", createdAt },
    ],
  };
  jobs = [job, ...jobs].slice(0, MAX_JOBS);
  pruneDetachedHandlers();
  if (options.onCancel) cancelHandlers.set(jobId, options.onCancel);
  if (options.onRetry) retryHandlers.set(jobId, options.onRetry);
  // Persist the job before background progress starts. High-frequency updates
  // below are batched, while terminal states are flushed synchronously.
  emit("immediate");

  const finish = (
    status: "completed" | "failed" | "canceled",
    message: string,
    level: OperationLogLevel,
  ) => {
    const at = now();
    updateJob(
      jobId,
      (current) => ({
        ...appendLog(current, message, level),
        status,
        current:
          status === "completed" && current.total !== null
            ? current.total
            : current.current,
        updatedAt: at,
        finishedAt: at,
        canCancel: false,
        canRetry: status === "failed" && retryHandlers.has(jobId),
      }),
      "immediate",
    );
    cancelHandlers.delete(jobId);
    cancelRequests.delete(jobId);
    if (status !== "failed") retryHandlers.delete(jobId);
  };

  return {
    id: jobId,
    progress: (current, total, message) =>
      updateJob(jobId, (job) => ({
        ...appendLog(job, message ?? "", "info"),
        current: Math.max(0, Math.trunc(current)),
        total:
          total === undefined
            ? job.total
            : total === null
              ? null
              : Math.max(0, Math.trunc(total)),
        updatedAt: now(),
      })),
    log: (message, level = "info") =>
      updateJob(jobId, (job) => ({
        ...appendLog(job, message, level),
        updatedAt: now(),
      })),
    complete: (message = "処理が完了しました") =>
      finish("completed", message, "success"),
    fail: (error) =>
      finish(
        "failed",
        error instanceof Error ? error.message : String(error),
        "error",
      ),
    cancel: (message = "処理をキャンセルしました") =>
      finish("canceled", message, "warn"),
    isCancelRequested: () => cancelRequests.has(jobId),
    linkExternalJob: (externalJobId) =>
      updateJob(
        jobId,
        (job) => ({ ...job, externalJobId, updatedAt: now() }),
        "immediate",
      ),
  };
}

export async function requestOperationCancel(jobId: string): Promise<void> {
  const handler = cancelHandlers.get(jobId);
  if (!handler) return;
  cancelRequests.add(jobId);
  updateJob(
    jobId,
    (job) => ({
      ...appendLog(job, "キャンセルを要求しました", "warn"),
      status: "canceling",
      canCancel: false,
      updatedAt: now(),
    }),
    "immediate",
  );
  try {
    await handler();
  } catch (error) {
    cancelRequests.delete(jobId);
    updateJob(
      jobId,
      (job) => ({
        ...appendLog(
          job,
          error instanceof Error ? error.message : String(error),
          "error",
        ),
        status: "running",
        canCancel: true,
        updatedAt: now(),
      }),
      "immediate",
    );
  }
}

export async function retryOperation(jobId: string): Promise<void> {
  const handler = retryHandlers.get(jobId);
  if (!handler) return;
  // A retry is a new execution with its own duration and logs. Keep the failed
  // attempt immutable so diagnostics do not lose the original failure.
  updateJob(
    jobId,
    (job) => ({
      ...appendLog(job, "新しい操作として再試行しました", "info"),
      canRetry: false,
      updatedAt: now(),
    }),
    "immediate",
  );
  retryHandlers.delete(jobId);
  await handler();
}

export function clearCompletedOperations(): void {
  const active = new Set<OperationStatus>(["queued", "running", "canceling"]);
  for (const job of jobs) {
    if (!active.has(job.status)) {
      cancelHandlers.delete(job.id);
      cancelRequests.delete(job.id);
      retryHandlers.delete(job.id);
    }
  }
  jobs = jobs.filter((job) => active.has(job.status));
  emit("immediate");
}

export function getOperationJobs(): OperationJob[] {
  return jobs;
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
export function useOperationJobs(): OperationJob[] {
  return useSyncExternalStore(subscribe, getOperationJobs, getOperationJobs);
}
