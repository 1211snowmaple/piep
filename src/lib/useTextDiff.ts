import { useEffect, useMemo, useState } from "react";
import { createTextDiff, type TextDiffResult } from "./textDiff";

/**
 * これより短ければ、全体を書き換えた版どうしでも差分は1フレームのうちに終わる。
 *
 * 重さを決めるのは長さより違いの多さで、まったく違う本文どうしだと合わせて
 * 4,000字で約200ms、20,000字で約700msかかる。以前は20,000字まで画面の側で
 * 計算していたので、書き換えの多い版を開くと画面が止まっていた。
 */
const WORKER_THRESHOLD = 1_000;

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
