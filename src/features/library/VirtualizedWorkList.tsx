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
  revisedIds?: ReadonlySet<number>;
}

// Entity pages keep a shared author/series detail rail. At the default desktop
// width their work column is about 680px, so 360px cards could never form the
// two-column gallery the switch promises. 328px still leaves the 104px cover
// and five footer actions intact, while making two columns fit at 672px.
const GALLERY_MIN_WIDTH = 328;
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
/**
 * 一覧の幅を、外れているあいだも覚えておく。
 *
 * タブを切り替えると仮想リストは丸ごと外れて付け直される。付け直した最初の
 * 描画では幅がまだ 0 なので、**高さ 0 の空div**を返していた。そこでスクロール
 * 容器の高さが一瞬つぶれ、ブラウザは scrollTop を 0 へ切り詰める。戻ってきた
 * ときに読み直す位置は、その切り詰められた 0 である。
 *
 * 幅は「この一覧の状態」ではなく「いまの画面の形」なので、部品が外れても
 * 失う理由がない。覚えておけば、付け直した一枚目から本物の行を出せる。
 * 窓の大きさが変わっていたら、直後の layout effect が測り直して直す。
 */
const lastKnownWidth = new Map<string, number>();

export function VirtualizedWorkList({
  items,
  view,
  selectionMode = false,
  selected,
  onSelect,
  onToggleFavorite,
  onToggleWatch,
  revisedIds,
}: VirtualizedWorkListProps) {
  const listRef = useRef<HTMLDivElement>(null);
  // AppFrame is already mounted before a route child renders. Discovering its
  // viewport here avoids first constructing up to 120 real cards only to tear
  // them down in the following layout effect when virtualization activates.
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(() =>
    document.getElementById("main-content"),
  );
  const [width, setWidth] = useState(() => lastKnownWidth.get(view) ?? 0);
  const [scrollMargin, setScrollMargin] = useState(0);
  const columns = libraryColumnCount(width, view);
  const rowCount = Math.ceil(items.length / columns);

  useLayoutEffect(() => {
    const node = listRef.current;
    if (!node) return;
    const scroller = node.closest<HTMLElement>(".app-main");
    setScrollElement(scroller);
    const updateGeometry = () => {
      const measured = node.clientWidth;
      // 0 は「幅が無い」ではなく「まだ測れていない」。タブを隠したときや
      // 付け直した直後に ResizeObserver が 0 を報せることがあり、それを
      // 採ると一覧が高さ 0 につぶれ、スクロール位置が切り詰められる。
      // 一度測った幅がある間は、0 で上書きしない。
      if (measured <= 0) return;
      lastKnownWidth.set(view, measured);
      setWidth((current) => current === measured ? current : measured);
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
    // The viewport is the whole page, not a box of our own, and attaching to it
    // scrolls it to whatever offset we claim to be at. Left at its default that
    // claim is zero, so opening this list - a tab, a filter, the back button -
    // threw the page to the top under the reader. Where they already are is the
    // only honest answer. Staying disabled until the viewport exists is what
    // makes that answer available: the offset is settled on first use and kept,
    // and the first use would otherwise come a render too early, when there is
    // nothing to read a position from and zero is all it could conclude.
    enabled: Boolean(scrollElement),
    initialOffset: () => scrollElement?.scrollTop ?? 0,
    // 実測値。行の下限を変えたらここも合わせる — 見積もりが小さいと、
    // 実寸を測るまで全体の高さを短く言い、遷移の途中でつまみが跳ねる。
    estimateSize: () => view === "compact" ? 173 : 238,
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
      revised={revisedIds?.has(work.id)}
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
  if (scrollElement && width <= 0) {
    return <div ref={listRef} data-virtualization-measuring />;
  }
  if (!scrollElement) {
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
