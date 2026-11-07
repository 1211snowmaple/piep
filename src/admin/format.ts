import { relativeTime } from "@/lib/serverFormat";
import type { AdminStatus, Connection, Storage } from "./api";

export { dayTime, isNewer, relativeTime, windowLabel } from "@/lib/serverFormat";

/** 記録の日時。並べたときに桁がそろうよう、秒まで同じ形で出す。年は今年でなければ付ける。 */
export function stamp(at: Date, now = new Date()): string {
  const two = (value: number) => String(value).padStart(2, "0");
  const day = `${two(at.getMonth() + 1)}/${two(at.getDate())}`;
  const time = `${two(at.getHours())}:${two(at.getMinutes())}:${two(at.getSeconds())}`;
  return at.getFullYear() === now.getFullYear() ? `${day} ${time}` : `${at.getFullYear()}/${day} ${time}`;
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

export type AdminPage = "overview" | "updates" | "devices" | "library" | "records" | "settings" | "account";

/** 前の版の名前で開かれたページを、今の名前へ寄せる。 */
export function pageFromName(name: string): AdminPage | null {
  if (name === "security") return "settings";
  const pages: AdminPage[] = ["overview", "updates", "devices", "library", "records", "settings", "account"];
  return pages.includes(name as AdminPage) ? (name as AdminPage) : null;
}

const isOpen = (address: string) => !address.startsWith("127.") && !address.startsWith("[::1]") && !address.startsWith("localhost");
const isAnyAddress = (address: string) => address.startsWith("0.0.0.0:") || address.startsWith("[::]:");

/** 管理画面を開く場所。出さない設定なら null。 */
export function adminPlace(connection: Connection): { url: string; thisMachineOnly: boolean } | null {
  const scheme = connection.tls === "none" ? "http" : "https";
  const at = (address: string) => isAnyAddress(address) ? `サーバーの PC のアドレス:${address.slice(address.lastIndexOf(":") + 1)}` : address;
  if (connection.adminListen) {
    return { url: `${scheme}://${at(connection.adminListen)}/admin/`, thisMachineOnly: !isOpen(connection.adminListen) };
  }
  if (!connection.publicUrl) {
    return { url: `${scheme}://${at(connection.listen)}/admin/`, thisMachineOnly: !isOpen(connection.listen) };
  }
  if (connection.localAdmin) {
    return { url: `http://${connection.localAdmin}/admin/`, thisMachineOnly: true };
  }
  return null;
}

/** 危ない組み合わせと、その直し方。 */
export function connectionWarnings(connection: Connection): string[] {
  const warnings: string[] = [];
  if (connection.adminListen && isOpen(connection.adminListen)) {
    warnings.push(`管理画面のアドレス ${connection.adminListen} に、ほかの機器から届きます。家の中のネットワークか VPN の中だけに置いてください。`);
  }
  if (!connection.adminListen && !connection.publicUrl && isOpen(connection.listen)) {
    warnings.push(`携帯用と管理用が同じアドレス ${connection.listen} で、ほかの機器から届きます。「管理画面のアドレスを分ける」で分けてください。`);
  }
  return warnings;
}

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

/** 接続の設定の書きかけ。分けない間も管理用のアドレスを覚えておく。 */
export interface ConnectionDraft {
  listen: string;
  split: boolean;
  adminListen: string;
  publicUrl: string;
  tls: Connection["tls"];
  /** 空なら今のファイルのまま。 */
  cert: string;
  key: string;
}

export function draftConnection(connection: Connection): ConnectionDraft {
  return {
    listen: connection.listen,
    split: connection.adminListen !== null,
    adminListen: connection.adminListen ?? "127.0.0.1:38778",
    publicUrl: connection.publicUrl ?? "",
    tls: connection.tls,
    cert: "",
    key: "",
  };
}

export function sameConnection(a: Connection, b: Connection): boolean {
  return a.listen === b.listen && a.adminListen === b.adminListen && a.publicUrl === b.publicUrl && a.tls === b.tls;
}
