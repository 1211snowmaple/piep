import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
  invoke: vi.fn(),
}));

import { encodeShelfCursor, shelfEntry, shelfRequest, ShelfUnsupportedError } from "@/features/library/shelfV2";
import type { ShelfCard } from "@/services/libraryV2Api";

const card: ShelfCard = {
  id: "0195f1c2-0000-7000-8000-000000000001",
  snapshot_id: "0195f1c2-0000-7000-8000-000000000002",
  edit_id: null,
  provider: "pixiv",
  external_key: "12345",
  content_type: "novel",
  title: "題",
  author_name: "作者",
  saved_at: Date.UTC(2026, 0, 2, 3, 4, 5),
  published_at: Date.UTC(2025, 11, 31),
  updated_at: null,
  text_length: 1200,
  logical_size: 4096,
  media_count: 2,
  version_number: 3,
  favorite: true,
  watching: false,
  revised: true,
  image_count: 2,
  file_count: 0,
  excerpt: "あらすじ",
  tags: ["a", "b"],
  cover: "ab".repeat(32),
  series_title: "シリーズ",
  series_position: 4,
  coverFile: "ab".repeat(32),
  legacyId: null,
};

describe("shelfRequest", () => {
  it("maps the library's conditions onto the new shelf's query", () => {
    const { search, query } = shelfRequest({
      text: "雨",
      source: "pixiv",
      favorite: true,
      watchFilter: "unwatched",
      tagsInclude: ["x", " "],
      tagFilterMode: "or",
      tagsExclude: ["y"],
      authorsInclude: ["A"],
      minCharCount: 100,
      maxFileSizeBytes: 2048,
      assetFilter: "has_images",
      seriesFilter: "standalone",
      editFilter: "edited",
      coverFilter: "no_cover",
      revisionFilter: "pending_revision",
      acquisitionFilter: "attention",
      dateField: "source_created_at",
      dateFrom: "2026-01-01",
      dateTo: "2026-01-31",
      sortBy: "text_length",
      sortOrder: "asc",
      limit: 40,
      searchMode: "semantic",
      versionScope: "current",
    });
    expect(search).toEqual({ text: "雨", versions: "current", mode: "meaning" });
    expect(query.sort).toBe("length");
    expect(query.descending).toBe(false);
    expect(query.limit).toBe(40);
    expect(query.after).toBeNull();
    expect(query.count).toBe(true);
    expect(query.filter).toMatchObject({
      provider: "pixiv",
      favorite: true,
      watching: false,
      edited: true,
      has_cover: false,
      in_series: false,
      media: "images",
      revision: "pending_revision",
      acquisition: "attention",
      text_length: { min: 100, max: null },
      logical_size: { min: null, max: 2048 },
      tags: ["x"],
      any_tag: true,
      excluded_tags: ["y"],
      authors: ["A"],
      // 旧棚と同じく UTC の日で、終わりの日を含む。
      date: { field: "published", from: Date.UTC(2026, 0, 1), until: Date.UTC(2026, 1, 1) },
    });
  });

  it("names a series by the provider's key through the query syntax", () => {
    expect(shelfRequest({ text: "雨", seriesSource: "pixiv", seriesKey: "77" }).search.text).toBe("雨 series:pixiv:77");
    expect(shelfRequest({ seriesKey: "77" }).search.text).toBe("series:77");
  });

  it("continues from the cursor of the previous page without recounting", () => {
    const cursor = encodeShelfCursor({ value: 5, work: card.id });
    const { query } = shelfRequest({ cursor });
    expect(query.after).toEqual({ value: 5, work: card.id });
    expect(query.count).toBe(false);
  });

  it("refuses conditions the new shelf cannot answer instead of dropping them", () => {
    expect(() => shelfRequest({ offset: 60, sortBy: "series_order" })).toThrow(ShelfUnsupportedError);
    try {
      shelfRequest({ offset: 60, sortBy: "series_order" });
    } catch (error) {
      expect((error as ShelfUnsupportedError).conditions).toEqual(["ページ番号", "シリーズ順"]);
    }
  });

  it("passes the reading shelf as the old shelf's numbers, which reading positions are kept under", () => {
    expect(shelfRequest({ idsInclude: [3, 1] }).legacyWorks).toEqual([3, 1]);
    expect(shelfRequest({}).legacyWorks).toBeNull();
  });
});

describe("shelfEntry", () => {
  it("keeps the new shelf's id and serves the cover by its fingerprint", () => {
    const entry = shelfEntry(card, {
      work: card.id, score: 2.5, matched_version: 2, historical_matches: 3, semantic: null,
    }, {
      work: card.id, match_fields: ["body"], score_reasons: [], highlights: [], field_score: 1,
    });
    expect(entry.id).toBe(card.id);
    expect(entry.coverUrl).toBe(`http://piep-library.localhost/${"ab".repeat(32)}`);
    expect(entry.coverPath).toBeNull();
    expect(entry.source).toBe("pixiv");
    expect(entry.currentVersion).toBe(3);
    expect(entry.assetCount).toBe(2);
    expect(entry.sourceCreatedAt).toBe("2025-12-31T00:00:00.000Z");
    expect(entry.sourceUpdatedAt).toBeNull();
    expect(entry.matchFields).toEqual(["body"]);
    expect(entry.matchedVersion).toBe(2);
    expect(entry.historicalMatchCount).toBe(3);
  });
});
