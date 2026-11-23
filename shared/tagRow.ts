/** 行に収まる先頭からの件数。タグがある行には最低一件を出す。 */
export function visibleTagCount({
  widths, available, gap, overflowWidth, limit = widths.length,
}: {
  widths: readonly number[];
  available: number;
  gap: number;
  overflowWidth?: (hiddenCount: number) => number;
  limit?: number;
}): number {
  if (widths.length === 0) return 0;
  let used = 0;
  let fits = 0;
  for (let index = 0; index < limit; index += 1) {
    const next = used + (index === 0 ? 0 : gap) + widths[index];
    const hidden = widths.length - index - 1;
    const reserve = hidden > 0 && overflowWidth ? gap + overflowWidth(hidden) : 0;
    if (next + reserve > available) break;
    used = next;
    fits += 1;
  }
  return Math.max(1, fits);
}
