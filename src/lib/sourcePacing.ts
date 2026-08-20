/**
 * 取得元への当たり方を一箇所に決める。
 *
 * pixiv も FANBOX も、続けざまに叩けば断ってくる。断られてから考えるのでは
 * 遅く、断られたあとも同じ速さで叩き続ければ、残りの全部が同じように落ちる。
 * 保存も更新確認も、間隔と引き下がり方はここに揃える。
 */

/** 1件ごとに空ける間隔。更新ジョブ（Rust側）の 800ms と揃えてある。 */
export const SOURCE_REQUEST_DELAY_MS = 900;
/** 断られたときに間隔を何倍まで広げるか。900ms × 32 ≒ 29秒。 */
export const MAX_RATE_LIMIT_BACKOFF = 32;
/** 同じ項目を取得制限で何回までやり直すか。 */
export const MAX_RATE_LIMIT_RETRIES = 2;

/**
 * 待つ。ただし、途中でやめられる。
 *
 * 取得制限のあとの待ちは 30 秒近くまで伸びる。そのあいだに中止を押した人を
 * 待たせ続ければ、押せていないのと同じになる。
 */
export const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve) => {
    if (signal?.aborted) return resolve();
    const done = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", done);
      resolve();
    };
    const timer = setTimeout(done, ms);
    signal?.addEventListener("abort", done, { once: true });
  });

/**
 * この失敗は「今は無理」か。
 *
 * 取得元は制限を素直に 429 で返すとは限らず、本文にだけ Rate Limit と
 * 書いて 200 で返してくることもある。日本語の言い回しも拾うのは、
 * バックエンドが訳したメッセージがそのまま上がってくるため。
 */
export function isRateLimited(error: unknown): boolean {
  const text = (error instanceof Error ? error.message : String(error ?? "")).toLowerCase();
  return [
    "rate limit",
    "too many requests",
    "http 429",
    "status: 429",
    "レートリミット",
    "アクセス制限",
  ].some((marker) => text.includes(marker));
}

export interface SourcePacer {
  /** 次の1件へ進む前に待つ。 */
  wait(): Promise<void>;
  /** 断られた。間隔を広げ、待った時間（ミリ秒）を返す。 */
  backOff(): Promise<number>;
  /** 通った。広げた間隔を半分ずつ元へ戻す。 */
  relax(): void;
  /** いまの待ち時間。表示用。 */
  currentDelayMs(): number;
  /** もう待たない。待っている最中なら、その待ちをその場で終わらせる。 */
  abort(): void;
}

/**
 * 間隔を持って回る係。
 *
 * 通れば少しずつ元の速さへ戻し、断られれば倍にして下がる。取得元の機嫌を
 * こちらで測るのではなく、返ってきた答えに合わせる。
 */
export function createSourcePacer(
  delayMs = SOURCE_REQUEST_DELAY_MS,
  wait: (ms: number, signal?: AbortSignal) => Promise<void> = sleep,
): SourcePacer {
  let multiplier = 1;
  // 中止は待ちの外から来る。合図は係が自分で持ち、待つたびに渡す。
  const stop = new AbortController();
  return {
    currentDelayMs: () => delayMs * multiplier,
    wait: () => wait(delayMs * multiplier, stop.signal),
    backOff: async () => {
      multiplier = Math.min(MAX_RATE_LIMIT_BACKOFF, multiplier * 2);
      const waited = delayMs * multiplier;
      await wait(waited, stop.signal);
      return waited;
    },
    relax: () => { multiplier = Math.max(1, Math.floor(multiplier / 2)); },
    abort: () => stop.abort(),
  };
}
