import { describe, expect, it } from "vitest";
import { hasSourceRevision, workFreshness } from "./workFreshness";

describe("workFreshness", () => {
  // 改稿を見つけたとき、基準値はわざと書き換えない（取り直して初めて追いつく）。
  // だから手元の列だけを見ると「照合済み」に見える。候補のほうが答えである。
  it("lets a pending revision win over the stored baseline", () => {
    expect(workFreshness({ sourceUpdatedAt: "2026-07-27T00:02:20+09:00", hasPendingRevision: true })).toBe("revised");
  });

  it("calls a work current only when it has actually been checked", () => {
    expect(workFreshness({ sourceUpdatedAt: "2026-07-27T00:02:20+09:00" })).toBe("current");
  });

  // 照合していないものを「最新」と言わない。言えば、確かめていないことが
  // 確かめた結果として画面に出る。
  it("never passes an unchecked work off as current", () => {
    expect(workFreshness({ sourceUpdatedAt: null })).toBe("unchecked");
    expect(workFreshness({ sourceUpdatedAt: null, hasPendingRevision: false })).toBe("unchecked");
  });
});

describe("hasSourceRevision", () => {
  // 公開したきりの作品では、最終更新は公開日と同じ瞬間を指す。行を作らない。
  it("stays quiet when the work was never touched after publishing", () => {
    expect(hasSourceRevision({
      sourceCreatedAt: "2026-07-26T00:00:01+09:00",
      sourceUpdatedAt: "2026-07-26T00:00:01+09:00",
    })).toBe(false);
  });

  it("speaks up once the source moved past the publication date", () => {
    expect(hasSourceRevision({
      sourceCreatedAt: "2026-07-26T00:00:01+09:00",
      sourceUpdatedAt: "2026-07-27T00:02:20+09:00",
    })).toBe(true);
  });

  // 取得元は詳細で +00:00、一覧で +09:00 を返す。文字列ではなく瞬間で比べる。
  it("compares instants rather than the way they are written", () => {
    expect(hasSourceRevision({
      sourceCreatedAt: "2026-07-25T15:00:01+00:00",
      sourceUpdatedAt: "2026-07-26T00:00:01+09:00",
    })).toBe(false);
  });

  it("has nothing to say without an update date", () => {
    expect(hasSourceRevision({ sourceCreatedAt: "2026-07-26T00:00:01+09:00", sourceUpdatedAt: null })).toBe(false);
  });

  // 読めない値を「更新された」と読み替えない。
  it("does not invent a revision out of an unreadable value", () => {
    expect(hasSourceRevision({ sourceCreatedAt: "2026-07-26T00:00:01+09:00", sourceUpdatedAt: "きのう" })).toBe(false);
  });
});
