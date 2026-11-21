import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";

const listUpdateTargets = vi.fn();
const listWatchGroups = vi.fn();
const listUpdateJobsCommand = vi.fn();
const startUpdateJobCommand = vi.fn();
const libraryV2Status = vi.fn();
const libraryV2Search = vi.fn();
const storeValues = new Map<string, unknown>();

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: vi.fn().mockResolvedValue(false),
  requestPermission: vi.fn().mockResolvedValue("denied"),
  sendNotification: vi.fn(),
}));
vi.mock("@/services/dbApi", () => ({
  isTauriRuntime: () => true,
  listUpdateTargets: (...args: unknown[]) => listUpdateTargets(...args),
  listWatchGroups: () => listWatchGroups(),
}));
vi.mock("@/services/updateJobApi", () => ({
  listUpdateJobsCommand: () => listUpdateJobsCommand(),
  startUpdateJobCommand: (...args: unknown[]) => startUpdateJobCommand(...args),
}));
vi.mock("@/services/libraryV2Api", () => ({
  libraryV2Status: () => libraryV2Status(),
  libraryV2Search: (...args: unknown[]) => libraryV2Search(...args),
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
  listWatchGroups.mockReset().mockResolvedValue([]);
  listUpdateJobsCommand.mockReset().mockResolvedValue([]);
  startUpdateJobCommand.mockReset().mockResolvedValue({ totals: 1 });
  libraryV2Status.mockReset().mockResolvedValue({ state: "ready", path: "x" });
  libraryV2Search.mockReset().mockResolvedValue({ cards: [] });
});

describe("automatic update checks", () => {
  // 監視対象が空でも、作品の監視は棚の作品の旗として残っている。
  it("checks the shelf when only a saved work is watched", async () => {
    libraryV2Search.mockResolvedValue({ cards: [{ id: "019ad000-0000-7000-8000-000000000001" }] });
    await runStartupTick();
    expect(startUpdateJobCommand).toHaveBeenCalledWith(expect.objectContaining({ scope: "all" }));
  });

  it("adopts the migrated group UUID and sends only its enabled targets", async () => {
    const id = "019ad000-0000-7000-8000-000000000001";
    const member = "019ad000-0000-7000-8000-000000000002";
    storeValues.set("update_schedule", { onStartup: true, groupId: 7 });
    listWatchGroups.mockResolvedValue([{ id, legacyId: 7, name: "毎日", targetIds: [member] }]);
    listUpdateTargets.mockResolvedValue([{ id: member }]);
    await runStartupTick();
    expect(startUpdateJobCommand).toHaveBeenCalledWith(expect.objectContaining({ scope: "targets", targetIds: [member] }));
    expect(storeValues.get("update_schedule")).toMatchObject({ groupId: id });
  });

  it("waits for the authoritative shelf to open before scheduling a job", async () => {
    libraryV2Status.mockResolvedValue({ state: "indexing", path: "x" });
    listUpdateTargets.mockResolvedValue([{ id: 1 }]);
    await runStartupTick();
    expect(startUpdateJobCommand).not.toHaveBeenCalled();
    expect(storeValues.has("update_schedule_last_run")).toBe(false);
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

  // 範囲をグループにしたら、その中の止めていない対象だけを見る。
  it("checks only the enabled members of the chosen group", async () => {
    storeValues.set("update_schedule", { onStartup: true, intervalHours: 0, mode: "check_only", groupId: 7 });
    listWatchGroups.mockResolvedValue([{ id: 7, name: "毎日", targetIds: [1, 2, 3] }]);
    listUpdateTargets.mockResolvedValue([{ id: 1 }, { id: 3 }, { id: 9 }]);
    await runStartupTick();
    expect(startUpdateJobCommand).toHaveBeenCalledWith(expect.objectContaining({ scope: "targets", targetIds: [1, 3] }));
  });

  // 消えたグループを「すべて」へ広げると、絞った意味が無くなる。
  it("does not widen to everything when the chosen group is gone", async () => {
    storeValues.set("update_schedule", { onStartup: true, intervalHours: 0, mode: "check_only", groupId: 7 });
    listUpdateTargets.mockResolvedValue([{ id: 1 }]);
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    await runStartupTick();
    error.mockRestore();
    expect(startUpdateJobCommand).not.toHaveBeenCalled();
  });

  it.each(["queued", "running", "canceling"])("leaves a %s job to its worker", async (status) => {
    listUpdateTargets.mockResolvedValue([{ id: 1 }]);
    listUpdateJobsCommand.mockResolvedValue([{ jobId: "job-1", status }]);
    await runStartupTick();
    expect(startUpdateJobCommand).not.toHaveBeenCalled();
  });
});
