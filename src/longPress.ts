import { useEffect, useRef, type PointerEvent as ReactPointerEvent } from "react";

const holdMs = 450;
const slopPx = 8;

/**
 * 長押しで別の行き先を開く。指が動いたら（一覧を送っている）取りやめる。
 *
 * 長押しのあとに来るクリックは、どこに落ちても読み捨てる。
 * 開いた詳細の背景や、指の下に残ったカードの操作を押したことにしないため。
 * 文字の選択や画像の保存メニューも出さない。
 */
export function useLongPress(onLongPress: () => void) {
  const timer = useRef<number | null>(null);
  const origin = useRef<{ x: number; y: number } | null>(null);
  const action = useRef(onLongPress);
  action.current = onLongPress;
  const release = useRef<(() => void) | null>(null);

  const cancel = () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
    origin.current = null;
  };

  useEffect(() => () => { cancel(); release.current?.(); }, []);

  const swallowNextClick = () => {
    release.current?.();
    const swallow = (event: Event) => { event.preventDefault(); event.stopPropagation(); stop(); };
    // 指を離すまでは、開いた画面の文字を選んだり、そこでメニューを出したりしない。
    // 長押しの続きが、指の下に現れた詳細の概要に届いてしまうため。
    const hold = (event: Event) => { event.preventDefault(); };
    let after: number | null = null;
    const later = (ms: number) => { if (after !== null) window.clearTimeout(after); after = window.setTimeout(stop, ms); };
    // 離したら選択の禁止は解く。クリックの読み捨ては、離した直後の 1 回のためだけに少し残す。
    const lifted = () => {
      window.removeEventListener("selectstart", hold, true);
      window.removeEventListener("contextmenu", hold, true);
      later(700);
    };
    // 端末が長押しを自分の操作として引き取ると、離したことが届かない。そのときは時間で解く。
    const taken = () => later(2000);
    // 次に触れたら、それはもう新しい操作。
    const next = () => stop();
    function stop() {
      window.removeEventListener("click", swallow, true);
      window.removeEventListener("selectstart", hold, true);
      window.removeEventListener("contextmenu", hold, true);
      window.removeEventListener("pointerup", lifted, true);
      window.removeEventListener("pointercancel", taken, true);
      window.removeEventListener("pointerdown", next, true);
      if (after !== null) window.clearTimeout(after);
      release.current = null;
    }
    window.addEventListener("click", swallow, true);
    window.addEventListener("selectstart", hold, true);
    window.addEventListener("contextmenu", hold, true);
    window.addEventListener("pointerup", lifted, true);
    window.addEventListener("pointercancel", taken, true);
    window.addEventListener("pointerdown", next, true);
    release.current = stop;
  };

  return {
    onPointerDown(event: ReactPointerEvent) {
      if (event.pointerType === "mouse" && event.button !== 0) return;
      release.current?.();
      cancel();
      origin.current = { x: event.clientX, y: event.clientY };
      timer.current = window.setTimeout(() => {
        timer.current = null;
        origin.current = null;
        swallowNextClick();
        navigator.vibrate?.(10);
        action.current();
      }, holdMs);
    },
    onPointerMove(event: ReactPointerEvent) {
      const start = origin.current;
      if (start && Math.hypot(event.clientX - start.x, event.clientY - start.y) > slopPx) cancel();
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    onContextMenu(event: { preventDefault: () => void }) { event.preventDefault(); },
  };
}
