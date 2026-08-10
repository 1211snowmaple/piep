import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_PAGE_SIZE, PAGE_SIZE_OPTIONS, pageWindow, usePagingMode } from "@/components/ListPager";

beforeEach(() => window.localStorage.clear());

describe("page window", () => {
  it("shows every page when they all fit", () => {
    expect(pageWindow(1, 5, 10)).toEqual([1, 2, 3, 4, 5]);
    expect(pageWindow(3, 3, 7)).toEqual([1, 2, 3]);
    expect(pageWindow(1, 1, 7)).toEqual([1]);
  });

  it("uses the width it is given rather than a fixed few", () => {
    // A wide window showing the same three pages as a narrow one wastes the
    // space and makes a long library tedious to move through.
    const narrow = pageWindow(20, 38, 5).filter((entry) => entry !== "gap");
    const wide = pageWindow(20, 38, 15).filter((entry) => entry !== "gap");
    expect(wide.length).toBeGreaterThan(narrow.length);
    expect(wide.length).toBeLessThanOrEqual(15);
  });

  it("always keeps the first and last page reachable", () => {
    for (const current of [1, 2, 19, 37, 38]) {
      const window = pageWindow(current, 38, 9);
      expect(window[0]).toBe(1);
      expect(window[window.length - 1]).toBe(38);
      expect(window).toContain(current);
    }
  });

  it("centres the run on where the reader is", () => {
    const middle = pageWindow(20, 38, 9).filter((entry): entry is number => entry !== "gap");
    expect(Math.min(...middle.filter((page) => page > 1))).toBeLessThan(20);
    expect(Math.max(...middle.filter((page) => page < 38))).toBeGreaterThan(20);
  });

  it("does not run off either end", () => {
    for (const current of [1, 2, 3, 36, 37, 38]) {
      const window = pageWindow(current, 38, 9);
      const numbers = window.filter((entry): entry is number => entry !== "gap");
      expect(Math.min(...numbers)).toBe(1);
      expect(Math.max(...numbers)).toBe(38);
      expect(new Set(numbers).size).toBe(numbers.length);
      expect(numbers).toEqual([...numbers].sort((a, b) => a - b));
    }
  });

  it("never offers a page that does not exist, at any width", () => {
    for (const last of [1, 2, 7, 8, 38, 500]) {
      for (const slots of [5, 7, 11, 21]) {
        for (const current of [1, Math.ceil(last / 2), last]) {
          const window = pageWindow(current, last, slots);
          const numbers = window.filter((entry): entry is number => entry !== "gap");
          expect(new Set(numbers).size, `${last}/${slots}/${current}`).toBe(numbers.length);
          expect(Math.max(...numbers), `${last}/${slots}/${current}`).toBeLessThanOrEqual(last);
          expect(Math.min(...numbers), `${last}/${slots}/${current}`).toBeGreaterThanOrEqual(1);
          expect(numbers).toEqual([...numbers].sort((a, b) => a - b));
        }
      }
    }
  });

  it("keeps a usable minimum even when told there is no room", () => {
    const window = pageWindow(20, 38, 0);
    expect(window.filter((entry) => entry !== "gap").length).toBeGreaterThanOrEqual(3);
  });
});

describe("page size", () => {
  it("defaults to a page that fits on a screen", () => {
    // Sixty rows a page meant scrolling through a page to reach its pager.
    expect(DEFAULT_PAGE_SIZE).toBe(20);
    expect(PAGE_SIZE_OPTIONS).toContain(DEFAULT_PAGE_SIZE);
  });

  it("offers sizes in increasing order", () => {
    expect([...PAGE_SIZE_OPTIONS]).toEqual([...PAGE_SIZE_OPTIONS].sort((a, b) => a - b));
  });
});

describe("paging mode", () => {
  it("keeps anyone left on the removed third mode on a working one", () => {
    // "manual" was scrolling with the scrolling taken out; the button it left
    // behind is still there in "auto", so there was nothing to keep.
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("manual"));
    const { result } = renderHook(() => usePagingMode());
    expect(result.current[0]).toBe("auto");
  });

  it("remembers the choice that remains", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    expect(renderHook(() => usePagingMode()).result.current[0]).toBe("pages");
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("nonsense"));
    expect(renderHook(() => usePagingMode()).result.current[0]).toBe("auto");
  });
});
