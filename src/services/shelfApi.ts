import { invoke } from "@tauri-apps/api/core";
import type {
  EntityFacet,
  FacetCount,
  LibraryShelfCounts,
  SavedSearchRecord,
  SavedSearchInput,
} from "@/types/library";

/** The series an author has works in. */
export async function listEntitySeries(source: string, sourceKey: string, limit = 60): Promise<EntityFacet[]> {
  return invoke<EntityFacet[]>("db_list_entity_series", { source, sourceKey, limit });
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
