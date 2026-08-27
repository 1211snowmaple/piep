import { MantineProvider } from "@mantine/core";
import { act, render, renderHook, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_PAGE_SIZE,
  ListPager,
  MAX_NUMBERED_OFFSET,
  PAGE_SIZE_OPTIONS,
  maxDirectNumberedPage,
  normalizeNumberedPage,
  pageWindow,
  usePagingMode,
} from "@/components/ListPager";

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

describe("bounded numbered pages", () => {
  it("keeps direct jumps inside one OFFSET budget at every page size", () => {
    expect(maxDirectNumberedPage(20)).toBe(251);
    expect(maxDirectNumberedPage(100)).toBe(51);
    expect((maxDirectNumberedPage(20) - 1) * 20).toBe(MAX_NUMBERED_OFFSET);
    expect((maxDirectNumberedPage(100) - 1) * 100).toBe(MAX_NUMBERED_OFFSET);
  });

  it("strictly canonicalizes malformed and oversized URL pages", () => {
    expect(normalizeNumberedPage("12oops", 20)).toMatchObject({ page: 1, exceededLimit: false, urlValue: null });
    expect(normalizeNumberedPage("0", 20)).toMatchObject({ page: 1, exceededLimit: false, urlValue: null });
    expect(normalizeNumberedPage("999999", 20)).toMatchObject({ page: 251, exceededLimit: true, urlValue: "251" });
    expect(normalizeNumberedPage("9".repeat(100), 20)).toMatchObject({ page: 251, exceededLimit: true, urlValue: "251" });
  });
});

describe("paging mode", () => {
  it("keeps anyone left on the removed third mode on a working one", () => {
    // "manual" was scrolling with the scrolling taken out; the button it left
    // behind is still there in "auto", so there was nothing to keep.
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("manual"));
    const { result } = renderHook(() => usePagingMode("library-works"));
    expect(result.current[0]).toBe("auto");
  });

  it("remembers the choice that remains", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    expect(renderHook(() => usePagingMode("library-works")).result.current[0]).toBe("pages");
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("nonsense"));
    expect(renderHook(() => usePagingMode("library-works")).result.current[0]).toBe("auto");
  });

  /**
   * 一覧ごとに覚える。全体の既定は「まだ自分で決めていない一覧」のためにある。
   *
   * ここを取り違えると、束の中で番号にした人が棚まで番号になったり、逆に
   * 設定でまとめて変えたのに個別に決めた一覧まで巻き込んだりする。
   */
  it("個別に決めていない一覧は、全体の既定に従う", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    expect(renderHook(() => usePagingMode("collection-members")).result.current[0]).toBe("pages");
  });

  it("個別に決めた一覧は、全体の既定より自分の設定を採る", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    window.localStorage.setItem("piep.paging-mode.collection-members", JSON.stringify("auto"));
    expect(renderHook(() => usePagingMode("collection-members")).result.current[0]).toBe("auto");
    // 隣の一覧は巻き込まれない。
    expect(renderHook(() => usePagingMode("library-works")).result.current[0]).toBe("pages");
  });

  it("ボタンで変えると、その一覧の設定として残る", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("auto"));
    const { result } = renderHook(() => usePagingMode("library-collections"));
    act(() => result.current[1]("pages"));

    expect(JSON.parse(window.localStorage.getItem("piep.paging-mode.library-collections") ?? "null")).toBe("pages");
    // 全体の既定は動かさない。押した一覧の話でしかない。
    expect(JSON.parse(window.localStorage.getItem("piep.paging-mode") ?? "null")).toBe("auto");
  });
});

describe("the pager itself", () => {
  function renderPager(mode: "auto" | "pages") {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify(mode));
    return render(
      <MantineProvider>
        <ListPager
          scope="library-works"
          hasNext
          loading={false}
          loaded={20}
          total={2237}
          onLoad={vi.fn()}
          pages={{ current: 3, size: 20, onGoTo: vi.fn() }}
        />
      </MantineProvider>,
    );
  }

  it("does not repeat the mode switch under the listing", () => {
    // It already sits beside the count at the top of every listing. A second
    // copy at the bottom is one more thing between the reader and the page
    // numbers, and in scrolling mode the bottom is a moving target anyway.
    renderPager("pages");
    expect(screen.queryByRole("radiogroup", { name: "一覧の読み込み方" })).toBeNull();
    expect(screen.getByRole("button", { name: "次へ" })).toBeInTheDocument();
  });

  it("keeps the mode switch out of the scrolling pager too", () => {
    const { container } = renderPager("auto");
    expect(screen.queryByRole("radiogroup", { name: "一覧の読み込み方" })).toBeNull();
    expect(screen.getByRole("button", { name: /さらに読み込む/ })).toBeInTheDocument();
    // The element that decides when the next batch is fetched has to occupy
    // area: a zero-sized one is not reliably reported as intersecting, and the
    // listing would then simply stop loading as you scrolled.
    const sentinel = container.querySelector("[aria-hidden]") as HTMLElement | null;
    expect(sentinel?.style.height).toBe("1px");
    expect(sentinel?.style.width).toBe("100%");
  });

  it("marks where the reader is, for people who cannot see the fill", () => {
    renderPager("pages");
    expect(screen.getByRole("button", { name: "3ページ目" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("2,237件中 3 / 112ページ")).toBeInTheDocument();
  });

  it("explains a safety stop without claiming the whole result was displayed", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("auto"));
    render(
      <MantineProvider>
        <ListPager scope="library-works" hasNext={false} loading={false} loaded={500} total={10_000} onLoad={vi.fn()} endMessage="ページ番号に切り替えると続きへ移動できます。" />
      </MantineProvider>,
    );
    expect(screen.getByText("ページ番号に切り替えると続きへ移動できます。")).toBeInTheDocument();
    expect(screen.queryByText(/すべて表示しました/)).toBeNull();
  });

  it("does not offer a last-page button that would exceed the OFFSET budget", () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    render(
      <MantineProvider>
        <ListPager
          scope="library-works"
          hasNext
          loading={false}
          loaded={20}
          total={1_000_000}
          onLoad={vi.fn()}
          pages={{ current: 1, size: 20, maxDirectPage: 251, onGoTo: vi.fn() }}
        />
      </MantineProvider>,
    );

    expect(screen.getByRole("button", { name: "251ページ目" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "50000ページ目" })).toBeNull();
    expect(screen.getByText(/ページ番号で直接移動できるのは251ページ目まで/)).toBeInTheDocument();
  });
});

describe("a listing that cannot say how long it is", () => {
  function renderOpenEnded(hasNext: boolean, current = 1) {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    return render(
      <MantineProvider>
        <ListPager
          scope="library-works"
          hasNext={hasNext}
          loading={false}
          loaded={20}
          total={null}
          onLoad={vi.fn()}
          pages={{ current, size: 20, onGoTo: vi.fn() }}
        />
      </MantineProvider>,
    );
  }

  it("still offers page numbers", () => {
    // Authors and series are counted by a query nobody wants to run twice, so
    // the total is unknown. Refusing numbers on that basis meant the preference
    // was silently ignored on those two tabs, with nothing said about it.
    renderOpenEnded(true);
    expect(screen.getByRole("button", { name: "1ページ目" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /さらに読み込む/ })).toBeNull();
  });

  it("lets the reader move on while there is more", () => {
    renderOpenEnded(true);
    // Treating the current page as the last one stranded the reader on page one.
    expect(screen.getByRole("button", { name: "次へ" })).toBeEnabled();
    expect(screen.getByText("1ページ目")).toBeInTheDocument();
  });

  it("says so once the end is reached", () => {
    renderOpenEnded(false, 3);
    expect(screen.getByRole("button", { name: "次へ" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "前へ" })).toBeEnabled();
    expect(screen.getByText("3ページ目（最後）")).toBeInTheDocument();
  });
});
