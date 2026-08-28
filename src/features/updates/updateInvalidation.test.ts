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
    const keys: (readonly unknown[])[] = [];
    vi.spyOn(client, "invalidateQueries").mockImplementation((filters?: { queryKey?: readonly unknown[] }) => {
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

  /**
   * 自動保存のジョブは作品を増やす。増えたことを知らないままだと、確認が
   * 終わっても棚は前の一覧、ホームは前の件数を出しつづける。増えたときに
   * 古くなる場所は、消したときに古くなる場所と同じである。
   */
  it("取り込みで増えた作品を、棚とホームにも知らせる", () => {
    const invalidated = collectInvalidatedKeys();
    expect(invalidated).toContain("library");
    expect(invalidated).toContain("library-facets");
    expect(invalidated).toContain("library-entities");
    expect(invalidated).toContain("entity-works");
    expect(invalidated).toContain("dashboard");
  });

  /**
   * 作者・シリーズの見出しに出る作品数と、一覧の件数は別の鍵から引いている。
   * 一覧だけ新しくすると、行は増えたのにバッジは前の数のまま残り、頁送りの
   * 最終ページが実在しない番号を指す。
   */
  it("作者・シリーズの作品数と一覧の件数にも知らせる", () => {
    const invalidated = collectInvalidatedKeys();
    expect(invalidated).toContain("entity");
    expect(invalidated).toContain("entity-tags");
    expect(invalidated).toContain("library-entity-count");
  });

  // 増やすのはよいが、減らすと静かに壊れる。数も見張っておく。
  it("知らせ先を取りこぼしていない", () => {
    expect(collectInvalidatedKeys()).toHaveLength(10);
  });
});
