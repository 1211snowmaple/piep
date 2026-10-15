import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";

const listUpdateTargets = vi.fn();
const searchDownloadsV2 = vi.fn();
const listUpdateJobsCommand = vi.fn();
const startUpdateJobCommand = vi.fn();
const storeValues = new Map<string, unknown>();

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: vi.fn().mockResolvedValue(false),
  requestPermission: vi.fn().mockResolvedValue("denied"),
  sendNotification: vi.fn(),
}));
vi.mock("@/services/dbApi", () => ({
  isTauriRuntime: () => true,
  listUpdateTargets: (...args: unknown[]) => listUpdateTargets(...args),
  searchDownloadsV2: (...args: unknown[]) => searchDownloadsV2(...args),
}));
vi.mock("@/services/updateJobApi", () => ({
  listUpdateJobsCommand: () => listUpdateJobsCommand(),
  startUpdateJobCommand: (...args: unknown[]) => startUpdateJobCommand(...args),
}));
vi.mock("@/store", () => ({
  store: {
    get: async (key: string) => storeValues.get(key),
    set: async (key: string, value: unknown) => { storeValues.set(key, value); },
    save: async () => undefined,
  },
}));

const { useUpdateScheduler } = await import("@/features/updates/useUpdateScheduler");

/** 起動直後の1回が走るところまで進める。 */
async function runStartupTick() {
  vi.useFakeTimers();
  try {
    renderHook(() => useUpdateScheduler(true));
    await vi.advanceTimersByTimeAsync(8_000);
    // tick 内の await 連鎖が解けるまで回す。
    await vi.advanceTimersByTimeAsync(0);
  } finally {
    vi.useRealTimers();
  }
}

beforeEach(() => {
  storeValues.clear();
  storeValues.set("update_schedule", { onStartup: true, intervalHours: 0, mode: "check_only", watchSaved: false, notify: false });
  listUpdateTargets.mockReset().mockResolvedValue([]);
  searchDownloadsV2.mockReset().mockResolvedValue({ items: [] });
  listUpdateJobsCommand.mockReset().mockResolvedValue([]);
  startUpdateJobCommand.mockReset().mockResolvedValue({ totals: 1 });
});

describe("automatic update checks", () => {
  // 監視対象が空でも、作品の監視は downloads 側の旗として残っている。
  // update_targets だけを見ていた頃は、この人の自動確認が一度も走らなかった。
  it("runs when only individual works are watched", async () => {
    searchDownloadsV2.mockResolvedValue({ items: [{ id: 5 }] });
    await runStartupTick();
    expect(startUpdateJobCommand).toHaveBeenCalledTimes(1);
  });

  it("stays quiet when there is nothing to check at all", async () => {
    await runStartupTick();
    expect(startUpdateJobCommand).not.toHaveBeenCalled();
  });

  it("does not postpone the next run when creating the job fails", async () => {
    listUpdateTargets.mockResolvedValue([{ id: 1 }]);
    startUpdateJobCommand.mockRejectedValue(new Error("database busy"));
    await runStartupTick();
    expect(storeValues.has("update_schedule_last_run")).toBe(false);
  });

  // 止まったままのジョブに worker はいない。「終わっていない」を理由に譲ると、
  // 放置された1件が以後の自動確認を永久に塞ぐ。
  it.each(["paused", "auth_required"])("does not defer forever to a %s job", async (status) => {
    listUpdateTargets.mockResolvedValue([{ id: 1 }]);
    listUpdateJobsCommand.mockResolvedValue([{ jobId: "job-1", status }]);
    await runStartupTick();
    expect(startUpdateJobCommand).toHaveBeenCalledTimes(1);
  });

  it.each(["queued", "running", "canceling"])("leaves a %s job to its worker", async (status) => {
    listUpdateTargets.mockResolvedValue([{ id: 1 }]);
    listUpdateJobsCommand.mockResolvedValue([{ jobId: "job-1", status }]);
    await runStartupTick();
    expect(startUpdateJobCommand).not.toHaveBeenCalled();
  });
});
