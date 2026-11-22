import { beforeEach, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: api.invoke,
  isTauri: () => true,
  convertFileSrc: (path: string) => `http://pcimg.localhost/${path}`,
}));

const library = "0190a000-0000-7000-8000-000000000001";
const work = "0190a000-0000-7000-8000-000000000002";
const snapshot = "0190a000-0000-7000-8000-000000000003";
const collection = "0190a000-0000-7000-8000-000000000007";

const shelfItem = {
  id: work, snapshotId: snapshot, source: "pixiv", sourceId: "42", title: "題", authorName: "作者", authorId: "7",
  contentType: "novel", tags: ["a"], excerpt: null, textLength: 10, assetCount: 0, hasCover: true, currentVersion: 2,
  edited: false, editId: null, favorite: true, favoriteGeneration: "1", seriesTitle: null, seriesId: null, sourceCreatedAt: null, savedAt: "2026-11-18T00:00:00.000Z",
};

const summary = {
  id: collection, name: "まとめ", description: null, collectionKind: "ordered", memberCount: 2, availableCount: 1,
  coverWorkId: null, coverMode: "single", coverRevision: 4, hasSingleCover: true, hasFileCover: false, coverTiles: [],
};

function status(reachable = true) {
  return {
    url: "https://server", reachable,
    nativeLibrary: {
      scope: { libraryId: library, subjectId: "0190a000-0000-7000-8000-000000000004", syncEpoch: "0190a000-0000-7000-8000-000000000005", connectionGeneration: 1 },
      readerBundle: true,
    },
  } as never;
}

const last = () => api.invoke.mock.calls[api.invoke.mock.calls.length - 1];

beforeEach(() => {
  vi.resetModules();
  api.invoke.mockReset();
});

it("作者・シリーズ・コレクションの画面と画像を棚から引く", async () => {
  const native = await import("./nativeReading");
  const pc = await import("./pcApi");
  const images = await import("./images");
  native.adoptNativeLibrary(status());
  api.invoke.mockImplementation(async (command: string) => {
    if (command === "pc_native_entities") return { items: [], nextOffset: null, total: 0 };
    if (command === "pc_native_entity_detail") return { source: "pixiv", sourceKey: "7", displayName: "作者", description: null, workCount: 1, hasCover: true, hasBanner: false, isConcluded: null, publishedCount: null, links: [], tags: [], seriesCount: 0, collections: [summary] };
    if (command === "pc_native_entity_series") return { items: [], nextCursor: "30", total: 31 };
    if (command === "pc_native_collections" || command === "pc_native_work_collections") return [{ ...summary, position: command === "pc_native_work_collections" ? 0 : null }];
    if (command === "pc_native_collection") return { ...summary, members: [{ title: "題", authorName: "作者", position: 0, work: shelfItem }, { title: "無い回", authorName: "作者", position: 1, work: null }] };
    throw new Error(command);
  });

  await pc.listPcEntities("people", "名", 30, { sort: "name", order: "asc" });
  expect(last()).toEqual(["pc_native_entities", { library: expect.objectContaining({ libraryId: library }), kind: "people", text: "名", offset: 30, sort: "name", order: "asc" }]);
  const detail = await pc.getPcEntityDetail("people", "pixiv", "7");
  expect(detail.collections[0].shelfLibrary).toBe(library);
  const series = await pc.listPcEntitySeries("pixiv", "7", "", "30");
  expect(series.nextCursor).toBe("30");
  expect(last()?.[1]).toMatchObject({ source: "pixiv", key: "7", cursor: "30" });
  const listed = await pc.listPcCollections();
  expect(listed[0].shelfLibrary).toBe(library);
  // 作品の入ったコレクションも出る。
  const memberships = await pc.getPcWorkCollections(work);
  expect(memberships[0].position).toBe(0);
  expect(last()).toEqual(["pc_native_work_collections", { library: expect.objectContaining({ libraryId: library }), work }]);
  const opened = await pc.getPcCollection(collection);
  expect(opened.members[0].work?.id).toBe(work);
  expect(opened.members[0].work?.shelf?.snapshotId).toBe(snapshot);
  expect(opened.members[1].work).toBeNull();

  // 画像も棚から。単独の表紙は作品の番号ではなく `single` で引く。
  expect(images.entityCoverUrl("people", "pixiv", "a b", true)?.startsWith(`http://pcimg.localhost/shelf-entity/${library}/people/pixiv/a%20b/banner?connection=`)).toBe(true);
  expect(images.collectionCoverUrl(opened, "single", 4)?.startsWith(`http://pcimg.localhost/shelf-collection/${library}/${collection}/single/4?connection=`)).toBe(true);
  expect(native.nativeShelfKey()).toContain(library);
});

it("棚に届かない接続では、画面も画像も出さない", async () => {
  const native = await import("./nativeReading");
  const pc = await import("./pcApi");
  const images = await import("./images");
  native.adoptNativeLibrary(status(false));
  await expect(pc.listPcCollections()).rejects.toMatchObject({ code: "library_changed" });
  expect(await pc.getPcWorkCollections(work)).toEqual([]);
  expect(api.invoke).not.toHaveBeenCalled();
  expect(images.entityCoverUrl("series", "pixiv", "9")).toBeNull();
  expect(images.collectionCoverUrl({ id: "other" }, "tile-0", 1)).toBeNull();
  expect(native.nativeShelfKey()).toBeNull();
});
