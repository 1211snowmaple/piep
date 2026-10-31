import { useEffect, useMemo, useState } from "react";
import { createTextDiff, type TextDiffResult } from "./textDiff";

/** Below this many characters a diff finishes within a frame, so a worker would only add a flash of loading. */
const WORKER_THRESHOLD = 20_000;

/**
 * Diff two versions without freezing the page. A long work that was revised
 * throughout takes a few hundred milliseconds, so it runs in a worker; each
 * comparison gets its own worker so switching versions cancels the old one.
 */
export function useTextDiff(before: string | null, after: string): { diff: TextDiffResult | null; pending: boolean } {
  const inline = before === null || typeof Worker === "undefined" || before.length + after.length < WORKER_THRESHOLD;
  const inlineDiff = useMemo(() => inline && before !== null ? createTextDiff(before, after) : null, [inline, before, after]);
  const [computed, setComputed] = useState<{ before: string; after: string; diff: TextDiffResult } | null>(null);

  useEffect(() => {
    if (inline || before === null) return;
    const fallback = () => setComputed({ before, after, diff: createTextDiff(before, after) });
    let worker: Worker;
    try {
      worker = new Worker(new URL("./textDiff.worker.ts", import.meta.url), { type: "module" });
    } catch {
      fallback();
      return;
    }
    worker.onmessage = (event: MessageEvent<TextDiffResult>) => {
      setComputed({ before, after, diff: event.data });
      worker.terminate();
    };
    worker.onerror = () => {
      worker.terminate();
      fallback();
    };
    worker.postMessage({ before, after });
    return () => worker.terminate();
  }, [inline, before, after]);

  if (inline) return { diff: inlineDiff, pending: false };
  const ready = computed !== null && computed.before === before && computed.after === after;
  return { diff: ready ? computed.diff : null, pending: !ready };
}
