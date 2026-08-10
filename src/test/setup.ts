import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// Node 26 exposes an unusable `localStorage` placeholder unless a backing file
// is configured. jsdom tests need a deterministic, isolated implementation.
if (!window.localStorage) {
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
