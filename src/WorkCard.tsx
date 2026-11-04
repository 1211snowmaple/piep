import { useEffect, useMemo, useState, type MouseEvent } from "react";
import { CalendarDays, Clock3, Download, FileText, History, Images, Layers, MoreHorizontal, UserRound } from "lucide-react";
import type { PcWork } from "./pcApi";
import { WorkThumbnail } from "./WorkThumbnail";
import { ProviderMark } from "./ProviderMark";
import { MobileTagRow } from "./MobileTagRow";
import { entityCoverUrl, imageRecentlyFailed, savedEntityCoverUrl, SmoothImage } from "./images";
import { useAppNavigation } from "./appHistory";
import { displayDate, plainText, readingMinutes } from "./workText";

export function AuthorAvatar({ work, offline, offlineAuthorCover, snapshotGeneration }: { work: PcWork; offline: boolean; offlineAuthorCover: boolean; snapshotGeneration?: number }) {
  const src = offline
    ? offlineAuthorCover ? savedEntityCoverUrl(work.id, "people", snapshotGeneration ?? 0) : null
    : work.authorId ? entityCoverUrl("people", work.source, work.authorId) : null;
  const [failed, setFailed] = useState(() => imageRecentlyFailed(src));
  useEffect(() => { setFailed(imageRecentlyFailed(src)); }, [src]);
  return <span className="work-author-avatar" aria-hidden="true"><UserRound />{src && !failed && <SmoothImage src={src} alt="" onFailed={() => setFailed(true)} />}</span>;
}

/** カードの余白を押したら読む。作者やタグなど中の操作はそれぞれの行き先を優先する。 */
function isCardControl(target: EventTarget): boolean {
  return target instanceof Element && target.closest("button, a, input, select, summary") !== null;
}

/**
 * 情報の量はカード自身の幅で決める。画面の幅で決めると、タブレットの 2 列と携帯の 1 列で
 * 同じ幅でも見え方が変わってしまう。優先度の順に並べ、入らない項目は末尾から落ちる。
 * （段落ちの仕組みはデスクトップ版の .work-card__facts と同じ。CSS を参照）
 */
export function WorkCard({ work, onRead, onDetail, offline = false, offlineAuthorCover = false, snapshotGeneration, savedState, prefix, omit }: {
  work: PcWork;
  onRead: () => void;
  onDetail: () => void;
  offline?: boolean;
  offlineAuthorCover?: boolean;
  snapshotGeneration?: number;
  savedState?: { label: string; changed: boolean } | null;
  prefix?: string;
  /** 作者やシリーズの画面では、その名前を全カードで繰り返さない。 */
  omit?: "author" | "series";
}) {
  const navigation = useAppNavigation();
  const browsable = navigation.canBrowseServer;
  const date = displayDate(work.sourceCreatedAt);
  const savedDate = displayDate(work.downloadedAt);
  const minutes = readingMinutes(work.textLength);
  const excerpt = useMemo(() => plainText(work.excerpt).replace(/\s+/g, " "), [work.excerpt]);
  const authorInner = <><AuthorAvatar work={work} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} /><span>{work.authorName}</span></>;

  return <article className="work-card" onClick={(event: MouseEvent) => { if (!isCardControl(event.target)) onRead(); }}>
    <div className="work-card-inner">
      <button className="work-card-cover" type="button" tabIndex={-1} aria-label={`${work.title}を読む`} onClick={onRead}><WorkThumbnail work={work} offline={offline} snapshotGeneration={snapshotGeneration} /></button>
      <div className="work-card-body">
        {omit !== "series" && work.seriesTitle && (browsable && work.seriesId
          ? <button className="work-card-series" type="button" aria-label={`シリーズ ${work.seriesTitle} を開く`} onClick={() => navigation.openEntity("series", work.source, work.seriesId!, work.seriesTitle!)}><Layers aria-hidden="true" /><span>{work.seriesTitle}</span></button>
          : <span className="work-card-series"><Layers aria-hidden="true" /><span>{work.seriesTitle}</span></span>)}
        <button className="work-card-title" type="button" onClick={onRead} aria-label={`${work.title}を読む`}>{work.title}</button>
        {omit !== "author" && <div className="work-card-identity">
          {browsable && work.authorId
            ? <button className="work-card-author" type="button" aria-label={`作者 ${work.authorName} を開く`} onClick={() => navigation.openEntity("person", work.source, work.authorId, work.authorName)}>{authorInner}</button>
            : <span className="work-card-author">{authorInner}</span>}
          <span className="work-card-provider"><ProviderMark source={work.source} compact /></span>
          <span className="work-card-provider-wide"><ProviderMark source={work.source} /></span>
        </div>}
        {excerpt && <p className="work-card-excerpt">{excerpt}</p>}
        <MobileTagRow tags={work.tags ?? []} onTag={browsable ? navigation.openTag : undefined} />
      </div>
      <footer className="work-card-footer">
        <div className="work-card-facts">
          {prefix && <span className="work-meta">{prefix}</span>}
          {date && <span className="work-meta" title="公開日"><CalendarDays aria-hidden="true" />{date}</span>}
          <span className="work-meta" title="文字数"><FileText aria-hidden="true" />{work.textLength.toLocaleString("ja-JP")} 字</span>
          {work.assetCount > 0 && <span className="work-meta" title="画像"><Images aria-hidden="true" />{work.assetCount.toLocaleString("ja-JP")}</span>}
          {minutes && <span className="work-meta work-meta--wide" title="読む時間の目安"><Clock3 aria-hidden="true" />{minutes}</span>}
          {savedDate && <span className="work-meta work-meta--wide" title="保存日"><Download aria-hidden="true" />{savedDate}</span>}
          {work.currentVersion > 1 && <span className="work-meta work-meta--wide" title="版"><History aria-hidden="true" />{work.currentVersion} 版</span>}
        </div>
        {savedState && <span className={savedState.changed ? "saved-state changed" : "saved-state"}>{savedState.label}</span>}
        <button className="work-card-detail" type="button" onClick={onDetail} aria-label={`${work.title}の詳細`}><MoreHorizontal aria-hidden="true" /><span>詳細</span></button>
      </footer>
    </div>
  </article>;
}

export function WorkCardSkeleton() {
  return <div className="work-card work-card--skeleton" aria-hidden="true"><div className="work-card-inner">
    <span className="work-card-cover"><span className="skeleton-block skeleton-cover" /></span>
    <div className="work-card-body"><span className="skeleton-block skeleton-line skeleton-line--title" /><span className="skeleton-block skeleton-line skeleton-line--short" /><span className="skeleton-block skeleton-line" /><span className="skeleton-block skeleton-line skeleton-line--tags" /></div>
    <div className="work-card-footer"><span className="skeleton-block skeleton-line skeleton-line--short" /></div>
  </div></div>;
}
