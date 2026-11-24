/** 棚の `coalesce(published_at,saved_at)` と同じ優先順で、日時をミリ秒の鍵にする。 */
export function publishedSortTime(publishedAt: string | null, savedAt: string): number {
  const published = Date.parse(publishedAt ?? "");
  return Number.isNaN(published) ? Date.parse(savedAt) : published;
}
