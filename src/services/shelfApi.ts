import { invoke } from "@tauri-apps/api/core";
import type {
  SavedSearchRecord,
  SavedSearchInput,
} from "@/types/library";

/** 作者のシリーズの一覧の一ページの件数。 */
export const ENTITY_SERIES_PAGE_SIZE = 60;

export async function listSavedSearches(): Promise<SavedSearchRecord[]> {
  return invoke<SavedSearchRecord[]>("db_list_saved_searches");
}

export async function upsertSavedSearch(input: SavedSearchInput): Promise<SavedSearchRecord> {
  return invoke<SavedSearchRecord>("db_upsert_saved_search", { input });
}

export async function deleteSavedSearch(id: string): Promise<boolean> {
  return invoke<boolean>("db_delete_saved_search", { id });
}
