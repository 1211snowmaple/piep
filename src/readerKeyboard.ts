import type { KeyboardEvent } from "react";

/** ページを送るキーなら向きを返す。送らないキーは 0。 */
export function readerKeyDirection(event: KeyboardEvent<HTMLElement>): -1 | 0 | 1 {
  if (event.defaultPrevented || event.target !== event.currentTarget || event.nativeEvent.isComposing
    || event.altKey || event.ctrlKey || event.metaKey || (event.shiftKey && event.key !== " ")) return 0;
  return event.key === "PageDown" || event.key === "ArrowRight" || (event.key === " " && !event.shiftKey)
    ? 1
    : event.key === "PageUp" || event.key === "ArrowLeft" || (event.key === " " && event.shiftKey)
      ? -1
      : 0;
}

/** Move within a long transfer page before turning to the next one. */
export function handleReaderKeyboard(
  event: KeyboardEvent<HTMLElement>,
  page: number,
  pageCount: number,
  changePage: (page: number, edge: "start" | "end") => void,
): void {
  const direction = pageCount < 1 ? 0 : readerKeyDirection(event);
  if (!direction) return;

  const viewport = event.currentTarget;
  const end = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
  if ((direction > 0 && viewport.scrollTop < end - 2) || (direction < 0 && viewport.scrollTop > 2)) {
    event.preventDefault();
    viewport.scrollBy({ top: direction * Math.max(1, viewport.clientHeight - 48), behavior: "smooth" });
    return;
  }
  const next = page + direction;
  if (next >= 0 && next < pageCount) {
    event.preventDefault();
    changePage(next, direction < 0 ? "end" : "start");
  }
}
