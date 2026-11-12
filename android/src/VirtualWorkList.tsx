import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";

// 測った行の高さを一覧ごとに覚える。戻ったときに同じ高さで並び直し、位置がずれないようにする。
const measured = new Map<string, VirtualItem[]>();
const maxRememberedLists = 30;

/** 画面の近くにある行だけを置く。列数は一覧自身の幅で決める。 */
export function VirtualWorkList<T>({ items, itemKey, renderItem, listKey, minColumn = 360, estimatedHeight = 190 }: {
  items: T[];
  itemKey: (item: T) => string | number;
  renderItem: (item: T) => ReactNode;
  /** 同じ一覧を指す名前。戻ったときの高さの復元に使う。 */
  listKey?: string;
  minColumn?: number;
  estimatedHeight?: number;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const [scroller, setScroller] = useState<HTMLElement | null>(() => document.querySelector<HTMLElement>(".main-content"));
  const [width, setWidth] = useState(0);
  const [scrollMargin, setScrollMargin] = useState(0);
  const gap = width > 0 && width < 600 ? 10 : 14;
  const columns = Math.max(1, Math.floor((width + gap) / (minColumn + gap)));
  const memoryKey = listKey ? `${listKey}:${columns}` : null;

  useLayoutEffect(() => {
    const node = listRef.current;
    if (!node) return;
    const scrollNode = node.closest<HTMLElement>(".main-content");
    setScroller(scrollNode);
    const measure = () => {
      if (node.clientWidth > 0) setWidth((old) => old === node.clientWidth ? old : node.clientWidth);
      if (scrollNode) {
        const margin = node.getBoundingClientRect().top - scrollNode.getBoundingClientRect().top + scrollNode.scrollTop;
        setScrollMargin((old) => Math.abs(old - margin) < 0.5 ? old : margin);
      }
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    if (scrollNode) observer.observe(scrollNode);
    return () => observer.disconnect();
  }, []);

  const virtualizer = useVirtualizer({
    count: Math.ceil(items.length / columns),
    getScrollElement: () => scroller,
    enabled: Boolean(scroller),
    initialOffset: () => scroller?.scrollTop ?? 0,
    initialMeasurementsCache: memoryKey ? measured.get(memoryKey) : undefined,
    estimateSize: () => estimatedHeight,
    getItemKey: (index) => `${columns}:${items[index * columns] ? itemKey(items[index * columns]) : index}`,
    gap,
    overscan: 3,
    scrollMargin,
  });

  const latest = useRef({ memoryKey, virtualizer });
  latest.current = { memoryKey, virtualizer };
  useEffect(() => () => {
    const { memoryKey: key, virtualizer: list } = latest.current;
    if (!key) return;
    measured.delete(key);
    measured.set(key, list.measurementsCache);
    if (measured.size > maxRememberedLists) measured.delete(measured.keys().next().value!);
  }, []);

  // 幅は最初の 1 回の描画まで分からない。件数を抑えて先に見せる。
  if (!scroller || width <= 0) {
    return <div className="work-list" ref={listRef}>{items.slice(0, 12).map((item) => <div key={itemKey(item)}>{renderItem(item)}</div>)}</div>;
  }

  return <div ref={listRef} className="virtual-work-list" style={{ height: virtualizer.getTotalSize() }}>
    {virtualizer.getVirtualItems().map((row) => <div
      className="virtual-work-row"
      key={row.key}
      ref={virtualizer.measureElement}
      data-index={row.index}
      style={{ gap, gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`, transform: `translateY(${row.start - scrollMargin}px)` }}
    >{items.slice(row.index * columns, (row.index + 1) * columns).map((item) => <div className="virtual-work-cell" key={itemKey(item)}>{renderItem(item)}</div>)}</div>)}
  </div>;
}
