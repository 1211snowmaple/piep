import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { invalidateAfterUpdateJob } from "./updateJobs";

/**
 * 更新確認が見つけた事実は、それを読む画面とは別の場所にある。
 *
 * 改稿の印はカードと作品ページ、件数はサイドバーの棚。どれも自分では
 * 「確認が終わった」ことを知らないので、変えた側から知らせるしかない。
 * 知らせ先が一つでも抜けると、確認が終わったのに前の答えを出しつづける -
 * しかもエラーは出ないので、気づくまでに時間がかかる。
 */
describe("更新確認のあとに古くなるもの", () => {
  const collectInvalidatedKeys = () => {
    const client = new QueryClient();
    const keys: unknown[][] = [];
    vi.spyOn(client, "invalidateQueries").mockImplementation((filters?: { queryKey?: unknown[] }) => {
      if (filters?.queryKey) keys.push(filters.queryKey);
      return Promise.resolve();
    });
    invalidateAfterUpdateJob(client);
    return keys.map((key) => key[0]);
  };

  it("改稿の印と棚の件数の両方に知らせる", () => {
    const invalidated = collectInvalidatedKeys();
    expect(invalidated).toContain("pending-revisions");
    expect(invalidated).toContain("library-shelf-counts");
  });

  // 増やすのはよいが、減らすと静かに壊れる。数も見張っておく。
  it("知らせ先を取りこぼしていない", () => {
    expect(collectInvalidatedKeys()).toHaveLength(2);
  });
});
