import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { BookOpen, CalendarDays, Check, ChevronLeft, ChevronRight, Clock3, Download, FileText, History, Images, Layers, MoreHorizontal, RefreshCw, Trash2 } from "lucide-react";
import { AuthorAvatar } from "./WorkCard";
import { ProviderMark } from "./ProviderMark";
import { WorkThumbnail } from "./WorkThumbnail";
import { defaultPcSearch, deleteOfflineWork, getPcMetadata, getPcWorkCollections, listOfflineWorks, pcErrorMessage, saveOfflineWork, type OfflineWork, type PcCollection, type PcWork } from "./pcApi";
import { homePage, useAppNavigation } from "./appHistory";
import { hasReadingPosition } from "./OnlineReader";
import { displayDate, plainText, readingMinutes } from "./workText";

export type SelectedWork = { work: PcWork; offline: false } | { work: PcWork; offline: true; snapshot: OfflineWork };

const typeLabels: Record<string, string> = { novel: "小説", article: "記事", text: "テキスト", image: "画像", file: "ファイル" };

/**
 * 作品の詳細。ここへ来るのは「読む前に確かめたい」か「作者やシリーズへ移りたい」とき。
 * 携帯は片手で届くよう、読むボタンを画面の下に固定し、表紙と題名を中央に置く。
 * 広い画面は左に表紙、右に情報の 2 列にして、操作はデスクトップ版と同じく右下にまとめる。
 */
export function WorkDetail({ selected, onBack, onRead, onSaved, onRemoved, savedRevision }: {
  selected: SelectedWork;
  onBack: () => void;
  onRead: () => void;
  onSaved: () => void;
  onRemoved: () => void;
  savedRevision: number;
}) {
  const navigation = useAppNavigation();
  const browsable = navigation.canBrowseServer;
  const online = !selected.offline;
  const metadataQuery = useQuery({ queryKey: ["metadata", selected.work.id], queryFn: () => getPcMetadata(selected.work.id), enabled: online });
  const collectionsQuery = useQuery({ queryKey: ["work-collections", selected.work.id], queryFn: () => getPcWorkCollections(selected.work.id), enabled: online });
  const savedQuery = useQuery({ queryKey: ["offline-works", savedRevision], queryFn: listOfflineWorks, enabled: online, staleTime: Infinity });
  const metadata = metadataQuery.data;
  const work = metadata?.work ?? selected.work;
  const collections: PcCollection[] = selected.offline ? selected.snapshot.collections ?? [] : collectionsQuery.data ?? [];
  const [justSaved, setJustSaved] = useState<OfflineWork | null>(null);
  const saved = selected.offline ? selected.snapshot : justSaved ?? savedQuery.data?.find((item) => item.work.id === selected.work.id) ?? null;
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [clamped, setClamped] = useState(false);
  const descriptionRef = useRef<HTMLParagraphElement>(null);

  const people = selected.offline ? selected.snapshot.people : metadata?.people;
  const seriesList = selected.offline ? selected.snapshot.series : metadata?.series;
  const authors = people?.length ? people : work.authorId ? [{ source: work.source, key: work.authorId, role: "", displayName: work.authorName }] : [];
  const series = seriesList?.length ? seriesList : work.seriesTitle ? [{ source: work.source, key: work.seriesId ?? "", title: work.seriesTitle, contentOrder: null }] : [];
  const description = useMemo(() => plainText(work.excerpt), [work.excerpt]);
  const date = displayDate(work.sourceCreatedAt);
  const savedDate = displayDate(work.downloadedAt);
  const minutes = readingMinutes(work.textLength);
  const tags = work.tags ?? [];
  const resume = hasReadingPosition(work, selected.offline ? selected.snapshot.savedEditHash : metadata?.activeEditHash ?? null);
  const outdated = Boolean(online && saved && (work.currentVersion !== saved.savedVersion || work.contentHash !== saved.savedContentHash || (metadata && metadata.activeEditHash !== saved.savedEditHash)));

  useEffect(() => {
    const node = descriptionRef.current;
    if (node && !expanded) setClamped(node.scrollHeight > node.clientHeight + 2);
  }, [description, expanded]);

  const save = async () => {
    setSaving(true);
    setProblem(null);
    try {
      setJustSaved(await saveOfflineWork(work.id));
      onSaved();
    } catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setSaving(false); }
  };

  const remove = async () => {
    setMenuOpen(false);
    if (!saved || !window.confirm(`「${work.title}」の端末内の保存を削除しますか？`)) return;
    setRemoving(true);
    setProblem(null);
    try {
      await deleteOfflineWork(work.id);
      setJustSaved(null);
      onRemoved();
    } catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setRemoving(false); }
  };

  const readLabel = resume ? "続きから読む" : selected.offline ? "端末の保存で読む" : "読む";

  return <div className="detail" role="dialog" aria-modal="true" aria-label={`${work.title}の詳細`}>
    <header className="detail-bar">
      <button className="detail-icon" type="button" onClick={onBack} aria-label="戻る"><ChevronLeft aria-hidden="true" /></button>
      <span className="detail-bar-state">{selected.offline ? "端末に保存した作品" : ""}</span>
      {saved && <div className="detail-menu-root"><button className="detail-icon" type="button" aria-label="その他の操作" aria-expanded={menuOpen} onClick={() => setMenuOpen((open) => !open)}><MoreHorizontal aria-hidden="true" /></button>{menuOpen && <div className="detail-menu"><button type="button" disabled={removing} onClick={() => void remove()}><Trash2 aria-hidden="true" />{removing ? "削除中…" : "端末から削除"}</button></div>}</div>}
    </header>
    <main className="detail-scroll">
      <div className="detail-layout">
        <div className="detail-rail"><div className="detail-cover"><WorkThumbnail work={work} offline={selected.offline} snapshotGeneration={selected.offline ? selected.snapshot.savedAtMs : undefined} variant="detail" /></div></div>
        <div className="detail-content">
          <div className="detail-heading">
            <div className="detail-labels"><ProviderMark source={work.source} /><span>{typeLabels[work.contentType] ?? "作品"}</span>{work.favorite && <span>お気に入り</span>}{metadata?.isEdited && <span>編集版</span>}</div>
            <h1>{work.title}</h1>
            <div className="detail-people">
              {authors.map((person) => {
                const inner = <><AuthorAvatar work={{ ...work, authorId: person.key, source: person.source }} offline={selected.offline} offlineAuthorCover={selected.offline ? Boolean(selected.snapshot.hasAuthorCover) : false} snapshotGeneration={selected.offline ? selected.snapshot.savedAtMs : undefined} /><span>{person.displayName}</span></>;
                return browsable && person.key
                  ? <button key={`${person.source}:${person.key}`} type="button" className="detail-person" onClick={() => navigation.openEntity("person", person.source, person.key, person.displayName)}>{inner}<ChevronRight aria-hidden="true" /></button>
                  : <span key={`${person.source}:${person.key}`} className="detail-person">{inner}</span>;
              })}
            </div>
            {series.map((entry) => {
              const inner = <><Layers aria-hidden="true" /><span>{entry.title}</span>{entry.contentOrder != null && <small>第 {entry.contentOrder} 話</small>}</>;
              return browsable && entry.key
                ? <button key={`${entry.source}:${entry.key}`} type="button" className="detail-series" onClick={() => navigation.openEntity("series", entry.source, entry.key, entry.title)}>{inner}<ChevronRight aria-hidden="true" /></button>
                : <span key={`${entry.source}:${entry.key || entry.title}`} className="detail-series">{inner}</span>;
            })}
          </div>
          <dl className="detail-stats">
            <div><dt><FileText aria-hidden="true" />文字数</dt><dd>{work.textLength.toLocaleString("ja-JP")} 字</dd></div>
            {minutes && <div><dt><Clock3 aria-hidden="true" />読む時間</dt><dd>{minutes}</dd></div>}
            {work.assetCount > 0 && <div><dt><Images aria-hidden="true" />画像</dt><dd>{work.assetCount.toLocaleString("ja-JP")} 枚</dd></div>}
            {date && <div><dt><CalendarDays aria-hidden="true" />公開日</dt><dd>{date}</dd></div>}
            {savedDate && <div><dt><Download aria-hidden="true" />保存日</dt><dd>{savedDate}</dd></div>}
            {work.currentVersion > 1 && <div><dt><History aria-hidden="true" />版</dt><dd>{work.currentVersion} 版</dd></div>}
            {selected.offline && <div><dt><BookOpen aria-hidden="true" />ページ</dt><dd>{selected.snapshot.pageCount}</dd></div>}
          </dl>
          {description && <section className="detail-section" aria-label="概要"><p ref={descriptionRef} className="detail-description" data-expanded={expanded || undefined}>{description}</p>{(clamped || expanded) && <button type="button" className="detail-more" aria-expanded={expanded} onClick={() => setExpanded((open) => !open)}>{expanded ? "閉じる" : "続きを読む"}</button>}</section>}
          {tags.length > 0 && <section className="detail-section"><h2>タグ</h2><div className="detail-tags">{tags.map((tag, index) => browsable ? <button key={`${tag}:${index}`} type="button" onClick={() => navigation.openTag(tag)}>#{tag}</button> : <span key={`${tag}:${index}`}>#{tag}</span>)}</div></section>}
          {collections.length > 0 && <section className="detail-section"><h2>コレクション</h2><div className="detail-collections">{collections.map((collection) => browsable ? <button key={collection.id} type="button" onClick={() => navigation.push({ page: { ...homePage, criteria: defaultPcSearch, tab: "collections", collectionId: collection.id }, detail: null, reader: null })}>{collection.name}<ChevronRight aria-hidden="true" /></button> : <span key={collection.id}>{collection.name}</span>)}</div></section>}
          {problem && <p className="inline-alert" role="alert">{problem}</p>}
        </div>
      </div>
    </main>
    <footer className="detail-actions">
      <span className="detail-actions-note">{selected.offline ? "サーバーにつながらなくても読めます" : outdated ? "端末の保存より新しい版があります" : saved ? "端末にも保存済みです" : ""}</span>
      {online && <button className="secondary-button" type="button" disabled={saving} onClick={() => void save()}>{saving ? "保存中…" : saved ? outdated ? <><RefreshCw aria-hidden="true" />端末を更新</> : <><Check aria-hidden="true" />保存済み</> : <><Download aria-hidden="true" />端末に保存</>}</button>}
      <button className="primary-button" type="button" onClick={onRead}><BookOpen aria-hidden="true" />{readLabel}</button>
    </footer>
  </div>;
}
