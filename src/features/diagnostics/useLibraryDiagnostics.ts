import { useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import type { LibraryDiagnostics } from "@/types/library";

export interface DiagnosticsProgress {
  phase: string;
  step: number;
  total: number;
  startedAt: number;
}

/**
 * プレビューの見本の計測。棚の診断は `ShelfDiagnosticsSection` が棚から引く。
 *
 * 押したときだけ見本を返す（開いただけでは「まだ診断していません」のまま）。
 */
export function useLibraryDiagnostics(previewData: LibraryDiagnostics) {
  const query = useQuery({
    queryKey: ["library-diagnostics"],
    queryFn: () => Promise.resolve(previewData),
    enabled: false, staleTime: Infinity, gcTime: Infinity, retry: false,
  });
  const refetchQuery = query.refetch;
  const refetch = useCallback(() => refetchQuery({ cancelRefetch: false }), [refetchQuery]);
  return { ...query, refetch, progress: null as DiagnosticsProgress | null };
}
