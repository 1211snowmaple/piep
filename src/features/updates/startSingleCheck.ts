import { notifications } from "@mantine/notifications";
import { errorMessage } from "@/lib/format";
import { isTauriRuntime } from "@/services/dbApi";
import { startUpdateJobCommand } from "@/services/updateJobApi";
import { loadSchedule } from "@/features/updates/updateSchedule";

/**
 * Checks one work, author or series right now.
 *
 * Nothing is added to the watch list: this is a one-off look, which is what
 * "check this one thing" should mean. If the same author or series is already
 * watched, the job reuses its last-seen position instead of walking the whole
 * history again.
 */
export async function startSingleCheck(
  target:
    | { kind: "work"; workId: number; label: string }
    | { kind: "author" | "series"; source: string; sourceKey: string; label: string },
): Promise<void> {
  if (!isTauriRuntime()) {
    notifications.show({ color: "gray", message: "デスクトップアプリでのみ実行できます" });
    return;
  }
  const schedule = await loadSchedule();
  await startUpdateJobCommand({
    scope: target.kind === "work" ? "work" : (target.kind as "author" | "series"),
    mode: "check_only",
    workIds: target.kind === "work" ? [target.workId] : null,
    adhocTargets: target.kind === "work"
      ? null
      : [{ targetType: target.kind, source: target.source, sourceKey: target.sourceKey, displayName: target.label }],
    watchSaved: schedule.watchSaved,
  });
}

/** Starts the check and reports the outcome, for use straight from a button. */
export async function runSingleCheck(
  target: Parameters<typeof startSingleCheck>[0],
  onStarted?: () => void,
): Promise<void> {
  try {
    await startSingleCheck(target);
    notifications.show({ color: "piep", title: "更新を確認しています", message: `「${target.label}」の結果は更新センターに出ます` });
    onStarted?.();
  } catch (error) {
    notifications.show({ color: "red", title: "更新確認を開始できません", message: errorMessage(error) });
  }
}
