import { afterEach, describe, expect, it, vi } from "vitest";
import { cancelContentTransition, transitionContent } from "@/lib/contentTransition";

interface FakeTransition {
  skipped: number;
  finish: () => void;
}

/**
 * jsdom has no view transitions. Stand in for one so the naming can be observed.
 *
 * `stall` is the window that has stopped painting: a real transition needs
 * frames to reach its update callback, and one that never paints never gets
 * there on its own.
 */
function installViewTransitions(options: { stall?: boolean } = {}) {
  const transitions: FakeTransition[] = [];
  const start = (callback: () => void | Promise<void>) => {
    let settle = () => undefined as void;
    const finished = new Promise<void>((resolve) => { settle = () => resolve(); });
    let called = false;
    const run = () => called ? Promise.resolve() : (called = true, Promise.resolve(callback()));
    const entry: FakeTransition = { skipped: 0, finish: settle };
    transitions.push(entry);
    return {
      ready: Promise.resolve(),
      // The browser runs the callback a frame later, once the old frame is captured.
      updateCallbackDone: options.stall ? new Promise<void>(() => undefined) : Promise.resolve().then(run),
      finished,
      // Skipping a transition still runs its callback, so the change it carries
      // is not lost along with the animation.
      skipTransition: () => { entry.skipped += 1; void run(); },
    };
  };
  Object.defineProperty(document, "startViewTransition", { configurable: true, writable: true, value: start });
  return transitions;
}

/** Let the update callback and both of its awaits settle. */
const settle = () => new Promise<void>((resolve) => { setTimeout(resolve, 0); });

function region(className = "region") {
  const element = document.createElement("div");
  element.className = className;
  document.body.append(element);
  return element;
}

afterEach(() => {
  cancelContentTransition();
  document.body.innerHTML = "";
  Reflect.deleteProperty(document, "startViewTransition");
});

describe("content transitions", () => {
  it("applies the change straight away where the browser has none", () => {
    const update = vi.fn();
    const content = region();
    transitionContent(update, { content: () => content });
    expect(update).toHaveBeenCalledOnce();
    expect(document.documentElement.dataset.contentTransition).toBeUndefined();
  });

  it("applies the change straight away when motion is turned down", () => {
    installViewTransitions();
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({ ...original(query), matches: query.includes("reduce") })) as typeof window.matchMedia;
    try {
      const update = vi.fn();
      const content = region();
      transitionContent(update, { content: () => content });
      expect(update).toHaveBeenCalledOnce();
      expect(document.documentElement.dataset.contentTransition).toBeUndefined();
    } finally {
      window.matchMedia = original;
    }
  });

  it("does nothing but the update when there is no region to animate", () => {
    const transitions = installViewTransitions();
    const update = vi.fn();
    transitionContent(update, { content: () => null });
    expect(update).toHaveBeenCalledOnce();
    expect(transitions).toHaveLength(0);
  });

  it("hands the one name from the outgoing region to its replacement", async () => {
    const transitions = installViewTransitions();
    const before = region("before");
    const after = region("after");
    let current = before;

    transitionContent(() => { current = after; }, { content: () => current });
    expect(before.style.viewTransitionName).toBe("piep-content");
    expect(document.documentElement.dataset.contentTransition).toBe("");

    await settle();
    // Two elements may never hold one name at the same time, or the browser
    // drops the whole transition.
    expect(before.style.viewTransitionName).toBe("");
    expect(after.style.viewTransitionName).toBe("piep-content");

    transitions[0].finish();
    await settle();
    expect(after.style.viewTransitionName).toBe("");
    expect(document.documentElement.dataset.contentTransition).toBeUndefined();
  });

  it("names the region and nothing inside it", () => {
    installViewTransitions();
    const content = region();
    // Naming the cards too paired unrelated works and left the finished screen
    // creeping the last few pixels after the text had settled.
    const cards = Array.from({ length: 3 }, () => {
      const element = document.createElement("div");
      element.className = "work-card";
      content.append(element);
      return element;
    });

    transitionContent(() => undefined, { content: () => content });

    expect(content.style.viewTransitionName).toBe("piep-content");
    expect(cards.map((item) => item.style.viewTransitionName)).toEqual(["", "", ""]);
  });

  it("keeps the change and drops only the animation when a newer press cuts in", async () => {
    const transitions = installViewTransitions();
    const content = region();
    const update = vi.fn();

    transitionContent(update, { content: () => content });
    expect(content.style.viewTransitionName).toBe("piep-content");

    cancelContentTransition();
    expect(transitions[0].skipped).toBe(1);
    expect(update).toHaveBeenCalledOnce();
    expect(content.style.viewTransitionName).toBe("");
    expect(document.documentElement.dataset.contentTransition).toBeUndefined();

    // The rest of the cancelled callback must not name anything again.
    await settle();
    expect(content.style.viewTransitionName).toBe("");
  });

  it("starts one transition at a time", () => {
    const transitions = installViewTransitions();
    const content = region();
    transitionContent(() => undefined, { content: () => content });
    transitionContent(() => undefined, { content: () => content });
    expect(transitions).toHaveLength(2);
    expect(transitions[0].skipped).toBe(1);
    expect(transitions[1].skipped).toBe(0);
  });

  it("gives up on an animation a window too idle to paint would never finish", async () => {
    vi.useFakeTimers();
    try {
      const transitions = installViewTransitions({ stall: true });
      const content = region();
      const update = vi.fn();

      transitionContent(update, { content: () => content });
      await vi.advanceTimersByTimeAsync(300);
      expect(update).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(400);
      expect(transitions[0].skipped).toBe(1);
      expect(update).toHaveBeenCalledOnce();

      transitions[0].finish();
      await vi.advanceTimersByTimeAsync(200);
      expect(content.style.viewTransitionName).toBe("");
      expect(document.documentElement.dataset.contentTransition).toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });
});
