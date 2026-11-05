import type { AdminStatus, Storage } from "./api";

/** 「5 分前」「今日 20:40」の形にする。いつの情報かが分かることを優先する。 */
export function relativeTime(value: string | number | null | undefined, now = Date.now()): string {
  if (value === null || value === undefined) return "なし";
  const at = new Date(value);
  if (Number.isNaN(at.getTime())) return "不明";
  const minutes = Math.floor((now - at.getTime()) / 60_000);
  if (minutes < 1) return "たった今";
  if (minutes < 60) return `${minutes} 分前`;
  return dayTime(at, new Date(now));
}

export function dayTime(at: Date, now = new Date()): string {
  const time = `${at.getHours()}:${String(at.getMinutes()).padStart(2, "0")}`;
  const days = Math.round((startOfDay(now) - startOfDay(at)) / 86_400_000);
  if (days === 0) return `今日 ${time}`;
  if (days === 1) return `昨日 ${time}`;
  if (at.getFullYear() === now.getFullYear()) return `${at.getMonth() + 1}月${at.getDate()}日 ${time}`;
  return `${at.getFullYear()}年${at.getMonth() + 1}月${at.getDate()}日 ${time}`;
}

function startOfDay(at: Date): number {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime();
}

export function bytes(value: number | null | undefined): string {
  if (value === null || value === undefined) return "不明";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let amount = value;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  return `${unit === 0 || amount >= 100 ? Math.round(amount) : amount.toFixed(1)} ${units[unit]}`;
}

export function isNewer(candidate: string | null | undefined, current: string): boolean {
  if (!candidate) return false;
  const parts = (text: string) => text.split(".").map((part) => Number.parseInt(part, 10) || 0);
  const [a, b] = [parts(candidate), parts(current)];
  for (let index = 0; index < 3; index += 1) {
    if ((a[index] ?? 0) !== (b[index] ?? 0)) return (a[index] ?? 0) > (b[index] ?? 0);
  }
  return false;
}

/** `03:00-06:00` を「3:00〜6:00」にする。 */
export function windowLabel(window: string): string {
  const [start, end] = window.split("-");
  const short = (time: string | undefined) => (time ?? "").trim().replace(/^0(\d:)/, "$1");
  return `${short(start)}〜${short(end)}`;
}

export type AdminPage = "overview" | "updates" | "devices" | "library" | "records" | "security";

export interface Issue {
  message: string;
  page: AdminPage;
}

/** 空きがこれより少なければ知らせる。 */
const LOW_FREE_BYTES = 10 * 1024 ** 3;

/** 今ある問題を、重い順に返す。無ければ空。 */
export function findIssues(status: AdminStatus, storage: Storage | undefined): Issue[] {
  const issues: Issue[] = [];
  if (storage?.freeBytes !== null && storage?.freeBytes !== undefined && storage.freeBytes < LOW_FREE_BYTES) {
    issues.push({ message: `ドライブの空きが少なくなっています（${bytes(storage.freeBytes)}）`, page: "library" });
  }
  if (status.update?.lastError) {
    issues.push({ message: `更新に失敗しました（${relativeTime(status.update.lastCheckedAt)}に確認）`, page: "updates" });
  }
  for (const version of status.update?.skipped ?? []) {
    issues.push({ message: `${version} は起動に失敗したため飛ばしています`, page: "updates" });
  }
  if (status.sync?.indexError) {
    issues.push({ message: "検索の索引を作れていません", page: "library" });
  }
  return issues;
}
