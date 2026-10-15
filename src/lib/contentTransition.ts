import { flushSync } from "react-dom";

interface ContentTransitionOptions {
  content: () => HTMLElement | null;
}

interface PendingTransition {
  transition?: ViewTransition;
  controller: AbortController;
  restore: (() => void)[];
  watchdog?: number;
}

let active: PendingTransition | null = null;
/** An animation only advances while the window paints, and a minimised or
 * occluded one paints nothing: the press would sit behind a transition that
 * never starts. Skipping still runs the update, so the change arrives late
 * rather than never. Well past the quarter-second the crossfade takes. */
const STALL_LIMIT = 600;

function clear(request: PendingTransition) {
  window.clearTimeout(request.watchdog);
  request.controller.abort();
  request.restore.forEach((restore) => restore());
  request.restore = [];
  if (active === request) {
    active = null;
    delete document.documentElement.dataset.contentTransition;
  }
}

export function cancelContentTransition() {
  if (!active) return;
  const request = active;
  request.transition?.skipTransition();
  clear(request);
}

function name(element: HTMLElement, value: string, request: PendingTransition) {
  const previous = element.style.viewTransitionName;
  element.style.viewTransitionName = value;
  request.restore.push(() => { element.style.viewTransitionName = previous; });
}

/** Give a fast cache/IPC response time to replace its loading placeholder.
 * Slow requests still reveal their loading state after a bounded wait. */
function waitForContent(content: HTMLElement | null, signal: AbortSignal): Promise<void> {
  if (!content?.querySelector("[data-motion-pending]") || signal.aborted) return Promise.resolve();
  return new Promise((resolve) => {
    const finish = () => {
      observer.disconnect();
      window.clearTimeout(timer);
      signal.removeEventListener("abort", finish);
      resolve();
    };
    const observer = new MutationObserver(() => {
      if (!content.querySelector("[data-motion-pending]")) finish();
    });
    const timer = window.setTimeout(finish, 120);
    observer.observe(content, { childList: true, subtree: true });
    signal.addEventListener("abort", finish, { once: true });
  });
}

/** Animate the changed content, leaving navigation and controls live. */
export function transitionContent(update: () => void, options: ContentTransitionOptions) {
  cancelContentTransition();
  const before = options.content();
  if (!before || !document.startViewTransition || document.visibilityState === "hidden"
    || window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    update();
    return;
  }

  const request: PendingTransition = { controller: new AbortController(), restore: [] };
  const href = window.location.href;
  active = request;
  document.documentElement.dataset.contentTransition = "";
  // Only the region is named. Carrying the cards inside it across was tried and
  // was wrong twice over: between tabs, the first card of one listing and the
  // first of the next are different works, so pairing them joins things that
  // have nothing to do with each other; and both listings use the same grid, so
  // each frame travelled a few pixels and arrived *after* the text had finished
  // fading in - read as the finished screen nudging itself into place.
  name(before, "piep-content", request);
  const transition = document.startViewTransition(async () => {
    // skipTransition still invokes its callback. A newer press, URL, or page
    // takes precedence, even while a lazy route keeps the old page mounted.
    if (active !== request || window.location.href !== href || !before.isConnected) return;
    flushSync(update);
    const after = options.content();
    await waitForContent(after, request.controller.signal);
    if (active !== request) return;
    request.restore.forEach((restore) => restore());
    request.restore = [];
    if (after?.isConnected) name(after, "piep-content", request);
  });
  request.transition = transition;
  request.watchdog = window.setTimeout(() => { if (active === request) transition.skipTransition(); }, STALL_LIMIT);
  void transition.ready.catch(() => undefined);
  void transition.finished.then(() => clear(request), () => clear(request));
}
