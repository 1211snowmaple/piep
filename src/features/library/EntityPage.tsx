import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Anchor,
  Avatar,
  Badge,
  Box,
  Breadcrumbs,
  Button,
  Card,
  Grid,
  Group,
  Image,
  Paper,
  Popover,
  ScrollArea,
  SegmentedControl,
  Select,
  SimpleGrid,
  Stack,
  Switch,
  Tabs,
  Text,
  TextInput,
  Timeline,
  Title,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { NoImageMark } from "@/components/NoImageMark";
import { Note } from "@/components/Note";
import { useAppNavigate, useAppSearchParams, useReturnTo, useRouteParams } from "@/app/router";
import { ActionBar } from "@/components/ActionBar";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { BoundedJsonView } from "@/components/BoundedJsonView";
import { ExpandableText } from "@/components/ExpandableText";
import { ListPager, PagingModeToggle, useBoundedNumberedPage, usePageSize, usePagingMode } from "@/components/ListPager";
import { ScrollToTop } from "@/components/ScrollToTop";
import { holdRegionInPlace, scrollRegionIntoView } from "@/lib/scroll";
import { VirtualizedWorkList } from "@/features/library/VirtualizedWorkList";
import { VirtualizedEntityGrid } from "@/features/library/VirtualizedEntityGrid";
import { parseViewMode, useViewMode } from "@/lib/viewMode";
import { boundedInfiniteListOptions, INFINITE_LIST_MAX_PAGES } from "@/lib/queryLimits";
import { externalBrand, ExternalServiceMark, getProvider, ProviderMark, sourceUrl } from "@/lib/providers";
import { errorMessage, formatBytes, formatDate, formatFreshness, formatNumber } from "@/lib/format";
import { runSingleCheck } from "@/features/updates/startSingleCheck";
import { demoFacets, searchDemoWorks } from "@/mocks/demoData";
import { exportEntityZip } from "@/services/archiveApi";
import { AuthorAssist } from "@/features/assist/AuthorAssist";
import { CollectionCard } from "@/features/collections/CollectionCard";
import { listCollectionsForPerson, listCollectionsForSeries } from "@/services/collectionApi";
import type { WorkCollectionSummary } from "@/types/collections";
import {
  getAssetUrl,
  getLatestEntityProfileJson,
  getPerson,
  getSeries,
  getUpdateTarget,
  isTauriRuntime,
  listEntityVersions,
  refreshEntityProfile,
  searchDownloadsV2,
  upsertUpdateTarget,
} from "@/services/dbApi";
import { ENTITY_SERIES_PAGE_SIZE, listEntitySeriesPage, listEntityTags } from "@/services/shelfApi";
import { saveDialog } from "@/services/dialogApi";
import { openExternalUrl } from "@/services/openerApi";
import { store } from "@/store";
import type { EntityFacet, EntityVersion, FacetCount, LibrarySortBy, PersonEntry, SeriesEntry, UpdateTarget } from "@/types/library";

/**
 * Every option says both what it sorts by and which way round.
 *
 * "タイトル" alone does not tell you whether あ or ん comes first, and "文字数"
 * does not say whether the long ones are at the top. Naming the direction in
 * the option removes the guess.
 */
const ENTITY_SORT_OPTIONS: { value: LibrarySortBy; label: string }[] = [
  { value: "source_created_at", label: "公開が新しい順" },
  { value: "downloaded_at", label: "保存が新しい順" },
  { value: "title", label: "タイトル昇順（あ→ん）" },
  { value: "text_length", label: "文字数が多い順" },
  { value: "series_order", label: "シリーズの話数順" },
];

/** Chips for the tags this author actually uses, with the rest behind a popover. */
const VISIBLE_TAG_CHIPS = 10;

export default function EntityPage({ kind }: { kind: "person" | "series" }) {
  const { source = "", sourceKey = "" } = useRouteParams(kind === "person" ? "/people/:source/:sourceKey" : "/series/:source/:sourceKey");
  // Route params are decoded by the router. Decoding a second time crashes on
  // a valid literal '%' and corrupts keys containing encoded delimiters.
  const key = sourceKey;
  const navigate = useAppNavigate();
  const returnTo = useReturnTo();
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [urlParams, setUrlParams] = useAppSearchParams();
  const tab = parseEntityTab(urlParams.get("tab"), kind);
  const tabsRef = useRef<HTMLDivElement>(null);
  const entityHeroRef = useRef<HTMLDivElement>(null);
  const entityMarksRef = useRef<HTMLDivElement>(null);
  const seriesCoverRef = useRef<HTMLDivElement>(null);
  // Switching tabs does not move the page: a tab changes what is listed, not
  // where the reader is standing. Rewriting the query string cannot move it -
  // the navigation is a replace, which scroll restoration leaves alone - but the
  // panels themselves can: the incoming one has not fetched its rows yet, so the
  // page collapses to a fraction of its height and the browser clamps the
  // reader's offset away. Holding the tabs where they are outlasts that.
  const setTab = (value: string | null) => {
    holdRegionInPlace(tabsRef.current);
    const next = new URLSearchParams(urlParams);
    if (value && value !== "works") next.set("tab", value); else next.delete("tab");
    setUrlParams(next, { replace: true });
  };
  // Narrowing lives in the URL so it survives leaving the page - the back
  // button from a work returns to the filtered listing rather than to an
  // unfiltered one - and so a narrowed view of an author can be linked to. It
  // is written with `replace`, so the narrowing itself is not a history step:
  // one entry per keystroke in the search box would bury everything else.
  const workQuery = (urlParams.get("q") ?? "").slice(0, 200);
  const activeTags = urlParams.getAll("tag").slice(0, 20);
  const sortBy = parseEntitySort(urlParams.get("sort"));
  const patchUrl = (patch: { q?: string; tags?: string[]; sort?: LibrarySortBy }) => {
    const next = new URLSearchParams(urlParams);
    if (patch.q !== undefined) { if (patch.q) next.set("q", patch.q); else next.delete("q"); }
    if (patch.tags !== undefined) {
      next.delete("tag");
      patch.tags.forEach((tag) => next.append("tag", tag));
    }
    if (patch.sort !== undefined) { if (patch.sort !== "source_created_at") next.set("sort", patch.sort); else next.delete("sort"); }
    // Page 7 of one filter is not page 7 of another.
    next.delete("page");
    setUrlParams(next, { replace: true });
  };
  const [pagingMode] = usePagingMode("entity");
  const [view, setView] = useViewMode();
  const [pageSize] = usePageSize();
  // Relevance is walked with a score cursor and has no nth page, so numbers are
  // only offered once an ordering has been chosen.
  const searchingByRelevance = Boolean(workQuery) && sortBy === "source_created_at";
  const numberedPages = pagingMode === "pages" && !searchingByRelevance;
  const {
    page: pageParam,
    maxPage: maxDirectPage,
    limitNotice: pageLimitNotice,
    clearLimitNotice,
  } = useBoundedNumberedPage(numberedPages, urlParams, setUrlParams, pageSize);
  // The next page of the works lands on the tabs, not at the top of a profile
  // the reader has already read and not halfway down a list. Set by the press:
  // arriving at this screen is also a change of address, and that one opens at
  // the very top, where the profile is.
  //
  // Spent once the new rows are on screen, so there is one movement rather than
  // a jump to the tabs followed by the rows changing underneath.
  const pendingPageScroll = useRef(false);
  const setPage = (page: number) => {
    const boundedPage = Number.isFinite(page)
      ? Math.min(maxDirectPage, Math.max(1, Math.trunc(page)))
      : 1;
    const next = new URLSearchParams(urlParams);
    if (boundedPage > 1) next.set("page", String(boundedPage)); else next.delete("page");
    setUrlParams(next, { replace: false });
    clearLimitNotice();
    pendingPageScroll.current = true;
  };
  const toggleTag = (tag: string) => patchUrl({
    tags: activeTags.includes(tag) ? activeTags.filter((item) => item !== tag) : [...activeTags, tag],
  });
  const entity = useQuery({
    queryKey: ["entity", kind, source, key],
    queryFn: async (): Promise<PersonEntry | SeriesEntry> => {
      if (runtime) return kind === "person" ? getPerson<PersonEntry>(source, key) : getSeries<SeriesEntry>(source, key);
      const facet = (kind === "person" ? demoFacets.authorEntities : demoFacets.series).find((item) => item.source === source && item.sourceKey === key) ?? (kind === "person" ? demoFacets.authorEntities[0] : demoFacets.series[0]);
      const common = { id: 1, source: facet.source, sourceKey: facet.sourceKey, coverPath: null, description: facet.description ?? null, contentHash: null, currentVersion: 2, lastCheckedAt: new Date().toISOString(), lastFetchedAt: new Date().toISOString(), createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), workCount: facet.count };
      return kind === "person" ? { ...common, displayName: facet.displayName, iconPath: null, linksJson: null } : { ...common, title: facet.displayName, isConcluded: false, publishedContentCount: facet.count + 2 };
    },
  });
  // Prolific authors have more works than any single request should return, so
  // this pages through them instead of silently stopping at a fixed slab.
  const works = useInfiniteQuery({
    ...boundedInfiniteListOptions,
    queryKey: ["entity-works", kind, source, key, workQuery, activeTags, sortBy, pageSize, numberedPages ? pageParam : 0],
    queryFn: ({ pageParam: cursor }) => runtime
      ? searchDownloadsV2({
        ...(kind === "person" ? { personSource: source, personKey: key } : { seriesSource: source, seriesKey: key }),
        text: workQuery || null,
        tagsInclude: activeTags.length ? activeTags : null,
        limit: pageSize,
        cursor,
        offset: numberedPages ? (pageParam - 1) * pageSize : null,
        // A text search inside an author ranks by relevance unless a column was
        // asked for, matching how the library itself behaves.
        sortBy: workQuery && sortBy === "source_created_at" ? "relevance" : sortBy,
        sortOrder: sortBy === "title" ? "asc" : "desc",
        projection: "libraryGallery",
      })
      : Promise.resolve(searchDemoWorks(workQuery, source)),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage, allPages) => {
      const cursor = lastPage.items.length ? lastPage.nextCursor : null;
      return cursor && !allPages.slice(0, -1).some((page) => page.nextCursor === cursor) ? cursor : undefined;
    },
    // Replacing the whole list with a spinner collapsed the page to a fraction
    // of its height, and the browser clamped the scroll position away with it -
    // which is what dropped the reader at the top of the profile on every page
    // change. Holding the previous page keeps the geometry still.
    placeholderData: keepPreviousData,
    enabled: tab === "works",
  });
  // Series search has its own URL key: `q` already belongs to the works tab.
  // Keeping the two separate means following a filtered-series link does not
  // unexpectedly turn the author's works into a relevance search as well.
  const rawSeriesQuery = urlParams.get("series_q");
  const seriesQuery = (rawSeriesQuery ?? "").trim().slice(0, 200);
  const serverSeriesQuery = seriesQuery;
  const [seriesInput, setSeriesInput] = useState(seriesQuery);
  const latestParams = useRef(urlParams);
  latestParams.current = urlParams;
  // 作品の検索欄。**URL を直に握らせない。**
  //
  // ここだけ `onChange` が毎打鍵で `history.replaceState` と IPC を撃っていた。
  // 日本語では変換中の中間状態（`a` `ao` `あ` `あお` …）まで一つずつ飛ぶうえ、
  // 制御値を変換の途中で差し戻すので、WebView2 では変換そのものが切れることが
  // ある。隣のシリーズ検索は最初からこの形で、同じ画面で作りが割れていた。
  const [workInput, setWorkInput] = useState(workQuery);
  const workComposing = useRef(false);
  const workWriteTimer = useRef<number | null>(null);
  const cancelWorkWrite = () => {
    if (workWriteTimer.current === null) return;
    window.clearTimeout(workWriteTimer.current);
    workWriteTimer.current = null;
  };
  const scheduleWorkQuery = (value: string) => {
    setWorkInput(value);
    cancelWorkWrite();
    // 変換中は確定していない。確定を待ってから書く。
    if (workComposing.current) return;
    workWriteTimer.current = window.setTimeout(() => {
      workWriteTimer.current = null;
      patchUrl({ q: value.trim().slice(0, 200) });
    }, 260);
  };
  const clearWorkQuery = () => {
    cancelWorkWrite();
    setWorkInput("");
    patchUrl({ q: "" });
  };
  const seriesWriteTimer = useRef<number | null>(null);
  const cancelSeriesWrite = () => {
    if (seriesWriteTimer.current === null) return;
    window.clearTimeout(seriesWriteTimer.current);
    seriesWriteTimer.current = null;
  };
  const writeSeriesQuery = (value: string) => {
    const bounded = value.trim().slice(0, 200);
    const next = new URLSearchParams(latestParams.current);
    if (bounded) next.set("series_q", bounded); else next.delete("series_q");
    if (next.toString() !== latestParams.current.toString()) setUrlParams(next, { replace: true });
  };
  const onSeriesQueryChange = (value: string) => {
    setSeriesInput(value);
    cancelSeriesWrite();
    seriesWriteTimer.current = window.setTimeout(() => {
      seriesWriteTimer.current = null;
      writeSeriesQuery(value);
    }, 260);
  };
  const clearSeriesQuery = () => {
    cancelSeriesWrite();
    setSeriesInput("");
    writeSeriesQuery("");
  };
  useEffect(() => {
    cancelSeriesWrite();
    setSeriesInput((current) => current === seriesQuery ? current : seriesQuery);
    // Canonicalize overlong and empty deep links without adding a history step.
    const canonical = seriesQuery || null;
    if (rawSeriesQuery === canonical) return;
    const next = new URLSearchParams(urlParams);
    if (canonical) next.set("series_q", canonical); else next.delete("series_q");
    setUrlParams(next, { replace: true });
  }, [rawSeriesQuery, seriesQuery, setUrlParams, urlParams]);
  useEffect(() => () => cancelSeriesWrite(), []);
  useEffect(() => () => cancelWorkWrite(), []);
  // 戻る・保存した検索の復元など、URL が外から変わったときは手元を合わせる。
  useEffect(() => {
    if (workWriteTimer.current === null && !workComposing.current) setWorkInput(workQuery);
  }, [workQuery]);

  // What this author writes, and who they write it with: the two things a flat
  // list of works cannot tell you. Every continuation is an opaque keyset
  // cursor; prolific authors can move beyond the old 200-result slab, up to
  // the same explicit in-memory safety stop used by other infinite listings.
  const authorSeries = useInfiniteQuery({
    ...boundedInfiniteListOptions,
    queryKey: ["entity-series", source, key, serverSeriesQuery],
    queryFn: ({ pageParam: cursor }) => runtime
      ? listEntitySeriesPage(source, key, {
        query: serverSeriesQuery || null,
        limit: ENTITY_SERIES_PAGE_SIZE,
        cursor,
      })
      : Promise.resolve({ items: [] as EntityFacet[], nextCursor: null, total: 0 }),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage, allPages) => {
      const cursor = lastPage.nextCursor;
      return cursor && !allPages.slice(0, -1).some((page) => page.nextCursor === cursor) ? cursor : undefined;
    },
    // A debounced search changes the query key. Keep the list that is already
    // painted until the replacement arrives instead of flashing a full-height
    // loading state between keystrokes.
    placeholderData: keepPreviousData,
    // **タブを開くまで待たない。** 見出しの件数はこの問い合わせの `total` から
    // 出しているので、開いてから取りに行くと数字が遅れて現れる。1ページ分
    // （60件）は開いたときに要るものでもあるので、先に取っておけば
    // タブの中身も待たされない。
    enabled: kind === "person",
    staleTime: 5 * 60_000,
  });
  // この作者・このシリーズの作品を 1 件でも含むコレクション。詳細から、
  // それが関わっているまとまりへ直接辿れるようにする。**作者にしか無いと、
  // 同じ画面で作りが割れる。**
  const entityCollections = useQuery({
    queryKey: ["collections-for-entity", kind, source, key],
    queryFn: () => runtime
      ? (kind === "person" ? listCollectionsForPerson(source, key) : listCollectionsForSeries(source, key))
      : Promise.resolve([] as WorkCollectionSummary[]),
    staleTime: 60_000,
  });
  const entityTags = useQuery({
    queryKey: ["entity-tags", kind, source, key],
    queryFn: () => runtime ? listEntityTags(kind, source, key) : Promise.resolve([] as FacetCount[]),
    staleTime: 5 * 60_000,
  });
  const versions = useQuery({
    queryKey: ["entity-versions", kind, source, key],
    queryFn: () => runtime ? listEntityVersions<EntityVersion[]>(kind, source, key) : Promise.resolve<EntityVersion[]>([{ id: 1, entityType: kind, source, sourceKey: key, version: 2, contentHash: null, jsonPath: "preview.json", assetCount: 1, fileSizeBytes: 24000, createdAt: new Date().toISOString(), changeSummary: "プロフィール更新" }]),
    enabled: tab === "history",
  });
  const profileJson = useQuery({
    queryKey: ["entity-json", kind, source, key],
    queryFn: () => runtime ? getLatestEntityProfileJson<Record<string, unknown>>(kind, source, key) : Promise.resolve(entity.data ?? {}),
    enabled: Boolean(entity.data),
  });
  const target = useQuery({
    // Same namespace as the shelf and the update centre. A singular key looked
    // tidier but shares no prefix with ["update-targets"], so turning watching
    // on here left those screens showing the old state, and vice versa.
    queryKey: ["update-targets", kind, source, key],
    queryFn: () => runtime ? getUpdateTarget<UpdateTarget>(kind === "person" ? "author" : "series", source, key) : Promise.resolve(null),
  });
  const displayName = useMemo(() => entity.data ? (kind === "person" ? (entity.data as PersonEntry).displayName : (entity.data as SeriesEntry).title) : "", [entity.data, kind]);
  const refreshMutation = useMutation({
    mutationFn: async () => {
      if (!runtime) return {};
      return refreshEntityProfile({ entityType: kind, source, sourceKey: key, force: true, refreshToken: await store.get<string>("pixiv_refresh_token"), cookie: await store.get<string>("fanbox_session_id"), userAgent: await store.get<string>("fanbox_user_agent") || "Mozilla/5.0" });
    },
    onSuccess: () => { notifications.show({ color: "green", message: "プロフィールを更新しました" }); queryClient.invalidateQueries({ queryKey: ["entity", kind, source, key] }); queryClient.invalidateQueries({ queryKey: ["entity-versions", kind, source, key] }); queryClient.invalidateQueries({ queryKey: ["entity-json", kind, source, key] }); },
    onError: (error) => notifications.show({ color: "red", title: "更新できません", message: errorMessage(error) }),
  });
  const targetMutation = useMutation({
    mutationFn: async (enabled: boolean) => {
      if (!runtime) return;
      await upsertUpdateTarget({ targetType: kind === "person" ? "author" : "series", source, sourceKey: key, displayName, enabled, metadataJson: null });
    },
    onSuccess: () => { queryClient.invalidateQueries({ queryKey: ["update-targets"] }); notifications.show({ color: "green", message: "更新監視を変更しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "更新監視を変更できません", message: errorMessage(error) }),
  });
  const workItems = useMemo(() => {
    const seen = new Set<number>();
    return works.data?.pages.flatMap((page) => page.items.filter((work) => {
      if (seen.has(work.id)) return false;
      seen.add(work.id);
      return true;
    })) ?? [];
  }, [works.data?.pages]);
  const worksAtCacheLimit = !numberedPages
    && (works.data?.pages.length ?? 0) >= INFINITE_LIST_MAX_PAGES
    && Boolean(works.hasNextPage);
  const authorSeriesItems = useMemo(() => {
    const seen = new Set<string>();
    return authorSeries.data?.pages.flatMap((page) => page.items.filter((series) => {
      const identity = `${series.source}\0${series.sourceKey}`;
      if (seen.has(identity)) return false;
      seen.add(identity);
      return true;
    })) ?? [];
  }, [authorSeries.data?.pages]);
  const authorSeriesTotal = authorSeries.data?.pages[0]?.total ?? null;
  const authorSeriesAtCacheLimit = (authorSeries.data?.pages.length ?? 0) >= INFINITE_LIST_MAX_PAGES
    && Boolean(authorSeries.hasNextPage);

  // The one movement, made when the rows for the page that was asked for have
  // arrived, in the frame they are painted in.
  const showingRequestedPage = !works.isPlaceholderData && !works.isFetching;
  // `pageParam` is in the dependencies as well as the settled flag: a page that
  // is already cached settles in the same render it was asked for, so the flag
  // never changes and an effect watching only it would never run.
  useLayoutEffect(() => {
    if (!pendingPageScroll.current || !showingRequestedPage) return;
    pendingPageScroll.current = false;
    scrollRegionIntoView(tabsRef.current);
  }, [pageParam, showingRequestedPage]);
  // 作品と同じ視線の始点にする。操作列の高さを決め打ちせず、実際の
  // 保存元表示（pixiv など）の上辺へ表紙を合わせる。
  useLayoutEffect(() => {
    if (kind !== "series") return;
    const hero = entityHeroRef.current;
    const marks = entityMarksRef.current;
    const cover = seriesCoverRef.current;
    if (!hero || !marks || !cover || !entity.data) return;
    const align = () => {
      // The cover already carries the previous measurement as margin. Remove
      // that value from its rendered position so repeated ResizeObserver runs
      // keep measuring the same, unshifted origin instead of toggling to zero.
      const appliedMargin = Number.parseFloat(getComputedStyle(cover).marginTop) || 0;
      const coverOrigin = cover.getBoundingClientRect().top - appliedMargin;
      const offset = Math.max(0, marks.getBoundingClientRect().top - coverOrigin);
      const value = `${Math.round(offset)}px`;
      if (hero.style.getPropertyValue("--series-cover-mark-offset") !== value) {
        hero.style.setProperty("--series-cover-mark-offset", value);
      }
    };
    align();
    const observer = new ResizeObserver(align);
    observer.observe(hero);
    observer.observe(marks);
    window.addEventListener("resize", align);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", align);
    };
  }, [entity.data, kind]);

  if (entity.isLoading) return <div className="page"><LoadingState /></div>;
  if (entity.error || !entity.data) return <div className="page"><ErrorState error={entity.error ?? "情報がありません"} retry={() => entity.refetch()} /></div>;
  const entry = entity.data;
  // シリーズで知りたいのは日付ではなく「何話取りこぼしているか」である。
  // 数が合っているときは差を出さない - そこに情報は無い。
  const seriesEntry = kind === "series" ? (entry as SeriesEntry) : null;
  const publishedCount = seriesEntry?.publishedContentCount ?? null;
  const localCount = entry.workCount ?? 0;
  const missingCount = publishedCount === null ? 0 : Math.max(0, publishedCount - localCount);
  const profileData = (profileJson.data ?? {}) as Record<string, any>;
  const profileStats = profileData.stats as Record<string, number> | undefined;
  const coverPath = entry.coverPath;
  const avatarPath = kind === "person" ? (entry as PersonEntry).iconPath : coverPath;
  // 保存元が画像を持ちうるのに置かれていない場合は、保存元と同じく一枚の絵で
  // 示す。概念そのものが無い保存元は、今までどおり記号のまま。人物に限るのは、
  // あの一枚がプロフィール画像の代わりであって表紙の代わりではないからである。
  const noImage = kind === "person" && !avatarPath && getProvider(source).hasProfileImages;
  const sourceProfileUrl = sourceUrl(source, key, kind);
  const profileLinks = kind === "person" ? profileLinkList(parseLinks((entry as PersonEntry).linksJson), sourceProfileUrl) : [];
  // 「開く」には行き先が2つある。アプリ内なら内蔵ブラウザで開き、そのまま
  // 保存へ進める。外部なら普段のブラウザ（ログイン済みの環境）で開く。
  const openSourceExternally = async () => {
    const url = sourceProfileUrl;
    if (!url) return;
    if (runtime) await openExternalUrl(url); else window.open(url, "_blank", "noopener,noreferrer");
  };
  const openSourceInApp = () => {
    const url = sourceProfileUrl;
    if (!url) return;
    navigate(`/save/${source}?url=${encodeURIComponent(url)}`);
  };
  const exportZip = async () => {
    if (!runtime) return notifications.show({ color: "piep", message: "書き出しはデスクトップアプリで利用できます" });
    const path = await saveDialog({ title: "アーカイブを書き出す", defaultPath: `${displayName.replace(/[\\/:*?"<>|]/g, "_")}.zip`, filters: [{ name: "ZIP archive", extensions: ["zip"] }] });
    if (!path) return;
    try { await exportEntityZip(kind, source, key, path); notifications.show({ color: "green", title: "書き出しました", message: path }); }
    catch (error) { notifications.show({ color: "red", title: "書き出しに失敗しました", message: errorMessage(error) }); }
  };

  return (
    <div className="page page--contained entity-page">
      {/* Returns to the listing rather than opening a bare one: the search, the
          sort and the row the reader came from are all still on the entry
          behind this one, and pushing a clean copy throws them away. */}
      <Breadcrumbs mb="md"><Anchor component="button" type="button" size="sm" onClick={() => returnTo(`/library?tab=${kind === "person" ? "people" : "series"}`)}>{kind === "person" ? "作者・クリエイター" : "シリーズ"}</Anchor><Text size="sm" c="dimmed">{displayName}</Text></Breadcrumbs>
      <Card ref={entityHeroRef} className="entity-hero" data-kind={kind} padding={0}>
        {kind === "person" && coverPath && <Box className="entity-hero__banner"><Image src={getAssetUrl(coverPath)} alt={`${displayName}のヘッダー画像`} /></Box>}
        <Box className="entity-hero__body">
          <Box className="entity-hero__primary">
            <Group align="flex-start" wrap="nowrap" miw={0} className="entity-hero__identity">
              {kind === "person"
                ? <Avatar className="entity-hero__avatar" src={getAssetUrl(avatarPath)} size={112} radius="xl" color="piep">{noImage ? <NoImageMark /> : <Icons.person size={IconSize.avatar} />}</Avatar>
                : <Box ref={seriesCoverRef} className="entity-hero__series-cover">{avatarPath ? <Image src={getAssetUrl(avatarPath)} alt={`${displayName}の表紙`} fit="contain" /> : <Icons.series size={IconSize.avatar} />}</Box>}
              <Stack gap={7} mb={5} miw={0} align="flex-start" className="entity-hero__identity-copy">
                {/* 作品詳細と同じく、操作を上、名前を下に置く。別段なので長い
                    正式名称の表示幅は奪わない。 */}
                <Box className="entity-hero__actions">
                  <ActionBar
                    label={`${displayName}の操作`}
                    items={[
                      { key: "in-app", label: "アプリ内で開く", icon: Icons.inAppBrowser, onClick: openSourceInApp },
                      { key: "browser", label: "ブラウザで開く", icon: Icons.externalLink, onClick: openSourceExternally },
                      { key: "archive", label: "アーカイブ", icon: Icons.archive, onClick: exportZip },
                      { key: "refresh", label: "情報を更新", icon: Icons.watch, primary: true, loading: refreshMutation.isPending, onClick: () => refreshMutation.mutate() },
                    ]}
                  />
                </Box>
                <Box ref={entityMarksRef}><ProviderMark provider={source} /></Box>
                <Title order={1} className="entity-hero__title">{displayName}</Title>{/* かつてここには「更新 …」と出ていたが、指していたのは取得元での更新では
                    なく piep の行が書き換わった日だった。読む側には区別がつかない。
                    取得元の話をしないなら、手元の言葉で言う。確認していなければ何も出さない。 */}
                <Group gap="xs">{seriesEntry ? <><Badge variant="light" color="gray">{missingCount > 0 ? `手元 ${formatNumber(localCount)}話 / 取得元 ${formatNumber(publishedCount ?? 0)}話` : `${formatNumber(localCount)}話`}</Badge>{missingCount > 0 && <Badge variant="light" color="yellow">{formatNumber(missingCount)}話 未取得</Badge>}{seriesEntry.isConcluded && <Badge variant="light" color="gray">完結</Badge>}</> : <Badge variant="light" color="gray">{formatNumber(localCount)}作品</Badge>}{entry.lastCheckedAt && <Text size="xs" c="dimmed">{formatFreshness(entry.lastCheckedAt)}に確認</Text>}</Group>
              </Stack>
            </Group>
          </Box>
          <Stack gap="md" mt="lg">
            {/* Profiles run from one line to several screens of release notes
                and shop links; unfolded, the long ones pushed the works this
                page exists for off the bottom of the window. */}
            {entry.description && <ExpandableText className="entity-description" maw={860} lines={4} label={kind === "person" ? "プロフィール" : "概要"}>{entry.description}</ExpandableText>}
            {profileLinks.length > 0 && <Group gap="xs" className="profile-links">{profileLinks.map((url) => { const brand = externalBrand(url); return <Button key={url} className="profile-link" style={{ "--profile-link-color": brand.color }} variant="default" aria-label={`${brand.label}をブラウザで開く`} onClick={() => runtime ? openExternalUrl(url) : window.open(url, "_blank", "noopener,noreferrer")}><ExternalServiceMark url={url} /><Text component="span" size="xs" c="dimmed" className="profile-link__value">{profileLinkValue(url)}</Text><Icons.externalLink size={IconSize.inline} className="profile-link__out" aria-hidden /></Button>; })}</Group>}
          </Stack>
        </Box>
      </Card>

      <Grid gap="lg" mt="lg">
        <Grid.Col span={{ base: 12, lg: 9 }} ref={tabsRef}>
          {/* The panels are torn down rather than hidden. A kept-mounted panel
              still holds a virtualiser bound to the page's own scroller, and
              revealing it re-attaches that virtualiser and drags the page to
              whatever offset it was left holding - which, since it was hidden
              while the page was at its shortest, is the top. */}
          <Tabs value={tab} onChange={setTab} keepMounted={false}>
            <Tabs.List>
              <Tabs.Tab value="works" leftSection={<Icons.read size={IconSize.inline} />}>作品</Tabs.Tab>
              {kind === "person" && <Tabs.Tab value="series" leftSection={<Icons.series size={IconSize.inline} />} rightSection={authorSeriesTotal !== null ? <Badge size="xs" variant="light">{formatNumber(authorSeriesTotal)}</Badge> : undefined}>シリーズ</Tabs.Tab>}
              {entityCollections.data && entityCollections.data.length > 0 && <Tabs.Tab value="collections" leftSection={<Icons.collection size={IconSize.inline} />} rightSection={<Badge size="xs" variant="light">{formatNumber(entityCollections.data.length)}</Badge>}>コレクション</Tabs.Tab>}
              <Tabs.Tab value="history">プロフィール履歴</Tabs.Tab>
              <Tabs.Tab value="json">JSON</Tabs.Tab>
            </Tabs.List>
            <Tabs.Panel value="works" pt="lg">
              <Stack gap="sm">
                {/* 作風のメモ。作者のときだけ、設定してあるときだけ出る。 */}
                {kind === "person" && <AuthorAssist source={source} personKey={key} />}
                <Group gap="sm" wrap="nowrap" align="center">
                  <TextInput
                    flex={1}
                    size="sm"
                    value={workInput}
                    onChange={(event) => scheduleWorkQuery(event.currentTarget.value)}
                    onCompositionStart={() => { workComposing.current = true; cancelWorkWrite(); }}
                    onCompositionEnd={(event) => { workComposing.current = false; scheduleWorkQuery(event.currentTarget.value); }}
                    leftSection={<Icons.search size={IconSize.menu} />}
                    rightSection={workInput ? <ActionIcon variant="subtle" color="gray" size="sm" aria-label="検索語を消す" onClick={clearWorkQuery}><Icons.cancel size={IconSize.inline} /></ActionIcon> : null}
                    placeholder={`${displayName}の作品を検索`}
                    aria-label={`${displayName}の作品を検索`}
                    maxLength={200}
                  />
                  <Select
                    size="sm"
                    w={188}
                    value={sortBy}
                    onChange={(value) => patchUrl({ sort: parseEntitySort(value) })}
                    data={ENTITY_SORT_OPTIONS}
                    leftSection={<Icons.sort size={IconSize.menu} />}
                    aria-label="並び順"
                    allowDeselect={false}
                  />
                </Group>
                {(entityTags.data?.length ?? 0) > 0 && (
                  <TagFilterBar
                    tags={entityTags.data ?? []}
                    active={activeTags}
                    onToggle={toggleTag}
                    onClear={() => patchUrl({ tags: [] })}
                  />
                )}
              </Stack>
              {pageLimitNotice && (
                <Alert color="yellow" title="ページ番号を調整しました" role="status" mt="md">
                  負荷を抑えるため、直接開けるのは{maxDirectPage}ページ目までです。それより先は「自動」で続きを読み込むか、作品検索・タグで対象を狭めてください。
                </Alert>
              )}
              <Box mt="md">
                {works.isLoading ? <LoadingState /> : works.error ? <ErrorState error={works.error} retry={() => works.refetch()} /> : workItems.length ? (
                  <>
                    <Group gap="xs" mb="sm" wrap="nowrap">
                      <Text size="sm" c="dimmed">{formatNumber(works.data?.pages[0]?.totalEstimate ?? workItems.length)}件</Text>
                      {/* Beside the count, which is the thing it changes. */}
                      <PagingModeToggle scope="entity" />
                      {/* ここだけ view を "gallery" に固定していた。棚で一覧を選んでいても
                          作者を開いた瞬間にカードへ戻る、同じ作品の違う顔だった。 */}
                      <SegmentedControl
                        className="view-mode-switch"
                        ml="auto"
                        value={view}
                        onChange={(value) => setView(parseViewMode(value))}
                        data={[
                          { value: "gallery", label: <Tooltip label="ギャラリー"><Icons.viewGrid size={IconSize.menu} aria-label="ギャラリー表示" /></Tooltip> },
                          { value: "compact", label: <Tooltip label="リスト"><Icons.viewList size={IconSize.menu} aria-label="リスト表示" /></Tooltip> },
                        ]}
                        aria-label="表示形式"
                      />
                    </Group>
                    <VirtualizedWorkList items={workItems} view={view} />
                    <ListPager
                      scope="entity"
                      hasNext={Boolean(works.hasNextPage) && !worksAtCacheLimit}
                      loading={works.isFetchingNextPage || works.isFetching}
                      loaded={workItems.length}
                      total={works.data?.pages[0]?.totalEstimate ?? null}
                      onLoad={() => works.fetchNextPage()}
                      endMessage={worksAtCacheLimit
                        ? searchingByRelevance
                          ? `メモリ使用量を抑えるため${formatNumber(workItems.length)}件で停止しました。検索条件を絞ると続きへ到達できます。`
                          : `メモリ使用量を抑えるため${formatNumber(workItems.length)}件で停止しました。「ページ番号」に切り替えると続きへ移動できます。`
                        : undefined}
                      pages={{
                        current: pageParam,
                        size: pageSize,
                        onGoTo: setPage,
                        maxDirectPage,
                        limitNotice: pageLimitNotice,
                        unavailableReason: searchingByRelevance
                          ? "関連度順はページ番号で移動できません。並び順を選ぶとページ番号が使えます。"
                          : null,
                      }}
                    />
                  </>
                ) : workQuery || activeTags.length ? (
                  <EmptyState
                    icon={Icons.search}
                    title="条件に合う作品がありません"
                    description="検索語やタグを減らしてください。"
                    action={<Button variant="light" onClick={() => patchUrl({ q: "", tags: [] })}>絞り込みを解除</Button>}
                  />
                ) : <EmptyState icon={Icons.read} title="保存作品がありません" description="このページを保存ワークスペースで開き、作品を取り込んでください。" action={<Button onClick={() => navigate(`/save/${source}`)}>保存ワークスペースを開く</Button>} />}
              </Box>
            </Tabs.Panel>
            {kind === "person" && (
              <Tabs.Panel value="series" pt="lg" aria-busy={authorSeries.isFetching}>
                <Stack gap="md">
                  <TextInput
                    value={seriesInput}
                    onChange={(event) => onSeriesQueryChange(event.currentTarget.value)}
                    leftSection={<Icons.search size={IconSize.menu} />}
                    rightSection={seriesInput ? <ActionIcon variant="subtle" color="gray" size="sm" aria-label="シリーズの検索語を消す" onClick={clearSeriesQuery}><Icons.cancel size={IconSize.inline} /></ActionIcon> : null}
                    label="この作者のシリーズを検索"
                    description="保存済みシリーズ全体を名前・説明から検索します"
                    placeholder="シリーズ名"
                    maxLength={200}
                  />
                  {authorSeries.isFetching && !authorSeries.isFetchingNextPage && !authorSeries.isLoading
                    ? <Text size="xs" c="dimmed" role="status">シリーズを検索しています</Text>
                    : null}
                  {authorSeries.isLoading ? <LoadingState /> : authorSeries.error && authorSeriesItems.length === 0 ? <ErrorState error={authorSeries.error} retry={() => authorSeries.refetch()} /> : authorSeriesItems.length > 0 ? (
                    <>
                      {authorSeries.isFetchNextPageError && (
                        <Alert color="red" title="シリーズの続きを読み込めません" role="alert">
                          <Stack gap="xs">
                            <Text size="sm">ライブラリの更新でカーソルが古くなった可能性があります。表示中のシリーズはそのまま残しています。</Text>
                            <Group gap="xs">
                              <Button size="xs" variant="light" color="red" onClick={() => authorSeries.fetchNextPage()}>もう一度試す</Button>
                              <Button size="xs" variant="default" onClick={() => queryClient.resetQueries({ queryKey: ["entity-series", source, key, serverSeriesQuery], exact: true })}>先頭から読み直す</Button>
                            </Group>
                          </Stack>
                        </Alert>
                      )}
                      <Text size="sm" c="dimmed" role="status" aria-live="polite">
                        {authorSeriesTotal === null
                          ? `${formatNumber(authorSeriesItems.length)}件を表示中`
                          : `${formatNumber(authorSeriesItems.length)} / ${formatNumber(authorSeriesTotal)}件を表示中`}
                      </Text>
                      <VirtualizedEntityGrid items={authorSeriesItems} kind="series" />
                      <ListPager
                        scope="entity"
                        hasNext={Boolean(authorSeries.hasNextPage) && !authorSeriesAtCacheLimit}
                        loading={authorSeries.isFetchingNextPage}
                        loaded={authorSeriesItems.length}
                        total={authorSeriesTotal}
                        onLoad={() => authorSeries.fetchNextPage()}
                        endMessage={authorSeriesAtCacheLimit
                          ? `メモリ使用量を抑えるため${formatNumber(authorSeriesItems.length)}件で停止しました。検索語を追加すると残りのシリーズへ到達できます。`
                          : undefined}
                        pages={{
                          current: 1,
                          size: ENTITY_SERIES_PAGE_SIZE,
                          onGoTo: () => undefined,
                          unavailableReason: "作者シリーズはカーソルで順に読み込みます。ページ番号への直接移動は利用できません。",
                        }}
                      />
                    </>
                  ) : serverSeriesQuery ? (
                    <EmptyState icon={Icons.search} title="一致するシリーズがありません" description="検索語を短くするか、別の名前を試してください。" action={<Button variant="light" onClick={clearSeriesQuery}>検索を解除</Button>} />
                  ) : <EmptyState icon={Icons.series} title="シリーズはありません" description="この作者の保存済み作品は、どのシリーズにも属していません。" />}
                </Stack>
              </Tabs.Panel>
            )}
            <Tabs.Panel value="collections" pt="lg">
              <Stack gap="sm">
                <Text size="sm" c="dimmed">{displayName}の作品を含むコレクションです。{kind === "person" ? "ほかの作者" : "ほかのシリーズ"}の作品が一緒に入っている場合もあります。</Text>
                {entityCollections.isLoading ? <LoadingState /> : entityCollections.error ? <ErrorState error={entityCollections.error} retry={() => entityCollections.refetch()} /> : (entityCollections.data ?? []).length === 0
                  ? <EmptyState icon={Icons.collection} title="コレクションはありません" description={`この${kind === "person" ? "作者" : "シリーズ"}の作品は、まだどのコレクションにも入っていません。`} />
                  : <SimpleGrid cols={{ base: 1, md: 2 }}>{(entityCollections.data ?? []).map((collection) => (
                      <CollectionCard key={collection.id} collection={collection} />
                    ))}</SimpleGrid>}
              </Stack>
            </Tabs.Panel>
            <Tabs.Panel value="history" pt="lg"><Paper p="lg" withBorder>{versions.isLoading ? <LoadingState /> : <Timeline active={versions.data?.length ?? 0}>{versions.data?.map((version) => <Timeline.Item key={version.id} bullet={<Icons.versionHistory size={IconSize.inline} />} title={`バージョン ${version.version}`}><Text size="sm" c="dimmed">{version.changeSummary || "プロフィールを保存"}</Text><Text size="xs" c="dimmed" mt={4}>{formatDate(version.createdAt, true)} · {formatBytes(version.fileSizeBytes)}</Text></Timeline.Item>)}</Timeline>}</Paper></Tabs.Panel>
            <Tabs.Panel value="json" pt="lg">{tab !== "json" ? null : profileJson.isLoading ? <LoadingState /> : profileJson.error ? <ErrorState error={profileJson.error} retry={() => profileJson.refetch()} /> : <BoundedJsonView value={profileJson.data ?? {}} />}</Tabs.Panel>
          </Tabs>
        </Grid.Col>
        <Grid.Col span={{ base: 12, lg: 3 }}>
          <Stack gap="lg">
            <Card p="lg"><Stack gap="md"><Group justify="space-between"><Box><Text fw={700}>新着を監視</Text><Text size="xs" c="dimmed" mt={4}>{kind === "person" ? "新しい作品を検出" : "シリーズの続編を検出"}</Text></Box><Switch checked={targetMutation.isPending ? targetMutation.variables : target.data?.enabled ?? false} disabled={targetMutation.isPending || target.isLoading} onChange={(event) => targetMutation.mutate(event.currentTarget.checked)} aria-label="新着の更新監視" /></Group>{/* 監視に入れなくても、その場で一度だけ見に行ける。 */}<Button variant="light" leftSection={<Icons.updates size={IconSize.menu} />} onClick={() => runSingleCheck({ kind: kind === "person" ? "author" : "series", source, sourceKey: key, label: displayName }, () => navigate("/updates"))}>いま新作を確認</Button></Stack></Card>
            <Card p="lg"><Text fw={700} mb="md">プロフィール情報</Text><Stack gap="sm">{profileData.account && <Meta label="アカウント" value={`@${profileData.account}`} />}{profileStats && <><Meta label="小説" value={`${formatNumber(profileStats.totalNovels ?? 0)}作品`} /><Meta label="小説シリーズ" value={`${formatNumber(profileStats.totalNovelSeries ?? 0)}件`} /></>}{typeof profileData.sampleNovelCount === "number" && <Meta label="取得済み構成作品" value={`${formatNumber(profileData.sampleNovelCount)}件`} />}<Meta label="現在のバージョン" value={`v${entry.currentVersion}`} /><Meta label="最終取得" value={formatDate(entry.lastFetchedAt, true)} /><Meta label="最終確認" value={formatDate(entry.lastCheckedAt, true)} /></Stack></Card>
            {!runtime && <Note>プレビューではデモプロフィールを表示しています。</Note>}
          </Stack>
        </Grid.Col>
      </Grid>
      {/* A prolific author's works list is as long as the library's own. */}
      <ScrollToTop />
    </div>
  );
}

function parseEntityTab(value: string | null, kind: "person" | "series"): "works" | "series" | "collections" | "history" | "json" {
  if (value === "history" || value === "json" || value === "collections") return value;
  // シリーズの中にシリーズは無い。コレクションは作者にもシリーズにもある。
  if (value === "series" && kind === "person") return value;
  return "works";
}

function parseEntitySort(value: unknown): LibrarySortBy {
  const allowed = new Set(ENTITY_SORT_OPTIONS.map((option) => option.value));
  return typeof value === "string" && allowed.has(value as LibrarySortBy)
    ? value as LibrarySortBy
    : "source_created_at";
}

/**
 * The tags this author uses, as filters rather than decoration.
 *
 * A prolific author's page is unusable as one flat list; their own tags are the
 * dimension that actually divides their work, so they narrow it in place
 * instead of sending the reader off to the library with a query.
 */
function TagFilterBar({ tags, active, onToggle, onClear }: {
  tags: FacetCount[];
  active: string[];
  onToggle: (tag: string) => void;
  onClear: () => void;
}) {
  // A selected tag stays visible even when it is not among the most used, or
  // the control that switched it on would vanish under the reader's cursor.
  const visible = tags.slice(0, VISIBLE_TAG_CHIPS);
  const shown = [...visible, ...tags.filter((tag) => active.includes(tag.name) && !visible.includes(tag))];
  const rest = tags.filter((tag) => !shown.includes(tag));
  return (
    <Group gap={6}>
      <Icons.tag size={IconSize.inline} aria-hidden style={{ opacity: 0.6 }} />
      {shown.map((tag) => (
        <Badge
          key={tag.name}
          component="button"
          type="button"
          variant={active.includes(tag.name) ? "filled" : "light"}
          color={active.includes(tag.name) ? "piep" : "gray"}
          style={{ cursor: "pointer", textTransform: "none" }}
          rightSection={<Text component="span" size="9px" opacity={0.75}>{formatNumber(tag.count)}</Text>}
          onClick={() => onToggle(tag.name)}
          aria-pressed={active.includes(tag.name)}
        >
          {tag.name}
        </Badge>
      ))}
      {rest.length > 0 && (
        <Popover width={300} position="bottom-start" withArrow shadow="md">
          <Popover.Target>
            <Badge variant="default" style={{ cursor: "pointer", textTransform: "none" }}>他{formatNumber(rest.length)}件</Badge>
          </Popover.Target>
          <Popover.Dropdown>
            <ScrollArea.Autosize mah={260}>
              <Group gap={6}>
                {rest.map((tag) => (
                  <Badge
                    key={tag.name}
                    component="button"
                    type="button"
                    variant="light"
                    color="gray"
                    style={{ cursor: "pointer", textTransform: "none" }}
                    rightSection={<Text component="span" size="9px" opacity={0.75}>{formatNumber(tag.count)}</Text>}
                    onClick={() => onToggle(tag.name)}
                  >
                    {tag.name}
                  </Badge>
                ))}
              </Group>
            </ScrollArea.Autosize>
          </Popover.Dropdown>
        </Popover>
      )}
      {active.length > 0 && <Button size="compact-xs" variant="subtle" color="gray" onClick={onClear}>タグを解除</Button>}
    </Group>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return <Group justify="space-between" gap="md"><Text size="xs" c="dimmed">{label}</Text><Text size="xs" fw={650} ta="right">{value}</Text></Group>;
}

/**
 * Trims the prose a profile link was extracted from.
 *
 * Profiles write links inline - `twitter【https://twitter.com/name】※進捗報告`
 * - and the extractor keeps everything up to the next space, so the bracket and
 * the sentence after it end up inside the URL. The same account then appears
 * twice, once clean and once with a tail, because the two strings differ.
 */
export function cleanProfileLink(raw: string): string | null {
  const match = /^https?:\/\/\S+/.exec(raw.trim());
  if (!match) return null;
  // Cut at the first character that cannot be part of a URL. Full-width
  // brackets and CJK text are prose that ran into it.
  let url = match[0].split(/[】【〕〔）（＞＜「」『』、。※\s]/)[0];
  // A closing bracket or sentence punctuation at the very end is never part of
  // the address either.
  url = url.replace(/[)\]}>,.;:!?'"｝］〉》]+$/, "");
  try {
    const parsed = new URL(url);
    if (!/^https?:$/.test(parsed.protocol)) return null;
    return parsed.toString();
  } catch {
    return null;
  }
}

/** Two links are the same account however they were written down. */
function profileLinkIdentity(url: string): string {
  try {
    const parsed = new URL(url);
    const host = parsed.hostname.replace(/^www\./, "").toLowerCase();
    const path = parsed.pathname.replace(/\/+$/, "").toLowerCase();
    return `${host}${path}`;
  } catch {
    return url.toLowerCase();
  }
}

/** Distinct profile links, cleaned of the prose they were written in. */
export function profileLinkList(rawLinks: string[], extra: string | null): string[] {
  const seen = new Map<string, string>();
  for (const candidate of [...rawLinks, ...(extra ? [extra] : [])]) {
    const url = cleanProfileLink(candidate);
    if (!url) continue;
    const identity = profileLinkIdentity(url);
    // Keep the first, shortest form: a later copy is the one with a tail.
    const existing = seen.get(identity);
    if (!existing || url.length < existing.length) seen.set(identity, url);
  }
  return [...seen.values()];
}

function parseLinks(raw: string | null): string[] {
  try { return (JSON.parse(raw ?? "[]") as unknown[]).filter((value): value is string => typeof value === "string"); }
  catch { return []; }
}

function profileLinkValue(url: string): string {
  try {
    const parsed = new URL(url);
    const path = decodeURIComponent(parsed.pathname).replace(/^\//, "").replace(/\/$/, "");
    if (/^(x\.com|twitter\.com)$/i.test(parsed.hostname.replace(/^www\./, "")) && path) return `@${path.split("/")[0]}`;
    return path ? path.replace(/^@/, "@") : parsed.hostname.replace(/^www\./, "");
  } catch { return url; }
}
