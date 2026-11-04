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

/** 日本語の黙読はおよそ毎分 600 字。目安なので分の単位で丸める。 */
export function readingMinutes(chars: number): string | null {
  if (chars <= 0) return null;
  const minutes = Math.max(1, Math.round(chars / 600));
  if (minutes < 60) return `約 ${minutes} 分`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `約 ${hours} 時間 ${rest} 分` : `約 ${hours} 時間`;
}

export function scrollMainToTop(): void {
  document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
}
