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
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : (() => {
    try { return JSON.stringify(error); } catch { return "不明なエラー"; }
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
