import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  convertFileSrc: (path: string) => `asset://${path}`,
}));

import {
  deleteDownloads,
  getDownloads,
  optimizeSearchIndex,
  searchEntityFacets,
  searchFilterFacets,
  setFlagsForIds,
} from "@/services/dbApi";

describe("dbApi bulk and facet guards", () => {
  beforeEach(() => invoke.mockReset());

  it("does not cross IPC for empty bulk operations", async () => {
    await expect(deleteDownloads([])).resolves.toEqual({ matchedCount: 0, changedCount: 0 });
    await expect(setFlagsForIds([], { favorite: true })).resolves.toEqual({ matchedCount: 0, changedCount: 0 });
    await expect(getDownloads([])).resolves.toEqual([]);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("deduplicates ids and rejects values that cannot be represented safely", async () => {
    invoke.mockResolvedValueOnce([]);
    await getDownloads([9, 4, 9]);
    expect(invoke).toHaveBeenCalledWith("db_get_downloads", { ids: [9, 4] });
    await expect(getDownloads([Number.NaN])).rejects.toThrow(RangeError);
    await expect(deleteDownloads([0])).rejects.toThrow(RangeError);
  });

  it("trims facet queries and clamps pagination before invoking SQLite", async () => {
    invoke.mockResolvedValue([]);
    await searchFilterFacets("tags", "  uncommon  ", 10_000);
    expect(invoke).toHaveBeenLastCalledWith("db_search_filter_facets", { kind: "tags", query: "uncommon", limit: 200 });

    await searchEntityFacets("person", "  author ", Number.NaN, -50);
    expect(invoke).toHaveBeenLastCalledWith("db_search_entity_facets", { kind: "person", query: "author", limit: 60, offset: 0 });
  });

  it("keeps expensive search index optimization behind its explicit command", async () => {
    invoke.mockResolvedValueOnce({ optimized: false });
    await optimizeSearchIndex();
    expect(invoke).toHaveBeenLastCalledWith("db_optimize_search_index");
  });
});
