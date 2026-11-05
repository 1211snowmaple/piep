import { ChevronRight } from "lucide-react";
import type { PcWork } from "./pcApi";
import { WorkThumbnail } from "./WorkThumbnail";
import type { RecentWork } from "./recentReading";

/** 本棚の頭に置く「続きを読む」の列。横に送り、一覧を押し下げすぎないよう一段に収める。 */
export function ContinueShelf({ items, onOpen, onAll }: { items: RecentWork[]; onOpen: (work: PcWork) => void; onAll: () => void }) {
  if (!items.length) return null;
  return <section className="continue-shelf" aria-label="続きを読む">
    <div className="continue-heading"><h2>続きを読む</h2><button type="button" className="quiet-button" onClick={onAll}>すべて<ChevronRight aria-hidden="true" /></button></div>
    <div className="continue-row">{items.slice(0, 12).map(({ work, progress }) => <button type="button" className="continue-item" key={work.id} aria-label={`${work.title}の続きを読む（${Math.round(progress * 100)}%）`} onClick={() => onOpen(work)}>
      <WorkThumbnail work={work} />
      <span className="continue-progress" aria-hidden="true"><span style={{ width: `${Math.max(3, Math.round(progress * 100))}%` }} /></span>
      <span className="continue-title">{work.title}</span>
    </button>)}</div>
  </section>;
}
