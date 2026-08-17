import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  ENTITY_SERIES_LIMIT,
  ENTITY_SERIES_PAGE_SIZE,
  listEntitySeries,
  listEntitySeriesPage,
} from "@/services/shelfApi";

describe("entity-series request bounds", () => {
  beforeEach(() => invoke.mockReset().mockResolvedValue([]));

  it("requests the native maximum by default", async () => {
    await listEntitySeries("pixiv", "author");
    expect(invoke).toHaveBeenCalledWith("db_list_entity_series", {
      source: "pixiv",
      sourceKey: "author",
      limit: ENTITY_SERIES_LIMIT,
    });
  });

  it("clamps oversized, invalid, and non-positive limits before invoke", async () => {
    await listEntitySeries("pixiv", "author", 20_000);
    await listEntitySeries("pixiv", "author", Number.NaN);
    await listEntitySeries("pixiv", "author", 0);

    expect(invoke).toHaveBeenNthCalledWith(1, "db_list_entity_series", expect.objectContaining({ limit: 200 }));
    expect(invoke).toHaveBeenNthCalledWith(2, "db_list_entity_series", expect.objectContaining({ limit: 200 }));
    expect(invoke).toHaveBeenNthCalledWith(3, "db_list_entity_series", expect.objectContaining({ limit: 1 }));
  });

  it("passes an opaque cursor and normalized server query to the paged command", async () => {
    invoke.mockResolvedValue({ items: [], nextCursor: null, total: 0 });
    await listEntitySeriesPage("pixiv", "author", {
      query: "  季節  ",
      cursor: "opaque-cursor",
    });

    expect(invoke).toHaveBeenCalledWith("db_list_entity_series_paged", {
      source: "pixiv",
      sourceKey: "author",
      query: "季節",
      limit: ENTITY_SERIES_PAGE_SIZE,
      cursor: "opaque-cursor",
    });
  });

  it("bounds paged limits and converts a blank query to null", async () => {
    await listEntitySeriesPage("pixiv", "author", { query: "   ", limit: 50_000 });
    expect(invoke).toHaveBeenCalledWith("db_list_entity_series_paged", expect.objectContaining({
      query: null,
      limit: ENTITY_SERIES_LIMIT,
      cursor: null,
    }));
  });
});
