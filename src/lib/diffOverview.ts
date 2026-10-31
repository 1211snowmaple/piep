export type OverviewKind = "added" | "removed";

export interface OverviewSpan {
  kind: OverviewKind;
  /** スクロールする本文の中での位置。単位はピクセル。 */
  top: number;
  bottom: number;
}

export interface OverviewMark {
  kind: OverviewKind;
  /** スクロール全体の高さに対する割合。CSS で % として置けるように。 */
  top: number;
  height: number;
}

/**
 * 変更箇所を、スクロールの溝と同じ高さの帯へ写す。
 *
 * 何千もの散らばった変更も、同じ画素に重なれば数本の印にまとまる。一字だけの
 * 変更でも見えるよう、どの印も少なくとも `minPixels` の高さを持たせる。
 */
export function overviewMarks(spans: OverviewSpan[], contentHeight: number, rulerHeight: number, minPixels = 2): OverviewMark[] {
  if (contentHeight <= 0 || rulerHeight <= 0) return [];
  const scale = rulerHeight / contentHeight;
  const marks: OverviewMark[] = [];
  for (const kind of ["removed", "added"] as const) {
    const ranges = spans
      .filter((span) => span.kind === kind)
      .map((span) => {
        const start = Math.min(Math.max(span.top * scale, 0), rulerHeight - minPixels);
        return [start, Math.min(Math.max(span.bottom * scale, start + minPixels), rulerHeight)] as const;
      })
      .sort((left, right) => left[0] - right[0]);
    let current: [number, number] | null = null;
    for (const [start, end] of ranges) {
      if (current && start <= current[1] + 1) current[1] = Math.max(current[1], end);
      else {
        if (current) marks.push(toMark(kind, current, rulerHeight));
        current = [start, end];
      }
    }
    if (current) marks.push(toMark(kind, current, rulerHeight));
  }
  return marks;
}

function toMark(kind: OverviewKind, [start, end]: [number, number], rulerHeight: number): OverviewMark {
  return { kind, top: start / rulerHeight, height: (end - start) / rulerHeight };
}
