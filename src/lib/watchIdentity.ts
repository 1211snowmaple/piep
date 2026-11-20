import type { WatchGroup, WatchId } from "@/types/library";

/** 移行前に端末へ残したグループ番号も、同じグループの UUID へ引き継ぐ。 */
export function watchGroupMatches(group: WatchGroup, id: WatchId | null): boolean {
  return group.id === id || (typeof id === "number" && group.legacyId === id);
}

export function watchGroupSelection(value: string | null, groups: WatchGroup[]): WatchId | null {
  if (!value || value === "all") return null;
  return groups.find((group) => String(group.id) === value)?.id ?? value;
}
