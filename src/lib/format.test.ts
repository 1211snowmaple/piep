import { describe, expect, it } from "vitest";
import { errorMessage, formatFreshness } from "./format";

describe("formatFreshness", () => {
  const now = new Date("2026-08-22T12:00:00+09:00");

  it("reads recent checks as elapsed time, not as a calendar date", () => {
    expect(formatFreshness("2026-08-22T11:30:00+09:00", now)).toBe("さっき");
    expect(formatFreshness("2026-08-22T09:00:00+09:00", now)).toBe("3時間前");
    expect(formatFreshness("2026-08-20T12:00:00+09:00", now)).toBe("2日前");
  });

  // 「183日前」は読み解く手間がかかる。遠くなったら絶対日付のほうが早い。
  it("falls back to a calendar date once the relative form stops helping", () => {
    expect(formatFreshness("2026-07-23T12:00:00+09:00", now)).toBe("30日前");
    expect(formatFreshness("2026-07-22T12:00:00+09:00", now)).toContain("2026");
  });

  // 確認したことが無い相手に「さっき」と言わない。
  it("says nothing rather than something wrong when there is no timestamp", () => {
    expect(formatFreshness(null, now)).toBe("—");
    expect(formatFreshness(undefined, now)).toBe("—");
    expect(formatFreshness("", now)).toBe("—");
  });

  // 時計のずれで未来になった値を「これから」と読ませない。
  it("treats a future timestamp as clock skew", () => {
    expect(formatFreshness("2026-08-22T13:00:00+09:00", now)).toBe("さっき");
  });

  it("returns an unreadable value unchanged instead of inventing one", () => {
    expect(formatFreshness("きのう", now)).toBe("きのう");
  });
});

describe("errorMessage", () => {
  it("does not expose an entire FANBOX response in a notification", () => {
    const message = errorMessage(`デシリアライズエラー: missing field, body: {"body":"${"長".repeat(1000)}"}`);
    expect(message).toBe("デシリアライズエラー: missing field");
  });

  it("extracts common structured messages and caps unknown errors", () => {
    expect(errorMessage('{"message":"接続できません"}')).toBe("接続できません");
    expect(errorMessage("x".repeat(500))).toHaveLength(280);
  });

  // エラーを言葉にする側が投げると、扱えたはずの失敗が捕まらない失敗になる。
  // JSON.stringify が undefined を返す型が、そのまま入ってくることがある。
  it("never throws on values JSON.stringify cannot describe", () => {
    for (const value of [undefined, null, () => undefined, Symbol("x")]) {
      expect(errorMessage(value)).toBe("不明なエラー");
    }
  });
});
