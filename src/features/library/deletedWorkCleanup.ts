import type { QueryClient } from "@tanstack/react-query";
import { forgetReadingPositions } from "@/features/library/readingShelf";
import { invalidateWorkSetViews } from "@/features/library/workSetInvalidation";

const WORK_QUERY_PREFIXES = [
  "reader-metadata",
  "reader-content-page",
  "reader-content-search",
  "editor-document",
  "work-assets",
  "work-json",
] as const;

export interface DeletedWorkCleanup {
  queryClient: QueryClient;
  ids: readonly number[];
  removeFromEpubQueue: (ids: number[]) => void;
}

/** Removes every client-side reference after the database deletion commits. */
export function cleanupDeletedWorks({ queryClient, ids, removeFromEpubQueue }: DeletedWorkCleanup): void {
  const uniqueIds = [...new Set(ids.filter((id) => Number.isSafeInteger(id) && id > 0))];
  if (!uniqueIds.length) return;
  uniqueIds.forEach((id) => {
    forgetReadingPositions(id);
    WORK_QUERY_PREFIXES.forEach((prefix) => queryClient.removeQueries({ queryKey: [prefix, id] }));
  });
  removeFromEpubQueue(uniqueIds);
  // Every saved search can contain a deleted work; keeping any listing cache
  // would let a card reopen data that no longer exists.
  queryClient.removeQueries({ queryKey: ["library"] });
  invalidateWorkSetViews(queryClient);
}

/** Cleanup is deliberately sequenced after success; a failed delete retains all local state. */
export async function deleteThenCleanup<T>(deleteAction: () => Promise<T>, cleanup: DeletedWorkCleanup): Promise<T> {
  const result = await deleteAction();
  cleanupDeletedWorks(cleanup);
  return result;
}
