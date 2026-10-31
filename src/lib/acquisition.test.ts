import { describe, expect, it } from "vitest";
import { incompleteSummary } from "./acquisition";

describe("incompleteSummary", () => {
  it("names masked details separately from missing illustrations", () => {
    expect(incompleteSummary(true, [])).toBe("作品は保存済みです。一部の挿絵を取得できませんでした。");
    expect(incompleteSummary(false, ["caption", "tags"])).toBe("作品は保存済みです。作品情報（説明・タグ）が取得元で伏せられた状態で保存されています。再確認すると補完します。");
    expect(incompleteSummary(true, ["title"])).toBe("作品は保存済みです。一部の挿絵と作品情報（題名）を取得できませんでした。");
    expect(incompleteSummary(false, [])).toBe("作品は保存済みです。一部の内容を取得できませんでした。");
  });
});
