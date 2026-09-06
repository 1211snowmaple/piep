import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MotionTabs } from "@/components/MotionTabs";

/** jsdom measures everything as zero, and an unmeasurable tab grows no underline. */
function stubRects(box: (element: Element) => Partial<DOMRect> | null) {
  const original = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function (this: Element) {
    const rect = { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, ...box(this) };
    return { ...rect, toJSON: () => rect } as DOMRect;
  };
  return () => { Element.prototype.getBoundingClientRect = original; };
}

const TAB_BOXES: Record<string, Partial<DOMRect>> = {
  作品: { left: 0, width: 80, bottom: 40 },
  作者: { left: 88, width: 104, bottom: 40 },
};

function measuredTabs() {
  return stubRects((element) => {
    if (element.classList.contains("motion-tabs")) return { left: 0, top: 0, width: 400, height: 40, bottom: 40 };
    if (element.getAttribute("role") === "tab") return TAB_BOXES[element.textContent ?? ""] ?? null;
    return { width: 400, height: 40 };
  });
}

function view(value: string, onChange = vi.fn()) {
  const result = render(
    <MantineProvider>
      <MotionTabs value={value} onChange={onChange}>
        <MotionTabs.List>
          <MotionTabs.Tab value="works">作品</MotionTabs.Tab>
          <MotionTabs.Tab value="people">作者</MotionTabs.Tab>
        </MotionTabs.List>
        <MotionTabs.Panel value="works">作品の一覧</MotionTabs.Panel>
        <MotionTabs.Panel value="people">作者の一覧</MotionTabs.Panel>
      </MotionTabs>
    </MantineProvider>,
  );
  return { ...result, onChange };
}

const indicator = (container: HTMLElement) => container.querySelector<HTMLElement>(".motion-tabs__indicator");

afterEach(() => vi.restoreAllMocks());

describe("motion tabs", () => {
  it("keeps the roles and the selected state Mantine gives its tabs", () => {
    view("works");
    expect(screen.getByRole("tab", { name: "作品" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "作者" })).toHaveAttribute("aria-selected", "false");
    expect(screen.getByRole("tabpanel")).toHaveTextContent("作品の一覧");
  });

  it("reports the tab that was pressed", () => {
    const { onChange } = view("works");
    fireEvent.click(screen.getByRole("tab", { name: "作者" }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith("people");
  });

  it("stays quiet when the tab already shown is pressed again", () => {
    const { onChange } = view("works");
    fireEvent.click(screen.getByRole("tab", { name: "作品" }));
    expect(onChange).not.toHaveBeenCalled();
  });

  it("still moves with the arrow keys", () => {
    const { onChange } = view("works");
    fireEvent.keyDown(screen.getByRole("tab", { name: "作品" }), { key: "ArrowRight" });
    expect(onChange).toHaveBeenCalledExactlyOnceWith("people");
  });

  it("draws the underline under the selected tab and moves it when the selection changes", () => {
    const restore = measuredTabs();
    try {
      const { container, rerender, onChange } = view("works");
      const root = container.querySelector(".motion-tabs");
      expect(root).toHaveAttribute("data-indicator-ready");
      expect(indicator(container)).toHaveStyle({ width: "80px", transform: "translate(0px, 38px)" });

      rerender(
        <MantineProvider>
          <MotionTabs value="people" onChange={onChange}>
            <MotionTabs.List>
              <MotionTabs.Tab value="works">作品</MotionTabs.Tab>
              <MotionTabs.Tab value="people">作者</MotionTabs.Tab>
            </MotionTabs.List>
            <MotionTabs.Panel value="works">作品の一覧</MotionTabs.Panel>
            <MotionTabs.Panel value="people">作者の一覧</MotionTabs.Panel>
          </MotionTabs>
        </MantineProvider>,
      );
      expect(indicator(container)).toHaveStyle({ width: "104px", transform: "translate(88px, 38px)" });
    } finally {
      restore();
    }
  });

  it("leaves Mantine's own underline alone where nothing can be measured", () => {
    const { container } = view("works");
    expect(container.querySelector(".motion-tabs")).not.toHaveAttribute("data-indicator-ready");
    expect(indicator(container)).toBeNull();
  });
});
