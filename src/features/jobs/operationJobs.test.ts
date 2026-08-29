import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

describe("operation history", () => {
  beforeEach(() => {
    window.localStorage.clear();
    vi.resetModules();
  });

  afterEach(() => vi.useRealTimers());

  it("records progress, completion and logs", async () => {
    const api = await import("./operationJobs");
    const operation = api.startOperation({
      kind: "epub",
      label: "3冊を書き出す",
      total: 3,
    });
    operation.progress(2, 3, "2冊目を生成中");
    operation.complete();
    const job = api.getOperationJobs()[0];
    expect(job).toMatchObject({
      kind: "epub",
      status: "completed",
      current: 3,
      total: 3,
    });
    expect(job.logs.map((log) => log.message)).toContain("2冊目を生成中");
  });

  it("routes cancellation to the active operation", async () => {
    const api = await import("./operationJobs");
    const cancel = vi.fn();
    const operation = api.startOperation({
      kind: "save",
      label: "保存",
      onCancel: cancel,
    });
    await api.requestOperationCancel(operation.id);
    expect(cancel).toHaveBeenCalledOnce();
    expect(operation.isCancelRequested()).toBe(true);
    operation.cancel();
    expect(api.getOperationJobs()[0].status).toBe("canceled");
  });

  it("keeps every progress log instead of silently dropping entries at 100", async () => {
    const api = await import("./operationJobs");
    const operation = api.startOperation({
      kind: "save",
      label: "164件を保存",
      total: 164,
    });
    for (let current = 1; current <= 164; current += 1)
      operation.progress(current, 164, `${current}件目`);
    expect(api.getOperationJobs()[0].logs).toHaveLength(165);
    expect(api.getOperationJobs()[0].logs[1].message).toBe("1件目");
  });

  it("clears a rejected cancel request so the operation can continue", async () => {
    const api = await import("./operationJobs");
    const operation = api.startOperation({
      kind: "save",
      label: "保存",
      onCancel: vi.fn().mockRejectedValue(new Error("worker refused")),
    });
    await api.requestOperationCancel(operation.id);
    expect(operation.isCancelRequested()).toBe(false);
    expect(api.getOperationJobs()[0]).toMatchObject({
      status: "running",
      canCancel: true,
    });
  });

  it("keeps failed jobs retryable during the session", async () => {
    const api = await import("./operationJobs");
    const retry = vi.fn();
    const operation = api.startOperation({
      kind: "backup",
      label: "バックアップ",
      onRetry: retry,
    });
    operation.fail(new Error("disk full"));
    await api.retryOperation(operation.id);
    expect(retry).toHaveBeenCalledOnce();
    await api.retryOperation(operation.id);
    expect(retry).toHaveBeenCalledOnce();
  });

  it("batches high-frequency progress persistence while keeping in-memory state immediate", async () => {
    vi.useFakeTimers();
    const writes = vi.spyOn(window.localStorage, "setItem");
    const api = await import("./operationJobs");
    const operation = api.startOperation({
      kind: "epub",
      label: "大量書き出し",
      total: 1_000,
    });
    expect(writes).toHaveBeenCalledTimes(1);

    for (let current = 1; current <= 100; current += 1)
      operation.progress(current, 1_000);
    expect(api.getOperationJobs()[0].current).toBe(100);
    expect(writes).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(api.OPERATION_HISTORY_PERSIST_DELAY_MS);
    expect(writes).toHaveBeenCalledTimes(2);
    writes.mockRestore();
  });

  it("flushes a terminal state and cancels a pending progress write", async () => {
    vi.useFakeTimers();
    const writes = vi.spyOn(window.localStorage, "setItem");
    const api = await import("./operationJobs");
    const operation = api.startOperation({
      kind: "save",
      label: "保存",
      total: 10,
    });
    operation.progress(5);
    operation.complete();

    expect(writes).toHaveBeenCalledTimes(2);
    await vi.runAllTimersAsync();
    expect(writes).toHaveBeenCalledTimes(2);
    expect(
      JSON.parse(
        window.localStorage.getItem("piep.operation-history.v1") ?? "[]",
      )[0],
    ).toMatchObject({ status: "completed", current: 10 });
    writes.mockRestore();
  });
});
