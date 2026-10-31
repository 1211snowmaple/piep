import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { notifications } from "@mantine/notifications";
import { isTauriRuntime } from "@/services/dbApi";
import { getUpdateJobCommand, getUpdateJobCredentials, listIncompleteSaveRepairWorksCommand, listUpdateJobsCommand, recordIncompleteSaveRepairAttemptsCommand, startSaveJobCommand, type UpdateJobSnapshot } from "@/services/updateJobApi";
import { invalidateAfterUpdateJob, isUpdateJobActive } from "./updateJobs";
import { subscribeTauriEvent } from "@/services/eventBus";

// 以前は試した作品をここ（画面側の保存領域）に控えていた。入れ直すと消えて
// 全件を取り直し、版が進んでも二度と試さなかった。いまはDBが版ごとに覚える。
// 残っている控えは一度だけDBへ移して消す。
const LEGACY_ATTEMPTS_KEY = "piep.incomplete-save-repair.v2";
// A backlog (e.g. R-18 details masked by older builds) becomes a series of
// short jobs. Each waits for the user's own jobs, so none monopolises the
// connection for hours.
export const REPAIR_BATCH_SIZE = 50;
type RepairResult = { waiting: boolean; job?: UpdateJobSnapshot };
let starting: Promise<RepairResult> | null = null;

async function moveLegacyAttempts(): Promise<void> {
  let keys: string[] = [];
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(LEGACY_ATTEMPTS_KEY) ?? "[]");
    if (Array.isArray(parsed)) keys = parsed.filter((key): key is string => typeof key === "string");
  } catch { return; }
  const sourceIds = keys.filter((key) => key.startsWith("pixiv:")).map((key) => key.slice("pixiv:".length));
  if (sourceIds.length) await recordIncompleteSaveRepairAttemptsCommand(sourceIds);
  try { localStorage.removeItem(LEGACY_ATTEMPTS_KEY); } catch { /* 次の起動で移し直しても、記録が重なるだけ。 */ }
}

async function startRepair(shouldContinue: () => boolean): Promise<RepairResult> {
  if ((await listUpdateJobsCommand()).some((job) => (isUpdateJobActive(job.status) || job.status === "auth_required"))) return { waiting: true };
  const credentials = await getUpdateJobCredentials();
  if (!credentials.pixivRefreshToken || !credentials.pixivCookie || !credentials.pixivUserAgent) return { waiting: true };
  await moveLegacyAttempts();
  const repairWorks = await listIncompleteSaveRepairWorksCommand(REPAIR_BATCH_SIZE);
  if (!repairWorks.length || !shouldContinue()) return { waiting: false };
  const job = await startSaveJobCommand(repairWorks, false, credentials);
  await recordIncompleteSaveRepairAttemptsCommand(repairWorks.map((work) => work.sourceId));
  return { waiting: false, job };
}

/** 一部保存の作品を、版ごとに一度だけ、通常の保存ジョブで取り直す。 */
export function startIncompleteSaveRepair(shouldContinue: () => boolean = () => true): Promise<RepairResult> {
  starting ??= startRepair(shouldContinue).finally(() => { starting = null; });
  return starting;
}

export function useIncompleteSaveRepair() {
  const client = useQueryClient();
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let running = false;
    let finished = false;
    let jobId: string | undefined;
    let discovered = false;
    let announced = false;
    let timer: number;
    const tick = async () => {
      if (disposed || running || finished) return;
      running = true;
      try {
        if (jobId) {
          // A finished batch may leave more of the backlog; the next tick
          // decides, after the ordinary pause between batches.
          const job = await getUpdateJobCommand(jobId);
          if (!isUpdateJobActive(job.status)) { invalidateAfterUpdateJob(client); jobId = undefined; }
          return;
        }
        discovered = false;
        const result = await startIncompleteSaveRepair(() => !disposed);
        if (result.job) {
          jobId = result.job.jobId;
          if (!announced) notifications.show({ color: "blue", title: "保存済み作品を補完しています", message: "不足していた挿絵や作品情報を少しずつ再確認します。進捗は操作履歴で確認できます。" });
          announced = true;
        } else if (!result.waiting && !discovered) finished = true;
      } catch (error) { console.error("保存済み作品の補完を開始できませんでした", error); }
      finally {
        running = false;
        if (!disposed && !finished) timer = window.setTimeout(() => void tick(), jobId ? 2_000 : discovered ? 1_000 : 60_000);
      }
    };
    const unlisten = subscribeTauriEvent<number[]>("acquisition-audited", ({ payload }) => {
      if (!payload.length) return;
      discovered = true;
      finished = false;
      if (!running) { window.clearTimeout(timer); timer = window.setTimeout(() => void tick(), 1_000); }
    });
    timer = window.setTimeout(() => void tick(), 5_000);
    return () => { disposed = true; window.clearTimeout(timer); unlisten(); };
  }, [client]);
}
