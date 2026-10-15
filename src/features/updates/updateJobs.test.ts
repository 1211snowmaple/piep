import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  UpdateJobProgressDelta,
  UpdateJobSnapshot,
} from "@/services/updateJobApi";

const mocks = vi.hoisted(() => ({
  getJob: vi.fn(),
  listJobs: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@/services/eventBus", () => ({
  subscribeTauriEvent: (
    event: string,
    handler: (event: { payload: unknown }) => void,
  ) => {
    mocks.listeners.set(event, handler);
    return () => mocks.listeners.delete(event);
  },
}));

vi.mock("@/services/updateJobApi", () => ({
  cancelUpdateJobCommand: vi.fn(),
  clearUpdateJobCommand: vi.fn(),
  getUpdateJobCommand: mocks.getJob,
  getUpdateJobCredentials: vi.fn(),
  listUpdateJobsCommand: mocks.listJobs,
  pauseUpdateJobCommand: vi.fn(),
  resumeUpdateJobCommand: vi.fn(),
  saveUpdateJobCandidatesCommand: vi.fn(),
  startUpdateJobCommand: vi.fn(),
}));

import {
  MAX_LIVE_UPDATE_LOGS,
  mergeVisibleUpdateJobSnapshot,
  preferredVisibleUpdateJob,
  refreshUpdateJobSummaries,
  useUpdateJobSummaries,
  useUpdateJobs,
  waitForUpdateJob,
  updateJobStatusMeta,
} from "./updateJobs";

it("distinguishes total failure, partial failure and deferred permissions", () => {
  expect(updateJobStatusMeta({ ...initial, status: "failed", processed: 2, errorCount: 1 }).label).toBe("一部失敗");
  expect(updateJobStatusMeta({ ...initial, status: "failed", processed: 2, errorCount: 2 }).label).toBe("失敗");
  expect(updateJobStatusMeta({ ...initial, status: "completed", heldCount: 1 }).label).toBe("完了（保留あり）");
  expect(updateJobStatusMeta({ ...initial, status: "canceled", heldCount: 1 }).label).toBe("中止");
});

it("候補件数だけ先に届いても、空の DB 読み取りを連打しない", async () => {
  const waiting = { ...initial, candidateCount: 2 };
  mocks.getJob.mockReset().mockResolvedValue(waiting);
  mocks.listJobs.mockReset().mockResolvedValue([waiting]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
  const view = renderHook(() => useUpdateJobs(), { wrapper });

  await vi.waitFor(() => expect(mocks.getJob).toHaveBeenCalledWith("save-1", 0, null));
  const count = mocks.getJob.mock.calls.length;
  await new Promise((resolve) => setTimeout(resolve, 100));
  expect(mocks.getJob).toHaveBeenCalledTimes(count);
  view.unmount();
  client.clear();
});

it("別のジョブへ切り替えた後は、古い候補ページの続きを読まない", async () => {
  const firstCandidate = {
    id: 1, key: "pixiv:1", source: "pixiv" as const, sourceId: "1",
    title: "作品 1", subtitle: "", targetLabel: "作者", targetType: "author" as const,
    selected: true, status: "candidate" as const, kind: "new" as const,
  };
  const firstJob = {
    ...initial, candidateCount: 3, candidates: [firstCandidate], nextCandidateCursor: 1,
  };
  const secondJob = { ...initial, jobId: "save-2" };
  let resolvePage!: (page: UpdateJobSnapshot) => void;
  mocks.getJob.mockReset().mockImplementation((jobId: string, cursor?: number) => {
    if (jobId === "save-2") return Promise.resolve(secondJob);
    if (cursor === 1) return new Promise<UpdateJobSnapshot>((resolve) => { resolvePage = resolve; });
    return Promise.resolve(firstJob);
  });
  mocks.listJobs.mockReset().mockResolvedValue([firstJob]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
  const view = renderHook(() => useUpdateJobs(), { wrapper });

  await vi.waitFor(() => expect(mocks.getJob).toHaveBeenCalledWith("save-1", 1, null));
  await act(async () => { await view.result.current.selectJob("save-2"); });
  await act(async () => {
    resolvePage({ ...firstJob, candidates: [{ ...firstCandidate, id: 2, sourceId: "2" }], nextCandidateCursor: 2 });
  });

  expect(view.result.current.activeSnapshot?.jobId).toBe("save-2");
  expect(mocks.getJob).not.toHaveBeenCalledWith("save-1", 2, null);
  view.unmount();
  client.clear();
});

it("先に選んだジョブの遅い応答が、後の選択を上書きしない", async () => {
  const secondJob = { ...initial, jobId: "save-2" };
  const thirdJob = { ...initial, jobId: "save-3" };
  let resolveSecond!: (snapshot: UpdateJobSnapshot) => void;
  mocks.getJob.mockReset().mockImplementation((jobId: string) => {
    if (jobId === "save-2")
      return new Promise<UpdateJobSnapshot>((resolve) => { resolveSecond = resolve; });
    return Promise.resolve(jobId === "save-3" ? thirdJob : initial);
  });
  mocks.listJobs.mockReset().mockResolvedValue([initial]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
  const view = renderHook(() => useUpdateJobs(), { wrapper });
  await vi.waitFor(() => expect(view.result.current.activeSnapshot?.jobId).toBe("save-1"));

  const staleSelection = view.result.current.selectJob("save-2");
  await act(async () => { await view.result.current.selectJob("save-3"); });
  await act(async () => { resolveSecond(secondJob); await staleSelection; });

  expect(view.result.current.activeSnapshot?.jobId).toBe("save-3");
  view.unmount();
  client.clear();
});

it("一覧の遅い再読込が、後から選んだジョブを上書きしない", async () => {
  const thirdJob = { ...initial, jobId: "save-3" };
  let resolveList!: (jobs: UpdateJobSnapshot[]) => void;
  mocks.getJob.mockReset().mockImplementation((jobId: string) =>
    Promise.resolve(jobId === "save-3" ? thirdJob : initial),
  );
  mocks.listJobs.mockReset().mockResolvedValueOnce([initial]).mockImplementationOnce(
    () => new Promise<UpdateJobSnapshot[]>((resolve) => { resolveList = resolve; }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
  const view = renderHook(() => useUpdateJobs(), { wrapper });
  await vi.waitFor(() => expect(view.result.current.activeSnapshot?.jobId).toBe("save-1"));

  const staleReload = view.result.current.loadJobs();
  await act(async () => { await view.result.current.selectJob("save-3"); });
  await act(async () => { resolveList([initial]); await staleReload; });

  expect(view.result.current.activeSnapshot?.jobId).toBe("save-3");
  view.unmount();
  client.clear();
});

it("手動で選んだジョブを、別ジョブの進捗イベントが切り替えない", async () => {
  const secondJob = { ...initial, jobId: "save-2" };
  const thirdJob = { ...initial, jobId: "save-3" };
  mocks.getJob.mockReset().mockImplementation((jobId: string) =>
    Promise.resolve(jobId === "save-2" ? secondJob : initial),
  );
  mocks.listJobs.mockReset().mockResolvedValue([initial]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
  const view = renderHook(() => useUpdateJobs(), { wrapper });
  await vi.waitFor(() => expect(view.result.current.activeSnapshot?.jobId).toBe("save-1"));
  await act(async () => { await view.result.current.selectJob("save-2"); });

  act(() => { mocks.listeners.get("update-job-progress")?.({ payload: thirdJob }); });

  expect(view.result.current.activeSnapshot?.jobId).toBe("save-2");
  act(() => {
    mocks.listeners.get("update-job-progress")?.({
      payload: { ...secondJob, processed: 1 },
    });
  });
  expect(view.result.current.activeSnapshot?.processed).toBe(1);
  view.unmount();
  client.clear();
});

const initial: UpdateJobSnapshot = {
  jobId: "save-1",
  status: "running",
  scope: "save",
  mode: "save",
  totals: 2,
  processed: 0,
  candidateCount: 0,
  savedCount: 0,
  errorCount: 0,
  activeLabel: "準備中",
  startedAt: "2026-08-29T00:00:00Z",
  updatedAt: "2026-08-29T00:00:00Z",
  finishedAt: null,
  logs: [],
  candidates: [],
  nextCandidateCursor: null,
  previousLogCursor: null,
};

describe("visible update job", () => {
  it("moves a canceled job out of the progress area but keeps it selectable from history", () => {
    const canceled = { ...initial, status: "canceled" as const };
    const completed = {
      ...initial,
      jobId: "older-completed",
      status: "completed" as const,
    };

    expect(preferredVisibleUpdateJob([canceled, completed])).toBe(completed);
    expect(preferredVisibleUpdateJob([canceled])).toBeUndefined();
    expect(mergeVisibleUpdateJobSnapshot(initial, canceled)).toBeNull();
  });

  it("does not let another job's cancellation replace the job being viewed", () => {
    const canceledElsewhere = {
      ...initial,
      jobId: "canceled-elsewhere",
      status: "canceled" as const,
    };

    expect(mergeVisibleUpdateJobSnapshot(initial, canceledElsewhere)).toBe(initial);
  });

  it("keeps a newer progress count while appending a late candidate page", () => {
    const candidate = (id: number) => ({
      id, key: `pixiv:${id}`, source: "pixiv" as const, sourceId: String(id),
      title: `作品 ${id}`, subtitle: "", targetLabel: "作者", targetType: "author" as const,
      selected: true, status: "candidate" as const, kind: "new" as const,
    });
    const current = {
      ...initial, candidateCount: 3, candidates: [{ ...candidate(1), status: "saved" as const }],
      nextCandidateCursor: 1, updatedAt: "2026-08-29T00:00:03Z",
    };
    const olderPage = {
      ...current, candidateCount: 2, candidates: [candidate(1), candidate(2), candidate(3)],
      nextCandidateCursor: null, updatedAt: "2026-08-29T00:00:02Z",
    };
    const merged = mergeVisibleUpdateJobSnapshot(current, olderPage);
    expect(merged?.candidateCount).toBe(3);
    expect(merged?.candidates.map((item) => item.id)).toEqual([1, 2, 3]);
    expect(merged?.candidates[0].status).toBe("saved");
    expect(merged?.nextCandidateCursor).toBeNull();
  });
});

describe("waitForUpdateJob", () => {
  beforeEach(() => {
    mocks.listeners.clear();
    mocks.getJob.mockReset().mockResolvedValue(initial);
    mocks.listJobs.mockReset().mockResolvedValue([]);
  });

  it("merges a small terminal delta and forwards only the changed row", async () => {
    const onSnapshot = vi.fn();
    const onItemState = vi.fn();
    const waiting = waitForUpdateJob("save-1", onSnapshot, onItemState);
    await vi.waitFor(() =>
      expect(mocks.listeners.has("update-job-progress-delta")).toBe(true),
    );

    const delta: UpdateJobProgressDelta = {
      summary: {
        ...initial,
        status: "completed",
        processed: 2,
        savedCount: 2,
        activeLabel: "完了しました",
        finishedAt: "2026-08-29T00:01:00Z",
      },
      changedItem: {
        source: "pixiv",
        sourceId: "22",
        status: "saved",
        error: null,
      },
      latestLog: {
        id: 9,
        logType: "success",
        message: "保存しました",
        createdAt: "2026-08-29T00:01:00Z",
      },
    };
    mocks.listeners.get("update-job-progress-delta")?.({ payload: delta });

    const result = await waiting;
    expect(result.status).toBe("completed");
    expect(result.logs).toEqual([delta.latestLog]);
    expect(onItemState).toHaveBeenCalledOnce();
    expect(onItemState).toHaveBeenCalledWith(delta.changedItem);
    expect(onSnapshot).toHaveBeenCalledTimes(2);
    expect(mocks.getJob).toHaveBeenCalledTimes(1);
  });

  it("bounds a long-running job's live log tail", async () => {
    const waiting = waitForUpdateJob("save-1", vi.fn());
    await vi.waitFor(() =>
      expect(mocks.listeners.has("update-job-progress-delta")).toBe(true),
    );

    for (let id = 1; id <= MAX_LIVE_UPDATE_LOGS + 20; id += 1) {
      const terminal = id === MAX_LIVE_UPDATE_LOGS + 20;
      mocks.listeners.get("update-job-progress-delta")?.({
        payload: {
          summary: {
            ...initial,
            status: terminal ? "completed" : "running",
            finishedAt: terminal ? "2026-08-29T00:01:00Z" : null,
          },
          changedItem: null,
          latestLog: {
            id,
            logType: "info",
            message: `log-${id}`,
            createdAt: "2026-08-29T00:01:00Z",
          },
        } satisfies UpdateJobProgressDelta,
      });
    }

    const result = await waiting;
    expect(result.logs).toHaveLength(MAX_LIVE_UPDATE_LOGS);
    expect(result.logs[0].id).toBe(21);
    expect(result.logs[result.logs.length - 1].id).toBe(MAX_LIVE_UPDATE_LOGS + 20);
  });

  it("shares backend summaries and applies live delta events", async () => {
    mocks.listJobs.mockResolvedValue([initial]);
    const view = renderHook(() => useUpdateJobSummaries(true));
    await vi.waitFor(() => expect(view.result.current).toHaveLength(1));

    const delta: UpdateJobProgressDelta = {
      summary: { ...initial, processed: 1, savedCount: 1 },
      changedItem: null,
      latestLog: null,
    };
    mocks.listeners.get("update-job-progress-delta")?.({ payload: delta });

    await vi.waitFor(() =>
      expect(view.result.current[0]).toMatchObject({
        jobId: "save-1",
        processed: 1,
        savedCount: 1,
      }),
    );
    view.unmount();
  });

  it("performs a fresh read after an in-flight read when explicitly reloading", async () => {
    let resolveFirst!: (jobs: UpdateJobSnapshot[]) => void;
    mocks.listJobs
      .mockImplementationOnce(
        () =>
          new Promise<UpdateJobSnapshot[]>((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockResolvedValueOnce([]);

    const first = refreshUpdateJobSummaries();
    const forced = refreshUpdateJobSummaries(true);
    expect(mocks.listJobs).toHaveBeenCalledTimes(1);
    resolveFirst([initial]);
    await first;
    await forced;

    expect(mocks.listJobs).toHaveBeenCalledTimes(2);
  });

  it("does not revive a deleted history row when another job emits during refresh", async () => {
    const removed = { ...initial, jobId: "removed", status: "completed" as const };
    const remaining = { ...initial, jobId: "remaining" };
    mocks.listJobs.mockReset().mockResolvedValue([removed, remaining]);
    await refreshUpdateJobSummaries(true);
    const view = renderHook(() => useUpdateJobSummaries(true));
    await vi.waitFor(() => expect(mocks.listeners.has("update-job-progress-delta")).toBe(true));
    await vi.waitFor(() => expect(view.result.current).toHaveLength(2));

    let resolveList!: (jobs: UpdateJobSnapshot[]) => void;
    mocks.listJobs.mockImplementationOnce(() => new Promise<UpdateJobSnapshot[]>((resolve) => {
      resolveList = resolve;
    }));
    const pending = refreshUpdateJobSummaries(true);
    await act(async () => {
      mocks.listeners.get("update-job-progress-delta")?.({ payload: {
        summary: { ...remaining, processed: 1, updatedAt: "2026-08-29T00:00:01Z" },
        changedItem: null,
        latestLog: null,
      } satisfies UpdateJobProgressDelta });
      resolveList([remaining]);
      await pending;
    });

    expect(view.result.current.map((job) => job.jobId)).toEqual(["remaining"]);
    expect(view.result.current[0].processed).toBe(1);
    view.unmount();
  });
});
