import { MantineProvider } from "@mantine/core";
import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { CollectionCover } from "@/components/CollectionCover";
import type { CollectionCoverTile, WorkCollectionSummary } from "@/types/collections";

function tile(index: number, coverPath: string | null): CollectionCoverTile {
  return {
    source: "pixiv",
    sourceId: String(100 + index),
    title: `作品${index}`,
    authorName: "作者",
    coverPath,
  };
}

function summary(overrides: Partial<WorkCollectionSummary> = {}): WorkCollectionSummary {
  return {
    id: "collection-1",
    name: "雨の記憶",
    description: null,
    collectionKind: "ordered",
    coverDownloadId: null,
    coverPath: null,
    coverMode: "mosaic",
    coverImagePath: null,
    coverTiles: [],
    nameSource: "manual",
    revision: 1,
    memberCount: 0,
    availableCount: 0,
    totalTextLength: 0,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function renderCover(collection: WorkCollectionSummary) {
  return render(
    <MantineProvider>
      <CollectionCover collection={collection} />
    </MantineProvider>,
  );
}

describe("CollectionCover", () => {
  it("falls back to a mark when there is nothing to show", () => {
    const { container } = renderCover(summary());
    expect(container.querySelector(".collection-cover--sigil")).not.toBeNull();
    // 紋は束ごとに決まる。同じ束は何度描いても同じ形になる。
    const first = container.querySelectorAll(".collection-cover__sigil i[data-on]").length;
    const again = renderCover(summary());
    expect(again.container.querySelectorAll(".collection-cover__sigil i[data-on]").length).toBe(first);
  });

  it("tiles four covers and keeps the seat of a member that has none", () => {
    const { container } = renderCover(
      summary({ coverTiles: [tile(1, "a.jpg"), tile(2, null), tile(3, "c.jpg"), tile(4, "d.jpg")], memberCount: 4 }),
    );
    const cover = container.querySelector(".collection-cover--mosaic");
    expect(cover?.getAttribute("data-tiles")).toBe("4");
    // 表紙の無いマスも席を残す。詰めると4作の束が2作の束と同じ顔になる。
    expect(container.querySelectorAll(".collection-cover__slot").length).toBe(4);
  });

  it("shows every available cover when the collection is too small for four tiles", () => {
    const { container } = renderCover(summary({ coverTiles: [tile(1, "a.jpg"), tile(2, "b.jpg")], memberCount: 2 }));
    expect(container.querySelector(".collection-cover--mosaic")?.getAttribute("data-tiles")).toBe("2");
    expect(container.querySelectorAll(".collection-cover__slot")).toHaveLength(2);
  });

  it("falls back rather than showing an empty frame when a chosen image is missing", () => {
    const { container } = renderCover(summary({ coverMode: "file", coverImagePath: null, coverTiles: [tile(1, "a.jpg")] }));
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".collection-cover--mosaic")).not.toBeNull();

    const single = renderCover(summary({ coverMode: "single", coverPath: null, coverTiles: [] }));
    expect(single.container.querySelector(".collection-cover--sigil")).not.toBeNull();
  });

  it("falls back to the collection mark when a managed image cannot be loaded", () => {
    const { container, getByRole } = renderCover(
      summary({ coverMode: "file", coverImagePath: "data:image/png;base64,broken", coverTiles: [] }),
    );
    fireEvent.error(getByRole("img", { name: "雨の記憶の表紙" }));
    expect(container.querySelector(".collection-cover--sigil")).not.toBeNull();
  });

  it("stacks spines only when there is more than one to stack", () => {
    const { container } = renderCover(
      summary({ coverMode: "spine", coverTiles: [tile(1, "a.jpg"), tile(2, "b.jpg"), tile(3, "c.jpg")] }),
    );
    expect(container.querySelector(".collection-cover--spine")).not.toBeNull();
    expect(container.querySelectorAll(".collection-cover__slot").length).toBe(3);

    const lonely = renderCover(summary({ coverMode: "spine", coverTiles: [tile(1, "a.jpg")] }));
    expect(lonely.container.querySelector(".collection-cover--spine")).toBeNull();
  });
});
