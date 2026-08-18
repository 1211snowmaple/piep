import { invoke } from "@tauri-apps/api/core";
import type {
  CollectionKind,
  CollectionSuggestion,
  WorkCollection,
  WorkCollectionInput,
  WorkCollectionMemberInput,
  WorkCollectionSummary,
  WorkKey,
  WorkLink,
} from "@/types/collections";

export function listWorkCollections(): Promise<WorkCollectionSummary[]> {
  return invoke<WorkCollectionSummary[]>("db_list_work_collections");
}

export function getWorkCollection(collectionId: string): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_get_work_collection", { collectionId });
}

export function upsertWorkCollection(input: WorkCollectionInput): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_upsert_work_collection", { input });
}

export function deleteWorkCollection(collectionId: string): Promise<void> {
  return invoke<void>("db_delete_work_collection", { collectionId });
}

export function addWorkCollectionMembers(collectionId: string, members: WorkCollectionMemberInput[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_add_work_collection_members", { collectionId, members });
}

export function removeWorkCollectionMembers(collectionId: string, members: WorkKey[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_remove_work_collection_members", { collectionId, members });
}

export function reorderWorkCollectionMembers(collectionId: string, members: WorkKey[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_reorder_work_collection_members", { collectionId, members });
}

export function listCollectionsForWork(source: string, sourceId: string): Promise<WorkCollectionSummary[]> {
  return invoke<WorkCollectionSummary[]>("db_list_collections_for_work", { source, sourceId });
}

export function refreshWorkLinks(downloadId: number): Promise<WorkLink[]> {
  return invoke<WorkLink[]>("db_refresh_work_links", { downloadId });
}

export function listWorkLinksForWork(source: string, sourceId: string): Promise<WorkLink[]> {
  return invoke<WorkLink[]>("db_list_work_links_for_work", { source, sourceId });
}

export function generateCollectionSuggestion(seedDownloadIds: number[], limit = 60): Promise<CollectionSuggestion> {
  return invoke<CollectionSuggestion>("db_generate_collection_suggestion", {
    request: { seedDownloadIds, limit },
  });
}

export function listCollectionSuggestions(stateFilter: "pending" | "accepted" | "rejected" | "all" = "pending"): Promise<CollectionSuggestion[]> {
  return invoke<CollectionSuggestion[]>("db_list_collection_suggestions", { stateFilter });
}

export function acceptCollectionSuggestion(input: {
  suggestionId: string;
  name?: string | null;
  collectionKind?: CollectionKind | null;
  memberKeys?: WorkKey[] | null;
}): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_accept_collection_suggestion", { input });
}

/** `memberKeys` limits the negative feedback to the works the reader left
 *  ticked, so unticking the ones that do belong keeps them eligible later. */
export function rejectCollectionSuggestion(suggestionId: string, memberKeys?: WorkKey[]): Promise<boolean> {
  return invoke<boolean>("db_reject_collection_suggestion", { suggestionId, memberKeys: memberKeys ?? null });
}

/** Removes the draft from the inbox without recording negative feedback, so
 *  the same works can be suggested again later. */
export function dismissCollectionSuggestion(suggestionId: string): Promise<boolean> {
  return invoke<boolean>("db_dismiss_collection_suggestion", { suggestionId });
}

export function listCollectionsForPerson(source: string, personKey: string): Promise<WorkCollectionSummary[]> {
  return invoke<WorkCollectionSummary[]>("db_list_collections_for_person", { source, personKey });
}
