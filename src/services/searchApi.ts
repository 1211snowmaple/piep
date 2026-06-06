import { invoke } from "@tauri-apps/api/core";
import type {
  BulkMutationResult,
  SearchRebuildProgress,
  SearchSuggestParams,
  SearchSuggestResult,
  SearchV2Params,
  SearchV2Result,
} from "@/types/library";

export async function searchDownloadsV2(params: SearchV2Params): Promise<SearchV2Result> {
  return invoke<SearchV2Result>("search_downloads_v2", { params });
}

export async function searchSuggest(params: SearchSuggestParams): Promise<SearchSuggestResult> {
  return invoke<SearchSuggestResult>("search_suggest", { params });
}

export async function startSearchRebuildIndex(batchSize = 64): Promise<string> {
  return invoke<string>("search_rebuild_index", { jobOptions: { batchSize } });
}

export async function cancelSearchRebuildIndex(jobId: string): Promise<void> {
  return invoke<void>("search_cancel_rebuild_index", { jobId });
}

export async function deleteDownloadsForSearch(params: SearchV2Params): Promise<BulkMutationResult> {
  return invoke<BulkMutationResult>("db_delete_downloads_for_search", { params });
}

export type { SearchRebuildProgress };
