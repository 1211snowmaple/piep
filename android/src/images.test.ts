import { expect, it, vi } from "vitest";
import type { PcWork } from "./pcApi";

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  convertFileSrc: (path: string) => `http://pcimg.localhost/${path}`,
}));

const library = "0190a000-0000-7000-8000-000000000001";
const id = "0190a000-0000-7000-8000-000000000002";

async function load() {
  const shelf = await import("./shelf");
  shelf.adoptShelf({
    url: "https://server", reachable: true,
    shelf: {
      scope: { libraryId: library, subjectId: "0190a000-0000-7000-8000-000000000004", syncEpoch: "0190a000-0000-7000-8000-000000000005", connectionGeneration: 1 },
      readerBundle: true,
    },
  } as never);
  return import("./images");
}

it("同じ作品でも接続を変えたら、WebView の古い画像キャッシュを使わない", async () => {
  const images = await load();
  const work = { id, currentVersion: 1, snapshotId: "0190a000-0000-7000-8000-000000000003", edited: false } as unknown as PcWork;
  const sources = () => [images.workCoverUrl(work), images.entityCoverUrl("people", "pixiv", "1")];
  const before = sources();
  expect(before.every((url) => url !== null)).toBe(true);
  expect(sources()).toEqual(before);
  const offline = images.savedCoverUrl(id, 1);
  images.forgetServerImages();
  expect(sources().every((url, index) => url !== before[index])).toBe(true);
  expect(images.savedCoverUrl(id, 1)).toBe(offline);
});

it("アプリを起動し直したときも、以前の接続の HTTP キャッシュを引き継がない", async () => {
  const first = (await load()).entityCoverUrl("series", "pixiv", "1");
  expect(first).not.toBeNull();
  vi.resetModules();
  expect((await load()).entityCoverUrl("series", "pixiv", "1")).not.toBe(first);
});
