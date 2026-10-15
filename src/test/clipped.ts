/**
 * jsdom には版組みが無く、どの要素も 0×0 を返す。
 *
 * 「切れているときだけ出す」ものは、切れている状態を作らないと試せない。
 * 実寸を測る側（`ClippedTooltip`）は scrollWidth/scrollHeight を見るので、
 * その二つだけを、この中で描かれるあいだ大きく答えさせる。
 */
export function withClippedText<T>(run: () => T): T {
  const keys = ["scrollWidth", "scrollHeight"] as const;
  for (const key of keys) {
    Object.defineProperty(HTMLElement.prototype, key, { configurable: true, get: () => 999 });
  }
  try {
    return run();
  } finally {
    for (const key of keys) delete (HTMLElement.prototype as unknown as Record<string, unknown>)[key];
  }
}
