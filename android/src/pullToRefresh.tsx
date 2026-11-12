import { useEffect, useRef, useState, type CSSProperties, type RefObject } from "react";
import { RefreshCw } from "lucide-react";

/** 指を動かした量の半分だけ印を下げる。これだけ下がったら、離したときに読み込み直す。 */
const triggerAt = 64;
const maxPull = 96;
/** すぐ終わる読み込みでも、回る印を一瞬で消さない。 */
const minimumSpinMs = 450;

/**
 * 一番上で下へ引いて離すと読み込み直す。スマホのアプリでよくある形。
 *
 * 触れ始めに `scroller` が一番上にあるときだけ数える。横に動かした指（ページ送りのスワイプ）と、
 * 上へ動かした指は数えない。`onRefresh` が null の画面では何もしない。
 * 指の動きは `area` で受ける。中の要素が作り直されても、受け口が外れない。
 */
export function usePullToRefresh(area: RefObject<HTMLElement | null>, scroller: RefObject<HTMLElement | null>, onRefresh: (() => unknown) | null) {
  const [pull, setPull] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const latest = useRef(onRefresh);
  latest.current = onRefresh;
  const enabled = onRefresh !== null;

  useEffect(() => {
    const element = area.current;
    if (!element || !enabled) return;
    let start: { x: number; y: number } | null = null;
    let pulling = false;
    let distance = 0;
    let busy = false;
    const reset = () => { start = null; pulling = false; distance = 0; };
    const down = (event: TouchEvent) => {
      reset();
      if (busy || event.touches.length !== 1 || (scroller.current?.scrollTop ?? 0) > 0) return;
      start = { x: event.touches[0].clientX, y: event.touches[0].clientY };
    };
    const move = (event: TouchEvent) => {
      if (!start) return;
      const dy = event.touches[0].clientY - start.y;
      const dx = event.touches[0].clientX - start.x;
      if (!pulling) {
        if (Math.abs(dy) < 8 && Math.abs(dx) < 8) return;
        // 上へ、または横へ動かした指は、ふつうのスクロールとページ送りに任せる。
        if (dy <= 0 || Math.abs(dx) > Math.abs(dy) || (scroller.current?.scrollTop ?? 0) > 0) { reset(); return; }
        pulling = true;
      }
      distance = Math.max(0, Math.min(maxPull, dy / 2));
      setPull(distance);
    };
    const up = () => {
      const fire = pulling && distance >= triggerAt;
      reset();
      if (!fire) { setPull(0); return; }
      busy = true;
      setRefreshing(true);
      const began = Date.now();
      void Promise.resolve()
        .then(() => latest.current?.())
        .catch(() => undefined)
        .then(() => new Promise((resolve) => window.setTimeout(resolve, Math.max(0, minimumSpinMs - (Date.now() - began)))))
        .then(() => { busy = false; setRefreshing(false); setPull(0); });
    };
    const cancel = () => { reset(); if (!busy) setPull(0); };
    element.addEventListener("touchstart", down, { passive: true });
    element.addEventListener("touchmove", move, { passive: true });
    element.addEventListener("touchend", up);
    element.addEventListener("touchcancel", cancel);
    return () => {
      element.removeEventListener("touchstart", down);
      element.removeEventListener("touchmove", move);
      element.removeEventListener("touchend", up);
      element.removeEventListener("touchcancel", cancel);
      setPull(0);
    };
  }, [area, scroller, enabled]);

  return { pull, refreshing };
}

/** 引いている量に合わせて下りてくる丸い印。位置を決める親（position: relative）の上端の中央に出す。 */
export function PullIndicator({ pull, refreshing }: { pull: number; refreshing: boolean }) {
  if (!pull && !refreshing) return null;
  const shown = refreshing ? triggerAt : pull;
  return <div className="pull-indicator" data-ready={pull >= triggerAt || undefined} data-refreshing={refreshing || undefined}
    style={{ "--pull": `${shown}px`, opacity: Math.min(1, shown / triggerAt) } as CSSProperties}
    role={refreshing ? "status" : undefined} aria-label={refreshing ? "読み込み直しています" : undefined}>
    <RefreshCw aria-hidden="true" style={refreshing ? undefined : { transform: `rotate(${shown * 4}deg)` }} />
  </div>;
}
