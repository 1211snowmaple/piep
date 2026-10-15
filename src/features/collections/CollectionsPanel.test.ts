import { describe, expect, it } from "vitest";
import type { WorkCollectionSummary } from "@/types/collections";
import { sortCollections } from "./CollectionsPanel";

function collection(
  id: string,
  values: Partial<WorkCollectionSummary>,
): WorkCollectionSummary {
  return {
    id,
    name: id,
    description: null,
    collectionKind: "unordered",
    coverDownloadId: null,
    coverPath: null,
    coverMode: "mosaic",
    coverImagePath: null,
    coverTiles: [],
    nameSource: "manual",
    revision: 1,
    memberCount: 1,
    availableCount: 1,
    totalTextLength: 1,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...values,
  };
}

describe("collection ordering", () => {
  const items = [
    collection("small", {
      name: "あ",
      revision: 1,
      memberCount: 2,
      availableCount: 1,
      totalTextLength: 100,
      createdAt: "2025-01-01T00:00:00Z",
      updatedAt: "2026-03-01T00:00:00Z",
    }),
    collection("large", {
      name: "ん",
      revision: 8,
      memberCount: 9,
      availableCount: 7,
      totalTextLength: 50_000,
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-02-01T00:00:00Z",
    }),
  ];

  it.each([
    ["updated_at", "desc", "small"],
    ["member_count", "asc", "small"],
    ["available_count", "desc", "large"],
    ["text_length", "desc", "large"],
    ["revision", "asc", "small"],
    ["name", "desc", "large"],
  ] as const)("sorts %s %s across the whole collection list", (sortBy, order, first) => {
    expect(sortCollections(items, sortBy, order)[0].id).toBe(first);
  });
});
