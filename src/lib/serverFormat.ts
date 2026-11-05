/** サーバーの画面で使う書き方。デスクトップ版と管理画面で同じにする。 */

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
