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

const shelfItem = {
  id: work, snapshotId: snapshot, source: "pixiv", sourceId: "42", title: "題", authorName: "作者", authorId: "7",
  contentType: "novel", tags: ["a"], excerpt: null, textLength: 10, assetCount: 0, hasCover: true, currentVersion: 2,
  edited: true, favorite: false, seriesTitle: null, seriesId: null, sourceCreatedAt: null, downloadedAt: "2026-11-18T00:00:00.000Z",
};

function status(worksSearch: boolean) {
  return {
    url: "https://server", reachable: true,
    nativeLibrary: {
      scope: { libraryId: library, subjectId: "0190a000-0000-7000-8000-000000000004", syncEpoch: "0190a000-0000-7000-8000-000000000005", connectionGeneration: 1 },
      worksList: true, worksSearch, readerBundle: true,
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
  expect(metadata.activeEditHash).toBe(`edit:${snapshot}`);

  // 旧 API の番号でしか指せない操作は、送らずに断る。
  await expect(pc.saveOfflineWork(item.id)).rejects.toMatchObject({ code: "shelf_not_yet" });
  await expect(pc.getPcWorkCollections(item.id)).resolves.toEqual([]);
  expect(api.invoke).toHaveBeenCalledTimes(2);
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
