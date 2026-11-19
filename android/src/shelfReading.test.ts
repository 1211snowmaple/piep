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

const document = { key: "k", entityId: work, view: { kind: "snapshot" as const, id: snapshot }, edited: false, pageCount: 3, sourcePageStarts: [0], totalPlainTextChars: 5, outline: [] };

function status(reading: boolean) {
  return {
    url: "https://server", reachable: true,
    nativeLibrary: {
      scope: { libraryId: library, subjectId: "0190a000-0000-7000-8000-000000000004", syncEpoch: "0190a000-0000-7000-8000-000000000005", connectionGeneration: 1 },
      worksList: true, worksSearch: true, favoriteWrite: true, groups: true, suggest: true, reading, readerBundle: true,
    },
  } as never;
}

const remote = (patch: object) => ({ work, location: { viewKind: "snapshot", viewId: snapshot, page: 2, ratioPpm: 400_000 }, finished: false, mine: false, updatedAt: 2_000, generation: "3", ...patch });

beforeEach(() => {
  vi.resetModules();
  api.invoke.mockReset();
  api.invoke.mockResolvedValue(null);
  window.localStorage.clear();
});

it("棚と受け渡せる接続では、位置を棚の作品と開いた版に結んで置き、同期で棚とも受け渡す", async () => {
  const native = await import("./nativeReading");
  const sync = await import("./readingSync");
  native.adoptNativeLibrary(status(true));
  const shelf = sync.shelfReadingFor(document);
  expect(shelf).toBe(document);
  await sync.recordShelfReading(work, sync.shelfLocation(document, 1, 0.25), false);
  expect(api.invoke).toHaveBeenLastCalledWith("pc_native_reading_record", {
    scope: expect.objectContaining({ libraryId: library }), work,
    location: { viewKind: "snapshot", viewId: snapshot, page: 1, ratioPpm: 250_000 }, finished: false,
  });
  await sync.sendReading();
  expect(api.invoke.mock.calls.map(([command]) => command)).toEqual(["pc_native_reading_record", "reading_sync", "pc_native_reading_sync"]);
  await sync.readShelfPosition(work);
  expect(api.invoke).toHaveBeenLastCalledWith("pc_native_reading_get", { scope: expect.objectContaining({ libraryId: library }), work });
});

it("受け渡せない接続や同期を切ったときは、棚へ書かず旧 API の同期のまま", async () => {
  const native = await import("./nativeReading");
  const sync = await import("./readingSync");
  native.adoptNativeLibrary(status(false));
  expect(sync.shelfReadingFor(document)).toBeNull();
  await sync.recordShelfReading(work, sync.shelfLocation(document, 0, 0), false);
  await sync.sendReading();
  expect(api.invoke.mock.calls.map(([command]) => command)).toEqual(["reading_sync"]);
  native.adoptNativeLibrary(status(true));
  sync.setReadingSyncEnabled(false);
  expect(sync.shelfReadingFor(document)).toBeNull();
});

it("棚の位置は手元より新しいほかの端末のものだけ使い、別の版の位置はページの目安にする", async () => {
  const sync = await import("./readingSync");
  expect(sync.shelfStart(remote({}), 1_000, document)).toEqual({ page: 2, ratio: 0.4, guide: false, deviceTime: 2_000 });
  // 手元のほうが新しい・消した位置・ページで表せない位置は使わない。
  expect(sync.shelfStart(remote({}), 3_000, document)).toBeNull();
  expect(sync.shelfStart(remote({ location: null }), 0, document)).toBeNull();
  expect(sync.shelfStart(null, 0, document)).toBeNull();
  // この端末が書いた位置は、手元に記録が無いときだけ（入れ直したあと）。
  expect(sync.shelfStart(remote({ mine: true }), 1_000, document)).toBeNull();
  expect(sync.shelfStart(remote({ mine: true }), 0, document)?.page).toBe(2);
  // 別の版の写しで読んでいた位置は、割合を捨てて案内だけ。
  expect(sync.shelfStart(remote({ location: { viewKind: "snapshot", viewId: older, page: 1, ratioPpm: 900_000 } }), 0, document)).toEqual({ page: 1, ratio: 0, guide: true, deviceTime: 2_000 });
  expect(sync.shelfLocation(document, 0, 1.7).ratioPpm).toBe(1_000_000);
});
