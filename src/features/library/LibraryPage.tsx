import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Checkbox,
  Combobox,
  Divider,
  Drawer,
  Group,
  InputBase,
  Indicator,
  Menu,
  NumberInput,
  Paper,
  Pill,
  PillsInput,
  Popover,
  Radio,
  ScrollArea,
  SegmentedControl,
  Select,
  SimpleGrid,
  Stack,
  Tabs,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
  useCombobox,
} from "@mantine/core";
import { useDebouncedValue, useDisclosure, useLocalStorage } from "@mantine/hooks";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { EntityCard } from "@/components/EntityCard";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { ListPager, PagingModeToggle, usePageSize, usePagingMode } from "@/components/ListPager";
import { ScrollToTop } from "@/components/ScrollToTop";
import { demoFacets, searchDemoWorks } from "@/mocks/demoData";
import { errorMessage, formatNumber } from "@/lib/format";
import { VirtualizedWorkList } from "@/features/library/VirtualizedWorkList";
import {
  deleteDownloads,
  getFilterFacets,
  isTauriRuntime,
  searchDownloadsV2,
  searchEntityFacets,
  searchFilterFacets,
  setFavorite,
  setFlagsForIds,
  setWatchUpdates,
} from "@/services/dbApi";
import { searchSuggest } from "@/services/searchApi";
import { deleteSavedSearch, listSavedSearches, upsertSavedSearch } from "@/services/shelfApi";
import { forgetReadingPositions, readingWorkIds } from "@/features/library/readingShelf";
import { useSavedSearchMigration } from "@/features/library/savedSearchMigration";
import type {
  FacetCount,
  LibrarySortBy,
  LibraryWatchFilter,
  SavedSearchRecord,
  SearchSuggestion,
  SearchV2Params,
  SearchV2Result,
} from "@/types/library";

type LibraryTab = "works" | "people" | "series";
type ViewMode = "gallery" | "compact";

interface Filters {
  sources: string[];
  contentType: string | null;
  favorite: boolean;
  watch: LibraryWatchFilter | null;
  tagsInclude: string[];
  tagsExclude: string[];
  tagMode: "and" | "or";
  minChars: number | string;
  maxChars: number | string;
}

const initialFilters: Filters = {
  sources: [], contentType: null, favorite: false, watch: null,
  tagsInclude: [], tagsExclude: [], tagMode: "and", minChars: "", maxChars: "",
};

const MAX_SEARCH_LENGTH = 512;
/** Each option names its direction, so no ordering has to be guessed at. */
const SORT_OPTIONS: { value: LibrarySortBy; label: string }[] = [
  { value: "downloaded_at", label: "保存が新しい順" },
  { value: "source_updated_at", label: "更新が新しい順" },
  { value: "title", label: "タイトル昇順（あ→ん）" },
  { value: "author_name", label: "作者名昇順（あ→ん）" },
  { value: "text_length", label: "文字数が多い順" },
  { value: "file_size_bytes", label: "容量が大きい順" },
];
/** Only offered while searching, where it is the default and the backend ranks
 *  by score instead of by a column. */
const RELEVANCE_SORT: { value: LibrarySortBy; label: string } = { value: "relevance", label: "関連度が高い順" };
const SEARCH_SORT_OPTIONS = [RELEVANCE_SORT, ...SORT_OPTIONS];
const SORT_VALUES = new Set<LibrarySortBy>(SEARCH_SORT_OPTIONS.map((option) => option.value));

function parseLibraryTab(value: string | null): LibraryTab {
  return value === "people" || value === "series" ? value : "works";
}

function parseWatchFilter(value: unknown): LibraryWatchFilter | null {
  return value === "watched" || value === "unwatched" ? value : null;
}

function parseSortBy(value: unknown): LibrarySortBy {
  return typeof value === "string" && SORT_VALUES.has(value as LibrarySortBy) ? value as LibrarySortBy : "downloaded_at";
}

/**
 * Relevance is the default while searching and is not offered otherwise, so the
 * default sort depends on whether there is a query. Clearing the search box
 * therefore has to fall back rather than ask for a ranking that has no meaning.
 */
function resolveSortBy(raw: string | null, hasQuery: boolean): LibrarySortBy {
  const parsed = typeof raw === "string" && SORT_VALUES.has(raw as LibrarySortBy) ? raw as LibrarySortBy : null;
  if (!hasQuery) return parsed && parsed !== "relevance" ? parsed : "downloaded_at";
  return parsed ?? "relevance";
}

/** Ascending only reads as "correct" for the alphabetical keys. */
function sortOrderFor(sortBy: LibrarySortBy): "asc" | "desc" {
  return sortBy === "title" || sortBy === "author_name" ? "asc" : "desc";
}

function parseViewMode(value: unknown): ViewMode {
  return value === "compact" ? "compact" : "gallery";
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const result: string[] = [];
  const seen = new Set<string>();
  for (const item of value) {
    if (result.length >= 200) break;
    if (typeof item !== "string") continue;
    const normalized = item.slice(0, 200).trim();
    if (normalized && !seen.has(normalized)) { seen.add(normalized); result.push(normalized); }
  }
  return result;
}

function numericFilter(value: unknown): number | "" {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : "";
}

function numericFilterOrNull(value: unknown): number | null {
  const normalized = numericFilter(value);
  return normalized === "" ? null : normalized;
}

function normalizeFilters(value: unknown): Filters {
  if (!value || typeof value !== "object") return initialFilters;
  const candidate = value as Partial<Filters>;
  const tagsInclude = stringList(candidate.tagsInclude);
  const included = new Set(tagsInclude);
  return {
    sources: stringList(candidate.sources).filter((source) => source === "pixiv" || source === "fanbox"),
    contentType: typeof candidate.contentType === "string" && candidate.contentType.trim() ? candidate.contentType.trim() : null,
    favorite: candidate.favorite === true,
    watch: parseWatchFilter(candidate.watch),
    tagsInclude,
    tagsExclude: stringList(candidate.tagsExclude).filter((tag) => !included.has(tag)),
    tagMode: candidate.tagMode === "or" ? "or" : "and",
    minChars: numericFilter(candidate.minChars),
    maxChars: numericFilter(candidate.maxChars),
  };
}

/** Reads back the conditions stored with a saved search, however old they are. */
function parseSavedParams(json: string): { tab: LibraryTab; filters: Filters; sortBy: LibrarySortBy } {
  try {
    const raw = JSON.parse(json) as { tab?: unknown; filters?: unknown; sortBy?: unknown };
    return {
      tab: parseLibraryTab(typeof raw.tab === "string" ? raw.tab : null),
      filters: normalizeFilters(raw.filters),
      sortBy: parseSortBy(raw.sortBy),
    };
  } catch {
    // A saved search whose conditions cannot be read still opens the library,
    // just unfiltered - better than an error where a list of works should be.
    return { tab: "works", filters: initialFilters, sortBy: "downloaded_at" };
  }
}

function SaveSearchForm({ defaultName, onConfirm }: { defaultName: string; onConfirm: (name: string) => void }) {
  const [name, setName] = useState(defaultName);
  const trimmed = name.trim();
  const submit = () => { if (trimmed) onConfirm(trimmed); };
  return (
    <Stack gap="sm">
      <TextInput
        label="名前"
        description="サイドバーの「保存した検索」に、この名前で並びます。"
        value={name}
        maxLength={80}
        data-autofocus
        onChange={(event) => setName(event.currentTarget.value)}
        onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); submit(); } }}
      />
      <Text size="xs" c="dimmed">同じ名前で保存すると、その検索を置き換えます。</Text>
      <Group justify="flex-end">
        <Button variant="default" onClick={() => modals.close(SAVE_SEARCH_MODAL)}>キャンセル</Button>
        <Button disabled={!trimmed} onClick={submit}>保存</Button>
      </Group>
    </Stack>
  );
}

const SAVE_SEARCH_MODAL = "save-search";

function openSaveSearchModal(defaultName: string, onConfirm: (name: string) => void) {
  modals.open({
    modalId: SAVE_SEARCH_MODAL,
    title: "この検索条件を保存",
    children: <SaveSearchForm defaultName={defaultName} onConfirm={(name) => { modals.close(SAVE_SEARCH_MODAL); onConfirm(name); }} />,
  });
}

const MANAGE_SEARCH_MODAL = "manage-saved-searches";

function openManageSavedSearches(saved: SavedSearchRecord[], onDelete: (id: number) => void) {
  modals.open({
    modalId: MANAGE_SEARCH_MODAL,
    title: "保存した検索を整理",
    children: (
      <Stack gap="xs">
        {saved.length === 0
          ? <Text size="sm" c="dimmed">保存した検索はまだありません。</Text>
          : saved.map((item) => (
            <Group key={item.id} justify="space-between" wrap="nowrap">
              <Box miw={0}>
                <Text size="sm" fw={600} className="line-clamp-1">{item.name}</Text>
                {item.query && <Text size="xs" c="dimmed" className="line-clamp-1">{item.query}</Text>}
              </Box>
              <Button
                size="compact-xs"
                variant="subtle"
                color="red"
                leftSection={<Icons.delete size={IconSize.inline} />}
                onClick={() => onDelete(item.id)}
              >
                削除
              </Button>
            </Group>
          ))}
        <Group justify="flex-end" mt="sm">
          <Button variant="default" onClick={() => modals.close(MANAGE_SEARCH_MODAL)}>閉じる</Button>
        </Group>
      </Stack>
    ),
  });
}

function uniqueWorks(pages: SearchV2Result[] | undefined) {
  const seen = new Set<number>();
  return pages?.flatMap((page) => page.items.filter((item) => {
    if (seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  })) ?? [];
}

function updateWorkFlag(
  data: InfiniteData<SearchV2Result, string | null> | undefined,
  id: number,
  patch: { favorite?: boolean; watchUpdates?: boolean },
  params: SearchV2Params,
) {
  if (!data) return data;
  const pageIndex = data.pages.findIndex((page) => page.items.some((item) => item.id === id));
  if (pageIndex < 0) return data;
  const itemIndex = data.pages[pageIndex].items.findIndex((item) => item.id === id);
  const next = { ...data.pages[pageIndex].items[itemIndex], ...patch };
  const removed = (params.favorite === true && !next.favorite)
    || (params.watchFilter === "watched" && !next.watchUpdates)
    || (params.watchFilter === "unwatched" && next.watchUpdates);
  const pages = [...data.pages];
  const items = [...pages[pageIndex].items];
  if (removed) items.splice(itemIndex, 1); else items[itemIndex] = next;
  pages[pageIndex] = { ...pages[pageIndex], items };
  if (removed && pages[0]) {
    const totalEstimate = pages[0].totalEstimate;
    pages[0] = {
      ...pages[0],
      totalEstimate: totalEstimate === null ? null : Math.max(0, totalEstimate - 1),
      searchMeta: {
        ...pages[0].searchMeta,
        totalEstimate: pages[0].searchMeta.totalEstimate === null ? null : Math.max(0, pages[0].searchMeta.totalEstimate - 1),
      },
    };
  }
  return { ...data, pages };
}

export default function LibraryPage() {
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  useSavedSearchMigration();
  const [urlParams, setUrlParams] = useAppSearchParams();
  // Query, tab and the favourite flag live in the URL so history navigation and
  // deep links such as /library?favorite=1 actually change what is shown.
  // Mirroring them into state as well would let the writer and the reader
  // overwrite each other on every change.
  const searchText = (urlParams.get("q") ?? "").slice(0, MAX_SEARCH_LENGTH);
  const tab = parseLibraryTab(urlParams.get("tab"));
  const favorite = urlParams.get("favorite") === "1";
  // Deep-linkable so the home tiles can open the library already filtered to
  // exactly what their number counts.
  const watch = parseWatchFilter(urlParams.get("watch"));
  const writeUrl = useCallback((patch: { q?: string; tab?: LibraryTab; favorite?: boolean; watch?: LibraryWatchFilter | null; sortBy?: LibrarySortBy; saved?: number | null }) => {
    const next = new URLSearchParams(urlParams);
    const q = patch.q ?? searchText;
    const nextTab = patch.tab ?? tab;
    const nextFavorite = patch.favorite ?? favorite;
    const nextWatch = patch.watch === undefined ? watch : patch.watch;
    const nextSort = patch.sortBy ?? resolveSortBy(urlParams.get("sort"), Boolean(q));
    // Changing anything means this is no longer the saved search it started
    // from, so the marker is dropped unless the caller is the one applying it.
    if (patch.saved) next.set("saved", String(patch.saved)); else next.delete("saved");
    if (q) next.set("q", q); else next.delete("q");
    if (nextTab !== "works") next.set("tab", nextTab); else next.delete("tab");
    if (nextFavorite) next.set("favorite", "1"); else next.delete("favorite");
    if (nextWatch) next.set("watch", nextWatch); else next.delete("watch");
    // The default depends on whether a query is active, so the parameter is
    // only written when the choice differs from the default for this state.
    if (nextSort !== resolveSortBy(null, Boolean(q))) next.set("sort", nextSort); else next.delete("sort");
    if (next.toString() === urlParams.toString()) return;
    setUrlParams(next, { replace: true });
  }, [searchText, favorite, setUrlParams, tab, urlParams, watch]);
  const setTab = useCallback((next: LibraryTab) => writeUrl({ tab: next }), [writeUrl]);

  // The input keeps its own value so typing stays responsive, and settles into
  // the URL once the user pauses. Only typing writes: deriving the write from a
  // debounced value let a stale keystroke overwrite a URL that had meanwhile
  // changed underneath it (back button, deep link), which flipped the two
  // writers into a loop.
  const [query, setQuery] = useState(searchText);
  const writeUrlRef = useRef(writeUrl);
  writeUrlRef.current = writeUrl;
  const searchTextRef = useRef(searchText);
  const queryWriteTimer = useRef<number | null>(null);
  const cancelPendingQueryWrite = () => {
    if (queryWriteTimer.current === null) return;
    window.clearTimeout(queryWriteTimer.current);
    queryWriteTimer.current = null;
  };
  useEffect(() => {
    searchTextRef.current = searchText;
    cancelPendingQueryWrite();
    setQuery((current) => (current === searchText ? current : searchText));
  }, [searchText]);
  useEffect(() => cancelPendingQueryWrite, []);
  const onQueryChange = useCallback((value: string) => {
    setQuery(value);
    cancelPendingQueryWrite();
    queryWriteTimer.current = window.setTimeout(() => {
      queryWriteTimer.current = null;
      if (value !== searchTextRef.current) writeUrlRef.current({ q: value });
    }, 260);
  }, []);

  const [pagingMode] = usePagingMode();
  const [pageSize] = usePageSize();
  const pageParam = Math.max(1, Number.parseInt(urlParams.get("page") ?? "1", 10) || 1);
  const setPage = useCallback((page: number) => {
    const next = new URLSearchParams(urlParams);
    if (page > 1) next.set("page", String(page)); else next.delete("page");
    setUrlParams(next, { replace: false });
  }, [setUrlParams, urlParams]);
  const [otherFilters, setOtherFilters] = useState<Filters>(initialFilters);
  const urlSortBy = resolveSortBy(urlParams.get("sort"), Boolean(searchText));
  const [sortBy, setSortByState] = useState<LibrarySortBy>(urlSortBy);
  useEffect(() => setSortByState((current) => current === urlSortBy ? current : urlSortBy), [urlSortBy]);
  const setSortBy = useCallback((next: LibrarySortBy) => {
    setSortByState(next);
    writeUrl({ sortBy: next });
  }, [writeUrl]);
  const [storedView, setStoredView] = useLocalStorage<unknown>({ key: "piep.library-view", defaultValue: "gallery" });
  const view = parseViewMode(storedView);
  const setView = useCallback((next: ViewMode) => setStoredView(next), [setStoredView]);
  // Saved searches are part of the library now, not per-install browser state,
  // so the sidebar can list them and they survive a reinstall.
  const savedSearchesQuery = useQuery({
    queryKey: ["saved-searches"],
    queryFn: () => runtime ? listSavedSearches() : Promise.resolve([] as SavedSearchRecord[]),
    staleTime: 60_000,
  });
  const savedSearches = savedSearchesQuery.data ?? [];
  const [filterOpened, filterDrawer] = useDisclosure(false);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  const { addToEpubQueue } = useWorkspace();

  // Deep-linkable flags are owned outright by the URL. Falling back to a stale
  // state copy when the URL flag is removed makes Back navigation unable to
  // clear the filter.
  const filters = useMemo<Filters>(() => ({ ...otherFilters, favorite, watch }), [favorite, otherFilters, watch]);
  const setFilters = useCallback((next: Filters) => {
    const normalized = normalizeFilters(next);
    setOtherFilters(normalized);
    if (normalized.favorite !== favorite || (normalized.watch ?? null) !== watch) writeUrl({ favorite: normalized.favorite, watch: normalized.watch ?? null });
  }, [favorite, watch, writeUrl]);
  useEffect(() => { setSelected([]); }, [searchText, filters, sortBy, tab]);

  // The "読みかけ" shelf is a membership list rather than a filter: reading
  // positions are kept per device, so the library is told which works to show.
  // Read once per visit to the shelf; it must not change under an open page.
  const shelfParam = urlParams.get("shelf");
  const readingIds = useMemo(() => shelfParam === "reading" ? readingWorkIds() : null, [shelfParam]);

  // Relevance is walked with a score cursor and has no nth page, so numbers
  // are only offered once an ordering has been chosen.
  const numberedPages = pagingMode === "pages" && sortBy !== "relevance";
  const params = useMemo<SearchV2Params>(() => ({
    idsInclude: readingIds,
    text: searchText || null,
    // Selecting both providers is the same as no provider filter; the backend
    // takes a single source, and post-filtering a page would break paging.
    source: filters.sources.length === 1 ? filters.sources[0] : null,
    contentType: filters.contentType,
    favorite: filters.favorite || null,
    tagsInclude: filters.tagsInclude.length ? filters.tagsInclude : null,
    tagsExclude: filters.tagsExclude.length ? filters.tagsExclude : null,
    tagFilterMode: filters.tagMode,
    minCharCount: numericFilterOrNull(filters.minChars),
    maxCharCount: numericFilterOrNull(filters.maxChars),
    watchFilter: filters.watch,
    sortBy,
    sortOrder: sortOrderFor(sortBy),
    limit: pageSize,
    // Numbered pages fetch one page outright; scrolling walks with a cursor.
    offset: numberedPages ? (pageParam - 1) * pageSize : null,
    projection: view === "compact" ? "libraryCompact" : "libraryGallery",
  }), [readingIds, searchText, filters, sortBy, view, numberedPages, pageParam, pageSize]);

  // Keyset pagination: fetching a fixed slab and paging it in the browser
  // capped the library at that slab and reported the wrong total.
  const works = useInfiniteQuery({
    queryKey: ["library", params],
    queryFn: ({ pageParam }) => runtime
      ? searchDownloadsV2({ ...params, cursor: pageParam })
      : Promise.resolve(searchDemoWorks(searchText, filters.sources.length === 1 ? filters.sources[0] : null)),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage, allPages) => {
      const cursor = lastPage.items.length ? lastPage.nextCursor : null;
      return cursor && !allPages.slice(0, -1).some((page) => page.nextCursor === cursor) ? cursor : undefined;
    },
    enabled: tab === "works",
  });
  const facets = useQuery({
    queryKey: ["library-facets"],
    queryFn: () => runtime ? getFilterFacets() : Promise.resolve(demoFacets),
    enabled: filterOpened,
  });
  const loadedItems = useMemo(() => uniqueWorks(works.data?.pages), [works.data?.pages]);
  const totalCount = works.data?.pages[0]?.totalEstimate ?? null;
  const searchMeta = works.data?.pages[0]?.searchMeta;
  const activeFilterCount = filters.sources.length + Number(Boolean(filters.contentType)) + Number(filters.favorite) + Number(Boolean(filters.watch)) + filters.tagsInclude.length + filters.tagsExclude.length + Number(filters.minChars !== "") + Number(filters.maxChars !== "");
  const loadedIds = useMemo(() => loadedItems.map((item) => item.id), [loadedItems]);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const allLoadedSelected = loadedIds.length > 0 && loadedIds.every((id) => selectedSet.has(id));

  const flagMutation = useMutation({
    mutationFn: ({ id, kind, value }: { id: number; kind: "favorite" | "watch"; value: boolean }) => {
      if (!runtime) return Promise.resolve();
      return kind === "favorite" ? setFavorite(id, value) : setWatchUpdates(id, value);
    },
    onMutate: (input) => {
      const key = ["library", params] as const;
      const previous = queryClient.getQueryData<InfiniteData<SearchV2Result, string | null>>(key);
      queryClient.setQueryData(key, updateWorkFlag(previous, input.id, input.kind === "favorite" ? { favorite: input.value } : { watchUpdates: input.value }, params));
      return { key, previous };
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["dashboard"] });
      // The sidebar shelf counts are on screen the whole time; a favourite that
      // does not change them reads as the click not having registered.
      queryClient.invalidateQueries({ queryKey: ["library-shelf-counts"] });
    },
    onError: (error, _input, context) => {
      if (context?.previous) queryClient.setQueryData(context.key, context.previous);
      notifications.show({ color: "red", title: "作品情報を更新できません", message: errorMessage(error) });
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["library"], refetchType: "none" }),
  });
  const deleteMutation = useMutation({
    mutationFn: (ids: number[]) => runtime ? deleteDownloads(ids) : Promise.resolve({ matchedCount: ids.length, changedCount: ids.length }),
    onSuccess: (result, ids) => {
      notifications.show({ color: "green", title: "ライブラリから削除しました", message: `${formatNumber(result.changedCount)}件を削除しました` });
      // Reading positions outlive the works they point at, and a "読みかけ"
      // shelf counting deleted works would never go back down.
      ids.forEach(forgetReadingPositions);
      setSelected([]); setSelectionMode(false);
      queryClient.resetQueries({ queryKey: ["library", params], exact: true });
      queryClient.invalidateQueries({ queryKey: ["library"], refetchType: "none" });
      queryClient.invalidateQueries({ queryKey: ["library-facets"] });
      queryClient.invalidateQueries({ queryKey: ["library-entities"] });
      queryClient.invalidateQueries({ queryKey: ["library-shelf-counts"] });
      queryClient.invalidateQueries({ queryKey: ["dashboard"] });
    },
    onError: (error) => notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }),
  });
  const selectionMutation = useMutation({
    // One bulk command instead of one call per selected work: a large
    // selection previously fired thousands of concurrent invokes.
    mutationFn: async ({ action }: { action: "favorite" | "watch" }) => {
      if (!runtime) return selected.length;
      const result = await setFlagsForIds(selected, action === "favorite" ? { favorite: true } : { watch: true });
      return result.matchedCount;
    },
    onSuccess: (count, input) => {
      notifications.show({ color: "green", message: `${formatNumber(count)}件を${input.action === "favorite" ? "お気に入り" : "更新監視"}に追加しました` });
      queryClient.resetQueries({ queryKey: ["library", params], exact: true });
      queryClient.invalidateQueries({ queryKey: ["library"], refetchType: "none" });
      queryClient.invalidateQueries({ queryKey: ["library-shelf-counts"] });
      queryClient.invalidateQueries({ queryKey: ["dashboard"] });
    },
    onError: (error) => notifications.show({ color: "red", title: "一括操作に失敗しました", message: errorMessage(error) }),
  });

  const suggestedName = () => query.trim()
    || [filters.sources.join("・"), filters.tagsInclude.map((tag) => `#${tag}`).join(" ")].filter(Boolean).join(" / ")
    || "すべての作品";

  const saveMutation = useMutation({
    mutationFn: (name: string) => upsertSavedSearch({
      name,
      query: query.slice(0, MAX_SEARCH_LENGTH) || null,
      paramsJson: JSON.stringify({ tab, filters: normalizeFilters(filters), sortBy }),
    }),
    onSuccess: (record) => {
      notifications.show({ color: "green", title: "検索を保存しました", message: `サイドバーの「保存した検索」から開けます · ${record.name}` });
      queryClient.invalidateQueries({ queryKey: ["saved-searches"] });
      writeUrl({ saved: record.id });
    },
    onError: (error) => notifications.show({ color: "red", title: "検索を保存できません", message: errorMessage(error) }),
  });
  const deleteSavedMutation = useMutation({
    mutationFn: (id: number) => deleteSavedSearch(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["saved-searches"] });
      notifications.show({ color: "gray", message: "保存した検索を削除しました" });
    },
    onError: (error) => notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }),
  });

  const saveCurrentSearch = () => {
    if (!runtime) {
      notifications.show({ color: "blue", message: "検索の保存はデスクトップアプリで利用できます" });
      return;
    }
    openSaveSearchModal(suggestedName(), (name) => saveMutation.mutate(name));
  };

  const applySavedSearch = useCallback((record: SavedSearchRecord) => {
    const parsed = parseSavedParams(record.paramsJson);
    setOtherFilters(parsed.filters);
    setSortByState(parsed.sortBy);
    const nextQuery = record.query ?? "";
    cancelPendingQueryWrite();
    setQuery(nextQuery);
    writeUrl({
      q: nextQuery,
      tab: parsed.tab,
      favorite: parsed.filters.favorite,
      watch: parsed.filters.watch,
      sortBy: parsed.sortBy,
      saved: record.id,
    });
  }, [writeUrl]);

  // A saved search reached by URL - from the sidebar, a deep link or the back
  // button - is expanded into the visible conditions once, so what is on screen
  // always matches what the controls say.
  const savedIdParam = Number.parseInt(urlParams.get("saved") ?? "", 10);
  const appliedSavedRef = useRef<number | null>(null);
  useEffect(() => {
    if (!Number.isSafeInteger(savedIdParam)) {
      appliedSavedRef.current = null;
      return;
    }
    if (appliedSavedRef.current === savedIdParam) return;
    if (!savedSearchesQuery.isSuccess) return;
    const record = savedSearches.find((item) => item.id === savedIdParam);
    appliedSavedRef.current = savedIdParam;
    // Deleted elsewhere: drop the marker rather than highlighting a search that
    // no longer exists, and leave the results the reader is looking at alone.
    if (!record) writeUrl({ saved: null });
    else applySavedSearch(record);
  }, [applySavedSearch, savedIdParam, savedSearches, savedSearchesQuery.isSuccess, writeUrl]);

  // Stable across renders so the memoised cards are not all invalidated by a
  // fresh closure every time the list re-renders.
  const toggleSelected = useCallback((id: number, checked: boolean) => setSelected((current) => {
    if (checked) return current.includes(id) ? current : [...current, id];
    return current.filter((item) => item !== id);
  }), []);
  const mutateFlag = flagMutation.mutate;
  const toggleFavorite = useCallback((id: number, favorite: boolean) => mutateFlag({ id, kind: "favorite", value: favorite }), [mutateFlag]);
  const toggleWatch = useCallback((id: number, watch: boolean) => mutateFlag({ id, kind: "watch", value: watch }), [mutateFlag]);
  const toggleVisibleSelection = () => setSelected((current) => {
    if (allLoadedSelected) {
      const loaded = new Set(loadedIds);
      return current.filter((id) => !loaded.has(id));
    }
    return [...new Set([...current, ...loadedIds])];
  });
  const leaveSelection = () => { setSelectionMode(false); setSelected([]); };
  const confirmDelete = () => modals.openConfirmModal({
    title: "選択した作品を削除しますか？",
    children: <Text size="sm">{selected.length}件の作品とローカルアセットを削除します。この操作は元に戻せません。</Text>,
    confirmProps: { color: "red" }, labels: { confirm: "削除する", cancel: "キャンセル" },
    onConfirm: () => deleteMutation.mutate(selected),
  });

  // Authors and series are searched and paged in SQLite. Filtering the
  // dashboard facets in the browser only ever saw their top 60 rows, so most
  // of a large library was unreachable from these tabs.
  const entityKind = tab === "people" ? "person" : "series";
  const entities = useInfiniteQuery({
    queryKey: ["library-entities", entityKind, searchText, pageSize],
    queryFn: ({ pageParam }) => runtime
      ? searchEntityFacets(entityKind, searchText || null, pageSize, pageParam)
      : Promise.resolve((entityKind === "person" ? demoFacets.authorEntities : demoFacets.series)
        .filter((entity) => !searchText || `${entity.displayName} ${entity.description ?? ""}`.toLocaleLowerCase("ja-JP").includes(searchText.toLocaleLowerCase("ja-JP")))),
    initialPageParam: 0,
    getNextPageParam: (lastPage, allPages) => lastPage.length < pageSize ? undefined : allPages.reduce((count, page) => count + page.length, 0),
    enabled: tab !== "works",
  });
  const entityItems = useMemo(() => {
    const seen = new Set<string>();
    return entities.data?.pages.flatMap((page) => page.filter((entity) => {
      const identity = `${entity.source}\0${entity.sourceKey}`;
      if (seen.has(identity)) return false;
      seen.add(identity);
      return true;
    })) ?? [];
  }, [entities.data?.pages]);

  return (
    <div className="page page--contained library-page">
      <Paper p="md" className="library-toolbar" withBorder>
        <Stack gap="sm">
          <Group wrap="nowrap" align="flex-start">
            <Box className="library-toolbar__search"><LibrarySearch value={query} onChange={onQueryChange} runtime={runtime} /></Box>
            <Tooltip label="詳細フィルター">
              <Indicator label={activeFilterCount} size={16} disabled={!activeFilterCount}>
                <Button variant="default" leftSection={<Icons.filter size={IconSize.action} />} onClick={filterDrawer.open}>絞り込み</Button>
              </Indicator>
            </Tooltip>
            <Select
              value={sortBy}
              onChange={(value) => setSortBy(parseSortBy(value))}
              data={searchText ? SEARCH_SORT_OPTIONS : SORT_OPTIONS}
              leftSection={<Icons.sort size={IconSize.menu} />}
              className="library-toolbar__sort"
              aria-label="並び順"
            />
            <SegmentedControl
              value={view}
              onChange={(value) => setView(parseViewMode(value))}
              data={[{ value: "gallery", label: <Tooltip label="ギャラリー"><Icons.viewGrid size={IconSize.menu} aria-label="ギャラリー表示" /></Tooltip> }, { value: "compact", label: <Tooltip label="リスト"><Icons.viewList size={IconSize.menu} aria-label="リスト表示" /></Tooltip> }]}
              aria-label="表示形式"
            />
            <Menu position="bottom-end" width={280} withinPortal>
              <Menu.Target><Tooltip label="保存した検索"><ActionIcon variant="default" size={36} aria-label="保存した検索"><Icons.saveSearch size={IconSize.action} /></ActionIcon></Tooltip></Menu.Target>
              <Menu.Dropdown>
                <Menu.Item leftSection={<Icons.saveSearch size={IconSize.menu} />} onClick={saveCurrentSearch}>現在の検索条件を保存</Menu.Item>
                {savedSearches.length > 0 && <>
                  <Menu.Divider />
                  <Menu.Label>保存した検索</Menu.Label>
                  {savedSearches.map((saved) => (
                    <Menu.Item key={saved.id} leftSection={<Icons.search size={IconSize.menu} />} onClick={() => applySavedSearch(saved)}>
                      <Text size="sm" className="line-clamp-1">{saved.name}</Text>
                    </Menu.Item>
                  ))}
                  <Menu.Divider />
                  <Menu.Item leftSection={<Icons.delete size={IconSize.menu} />} onClick={() => openManageSavedSearches(savedSearches, (id) => deleteSavedMutation.mutate(id))}>
                    保存した検索を整理…
                  </Menu.Item>
                </>}
              </Menu.Dropdown>
            </Menu>
          </Group>
          {activeFilterCount > 0 && (
            <Group gap="xs">
              <Text size="xs" c="dimmed" fw={600}>適用中</Text>
              {filters.sources.map((source) => <FilterChip key={source} label={source} onRemove={() => setFilters({ ...filters, sources: filters.sources.filter((item) => item !== source) })} />)}
              {filters.tagsInclude.map((tag) => <FilterChip key={`+${tag}`} label={`#${tag}`} onRemove={() => setFilters({ ...filters, tagsInclude: filters.tagsInclude.filter((item) => item !== tag) })} />)}
              {filters.tagsExclude.map((tag) => <FilterChip key={`-${tag}`} label={`除外 #${tag}`} color="red" onRemove={() => setFilters({ ...filters, tagsExclude: filters.tagsExclude.filter((item) => item !== tag) })} />)}
              <Button size="compact-xs" variant="subtle" color="gray" onClick={() => setFilters(initialFilters)}>すべて解除</Button>
            </Group>
          )}
        </Stack>
      </Paper>

      <Tabs value={tab} onChange={(value) => setTab((value as LibraryTab) ?? "works")} mt="lg">
        <Tabs.List>
          <Tabs.Tab value="works" leftSection={<Icons.epubAdd size={IconSize.menu} />}>作品</Tabs.Tab>
          <Tabs.Tab value="people" leftSection={<Icons.people size={IconSize.menu} />}>作者・クリエイター</Tabs.Tab>
          <Tabs.Tab value="series" leftSection={<Icons.series size={IconSize.menu} />}>シリーズ</Tabs.Tab>
        </Tabs.List>
      </Tabs>

      <Group justify="space-between" my="md" gap="xs" wrap="nowrap">
        <Group gap={8} wrap="nowrap" miw={0}>
          <Text size="sm" c="dimmed" style={{ whiteSpace: "nowrap" }}>{tab === "works"
            ? `${formatNumber(totalCount ?? loadedItems.length)}件${totalCount !== null && loadedItems.length < totalCount ? `（${formatNumber(loadedItems.length)}件を表示中）` : ""}`
            : `${formatNumber(entityItems.length)}件${entities.hasNextPage ? "以上" : ""}`}</Text>
          {tab === "works" && searchText && searchMeta?.explanations?.length
            ? <SearchInterpretation meta={searchMeta} />
            : null}
          {/* Beside the count, which is the thing it changes. */}
          <PagingModeToggle />
        </Group>
        {tab === "works" && !selectionMode && <Button size="xs" variant="subtle" color="gray" leftSection={<Icons.confirm size={IconSize.menu} />} onClick={() => setSelectionMode(true)}>複数選択</Button>}
      </Group>

      {tab === "works" ? (
        works.isLoading ? <LoadingState label="ライブラリを検索しています" /> : works.error ? <ErrorState error={works.error} retry={() => works.refetch()} /> : loadedItems.length ? (
          <>
            <VirtualizedWorkList items={loadedItems} view={view} selectionMode={selectionMode} selected={selectedSet} onSelect={toggleSelected} onToggleFavorite={toggleFavorite} onToggleWatch={toggleWatch} />
            <ListPager
              hasNext={works.hasNextPage}
              loading={works.isFetchingNextPage || works.isFetching}
              loaded={loadedItems.length}
              total={totalCount}
              onLoad={() => works.fetchNextPage()}
              pages={{
                current: pageParam,
                size: pageSize,
                onGoTo: setPage,
                unavailableReason: sortBy === "relevance"
                  ? "関連度順はページ番号で移動できません。並び順を選ぶとページ番号が使えます。"
                  : null,
              }}
            />
          </>
        ) : <EmptyState icon={Icons.search} title="一致する作品がありません" description="検索語やフィルターを減らすか、新しい作品を保存してください。" action={<Button variant="light" onClick={() => { onQueryChange(""); setOtherFilters(initialFilters); writeUrl({ q: "", favorite: false, watch: null }); }}>検索をリセット</Button>} />
      ) : entities.isLoading ? <LoadingState /> : entities.error ? <ErrorState error={entities.error} retry={() => entities.refetch()} /> : entityItems.length ? (
        <>
          <SimpleGrid cols={{ base: 1, sm: 2, xl: 3 }}>{entityItems.map((entity) => <div key={`${entity.source}:${entity.sourceKey}`} style={{ contentVisibility: "auto", containIntrinsicSize: "auto 138px" }}><EntityCard entity={entity} kind={entityKind} /></div>)}</SimpleGrid>
          <ListPager hasNext={entities.hasNextPage} loading={entities.isFetchingNextPage} loaded={entityItems.length} total={null} onLoad={() => entities.fetchNextPage()} />
        </>
      ) : <EmptyState icon={Icons.people} title="一致する項目がありません" description="名前を変えて検索してください。" />}

      <Drawer opened={filterOpened} onClose={filterDrawer.close} title="詳細フィルター" position="right" size={420} scrollAreaComponent={ScrollArea.Autosize}>
        <FilterForm value={filters} runtime={runtime} tags={facets.data?.tags ?? []} contentTypes={facets.data?.contentTypes.map((item) => ({ value: item.name, label: `${item.name} (${item.count})` })) ?? []} onApply={(next) => { setFilters(next); filterDrawer.close(); }} />
      </Drawer>

      {/* Raised while the selection bar is up so the two never stack. */}
      <ScrollToTop offsetBottom={selectionMode ? 104 : undefined} />

      {selectionMode && (
        <Paper className="selection-bar" shadow="xl" withBorder p="sm" role="region" aria-label="複数選択の操作">
          <div className="selection-bar__inner">
            <Group gap="sm" wrap="nowrap" className="selection-bar__summary">
              <div className="selection-bar__count" data-empty={!selected.length || undefined}>
                <Icons.confirm size={IconSize.action} strokeWidth={2.6} aria-hidden />
                <Text fw={750}>{selected.length}件</Text>
                <Text size="xs" c="dimmed">選択中</Text>
              </div>
              <Button variant="subtle" size="compact-sm" onClick={toggleVisibleSelection}>{allLoadedSelected ? "表示中を解除" : "表示中をすべて選択"}</Button>
              {selected.length > 0 && <Button variant="subtle" color="gray" size="compact-sm" onClick={() => setSelected([])}>すべて解除</Button>}
            </Group>
            <Group gap="xs" wrap="nowrap" className="selection-bar__actions">
              <Button size="sm" variant="light" leftSection={<Icons.epubAdd size={IconSize.menu} />} disabled={!selected.length} onClick={() => { addToEpubQueue(selected); notifications.show({ color: "green", message: `${selected.length}件をEPUBキューに追加しました` }); }}>EPUB</Button>
              <Menu position="top-end"><Menu.Target><Button size="sm" variant="default" rightSection={<Icons.more size={IconSize.menu} />} disabled={!selected.length}>その他</Button></Menu.Target><Menu.Dropdown><Menu.Item leftSection={<Icons.favorite size={IconSize.menu} />} onClick={() => selectionMutation.mutate({ action: "favorite" })}>お気に入りに追加</Menu.Item><Menu.Item leftSection={<Icons.watch size={IconSize.menu} />} onClick={() => selectionMutation.mutate({ action: "watch" })}>更新監視を有効化</Menu.Item><Menu.Divider /><Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} onClick={confirmDelete}>削除</Menu.Item></Menu.Dropdown></Menu>
              <Divider orientation="vertical" h={26} mx={2} />
              <Tooltip label="複数選択を終了"><ActionIcon variant="subtle" color="gray" aria-label="複数選択を終了" onClick={leaveSelection}><Icons.cancel size={IconSize.nav} /></ActionIcon></Tooltip>
            </Group>
          </div>
        </Paper>
      )}
    </div>
  );
}

/**
 * How the search was read, on one line.
 *
 * This used to be a full-width alert above the results, which pushed the works
 * themselves below the fold on every search. The first line is the part worth
 * reading at a glance; the rest is a click away for when the results are not
 * what was expected.
 */
function SearchInterpretation({ meta }: { meta: SearchV2Result["searchMeta"] }) {
  const explanations = meta.explanations ?? [];
  const [first, ...rest] = explanations;
  const strict = meta.exactEntity?.strict;
  const summary = (
    <Badge
      size="sm"
      variant="light"
      color={strict ? "teal" : "gray"}
      leftSection={<Icons.search size={IconSize.inline} />}
      className="search-interpretation"
      style={{ cursor: rest.length ? "pointer" : "default", textTransform: "none" }}
    >
      {first}
      {rest.length > 0 ? ` +${rest.length}` : ""}
    </Badge>
  );
  if (rest.length === 0) return <Tooltip label={first} multiline w={280}>{summary}</Tooltip>;
  return (
    <Popover width={340} position="bottom-start" withArrow shadow="md">
      <Popover.Target>
        <UnstyledButton aria-label={`この検索の解釈、全${explanations.length}件`}>{summary}</UnstyledButton>
      </Popover.Target>
      <Popover.Dropdown>
        <Text size="xs" fw={700} c="dimmed" mb={6}>この検索の解釈</Text>
        <Stack gap={4}>
          {explanations.map((explanation) => <Text size="sm" key={explanation}>{explanation}</Text>)}
        </Stack>
      </Popover.Dropdown>
    </Popover>
  );
}

function FilterChip({ label, onRemove, color = "blue" }: { label: string; onRemove: () => void; color?: string }) {
  return <Badge variant="light" color={color} rightSection={<ActionIcon size="xs" variant="transparent" color={color} aria-label={`${label}を解除`} onClick={onRemove}><Icons.cancel size={IconSize.inline} /></ActionIcon>}>{label}</Badge>;
}

export type SearchSuggestionAction =
  | { kind: "navigate"; target: string }
  | { kind: "query"; query: string };

function quotedFilterValue(value: string) {
  // The backend query grammar intentionally has no escape syntax. Removing a
  // literal quote is safer than producing a token that swallows the remainder.
  return `"${value.split('"').join("").trim()}"`;
}

export function searchSuggestionAction(item: SearchSuggestion): SearchSuggestionAction {
  const source = item.source?.trim();
  const sourceKey = item.sourceKey?.trim();
  if (item.kind === "author" && source && sourceKey) {
    return { kind: "navigate", target: `/people/${encodeURIComponent(source)}/${encodeURIComponent(sourceKey)}` };
  }
  if (item.kind === "series" && source && sourceKey) {
    return { kind: "navigate", target: `/series/${encodeURIComponent(source)}/${encodeURIComponent(sourceKey)}` };
  }
  if (item.kind === "title" && sourceKey && /^\d+$/.test(sourceKey)) {
    return { kind: "navigate", target: `/works/${sourceKey}` };
  }
  if (item.kind === "author") return { kind: "query", query: `author:${quotedFilterValue(item.label)}` };
  if (item.kind === "series") return { kind: "query", query: `series:${quotedFilterValue(item.label)}` };
  if (item.kind === "tag") return { kind: "query", query: `tag:${quotedFilterValue(item.label)}` };
  return { kind: "query", query: quotedFilterValue(item.label) };
}

const SUGGESTION_KIND_LABEL: Record<string, string> = {
  author: "作者へ移動",
  series: "シリーズへ移動",
  title: "作品へ移動",
  tag: "タグで絞る",
};

function LibrarySearch({ value, onChange, runtime }: { value: string; onChange: (value: string) => void; runtime: boolean }) {
  const navigate = useAppNavigate();
  const [composing, setComposing] = useState(false);
  const [debounced] = useDebouncedValue(value, 180);
  const suggestionText = debounced.trim();
  const combobox = useCombobox({ onDropdownClose: () => combobox.resetSelectedOption(), onDropdownOpen: () => combobox.selectFirstOption() });
  const suggestions = useQuery({
    queryKey: ["search-suggest", suggestionText],
    queryFn: async (): Promise<SearchSuggestion[]> => {
      if (runtime) return (await searchSuggest({ text: suggestionText, limit: 8 })).items;
      const demoSuggestions: SearchSuggestion[] = [
        { kind: "tag", label: "創作", value: "創作", count: 2 },
        { kind: "tag", label: "ファンタジー", value: "ファンタジー", count: 1 },
        { kind: "author", label: "青葉しおり", value: "青葉しおり", count: 1, exactMatch: suggestionText === "青葉しおり", source: "pixiv", sourceKey: "aoba-shiori" },
        { kind: "series", label: "季節の栞", value: "pixiv:12552619", count: 1, exactMatch: suggestionText === "季節の栞", source: "pixiv", sourceKey: "12552619" },
      ];
      return demoSuggestions.filter((item) => item.label.includes(suggestionText));
    },
    enabled: !composing && Boolean(suggestionText),
    staleTime: 5 * 60_000,
    gcTime: 60_000,
  });
  const items = suggestions.data ?? [];
  return (
    <Combobox store={combobox} onOptionSubmit={(selected) => {
      const index = Number(selected.slice("suggestion:".length));
      const item = Number.isSafeInteger(index) ? items[index] : undefined;
      if (!item) return;
      const action = searchSuggestionAction(item);
      if (action.kind === "navigate") navigate(action.target);
      else onChange(action.query);
      combobox.closeDropdown();
    }} withinPortal>
      <Combobox.Target>
        <InputBase
          value={value}
          onChange={(event) => {
            onChange(event.currentTarget.value);
            if (!composing) { combobox.openDropdown(); combobox.updateSelectedOptionIndex(); }
          }}
          onCompositionStart={() => setComposing(true)}
          onCompositionEnd={(event) => { setComposing(false); onChange(event.currentTarget.value); combobox.openDropdown(); }}
          onFocus={() => combobox.openDropdown()}
          onClick={() => combobox.openDropdown()}
          onBlur={() => combobox.closeDropdown()}
          leftSection={<Icons.search size={IconSize.action} />}
          rightSection={value ? <Combobox.ClearButton onClear={() => onChange("")} /> : null}
          rightSectionPointerEvents={value ? "all" : "none"}
          placeholder="タイトル、作者、タグ、本文を検索"
          aria-label="ライブラリを検索"
          maxLength={MAX_SEARCH_LENGTH}
          size="md"
        />
      </Combobox.Target>
      <Combobox.Dropdown hidden={!value.trim() || composing}>
        <Combobox.Options>
          {suggestions.isLoading ? <Combobox.Empty>候補を検索中…</Combobox.Empty> : items.length ? items.map((item, index) => <Combobox.Option value={`suggestion:${index}`} key={`${item.kind}:${item.source ?? ""}:${item.sourceKey ?? item.value}`}><Group justify="space-between" wrap="nowrap"><Text size="sm" lineClamp={1}>{item.label}</Text><Group gap={5} wrap="nowrap">{item.exactMatch && <Badge size="xs" variant="filled" color="teal">完全一致</Badge>}<Badge size="xs" variant="light" color={item.kind === "tag" ? "teal" : "blue"}>{SUGGESTION_KIND_LABEL[item.kind] ?? item.kind}</Badge>{typeof item.count === "number" && <Text size="xs" c="dimmed">{formatNumber(item.count)}件</Text>}</Group></Group></Combobox.Option>) : <Combobox.Empty>Enterで全文検索</Combobox.Empty>}
        </Combobox.Options>
        <Combobox.Footer><Text size="xs" c="dimmed">例: tag:創作 -成人向け &quot;完全一致&quot;</Text></Combobox.Footer>
      </Combobox.Dropdown>
    </Combobox>
  );
}

function FilterForm({ value, runtime, tags, contentTypes, onApply }: {
  value: Filters;
  runtime: boolean;
  tags: FacetCount[];
  contentTypes: { value: string; label: string }[];
  onApply: (value: Filters) => void;
}) {
  const [draft, setDraft] = useState(() => normalizeFilters(value));
  useEffect(() => setDraft(normalizeFilters(value)), [value]);
  const min = numericFilterOrNull(draft.minChars);
  const max = numericFilterOrNull(draft.maxChars);
  const invalidRange = min !== null && max !== null && min > max;
  return (
    <Stack gap="lg">
      <Box><Text fw={700} size="sm" mb="xs">ソース</Text><Checkbox.Group value={draft.sources} onChange={(sources) => setDraft({ ...draft, sources })}><Group><Checkbox value="pixiv" label="pixiv" /><Checkbox value="fanbox" label="FANBOX" /></Group></Checkbox.Group></Box>
      <Select label="コンテンツ種別" placeholder="すべて" clearable data={contentTypes} value={draft.contentType} onChange={(contentType) => setDraft({ ...draft, contentType })} />
      <Divider />
      <TagFilterCombobox runtime={runtime} label="含めるタグ" tags={tags} value={draft.tagsInclude} onChange={(tagsInclude) => setDraft({ ...draft, tagsInclude, tagsExclude: draft.tagsExclude.filter((tag) => !tagsInclude.includes(tag)) })} />
      <TagFilterCombobox runtime={runtime} label="除外するタグ" tags={tags} value={draft.tagsExclude} onChange={(tagsExclude) => setDraft({ ...draft, tagsExclude, tagsInclude: draft.tagsInclude.filter((tag) => !tagsExclude.includes(tag)) })} />
      <Radio.Group label="複数タグの条件" value={draft.tagMode} onChange={(tagMode) => setDraft({ ...draft, tagMode: tagMode === "or" ? "or" : "and" })}><Group mt="xs"><Radio value="and" label="すべて含む" /><Radio value="or" label="いずれかを含む" /></Group></Radio.Group>
      <Divider />
      <SimpleGrid cols={2}><NumberInput label="最小文字数" min={0} max={Number.MAX_SAFE_INTEGER} allowDecimal={false} allowNegative={false} thousandSeparator="," value={draft.minChars} onChange={(minChars) => setDraft({ ...draft, minChars })} /><NumberInput label="最大文字数" min={0} max={Number.MAX_SAFE_INTEGER} allowDecimal={false} allowNegative={false} thousandSeparator="," value={draft.maxChars} onChange={(maxChars) => setDraft({ ...draft, maxChars })} /></SimpleGrid>
      {invalidRange && <Alert color="red">最大文字数は最小文字数以上にしてください。</Alert>}
      <Select label="更新監視" clearable placeholder="すべて" data={[{ value: "watched", label: "監視中" }, { value: "unwatched", label: "未監視" }]} value={draft.watch} onChange={(watch) => setDraft({ ...draft, watch: parseWatchFilter(watch) })} />
      <Checkbox label="お気に入りのみ" checked={draft.favorite} onChange={(event) => setDraft({ ...draft, favorite: event.currentTarget.checked })} />
      <Alert color="gray" variant="light">フィルターは作品タブに適用されます。作者・シリーズは上部の検索語で絞り込めます。</Alert>
      <Group grow><Button variant="default" onClick={() => setDraft(initialFilters)}>リセット</Button><Button disabled={invalidRange} onClick={() => onApply(normalizeFilters(draft))}>適用</Button></Group>
    </Stack>
  );
}

function TagFilterCombobox({ label, runtime, tags, value, onChange }: { label: string; runtime: boolean; tags: FacetCount[]; value: string[]; onChange: (value: string[]) => void }) {
  const [search, setSearch] = useState("");
  const [debouncedSearch] = useDebouncedValue(search, 180);
  const combobox = useCombobox({ onDropdownClose: () => { combobox.resetSelectedOption(); setSearch(""); } });
  const normalized = debouncedSearch.trim().toLocaleLowerCase("ja-JP");
  const remoteMatches = useQuery({
    queryKey: ["filter-facet-search", "tags", normalized],
    queryFn: () => searchFilterFacets("tags", normalized, 80),
    enabled: runtime && Boolean(normalized),
    staleTime: 5 * 60_000,
    gcTime: 60_000,
  });
  const localMatches = useMemo(() => tags
    .filter((tag) => !normalized || tag.name.toLocaleLowerCase("ja-JP").includes(normalized))
    .slice(0, 80), [normalized, tags]);
  const visible = runtime && normalized && remoteMatches.data ? remoteMatches.data : localMatches;
  const toggle = (name: string) => onChange(value.includes(name) ? value.filter((item) => item !== name) : [...value, name]);
  return <Combobox store={combobox} onOptionSubmit={toggle} withinPortal>
    <Combobox.DropdownTarget>
      <PillsInput label={label} onClick={() => combobox.openDropdown()} rightSection={<Combobox.Chevron />}>
        <Pill.Group>
          {value.map((tag) => <Pill key={tag} withRemoveButton onRemove={() => onChange(value.filter((item) => item !== tag))}>{tag}</Pill>)}
          <Combobox.EventsTarget>
            <PillsInput.Field value={search} maxLength={120} onChange={(event) => { setSearch(event.currentTarget.value); combobox.openDropdown(); combobox.updateSelectedOptionIndex(); }} onFocus={() => combobox.openDropdown()} onKeyDown={(event) => { if (event.key === "Backspace" && !search && value.length) onChange(value.slice(0, -1)); }} placeholder={value.length ? "タグを追加" : "タグを検索して選択"} />
          </Combobox.EventsTarget>
        </Pill.Group>
      </PillsInput>
    </Combobox.DropdownTarget>
    <Combobox.Dropdown>
      <Combobox.Options mah={300} style={{ overflowY: "auto" }}>
        {remoteMatches.isFetching ? <Combobox.Empty>タグを検索中…</Combobox.Empty> : visible.length ? visible.map((tag) => <Combobox.Option value={tag.name} key={tag.name} active={value.includes(tag.name)}>
          <Group justify="space-between" wrap="nowrap"><Group gap="xs" wrap="nowrap"><Icons.confirm size={IconSize.menu} opacity={value.includes(tag.name) ? 1 : 0} /><Text size="sm" className="line-clamp-1">{tag.name}</Text></Group><Badge size="xs" variant="light" color="gray">{formatNumber(tag.count)}</Badge></Group>
        </Combobox.Option>) : <Combobox.Empty>一致するタグがありません</Combobox.Empty>}
      </Combobox.Options>
      {!normalized && tags.length > visible.length && <Combobox.Footer><Text size="xs" c="dimmed">上位{visible.length}件を表示 · 入力すると全タグを検索</Text></Combobox.Footer>}
    </Combobox.Dropdown>
  </Combobox>;
}
