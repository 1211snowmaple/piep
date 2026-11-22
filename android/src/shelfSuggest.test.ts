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
const older = "0190a000-0000-7000-8000-000000000006";

const shelfItem = {
  id: work, snapshotId: snapshot, source: "pixiv", sourceId: "42", title: "題", authorName: "作者", authorId: "7",
  contentType: "novel", tags: [], excerpt: null, textLength: 10, assetCount: 2, hasCover: true, currentVersion: 2,
  edited: false, editId: null, favorite: false, favoriteGeneration: "1", seriesTitle: null, seriesId: null, sourceCreatedAt: null, savedAt: "2026-11-18T00:00:00.000Z",
};

const opened = { key: "k", entityId: work, view: { kind: "snapshot", id: older }, edited: false, pageCount: 1, sourcePageStarts: [0], totalPlainTextChars: 5, outline: [] };

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

it("候補と以前の版を出せる接続では、候補・作品の引き当て・以前の版を棚から", async () => {
  const native = await import("./nativeReading");
  const pc = await import("./pcApi");
  native.adoptNativeLibrary(status());
  api.invoke.mockImplementation(async (command: string) => {
    if (command === "pc_native_suggest") return { items: [{ kind: "title", label: "題", value: "42", count: 1, exactMatch: true, source: "pixiv", sourceKey: work }] };
    if (command === "pc_native_lookup_work") return shelfItem;
    if (command === "pc_native_work") return {
      work: shelfItem, people: [], series: [], editBaseVersion: null,
      versions: [{ snapshotId: older, version: 1, textLength: 8, savedAt: "2026-11-01T00:00:00.000Z" }, { snapshotId: snapshot, version: 2, textLength: null, savedAt: "2026-11-18T00:00:00.000Z" }],
    };
    if (command === "pc_native_open_snapshot") return opened;
    throw new Error(command);
  });

  const items = await pc.searchPcSuggestions("題");
  expect(items[0].sourceKey).toBe(work);
  expect(last()).toEqual(["pc_native_suggest", { library: expect.objectContaining({ libraryId: library }), text: "題" }]);
  const found = await pc.lookupPcWork("pixiv", "42");
  expect(found?.id).toBe(work);
  expect(last()).toEqual(["pc_native_lookup_work", { library: expect.objectContaining({ libraryId: library }), provider: "pixiv", key: "42" }]);

  // 版の一覧は写しの ID つき。今の版だけ挿絵の数が分かる。
  const metadata = await pc.getPcMetadata(work);
  expect(metadata.versions.map((v) => [v.version, v.snapshotId, v.textLength, v.assetCount])).toEqual([[1, older, 8, 0], [2, snapshot, 0, 2]]);
  expect(await native.openNativeSnapshot(work, older)).toEqual(opened);
  expect(last()).toEqual(["pc_native_open_snapshot", { scope: expect.objectContaining({ libraryId: library }), work, snapshot: older }]);
  // 二度目は組んだものを使い、開き直すときだけ取り直す。
  await native.openNativeSnapshot(work, older);
  expect(api.invoke.mock.calls.filter(([command]) => command === "pc_native_open_snapshot")).toHaveLength(1);
  await native.openNativeSnapshot(work, older, true);
  expect(api.invoke.mock.calls.filter(([command]) => command === "pc_native_open_snapshot")).toHaveLength(2);
});

it("棚に届かない接続では、候補を出さず以前の版も開かない", async () => {
  const native = await import("./nativeReading");
  const pc = await import("./pcApi");
  native.adoptNativeLibrary(status(false));
  expect(await pc.searchPcSuggestions("題")).toEqual([]);
  expect(await pc.lookupPcWork("pixiv", "42")).toBeNull();
  expect(await native.openNativeSnapshot(work, older)).toBeNull();
  expect(api.invoke).not.toHaveBeenCalled();
});
