import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { demoWorks } from "@/mocks/demoData";

vi.mock("@/components/WorkCard", () => ({
  WorkCard: ({ work }: { work: { id: number } }) => <div data-testid="virtual-work">{work.id}</div>,
}));

import { libraryColumnCount, VirtualizedWorkList } from "@/features/library/VirtualizedWorkList";

describe("VirtualizedWorkList", () => {
  it("matches the gallery's responsive minimum column width", () => {
    expect(libraryColumnCount(359, "gallery")).toBe(1);
    expect(libraryColumnCount(736, "gallery")).toBe(2);
    expect(libraryColumnCount(1500, "gallery")).toBe(4);
    expect(libraryColumnCount(1500, "compact")).toBe(1);
  });

  it("bounds the initial DOM while the AppFrame scroll viewport is discovered", () => {
    const works = Array.from({ length: 10_000 }, (_, index) => ({ ...demoWorks[0], id: index + 1 }));
    const view = render(<VirtualizedWorkList items={works} view="gallery" />);

    expect(screen.getAllByTestId("virtual-work")).toHaveLength(120);
    expect(view.container.querySelector("[data-virtualization-pending]")).toBeInTheDocument();
  });

  /**
   * 仮想化そのものを通す。
   *
   * jsdom には配置が無く `clientWidth` は 0 のままなので、これまでのテストは
   * **どれも「まだ仮想化していない」側の分岐しか見ていなかった**。行の鍵、
   * `scrollMargin`、実際に描く行数といった仮想化の本体は一度も走っていない。
   * 採寸に関わるものだけを本物らしく差し替えて、本体側へ入れる。
   */
  const withLayout = (width: number, height: number) => {
    const scroller = document.createElement("div");
    scroller.className = "app-main";
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: height },
      clientWidth: { configurable: true, value: width },
      // 仮想化が viewport の大きさを読むのは `offsetWidth`/`offsetHeight` で、
      // jsdom はどちらも 0 を返す。ここを埋めないと「描く行は無い」になる。
      offsetHeight: { configurable: true, value: height },
      offsetWidth: { configurable: true, value: width },
      scrollTop: { configurable: true, writable: true, value: 0 },
    });
    scroller.getBoundingClientRect = () => ({
      top: 0, left: 0, right: width, bottom: height, width, height, x: 0, y: 0, toJSON: () => ({}),
    });
    document.body.append(scroller);
    // 一覧の箱は、差し込まれたあとに幅を持つ。
    const clientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => width });
    return {
      scroller,
      restore: () => {
        if (clientWidth) Object.defineProperty(HTMLElement.prototype, "clientWidth", clientWidth);
        scroller.remove();
      },
    };
  };

  it("draws a window over the list instead of every row", () => {
    const layout = withLayout(1500, 800);
    try {
      const works = Array.from({ length: 10_000 }, (_, index) => ({ ...demoWorks[0], id: index + 1 }));
      const view = render(<VirtualizedWorkList items={works} view="gallery" />, { container: layout.scroller });

      // 仮想化側の分岐に入っている。
      expect(view.container.querySelector("[data-virtualized-work-list]")).toBeInTheDocument();
      expect(view.container.querySelector("[data-virtualization-pending]")).toBeNull();

      // 1500px は4列。10,000件のうち、見えているぶんの近くだけを描く。
      const drawn = screen.getAllByTestId("virtual-work");
      expect(drawn.length).toBeGreaterThan(0);
      expect(drawn.length).toBeLessThan(600);
      expect(drawn).toHaveLength(new Set(drawn.map((node) => node.textContent)).size);
    } finally {
      layout.restore();
    }
  });

  /**
   * 列数が変わると行の中身も変わる。鍵に列数が入っていないと、React は
   * 幅を変える前の行を使い回して、別の作品が並んだままになる。
   */
  it("keys rows by the column count so a width change rebuilds them", () => {
    const wide = withLayout(1500, 800);
    let wideFirstRow: string;
    try {
      const works = Array.from({ length: 40 }, (_, index) => ({ ...demoWorks[0], id: index + 1 }));
      render(<VirtualizedWorkList items={works} view="gallery" />, { container: wide.scroller });
      wideFirstRow = screen.getAllByTestId("virtual-work").slice(0, 4).map((node) => node.textContent).join(",");
      expect(wideFirstRow).toBe("1,2,3,4");
    } finally {
      wide.restore();
    }

    const narrow = withLayout(736, 800);
    try {
      const works = Array.from({ length: 40 }, (_, index) => ({ ...demoWorks[0], id: index + 1 }));
      render(<VirtualizedWorkList items={works} view="gallery" />, { container: narrow.scroller });
      const narrowFirstRow = screen.getAllByTestId("virtual-work").slice(0, 2).map((node) => node.textContent).join(",");
      expect(narrowFirstRow).toBe("1,2");
    } finally {
      narrow.restore();
    }
  });

  it("renders nothing at all for an empty list", () => {
    const layout = withLayout(1500, 800);
    try {
      render(<VirtualizedWorkList items={[]} view="gallery" />, { container: layout.scroller });
      expect(screen.queryAllByTestId("virtual-work")).toHaveLength(0);
    } finally {
      layout.restore();
    }
  });

  it("renders exactly one row for a single item", () => {
    const layout = withLayout(1500, 800);
    try {
      render(<VirtualizedWorkList items={[demoWorks[0]]} view="gallery" />, { container: layout.scroller });
      expect(screen.getAllByTestId("virtual-work")).toHaveLength(1);
    } finally {
      layout.restore();
    }
  });

});
