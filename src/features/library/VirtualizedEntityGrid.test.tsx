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
});
