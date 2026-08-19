import { describe, expect, it } from "vitest";
import { errorMessage } from "./format";

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
