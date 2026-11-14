import { describe, expect, it } from "vitest";
import { minimumReadProgress, readingFinished, readingPercent } from "./readingProgress";

describe("読書の進み具合", () => {
  it("最後のページの端まで読んだときだけ読了とみなす", () => {
    expect(readingFinished(200 / 201, 201)).toBe(false);
    expect(readingFinished(1, 201)).toBe(true);
    expect(readingPercent(200 / 201, 201)).toBe(99);
    expect(readingPercent(1, 201)).toBe(100);
  });

  it("読みかけに数える量に届いたら 1% から、届かなければ 0%", () => {
    expect(readingPercent(minimumReadProgress - 0.0001)).toBe(0);
    expect(readingPercent(minimumReadProgress)).toBe(1);
  });

  it("ページ数を渡さなければ 99% で止める。デスクトップの読書画面の丸め方と同じ値になる", () => {
    // ちょうど半分（14.5% など）は、割って掛け直す間の誤差で丸めが一つずれうるので比べない。
    for (let step = 0; step <= 1000; step += 1) {
      const percent = step / 10;
      if (step % 10 === 5) continue;
      expect(readingPercent(percent / 100), String(percent)).toBe(Math.min(99, Math.round(percent)));
    }
  });
});
