import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
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
  refreshUpdateJobSummaries,
  useUpdateJobSummaries,
  waitForUpdateJob,
} from "./updateJobs";

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
});
