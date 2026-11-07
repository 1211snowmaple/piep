import { expect, it, vi } from "vitest";
import type { PcWork } from "./pcApi";

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  convertFileSrc: (path: string) => `http://pcimg.localhost/${path}`,
}));

it("同じ作品番号でも接続を変えたら、WebView の古い画像キャッシュを使わない", async () => {
  const images = await import("./images");
  const work = { id: 1, currentVersion: 1, contentHash: "same" } as PcWork;
  const sources = () => [images.workCoverUrl(work), images.illustrationUrl(1, 2, 720, 1, "same"), images.entityCoverUrl("people", "pixiv", "1")];
  const before = sources();
  expect(sources()).toEqual(before);
  const offline = images.savedCoverUrl(1, 1);
  images.forgetServerImages();
  expect(sources().every((url, index) => url !== before[index])).toBe(true);
  expect(images.savedCoverUrl(1, 1)).toBe(offline);
});

it("アプリを起動し直したときも、以前の接続の HTTP キャッシュを引き継がない", async () => {
  const first = (await import("./images")).entityCoverUrl("series", "pixiv", "1");
  vi.resetModules();
  expect((await import("./images")).entityCoverUrl("series", "pixiv", "1")).not.toBe(first);
});
