// 読書の進み具合の規則。進み具合は 0〜1 の割合で受け取る。

/** ページ番号は 0 から。最後のページの末尾に着いたときに読了とみなす。 */
export function readingFinished(page: number, pageCount: number, atPageEnd: boolean): boolean {
  return pageCount > 0 && page === pageCount - 1 && atPageEnd;
}

/**
 * 読みかけとみなす最小の進み具合。本文の頭（1 ページ目の最上部）にいる作品を
 * 読みかけに数えない。開いて少し動かしただけ、読み進めてから頭へ戻した、も同じ扱いにする。
 */
export const minimumReadProgress = 0.005;

/**
 * 進み具合の百分率。読み終えたときだけ 100 にする。端まで来ていないのに丸めで 100 と出さない。
 * 読みかけに数える量（`minimumReadProgress`）に届いていれば 1 からにする。棚に 0% と並ばず、
 * 読書画面で 1% と出た作品は必ず棚にも載る。
 */
export function readingPercent(progress: number, pageCount = 0): number {
  if (pageCount > 0 && progress >= 1) return 100;
  if (!(progress >= minimumReadProgress)) return 0;
  return Math.max(1, Math.min(99, Math.round(progress * 100)));
}

export function progressLabel(progress: number, pageCount: number): string {
  return `${readingPercent(progress, pageCount)}%`;
}
