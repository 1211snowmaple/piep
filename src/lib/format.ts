export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value.toLocaleString("ja-JP", { maximumFractionDigits: index === 0 ? 0 : 1 })} ${units[index]}`;
}

export function formatNumber(value: number | null | undefined): string {
  return (value ?? 0).toLocaleString("ja-JP");
}

/**
 * A count for somewhere narrow, where the exact figure matters less than its
 * size.
 *
 * The sidebar is a fixed 194px and cannot grow, so a library that keeps growing
 * would eventually push its own labels onto a second line. Up to five digits
 * the real number still fits and is more useful; past that it becomes 万.
 */
export function formatCompactCount(value: number | null | undefined): string {
  const count = value ?? 0;
  if (!Number.isFinite(count)) return "0";
  if (count < 100_000) return count.toLocaleString("ja-JP");
  const man = count / 10_000;
  return man >= 100 ? `${Math.round(man).toLocaleString("ja-JP")}万` : `${man.toFixed(1)}万`;
}

export function formatDate(value: string | null | undefined, withTime = false): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("ja-JP", {
    year: "numeric",
    month: "short",
    day: "numeric",
    ...(withTime ? { hour: "2-digit", minute: "2-digit" } : {}),
  }).format(date);
}

/**
 * 「いつ確認したか」を、鮮度として読める形にする。
 *
 * 確認日は「何月何日か」より「どれだけ経ったか」が知りたい情報なので、
 * 近いうちは相対で書く。ただし遠くなると相対は読みにくくなる
 * （「183日前」より「2026年2月20日」のほうが早く分かる）ので、
 * 30日を越えたら絶対日付へ戻す。
 *
 * 基準時刻を渡せるようにしてあるのは、時計を止めて試せるようにするため。
 */
export function formatFreshness(value: string | null | undefined, now: Date = new Date()): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const elapsedMs = now.getTime() - date.getTime();
  // 未来の時刻は「これから」ではなく、時計のずれとして扱う。
  if (elapsedMs < 0) return "さっき";
  const hours = Math.floor(elapsedMs / 3_600_000);
  if (hours < 1) return "さっき";
  if (hours < 24) return `${hours}時間前`;
  const days = Math.floor(hours / 24);
  if (days <= 30) return `${days}日前`;
  return formatDate(value);
}

/** Compact numeric date (2026/08/05) for dense rows where a label would not fit. */
export function formatDateNumeric(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

/**
 * Human-scale duration for progress readouts. Anything under a minute is read
 * in seconds, because "0分52秒" reads as slower than "約52秒".
 */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return "—";
  const rounded = Math.round(seconds);
  if (rounded < 60) return `${rounded}秒`;
  const minutes = Math.floor(rounded / 60);
  if (minutes < 60) {
    const rest = rounded % 60;
    return rest ? `${minutes}分${rest}秒` : `${minutes}分`;
  }
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  return restMinutes ? `${hours}時間${restMinutes}分` : `${hours}時間`;
}

/** Throughput label that stays readable when the rate drops below one per second. */
export function formatRate(perSecond: number | null | undefined, unit = "件"): string {
  if (perSecond === null || perSecond === undefined || !Number.isFinite(perSecond) || perSecond <= 0) return "—";
  if (perSecond >= 10) return `${Math.round(perSecond).toLocaleString("ja-JP")} ${unit}/秒`;
  if (perSecond >= 1) return `${perSecond.toFixed(1)} ${unit}/秒`;
  return `${(perSecond * 60).toFixed(1)} ${unit}/分`;
}

export function contentTypeLabel(value: string): string {
  const labels: Record<string, string> = {
    novel: "小説",
    article: "記事",
    image: "画像",
    file: "ファイル",
    text: "テキスト",
  };
  return labels[value] ?? value;
}

export function errorMessage(error: unknown): string {
  // JSON.stringify は undefined を返しうる（undefined・関数・Symbol）。
  // 受け取ったものをそのまま .trim() すると、エラーを言葉にする途中で
  // 別のエラーを投げることになる - 一番やってはいけない場所で。
  if (error === null || error === undefined) return "不明なエラー";
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : (() => {
    try { return JSON.stringify(error) ?? "不明なエラー"; } catch { return "不明なエラー"; }
  })();
  let message = raw.trim();
  try {
    const parsed = JSON.parse(message) as { message?: unknown; error?: unknown };
    if (typeof parsed.message === "string") message = parsed.message;
    else if (typeof parsed.error === "string") message = parsed.error;
  } catch { /* Tauri errors are usually plain strings. */ }
  message = message
    .replace(/(?:,?\s*)body:\s*[\s\S]*$/i, "")
    .replace(/\s+/g, " ")
    .trim();
  if (!message) return "処理に失敗しました。もう一度お試しください";
  return message.length > 280 ? `${message.slice(0, 279)}…` : message;
}
