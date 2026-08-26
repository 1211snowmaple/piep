import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { holdRegionInPlace, scrollRegionIntoView, scrollViewportToTop } from "@/lib/scroll";

/**
 * jsdom lays nothing out, so the geometry is supplied: a viewport 800 tall that
 * is currently scrolled 4000 down, holding a region whose top is 600 below the
 * viewport's own.
 */
function mountRegion() {
  const viewport = document.createElement("div");
  viewport.className = "app-main";
  const region = document.createElement("div");
  viewport.append(region);
  document.body.append(viewport);
  const scrolled: number[] = [];
  Object.defineProperty(viewport, "scrollTop", {
    configurable: true,
    get: () => scrolled.length ? scrolled[scrolled.length - 1] : 4000,
    set: (value: number) => scrolled.push(value),
  });
  viewport.scrollTo = ((options: ScrollToOptions) => { viewport.scrollTop = options.top ?? 0; }) as typeof viewport.scrollTo;
  viewport.getBoundingClientRect = () => ({ top: 0 }) as DOMRect;
  region.getBoundingClientRect = () => ({ top: 600 }) as DOMRect;
  return { region, scrolled };
}

/** The viewport height the clamp below is measured against. */
const CLIENT_HEIGHT = 800;

/**
 * The same geometry, but with the browser's own behaviour modelled: a document
 * that can change height, and a scroll position clamped to whatever height it
 * currently has. This is what a tab switch does to the page.
 */
function mountCollapsingRegion() {
  const viewport = document.createElement("div");
  viewport.className = "app-main";
  const region = document.createElement("div");
  viewport.append(region);
  document.body.append(viewport);
  const state = { top: 4000, height: 12000 };
  const clamp = () => { state.top = Math.max(0, Math.min(state.top, state.height - CLIENT_HEIGHT)); };
  Object.defineProperty(viewport, "scrollHeight", { configurable: true, get: () => state.height });
  Object.defineProperty(viewport, "scrollTop", {
    configurable: true,
    get: () => state.top,
    set: (value: number) => { state.top = value; clamp(); },
  });
  viewport.scrollTo = ((options: ScrollToOptions) => { viewport.scrollTop = options.top ?? 0; }) as typeof viewport.scrollTo;
  viewport.getBoundingClientRect = () => ({ top: 0 }) as DOMRect;
  // The region sits 4600 down the document, so at a scroll of 4000 its top is
  // the same 600 below the viewport's as in mountRegion.
  region.getBoundingClientRect = () => ({ top: 4600 - state.top }) as DOMRect;
  /** Replacing the panel below the region with one of a different height. */
  const resizeTo = (height: number) => { state.height = height; clamp(); };
  return { region, state, resizeTo };
}

describe("moving the page in response to a press", () => {
  // performance is faked alongside the frames because holding an offset gives up
  // on a clock rather than on a frame count.
  beforeEach(() => { vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame", "performance"] }); });
  afterEach(() => { vi.useRealTimers(); document.body.innerHTML = ""; });

  it("brings the top of a region under the header", () => {
    const { region, scrolled } = mountRegion();
    scrollRegionIntoView(region);
    // 4000 down + 600 to the region's top, less the gap that keeps it clear of
    // the header. This is what the next page of an author's works lands on:
    // their tabs, rather than the top of the profile they have already read.
    expect(scrolled[scrolled.length - 1]).toBe(4588);
  });

  it("goes to the very top when that is what was asked for", () => {
    const { region, scrolled } = mountRegion();
    scrollViewportToTop(region);
    expect(scrolled[scrolled.length - 1]).toBe(0);
  });

  it("puts the reader back once a tab's new panel has filled out", () => {
    const { region, state, resizeTo } = mountCollapsingRegion();
    holdRegionInPlace(region);
    // The panel that was open unmounts before the one being opened has its
    // rows, so the page is briefly a quarter of its height and the browser
    // clamps the offset to fit. This is the jump to the top the reader sees.
    // It then sits on a loading state, perfectly still and far longer than a
    // settled page would be: height alone cannot tell the two apart.
    resizeTo(3000);
    vi.advanceTimersByTime(600);
    expect(state.top).toBe(2200);
    // The rows land, the page is tall enough to hold the offset again, and the
    // reader is standing where they were before the press.
    resizeTo(9000);
    vi.advanceTimersByTime(32);
    expect(state.top).toBe(4000);
  });

  it("leaves a page that never collapses exactly where it was", () => {
    const { region, state } = mountCollapsingRegion();
    holdRegionInPlace(region);
    vi.advanceTimersByTime(600);
    expect(state.top).toBe(4000);
  });

  it("does nothing when there is no region and no viewport to move", () => {
    const orphan = document.createElement("div");
    expect(() => scrollRegionIntoView(orphan)).not.toThrow();
    expect(() => scrollRegionIntoView(null)).not.toThrow();
    expect(() => scrollViewportToTop(null)).not.toThrow();
    expect(() => holdRegionInPlace(orphan)).not.toThrow();
    expect(() => holdRegionInPlace(null)).not.toThrow();
  });
});
