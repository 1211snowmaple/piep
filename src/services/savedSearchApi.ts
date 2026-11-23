import { invoke } from "@tauri-apps/api/core";
import type {
  SavedSearchRecord,
  SavedSearchInput,
} from "@/types/library";

export async function listSavedSearches(): Promise<SavedSearchRecord[]> {
  return invoke<SavedSearchRecord[]>("list_saved_searches");
}

export async function upsertSavedSearch(input: SavedSearchInput): Promise<SavedSearchRecord> {
  return invoke<SavedSearchRecord>("upsert_saved_search", { input });
}

export async function deleteSavedSearch(id: string): Promise<boolean> {
  return invoke<boolean>("delete_saved_search", { id });
}
