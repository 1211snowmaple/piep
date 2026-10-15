import { beforeEach, describe, expect, it, vi } from "vitest";
import { trackManualRebuild, trackedRebuildJobId } from "@/features/search/searchIndexProgress";
import { __testEmitSearchIndexProgress } from "@/features/search/searchIndexProgress";
import type { SearchRebuildProgress } from "@/types/library";

/**
 * 作り直しの見張りは、画面から切り離しておく。
 *
 * **見張りが設定画面の中にあったころ、画面を離れると結び付けが切れた。** 覚えて
 * いた仕事の番号も操作履歴の控えも unmount で失われ、戻ってきても更新が再開
 * しない。履歴は最初のひと塊（64件）で固まったまま、ヘッダーだけが進み続けて、
 * 同じ作業が二つの数字で食い違って見えていた。
 */
function progress(overrides: Partial<SearchRebuildProgress> = {}): SearchRebuildProgress {
  return {
    jobId: "job-1",
    origin: "manual",
    status: "running",
    totalDownloads: 9370,
    indexedDownloads: 0,
    pendingDownloads: 9370,
    isComplete: false,
    phase: "indexing",
    processed: 64,
    processedTotal: 9370,
    failed: 0,
    ...overrides,
  };
}

function operation() {
  return { progress: vi.fn(), complete: vi.fn(), fail: vi.fn(), cancel: vi.fn() };
}

describe("作り直しの見張り", () => {
  beforeEach(() => vi.clearAllMocks());

  it("画面がいなくても、進捗を操作履歴へ送り続ける", () => {
    const op = operation();
    trackManualRebuild({ jobId: "job-1", operation: op });

    __testEmitSearchIndexProgress(progress({ processed: 64 }));
    __testEmitSearchIndexProgress(progress({ processed: 4_096 }));
    __testEmitSearchIndexProgress(progress({ processed: 7_683 }));

    expect(op.progress).toHaveBeenCalledTimes(3);
    expect(op.progress).toHaveBeenLastCalledWith(7_683, 9_370);
  });

  it("終わったら履歴を閉じ、見張りを外す", () => {
    const settled = vi.fn();
    const op = operation();
    trackManualRebuild({ jobId: "job-1", operation: op, onSettled: settled });

    __testEmitSearchIndexProgress(progress({ status: "completed", processed: 9_370, failed: 0 }));

    expect(op.complete).toHaveBeenCalledOnce();
    expect(settled).toHaveBeenCalledOnce();
    expect(trackedRebuildJobId()).toBeNull();
  });

  it("中止と失敗は、それぞれの終わり方で閉じる", () => {
    const canceled = operation();
    trackManualRebuild({ jobId: "job-1", operation: canceled });
    __testEmitSearchIndexProgress(progress({ status: "canceled", processed: 128 }));
    expect(canceled.cancel).toHaveBeenCalledOnce();
    expect(canceled.complete).not.toHaveBeenCalled();

    const failed = operation();
    trackManualRebuild({ jobId: "job-2", operation: failed });
    __testEmitSearchIndexProgress(progress({ jobId: "job-2", status: "failed", error: "壊れました" }));
    expect(failed.fail).toHaveBeenCalledWith("壊れました");
  });

  it("別の仕事の進捗は取り違えない", () => {
    const op = operation();
    trackManualRebuild({ jobId: "job-1", operation: op });

    __testEmitSearchIndexProgress(progress({ jobId: "other-job", processed: 5_000 }));

    expect(op.progress).not.toHaveBeenCalled();
    expect(trackedRebuildJobId()).toBe("job-1");
  });
});
