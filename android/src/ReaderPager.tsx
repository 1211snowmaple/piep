import { Icons } from "./icons";
import { useEffect, useState } from "react";

/**
 * 番号の並びを作る。先頭と末尾は常に出し、離れたところは省略する。
 * 端の近くでは余った枠を隣の番号に回し、いつも同じ数のボタンを並べる。ページを送っても他のボタンが動かないようにするため。
 */
export function pageButtonItems(current: number, total: number, siblings: number): (number | "gap-start" | "gap-end")[] {
  if (total <= siblings * 2 + 5) return Array.from({ length: total }, (_, index) => index + 1);
  const start = Math.max(Math.min(current - siblings, total - 2 - siblings * 2), 3);
  const end = Math.min(Math.max(current + siblings, 3 + siblings * 2), total - 2);
  const items: (number | "gap-start" | "gap-end")[] = [1];
  items.push(start > 3 ? "gap-start" : 2);
  for (let page = start; page <= end; page++) items.push(page);
  items.push(end < total - 2 ? "gap-end" : total - 1);
  items.push(total);
  return items;
}

function useWideScreen(): boolean {
  const [wide, setWide] = useState(() => window.matchMedia("(min-width: 600px)").matches);
  useEffect(() => {
    const query = window.matchMedia("(min-width: 600px)");
    const update = () => setWide(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return wide;
}

/** デスクトップ版と同じ、番号が並ぶページボタン。 */
export function ReaderPager({ page, pageCount, disabled, onChange }: { page: number; pageCount: number; disabled: boolean; onChange: (page: number) => void }) {
  const wide = useWideScreen();
  const [jumpOpen, setJumpOpen] = useState(false);
  const [jumpValue, setJumpValue] = useState(String(page + 1));
  useEffect(() => { setJumpValue(String(page + 1)); }, [page]);
  const current = page + 1;
  const go = (next: number) => { if (next >= 1 && next <= pageCount && next !== current) onChange(next - 1); };
  const submitJump = () => {
    const value = Number(jumpValue);
    if (Number.isSafeInteger(value)) go(Math.min(pageCount, Math.max(1, value)));
    setJumpOpen(false);
  };
  return <nav className="reader-pager" aria-label="ページの移動">
    {wide && <button type="button" disabled={disabled || current <= 1} onClick={() => go(1)} aria-label="最初のページ"><Icons.skipBack aria-hidden="true" /></button>}
    <button type="button" disabled={disabled || current <= 1} onClick={() => go(current - 1)} aria-label="前のページ"><Icons.previous aria-hidden="true" /></button>
    {pageButtonItems(current, pageCount, wide ? 2 : 0).map((item) => typeof item === "number"
      ? <button key={item} type="button" disabled={disabled} aria-current={item === current ? "page" : undefined} aria-label={`${item} ページ`} onClick={() => { if (item === current) setJumpOpen((open) => !open); else go(item); }}>{item}</button>
      : <button key={item} type="button" className="reader-pager-gap" onClick={() => setJumpOpen((open) => !open)} aria-label="ページ番号を指定">…</button>)}
    <button type="button" disabled={disabled || current >= pageCount} onClick={() => go(current + 1)} aria-label="次のページ"><Icons.next aria-hidden="true" /></button>
    {wide && <button type="button" disabled={disabled || current >= pageCount} onClick={() => go(pageCount)} aria-label="最後のページ"><Icons.skipForward aria-hidden="true" /></button>}
    {jumpOpen && <form className="reader-pager-jump" onSubmit={(event) => { event.preventDefault(); submitJump(); }}>
      <label>移動先のページ<input type="number" inputMode="numeric" min={1} max={pageCount} value={jumpValue} onChange={(event) => setJumpValue(event.target.value)} autoFocus onFocus={(event) => event.target.select()} /></label>
      <span>/ {pageCount}</span>
      <button type="submit">移動</button>
    </form>}
  </nav>;
}
