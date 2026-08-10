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

});
