import { describe, expect, it } from "vitest";
import {
  createSourcePacer,
  isRateLimited,
  MAX_RATE_LIMIT_BACKOFF,
  SOURCE_REQUEST_DELAY_MS,
} from "./sourcePacing";

/** 実際には待たず、待つはずだった時間だけ記録する。 */
function recordingPacer(delayMs = SOURCE_REQUEST_DELAY_MS) {
  const waits: number[] = [];
  const pacer = createSourcePacer(delayMs, async (ms) => { waits.push(ms); });
  return { pacer, waits };
}

describe("取得制限の見分け", () => {
  // 相手は 429 で返すとは限らず、200 の本文にだけ書いてくることがある。
  // バックエンドが訳したあとの日本語も、同じものとして拾えないといけない。
  it("reads a refusal in every shape the sources send it", () => {
    for (const message of [
      'レスポンスを解析できませんでした: {"error":{"message":"Rate Limit"}}',
      "アクセス制限（レートリミット）に達しました。時間をおいてからやり直してください",
      "pixiv APIエラー（HTTP 429）",
      "Too Many Requests",
      // FANBOX は日本語だけで知らせてくる。英語の目印しか見ていないと素通りする。
      "FANBOXのアクセス制限に達しました。時間をおいて再試行してください",
    ]) {
      expect(isRateLimited(new Error(message)), message).toBe(true);
    }
  });

  it("does not mistake ordinary failures for a refusal", () => {
    for (const message of [
      "HTTP 404 Not Found",
      "認証が必要です",
      "この投稿の本文を取得できませんでした",
    ]) {
      expect(isRateLimited(new Error(message)), message).toBe(false);
    }
    expect(isRateLimited(undefined)).toBe(false);
  });
});

describe("取得元への当たり方", () => {
  it("leaves a gap between requests instead of firing them back to back", async () => {
    const { pacer, waits } = recordingPacer();
    await pacer.wait();
    await pacer.wait();
    expect(waits).toEqual([SOURCE_REQUEST_DELAY_MS, SOURCE_REQUEST_DELAY_MS]);
  });

  // 断られたら倍にして下がる。同じ速さで叩き続ければ、残り全部が同じように落ちる。
  it("doubles the gap each time it is refused, up to a ceiling", async () => {
    const { pacer, waits } = recordingPacer(1000);
    await pacer.backOff();
    await pacer.backOff();
    expect(waits).toEqual([2000, 4000]);

    for (let i = 0; i < 10; i += 1) await pacer.backOff();
    expect(pacer.currentDelayMs()).toBe(1000 * MAX_RATE_LIMIT_BACKOFF);
  });

  // 制限が解けたあとも遅いままにしない。通ったぶんだけ元へ戻す。
  it("returns toward the normal pace as requests start succeeding", async () => {
    const { pacer } = recordingPacer(1000);
    await pacer.backOff();
    await pacer.backOff();
    expect(pacer.currentDelayMs()).toBe(4000);

    pacer.relax();
    expect(pacer.currentDelayMs()).toBe(2000);
    pacer.relax();
    expect(pacer.currentDelayMs()).toBe(1000);
    // 元の速さより速くはならない。
    pacer.relax();
    expect(pacer.currentDelayMs()).toBe(1000);
  });
});
