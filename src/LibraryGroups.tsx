import { useEffect, useMemo, useState } from "react";
import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { BookOpen, UserRound } from "lucide-react";
import { getPcCollection, listPcCollections, listPcEntities, pcErrorMessage, type PcCollection, type PcEntity, type PcWork } from "./pcApi";
import { WorkCard } from "./WorkCard";
import { ProviderMark } from "./ProviderMark";
import { CollectionArtwork } from "./CollectionArtwork";
import { VirtualWorkList } from "./VirtualWorkList";
import { entityCoverUrl, imageRecentlyFailed, SmoothImage } from "./images";
import { splitEntitySort, type EntitySort } from "./librarySort";
import { refreshShortList, useAutoLoad, useStatusRecovery } from "./workList";

export function GroupArtwork({ kind, src, initial }: { kind: "people" | "series" | "collections"; src: string | null; initial?: string }) {
  const [failed, setFailed] = useState(() => imageRecentlyFailed(src));
  useEffect(() => { setFailed(imageRecentlyFailed(src)); }, [src]);
  return <span className={`group-card-art group-card-art--${kind}`} aria-hidden="true">
    {kind === "people" ? <UserRound /> : kind === "series" ? <BookOpen /> : initial}
    {src && !failed && <SmoothImage src={src} alt="" onFailed={() => setFailed(true)} />}
  </span>;
}

export function EntityCard({ kind, entity, onOpen }: { kind: "people" | "series"; entity: PcEntity; onOpen: () => void }) {
  return <button className="group-card" type="button" onClick={onOpen}>
    <GroupArtwork kind={kind} src={entity.hasCover ? entityCoverUrl(kind, entity.source, entity.sourceKey) : null} />
    <span className="group-card-text"><strong>{entity.displayName}</strong><small className="group-card-meta"><ProviderMark source={entity.source} compact /><span>{entity.count.toLocaleString("ja-JP")} 作品{kind === "series" && entity.isConcluded !== null ? ` · ${entity.isConcluded ? "完結" : "連載中"}` : ""}</span></small>{entity.sampleTitle && <span>{entity.sampleTitle}</span>}</span>
    <span className="work-arrow" aria-hidden="true">›</span>
  </button>;
}

export function GroupSkeleton() {
  return <div className="group-grid" role="status" aria-label="読み込んでいます">{Array.from({ length: 6 }, (_, index) => <div className="group-card group-card--skeleton" key={index} aria-hidden="true"><span className="skeleton-block group-card-art" /><span className="group-card-text"><span className="skeleton-block skeleton-line skeleton-line--title" /><span className="skeleton-block skeleton-line skeleton-line--short" /></span></div>)}</div>;
}

export function EntityBrowser({ kind, query, sort, onOpen }: {
  kind: "people" | "series";
  query: string;
  sort: EntitySort;
  onOpen: (entity: PcEntity) => void;
}) {
  const noun = kind === "people" ? "作者・クリエイター" : "シリーズ";
  const list = useInfiniteQuery({
    queryKey: ["entities", kind, query, sort],
    queryFn: ({ pageParam }) => listPcEntities(kind, query, pageParam, splitEntitySort(sort)),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
    placeholderData: keepPreviousData,
    refetchOnMount: refreshShortList,
  });
  useStatusRecovery(list.error);
  const items = useMemo(() => list.data?.pages.flatMap((page) => page.items) ?? [], [list.data]);
  const total = list.data?.pages[0]?.total ?? 0;
  const loadMore = useAutoLoad(list.hasNextPage && !list.isFetchingNextPage && !list.isPlaceholderData, items.length, () => { void list.fetchNextPage(); });

  return <section className="group-browser" data-stale={list.isPlaceholderData || undefined}>
    <div className="section-heading group-heading"><h2>{noun}</h2>{list.data && <span>{total.toLocaleString("ja-JP")} 件</span>}</div>
    {list.error ? <div className="inline-alert" role="alert">{pcErrorMessage(list.error)}</div> : null}
    {list.isPending ? <GroupSkeleton /> : items.length ? <VirtualWorkList listKey={`entities:${kind}:${query}:${sort}`} minColumn={300} estimatedHeight={104} items={items} itemKey={(entity) => `${entity.source}:${entity.sourceKey}`} renderItem={(entity) => <EntityCard kind={kind} entity={entity} onOpen={() => onOpen(entity)} />} />
      : !list.error && <section className="empty-card"><h2>{noun}が見つかりません</h2><p>検索語を変えて試してください。</p></section>}
    {list.hasNextPage && <div className="load-more" ref={loadMore}>{list.isFetchingNextPage ? <span className="load-more-status" role="status">続きを読み込んでいます…</span> : <button className="secondary-button" type="button" onClick={() => void list.fetchNextPage()}>さらに表示</button>}</div>}
  </section>;
}

export function CollectionCard({ collection, onOpen }: { collection: PcCollection; onOpen: () => void }) {
  return <button className="group-card" type="button" onClick={onOpen}>
    <CollectionArtwork collection={collection} />
    <span className="group-card-text"><strong>{collection.name}</strong><small>{collection.memberCount.toLocaleString("ja-JP")} 作品 · {collection.collectionKind === "ordered" ? "順序あり" : "順序なし"}</small>{collection.description && <span>{collection.description}</span>}</span>
    <span className="work-arrow" aria-hidden="true">›</span>
  </button>;
}

export function CollectionBrowser({ query, onOpen }: { query: string; onOpen: (collection: PcCollection) => void }) {
  const list = useQuery({ queryKey: ["collections"], queryFn: listPcCollections });
  useStatusRecovery(list.error);
  const items = list.data ?? [];
  const needle = query.toLocaleLowerCase();
  const visible = items.filter((item) => `${item.name} ${item.description ?? ""}`.toLocaleLowerCase().includes(needle));
  return <section className="group-browser">
    <div className="section-heading group-heading"><h2>コレクション</h2>{list.data && <span>{items.length} 件</span>}</div>
    {list.error ? <div className="inline-alert" role="alert">{pcErrorMessage(list.error)}</div> : null}
    {list.isPending ? <GroupSkeleton /> : visible.length ? <div className="group-grid">{visible.map((item) => <CollectionCard key={item.id} collection={item} onOpen={() => onOpen(item)} />)}</div>
      : !list.error && <section className="empty-card"><h2>{query ? "該当するコレクションがありません" : "コレクションがありません"}</h2><p>{query ? "検索語を変えて試してください。" : "デスクトップ版で作成したコレクションがここに表示されます。"}</p></section>}
  </section>;
}

export function CollectionMembers({ collectionId, query, onBack, onOpenWork, onOpenDetail }: { collectionId: string; query: string; onBack: () => void; onOpenWork: (work: PcWork) => void; onOpenDetail: (work: PcWork) => void }) {
  const detail = useQuery({ queryKey: ["collection", collectionId], queryFn: () => getPcCollection(collectionId) });
  useStatusRecovery(detail.error);
  const selected = detail.data;
  const needle = query.trim().toLocaleLowerCase();
  const members = selected?.members.filter((member) => !needle || `${member.title} ${member.authorName} ${member.work?.seriesTitle ?? ""} ${member.work?.tags.join(" ") ?? ""}`.toLocaleLowerCase().includes(needle)) ?? [];
  return <section className="group-browser">
    <button className="group-back" type="button" onClick={onBack}>← 戻る</button>
    {detail.error ? <div className="inline-alert" role="alert">{pcErrorMessage(detail.error)}</div> : null}
    {detail.isPending ? <GroupSkeleton /> : selected && <>
      <div className="section-heading group-heading"><h2>{selected.name}</h2><span>{selected.memberCount} 作品</span></div>
      {selected.description && <p className="group-description">{selected.description}</p>}
      {members.length ? <VirtualWorkList listKey={`collection:${collectionId}:${needle}`} items={members} itemKey={(member) => `${member.position}:${member.work?.id ?? member.title}`} renderItem={(member) => member.work ? <WorkCard work={member.work} onRead={() => onOpenWork(member.work!)} onDetail={() => onOpenDetail(member.work!)} prefix={selected.collectionKind === "ordered" ? `${member.position + 1} 番目` : undefined} /> : <div className="group-missing"><strong>{member.title}</strong><span>{member.authorName} · サーバーに作品データがありません</span></div>} />
        : <section className="empty-card"><h2>作品が見つかりません</h2><p>このコレクション内の検索語を変えてください。</p></section>}
    </>}
  </section>;
}
