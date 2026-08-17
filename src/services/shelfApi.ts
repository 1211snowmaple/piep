import { invoke } from "@tauri-apps/api/core";
import type {
  EntityFacet,
  FacetCount,
  LibraryShelfCounts,
  SavedSearchRecord,
  SavedSearchInput,
} from "@/types/library";

/** Matches the native command's current maximum result size. */
export const ENTITY_SERIES_LIMIT = 200;
/** Default page size for the keyset author-series listing. */
export const ENTITY_SERIES_PAGE_SIZE = 60;

export interface EntitySeriesPage {
  items: EntityFacet[];
  nextCursor: string | null;
  total: number;
}

export interface EntitySeriesPageParams {
  query?: string | null;
  limit?: number | null;
  cursor?: string | null;
}

/** The series an author has works in. */
export async function listEntitySeries(source: string, sourceKey: string, limit = ENTITY_SERIES_LIMIT): Promise<EntityFacet[]> {
  const boundedLimit = Number.isSafeInteger(limit)
    ? Math.min(ENTITY_SERIES_LIMIT, Math.max(1, limit))
    : ENTITY_SERIES_LIMIT;
  return invoke<EntityFacet[]>("db_list_entity_series", { source, sourceKey, limit: boundedLimit });
}

/**
 * Keyset-paged series for one author. The cursor is opaque and scoped by the
 * native command to this author and query, so the UI never derives or edits it.
 */
export async function listEntitySeriesPage(
  source: string,
  sourceKey: string,
  params: EntitySeriesPageParams = {},
): Promise<EntitySeriesPage> {
  const requestedLimit = params.limit ?? ENTITY_SERIES_PAGE_SIZE;
  const limit = Number.isSafeInteger(requestedLimit)
    ? Math.min(ENTITY_SERIES_LIMIT, Math.max(1, requestedLimit))
    : ENTITY_SERIES_PAGE_SIZE;
  const query = params.query?.trim().slice(0, 200) || null;
  return invoke<EntitySeriesPage>("db_list_entity_series_paged", {
    source,
    sourceKey,
    query,
    limit,
    cursor: params.cursor ?? null,
  });
}

/** The tags the works of one author or series carry, most used first. */
export async function listEntityTags(kind: "person" | "series", source: string, sourceKey: string, limit = 40): Promise<FacetCount[]> {
  return invoke<FacetCount[]>("db_list_entity_tags", { kind, source, sourceKey, limit });
}

/** Matches the backend cap, so an oversized request is refused before it is sent. */
const MAX_SHELF_IDS = 20_000;

export async function getLibraryShelfCounts(readingIds: number[] = []): Promise<LibraryShelfCounts> {
  const bounded = readingIds
    .filter((id) => Number.isSafeInteger(id) && id > 0)
    .slice(0, MAX_SHELF_IDS);
  return invoke<LibraryShelfCounts>("db_get_library_shelf_counts", { readingIds: bounded });
}

export async function listSavedSearches(): Promise<SavedSearchRecord[]> {
  return invoke<SavedSearchRecord[]>("db_list_saved_searches");
}

export async function upsertSavedSearch(input: SavedSearchInput): Promise<SavedSearchRecord> {
  return invoke<SavedSearchRecord>("db_upsert_saved_search", { input });
}

export async function deleteSavedSearch(id: number): Promise<boolean> {
  return invoke<boolean>("db_delete_saved_search", { id });
}
