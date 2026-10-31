import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { notifications } from "@mantine/notifications";
import { getDownloads, isTauriRuntime } from "@/services/dbApi";
import { getUpdateJobCommand, getUpdateJobCredentials, listDeferredUpdateCandidatesCommand, listUpdateJobsCommand, startSaveJobCommand, type SaveJobWork, type UpdateJobSnapshot } from "@/services/updateJobApi";
import { invalidateAfterUpdateJob, isUpdateJobActive } from "./updateJobs";
import { subscribeTauriEvent } from "@/services/eventBus";

// v1 jobs were stopped by the generic held-candidate gate before fetching.
// Retry that generation once after repairing the native dispatch path.
const ATTEMPTS_KEY = "piep.incomplete-save-repair.v2";
// A backlog (e.g. R-18 details masked by older builds) becomes a series of
// short jobs. Each waits for the user's own jobs, so none monopolises the
// connection for hours.
export const REPAIR_BATCH_SIZE = 50;
type RepairResult = { waiting: boolean; job?: UpdateJobSnapshot };
let starting: Promise<RepairResult> | null = null;

function readAttempts(): Set<string> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(ATTEMPTS_KEY) ?? "[]");
    return new Set(Array.isArray(parsed) ? parsed.filter((key): key is string => typeof key === "string") : []);
  } catch { return new Set(); }
}

async function startRepair(shouldContinue: () => boolean): Promise<RepairResult> {
  if ((await listUpdateJobsCommand()).some((job) => (isUpdateJobActive(job.status) || job.status === "auth_required"))) return { waiting: true };
  const credentials = await getUpdateJobCredentials();
  if (!credentials.pixivRefreshToken || !credentials.pixivCookie || !credentials.pixivUserAgent) return { waiting: true };
  const candidates = (await listDeferredUpdateCandidatesCommand()).filter((entry) => {
    if (entry.source !== "pixiv" || entry.status !== "held" || !entry.downloadId) return false;
    try { return JSON.parse(entry.payloadJson).holdKind === "incomplete"; } catch { return false; }
  });
  if (!candidates.length) return { waiting: false };
  const works = await getDownloads(candidates.map((entry) => entry.downloadId!));
  const attempted = readAttempts();
  const repairWorks: SaveJobWork[] = [];
  const keys: string[] = [];
  for (const work of works) {
    if (repairWorks.length >= REPAIR_BATCH_SIZE) break;
    const key = `pixiv:${work.sourceId}`;
    if (attempted.has(key)) continue;
    repairWorks.push({ source: "pixiv", sourceId: work.sourceId, title: work.title });
    keys.push(key);
  }
  if (!repairWorks.length || !shouldContinue()) return { waiting: false };
  const job = await startSaveJobCommand(repairWorks, false, credentials);
  keys.forEach((key) => attempted.add(key));
  try { localStorage.setItem(ATTEMPTS_KEY, JSON.stringify([...attempted])); } catch { /* The durable native job still owns the accepted work. */ }
  return { waiting: false, job };
}

/** Repair known partial saves once per work, through the normal journaled save job. */
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
