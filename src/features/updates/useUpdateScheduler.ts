import { useEffect, useRef } from "react";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { isTauriRuntime, listUpdateTargets, searchDownloadsV2 } from "@/services/dbApi";
import { listUpdateJobsCommand, startUpdateJobCommand } from "@/services/updateJobApi";
import { isUpdateJobActive, isUpdateJobTerminal, type UpdateJobSnapshot, type UpdateJobSummary } from "@/features/updates/updateJobs";
import { isRunDue, loadSchedule, readLastRun, writeLastRun } from "@/features/updates/updateSchedule";

/** How often the app re-checks whether a scheduled run has come due. */
const TICK_MS = 5 * 60_000;
/** Give the app a moment to finish opening before the first check. */
const STARTUP_DELAY_MS = 8_000;

/**
 * Announces something through the operating system.
 *
 * Only when the app is in the background: with the window in front, the
 * in-app notice already says the same thing, and a second copy sliding in
 * from the corner is just noise.
 *
 * On Windows the toast is attributed to the app only when it runs from an
 * install - the notification plugin sets the AppUserModelID everywhere except
 * `target/debug` and `target/release`, so a toast fired from `tauri dev`
 * reports itself as Windows PowerShell. That is the plugin's fallback for
 * programs without a Start Menu entry, and it goes away in a built app.
 */
async function notify(title: string, body: string) {
  if (typeof document !== "undefined" && document.hasFocus()) return;
  try {
    const granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
    if (granted) sendNotification({ title, body });
  } catch {
    /* 通知が使えない環境でも、確認そのものは済んでいる。 */
  }
}

function describeResult(snapshot: UpdateJobSnapshot | UpdateJobSummary): string {
  const parts = [`確認 ${snapshot.processed}件`];
  if (snapshot.candidateCount) parts.push(`候補 ${snapshot.candidateCount}件`);
  if (snapshot.savedCount) parts.push(`保存 ${snapshot.savedCount}件`);
  if (snapshot.errorCount) parts.push(`エラー ${snapshot.errorCount}件`);
  return parts.join(" · ");
}

/**
 * Runs the update check on the schedule the settings describe.
 *
 * Deliberately modest: it only runs while the app is open, never starts a
 * second job on top of a live one, and records the run only after the worker
 * has accepted it so a transient start failure remains eligible for retry.
 */
export function useUpdateScheduler(enabled = isTauriRuntime()) {
  const startupDone = useRef(false);
  const running = useRef(false);

  useEffect(() => {
    if (!enabled) return undefined;
    let cancelled = false;

    const tick = async (startup: boolean) => {
      if (cancelled || running.current) return;
      running.current = true;
      try {
        const settings = await loadSchedule();
        const lastRun = await readLastRun();
        if (!isRunDue(settings, lastRun, Date.now(), startup)) return;

        // すでに走っているジョブがあれば、そのまま任せる。待つのは worker が
        // 付いているものだけ - 一時停止や再接続待ちのまま放置された1件を
        // 「実行中」と数えると、以後の自動確認が永久に始まらなくなる。
        const jobs = await listUpdateJobsCommand();
        if (jobs.some((job) => isUpdateJobActive(job.status))) return;

        // 監視対象も監視作品も無いなら、確認する意味がない。作品の監視は
        // downloads 側の旗なので、update_targets を見るだけでは数え落とす。
        const [targets, watchedWorks] = await Promise.all([
          listUpdateTargets<{ id: number }>(null, true),
          searchDownloadsV2({ watchFilter: "watched", limit: 1, projection: "bulk" }),
        ]);
        if (!targets.length && !watchedWorks.items.length) return;

        const snapshot = await startUpdateJobCommand({
          scope: "all",
          mode: settings.mode,
          watchSaved: settings.watchSaved,
        });
        // ジョブを作れた時刻だけを記録する。開始前に保存すると、一時的な
        // IPC/DB失敗でも次の間隔まで確認を丸ごと飛ばしてしまう。
        await writeLastRun(Date.now());
        if (settings.notify) {
          await notify("更新の自動確認を開始しました", `対象 ${snapshot.totals}件`);
        }
      } catch {
        /* 自動実行の失敗で画面を止めない。次の機会に再試行される。 */
      } finally {
        running.current = false;
      }
    };

    const startupTimer = window.setTimeout(() => {
      startupDone.current = true;
      void tick(true);
    }, STARTUP_DELAY_MS);
    const interval = window.setInterval(() => void tick(false), TICK_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(startupTimer);
      window.clearInterval(interval);
    };
  }, [enabled]);
}

/** Announces a finished job once, through the operating system. */
export function useUpdateJobNotifications(snapshot: UpdateJobSnapshot | null, enabled = isTauriRuntime()) {
  const announced = useRef<string | null>(null);

  useEffect(() => {
    if (!enabled || !snapshot || !isUpdateJobTerminal(snapshot.status)) return;
    if (announced.current === snapshot.jobId) return;
    announced.current = snapshot.jobId;
    void (async () => {
      const settings = await loadSchedule();
      if (!settings.notify) return;
      const title = snapshot.candidateCount
        ? `新しい候補が${snapshot.candidateCount}件あります`
        : "更新の確認が終わりました";
      await notify(title, describeResult(snapshot));
    })();
  }, [enabled, snapshot]);
}
