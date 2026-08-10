import { useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { WorkCard } from "@/components/WorkCard";
import type { DownloadEntry } from "@/types/library";

type WorkListView = "gallery" | "compact";

interface VirtualizedWorkListProps {
  items: DownloadEntry[];
  view: WorkListView;
  selectionMode?: boolean;
  selected?: ReadonlySet<number>;
  onSelect?: (id: number, selected: boolean) => void;
  onToggleFavorite?: (id: number, favorite: boolean) => void;
  onToggleWatch?: (id: number, watch: boolean) => void;
}

const GALLERY_MIN_WIDTH = 360;
const GALLERY_GAP = 16;
const LIST_GAP = 10;
const INITIAL_RENDER_LIMIT = 120;

export function libraryColumnCount(width: number, view: WorkListView): number {
  if (view === "compact") return 1;
  return Math.max(1, Math.floor((Math.max(0, width) + GALLERY_GAP) / (GALLERY_MIN_WIDTH + GALLERY_GAP)));
}

/**
 * Only rows close to the AppFrame scroll viewport are mounted. The query can
 * retain every fetched item for selection and cursor continuity without
 * growing the WebView DOM to tens of thousands of cards.
 */
export function VirtualizedWorkList({
  items,
  view,
  selectionMode = false,
  selected,
  onSelect,
  onToggleFavorite,
  onToggleWatch,
}: VirtualizedWorkListProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(null);
  const [width, setWidth] = useState(0);
  const [scrollMargin, setScrollMargin] = useState(0);
  const columns = libraryColumnCount(width, view);
  const rowCount = Math.ceil(items.length / columns);

  useLayoutEffect(() => {
    const node = listRef.current;
    if (!node) return;
    const scroller = node.closest<HTMLElement>(".app-main");
    setScrollElement(scroller);
    const updateGeometry = () => {
      setWidth((current) => current === node.clientWidth ? current : node.clientWidth);
      if (scroller) {
        const nextMargin = node.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
        setScrollMargin((current) => Math.abs(current - nextMargin) < 0.5 ? current : nextMargin);
      }
    };
    updateGeometry();
    const observer = new ResizeObserver(updateGeometry);
    observer.observe(node);
    if (scroller) observer.observe(scroller);
    return () => observer.disconnect();
  }, [view]);

  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollElement,
    estimateSize: () => view === "compact" ? 92 : 238,
    getItemKey: (index) => `${columns}:${items[index * columns]?.id ?? index}`,
    gap: view === "compact" ? LIST_GAP : GALLERY_GAP,
    overscan: view === "compact" ? 8 : 3,
    scrollMargin,
  });
  const virtualRows = virtualizer.getVirtualItems();

  const renderWork = (work: DownloadEntry) => (
    <WorkCard
      key={work.id}
      compact={view === "compact"}
      work={work}
      selectionMode={selectionMode}
      selected={selected?.has(work.id)}
      onSelect={onSelect}
      onToggleFavorite={onToggleFavorite}
      onToggleWatch={onToggleWatch}
    />
  );

  // AppFrame supplies the scroll container in production. Keeping a bounded
  // regular layout until it is discovered avoids an empty first paint and
  // supports isolated component/test rendering without a synthetic viewport.
  if (!scrollElement || width <= 0) {
    return <div ref={listRef} className={view === "gallery" ? "library-grid" : undefined} style={view === "compact" ? { display: "grid", gap: LIST_GAP } : undefined} data-virtualization-pending>{items.slice(0, INITIAL_RENDER_LIMIT).map(renderWork)}</div>;
  }

  return (
    <div ref={listRef} style={{ position: "relative", height: virtualizer.getTotalSize(), width: "100%" }} data-virtualized-work-list>
      {virtualRows.map((virtualRow) => (
        <div
          key={virtualRow.key}
          ref={virtualizer.measureElement}
          data-index={virtualRow.index}
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: "100%",
            display: "grid",
            gridTemplateColumns: view === "gallery" ? `repeat(${columns}, minmax(0, 1fr))` : "minmax(0, 1fr)",
            gap: GALLERY_GAP,
            transform: `translateY(${virtualRow.start - scrollMargin}px)`,
          }}
        >
          {items.slice(virtualRow.index * columns, (virtualRow.index + 1) * columns).map(renderWork)}
        </div>
      ))}
    </div>
  );
}
