import { expect, it } from "vitest";
import { updateWorkMetadataText } from "./UpdateWorkMetadata";

it("shows the author and publication date without duplicating an equivalent update instant", () => {
  const text = updateWorkMetadataText({ authorName: "作者", publishedAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T09:00:00+09:00" });
  expect(text).toContain("作者 · 公開");
  expect(text).not.toContain("更新");
  expect(text).not.toContain("T00:00");
});

it("falls back to the save date, omitting malformed dates", () => {
  expect(updateWorkMetadataText({ publishedAt: "broken", savedAt: "2026-10-02" })).toContain("保存");
  expect(updateWorkMetadataText({ publishedAt: "broken" })).toBe("作品情報未取得");
});
