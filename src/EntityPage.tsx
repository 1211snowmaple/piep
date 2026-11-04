import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { BookOpen, Check, ChevronLeft, ExternalLink, Search, UserRound } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useAppNavigation, type EntityPageState, type EntityTab } from "./appHistory";
import { ProviderMark } from "./ProviderMark";
import { ChoiceMenu, relevanceLabel } from "./SortMenu";
import { CollectionCard, EntityCard, GroupSkeleton } from "./LibraryGroups";
import { VirtualWorkList } from "./VirtualWorkList";
import { entityCoverUrl, imageRecentlyFailed, SmoothImage } from "./images";
import { defaultPcSearch, getPcEntityDetail, listPcEntitySeries, pcErrorMessage, type PcCollection, type PcConnectionStatus, type PcEntityDetail, type PcWork, type PcWorkSort } from "./pcApi";
import { entitySeriesKey, entitySortGroups, entitySortLabel, entityWorkSortGroups, entityWorkSortLabel, loadEntitySort, loadEntityWorkSort, saveEntitySort, saveEntityWorkSort, splitEntitySort, type EntitySort, type EntityWorkSort } from "./librarySort";
import { refreshShortList, useAutoLoad, useStatusRecovery, useWorkList, WorkResults } from "./workList";
import { plainText } from "./workText";

const tagsShownFirst = 10;
type CollectionSort = "name" | "members" | "available";
const collectionSortLabels: Record<CollectionSort, string> = { name: "名前の順", members: "登録作品の多い順", available: "読める作品の多い順" };

/** 打ちながら絞る。変換中は確定を待つ。日本語は確定前に検索しても意味がないため。 */
function LiveSearch({ value, onCommit, label, children }: { value: string; onCommit: (text: string) => void; label: string; children?: ReactNode }) {
  const [input, setInput] = useState(value);
  const composing = useRef(false);
  const latest = useRef(onCommit);
  latest.current = onCommit;
  useEffect(() => { setInput(value); }, [value]);
  useEffect(() => {
    if (composing.current || input.trim() === value) return;
    const timer = window.setTimeout(() => latest.current(input.trim()), 260);
    return () => window.clearTimeout(timer);
  }, [input, value]);
  return <form className="entity-search" role="search" onSubmit={(event) => { event.preventDefault(); onCommit(input.trim()); event.currentTarget.querySelector("input")?.blur(); }}>
    <label className="entity-search-field"><Search aria-hidden="true" /><input type="search" aria-label={label} placeholder={label} value={input} onChange={(event) => setInput(event.target.value)} onCompositionStart={() => { composing.current = true; }} onCompositionEnd={(event) => { composing.current = false; setInput(event.currentTarget.value); }} /></label>
    {children}
  </form>;
}

function hostName(link: string): string {
  try { return new URL(link).hostname.replace(/^www\./, ""); } catch { return link; }
}

function Hero({ page, detail }: { page: EntityPageState; detail: PcEntityDetail | undefined }) {
  const isPerson = page.kind === "person";
  const kindPath = isPerson ? "people" : "series";
  const [expanded, setExpanded] = useState(false);
  const [clamped, setClamped] = useState(false);
  const descriptionRef = useRef<HTMLParagraphElement>(null);
  const description = useMemo(() => plainText(detail?.description), [detail?.description]);
  const cover = detail?.hasCover ? entityCoverUrl(kindPath, page.source, page.key) : null;
  const banner = detail?.hasBanner ? entityCoverUrl(kindPath, page.source, page.key, true) : null;
  const [coverFailed, setCoverFailed] = useState(false);
  const [bannerFailed, setBannerFailed] = useState(false);
  useEffect(() => { setCoverFailed(imageRecentlyFailed(cover)); setBannerFailed(imageRecentlyFailed(banner)); }, [cover, banner]);
  useEffect(() => {
    const node = descriptionRef.current;
    if (node && !expanded) setClamped(node.scrollHeight > node.clientHeight + 2);
  }, [description, expanded]);
  const workCount = detail?.workCount;
  const published = detail?.publishedCount;
  // 取得元の話数は、手元と違うときだけ意味がある。同じなら同じ数字を二度見せない。
  const differs = !isPerson && published != null && workCount != null && published !== workCount;

  return <header className="entity-hero" data-kind={page.kind} data-banner={banner && !bannerFailed ? "true" : undefined}>
    {banner && !bannerFailed && <div className="entity-banner"><SmoothImage src={banner} alt="" onFailed={() => setBannerFailed(true)} /></div>}
    <div className="entity-hero-main">
      <span className="entity-art" aria-hidden="true">{isPerson ? <UserRound /> : <BookOpen />}{cover && !coverFailed && <SmoothImage src={cover} alt="" onFailed={() => setCoverFailed(true)} />}</span>
      <div className="entity-hero-text">
        <div className="entity-labels"><ProviderMark source={page.source} />{detail?.isConcluded != null && <span className={detail.isConcluded ? "entity-badge entity-badge--done" : "entity-badge"}>{detail.isConcluded ? <><Check aria-hidden="true" />完結</> : "連載中"}</span>}</div>
        <h1>{detail?.displayName ?? page.displayName}</h1>
        {workCount != null && <p className="entity-stats">
          {differs ? <span>手元 <strong>{workCount.toLocaleString("ja-JP")}</strong> 話 / 取得元 <strong>{published!.toLocaleString("ja-JP")}</strong> 話</span> : <span><strong>{workCount.toLocaleString("ja-JP")}</strong> {isPerson ? "作品" : "話"}</span>}
          {isPerson && (detail?.seriesCount ?? 0) > 0 && <span><strong>{detail!.seriesCount!.toLocaleString("ja-JP")}</strong> シリーズ</span>}
        </p>}
      </div>
    </div>
    {description && <div className="entity-description"><p ref={descriptionRef} data-expanded={expanded || undefined}>{description}</p>{(clamped || expanded) && <button type="button" className="entity-more" aria-expanded={expanded} onClick={() => setExpanded((open) => !open)}>{expanded ? "閉じる" : "続きを読む"}</button>}</div>}
    {(detail?.links.length ?? 0) > 0 && <div className="entity-links">{detail!.links.map((link) => <button key={link} type="button" onClick={() => void openUrl(link)}><ExternalLink aria-hidden="true" />{hostName(link)}</button>)}</div>}
  </header>;
}

function TagFilter({ detail, selected, onToggle }: { detail: PcEntityDetail | undefined; selected: string[]; onToggle: (tag: string) => void }) {
  const [open, setOpen] = useState(false);
  const tags = detail?.tags ?? [];
  if (!tags.length) return null;
  const chosen = new Set(selected);
  // 選んだタグは「もっと見る」に隠さない。外せなくなるので。
  const shown = open ? tags : tags.filter((tag, index) => index < tagsShownFirst || chosen.has(tag.name));
  const rest = tags.length - shown.length;
  return <div className="entity-tags" data-open={open || undefined} role="group" aria-label="タグで絞り込む">
    {shown.map((tag) => <button key={tag.name} type="button" aria-pressed={chosen.has(tag.name)} onClick={() => onToggle(tag.name)}><span>#{tag.name}</span><small>{tag.count.toLocaleString("ja-JP")}</small></button>)}
    {(rest > 0 || open) && <button type="button" className="entity-tags-more" aria-expanded={open} onClick={() => setOpen((value) => !value)}>{open ? "閉じる" : `+${rest}`}</button>}
  </div>;
}

export function EntityPage({ page, status, savedRevision, pagingMode, onOpenWork, onOpenDetail }: {
  page: EntityPageState;
  status: PcConnectionStatus | null;
  savedRevision: number;
  pagingMode: "auto" | "pages";
  onOpenWork: (work: PcWork) => void;
  onOpenDetail: (work: PcWork) => void;
}) {
  const navigation = useAppNavigation();
  const reachable = Boolean(status?.reachable);
  const isPerson = page.kind === "person";
  const kindPath = isPerson ? "people" : "series";
  const tabsRef = useRef<HTMLElement>(null);

  const detailQuery = useQuery({ queryKey: ["entity-detail", kindPath, page.source, page.key], queryFn: () => getPcEntityDetail(kindPath, page.source, page.key), enabled: reachable, retry: false });
  useStatusRecovery(detailQuery.error);
  const detail = detailQuery.data;
  const seriesCount = detail?.seriesCount ?? 0;
  const collections = detail?.collections ?? [];

  const [workSort, setWorkSort] = useState<EntityWorkSort>(() => loadEntityWorkSort(page.kind));
  const [relevance, setRelevance] = useState(false);
  const [seriesSort, setSeriesSort] = useState<EntitySort>(() => loadEntitySort(entitySeriesKey));
  const [collectionSort, setCollectionSort] = useState<CollectionSort>("name");
  useEffect(() => { setWorkSort(loadEntityWorkSort(page.kind)); setRelevance(false); }, [page.kind, page.source, page.key]);
  useEffect(() => { if (!page.query) setRelevance(false); }, [page.query]);

  const sentSort: PcWorkSort = relevance && page.query ? "standard" : workSort;
  const search = useMemo(() => ({ ...defaultPcSearch, text: page.query, tag: page.tags.join(","), tagMode: "and" as const, sort: sentSort }), [page.query, page.tags, sentSort]);
  const scope = useMemo(() => ({ kind: page.kind, source: page.source, key: page.key }), [page.kind, page.source, page.key]);
  const list = useWorkList({ search, scope, pagingMode, page: page.listPage, enabled: reachable && page.tab === "works" });

  const series = useInfiniteQuery({
    queryKey: ["entity-series", page.source, page.key, page.seriesQuery, seriesSort],
    queryFn: ({ pageParam }) => listPcEntitySeries(page.source, page.key, page.seriesQuery, pageParam, splitEntitySort(seriesSort)),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
    enabled: reachable && isPerson && page.tab === "series",
    placeholderData: keepPreviousData,
    refetchOnMount: refreshShortList,
  });
  useStatusRecovery(series.error);
  const seriesItems = useMemo(() => series.data?.pages.flatMap((entry) => entry.items) ?? [], [series.data]);
  const loadMoreSeries = useAutoLoad(series.hasNextPage && !series.isFetchingNextPage && !series.isPlaceholderData, seriesItems.length, () => { void series.fetchNextPage(); });

  const sortedCollections = useMemo(() => {
    const items = [...collections];
    if (collectionSort === "name") items.sort((a, b) => a.name.localeCompare(b.name, "ja"));
    if (collectionSort === "members") items.sort((a, b) => b.memberCount - a.memberCount);
    if (collectionSort === "available") items.sort((a, b) => b.availableCount - a.availableCount);
    return items;
  }, [collections, collectionSort]);

  /** 並べ替えや絞り込みのあとは、一覧の頭を見せる。作者の紹介まで戻す必要はない。 */
  const showListStart = () => {
    const tabs = tabsRef.current;
    const scroller = tabs?.closest<HTMLElement>(".main-content");
    if (!tabs || !scroller) return;
    const top = tabs.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    if (top < 0) scroller.scrollTop += top;
  };
  const update = (patch: Partial<EntityPageState>) => {
    navigation.replace({ page: { ...page, ...patch } });
    showListStart();
  };

  const tabs: { id: EntityTab; label: string; count?: number }[] = [
    { id: "works", label: "作品", count: detail?.workCount },
    ...(isPerson && seriesCount > 0 ? [{ id: "series" as const, label: "シリーズ", count: seriesCount }] : []),
    ...(collections.length > 0 ? [{ id: "collections" as const, label: "コレクション", count: collections.length }] : []),
  ];
  const openCollection = (collection: PcCollection) => navigation.push({ page: { name: "library", tab: "collections", collectionId: collection.id, criteria: defaultPcSearch, queries: { people: "", series: "", collections: "", members: "" }, listPage: 1 } });

  const workLabel = sentSort === "standard" ? relevanceLabel : entityWorkSortLabel(sentSort);
  const workGroups = entityWorkSortGroups(page.kind, Boolean(page.query));

  return <div className="entity-page">
    <button className="page-back" type="button" onClick={navigation.back}><ChevronLeft aria-hidden="true" />戻る</button>
    <Hero page={page} detail={detail} />
    {!reachable && <section className="empty-card"><h2>サーバーにつながっていません</h2><p>つながると、この{isPerson ? "作者" : "シリーズ"}の作品がここに並びます。</p></section>}
    {reachable && <>
      {tabs.length > 1 && <nav className="library-tabs entity-tabs" ref={tabsRef} aria-label={`${isPerson ? "作者" : "シリーズ"}の中身`}>{tabs.map((tab) => <button key={tab.id} type="button" className={page.tab === tab.id ? "active" : ""} aria-current={page.tab === tab.id ? "page" : undefined} onClick={() => update({ tab: tab.id, listPage: 1 })}>{tab.label}{tab.count != null && <small>{tab.count.toLocaleString("ja-JP")}</small>}</button>)}</nav>}
      {tabs.length <= 1 && <span ref={tabsRef as React.RefObject<HTMLSpanElement>} className="entity-tabs-anchor" />}

      {page.tab === "works" && <section className="entity-section">
        <LiveSearch value={page.query} label={isPerson ? "この作者の作品を検索" : "このシリーズの作品を検索"} onCommit={(query) => update({ query, listPage: 1 })}>
          <ChoiceMenu<PcWorkSort> current={sentSort} label={workLabel} groups={workGroups} onSelect={(sort) => {
            if (sort === "standard") setRelevance(true);
            else { setRelevance(false); setWorkSort(sort as EntityWorkSort); saveEntityWorkSort(page.kind, sort as EntityWorkSort); }
            update({ listPage: 1 });
          }} />
        </LiveSearch>
        <TagFilter detail={detail} selected={page.tags} onToggle={(tag) => update({ tags: page.tags.includes(tag) ? page.tags.filter((item) => item !== tag) : [...page.tags, tag], listPage: 1 })} />
        {(page.query || page.tags.length > 0) && <div className="entity-filter-summary"><span>{list.total != null ? `約 ${list.total.toLocaleString("ja-JP")} 件` : list.loading ? "" : `${list.items.length} 件`}</span><button type="button" onClick={() => update({ query: "", tags: [], listPage: 1 })}>絞り込みを解除</button></div>}
        <WorkResults omit={isPerson ? "author" : "series"} list={list} savedRevision={savedRevision} reachable={reachable} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} onPage={(next) => navigation.push({ page: { ...page, listPage: next } })} emptyTitle="作品が見つかりません" emptyHint={page.query || page.tags.length ? "検索語やタグを変えて試してください。" : "この一覧に表示できる作品はありません。"} />
      </section>}

      {page.tab === "series" && isPerson && <section className="entity-section" data-stale={series.isPlaceholderData || undefined}>
        <LiveSearch value={page.seriesQuery} label="この作者のシリーズを検索" onCommit={(seriesQuery) => update({ seriesQuery })}>
          <ChoiceMenu current={seriesSort} label={entitySortLabel(seriesSort)} groups={entitySortGroups} onSelect={(sort) => { setSeriesSort(sort); saveEntitySort(entitySeriesKey, sort); showListStart(); }} />
        </LiveSearch>
        {series.error ? <div className="inline-alert" role="alert">{pcErrorMessage(series.error)}</div> : null}
        {series.isPending ? <GroupSkeleton /> : seriesItems.length ? <VirtualWorkList listKey={`entity-series:${page.source}:${page.key}:${page.seriesQuery}:${seriesSort}`} minColumn={300} estimatedHeight={104} items={seriesItems} itemKey={(entity) => `${entity.source}:${entity.sourceKey}`} renderItem={(entity) => <EntityCard kind="series" entity={entity} onOpen={() => navigation.openEntity("series", entity.source, entity.sourceKey, entity.displayName)} />} />
          : !series.error && <section className="empty-card"><h2>シリーズが見つかりません</h2><p>検索語を変えて試してください。</p></section>}
        {series.hasNextPage && <div className="load-more" ref={loadMoreSeries}>{series.isFetchingNextPage ? <span className="load-more-status" role="status">続きを読み込んでいます…</span> : <button className="secondary-button" type="button" onClick={() => void series.fetchNextPage()}>さらに表示</button>}</div>}
      </section>}

      {page.tab === "collections" && <section className="entity-section">
        <div className="entity-section-bar"><span>{isPerson ? "この作者の作品が入っているコレクション" : "このシリーズの作品が入っているコレクション"}</span>
          <ChoiceMenu current={collectionSort} label={collectionSortLabels[collectionSort]} groups={[{ label: "並び順", items: (Object.keys(collectionSortLabels) as CollectionSort[]).map((value) => ({ value, label: collectionSortLabels[value] })) }]} onSelect={setCollectionSort} />
        </div>
        <div className="group-grid">{sortedCollections.map((collection) => <CollectionCard key={collection.id} collection={collection} onOpen={() => openCollection(collection)} />)}</div>
      </section>}
    </>}
  </div>;
}
