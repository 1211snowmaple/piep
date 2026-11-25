import "@testing-library/jest-dom/vitest";
import { cleanup, configure } from "@testing-library/react";
import { afterEach } from "vitest";

// One storage implementation on every Node version, installed unconditionally.
//
// Two different ones used to be in play: Node 26 exposes an unusable
// `localStorage` placeholder unless a backing file is configured, so this shim
// took over locally, while on the LTS that CI runs jsdom's own Storage was left
// in place. That one is a Proxy, and `vi.spyOn` cannot attach to it - the spy
// silently records nothing - so tests that assert on writes passed here and
// failed there. A plain object behaves the same everywhere and can be spied on.
{
  let values = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      get length() { return values.size; },
      clear: () => { values = new Map(); },
      getItem: (key: string) => values.get(String(key)) ?? null,
      key: (index: number) => [...values.keys()][index] ?? null,
      removeItem: (key: string) => { values.delete(String(key)); },
      setItem: (key: string, value: string) => { values.set(String(key), String(value)); },
    } satisfies Storage,
  });
}

afterEach(() => cleanup());

// `findBy*` と `waitFor` の既定の待ちは 1 秒。手元の空いた PC なら足りるが、CI の実行役が
// 同じ PC で何本も走ると、画面を描くだけで 2〜3 秒かかり、表示されるべきものが出る前に
// 「見つからない」で落ちた（AUD-024）。負荷をかけた手元の実行でも同じ形で 4〜5 件落ちる。
// ここで見ているのは「出るべきものが出るか」で、速さではないので、待つ上限を広げる。
// 本当に出ない不具合は、5 秒後に同じように落ちる。
configure({ asyncUtilTimeout: 5_000 });

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => undefined,
    removeListener: () => undefined,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    dispatchEvent: () => false,
  }),
});

// jsdom implements no scrolling at all, so the app shell's "return to the top
// on navigation" effect throws instead of being a no-op.
if (!Element.prototype.scrollTo) {
  Object.defineProperty(Element.prototype, "scrollTo", { writable: true, value: () => undefined });
}

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

Object.defineProperty(globalThis, "ResizeObserver", { value: ResizeObserverMock, writable: true });

Object.defineProperty(window, "visualViewport", {
  configurable: true,
  value: {
    width: 1200,
    height: 800,
    offsetLeft: 0,
    offsetTop: 0,
    pageLeft: 0,
    pageTop: 0,
    scale: 1,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  },
});

Object.defineProperty(document, "fonts", {
  configurable: true,
  value: {
    ready: Promise.resolve(),
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  },
});
