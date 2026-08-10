import { beforeEach, describe, expect, it, vi } from "vitest";

describe("operation history", () => {
  beforeEach(() => {
    window.localStorage.clear();
    vi.resetModules();
  });

  it("records progress, completion and logs", async () => {
    const api = await import("./operationJobs");
    const operation = api.startOperation({ kind: "epub", label: "3冊を書き出す", total: 3 });
    operation.progress(2, 3, "2冊目を生成中");
    operation.complete();
    const job = api.getOperationJobs()[0];
    expect(job).toMatchObject({ kind: "epub", status: "completed", current: 3, total: 3 });
    expect(job.logs.map((log) => log.message)).toContain("2冊目を生成中");
  });

  it("routes cancellation to the active operation", async () => {
    const api = await import("./operationJobs");
    const cancel = vi.fn();
    const operation = api.startOperation({ kind: "save", label: "保存", onCancel: cancel });
    await api.requestOperationCancel(operation.id);
    expect(cancel).toHaveBeenCalledOnce();
    expect(operation.isCancelRequested()).toBe(true);
    operation.cancel();
    expect(api.getOperationJobs()[0].status).toBe("canceled");
  });

  it("keeps failed jobs retryable during the session", async () => {
    const api = await import("./operationJobs");
    const retry = vi.fn();
    const operation = api.startOperation({ kind: "backup", label: "バックアップ", onRetry: retry });
    operation.fail(new Error("disk full"));
    await api.retryOperation(operation.id);
    expect(retry).toHaveBeenCalledOnce();
  });
});
