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

/** Long enough to tell "the panel has finished growing" from "between rows". */
const QUIET_MS = 250;

/** Frames the held offset has to survive before it counts as taken. */
const STABLE_FRAMES = 3;

/** Keys that move a scrolling page even while focus remains in the fixed header. */
const PAGE_SCROLL_KEYS = new Set(["PageUp", "PageDown", "Home", "End"]);

/** Keys that scroll only when they are not being consumed by a control. */
const CONDITIONAL_SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", " ", "Spacebar"]);

function eventTarget(event: Event): Element | null {
  return event.target instanceof Element ? event.target : null;
}

function isEditable(target: Element | null): boolean {
  if (!target) return false;
  return Boolean(target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false']), [role='textbox'], [role='combobox']"));
}

function isInteractive(target: Element | null): boolean {
  if (!target) return false;
  return Boolean(target.closest("button, a, summary, [role='button'], [role='link'], [role='tab'], [role='menuitem'], [role='option'], [role='slider'], [role='switch']"));
}

/**
 * Observe intent to move one AppFrame viewport.
 *
 * Restoration must yield to the reader, but not to every interaction anywhere
 * in the window. A tap on the mobile menu or typing into a fixed-header field
 * does not move `.app-main`; treating either as scrolling used to abandon a
 * restore while a virtual list was still filling in. Pointer/touch/wheel input
 * therefore belongs to the viewport it starts in, while page-navigation keys
 * are also observed from the fixed header because browsers still apply them to
 * the page scroller from there.
 */
export function listenForScrollIntent(viewport: HTMLElement, onIntent: () => void): () => void {
  const withinViewport = (event: Event) => {
    const target = eventTarget(event);
    if (target && (target === viewport || viewport.contains(target))) onIntent();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const target = eventTarget(event);
    if (isEditable(target)) return;
    if (PAGE_SCROLL_KEYS.has(event.key)) {
      onIntent();
      return;
    }
    // Space and arrows activate or navigate many controls. They only express
    // page-scroll intent when no such control owns the key.
    if (CONDITIONAL_SCROLL_KEYS.has(event.key) && !isInteractive(target)) onIntent();
  };
  window.addEventListener("wheel", withinViewport, { passive: true });
  window.addEventListener("touchstart", withinViewport, { passive: true });
  window.addEventListener("pointerdown", withinViewport, { passive: true });
  window.addEventListener("keydown", onKeyDown);
  return () => {
    window.removeEventListener("wheel", withinViewport);
    window.removeEventListener("touchstart", withinViewport);
    window.removeEventListener("pointerdown", withinViewport);
    window.removeEventListener("keydown", onKeyDown);
  };
}

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
  let stopListening: () => void = () => undefined;
  const stop = () => {
    cancelAnimationFrame(frame);
    stopListening();
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
  // Keyboard focus can remain in AppFrame's fixed header while PageDown moves
  // this viewport. Observe intent at the window so that movement is not undone.
  stopListening = listenForScrollIntent(viewport, stop);
  apply();
  return stop;
}

/**
 * Keeps a region where it is on screen while what hangs below it is replaced.
 *
 * Swapping a tab's panel empties the page for a moment: the incoming panel has
 * not fetched its rows yet, so the document is briefly shorter than the offset
 * the reader is holding, the browser clamps that offset away to fit, and the
 * screen appears to throw itself back to the top on its own.
 *
 * Unlike scrollRegionIntoView this cannot stop at the first frame that already
 * matches - at the moment of the press nothing has moved yet, so every frame
 * matches until the panel swaps. It stops once the offset has survived a few
 * frames and the page has stopped changing height. A page too short to hold the
 * offset at all never satisfies the first half, so a panel that sits on a
 * loading state for a while - where the height is perfectly quiet and the
 * content is not final - is still waited out, up to the deadline.
 */
export function holdRegionInPlace(element: HTMLElement | null | undefined): () => void {
  const viewport = element && viewportFor(element);
  if (!element || !viewport) return () => undefined;
  // Where the region sits on screen right now is the gap to preserve, so the
  // offset this recomputes each frame is the one the reader already has.
  const gap = element.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
  let frame = 0;
  let held = 0;
  let height = viewport.scrollHeight;
  let quietSince = performance.now();
  const deadline = performance.now() + SETTLE_MS;
  let stopListening: () => void = () => undefined;
  const stop = () => {
    cancelAnimationFrame(frame);
    stopListening();
  };
  const apply = () => {
    const target = offsetOf(element, viewport, gap);
    viewport.scrollTo({ top: target, left: 0 });
    const now = performance.now();
    if (viewport.scrollHeight !== height) { height = viewport.scrollHeight; quietSince = now; }
    held = Math.abs(viewport.scrollTop - target) <= 1 ? held + 1 : 0;
    if ((held >= STABLE_FRAMES && now - quietSince >= QUIET_MS) || now >= deadline) {
      stop();
      return;
    }
    frame = requestAnimationFrame(apply);
  };
  stopListening = listenForScrollIntent(viewport, stop);
  apply();
  return stop;
}
