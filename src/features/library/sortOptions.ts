import type { EntitySortBy, LibrarySortBy, LibrarySortOrder } from "@/types/library";
import type { CollectionSortBy } from "@/types/collections";

/** A single select value owns both halves of an ordering. */
export type SortChoice = `${string}:${LibrarySortOrder}`;
export type SortOption = { value: SortChoice; label: string };

/**
 * The canonical work ordering vocabulary.
 *
 * LibraryPage and EntityPage used to carry separate copies. That made a new
 * ordering appear on the main shelf but not inside an author or series. Keep
 * one list so a work remains sortable the same way wherever it is shown.
 */
export const WORK_SORT_OPTIONS: SortOption[] = [
  { value: "downloaded_at:desc", label: "保存日：新しい順" },
  { value: "downloaded_at:asc", label: "保存日：古い順" },
  { value: "source_created_at:desc", label: "公開日：新しい順" },
  { value: "source_created_at:asc", label: "公開日：古い順" },
  { value: "source_updated_at:desc", label: "更新日：新しい順" },
  { value: "source_updated_at:asc", label: "更新日：古い順" },
  { value: "title:asc", label: "タイトル：昇順（あ→ん）" },
  { value: "title:desc", label: "タイトル：降順（ん→あ）" },
  { value: "author_name:asc", label: "作者名：昇順（あ→ん）" },
  { value: "author_name:desc", label: "作者名：降順（ん→あ）" },
  { value: "text_length:desc", label: "文字数：多い順" },
  { value: "text_length:asc", label: "文字数：少ない順" },
  { value: "file_size_bytes:desc", label: "容量：大きい順" },
  { value: "file_size_bytes:asc", label: "容量：小さい順" },
  { value: "asset_count:desc", label: "添付数：多い順" },
  { value: "asset_count:asc", label: "添付数：少ない順" },
  { value: "current_version:desc", label: "版番号：大きい順" },
  { value: "current_version:asc", label: "版番号：小さい順" },
];

export const SERIES_ORDER_SORT_OPTIONS: SortOption[] = [
  { value: "series_order:asc", label: "シリーズ内順：先頭から" },
  { value: "series_order:desc", label: "シリーズ内順：末尾から" },
];

export const RELEVANCE_SORT_OPTION: SortOption = {
  value: "relevance:desc",
  label: "関連度：高い順",
};

export const SEARCH_SORT_OPTIONS: SortOption[] = [RELEVANCE_SORT_OPTION, ...WORK_SORT_OPTIONS];

/** Aggregates of the works under an author or series. */
export const ENTITY_SORT_OPTIONS: SortOption[] = [
  { value: "work_count:desc", label: "作品数：多い順" },
  { value: "work_count:asc", label: "作品数：少ない順" },
  { value: "downloaded_at:desc", label: "保存日：新しい順" },
  { value: "downloaded_at:asc", label: "保存日：古い順" },
  { value: "source_created_at:desc", label: "公開日：新しい順" },
  { value: "source_created_at:asc", label: "公開日：古い順" },
  { value: "source_updated_at:desc", label: "更新日：新しい順" },
  { value: "source_updated_at:asc", label: "更新日：古い順" },
  { value: "name:asc", label: "名前：昇順（あ→ん）" },
  { value: "name:desc", label: "名前：降順（ん→あ）" },
  { value: "text_length:desc", label: "合計文字数：多い順" },
  { value: "text_length:asc", label: "合計文字数：少ない順" },
  { value: "file_size_bytes:desc", label: "合計容量：大きい順" },
  { value: "file_size_bytes:asc", label: "合計容量：小さい順" },
  { value: "asset_count:desc", label: "合計添付数：多い順" },
  { value: "asset_count:asc", label: "合計添付数：少ない順" },
  { value: "current_version:desc", label: "改稿回数：多い順" },
  { value: "current_version:asc", label: "改稿回数：少ない順" },
];

const ENTITY_SORT_VALUES = new Set<EntitySortBy>(
  ENTITY_SORT_OPTIONS.map((option) => option.value.split(":")[0] as EntitySortBy),
);

export function parseEntitySortBy(value: unknown): EntitySortBy {
  return typeof value === "string" && ENTITY_SORT_VALUES.has(value as EntitySortBy)
    ? value as EntitySortBy
    : "work_count";
}

export const COLLECTION_SORT_OPTIONS: SortOption[] = [
  { value: "created_at:desc", label: "作成日：新しい順" },
  { value: "created_at:asc", label: "作成日：古い順" },
  { value: "updated_at:desc", label: "更新日：新しい順" },
  { value: "updated_at:asc", label: "更新日：古い順" },
  { value: "name:asc", label: "名前：昇順（あ→ん）" },
  { value: "name:desc", label: "名前：降順（ん→あ）" },
  { value: "member_count:desc", label: "登録作品数：多い順" },
  { value: "member_count:asc", label: "登録作品数：少ない順" },
  { value: "available_count:desc", label: "保存済み作品数：多い順" },
  { value: "available_count:asc", label: "保存済み作品数：少ない順" },
  { value: "text_length:desc", label: "合計文字数：多い順" },
  { value: "text_length:asc", label: "合計文字数：少ない順" },
  { value: "revision:desc", label: "更新回数：多い順" },
  { value: "revision:asc", label: "更新回数：少ない順" },
];

const COLLECTION_SORT_VALUES = new Set<CollectionSortBy>(
  COLLECTION_SORT_OPTIONS.map((option) => option.value.split(":")[0] as CollectionSortBy),
);

export function parseCollectionSortBy(value: unknown): CollectionSortBy {
  return typeof value === "string" && COLLECTION_SORT_VALUES.has(value as CollectionSortBy)
    ? value as CollectionSortBy
    : "created_at";
}

export function defaultSortOrderFor(sortBy: string): LibrarySortOrder {
  return sortBy === "title"
    || sortBy === "author_name"
    || sortBy === "name"
    || sortBy === "series_order"
    ? "asc"
    : "desc";
}

export function parseSortOrder(value: unknown, fallback: LibrarySortOrder): LibrarySortOrder {
  return value === "asc" || value === "desc" ? value : fallback;
}

export function parseSortChoice(value: string | null): { key: string; order: LibrarySortOrder } | null {
  if (!value) return null;
  const separator = value.lastIndexOf(":");
  if (separator < 1) return null;
  const key = value.slice(0, separator);
  const order = value.slice(separator + 1);
  return order === "asc" || order === "desc" ? { key, order } : null;
}

export function sortChoice(key: string, order: LibrarySortOrder): SortChoice {
  return `${key}:${order}`;
}

export function sortKeys(options: SortOption[]): Set<LibrarySortBy> {
  return new Set(options.map((option) => option.value.split(":")[0] as LibrarySortBy));
}
