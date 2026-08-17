import { useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { EntityCard } from "@/components/EntityCard";
import type { EntityFacet } from "@/types/library";

interface VirtualizedEntityGridProps {
  items: EntityFacet[];
  kind: "person" | "series";
  selectionMode?: boolean;
  /** Keys of the selected entities, as `source:sourceKey`. */
  selected?: Set<string>;
  onSelect?: (entity: EntityFacet, selected: boolean) => void;
}

/** One entity's identity across a list. Facets have no row id of their own. */
export function entityKey(entity: { source: string; sourceKey: string }): string {
  return `${entity.source}:${entity.sourceKey}`;
}

const MIN_COLUMN_WIDTH = 320;
const GRID_GAP = 16;
const INITIAL_RENDER_LIMIT = 90;

export function entityGridColumnCount(width: number): number {
  return Math.max(1, Math.floor((Math.max(0, width) + GRID_GAP) / (MIN_COLUMN_WIDTH + GRID_GAP)));
}

/** Keep only entity rows close to AppFrame's scroll viewport in the DOM. */
export function VirtualizedEntityGrid({ items, kind, selectionMode = false, selected, onSelect }: VirtualizedEntityGridProps) {
  const gridRef = useRef<HTMLDivElement>(null);
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(null);
  const [width, setWidth] = useState(0);
  const [scrollMargin, setScrollMargin] = useState(0);
  const columns = entityGridColumnCount(width);
  const rowCount = Math.ceil(items.length / columns);

  useLayoutEffect(() => {
    const node = gridRef.current;
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
  }, []);

  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollElement,
    estimateSize: () => 154,
    getItemKey: (index) => {
      const entity = items[index * columns];
      return `${columns}:${entity?.source ?? "row"}:${entity?.sourceKey ?? index}`;
    },
    gap: GRID_GAP,
    overscan: 4,
    scrollMargin,
  });

  const renderEntity = (entity: EntityFacet) => (
    <EntityCard
      key={entityKey(entity)}
      entity={entity}
      kind={kind}
      selectionMode={selectionMode}
      selected={selected?.has(entityKey(entity)) ?? false}
      onSelect={onSelect}
    />
  );

  // AppFrame supplies the production viewport. The bounded fallback prevents
  // an empty first paint and remains safe in isolated previews and tests.
  if (!scrollElement || width <= 0) {
    return (
      <div
        ref={gridRef}
        style={{ display: "grid", gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`, gap: GRID_GAP }}
        data-entity-virtualization-pending
      >
        {items.slice(0, INITIAL_RENDER_LIMIT).map(renderEntity)}
      </div>
    );
  }

  return (
    <div ref={gridRef} style={{ position: "relative", height: virtualizer.getTotalSize(), width: "100%" }} data-virtualized-entity-grid>
      {virtualizer.getVirtualItems().map((virtualRow) => (
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
            gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
            gap: GRID_GAP,
            transform: `translateY(${virtualRow.start - scrollMargin}px)`,
          }}
        >
          {items.slice(virtualRow.index * columns, (virtualRow.index + 1) * columns).map(renderEntity)}
        </div>
      ))}
    </div>
  );
}
