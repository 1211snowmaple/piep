import { useEffect, useMemo, useState } from "react";
import { createTextDiff, type TextDiffResult } from "./textDiff";

/** これより短ければ差分は1フレームのうちに終わる。worker を使っても読み込み中の表示がちらつくだけになる。 */
const WORKER_THRESHOLD = 20_000;

/**
 * 二つの版の差分を、ページを固めずに取る。
 *
 * 全体に手が入った長い作品では数百ミリ秒かかるので worker で動かす。比べるたびに
 * 別の worker を立てるので、版を切り替えれば古い比較は打ち切られる。
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
