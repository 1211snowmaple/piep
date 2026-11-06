import { ChevronRight } from "lucide-react";
import type { PcWork } from "./pcApi";
import { WorkThumbnail } from "./WorkThumbnail";
import type { RecentWork } from "./recentReading";

/**
 * 本棚の頭に置く「続きを読む」の列。一覧が主役なので、表紙は小さく、題名と進み具合を横に添えた
 * 一段だけにする。横に送れば残りも見られ、すべては右端から読書中の棚へ移る。
 */
export function ContinueShelf({ items, onOpen, onAll }: { items: RecentWork[]; onOpen: (work: PcWork) => void; onAll: () => void }) {
  if (!items.length) return null;
  return <section className="continue-shelf" aria-label="続きを読む">
    <div className="continue-heading"><h2>続きを読む</h2><button type="button" className="continue-all" onClick={onAll}>すべて<ChevronRight aria-hidden="true" /></button></div>
    <div className="continue-row">{items.slice(0, 12).map(({ work, progress }) => {
      const percent = Math.min(99, Math.round(progress * 100));
      return <button type="button" className="continue-item" key={work.id} aria-label={`${work.title}の続きを読む（${percent}%）`} onClick={() => onOpen(work)}>
        <span className="continue-cover"><WorkThumbnail work={work} /></span>
        <span className="continue-text">
          <span className="continue-title">{work.title}</span>
          <span className="continue-meta"><span className="continue-progress" aria-hidden="true"><span style={{ width: `${Math.max(3, percent)}%` }} /></span><small>{percent}%</small></span>
        </span>
      </button>;
    })}</div>
  </section>;
}
