import { useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getLibraryDiagnostics } from "@/services/dbApi";
import { onTauriEvent } from "@/services/eventBus";
import type { LibraryDiagnostics } from "@/types/library";

export interface DiagnosticsProgress {
  phase: string;
  step: number;
  total: number;
  startedAt: number;
}

/** The measurement and its listener belong to the query, not a mounted page. */
export function useLibraryDiagnostics(runtime: boolean, previewData: LibraryDiagnostics) {
  const client = useQueryClient();
  const progress = useQuery<DiagnosticsProgress | null>({
    queryKey: ["library-diagnostics-progress"], queryFn: () => null,
    enabled: false, gcTime: Infinity,
  });
  const query = useQuery({
    queryKey: ["library-diagnostics"],
    queryFn: async () => {
      const startedAt = Date.now();
      client.setQueryData<DiagnosticsProgress>(["library-diagnostics-progress"], { phase: "database", step: 0, total: 7, startedAt });
      let dispose: (() => void) | undefined;
      try {
        if (runtime) dispose = await onTauriEvent<Omit<DiagnosticsProgress, "startedAt">>("library-diagnostics-progress", ({ payload }) => {
          client.setQueryData<DiagnosticsProgress>(["library-diagnostics-progress"], { ...payload, startedAt });
        });
        // Do not consume the observer's AbortSignal: leaving the page must not
        // cancel the query while native IPC keeps walking the storage folder.
        return runtime ? await getLibraryDiagnostics() : previewData;
      } finally {
        dispose?.();
        client.setQueryData(["library-diagnostics-progress"], null);
      }
    },
    enabled: false, staleTime: Infinity, gcTime: Infinity, retry: false,
  });
  const refetchQuery = query.refetch;
  const refetch = useCallback(() => refetchQuery({ cancelRefetch: false }), [refetchQuery]);
  return { ...query, refetch, progress: progress.data };
}
