import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { BookOpen, UserRound } from "lucide-react";
import { getPcCollection, getPcEntityCover, listPcCollections, listPcEntities, pcErrorMessage, type PcAsset, type PcCollection, type PcCollectionDetail, type PcEntity, type PcWork } from "./pcApi";
import { enqueueCoverRequest } from "./WorkThumbnail";
import { WorkCard } from "./WorkCard";
import { ProviderMark } from "./ProviderMark";
import { DataUrlCache } from "./DataUrlCache";
import { CollectionArtwork } from "./CollectionArtwork";
import { VirtualWorkList } from "./VirtualWorkList";

const groupCoverCache = new DataUrlCache(8 * 1024 * 1024);
const invalidGroupCovers = new Set<string>();
const pendingGroupCovers = new Map<string, Promise<string>>();

function loadGroupCover(cacheKey: string, load: () => Promise<PcAsset>): Promise<string> {
  const existing = pendingGroupCovers.get(cacheKey);
  if (existing) return existing;
  const request = enqueueCoverRequest(load).then((cover) => {
    const url = `data:${cover.mimeType};base64,${cover.base64}`;
    groupCoverCache.set(cacheKey, url);
    return url;
  }).finally(() => { pendingGroupCovers.delete(cacheKey); });
  pendingGroupCovers.set(cacheKey, request);
  return request;
}

function GroupArtwork({ cacheKey, initial, kind, load }: { cacheKey: string; initial: string; kind: "people" | "series" | "collections"; load: (() => Promise<PcAsset>) | null }) {
  const node = useRef<HTMLSpanElement>(null);
  const [image, setImage] = useState<string | null>(null);
  useEffect(() => {
    setImage(null);
    if (!load || !isTauri() || !node.current) return;
    let alive = true;
    let visible = false;
    let pending = false;
    const show = () => {
      if (invalidGroupCovers.has(cacheKey)) return;
      const cached = groupCoverCache.get(cacheKey);
      if (cached) { setImage(cached); return; }
      if (pending) return;
      pending = true;
      void loadGroupCover(cacheKey, load).then((url) => {
        if (alive && visible) setImage(url);
      }).catch(() => {}).finally(() => { pending = false; });
    };
    if (typeof IntersectionObserver === "undefined") { visible = true; show(); return () => { alive = false; }; }
    const observer = new IntersectionObserver((entries) => {
      visible = entries.some((entry) => entry.isIntersecting);
      if (visible) show();
      else setImage(null);
    }, { rootMargin: "180px" });
    observer.observe(node.current);
    return () => { alive = false; observer.disconnect(); };
  }, [cacheKey]);
  return <span className={`group-card-art group-card-art--${kind}`} ref={node} aria-hidden="true">{image ? <img src={image} alt="" loading="lazy" decoding="async" onError={() => { invalidGroupCovers.add(cacheKey); groupCoverCache.delete(cacheKey); setImage(null); }} /> : kind === "people" ? <UserRound /> : kind === "series" ? <BookOpen /> : initial}</span>;
}

export function EntityBrowser({ kind, query, onOpen }: {
  kind: "people" | "series";
  query: string;
  onOpen: (entity: PcEntity) => void;
}) {
  const [items, setItems] = useState<PcEntity[]>([]);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [total, setTotal] = useState(0);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const noun = kind === "people" ? "作者・クリエイター" : "シリーズ";

  useEffect(() => {
    let alive = true;
    setItems([]);
    setNextOffset(null);
    setBusy(true);
    setProblem(null);
    void listPcEntities(kind, query).then((page) => {
      if (!alive) return;
      setItems(page.items);
      setNextOffset(page.nextOffset);
      setTotal(page.total);
    }).catch((error) => { if (alive) setProblem(pcErrorMessage(error)); })
      .finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, [kind, query]);

  const more = async () => {
    if (nextOffset === null || busy) return;
    setBusy(true);
    setProblem(null);
    try {
      const page = await listPcEntities(kind, query, nextOffset);
      setItems((old) => [...old, ...page.items]);
      setNextOffset(page.nextOffset);
      setTotal(page.total);
    } catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setBusy(false); }
  };

  return <section className="group-browser">
    <div className="section-heading group-heading"><h2>{noun}</h2><span>{total.toLocaleString("ja-JP")} 件</span></div>
    {problem && <div className="inline-alert" role="alert">{problem}</div>}
    {busy && !items.length ? <section className="empty-card"><p>読み込んでいます…</p></section> : items.length ? <div className="group-grid">{items.map((entity) => <button className="group-card" key={`${entity.source}:${entity.sourceKey}`} onClick={() => onOpen(entity)}>
      <GroupArtwork cacheKey={`${kind}:${entity.source}:${entity.sourceKey}`} initial={entity.displayName.slice(0, 1)} kind={kind} load={entity.hasCover ? () => getPcEntityCover(kind, entity.source, entity.sourceKey) : null} />
      <span className="group-card-text"><strong>{entity.displayName}</strong><small className="group-card-meta"><ProviderMark source={entity.source} compact /><span>{entity.count.toLocaleString("ja-JP")} 作品{kind === "series" && entity.isConcluded !== null ? ` · ${entity.isConcluded ? "完結" : "連載中"}` : ""}</span></small>{entity.sampleTitle && <span>{entity.sampleTitle}</span>}</span>
      <span className="work-arrow" aria-hidden="true">›</span>
    </button>)}</div> : !busy && <section className="empty-card"><h2>{noun}が見つかりません</h2><p>検索語を変えて試してください。</p></section>}
    {nextOffset !== null && <div className="load-more"><button className="secondary-button" disabled={busy} onClick={() => void more()}>{busy ? "読み込み中…" : "さらに表示"}</button></div>}
  </section>;
}

export function CollectionBrowser({ query, onSelectionChange, onOpenWork, onOpenDetail }: { query: string; onSelectionChange: (selected: boolean) => void; onOpenWork: (work: PcWork) => void; onOpenDetail: (work: PcWork) => void }) {
  const [items, setItems] = useState<PcCollection[]>([]);
  const [selected, setSelected] = useState<PcCollectionDetail | null>(null);
  const [busy, setBusy] = useState(true);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void listPcCollections().then((collections) => { if (alive) setItems(collections); })
      .catch((error) => { if (alive) setProblem(pcErrorMessage(error)); })
      .finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, []);

  const open = async (id: string) => {
    setBusy(true);
    setProblem(null);
    try {
      setSelected(await getPcCollection(id));
      onSelectionChange(true);
      document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
    }
    catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setBusy(false); }
  };

  const memberQuery = query.trim().toLocaleLowerCase();
  const visibleMembers = selected?.members.filter((member) => !memberQuery || `${member.title} ${member.authorName} ${member.work?.seriesTitle ?? ""} ${member.work?.tags.join(" ") ?? ""}`.toLocaleLowerCase().includes(memberQuery)) ?? [];
  if (selected) return <section className="group-browser">
    <button className="group-back" onClick={() => { setSelected(null); onSelectionChange(false); document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" }); }}>← コレクション一覧</button>
    <div className="section-heading group-heading"><h2>{selected.name}</h2><span>{selected.memberCount} 作品</span></div>
    {selected.description && <p className="group-description">{selected.description}</p>}
    {visibleMembers.length ? <VirtualWorkList items={visibleMembers} itemKey={(member) => `${member.position}:${member.work?.id ?? member.title}`} renderItem={(member) => member.work ? <WorkCard work={member.work} onRead={() => onOpenWork(member.work!)} onDetail={() => onOpenDetail(member.work!)} prefix={selected.collectionKind === "ordered" ? `${member.position + 1} 番目` : undefined} /> : <div className="group-missing"><strong>{member.title}</strong><span>{member.authorName} · サーバーに作品データがありません</span></div>} /> : <section className="empty-card"><h2>作品が見つかりません</h2><p>このコレクション内の検索語を変えてください。</p></section>}
  </section>;

  const visible = items.filter((item) => `${item.name} ${item.description ?? ""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <section className="group-browser">
    <div className="section-heading group-heading"><h2>コレクション</h2><span>{items.length} 件</span></div>
    {problem && <div className="inline-alert" role="alert">{problem}</div>}
    {busy && !items.length ? <section className="empty-card"><p>読み込んでいます…</p></section> : visible.length ? <div className="group-grid">{visible.map((item) => <button className="group-card" key={item.id} onClick={() => void open(item.id)}>
      <CollectionArtwork collection={item} />
      <span className="group-card-text"><strong>{item.name}</strong><small>{item.memberCount.toLocaleString("ja-JP")} 作品 · {item.collectionKind === "ordered" ? "順序あり" : "順序なし"}</small>{item.description && <span>{item.description}</span>}</span>
      <span className="work-arrow" aria-hidden="true">›</span>
    </button>)}</div> : !busy && <section className="empty-card"><h2>{query ? "該当するコレクションがありません" : "コレクションがありません"}</h2><p>{query ? "検索語を変えて試してください。" : "デスクトップ版で作成したコレクションがここに表示されます。"}</p></section>}
  </section>;
}
