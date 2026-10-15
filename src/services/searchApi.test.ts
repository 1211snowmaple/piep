import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  convertFileSrc: (path: string) => `asset://${path}`,
}));

import { searchSuggest, setSemanticSearchEnabled, startSearchRebuildIndex } from "@/services/searchApi";

describe("searchApi input normalization", () => {
  beforeEach(() => invoke.mockReset());

  it("does not invoke the backend for an empty suggestion query", async () => {
    await expect(searchSuggest({ text: "   ", limit: 8 })).resolves.toEqual({ items: [] });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("bounds suggestion and rebuild batch sizes", async () => {
    invoke.mockResolvedValueOnce({ items: [] });
    await searchSuggest({ text: "  tag  ", limit: 5_000 });
    expect(invoke).toHaveBeenLastCalledWith("search_suggest", { params: { text: "tag", limit: 50 } });

    invoke.mockResolvedValueOnce("job-id");
    await startSearchRebuildIndex({ batchSize: -4 });
    expect(invoke).toHaveBeenLastCalledWith("search_rebuild_index", { jobOptions: { batchSize: 8 } });

    invoke.mockResolvedValueOnce("job-id");
    await startSearchRebuildIndex({ batchSize: 100_000, includeSemantic: true });
    expect(invoke).toHaveBeenLastCalledWith("search_rebuild_index", { jobOptions: { batchSize: 512, includeSemantic: true } });
  });

  it("preserves the persistent semantic policy when no override is supplied", async () => {
    invoke.mockResolvedValueOnce("job-id");
    await startSearchRebuildIndex();
    expect(invoke).toHaveBeenLastCalledWith("search_rebuild_index", { jobOptions: { batchSize: 64 } });
  });

  it("updates the persistent semantic capability explicitly", async () => {
    invoke.mockResolvedValueOnce({ semanticEnabled: true });
    await setSemanticSearchEnabled(true);
    expect(invoke).toHaveBeenLastCalledWith("search_set_semantic_enabled", { enabled: true });
  });
});
