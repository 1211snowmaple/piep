import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

const gap = 4;

// デスクトップ版 TagRow と同じく、固定件数ではなく実際の横幅で表示数を決める。
export function MobileTagRow({ tags, onTag }: { tags: string[]; onTag?: (tag: string) => void }) {
  const rowRef = useRef<HTMLSpanElement>(null);
  const [visibleCount, setVisibleCount] = useState(tags.length);

  const measure = useCallback(() => {
    const row = rowRef.current;
    if (!row || row.clientWidth <= 0) return;
    const items = [...row.querySelectorAll<HTMLElement>("[data-tag]")];
    const overflow = row.querySelector<HTMLElement>("[data-tag-overflow]");
    row.removeAttribute("data-overflow");
    items.forEach((item) => item.removeAttribute("data-hidden"));
    overflow?.removeAttribute("data-hidden");
    const naturalWidth = items.reduce((width, item, index) => width + item.offsetWidth + (index ? gap : 0), 0);
    if (naturalWidth <= row.clientWidth) {
      overflow?.setAttribute("data-hidden", "");
      setVisibleCount(items.length);
      return;
    }
    // 長い先頭タグは省略時の幅で測る。自然幅だけで判定すると表示可能なタグまで隠れる。
    row.setAttribute("data-overflow", "");
    let used = 0;
    let fits = 0;
    for (const [index, item] of items.slice(0, -1).entries()) {
      const next = used + (index ? gap : 0) + item.offsetWidth;
      // +9 と +10 では実際の幅が違うため、候補ごとの残件数を表示して測る。
      if (overflow) overflow.textContent = `+${items.length - index - 1}`;
      const overflowWidth = overflow ? overflow.offsetWidth + gap : 0;
      if (next + overflowWidth > row.clientWidth) break;
      used = next;
      fits += 1;
    }
    let count = Math.max(1, fits);
    const show = () => {
      items.forEach((item, index) => item.toggleAttribute("data-hidden", index >= count));
      if (overflow) overflow.textContent = `+${items.length - count}`;
      overflow?.toggleAttribute("data-hidden", count >= items.length);
      row.toggleAttribute("data-overflow", count < items.length);
    };
    show();
    while (count > 1 && row.scrollWidth > row.clientWidth + 1) {
      count -= 1;
      show();
    }
    setVisibleCount(count);
  }, [tags]);

  useLayoutEffect(measure, [measure]);
  useEffect(() => {
    const row = rowRef.current;
    if (!row) return;
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    let alive = true;
    document.fonts?.ready.then(() => { if (alive) measure(); }).catch(() => undefined);
    return () => { alive = false; observer.disconnect(); };
  }, [measure]);

  if (!tags.length) return null;
  return <span className="work-tags" ref={rowRef} data-overflow={visibleCount < tags.length || undefined}>
    {tags.map((tag, index) => onTag
      ? <button type="button" className="work-tag" data-tag data-hidden={index >= visibleCount || undefined} key={`${tag}:${index}`} onClick={() => onTag(tag)}>#{tag}</button>
      : <span className="work-tag" data-tag data-hidden={index >= visibleCount || undefined} key={`${tag}:${index}`}>#{tag}</span>)}
    {tags.length > 1 && <span className="work-tag-overflow" data-tag-overflow data-hidden={visibleCount >= tags.length || undefined}>+{tags.length - visibleCount}</span>}
  </span>;
}
