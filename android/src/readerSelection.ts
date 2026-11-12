import { useEffect, useRef, useState, type PointerEvent } from "react";

/** 短いタップ・スクロールを選択にしない。止めて長押ししたときは OS の選択へ渡す。 */
export function useReaderSelection() {
  const [enabled, setEnabled] = useState(false);
  const hold = useRef<{ id: number; x: number; y: number } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const armed = useRef(false);
  const root = useRef<HTMLElement | null>(null);
  const selected = () => {
    const selection = window.getSelection();
    return Boolean(selection && !selection.isCollapsed && selection.anchorNode && root.current?.contains(selection.anchorNode));
  };
  const cancel = () => { if (timer.current !== null) clearTimeout(timer.current); timer.current = null; };
  useEffect(() => {
    const changed = () => { if (!hold.current && !selected()) { armed.current = false; setEnabled(false); } };
    document.addEventListener("selectionchange", changed);
    return () => { cancel(); document.removeEventListener("selectionchange", changed); if (selected()) window.getSelection()?.removeAllRanges(); };
  }, []);
  return {
    enabled,
    selecting: () => armed.current || selected(),
    handlers: {
      onPointerDown: (event: PointerEvent<HTMLElement>) => {
        cancel();
        root.current = event.currentTarget;
        if (selected()) return;
        armed.current = false;
        if (event.pointerType === "mouse") { setEnabled(true); return; }
        setEnabled(false);
        if (!event.isPrimary || (event.target as Element).closest("a, button, input")) return;
        hold.current = { id: event.pointerId, x: event.clientX, y: event.clientY };
        // Android が長押しを認識する前に許可する。選択範囲・ハンドル・検索は OS に任せる。
        timer.current = setTimeout(() => { timer.current = null; armed.current = true; setEnabled(true); }, 280);
      },
      onPointerMove: (event: PointerEvent<HTMLElement>) => {
        const start = hold.current;
        if (!start || start.id !== event.pointerId) return;
        if (Math.hypot(event.clientX - start.x, event.clientY - start.y) > 8 && !selected()) {
          cancel(); hold.current = null; armed.current = false; setEnabled(false);
        }
      },
      onPointerUp: () => { cancel(); hold.current = null; if (!armed.current && !selected()) setEnabled(false); },
      onPointerCancel: () => { cancel(); hold.current = null; if (!selected()) { armed.current = false; setEnabled(false); } },
    },
  };
}
