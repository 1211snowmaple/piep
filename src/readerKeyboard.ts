import type { KeyboardEvent } from "react";

/** Move within a long transfer page before turning to the next one. */
export function handleReaderKeyboard(
  event: KeyboardEvent<HTMLElement>,
  page: number,
  pageCount: number,
  changePage: (page: number, edge: "start" | "end") => void,
): void {
  if (event.defaultPrevented || event.target !== event.currentTarget || event.nativeEvent.isComposing
    || event.altKey || event.ctrlKey || event.metaKey || (event.shiftKey && event.key !== " ")
    || pageCount < 1) return;
  const direction = event.key === "PageDown" || event.key === "ArrowRight" || (event.key === " " && !event.shiftKey)
    ? 1
    : event.key === "PageUp" || event.key === "ArrowLeft" || (event.key === " " && event.shiftKey)
      ? -1
      : 0;
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
