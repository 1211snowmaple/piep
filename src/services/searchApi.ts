import { invoke } from "@tauri-apps/api/core";
import type {
  SearchRebuildProgress,
  SearchIndexStatus,
} from "@/types/library";

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
    jobOptions: {
      batchSize,
      ...(options.includeSemantic === undefined ? {} : { includeSemantic: options.includeSemantic }),
    },
  });
}

export async function setSemanticSearchEnabled(enabled: boolean): Promise<SearchIndexStatus> {
  return invoke<SearchIndexStatus>("search_set_semantic_enabled", { enabled });
}

export async function cancelSearchRebuildIndex(jobId: string): Promise<void> {
  return invoke<void>("search_cancel_rebuild_index", { jobId });
}

export type { SearchRebuildProgress };
