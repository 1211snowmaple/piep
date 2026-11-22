import type { WatchGroup, WatchId } from "@/types/library";

export function watchGroupSelection(value: string | null, groups: WatchGroup[]): WatchId | null {
  if (!value || value === "all") return null;
  return groups.find((group) => String(group.id) === value)?.id ?? value;
}
