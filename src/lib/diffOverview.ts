export type OverviewKind = "added" | "removed";

export interface OverviewSpan {
  kind: OverviewKind;
  /** Offsets within the scrolled content, in pixels. */
  top: number;
  bottom: number;
}

export interface OverviewMark {
  kind: OverviewKind;
  /** Fractions of the whole scroll height, so CSS can place them with %. */
  top: number;
  height: number;
}

/**
 * Map changed spans onto a ruler as tall as the scroll track. Thousands of
 * scattered edits merge into a few marks once they share a pixel, and every
 * edit stays at least `minPixels` tall so a one-character change is visible.
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
