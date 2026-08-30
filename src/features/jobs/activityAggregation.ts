import { isUpdateJobActive } from "@/features/updates/updateJobs";
import type { UpdateJobSummary } from "@/services/updateJobApi";
import type { OperationJob } from "./operationJobs";

/**
 * 画面のなかで動いている操作。
 *
 * バックエンドのジョブは同じ語を使っていても持ち主が違うので、判定は
 * `isUpdateJobActive` に任せる。**同じ判断をふたつ置かない。**
 */
const ACTIVE_LOCAL_STATUSES = new Set(["queued", "running", "canceling"]);

type ActivityOrderKey = { status: string; updatedAt: string };

/** Running work stays visible; within the same state the newest change wins. */
export function compareActivityOrder(a: ActivityOrderKey, b: ActivityOrderKey): number {
  const aActive = ACTIVE_LOCAL_STATUSES.has(a.status);
  const bActive = ACTIVE_LOCAL_STATUSES.has(b.status);
  if (aActive !== bActive) return aActive ? -1 : 1;
  return Date.parse(b.updatedAt) - Date.parse(a.updatedAt);
}

/** Whether a page-local operation is only the UI mirror of a durable save job. */
export function isMirroredSaveOperation(
  operation: OperationJob,
  updateJobs: UpdateJobSummary[],
): boolean {
  if (operation.kind !== "save") return false;
  return updateJobs.some((job) =>
    job.mode === "save" &&
    (operation.externalJobId
      ? job.jobId === operation.externalJobId
      : job.totals === (operation.total ?? -1) &&
        Math.abs(
          Date.parse(job.startedAt) - Date.parse(operation.startedAt),
        ) < 10_000),
  );
}

/** Remove only UI mirrors; unrelated local work remains independently visible. */
export function dedupeActivityOperations(
  operations: OperationJob[],
  updateJobs: UpdateJobSummary[],
): OperationJob[] {
  return operations.filter(
    (operation) => !isMirroredSaveOperation(operation, updateJobs),
  );
}

/** One count for the sidebar and the operation-history summary. */
export function countActiveActivities(
  operations: OperationJob[],
  updateJobs: UpdateJobSummary[],
): number {
  const localCount = dedupeActivityOperations(operations, updateJobs).filter(
    (operation) => ACTIVE_LOCAL_STATUSES.has(operation.status),
  ).length;
  const updateCount = updateJobs.filter((job) =>
    isUpdateJobActive(job.status),
  ).length;
  return localCount + updateCount;
}
