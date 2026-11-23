import { expect, it } from "vitest";
import { sortOfflineWorks } from "./librarySort";
import type { OfflineWork, PcWork } from "./pcApi";

function saved(id: string, sourceCreatedAt: string | null, savedAt: string, savedAtMs: number): OfflineWork {
  const work: PcWork = {
    id, source: "pixiv", sourceId: id, title: id, authorName: "作者", authorId: "1", contentType: "novel",
    tags: [], excerpt: null, textLength: 100, assetCount: 0, hasCover: false, favorite: false, seriesTitle: null, seriesId: null,
    currentVersion: 1, snapshotId: "s1", edited: false, editId: null, favoriteGeneration: "0", sourceCreatedAt, savedAt,
  };
  return { work, savedAtMs, shelfLibrary: "l1", savedVersion: 1, savedEditHash: null, pageCount: 1 };
}

it("公開日が欠けた作品は、棚の保存日を使って公開日順に並べる", () => {
  const older = saved("1", "2026-01-01T00:00:00Z", "2026-06-01T00:00:00Z", Date.UTC(2026, 9, 1));
  const missing = saved("2", null, "2026-02-01T00:00:00Z", Date.UTC(2026, 9, 2));
  const newer = saved("3", "2026-03-01T00:00:00Z", "2026-08-01T00:00:00Z", Date.UTC(2026, 9, 3));
  const works = [newer, older, missing];
  expect(sortOfflineWorks(works, "published:asc")).toEqual([older, missing, newer]);
  expect(sortOfflineWorks(works, "published:desc")).toEqual([newer, missing, older]);
});
