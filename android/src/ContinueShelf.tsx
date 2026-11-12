import { Icons } from "./icons";
import type { PcWork } from "./pcApi";
import { WorkThumbnail } from "./WorkThumbnail";
import type { RecentWork } from "./recentReading";
import { readingPercent } from "./workText";

/**
 * 本棚の頭に置く「続きを読む」の列。表紙と進み具合だけを並べる。
 *
 * 表紙の横に題名を添えた版は、題名が 2 行で切れて中途半端になり、表紙も小さすぎた。
 * 題名は表紙そのもの（表紙のない作品は表紙に刷った題名）と読み上げの名前に任せる。
 * 横に送れば残りも見られ、すべては右端から読書中の棚へ移る。
 */
export function ContinueShelf({ items, onOpen, onAll }: { items: RecentWork[]; onOpen: (work: PcWork) => void; onAll: () => void }) {
  if (!items.length) return null;
  return <section className="continue-shelf" aria-label="続きを読む">
    <div className="continue-heading"><h2>続きを読む</h2><button type="button" className="continue-all" onClick={onAll}>すべて<Icons.next aria-hidden="true" /></button></div>
    <div className="continue-row">{items.slice(0, 12).map(({ work, progress }) => {
      const percent = readingPercent(progress);
      return <button type="button" className="continue-item" key={work.id} aria-label={`${work.title}の続きを読む（${percent}%）`} title={work.title} onClick={() => onOpen(work)}>
        {/* 進み具合は表紙の中に入れる。下の縁に棒、その上の右隅に数字。表紙の下に並べると、棒と数字の釣り合いが取りにくかった。 */}
        <span className="continue-cover">
          <WorkThumbnail work={work} />
          <small className="continue-percent" aria-hidden="true">{percent}%</small>
          <span className="continue-progress" aria-hidden="true"><span style={{ width: `${percent}%` }} /></span>
        </span>
      </button>;
    })}</div>
  </section>;
}
