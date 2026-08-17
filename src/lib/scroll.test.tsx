import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { scrollRegionIntoView, scrollViewportToTop } from "@/lib/scroll";

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

describe("moving the page in response to a press", () => {
  beforeEach(() => { vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] }); });
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

  it("does nothing when there is no region and no viewport to move", () => {
    const orphan = document.createElement("div");
    expect(() => scrollRegionIntoView(orphan)).not.toThrow();
    expect(() => scrollRegionIntoView(null)).not.toThrow();
    expect(() => scrollViewportToTop(null)).not.toThrow();
  });
});
