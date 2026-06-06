import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { cn } from "@/lib/utils";
import type { LibraryViewMode } from "@/types/library";

const DOWNLOAD_GRID_GAP = 16;

interface LibraryWorkGridProps<T> {
  items: T[];
  scrollSelector: string;
  renderItem: (item: T) => ReactNode;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  viewMode: LibraryViewMode;
}

export function LibraryWorkGrid<T>({
  items,
  scrollSelector,
  renderItem,
  hasMore,
  loadingMore,
  onLoadMore,
  viewMode,
}: LibraryWorkGridProps<T>) {
  const containerRef = useRef<HTMLDivElement>(null);
  const lastRequestedLengthRef = useRef(-1);

  const isCompact = viewMode === "compact" || viewMode === "epubSelection" || viewMode === "updateReview";
  const cardWidth = isCompact ? 338 : 220;
  const estimateSize = isCompact ? 134 : 360;

  const [containerWidth, setContainerWidth] = useState(cardWidth);

  useEffect(() => {
    const node = containerRef.current;
    if (!node) return;
    const updateWidth = () => setContainerWidth(Math.max(cardWidth, node.clientWidth));
    updateWidth();
    const observer = new ResizeObserver(updateWidth);
    observer.observe(node);
    return () => observer.disconnect();
  }, [cardWidth]);

  const columnCount = useMemo(() => {
    return Math.max(1, Math.floor((containerWidth + DOWNLOAD_GRID_GAP) / (cardWidth + DOWNLOAD_GRID_GAP)));
  }, [containerWidth, cardWidth]);

  const rowCount = Math.ceil(items.length / columnCount);
  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => document.querySelector(scrollSelector) as HTMLElement | null,
    estimateSize: () => estimateSize,
    overscan: 5,
  });
  const virtualRows = rowVirtualizer.getVirtualItems();

  useEffect(() => {
    const lastRow = virtualRows[virtualRows.length - 1];
    if (!lastRow || !hasMore || loadingMore) return;
    if (lastRow.index >= rowCount - 3 && lastRequestedLengthRef.current !== items.length) {
      lastRequestedLengthRef.current = items.length;
      onLoadMore();
    }
  }, [hasMore, items.length, loadingMore, onLoadMore, rowCount, virtualRows]);

  useEffect(() => {
    lastRequestedLengthRef.current = -1;
    rowVirtualizer.scrollToIndex(0, { align: "start" });
  }, [items, rowVirtualizer]);

  useEffect(() => {
    rowVirtualizer.measure();
  }, [columnCount, rowVirtualizer]);

  useEffect(() => {
    const resize = () => rowVirtualizer.measure();
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, [rowVirtualizer]);

  useEffect(() => {
    const scrollParent = document.querySelector(scrollSelector);
    if (!scrollParent) return;
    const onScroll = () => rowVirtualizer.measure();
    scrollParent.addEventListener("scroll", onScroll, { passive: true });
    return () => scrollParent.removeEventListener("scroll", onScroll);
  }, [rowVirtualizer, scrollSelector]);

  return (
    <div ref={containerRef} className={cn("virtual-work-grid", `view-${viewMode}`)}>
      <div className="virtual-work-grid-spacer" style={{ height: rowVirtualizer.getTotalSize() }}>
        {virtualRows.map(virtualRow => {
          const start = virtualRow.index * columnCount;
          const rowItems = items.slice(start, start + columnCount);
          return (
            <div
              key={virtualRow.key}
              ref={rowVirtualizer.measureElement}
              data-index={virtualRow.index}
              className="virtual-work-grid-row"
              style={{
                transform: `translateY(${virtualRow.start}px)`,
                gridTemplateColumns: isCompact
                  ? `repeat(${columnCount}, minmax(0, 1fr))`
                  : `repeat(${columnCount}, ${cardWidth}px)`,
              }}
            >
              {rowItems.map(renderItem)}
            </div>
          );
        })}
      </div>
    </div>
  );
}
