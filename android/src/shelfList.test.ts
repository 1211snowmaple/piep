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
const edit = "0190a000-0000-7000-8000-000000000006";

const shelfItem = {
  id: work, snapshotId: snapshot, source: "pixiv", sourceId: "42", title: "題", authorName: "作者", authorId: "7",
  contentType: "novel", tags: ["a"], excerpt: null, textLength: 10, assetCount: 0, hasCover: true, currentVersion: 2,
  edited: true, editId: edit, favorite: false, favoriteGeneration: "3", seriesTitle: null, seriesId: null, sourceCreatedAt: null, downloadedAt: "2026-11-18T00:00:00.000Z",
};

function status(worksSearch: boolean, favoriteWrite = false) {
  return {
    url: "https://server", reachable: true,
    nativeLibrary: {
      scope: { libraryId: library, subjectId: "0190a000-0000-7000-8000-000000000004", syncEpoch: "0190a000-0000-7000-8000-000000000005", connectionGeneration: 1 },
      worksList: true, worksSearch, favoriteWrite, readerBundle: true,
    },
  } as never;
}

beforeEach(() => {
  vi.resetModules();
  api.invoke.mockReset();
});

it("新しい棚の一覧を出せる接続では、一覧・作品の画面・表紙を新しい棚から引く", async () => {
  const native = await import("./nativeReading");
  const pc = await import("./pcApi");
  const images = await import("./images");
  native.adoptNativeLibrary(status(true));
  api.invoke.mockImplementation(async (command: string) => {
    if (command === "pc_native_search") return { items: [shelfItem], nextCursor: "next", totalEstimate: 1, indexComplete: true };
    if (command === "pc_native_work") return { work: shelfItem, people: [], series: [], versions: [], editBaseVersion: 1 };
    if (command === "offline_save_shelf_work" || command === "offline_list_works") {
      const saved = { work: shelfItem, shelfLibrary: library, savedAtMs: 1, savedVersion: 2, savedContentHash: null, savedEditHash: `edit:${edit}`, pageCount: 3 };
      return command === "offline_list_works" ? [saved, { ...saved, work: { ...shelfItem, id: 9, sourceId: "9" } }] : saved;
    }
    throw new Error(command);
  });

  const page = await pc.searchPcWorks({ ...pc.defaultPcSearch, text: "題", sort: "dateDesc" });
  expect(api.invoke.mock.calls[0][0]).toBe("pc_native_search");
  expect(api.invoke.mock.calls[0][1]).toMatchObject({ library: { libraryId: library }, cursor: null, scope: null });
  const [item] = page.items;
  expect(item.id).toBe(work);
  expect(item.contentHash).toBeNull();
  expect(pc.isShelfWork(item)).toBe(true);
  expect(images.workCoverUrl(item)?.startsWith(`http://pcimg.localhost/shelf/${library}/${work}/${snapshot}?connection=`)).toBe(true);
  expect(images.workCoverUrl(item, true)?.startsWith(`http://pcimg.localhost/shelf/${library}/${work}/${snapshot}/large?connection=`)).toBe(true);

  const metadata = await pc.getPcMetadata(item.id);
  expect(api.invoke.mock.calls[1]).toEqual(["pc_native_work", { library: expect.objectContaining({ libraryId: library }), work }]);
  expect(metadata.isEdited).toBe(true);
  // 編集の UUID で読書位置と保存版を分ける（端末に保存した版の印と同じ形）。
  expect(metadata.activeEditHash).toBe(`edit:${edit}`);
  expect(item.shelf).toEqual({ snapshotId: snapshot, edited: true, editId: edit, favoriteGeneration: "3" });

  // 端末への保存は、今つながっている棚から組む。返った保存版は一覧の一件と同じ形になる。
  const saved = await pc.saveOfflineWork(item.id);
  expect(api.invoke.mock.calls[2]).toEqual(["offline_save_shelf_work", { library: expect.objectContaining({ libraryId: library }), work }]);
  const { favoriteReadSequence: _read, ...listedItem } = item;
  expect(saved.work).toEqual(listedItem);
  expect(pc.sameSavedVersion(item, saved)).toBe(true);
  expect(pc.sameSavedVersion({ ...item, shelf: { ...item.shelf!, editId: null } }, saved)).toBe(false);
  expect(pc.sameSavedVersion({ ...item, shelf: { ...item.shelf!, snapshotId: work } }, saved)).toBe(false);
  const listed = await pc.listOfflineWorks();
  expect(listed[0].work).toEqual(listedItem);
  // 旧 API の作品の保存版は、そのままの形で返る。
  expect(listed[1].work.id).toBe(9);
  expect(pc.isShelfWork(listed[1].work)).toBe(false);

  await expect(pc.getPcWorkCollections(item.id)).resolves.toEqual([]);
  expect(api.invoke).toHaveBeenCalledTimes(4);
  expect(native.favoriteShelf()).toBeNull();
  native.adoptNativeLibrary(status(true, true));
  expect(native.favoriteShelf()).toMatchObject({ libraryId: library });
});

it("新しい棚の一覧を出せない接続では、旧 API の一覧のまま", async () => {
  const native = await import("./nativeReading");
  const pc = await import("./pcApi");
  native.adoptNativeLibrary(status(false));
  api.invoke.mockResolvedValue({ items: [], nextCursor: null, totalEstimate: 0, indexComplete: true });
  await pc.searchPcWorks({ ...pc.defaultPcSearch, sort: "dateDesc" });
  expect(api.invoke.mock.calls[0][0]).toBe("pc_search_works");
  expect(native.nativeShelfKey()).toBeNull();
});
