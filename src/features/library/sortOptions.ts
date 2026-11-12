import type { EntitySortBy, LibrarySortBy } from "@/types/library";
import type { CollectionSortBy } from "@/types/collections";
import { COLLECTION_SORT_OPTIONS, ENTITY_SORT_OPTIONS } from "@shared/sortOptions";
import type { SortOption } from "@shared/sortOptions";

// 並べ替えの名前は携帯と共通（shared/sortOptions.ts）。ここには型に結びつく解釈だけを置く。
export * from "@shared/sortOptions";

const ENTITY_SORT_VALUES = new Set<EntitySortBy>(
  ENTITY_SORT_OPTIONS.map((option) => option.value.split(":")[0] as EntitySortBy),
);

export function parseEntitySortBy(value: unknown): EntitySortBy {
  return typeof value === "string" && ENTITY_SORT_VALUES.has(value as EntitySortBy)
    ? value as EntitySortBy
    : "work_count";
}

const COLLECTION_SORT_VALUES = new Set<CollectionSortBy>(
  COLLECTION_SORT_OPTIONS.map((option) => option.value.split(":")[0] as CollectionSortBy),
);

export function parseCollectionSortBy(value: unknown): CollectionSortBy {
  return typeof value === "string" && COLLECTION_SORT_VALUES.has(value as CollectionSortBy)
    ? value as CollectionSortBy
    : "created_at";
}

export function sortKeys(options: SortOption[]): Set<LibrarySortBy> {
  return new Set(options.map((option) => option.value.split(":")[0] as LibrarySortBy));
}
