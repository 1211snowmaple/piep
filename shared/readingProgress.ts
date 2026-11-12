// 読書の進み具合の規則。進み具合は 0〜1 の割合で受け取る。

/**
 * 全体の割合を丸めると、201 ページの本は 200 ページ目で 100% になる。読書位置を片付ける
 * 条件と同じく、最後のページの端まで読んだときだけ読了とみなす。
 */
export function readingFinished(progress: number, pageCount: number): boolean {
  return pageCount > 0 && progress * pageCount >= pageCount - 0.02;
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
  if (pageCount > 0 && readingFinished(progress, pageCount)) return 100;
  if (!(progress >= minimumReadProgress)) return 0;
  return Math.max(1, Math.min(99, Math.round(progress * 100)));
}

export function progressLabel(progress: number, pageCount: number): string {
  return `${readingPercent(progress, pageCount)}%`;
}
