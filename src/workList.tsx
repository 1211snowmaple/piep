import { useCallback, useEffect, useMemo, useRef } from "react";
import { keepPreviousData, useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { WorkCard, WorkCardSkeleton, type SavedState } from "./WorkCard";
import { VirtualWorkList } from "./VirtualWorkList";
import { getPcMetadata, listOfflineWorks, pcErrorMessage, searchPcWorks, type PcSearchPage, type PcWork, type PcWorkScope, type PcWorkSearch } from "./pcApi";

export const statusQueryKey = ["pc-status"] as const;
const pageSize = 20;
const maxDirectPage = 251;
const statusErrors = new Set(["offline", "certificate_changed", "protocol_mismatch", "revoked"]);

/**
 * 長く読み進めた一覧を戻るたびに先頭から取り直すと、その間ずっと通信が続く。
 * 数ページまでなら古くなったときに取り直し、それより長いものは今の中身を保つ。
 */
export function refreshShortList(query: { state: { data: unknown } }): boolean {
  const pages = (query.state.data as { pages?: unknown[] } | undefined)?.pages?.length ?? 0;
  return pages <= 5;
}

/** 接続が切れたことに気づいたら、接続状態も確かめ直す。 */
export function useStatusRecovery(error: unknown) {
  const client = useQueryClient();
  useEffect(() => {
    const code = typeof error === "object" && error && "code" in error ? String(error.code) : null;
    if (code && statusErrors.has(code)) void client.invalidateQueries({ queryKey: statusQueryKey });
  }, [client, error]);
}

export type WorkList = {
  items: PcWork[];
  total: number | null;
  /** まだ何も出せない。 */
  loading: boolean;
  /** 前の一覧を出したまま、新しい一覧を取りにいっている。 */
  stale: boolean;
  fetchingMore: boolean;
  hasMore: boolean;
  fetchMore: () => void;
  numbered: boolean;
  page: number;
  pageCount: number;
  reachedDirectLimit: boolean;
  error: unknown;
  listKey: string;
};

export function useWorkList({ search, scope, pagingMode, page, enabled }: { search: PcWorkSearch; scope: PcWorkScope | null; pagingMode: "auto" | "pages"; page: number; enabled: boolean }): WorkList {
  // 関連度の順はカーソルでしか辿れない。
  const numbered = pagingMode === "pages" && !(search.text.trim() && search.sort === "standard");
  const scopeKey = scope ? { kind: scope.kind, source: scope.source, key: scope.key } : null;
  const infinite = useInfiniteQuery({
    queryKey: ["works", search, scopeKey],
    queryFn: ({ pageParam }) => searchPcWorks(search, pageParam, scopeKey),
    initialPageParam: null as string | null,
    getNextPageParam: (last: PcSearchPage) => last.nextCursor,
    enabled: enabled && !numbered,
    placeholderData: keepPreviousData,
    refetchOnMount: refreshShortList,
  });
  const paged = useQuery({
    queryKey: ["works-page", search, scopeKey, page],
    queryFn: () => searchPcWorks(search, null, scopeKey, (page - 1) * pageSize),
    enabled: enabled && numbered,
    placeholderData: keepPreviousData,
  });
  const items = useMemo(() => numbered ? paged.data?.items ?? [] : infinite.data?.pages.flatMap((entry) => entry.items) ?? [], [numbered, paged.data, infinite.data]);
  const first = numbered ? paged.data : infinite.data?.pages[0];
  const total = first?.totalEstimate ?? null;
  const estimated = Math.max(1, Math.ceil((total ?? page * pageSize) / pageSize));
  const error = numbered ? paged.error : infinite.error;
  useStatusRecovery(error);
  return {
    items,
    total,
    loading: numbered ? paged.isPending && enabled : infinite.isPending && enabled,
    stale: numbered ? paged.isPlaceholderData : infinite.isPlaceholderData,
    fetchingMore: infinite.isFetchingNextPage,
    hasMore: !numbered && infinite.hasNextPage && !infinite.isPlaceholderData,
    fetchMore: () => { void infinite.fetchNextPage(); },
    numbered,
    page,
    pageCount: Math.min(maxDirectPage, estimated),
    reachedDirectLimit: estimated > maxDirectPage,
    error,
    listKey: JSON.stringify(["works", search, scopeKey, numbered ? page : 0]),
  };
}

/** 一覧の終わりが近づいたら続きを取る。余裕を持って先に読むので、下まで来ても待たされない。 */
export function useAutoLoad(enabled: boolean, count: number, load: () => void) {
  const node = useRef<HTMLDivElement | null>(null);
  const latest = useRef(load);
  latest.current = load;
  const set = useCallback((element: HTMLDivElement | null) => { node.current = element; }, []);
  useEffect(() => {
    if (!enabled || !node.current) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        observer.disconnect();
        latest.current();
      }
    }, { root: node.current.closest(".main-content"), rootMargin: "900px 0px" });
    observer.observe(node.current);
    return () => observer.disconnect();
  }, [enabled, count]);
  return set;
}

export function NumberedPager({ page, pageCount, canGoNext, onPage, limitHint }: { page: number; pageCount: number; canGoNext: boolean; onPage: (page: number) => void; limitHint: boolean }) {
  return <>
    <nav className="numbered-pager" aria-label="一覧のページ">
      <button type="button" disabled={page <= 1} onClick={() => onPage(page - 1)}>前へ</button>
      <select aria-label="ページ番号" value={page} onChange={(event) => onPage(Number(event.target.value))}>{Array.from({ length: pageCount }, (_, index) => <option key={index + 1} value={index + 1}>{index + 1} / {pageCount} ページ</option>)}</select>
      <button type="button" disabled={page >= pageCount || !canGoNext} onClick={() => onPage(page + 1)}>次へ</button>
    </nav>
    {limitHint && <p className="paging-relevance-hint">残りの作品は「スクロールで自動」で表示できます。</p>}
  </>;
}

export function WorkListSkeleton({ count = 6 }: { count?: number }) {
  return <div className="work-skeleton-grid" role="status" aria-label="読み込んでいます">{Array.from({ length: count }, (_, index) => <WorkCardSkeleton key={index} />)}</div>;
}

export function useSavedWorks(savedRevision: number) {
  return useQuery({ queryKey: ["offline-works", savedRevision], queryFn: listOfflineWorks, placeholderData: keepPreviousData, staleTime: Infinity }).data ?? [];
}

/** 一覧の作品それぞれの保存状態。端末に保存した作品だけ、編集版が変わっていないかを問い合わせる。 */
export function useSavedStates(items: PcWork[], savedRevision: number, reachable: boolean) {
  const savedWorks = useSavedWorks(savedRevision);
  const savedById = useMemo(() => new Map(savedWorks.map((item) => [item.work.id, item])), [savedWorks]);
  const savedVisible = useMemo(() => items.filter((work) => savedById.has(work.id)).map((work) => work.id), [items, savedById]);
  const editHashes = useQuery({
    queryKey: ["saved-edit-hashes", savedVisible],
    enabled: reachable && savedVisible.length > 0,
    placeholderData: keepPreviousData,
    queryFn: async () => {
      const responses = await Promise.allSettled(savedVisible.map(async (id) => [id, (await getPcMetadata(id)).activeEditHash] as const));
      return Object.fromEntries(responses.flatMap((response) => response.status === "fulfilled" ? [response.value] : [])) as Record<number, string | null>;
    },
  }).data ?? {};
  return (work: PcWork): SavedState | null => {
    const saved = savedById.get(work.id);
    if (!saved) return null;
    return { changed: work.currentVersion !== saved.savedVersion || work.contentHash !== saved.savedContentHash || (Object.prototype.hasOwnProperty.call(editHashes, work.id) && editHashes[work.id] !== saved.savedEditHash) };
  };
}

/** 作品の一覧。読み込み中も枠の形を保ち、前の結果を消さない。 */
export function WorkResults({ list, savedRevision, reachable, onOpenWork, onOpenDetail, onSaved, onPage, emptyTitle, emptyHint, omit }: {
  list: WorkList;
  savedRevision: number;
  reachable: boolean;
  onOpenWork: (work: PcWork) => void;
  onOpenDetail: (work: PcWork) => void;
  onSaved: () => void;
  onPage: (page: number) => void;
  emptyTitle: string;
  emptyHint: string;
  omit?: "author" | "series";
}) {
  const savedState = useSavedStates(list.items, savedRevision, reachable);
  const loadMore = useAutoLoad(list.hasMore && !list.fetchingMore && !list.stale, list.items.length, list.fetchMore);

  if (list.loading) return <WorkListSkeleton />;
  if (list.error && !list.items.length) return <section className="empty-card"><h2>一覧を読み込めません</h2><p>{pcErrorMessage(list.error)}</p></section>;
  return <div className="work-results" data-stale={list.stale || undefined}>
    {list.error ? <div className="inline-alert" role="alert">{pcErrorMessage(list.error)}</div> : null}
    {list.items.length ? <VirtualWorkList listKey={list.listKey} items={list.items} itemKey={(work) => work.id} renderItem={(work) => <WorkCard key={work.id} work={work} onRead={() => onOpenWork(work)} onDetail={() => onOpenDetail(work)} omit={omit} savedState={savedState(work)} onSaved={reachable ? onSaved : undefined} />} /> : <section className="empty-card"><h2>{emptyTitle}</h2><p>{emptyHint}</p></section>}
    {list.numbered && list.items.length > 0 && <NumberedPager page={list.page} pageCount={list.pageCount} canGoNext={list.items.length >= pageSize} onPage={onPage} limitHint={list.reachedDirectLimit} />}
    {!list.numbered && list.hasMore && <div className="load-more" ref={loadMore}>{list.fetchingMore ? <span className="load-more-status" role="status">続きを読み込んでいます…</span> : <button className="secondary-button" type="button" onClick={list.fetchMore}>さらに表示</button>}</div>}
  </div>;
}
