import DOMPurify from "dompurify";

/** 取得元の説明は HTML で来る。改行だけ残して文字にする。 */
export function plainText(source: string | null | undefined): string {
  if (!source) return "";
  const safe = DOMPurify.sanitize(source, { ALLOWED_TAGS: ["br"], ALLOWED_ATTR: [] });
  const holder = document.createElement("div");
  holder.innerHTML = safe.replace(/<br\s*\/?\s*>/gi, "\n");
  return holder.textContent?.trim() ?? "";
}

export function displayDate(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleDateString("ja-JP");
}

export function longDate(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

/**
 * 全体の割合を丸めると、201 ページの本は 200 ページ目で 100% になる。読書位置を片付ける
 * 条件と同じく、最後のページの端まで読んだときだけ読了とみなす。デスクトップ版と同じ考え。
 */
export function readingFinished(progress: number, pageCount: number): boolean {
  return pageCount > 0 && progress * pageCount >= pageCount - 0.02;
}

/**
 * 進み具合の百分率。読み終えたときだけ 100 にする。端まで来ていないのに丸めで 100 と出さない。
 * 少しでも読み進めていれば 1 からにする。読みかけの棚に 0% と並ぶのを避ける。
 */
export function readingPercent(progress: number, pageCount = 0): number {
  if (pageCount > 0 && readingFinished(progress, pageCount)) return 100;
  if (!(progress > 0)) return 0;
  return Math.max(1, Math.min(99, Math.round(progress * 100)));
}

export function progressLabel(progress: number, pageCount: number): string {
  return `${readingPercent(progress, pageCount)}%`;
}

export function scrollMainToTop(): void {
  document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
}
