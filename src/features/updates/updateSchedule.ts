import { store } from "@/store";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * When the update check runs on its own.
 *
 * piep is an app you open, not a service that runs in the background, so the
 * schedule is deliberately modest: it can run once when the app starts, and
 * again while it stays open. A run that came due while the app was closed is
 * carried out once at the next start rather than replayed for every interval
 * that passed.
 */
export interface UpdateScheduleSettings {
  /** Run once shortly after the app opens, if a run is due. */
  onStartup: boolean;
  /** Hours between runs. 0 turns the interval off; startup can still fire. */
  intervalHours: number;
  /** What an automatic run does: list candidates, or save them too. */
  mode: "check_only" | "auto_save";
  /** Put every work the check saves under update watching. */
  watchSaved: boolean;
  /** Announce the result through the operating system. */
  notify: boolean;
}

export const updateScheduleDefaults: UpdateScheduleSettings = {
  onStartup: false,
  intervalHours: 0,
  mode: "check_only",
  watchSaved: false,
  notify: true,
};

const SETTINGS_KEY = "update_schedule";
const LAST_RUN_KEY = "update_schedule_last_run";

/** Values a person could plausibly want. Anything else is clamped into range. */
const MIN_INTERVAL_HOURS = 1;
const MAX_INTERVAL_HOURS = 24 * 14;

export function normalizeSchedule(raw: unknown): UpdateScheduleSettings {
  const value = (raw ?? {}) as Partial<UpdateScheduleSettings>;
  const hours = Number(value.intervalHours);
  return {
    onStartup: value.onStartup === true,
    intervalHours: !Number.isFinite(hours) || hours <= 0
      ? 0
      : Math.min(MAX_INTERVAL_HOURS, Math.max(MIN_INTERVAL_HOURS, Math.round(hours))),
    mode: value.mode === "auto_save" ? "auto_save" : "check_only",
    watchSaved: value.watchSaved === true,
    notify: value.notify !== false,
  };
}

export async function loadSchedule(): Promise<UpdateScheduleSettings> {
  try {
    return normalizeSchedule(await store.get(SETTINGS_KEY));
  } catch {
    return { ...updateScheduleDefaults };
  }
}

export async function saveSchedule(settings: UpdateScheduleSettings): Promise<void> {
  // ブラウザで開いたプレビューには保存先が無い。触っても何も起きないのが正しい。
  if (!isTauriRuntime()) return;
  await store.set(SETTINGS_KEY, normalizeSchedule(settings));
  await store.save();
}

export async function readLastRun(): Promise<number | null> {
  try {
    const raw = await store.get<number>(LAST_RUN_KEY);
    return typeof raw === "number" && Number.isFinite(raw) ? raw : null;
  } catch {
    return null;
  }
}

export async function writeLastRun(at: number): Promise<void> {
  await store.set(LAST_RUN_KEY, at);
  await store.save();
}

/**
 * Whether an automatic run is due.
 *
 * `startup` is only true for the first evaluation after the app opens; a run
 * that came due while the app was closed happens once there, not once per
 * interval that elapsed.
 */
export function isRunDue(
  settings: UpdateScheduleSettings,
  lastRun: number | null,
  now: number,
  startup: boolean,
): boolean {
  if (settings.onStartup && startup) {
    // 起動直後は、前回から間隔を空けているときだけ走らせる。間隔が無効の
    // ときは「開くたびに1回」が意図なので、そのまま走らせる。
    if (!settings.intervalHours) return true;
    return lastRun === null || now - lastRun >= settings.intervalHours * 3_600_000;
  }
  if (!settings.intervalHours) return false;
  if (lastRun === null) return !startup;
  return now - lastRun >= settings.intervalHours * 3_600_000;
}

/** Human summary for the settings screen. */
export function describeSchedule(settings: UpdateScheduleSettings): string {
  const what = settings.mode === "auto_save" ? "確認して保存" : "確認のみ";
  if (!settings.onStartup && !settings.intervalHours) return "自動では実行しません";
  const parts = [];
  if (settings.onStartup) parts.push("起動時");
  if (settings.intervalHours) parts.push(`${settings.intervalHours}時間ごと`);
  return `${parts.join(" と ")}に${what}`;
}
