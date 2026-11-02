import { useEffect, useRef, useState } from "react";
import DOMPurify from "dompurify";
import { BookOpen, CalendarDays, FileText, Images, MoreHorizontal, Trash2 } from "lucide-react";
import { AuthorAvatar } from "./WorkCard";
import { ProviderMark } from "./ProviderMark";
import { WorkThumbnail } from "./WorkThumbnail";
import { deleteOfflineWork, getPcMetadata, getPcWorkCollections, listOfflineWorks, pcErrorMessage, saveOfflineWork, type OfflineWork, type PcCollection, type PcMetadata, type PcWork } from "./pcApi";

export type SelectedWork = { work: PcWork; offline: false } | { work: PcWork; offline: true; snapshot: OfflineWork };

function plainDescription(source: string): string {
  const safe = DOMPurify.sanitize(source, { ALLOWED_TAGS: ["br"], ALLOWED_ATTR: [] });
  const holder = document.createElement("div");
  holder.innerHTML = safe.replace(/<br\s*\/?\s*>/gi, "\n");
  return holder.textContent?.trim() ?? "";
}

function displayDate(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleDateString("ja-JP");
}

export function WorkDetail({ selected, onBack, onRead, onSaved, onRemoved, savedRevision }: {
  selected: SelectedWork;
  onBack: () => void;
  onRead: () => void;
  onSaved: () => void;
  onRemoved: () => void;
  savedRevision: number;
}) {
  const [metadata, setMetadata] = useState<PcMetadata | null>(null);
  const [collections, setCollections] = useState<PcCollection[] | null>(selected.offline ? selected.snapshot.collections ?? [] : null);
  const [saved, setSaved] = useState<OfflineWork | null>(selected.offline ? selected.snapshot : null);
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const dragStart = useRef<{ x: number; y: number } | null>(null);
  const work = metadata?.work ?? selected.work;

  useEffect(() => {
    if (selected.offline) return;
    let alive = true;
    void Promise.allSettled([getPcMetadata(selected.work.id), getPcWorkCollections(selected.work.id)]).then(([detail, membership]) => {
      if (!alive) return;
      if (detail.status === "fulfilled") setMetadata(detail.value);
      if (membership.status === "fulfilled") setCollections(membership.value);
    });
    return () => { alive = false; };
  }, [selected]);

  useEffect(() => {
    if (selected.offline) return;
    let alive = true;
    void listOfflineWorks().then((items) => {
      if (alive) setSaved(items.find((item) => item.work.id === selected.work.id) ?? null);
    }).catch(() => {});
    return () => { alive = false; };
  }, [selected, savedRevision]);

  const save = async () => {
    setSaving(true);
    setProblem(null);
    try {
      setSaved(await saveOfflineWork(work.id));
      onSaved();
    } catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setSaving(false); }
  };

  const remove = async () => {
    if (!saved || !window.confirm(`「${work.title}」の端末内の保存を削除しますか？`)) return;
    setRemoving(true);
    setProblem(null);
    try {
      await deleteOfflineWork(work.id);
      setSaved(null);
      onRemoved();
    } catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setRemoving(false); }
  };

  const description = work.excerpt ? plainDescription(work.excerpt) : "";
  const date = displayDate(work.sourceCreatedAt ?? work.downloadedAt);
  const tags = work.tags ?? [];

  return <div className="work-detail-screen" role="dialog" aria-modal="true" aria-label="作品詳細">
    <div className="work-detail-top" onTouchStart={(event) => { if (event.touches.length === 1) dragStart.current = { x: event.touches[0].clientX, y: event.touches[0].clientY }; }} onTouchEnd={(event) => { const start = dragStart.current; dragStart.current = null; if (!start || event.changedTouches.length !== 1) return; const dx = event.changedTouches[0].clientX - start.x; const dy = event.changedTouches[0].clientY - start.y; if (dy > 65 && dy > Math.abs(dx) * 1.5) onBack(); }} onTouchCancel={() => { dragStart.current = null; }}><span className="work-detail-grab" aria-hidden="true" /><header className="work-detail-header"><button className="work-detail-back" type="button" onClick={onBack} aria-label="作品一覧に戻る">← 戻る</button><strong>作品詳細</strong>{saved ? <details className="work-detail-more"><summary aria-label="その他の操作"><MoreHorizontal aria-hidden="true" /></summary><div className="work-detail-menu"><button type="button" disabled={removing} onClick={() => void remove()}><Trash2 aria-hidden="true" />{removing ? "削除中…" : "端末から削除"}</button></div></details> : <span>サーバー</span>}</header></div>
    <main className="work-detail-scroll">
      <div className="work-detail-main">
        <div className="work-detail-cover"><WorkThumbnail work={work} offline={selected.offline} snapshotGeneration={selected.offline ? selected.snapshot.savedAtMs : undefined} variant="detail" /></div>
        <div className="work-detail-intro">
          <div className="work-detail-labels"><ProviderMark source={work.source} /><span>{work.contentType === "novel" ? "小説" : "作品"}</span>{work.favorite && <span>お気に入り</span>}{metadata?.isEdited && <span>編集版</span>}</div>
          {work.seriesTitle && <p className="work-detail-series">{work.seriesTitle}</p>}
          <h1>{work.title}</h1>
          <div className="work-detail-author"><AuthorAvatar work={work} offline={selected.offline} offlineAuthorCover={selected.offline ? Boolean(selected.snapshot.hasAuthorCover) : false} snapshotGeneration={selected.offline ? selected.snapshot.savedAtMs : undefined} /><span>{work.authorName}</span></div>
        </div>
      </div>
      <div className="work-detail-facts">
        {date && <span><CalendarDays aria-hidden="true" />公開 {date}</span>}
        <span><FileText aria-hidden="true" />{work.textLength.toLocaleString("ja-JP")} 字</span>
        <span><Images aria-hidden="true" />{work.assetCount.toLocaleString("ja-JP")} 画像</span>
        {selected.offline && <span>{selected.snapshot.pageCount} ページ</span>}
      </div>
      {description && <section className="work-detail-section work-detail-section--description" aria-label="概要"><p className="work-detail-description">{description}</p></section>}
      {tags.length > 0 && <section className="work-detail-section work-detail-section--tags" aria-label="タグ"><div className="work-detail-tags">{tags.map((tag, index) => <span key={`${tag}:${index}`}>#{tag}</span>)}</div></section>}
      {collections && collections.length > 0 && <section className="work-detail-section"><h2>コレクション</h2><div className="work-detail-collections">{collections.map((collection) => <span key={collection.id}>{collection.name}</span>)}</div></section>}
      {selected.offline && <p className="work-detail-saved-note">この作品は端末に保存されています。サーバーにつながらなくても読めます。</p>}
      {!selected.offline && saved && <p className="work-detail-saved-note">端末にも保存済みです。</p>}
      {problem && <p className="inline-alert" role="alert">{problem}</p>}
    </main>
    <footer className="work-detail-actions"><button className="primary-button" type="button" onClick={onRead}><BookOpen aria-hidden="true" />{selected.offline ? "オフラインで読む" : "読む"}</button>{!selected.offline && <button className="secondary-button" type="button" disabled={saving} onClick={() => void save()}>{saving ? "保存中…" : saved ? "端末内を更新" : "端末に保存"}</button>}</footer>
  </div>;
}
