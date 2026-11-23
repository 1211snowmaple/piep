import { expect, it } from "vitest";
import { publishedSortTime } from "./workSort";

it("公開日を優先し、欠損時は保存日を使う", () => {
  const savedAt = "2026-02-01T00:00:00Z";
  expect(publishedSortTime("2026-01-01T09:00:00+09:00", savedAt)).toBe(Date.UTC(2026, 0, 1));
  expect(publishedSortTime(null, savedAt)).toBe(Date.UTC(2026, 1, 1));
  expect(publishedSortTime("不明", savedAt)).toBe(Date.UTC(2026, 1, 1));
  expect(publishedSortTime("1970-01-01T00:00:00Z", savedAt)).toBe(0);
});
