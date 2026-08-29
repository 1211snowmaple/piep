import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { EntityFacet } from "@/types/library";

vi.mock("@/components/EntityCard", () => ({
  EntityCard: ({ entity }: { entity: EntityFacet }) => <div data-testid="virtual-entity">{entity.sourceKey}</div>,
}));

import { entityGridColumnCount, VirtualizedEntityGrid } from "@/features/library/VirtualizedEntityGrid";

function entity(index: number): EntityFacet {
  return {
    source: "pixiv",
    sourceKey: String(index),
    displayName: `作者 ${index}`,
    count: 1,
    coverPath: null,
    description: null,
    updatedAt: null,
    latestDownloadedAt: null,
    sampleTitle: null,
    iconPath: null,
    bannerPath: null,
  } as EntityFacet;
}

describe("VirtualizedEntityGrid", () => {
  it("uses responsive columns without ever returning zero", () => {
    expect(entityGridColumnCount(0)).toBe(1);
    expect(entityGridColumnCount(655)).toBe(1);
    expect(entityGridColumnCount(656)).toBe(2);
    expect(entityGridColumnCount(1000)).toBe(3);
  });

  it("bounds the initial DOM while the AppFrame viewport is discovered", () => {
    const view = render(<VirtualizedEntityGrid items={Array.from({ length: 10_000 }, (_, index) => entity(index))} kind="person" />);

    expect(screen.getAllByTestId("virtual-entity")).toHaveLength(90);
    expect(view.container.querySelector("[data-entity-virtualization-pending]")).toBeInTheDocument();
  });

  /**
   * 仮想化そのものを通す。jsdom には配置が無く、幅も `offsetHeight` も 0 の
   * ままなので、これまでのテストは**「まだ仮想化していない」側の分岐しか
   * 見ていなかった**。採寸に関わるものだけを差し替えて本体側へ入れる。
   */
  const withLayout = (width: number, height: number) => {
    const scroller = document.createElement("div");
    scroller.className = "app-main";
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: height },
      clientWidth: { configurable: true, value: width },
      // 仮想化が viewport の大きさを読むのはこの二つ。jsdom は 0 を返す。
      offsetHeight: { configurable: true, value: height },
      offsetWidth: { configurable: true, value: width },
      scrollTop: { configurable: true, writable: true, value: 0 },
    });
    scroller.getBoundingClientRect = () => ({
      top: 0, left: 0, right: width, bottom: height, width, height, x: 0, y: 0, toJSON: () => ({}),
    });
    document.body.append(scroller);
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

  it("draws a window over the grid instead of every card", () => {
    const layout = withLayout(1000, 800);
    try {
      const items = Array.from({ length: 10_000 }, (_, index) => entity(index));
      const view = render(<VirtualizedEntityGrid items={items} kind="person" />, { container: layout.scroller });

      expect(view.container.querySelector("[data-virtualized-entity-grid]")).toBeInTheDocument();
      expect(view.container.querySelector("[data-entity-virtualization-pending]")).toBeNull();

      const drawn = screen.getAllByTestId("virtual-entity");
      expect(drawn.length).toBeGreaterThan(0);
      expect(drawn.length).toBeLessThan(600);
      // 同じ相手を二度描かない。列数が鍵に入っていないと、幅を変えたときに
      // 前の行が使い回されて別の顔ぶれが並ぶ。
      expect(drawn).toHaveLength(new Set(drawn.map((node) => node.textContent)).size);
      expect(drawn.slice(0, 3).map((node) => node.textContent)).toEqual(["0", "1", "2"]);
    } finally {
      layout.restore();
    }
  });

  it("renders nothing at all for an empty grid", () => {
    const layout = withLayout(1000, 800);
    try {
      render(<VirtualizedEntityGrid items={[]} kind="person" />, { container: layout.scroller });
      expect(screen.queryAllByTestId("virtual-entity")).toHaveLength(0);
    } finally {
      layout.restore();
    }
  });

  it("renders exactly one card for a single entity", () => {
    const layout = withLayout(1000, 800);
    try {
      render(<VirtualizedEntityGrid items={[entity(7)]} kind="person" />, { container: layout.scroller });
      expect(screen.getAllByTestId("virtual-entity")).toHaveLength(1);
    } finally {
      layout.restore();
    }
  });
});
