import { describe, expect, it } from "vitest";
import type { UpdateJobSummary } from "@/services/updateJobApi";
import type { OperationJob } from "./operationJobs";
import {
  countActiveActivities,
  dedupeActivityOperations,
} from "./activityAggregation";

const startedAt = "2026-08-29T00:00:00Z";

function operation(overrides: Partial<OperationJob> = {}): OperationJob {
  return {
    id: "local-1",
    kind: "save",
    label: "2件を保存",
    detail: null,
    status: "running",
    current: 1,
    total: 2,
    startedAt,
    updatedAt: startedAt,
    finishedAt: null,
    canCancel: true,
    canRetry: false,
    logs: [],
    ...overrides,
  };
}

function updateJob(overrides: Partial<UpdateJobSummary> = {}): UpdateJobSummary {
  return {
    jobId: "backend-1",
    status: "running",
    scope: "save",
    mode: "save",
    totals: 2,
    processed: 1,
    candidateCount: 0,
    savedCount: 1,
    errorCount: 0,
    activeLabel: "保存中",
    startedAt,
    updatedAt: startedAt,
    finishedAt: null,
    ...overrides,
  };
}

describe("activity aggregation", () => {
  it("counts a linked save operation and backend job only once", () => {
    const local = operation({ externalJobId: "backend-1" });
    expect(dedupeActivityOperations([local], [updateJob()])).toEqual([]);
    expect(countActiveActivities([local], [updateJob()])).toBe(1);
  });

  it("counts an automatic update job even without a local operation", () => {
    expect(
      countActiveActivities([], [
        updateJob({ jobId: "automatic", mode: "check_only", scope: "all" }),
      ]),
    ).toBe(1);
  });

  it("does not call paused or completed jobs active", () => {
    expect(
      countActiveActivities([], [
        updateJob({ jobId: "paused", status: "paused" }),
        updateJob({ jobId: "done", status: "completed" }),
      ]),
    ).toBe(0);
  });
});
