import { relativeTime } from "@/lib/serverFormat";
import type { AdminStatus, Storage } from "./api";

export { dayTime, isNewer, relativeTime, windowLabel } from "@/lib/serverFormat";

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
