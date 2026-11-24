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
const subject = "0190a000-0000-7000-8000-000000000004";
const edit = "0190a000-0000-7000-8000-000000000006";

const shelfItem = {
  id: work, snapshotId: snapshot, source: "pixiv", sourceId: "42", title: "題", authorName: "作者", authorId: "7",
  contentType: "novel", tags: ["a"], excerpt: null, textLength: 10, assetCount: 0, hasCover: true, currentVersion: 2,
  edited: true, editId: edit, favorite: false, favoriteGeneration: "3", seriesTitle: null, seriesId: null, sourceCreatedAt: null, savedAt: "2026-11-18T00:00:00.000Z",
};

function status(reachable = true, libraryId = library) {
  return {
    url: "https://server", reachable,
    shelf: {
      scope: { libraryId, subjectId: subject, syncEpoch: "0190a000-0000-7000-8000-000000000005", connectionGeneration: 1 },
      readerBundle: true,
    },
  } as never;
}

beforeEach(() => {
  vi.resetModules();
  api.invoke.mockReset();
});

it("一覧・作品の画面・表紙を棚から引く", async () => {
  const shelf = await import("./shelf");
  const pc = await import("./pcApi");
  const images = await import("./images");
  shelf.adoptShelf(status());
  api.invoke.mockImplementation(async (command: string) => {
    if (command === "pc_search") return { items: [shelfItem], nextCursor: "next", totalEstimate: 1, indexComplete: true };
    if (command === "pc_work") return { work: shelfItem, people: [], series: [], versions: [], editBaseVersion: 1 };
    if (command === "offline_save_shelf_work" || command === "offline_list_works") {
      const saved = { work: shelfItem, readingScope: { libraryId: library, subjectId: subject }, savedAtMs: 1, pageCount: 3 };
      return command === "offline_list_works" ? [saved] : saved;
    }
    if (command === "pc_work_collections") return [];
    throw new Error(command);
  });

  const page = await pc.searchPcWorks({ ...pc.defaultPcSearch, text: "題", sort: "saved:desc" });
  expect(api.invoke.mock.calls[0][0]).toBe("pc_search");
  expect(api.invoke.mock.calls[0][1]).toMatchObject({ library: { libraryId: library }, cursor: null, scope: null });
  const [item] = page.items;
  expect(item.id).toBe(work);
  expect(images.workCoverUrl(item)?.startsWith(`http://pcimg.localhost/shelf/${library}/${work}/${snapshot}?connection=`)).toBe(true);
  expect(images.workCoverUrl(item, true)?.startsWith(`http://pcimg.localhost/shelf/${library}/${work}/${snapshot}/large?connection=`)).toBe(true);

  const metadata = await pc.getPcMetadata(item.id);
  expect(api.invoke.mock.calls[1]).toEqual(["pc_work", { library: expect.objectContaining({ libraryId: library }), work }]);
  expect(metadata.work.edited).toBe(true);
  expect(metadata.work).toMatchObject({ readingScope: { libraryId: library, subjectId: subject }, snapshotId: snapshot, editId: edit });
  expect(item).toMatchObject({ snapshotId: snapshot, edited: true, editId: edit, favoriteGeneration: "3" });

  // 端末への保存は、今つながっている棚から組む。返った保存版は一覧の一件と同じ形になる。
  const saved = await pc.saveOfflineWork(item.id);
  expect(api.invoke.mock.calls[2]).toEqual(["offline_save_shelf_work", { library: expect.objectContaining({ libraryId: library }), work }]);
  const { favoriteReadSequence: _read, ...listedItem } = item;
  expect(saved.work).toEqual(listedItem);
  expect(pc.sameSavedVersion(item, saved)).toBe(true);
  expect(pc.sameSavedVersion({ ...item, editId: null }, saved)).toBe(false);
  expect(pc.sameSavedVersion({ ...item, snapshotId: work }, saved)).toBe(false);
  // 同じ接続先で別の棚を配っても、保存済み作品には保存時の棚と読み手を使う。
  shelf.adoptShelf(status(true, work));
  const listed = await pc.listOfflineWorks();
  expect(listed[0].work).toEqual(listedItem);

  await expect(pc.getPcWorkCollections(item.id)).resolves.toEqual([]);
  expect(api.invoke).toHaveBeenCalledTimes(5);
  expect(shelf.connectedShelf()).toMatchObject({ libraryId: work });
});

it("棚に届かない接続では、一覧を引かずに知らせる", async () => {
  const shelf = await import("./shelf");
  const pc = await import("./pcApi");
  shelf.adoptShelf(status(false));
  await expect(pc.searchPcWorks({ ...pc.defaultPcSearch, sort: "saved:desc" })).rejects.toMatchObject({ code: "library_changed" });
  await expect(pc.saveOfflineWork(work)).rejects.toMatchObject({ code: "library_changed" });
  expect(api.invoke).not.toHaveBeenCalled();
  expect(shelf.connectedShelfKey()).toBeNull();
});
