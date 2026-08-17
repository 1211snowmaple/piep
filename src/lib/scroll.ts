/** Everything below the fixed header scrolls inside this element. */
const VIEWPORT_SELECTOR = ".app-main";

/** Clear of the sticky header, close enough to read as "the top of this". */
const DEFAULT_GAP = 12;

/**
 * Long enough for a page of rows to arrive over IPC and for the list to grow
 * back to its full height, short enough that a stale attempt cannot fight
 * somebody who has started scrolling.
 */
const SETTLE_MS = 1200;

/** Anything the reader does to the scroll position themselves. */
const USER_SCROLL_EVENTS = ["wheel", "touchstart", "keydown", "pointerdown"] as const;

function viewportFor(element: HTMLElement): HTMLElement | null {
  return element.closest<HTMLElement>(VIEWPORT_SELECTOR);
}

function offsetOf(element: HTMLElement, viewport: HTMLElement, gap: number): number {
  const top = element.getBoundingClientRect().top - viewport.getBoundingClientRect().top + viewport.scrollTop - gap;
  return Math.max(0, top);
}

/**
 * Everything these do is in response to a press.
 *
 * They are deliberately plain functions rather than a hook watching a value.
 * Watching meant anything that happened to change that value moved the page:
 * a stored preference settling a tick after mount was enough to scroll a
 * freshly opened author screen past the profile nobody had read yet. A press
 * is the only thing that should move the reader, so a press is what calls these.
 */

/** Back to the very top, which is where a fresh listing starts. */
export function scrollViewportToTop(within: HTMLElement | null | undefined): void {
  const viewport = within ? viewportFor(within) : document.querySelector<HTMLElement>(VIEWPORT_SELECTOR);
  viewport?.scrollTo({ top: 0, left: 0 });
}

/**
 * Puts the top of a region just under the header.
 *
 * The offset is recomputed and reapplied for a moment because the incoming rows
 * are still arriving: the page is briefly too short to hold any offset at all,
 * and whatever was set while it was short gets clamped away.
 */
export function scrollRegionIntoView(element: HTMLElement | null | undefined, gap = DEFAULT_GAP): () => void {
  const viewport = element && viewportFor(element);
  if (!element || !viewport) return () => undefined;
  let frame = 0;
  const deadline = performance.now() + SETTLE_MS;
  const stop = () => {
    cancelAnimationFrame(frame);
    for (const event of USER_SCROLL_EVENTS) viewport.removeEventListener(event, stop);
  };
  const apply = () => {
    const target = offsetOf(element, viewport, gap);
    viewport.scrollTo({ top: target, left: 0 });
    if (Math.abs(viewport.scrollTop - target) <= 1 || performance.now() >= deadline) {
      stop();
      return;
    }
    frame = requestAnimationFrame(apply);
  };
  // Scrolling by hand outranks this: being pulled back while trying to leave is
  // worse than not being moved in the first place.
  for (const event of USER_SCROLL_EVENTS) viewport.addEventListener(event, stop, { passive: true });
  apply();
  return stop;
}
