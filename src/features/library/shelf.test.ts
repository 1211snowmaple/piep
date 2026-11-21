import { describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
  invoke: tauri.invoke,
}));

import { encodeShelfCursor, searchShelfEntities, shelfCollectionSummary, shelfEntry, shelfFilter, shelfRequest, ShelfUnsupportedError } from "@/features/library/shelf";
import type { ShelfCard, ShelfCollectionSummary, ShelfEntityRow } from "@/services/shelfApi";

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
    expect(() => shelfRequest({ personKey: "8" })).toThrow(ShelfUnsupportedError);
    try {
      shelfRequest({ personKey: "8" });
    } catch (error) {
      expect((error as ShelfUnsupportedError).conditions).toEqual(["取得元のない作者の指名"]);
    }
  });

  it("opens a numbered page by skipping rows and counts it again", () => {
    const cursor = encodeShelfCursor({ value: 5, work: card.id });
    const { query } = shelfRequest({ offset: 120, cursor });
    expect(query).toMatchObject({ offset: 120, after: null, count: true });
    expect(shelfRequest({}).query.offset).toBe(0);
  });

  it("names an author by the provider's key for the shelf to look up", () => {
    expect(shelfRequest({ personSource: "pixiv", personKey: "8" }).person).toEqual({ provider: "pixiv", key: "8" });
    expect(shelfRequest({}).person).toBeNull();
  });

  it("orders a series by its own order and narrows an author or a series by the shelf's id", () => {
    expect(shelfRequest({ sortBy: "series_order", sortOrder: "asc" }).query).toMatchObject({ sort: "series_order", descending: false });
    const person = shelfRequest({ text: "雨" }, { person: "p-1" });
    expect(person.query.filter).toMatchObject({ person: "p-1", series: null });
    // 棚の ID で絞るときは、検索語に書き方を足さない。
    expect(person.search.text).toBe("雨");
    expect(shelfRequest({}, { series: "s-1" }).query.filter).toMatchObject({ person: null, series: "s-1" });
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

  it("names the author so the card links to the author and shows the author icon", () => {
    const plain = shelfEntry(card);
    expect(plain.authorId).toBe("");
    expect(plain.personIconPath).toBeNull();
    const entry = shelfEntry({ ...card, author_key: "77", authorIconFile: "cd".repeat(32) });
    expect(entry.authorId).toBe("77");
    expect(entry.personIconPath).toBe(`shelf:${"cd".repeat(32)}`);
  });
});

const row: ShelfEntityRow = {
  id: "0195f1c2-0000-7000-8000-0000000000a1",
  provider: "pixiv",
  external_key: "77",
  name: "作者一",
  work_count: 4,
  description: null,
  is_concluded: null,
  icon: null,
  banner: "ef".repeat(32),
  cover: null,
  sample_title: "見本",
  updated_at: null,
  latest_saved_at: Date.UTC(2026, 0, 2),
  latest_published_at: Date.UTC(2026, 0, 1),
  latest_updated_at: Date.UTC(2026, 0, 3),
  total_text_length: 10,
  total_size: 20,
  total_media: 0,
  total_revisions: 1,
  watch: "paused",
};

describe("searchShelfEntities", () => {
  it("narrows the authors by the same work filter as the works tab and keeps the watch state", async () => {
    tauri.invoke.mockResolvedValueOnce({ items: [row], total: 9 });
    const page = await searchShelfEntities("person", "作", 60, 120, { favorite: true, tagsInclude: ["雨"], idsInclude: [3] }, "name", "asc", { watch: "paused", minWorkCount: 2, concluded: null });
    const [command, args] = tauri.invoke.mock.calls[0];
    expect(command).toBe("shelf_entities");
    expect(args).toMatchObject({
      kind: "person",
      query: "作",
      filter: shelfFilter({ favorite: true, tagsInclude: ["雨"] }),
      legacyWorks: [3],
      series: null,
      scope: { watch: "paused", min_work_count: 2, concluded: null },
      sort: "name",
      order: "asc",
      offset: 120,
      limit: 60,
    });
    expect(page.total).toBe(9);
    const [facet] = page.items;
    expect(facet).toMatchObject({ shelf: true, source: "pixiv", sourceKey: "77", displayName: "作者一", count: 4, watch: "paused", sampleTitle: "見本" });
    // アイコンのない作者はヘッダーを顔にする（旧棚と同じ）。
    expect(facet.coverPath).toBe("ef".repeat(32));
    expect(facet.latestDownloadedAt).toBe("2026-01-02T00:00:00.000Z");
  });

  it("names a series by its provider, or by its key alone like the search syntax", async () => {
    tauri.invoke.mockResolvedValue({ items: [], total: 0 });
    await searchShelfEntities("series", null, 60, 0, { seriesSource: "pixiv", seriesKey: "s1" }, "work_count", null, null);
    expect(tauri.invoke.mock.lastCall?.[1]).toMatchObject({ series: { provider: "pixiv", key: "s1" }, scope: null });
    await searchShelfEntities("series", null, 60, 0, { seriesKey: "s1" }, "work_count", null, null);
    expect(tauri.invoke.mock.lastCall?.[1]).toMatchObject({ series: { provider: null, key: "s1" } });
  });
});

describe("shelfCollectionSummary", () => {
  it("opens the collection by its old id and serves its covers by fingerprint", () => {
    const item: ShelfCollectionSummary = {
      id: "0195f1c2-0000-7000-8000-0000000000d1",
      legacy_id: "c1",
      name: "束",
      description: null,
      ordered: false,
      cover: null,
      cover_image: "12".repeat(32),
      cover_mode: "file",
      name_source: "manual",
      track: "manual",
      tiles: [{ provider: "pixiv", external_key: "1", title: "題", author_name: "作者", cover: null }],
      revision: 3,
      member_count: 2,
      available_count: 1,
      total_text_length: 40,
      created_at: Date.UTC(2026, 0, 1),
      updated_at: Date.UTC(2026, 0, 5),
    };
    const summary = shelfCollectionSummary(item);
    expect(summary).toMatchObject({
      shelf: true,
      id: "c1",
      collectionKind: "unordered",
      coverMode: "file",
      coverImagePath: "12".repeat(32),
      memberCount: 2,
      availableCount: 1,
      updatedAt: "2026-01-05T00:00:00.000Z",
    });
    expect(summary.coverTiles[0]).toEqual({ source: "pixiv", sourceId: "1", title: "題", authorName: "作者", coverPath: null });
    // 旧棚に無い（新しい棚で作った）コレクションは新しい棚の ID のまま。
    expect(shelfCollectionSummary({ ...item, legacy_id: null }).id).toBe(item.id);
  });
});

