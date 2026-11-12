import DOMPurify from "dompurify";
import { formatDate, formatDateNumeric, parseDate } from "@shared/format";

export { minimumReadProgress, progressLabel, readingFinished, readingPercent } from "@shared/readingProgress";

/** 取得元の説明は HTML で来る。改行だけ残して文字にする。 */
export function plainText(source: string | null | undefined): string {
  if (!source) return "";
  const safe = DOMPurify.sanitize(source, { ALLOWED_TAGS: ["br"], ALLOWED_ATTR: [] });
  const holder = document.createElement("div");
  holder.innerHTML = safe.replace(/<br\s*\/?\s*>/gi, "\n");
  return holder.textContent?.trim() ?? "";
}

/** 一覧のカードに出す日付。読めない値は出さない。 */
export function displayDate(value: string | null | undefined): string | null {
  return value && !Number.isNaN(parseDate(value).getTime()) ? formatDateNumeric(value) : null;
}

/** 作品情報に出す日付。 */
export function longDate(value: string | null | undefined): string | null {
  return value && !Number.isNaN(parseDate(value).getTime()) ? formatDate(value) : null;
}

export function scrollMainToTop(): void {
  document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
}
