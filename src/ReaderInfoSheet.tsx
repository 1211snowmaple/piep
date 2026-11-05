import { useLayoutEffect, useRef, useState } from "react";
import { Check, Download, X } from "lucide-react";
import { ProviderMark } from "./ProviderMark";
import { WorkThumbnail } from "./WorkThumbnail";
import { WorkFacts, workFacts } from "./WorkFacts";
import { AuthorAvatar } from "./WorkCard";
import type { PcCollection, PcOutlineEntry, PcWork } from "./pcApi";
import { useAppNavigation } from "./appHistory";
import { plainText, progressLabel, readingFinished, readingMinutes } from "./workText";

export type InfoTab = "outline" | "info";

export function ReaderInfoSheet({ tab, onTabChange, onClose, work, isEdited, offline, offlineAuthorCover, snapshotGeneration, outline, currentPage, pageCount, progress, collections, saving, saveMessage, onSave, onOutline }: {
  tab: InfoTab;
  onTabChange: (tab: InfoTab) => void;
  onClose: () => void;
  work: PcWork;
  isEdited: boolean;
  offline: boolean;
  offlineAuthorCover: boolean;
  snapshotGeneration?: number;
  outline: PcOutlineEntry[];
  currentPage: number;
  pageCount: number;
  progress: number;
  collections: PcCollection[] | null;
  saving: boolean;
  saveMessage: string | null;
  onSave: () => void;
  onOutline: (entry: PcOutlineEntry) => void;
}) {
  const [descriptionOpen, setDescriptionOpen] = useState(false);
  const [descriptionLong, setDescriptionLong] = useState(false);
  const [titleOpen, setTitleOpen] = useState(false);
  const [titleLong, setTitleLong] = useState(false);
  const descriptionRef = useRef<HTMLParagraphElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const description = plainText(work.excerpt);
  const navigation = useAppNavigation();
  const percent = Math.round(progress * 100);
  const finished = readingFinished(progress, pageCount);
  const remaining = readingMinutes(Math.round(work.textLength * (1 - progress)));
  const hasOutline = outline.length > 0;
  const activeTab = hasOutline ? tab : "info";

  useLayoutEffect(() => {
    if (!descriptionOpen && descriptionRef.current) setDescriptionLong(descriptionRef.current.scrollHeight > descriptionRef.current.clientHeight + 1);
    if (!titleOpen && titleRef.current) setTitleLong(titleRef.current.scrollHeight > titleRef.current.clientHeight + 1);
  }, [activeTab, description, descriptionOpen, titleOpen, work.title]);

  // 同じページに見出しがいくつあっても、読んでいるページの最後のものを今の章として示す。
  const currentEntry = outline.reduce<PcOutlineEntry | null>((found, entry) => entry.page - 1 <= currentPage ? entry : found, null);

  return <div className="reader-sheet-layer">
    <button className="reader-sheet-scrim" type="button" onClick={onClose} aria-label="作品情報を閉じる" />
    <aside className="reader-sheet reader-info-sheet" role="dialog" aria-modal="true" aria-label={hasOutline ? "目次と作品情報" : "作品情報"}>
      <span className="reader-sheet-grab" aria-hidden="true" />
      <header className="reader-sheet-header">
        {hasOutline ? <div className="reader-sheet-tabs" role="tablist" aria-label="表示する内容">
          <button type="button" role="tab" aria-selected={activeTab === "outline"} onClick={() => onTabChange("outline")}>目次</button>
          <button type="button" role="tab" aria-selected={activeTab === "info"} onClick={() => onTabChange("info")}>作品情報</button>
        </div> : <h2>作品情報</h2>}
        <button type="button" className="reader-icon-button" onClick={onClose} aria-label="閉じる"><X aria-hidden="true" /></button>
      </header>
      <div className="reader-sheet-body">
        {activeTab === "outline" ? <ol className="reader-toc">
          {outline.map((entry) => {
            const current = entry === currentEntry;
            return <li key={`${entry.page}:${entry.index}`}><button type="button" aria-current={current ? "true" : undefined} onClick={() => onOutline(entry)}><span className="reader-toc-title">{entry.title}</span>{pageCount > 1 && <span className="reader-toc-page">{entry.page}</span>}</button></li>;
          })}
        </ol> : <>
          <div className="reader-info-hero">
            <span className="reader-info-cover"><WorkThumbnail work={work} offline={offline} snapshotGeneration={snapshotGeneration} variant="detail" /></span>
            <div className="reader-info-heading">
              <div className="reader-info-labels"><ProviderMark source={work.source} /><span>{work.contentType === "novel" ? "小説" : "作品"}</span>{work.favorite && <span>お気に入り</span>}{isEdited && <span>編集版</span>}</div>
              {work.seriesTitle && (navigation.canBrowseServer && work.seriesId
                ? <button type="button" className="reader-info-series" onClick={() => navigation.openEntity("series", work.source, work.seriesId!, work.seriesTitle!)}>{work.seriesTitle}</button>
                : <p className="reader-info-series">{work.seriesTitle}</p>)}
              <h2 ref={titleRef} className={titleOpen ? "expanded" : ""}>{work.title}</h2>
              {(titleLong || titleOpen) && <button type="button" className="reader-more" onClick={() => setTitleOpen((open) => !open)} aria-expanded={titleOpen}>{titleOpen ? "短く表示" : "すべて表示"}</button>}
              {navigation.canBrowseServer && work.authorId
                ? <button type="button" className="reader-info-author" onClick={() => navigation.openEntity("person", work.source, work.authorId, work.authorName)}><AuthorAvatar work={work} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} /><span>{work.authorName}</span></button>
                : <p className="reader-info-author"><AuthorAvatar work={work} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} /><span>{work.authorName}</span></p>}
            </div>
          </div>
          <div className="reader-info-progress">
            <div><strong>{progressLabel(progress, pageCount)}</strong>{pageCount > 1 && <span>{currentPage + 1} / {pageCount} ページ</span>}{remaining && !finished && <span className="reader-info-remaining">残り{remaining}</span>}</div>
            <span className="reader-progress-track" aria-hidden="true"><span style={{ width: `${percent}%` }} /></span>
          </div>
          <WorkFacts items={workFacts(work)} />
          {!offline && <div className="reader-info-actions"><button type="button" className="reader-info-save" disabled={saving} onClick={onSave}>{saveMessage === "端末に保存しました" ? <Check aria-hidden="true" /> : <Download aria-hidden="true" />}{saving ? "保存中…" : "端末に保存"}</button></div>}
          {saveMessage && <p className="reader-info-status" role="status">{saveMessage}</p>}
          {description && <section className="reader-info-section" aria-label="概要">
            <p ref={descriptionRef} className={descriptionOpen ? "reader-info-description expanded" : "reader-info-description"}>{description}</p>
            {(descriptionLong || descriptionOpen) && <button type="button" className="reader-more" onClick={() => setDescriptionOpen((open) => !open)} aria-expanded={descriptionOpen}>{descriptionOpen ? "短く表示" : "続きを読む"}</button>}
          </section>}
          {work.tags.length > 0 && <section className="reader-info-section" aria-label="タグ"><div className="reader-info-tags">{work.tags.map((tag, index) => navigation.canBrowseServer ? <button type="button" key={`${tag}:${index}`} onClick={() => navigation.openTag(tag)}>#{tag}</button> : <span key={`${tag}:${index}`}>#{tag}</span>)}</div></section>}
          {collections && collections.length > 0 && <section className="reader-info-section"><h3>コレクション</h3><div className="reader-info-collections">{collections.map((collection) => <span key={collection.id}>{collection.name}</span>)}</div></section>}
        </>}
      </div>
    </aside>
  </div>;
}
