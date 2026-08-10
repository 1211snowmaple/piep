import { invoke } from "@tauri-apps/api/core";
import type {
  SearchRebuildProgress,
  SearchSuggestParams,
  SearchSuggestResult,
} from "@/types/library";

export { deleteDownloadsForSearch, searchDownloadsV2 } from "@/services/dbApi";

export async function searchSuggest(params: SearchSuggestParams): Promise<SearchSuggestResult> {
  const text = params.text?.trim() || null;
  if (!text) return { items: [] };
  const rawLimit = params.limit ?? 8;
  const limit = Number.isFinite(rawLimit) ? Math.min(50, Math.max(1, Math.trunc(rawLimit))) : 8;
  return invoke<SearchSuggestResult>("search_suggest", { params: { text, limit } });
}

export interface SearchRebuildOptions {
  /** Documents read and analysed together. Larger keeps more cores busy. */
  batchSize?: number;
  /** Also build the semantic vectors. This is the GPU-accelerated part and is
   *  far slower than the lexical pass, so it is opt-in. */
  includeSemantic?: boolean;
}

export async function startSearchRebuildIndex(options: SearchRebuildOptions = {}): Promise<string> {
  const requested = options.batchSize ?? 64;
  const batchSize = Number.isFinite(requested) ? Math.min(512, Math.max(8, Math.trunc(requested))) : 64;
  return invoke<string>("search_rebuild_index", {
    jobOptions: { batchSize, includeSemantic: options.includeSemantic === true },
  });
}

export async function cancelSearchRebuildIndex(jobId: string): Promise<void> {
  return invoke<void>("search_cancel_rebuild_index", { jobId });
}

export type { SearchRebuildProgress };
