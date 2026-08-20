import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
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
  SegmentedControl,
  Select,
  SimpleGrid,
  Stack,
  Tabs,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
  VisuallyHidden,
  useCombobox,
} from "@mantine/core";
import { useDebouncedValue, useDisclosure, useLocalStorage } from "@mantine/hooks";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { ListPager, PagingModeToggle, useBoundedNumberedPage, usePageSize, usePagingMode } from "@/components/ListPager";
import { ScrollToTop } from "@/components/ScrollToTop";
import { demoFacets, searchDemoWorks } from "@/mocks/demoData";
import { errorMessage, formatNumber } from "@/lib/format";
import { scrollViewportToTop } from "@/lib/scroll";
import { VirtualizedWorkList } from "@/features/library/VirtualizedWorkList";
import { entityKey, VirtualizedEntityGrid } from "@/features/library/VirtualizedEntityGrid";
import { exportEntityZip } from "@/services/archiveApi";
import { openSingleDialog } from "@/services/dialogApi";
import type { EntityWatchState } from "@/components/EntityCard";
import { CollectionsPanel, filterCollections, type CollectionSortBy } from "@/features/collections/CollectionsPanel";
import { listWorkCollections } from "@/services/collectionApi";
import type { WorkCollectionSummary } from "@/types/collections";
import { boundedInfiniteListOptions, INFINITE_LIST_MAX_PAGES } from "@/lib/queryLimits";
import {
  countEntityFacets,
  deleteDownloads,
  getFilterFacets,
  isTauriRuntime,
  searchDownloadsV2,
  searchEntityFacets,
  searchFilterFacets,
  setFavorite,
  setFlagsForIds,
  setWatchUpdates,
  listUpdateTargets,
  upsertUpdateTarget,
} from "@/services/dbApi";
import { searchSuggest } from "@/services/searchApi";
import { deleteSavedSearch, listSavedSearches, upsertSavedSearch } from "@/services/shelfApi";
import { readingWorkIds } from "@/features/library/readingShelf";
import { useSavedSearchMigration } from "@/features/library/savedSearchMigration";
import { deleteThenCleanup } from "@/features/library/deletedWorkCleanup";
import type { EntityFacet, EntityFacetScope, EntitySortBy, FacetCount, LibrarySortBy, LibraryWatchFilter, SavedSearchRecord, SearchSuggestion, SearchV2Params, SearchV2Result, UpdateTarget } from "@/types/library";

type LibraryTab = "works" | "people" | "series" | "collections";
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

/**
 * 一覧そのものにかける条件。
 *
 * 上の `Filters` は「配下の作品」にかかる。こちらは束ね自身の性質 - 追いかけて
 * いるか、何作品以上あるか、完結しているか。同じ引き出しに置くが、見出しで
 * 分ける。「監視中」が作品の話か作者の話か、読めなくなるため。
 */
interface EntityScopeFilters {
  watch: "watched" | "paused" | "unwatched" | null;
  minWorkCount: number | "";
  /** シリーズだけ。null は指定なし。 */
  concluded: boolean | null;
}

const initialEntityScope: EntityScopeFilters = { watch: null, minWorkCount: "", concluded: null };

function parseEntityWatch(value: unknown): EntityScopeFilters["watch"] {
  return value === "watched" || value === "paused" || value === "unwatched" ? value : null;
}

function readEntityScope(params: URLSearchParams): EntityScopeFilters {
  const concluded = params.get("edone");
  return {
    watch: parseEntityWatch(params.get("ewatch")),
    minWorkCount: numericParam(params.get("emin")),
    concluded: concluded === "1" ? true : concluded === "0" ? false : null,
  };
}

function writeEntityScope(params: URLSearchParams, scope: EntityScopeFilters) {
  const single: [string, string | null][] = [
    ["ewatch", scope.watch],
    ["emin", scope.minWorkCount === "" || scope.minWorkCount <= 1 ? null : String(scope.minWorkCount)],
    ["edone", scope.concluded === null ? null : scope.concluded ? "1" : "0"],
  ];
  for (const [key, value] of single) { if (value) params.set(key, value); else params.delete(key); }
}

function entityScopeCount(scope: EntityScopeFilters): number {
  return Number(Boolean(scope.watch))
    + Number(scope.minWorkCount !== "" && scope.minWorkCount > 1)
    + Number(scope.concluded !== null);
}

/** 一括で EPUB キューへ送るとき、1つの作者・シリーズから取る作品数の上限。 */
const ENTITY_BULK_WORK_LIMIT = 500;
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
/**
 * 作者・シリーズの並べ替え。
 *
 * 作品の鍵をそのまま出すわけにいかない。束ねに文字数も容量も関連度も無く、
 * 代わりに「中にある作品を見て決まる」鍵がある - 保存が新しいとは、その人の
 * 作品でいちばん新しく保存したもののこと。
 */
const ENTITY_SORT_OPTIONS: { value: EntitySortBy; label: string }[] = [
  { value: "work_count", label: "作品が多い順" },
  { value: "downloaded_at", label: "保存が新しい順" },
  { value: "source_updated_at", label: "更新が新しい順" },
  { value: "name", label: "名前順（あ→ん）" },
];
const ENTITY_SORT_VALUES = new Set<EntitySortBy>(ENTITY_SORT_OPTIONS.map((option) => option.value));

/** 手で作った束ねの鍵。作品数と名前と、作った順で足りる。 */
const COLLECTION_SORT_OPTIONS: { value: CollectionSortBy; label: string }[] = [
  { value: "created_at", label: "追加が新しい順" },
  { value: "name", label: "名前順（あ→ん）" },
  { value: "member_count", label: "作品が多い順" },
];
const COLLECTION_SORT_VALUES = new Set<CollectionSortBy>(COLLECTION_SORT_OPTIONS.map((option) => option.value));

export function parseCollectionSortBy(value: unknown): CollectionSortBy {
  return typeof value === "string" && COLLECTION_SORT_VALUES.has(value as CollectionSortBy)
    ? (value as CollectionSortBy)
    : "created_at";
}

export function parseEntitySortBy(value: unknown): EntitySortBy {
  return typeof value === "string" && ENTITY_SORT_VALUES.has(value as EntitySortBy)
    ? (value as EntitySortBy)
    : "work_count";
}

/** Only offered while searching, where it is the default and the backend ranks
 *  by score instead of by a column. */
const RELEVANCE_SORT: { value: LibrarySortBy; label: string } = { value: "relevance", label: "関連度が高い順" };
const SEARCH_SORT_OPTIONS = [RELEVANCE_SORT, ...SORT_OPTIONS];
const SORT_VALUES = new Set<LibrarySortBy>(SEARCH_SORT_OPTIONS.map((option) => option.value));

function parseLibraryTab(value: string | null): LibraryTab {
  return value === "people" || value === "series" || value === "collections" ? value : "works";
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
 *
 * `preferred` is the order this reader last chose. It stands in for the built-in
 * default when nothing in the address says otherwise, which is what makes the
 * choice survive closing the app. It never applies to a search: ranking by
 * relevance is the point of typing a query.
 */
export function resolveSortBy(raw: string | null, hasQuery: boolean, preferred: LibrarySortBy = "downloaded_at"): LibrarySortBy {
  const parsed = typeof raw === "string" && SORT_VALUES.has(raw as LibrarySortBy) ? raw as LibrarySortBy : null;
  // 覚えていた値も信用しない。保存先は書き換えられるし、並び順の名前は版で変わる。
  const fallback = SORT_VALUES.has(preferred) && preferred !== "relevance" ? preferred : "downloaded_at";
  if (!hasQuery) return parsed && parsed !== "relevance" ? parsed : fallback;
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

function numericParam(value: string | null): number | "" {
  return numericFilter(value === null ? null : Number.parseInt(value, 10));
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

/**
 * Every condition lives in the address.
 *
 * The drawer's filters used to be component state, so opening a work unmounted
 * them and the back button returned to a library that had quietly forgotten
 * what it was showing. They are also what a link to a narrowed library has to
 * carry, and what the entity screens already did. `favorite` and `watch` keep
 * the names the home tiles deep-link with.
 */
function readFilters(params: URLSearchParams): Filters {
  return normalizeFilters({
    sources: params.getAll("source"),
    contentType: params.get("type"),
    favorite: params.get("favorite") === "1",
    watch: parseWatchFilter(params.get("watch")),
    tagsInclude: params.getAll("tag"),
    tagsExclude: params.getAll("nottag"),
    tagMode: params.get("tagmode") === "or" ? "or" : "and",
    minChars: numericParam(params.get("minchars")),
    maxChars: numericParam(params.get("maxchars")),
  });
}

function writeFilters(params: URLSearchParams, filters: Filters) {
  params.delete("source");
  filters.sources.forEach((source) => params.append("source", source));
  params.delete("tag");
  filters.tagsInclude.forEach((tag) => params.append("tag", tag));
  params.delete("nottag");
  filters.tagsExclude.forEach((tag) => params.append("nottag", tag));
  const single: [string, string | null][] = [
    ["type", filters.contentType],
    ["favorite", filters.favorite ? "1" : null],
    ["watch", filters.watch],
    ["tagmode", filters.tagMode === "or" ? "or" : null],
    ["minchars", filters.minChars === "" ? null : String(filters.minChars)],
    ["maxchars", filters.maxChars === "" ? null : String(filters.maxChars)],
  ];
  for (const [key, value] of single) { if (value) params.set(key, value); else params.delete(key); }
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

export function updateWorkFlag(
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

/** Reverts only the failed field, preserving optimistic/successful sibling mutations. */
export function rollbackWorkFlag(
  current: InfiniteData<SearchV2Result, string | null> | undefined,
  previous: InfiniteData<SearchV2Result, string | null> | undefined,
  id: number,
  kind: "favorite" | "watch",
  params: SearchV2Params,
) {
  if (!previous) return current;
  const previousPageIndex = previous.pages.findIndex((page) => page.items.some((item) => item.id === id));
  if (previousPageIndex < 0) return current;
  const previousItemIndex = previous.pages[previousPageIndex].items.findIndex((item) => item.id === id);
  const previousItem = previous.pages[previousPageIndex].items[previousItemIndex];
  if (!previousItem) return current;
  if (!current) return previous;
  const currentContainsItem = current.pages.some((page) => page.items.some((item) => item.id === id));
  if (currentContainsItem) {
    return updateWorkFlag(current, id, kind === "favorite" ? { favorite: previousItem.favorite } : { watchUpdates: previousItem.watchUpdates }, params);
  }

  // A filtered listing may have optimistically removed the failed item. Put
  // that one row back without replacing pages changed by another mutation.
  const pages = [...current.pages];
  const targetPageIndex = Math.min(previousPageIndex, Math.max(0, pages.length - 1));
  const targetPage = pages[targetPageIndex];
  if (!targetPage) return previous;
  const items = [...targetPage.items];
  items.splice(Math.min(previousItemIndex, items.length), 0, previousItem);
  pages[targetPageIndex] = { ...targetPage, items };
  if (pages[0]) {
    pages[0] = {
      ...pages[0],
      totalEstimate: pages[0].totalEstimate === null ? null : pages[0].totalEstimate + 1,
      searchMeta: {
        ...pages[0].searchMeta,
        totalEstimate: pages[0].searchMeta.totalEstimate === null ? null : pages[0].searchMeta.totalEstimate + 1,
      },
    };
  }
  return { ...current, pages };
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
  // writeUrl は URL 側の書き手。記憶した並び順は「既定はどれか」の判断にしか
  // 使わないので、参照で渡して依存関係を増やさない。
  const preferredSortRef = useRef<LibrarySortBy>("downloaded_at");
  const writeUrl = useCallback((patch: {
    q?: string;
    tab?: LibraryTab;
    sortBy?: LibrarySortBy;
    entitySortBy?: EntitySortBy;
    collectionSortBy?: CollectionSortBy;
    saved?: number | null;
    filters?: Filters;
    entityScope?: EntityScopeFilters;
  }) => {
    const next = new URLSearchParams(urlParams);
    const q = patch.q ?? searchText;
    const nextTab = patch.tab ?? tab;
    const nextSort = patch.sortBy ?? resolveSortBy(urlParams.get("sort"), Boolean(q), preferredSortRef.current);
    // Changing anything means this is no longer the saved search it started
    // from, so the marker is dropped unless the caller is the one applying it.
    if (patch.saved) next.set("saved", String(patch.saved)); else next.delete("saved");
    if (q) next.set("q", q); else next.delete("q");
    if (nextTab !== "works") next.set("tab", nextTab); else next.delete("tab");
    if (patch.filters) writeFilters(next, normalizeFilters(patch.filters));
    if (patch.entityScope) writeEntityScope(next, patch.entityScope);
    // 一覧の並べ替えは、作品の並べ替えとは別の語彙・別の既定を持つ。
    // 同じ鍵に載せると、作者タブで「文字数が多い順」のような無効な値が残る。
    if (patch.entitySortBy) {
      if (patch.entitySortBy !== "work_count") next.set("esort", patch.entitySortBy);
      else next.delete("esort");
    }
    if (patch.collectionSortBy) {
      if (patch.collectionSortBy !== "created_at") next.set("csort", patch.collectionSortBy);
      else next.delete("csort");
    }
    // The default depends on whether a query is active, so the parameter is
    // only written when the choice differs from the default for this state.
    if (nextSort !== resolveSortBy(null, Boolean(q), preferredSortRef.current)) next.set("sort", nextSort); else next.delete("sort");
    // Page 7 of one set of conditions is not page 7 of another.
    next.delete("page");
    if (next.toString() === urlParams.toString()) return;
    setUrlParams(next, { replace: true });
  }, [searchText, setUrlParams, tab, urlParams]);
  // Switching tabs does not move the page. Whatever the reader was looking at
  // stays where it is - a tab is a filter on the same screen, and having the
  // ground move under a press that was only meant to change what is listed is
  // worse than any position the move could have chosen.
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
  // 並び順は「この人の見かた」なので覚える。検索語・絞り込み・ページ番号は
  // 「そのときの問い」なので覚えない（保存した検索がその役目を持っている）。
  const [storedSort, setStoredSort] = useLocalStorage<unknown>({ key: "piep.library-sort", defaultValue: "downloaded_at", getInitialValueInEffect: false });
  const preferredSort = parseSortBy(storedSort);
  preferredSortRef.current = preferredSort;
  const urlSortBy = resolveSortBy(urlParams.get("sort"), Boolean(searchText), preferredSort);
  const [sortBy, setSortByState] = useState<LibrarySortBy>(urlSortBy);
  useEffect(() => setSortByState((current) => current === urlSortBy ? current : urlSortBy), [urlSortBy]);
  // Works ranked by relevance have only a score cursor. Entity tabs and
  // explicitly sorted works can use a bounded direct page; everything else
  // drops a stale page parameter rather than silently ignoring it.
  const numberedPageEnabled = pagingMode === "pages" && (tab !== "works" || sortBy !== "relevance");
  const {
    page: pageParam,
    maxPage: maxDirectPage,
    limitNotice: pageLimitNotice,
    clearLimitNotice,
  } = useBoundedNumberedPage(numberedPageEnabled, urlParams, setUrlParams, pageSize);
  // A page of the library is a whole new listing, so it starts where a listing
  // starts. Set by the press rather than watched: opening a work is also a
  // change of address, and the two must not be treated as the same thing.
  //
  // Spent once the new rows are actually on screen rather than here. Scrolling
  // on the press moved the reader to the top of the page they were leaving,
  // held them there while the next one loaded, and swapped the rows underneath
  // them - two movements where there should be one. It also raced the browser's
  // scroll anchoring, which sometimes put the offset straight back.
  const pendingPageScroll = useRef(false);
  const setPage = useCallback((page: number) => {
    const boundedPage = Number.isFinite(page)
      ? Math.min(maxDirectPage, Math.max(1, Math.trunc(page)))
      : 1;
    const next = new URLSearchParams(urlParams);
    if (boundedPage > 1) next.set("page", String(boundedPage)); else next.delete("page");
    setUrlParams(next, { replace: false });
    clearLimitNotice();
    pendingPageScroll.current = true;
  }, [clearLimitNotice, maxDirectPage, setUrlParams, urlParams]);
  const setSortBy = useCallback((next: LibrarySortBy) => {
    setSortByState(next);
    // 関連度は検索の中でしか意味がない。次に開いたときの既定にはしない。
    if (next !== "relevance") setStoredSort(next);
    writeUrl({ sortBy: next });
  }, [setStoredSort, writeUrl]);
  // 作者・シリーズの並び。作品とは別に覚える - 同じ人でも、作品は保存順で
  // 見たいが作者は作品数で見たい、はふつうにある。
  const [storedEntitySort, setStoredEntitySort] = useLocalStorage<unknown>({ key: "piep.library-entity-sort", defaultValue: "work_count", getInitialValueInEffect: false });
  const entitySortBy = parseEntitySortBy(urlParams.get("esort") ?? storedEntitySort);
  const setEntitySortBy = useCallback((next: EntitySortBy) => {
    setStoredEntitySort(next);
    writeUrl({ entitySortBy: next });
  }, [setStoredEntitySort, writeUrl]);
  const [storedCollectionSort, setStoredCollectionSort] = useLocalStorage<unknown>({ key: "piep.library-collection-sort", defaultValue: "created_at", getInitialValueInEffect: false });
  const collectionSortBy = parseCollectionSortBy(urlParams.get("csort") ?? storedCollectionSort);
  const setCollectionSortBy = useCallback((next: CollectionSortBy) => {
    setStoredCollectionSort(next);
    writeUrl({ collectionSortBy: next });
  }, [setStoredCollectionSort, writeUrl]);
  const entityScope = useMemo(() => readEntityScope(urlParams), [urlParams]);
  const setEntityScope = useCallback((next: EntityScopeFilters) => writeUrl({ entityScope: next }), [writeUrl]);
  const entityScopeParam = useMemo<EntityFacetScope | null>(() => {
    const scope: EntityFacetScope = {
      watch: entityScope.watch,
      minWorkCount: entityScope.minWorkCount === "" || entityScope.minWorkCount <= 1 ? null : entityScope.minWorkCount,
      // 完結はシリーズだけの性質。作者タブへ持ち込まない。
      concluded: tab === "series" ? entityScope.concluded : null,
    };
    return scope.watch || scope.minWorkCount || scope.concluded !== null ? scope : null;
  }, [entityScope, tab]);
  // Read on the first render: a gallery that becomes a list one frame later
  // rebuilds the whole virtualised grid in front of the reader.
  const [storedView, setStoredView] = useLocalStorage<unknown>({ key: "piep.library-view", defaultValue: "gallery", getInitialValueInEffect: false });
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
  // 件数の行は全タブに出す。コレクションの数は同じ問い合わせを共有するので、
  // 二重に読みには行かない。
  const collectionsForCount = useQuery({
    queryKey: ["work-collections"],
    queryFn: () => runtime ? listWorkCollections() : Promise.resolve([] as WorkCollectionSummary[]),
    enabled: tab === "collections",
    staleTime: 30_000,
  });
  const collectionCount = collectionsForCount.data ? filterCollections(collectionsForCount.data, searchText).length : undefined;
  const [filterOpened, filterDrawer] = useDisclosure(false);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  // 作者・シリーズは行 ID を持たないので `source:sourceKey` で数える。
  const [selectedEntities, setSelectedEntities] = useState<EntityFacet[]>([]);
  // 「表示中をすべて選択」が読む、いま画面に出ている一覧。
  const entityItemsRef = useRef<EntityFacet[]>([]);
  const selectedEntityKeys = useMemo(() => new Set(selectedEntities.map(entityKey)), [selectedEntities]);
  const toggleEntity = useCallback((entity: EntityFacet, next: boolean) => {
    setSelectedEntities((current) => next
      ? (current.some((item) => entityKey(item) === entityKey(entity)) ? current : [...current, entity])
      : current.filter((item) => entityKey(item) !== entityKey(entity)));
  }, []);
  const { addToEpubQueue, removeFromEpubQueue } = useWorkspace();

  // Owned outright by the URL. Falling back to a state copy when a parameter is
  // removed makes Back navigation unable to clear the filter - and keeping a
  // second copy at all is what made the drawer's filters vanish on the way back
  // from a work, since only the URL survives the page being unmounted.
  const filters = useMemo<Filters>(() => readFilters(urlParams), [urlParams]);
  const setFilters = useCallback((next: Filters) => writeUrl({ filters: next }), [writeUrl]);
  // Compared by value: the parameters object is new on every navigation, and
  // clearing the selection whenever any of them changed threw away a selection
  // the moment the reader turned the page.
  const filterKey = useMemo(() => JSON.stringify(filters), [filters]);
  const entityScopeKey = useMemo(() => JSON.stringify(entityScopeParam), [entityScopeParam]);
  useEffect(() => { setSelected([]); setSelectedEntities([]); }, [searchText, filterKey, sortBy, tab, entitySortBy, entityScopeKey]);

  // The "読みかけ" shelf is a membership list rather than a filter: reading
  // positions are kept per device, so the library is told which works to show.
  // Read once per visit to the shelf; it must not change under an open page.
  const shelfParam = urlParams.get("shelf");
  const readingIds = useMemo(() => shelfParam === "reading" ? readingWorkIds() : null, [shelfParam]);

  // Relevance is walked with a score cursor and has no nth page, so numbers
  // are only offered once an ordering has been chosen.
  const numberedPages = numberedPageEnabled && tab === "works";
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
    ...boundedInfiniteListOptions,
    queryKey: ["library", params],
    queryFn: ({ pageParam }) => runtime
      ? searchDownloadsV2({ ...params, cursor: pageParam })
      : Promise.resolve(searchDemoWorks(searchText, filters.sources.length === 1 ? filters.sources[0] : null)),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage, allPages) => {
      const cursor = lastPage.items.length ? lastPage.nextCursor : null;
      return cursor && !allPages.slice(0, -1).some((page) => page.nextCursor === cursor) ? cursor : undefined;
    },
    // Swapping the list for a spinner collapses the page to a fraction of its
    // height, and the browser clamps the scroll position away with it. Holding
    // the page that is already on screen keeps the geometry - and the reader's
    // place in it - still while the next one is fetched.
    placeholderData: keepPreviousData,
    enabled: tab === "works",
  });
  const facets = useQuery({
    queryKey: ["library-facets"],
    queryFn: () => runtime ? getFilterFacets() : Promise.resolve(demoFacets),
    enabled: filterOpened,
  });
  const loadedItems = useMemo(() => uniqueWorks(works.data?.pages), [works.data?.pages]);
  const worksAtCacheLimit = !numberedPages
    && (works.data?.pages.length ?? 0) >= INFINITE_LIST_MAX_PAGES
    && Boolean(works.hasNextPage);
  const totalCount = works.data?.pages[0]?.totalEstimate ?? null;
  const searchMeta = works.data?.pages[0]?.searchMeta;
  const workFilterCount = filters.sources.length + Number(Boolean(filters.contentType)) + Number(filters.favorite) + Number(Boolean(filters.watch)) + filters.tagsInclude.length + filters.tagsExclude.length + Number(filters.minChars !== "") + Number(filters.maxChars !== "");
  // 束ね自身の条件も「適用中」に数える。数えないと、絞られている理由が
  // どこにも出ない一覧ができる。
  const activeFilterCount = workFilterCount + (tab === "works" ? 0 : entityScopeCount(entityScope));
  const loadedIds = useMemo(() => loadedItems.map((item) => item.id), [loadedItems]);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const selectionCount = tab === "works" ? selected.length : selectedEntities.length;
  const allLoadedSelected = loadedIds.length > 0 && loadedIds.every((id) => selectedSet.has(id));
  const allVisibleEntitiesSelected = entityItemsRef.current.length > 0
    && entityItemsRef.current.every((entity) => selectedEntityKeys.has(entityKey(entity)));
  const allVisibleSelected = tab === "works" ? allLoadedSelected : allVisibleEntitiesSelected;
  const flagMutationsInFlight = useRef(0);

  const flagMutation = useMutation({
    mutationFn: ({ id, kind, value }: { id: number; kind: "favorite" | "watch"; value: boolean }) => {
      if (!runtime) return Promise.resolve();
      return kind === "favorite" ? setFavorite(id, value) : setWatchUpdates(id, value);
    },
    onMutate: (input) => {
      flagMutationsInFlight.current += 1;
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
    onError: (error, input, context) => {
      if (context?.previous) queryClient.setQueryData(
        context.key,
        (current: InfiniteData<SearchV2Result, string | null> | undefined) => rollbackWorkFlag(current, context.previous, input.id, input.kind, params),
      );
      notifications.show({ color: "red", title: "作品情報を更新できません", message: errorMessage(error) });
    },
    onSettled: () => {
      flagMutationsInFlight.current = Math.max(0, flagMutationsInFlight.current - 1);
      if (flagMutationsInFlight.current === 0) queryClient.invalidateQueries({ queryKey: ["library"] });
    },
  });
  const deleteMutation = useMutation({
    mutationFn: (ids: number[]) => deleteThenCleanup(
      () => runtime ? deleteDownloads(ids) : Promise.resolve({ matchedCount: ids.length, changedCount: ids.length }),
      { queryClient, ids, removeFromEpubQueue },
    ),
    onSuccess: (result) => {
      notifications.show({ color: "green", title: "ライブラリから削除しました", message: `${formatNumber(result.changedCount)}件を削除しました` });
      // Reading positions outlive the works they point at, and a "読みかけ"
      // shelf counting deleted works would never go back down.
      setSelected([]); setSelectionMode(false);
    },
    onError: (error) => notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }),
  });
  const selectionMutation = useMutation({
    // One bulk command instead of one call per selected work: a large
    // selection previously fired thousands of concurrent invokes.
    mutationFn: async ({ action }: { action: "favorite" | "watch" | "unwatch" }) => {
      if (!runtime) return selected.length;
      const flags = action === "favorite" ? { favorite: true } : { watch: action === "watch" };
      const result = await setFlagsForIds(selected, flags);
      return result.matchedCount;
    },
    onSuccess: (count, input) => {
      notifications.show({
        color: "green",
        message: input.action === "unwatch"
          ? `${formatNumber(count)}件の更新監視を止めました`
          : `${formatNumber(count)}件を${input.action === "favorite" ? "お気に入り" : "更新監視"}に追加しました`,
      });
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
      notifications.show({ color: "piep", message: "検索の保存はデスクトップアプリで利用できます" });
      return;
    }
    openSaveSearchModal(suggestedName(), (name) => saveMutation.mutate(name));
  };

  const applySavedSearch = useCallback((record: SavedSearchRecord) => {
    const parsed = parseSavedParams(record.paramsJson);
    setSortByState(parsed.sortBy);
    const nextQuery = record.query ?? "";
    cancelPendingQueryWrite();
    setQuery(nextQuery);
    writeUrl({
      q: nextQuery,
      tab: parsed.tab,
      filters: parsed.filters,
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
  const toggleVisibleWorkSelection = () => setSelected((current) => {
    if (allLoadedSelected) {
      const loaded = new Set(loadedIds);
      return current.filter((id) => !loaded.has(id));
    }
    return [...new Set([...current, ...loadedIds])];
  });
  const toggleVisibleEntitySelection = () => setSelectedEntities((current) => {
    const visible = entityItemsRef.current;
    const visibleKeys = new Set(visible.map(entityKey));
    const allSelected = visible.length > 0 && visible.every((entity) => current.some((item) => entityKey(item) === entityKey(entity)));
    if (allSelected) return current.filter((item) => !visibleKeys.has(entityKey(item)));
    const merged = new Map(current.map((item) => [entityKey(item), item]));
    visible.forEach((entity) => merged.set(entityKey(entity), entity));
    return [...merged.values()];
  });
  const leaveSelection = () => { setSelectionMode(false); setSelected([]); setSelectedEntities([]); };
  const confirmDelete = () => modals.openConfirmModal({
    title: "選択した作品を削除しますか？",
    children: <Text size="sm">{selected.length}件の作品とローカルアセットを削除します。この操作は元に戻せません。</Text>,
    confirmProps: { color: "red" }, labels: { confirm: "削除する", cancel: "キャンセル" },
    onConfirm: () => deleteMutation.mutate(selected),
  });

  /**
   * What a selection of authors or series can be asked to do.
   *
   * Watching is registered here because the reader asked for it by selecting
   * and pressing - the rule the update centre keeps is that nothing registers
   * itself. Sending works to the EPUB queue resolves each entity to its works,
   * which is the only way a queue of works can be filled from a queue of names.
   */
  const entitySelectionMutation = useMutation({
    mutationFn: async (action: "epub" | "watch" | "unwatch" | "archive") => {
      if (!runtime) return 0;
      if (action === "epub") {
        const ids: number[] = [];
        for (const entity of selectedEntities) {
          const result = await searchDownloadsV2({
            ...(entityKind === "person"
              ? { personSource: entity.source, personKey: entity.sourceKey }
              : { seriesSource: entity.source, seriesKey: entity.sourceKey }),
            limit: ENTITY_BULK_WORK_LIMIT,
            projection: "bulk",
            // シリーズは話数の順。保存した順に並べると、あとから拾った
            // 1話目が最後に来る本ができあがる。
            sortBy: entityKind === "series" ? "series_order" : "downloaded_at",
            sortOrder: "asc",
          });
          ids.push(...result.items.map((item) => item.id));
        }
        const unique = [...new Set(ids)];
        addToEpubQueue(unique);
        return unique.length;
      }
      if (action === "archive") {
        const directory = await openSingleDialog({ directory: true, title: "アーカイブの書き出し先" });
        if (!directory) return 0;
        let written = 0;
        for (const entity of selectedEntities) {
          const name = entity.displayName.replace(/[\/:*?"<>|]/g, "_").slice(0, 80) || entity.sourceKey;
          await exportEntityZip(entityKind, entity.source, entity.sourceKey, `${directory}/${name}.zip`);
          written += 1;
        }
        return written;
      }
      // すでにそうなっている相手には触らない。触れば「12件を更新監視に
      // 追加しました」が、実際に変わった数と食い違う。
      const targets = action === "watch" ? entitiesToWatch : entitiesToUnwatch;
      for (const entity of targets) {
        await upsertUpdateTarget({
          targetType: entityKind === "person" ? "author" : "series",
          source: entity.source,
          sourceKey: entity.sourceKey,
          displayName: entity.displayName,
          enabled: action === "watch",
          metadataJson: null,
        });
      }
      return targets.length;
    },
    onSuccess: (count, action) => {
      queryClient.invalidateQueries({ queryKey: ["update-targets"] });
      if (action === "archive" && count === 0) return;
      notifications.show({
        color: "green",
        message: action === "epub"
          ? `${formatNumber(count)}件の作品をEPUBキューに追加しました`
          : action === "archive"
            ? `${formatNumber(count)}件を書き出しました`
            : action === "watch"
              ? `${formatNumber(count)}件を更新監視に追加しました`
              : `${formatNumber(count)}件の更新監視を止めました`,
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "一括操作に失敗しました", message: errorMessage(error) }),
  });

  // Authors and series are searched and paged in SQLite. Filtering the
  // dashboard facets in the browser only ever saw their top 60 rows, so most
  // of a large library was unreachable from these tabs.
  const entityKind = tab === "people" ? "person" : "series";
  // Authors and series are paged by offset, so the numbered mode applies here
  // exactly as it does to the works. It used to be ignored on these two tabs -
  // the preference said page numbers and the list still grew as you scrolled,
  // with nothing on screen to say why.
  // Not `numberedPages`, which also asks whether the works are ranked by
  // relevance - a question these two tabs do not have.
  const entityOffset = numberedPageEnabled ? (pageParam - 1) * pageSize : 0;
  // Authors and series are groupings of works, so the drawer narrows them by
  // the same conditions it narrows the works by: an author appears when they
  // have a work that passes, and their count is how many of theirs do. Taken
  // from the works parameters rather than rebuilt, so the two tabs cannot drift
  // into two readings of the same filter. Text, ordering and paging belong to
  // the listing, not to the filter, and are left behind.
  const entityFilters = useMemo<SearchV2Params | null>(() => {
    const narrowed: SearchV2Params = {
      idsInclude: params.idsInclude,
      source: params.source,
      contentType: params.contentType,
      favorite: params.favorite,
      tagsInclude: params.tagsInclude,
      tagsExclude: params.tagsExclude,
      // Only meaningful alongside the tags it applies to, and it always has a
      // value - so carrying it unconditionally would make every listing look
      // filtered to the check below.
      tagFilterMode: params.tagsInclude ? params.tagFilterMode : null,
      minCharCount: params.minCharCount,
      maxCharCount: params.maxCharCount,
      watchFilter: params.watchFilter,
    };
    return Object.values(narrowed).some((value) => value !== null && value !== undefined) ? narrowed : null;
  }, [params]);
  // The preview's stand-in for the grouped query, so it pages and counts the
  // same way rather than handing back everything and never running out.
  // Only the provider is honoured here: the demo entities are a handful of
  // rows with no works behind them, so nothing else has anything to filter.
  const demoEntityMatches = useCallback(() => (entityKind === "person" ? demoFacets.authorEntities : demoFacets.series)
    .filter((entity) => !searchText || `${entity.displayName} ${entity.description ?? ""}`.toLocaleLowerCase("ja-JP").includes(searchText.toLocaleLowerCase("ja-JP")))
    .filter((entity) => !entityFilters?.source || entity.source === entityFilters.source),
  [entityKind, searchText, entityFilters]);
  const entities = useInfiniteQuery({
    ...boundedInfiniteListOptions,
    // The mode belongs in the key, not just the offset it produces. Scrolling
    // and page one both start at offset zero, so the two modes shared one cache
    // entry - and switching to page numbers after scrolling a long way redrew
    // every accumulated page as "page 1" instead of the first pageSize rows.
    queryKey: ["library-entities", entityKind, searchText, pageSize, entityOffset, numberedPageEnabled, entityFilters, entitySortBy, entityScopeParam],
    queryFn: ({ pageParam }) => runtime
      ? searchEntityFacets(entityKind, searchText || null, pageSize, pageParam, entityFilters, entitySortBy, null, entityScopeParam)
      : Promise.resolve(demoEntityMatches().slice(pageParam, pageParam + pageSize)),
    initialPageParam: entityOffset,
    getNextPageParam: (lastPage, _allPages, lastPageParam) => lastPage.length < pageSize
      ? undefined
      : lastPageParam + lastPage.length,
    placeholderData: keepPreviousData,
    enabled: tab !== "works",
  });
  // One pass over the same grouping, per set of conditions rather than per page
  // turned. Without it the pager cannot name a last page, and the count beside
  // the tabs had to hedge with "以上".
  const entityTotal = useQuery({
    queryKey: ["library-entity-count", entityKind, searchText, entityFilters, entityScopeParam],
    queryFn: () => runtime ? countEntityFacets(entityKind, searchText || null, entityFilters, entityScopeParam) : Promise.resolve(demoEntityMatches().length),
    enabled: tab !== "works",
    staleTime: 60_000,
  });
  /**
   * 追いかけている相手の一覧。
   *
   * 一覧を描くのに要るのは「この人は監視中か」だけなので、タブを開いたとき
   * 1回だけ読んで集合にする。カードの印も、選択バーの件数も、同じ1本から
   * 出る - 数百件・1クエリで、行ごとに聞きに行くより安い。
   */
  const watchTargets = useQuery({
    queryKey: ["update-targets", "library"],
    queryFn: () => runtime ? listUpdateTargets<UpdateTarget>(null, false) : Promise.resolve([] as UpdateTarget[]),
    enabled: tab !== "works",
    staleTime: 30_000,
  });
  const watchStateByKey = useMemo(() => {
    const map = new Map<string, EntityWatchState>();
    for (const target of watchTargets.data ?? []) {
      const kind = target.targetType === "author" ? "person" : target.targetType === "series" ? "series" : null;
      if (!kind) continue;
      map.set(`${kind}:${target.source}:${target.sourceKey}`, target.enabled ? "watching" : "paused");
    }
    return map;
  }, [watchTargets.data]);
  /**
   * 一覧からその場で追いかける・止める。
   *
   * 「止める」は登録を消すのではなく停止にする。消してしまうと、また追い
   * かけたくなったときに一から登録し直しになり、いつから見ていたかも消える。
   */
  const watchToggleMutation = useMutation({
    mutationFn: async ({ entity, next }: { entity: EntityFacet; next: boolean }) => {
      if (!runtime) return;
      await upsertUpdateTarget({
        targetType: entityKind === "person" ? "author" : "series",
        source: entity.source,
        sourceKey: entity.sourceKey,
        displayName: entity.displayName,
        enabled: next,
        metadataJson: null,
      });
    },
    onSuccess: (_result, input) => {
      queryClient.invalidateQueries({ queryKey: ["update-targets"] });
      notifications.show({
        color: input.next ? "piep" : "gray",
        message: `${input.entity.displayName}の更新監視を${input.next ? "開始しました" : "止めました"}`,
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "更新監視を変更できません", message: errorMessage(error) }),
  });
  const toggleEntityWatch = useCallback((entity: EntityFacet, next: boolean) => watchToggleMutation.mutate({ entity, next }), [watchToggleMutation]);
  const entityWatchState = useCallback((entity: { source: string; sourceKey: string }) =>
    watchStateByKey.get(`${entityKind}:${entity.source}:${entity.sourceKey}`) ?? null,
  [entityKind, watchStateByKey]);

  // 押す前に分かっていることは、押す前に言う。
  const entitiesToWatch = useMemo(() => selectedEntities.filter((entity) => entityWatchState(entity) !== "watching"), [entityWatchState, selectedEntities]);
  const entitiesToUnwatch = useMemo(() => selectedEntities.filter((entity) => entityWatchState(entity) === "watching"), [entityWatchState, selectedEntities]);
  const selectedEntityWorkCount = useMemo(() => selectedEntities.reduce((total, entity) => total + entity.count, 0), [selectedEntities]);

  const entityItems = useMemo(() => {
    const seen = new Set<string>();
    return entities.data?.pages.flatMap((page) => page.filter((entity) => {
      const identity = `${entity.source}\0${entity.sourceKey}`;
      if (seen.has(identity)) return false;
      seen.add(identity);
      return true;
    })) ?? [];
  }, [entities.data?.pages]);
  entityItemsRef.current = entityItems;
  const entitiesAtCacheLimit = pagingMode !== "pages"
    && (entities.data?.pages.length ?? 0) >= INFINITE_LIST_MAX_PAGES
    && Boolean(entities.hasNextPage);

  // The one movement, made when the rows for the page that was asked for have
  // arrived. A layout effect so it lands in the same frame the new rows are
  // painted in, rather than a frame after them.
  const showingRequestedPage = tab === "works"
    ? !works.isPlaceholderData && !works.isFetching
    : !entities.isPlaceholderData && !entities.isFetching;
  // `pageParam` is in the dependencies as well as the settled flag: a page that
  // is already cached settles in the same render it was asked for, so the flag
  // never changes and an effect watching only it would never run.
  useLayoutEffect(() => {
    if (!pendingPageScroll.current || !showingRequestedPage) return;
    pendingPageScroll.current = false;
    scrollViewportToTop(document.getElementById("main-content"));
  }, [pageParam, showingRequestedPage]);

  return (
    <div className="page page--contained library-page">
      <VisuallyHidden component="h1">ライブラリ</VisuallyHidden>
      {/* どのタブでも出したままにする。タブは同じ画面の切り替えなのに、
          コレクションだけツールバーごと消えていたので、押した瞬間に下の
          ものが70px跳ね上がっていた。中身は器ごと入れ替えるのではなく、
          このタブで意味を持つものだけを残す。 */}
      <Paper p="md" className="library-toolbar" withBorder>
        <Stack gap="sm">
          {/* Centred, not top-aligned: the search field is taller than every
              control beside it, so aligning tops left them all sitting three
              pixels high against it. */}
          <Group wrap="nowrap" align="center">
            <Box className="library-toolbar__search"><LibrarySearch value={query} onChange={onQueryChange} runtime={runtime} /></Box>
            {tab !== "collections" && (
              <Tooltip label="詳細フィルター">
                <Indicator label={activeFilterCount} size={16} disabled={!activeFilterCount}>
                  <Button variant="default" leftSection={<Icons.filter size={IconSize.action} />} onClick={filterDrawer.open}>絞り込み</Button>
                </Indicator>
              </Tooltip>
            )}
            {/* 場所は変えず、中身だけ差し替える。タブを切り替えるたびに
                ツールバーの部品が動くのが、いちばん目に障る。 */}
            <Select
              value={tab === "works" ? sortBy : tab === "collections" ? collectionSortBy : entitySortBy}
              onChange={(value) => {
                if (tab === "works") setSortBy(parseSortBy(value));
                else if (tab === "collections") setCollectionSortBy(parseCollectionSortBy(value));
                else setEntitySortBy(parseEntitySortBy(value));
              }}
              data={tab === "works" ? (searchText ? SEARCH_SORT_OPTIONS : SORT_OPTIONS) : tab === "collections" ? COLLECTION_SORT_OPTIONS : ENTITY_SORT_OPTIONS}
              leftSection={<Icons.sort size={IconSize.menu} />}
              className="library-toolbar__sort"
              aria-label="並び順"
            />
            {/* 表示形式と保存した検索は、作品・作者・シリーズの道具。
                コレクションでは意味を持たないので出さない。高さは変わらない。 */}
            {tab !== "collections" && <SegmentedControl
              value={view}
              onChange={(value) => setView(parseViewMode(value))}
              data={[{ value: "gallery", label: <Tooltip label="ギャラリー"><Icons.viewGrid size={IconSize.menu} aria-label="ギャラリー表示" /></Tooltip> }, { value: "compact", label: <Tooltip label="リスト"><Icons.viewList size={IconSize.menu} aria-label="リスト表示" /></Tooltip> }]}
              aria-label="表示形式"
            />}
            {tab !== "collections" && <Menu position="bottom-end" width={280} withinPortal>
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
            </Menu>}
          </Group>
          {activeFilterCount > 0 && (
            <Group gap="xs">
              <Text size="xs" c="dimmed" fw={600}>適用中</Text>
              {filters.sources.map((source) => <FilterChip key={source} label={source} onRemove={() => setFilters({ ...filters, sources: filters.sources.filter((item) => item !== source) })} />)}
              {filters.tagsInclude.map((tag) => <FilterChip key={`+${tag}`} label={`#${tag}`} onRemove={() => setFilters({ ...filters, tagsInclude: filters.tagsInclude.filter((item) => item !== tag) })} />)}
              {filters.tagsExclude.map((tag) => <FilterChip key={`-${tag}`} label={`除外 #${tag}`} color="red" onRemove={() => setFilters({ ...filters, tagsExclude: filters.tagsExclude.filter((item) => item !== tag) })} />)}
              {tab !== "works" && entityScope.watch && <FilterChip label={entityScope.watch === "watched" ? "監視中" : entityScope.watch === "paused" ? "停止中" : "未登録"} onRemove={() => setEntityScope({ ...entityScope, watch: null })} />}
              {tab !== "works" && entityScope.minWorkCount !== "" && entityScope.minWorkCount > 1 && <FilterChip label={`${entityScope.minWorkCount}作品以上`} onRemove={() => setEntityScope({ ...entityScope, minWorkCount: "" })} />}
              {tab === "series" && entityScope.concluded !== null && <FilterChip label={entityScope.concluded ? "完結" : "連載中"} onRemove={() => setEntityScope({ ...entityScope, concluded: null })} />}
              <Button size="compact-xs" variant="subtle" color="gray" onClick={() => writeUrl({ filters: initialFilters, entityScope: initialEntityScope })}>すべて解除</Button>
            </Group>
          )}
        </Stack>
      </Paper>

      <Tabs value={tab} onChange={(value) => setTab((value as LibraryTab) ?? "works")} mt="lg">
        <Tabs.List>
          <Tabs.Tab value="works" leftSection={<Icons.epubAdd size={IconSize.menu} />}>作品</Tabs.Tab>
          <Tabs.Tab value="people" leftSection={<Icons.people size={IconSize.menu} />}>作者・クリエイター</Tabs.Tab>
          <Tabs.Tab value="series" leftSection={<Icons.series size={IconSize.menu} />}>シリーズ</Tabs.Tab>
          <Tabs.Tab value="collections" leftSection={<Icons.collection size={IconSize.menu} />}>コレクション</Tabs.Tab>
        </Tabs.List>
      </Tabs>

      {/* 件数の行も、タブで消さない。ここが消えると下がもう一段跳ねる。 */}
      <Group justify="space-between" my="md" gap="xs" wrap="nowrap">
        <Group gap={8} wrap="nowrap" miw={0}>
          <Text size="sm" c="dimmed" style={{ whiteSpace: "nowrap" }}>{tab === "works"
            ? `${formatNumber(totalCount ?? loadedItems.length)}件${totalCount !== null && loadedItems.length < totalCount ? `（${formatNumber(loadedItems.length)}件を表示中）` : ""}`
            : tab === "collections"
              ? `${formatNumber(collectionCount ?? 0)}件`
              : entityTotal.data !== undefined
                ? `${formatNumber(entityTotal.data)}件`
                : `${formatNumber(entityItems.length)}件${entities.hasNextPage ? "以上" : ""}`}</Text>
          {tab === "works" && searchText && searchMeta?.explanations?.length
            ? <SearchInterpretation meta={searchMeta} />
            : null}
          {/* Beside the count, which is the thing it changes. */}
          {tab !== "collections" && <PagingModeToggle />}
        </Group>
        {tab !== "collections" && !selectionMode && <Button size="xs" variant="subtle" color="gray" leftSection={<Icons.confirm size={IconSize.menu} />} onClick={() => setSelectionMode(true)}>複数選択</Button>}
      </Group>

      {tab === "collections" ? <CollectionsPanel query={searchText} sortBy={collectionSortBy} /> : <>

      {pageLimitNotice && (
        <Alert color="yellow" title="ページ番号を調整しました" role="status" mb="md">
          負荷を抑えるため、直接開けるのは{maxDirectPage}ページ目までです。それより先は「自動」で続きを読み込むか、検索・絞り込みで対象を狭めてください。
        </Alert>
      )}

      {tab === "works" ? (
        works.isLoading ? <LoadingState label="ライブラリを検索しています" /> : works.error ? <ErrorState error={works.error} retry={() => works.refetch()} /> : loadedItems.length ? (
          <>
            <VirtualizedWorkList items={loadedItems} view={view} selectionMode={selectionMode} selected={selectedSet} onSelect={toggleSelected} onToggleFavorite={toggleFavorite} onToggleWatch={toggleWatch} />
            <ListPager
              hasNext={Boolean(works.hasNextPage) && !worksAtCacheLimit}
              loading={works.isFetchingNextPage || works.isFetching}
              loaded={loadedItems.length}
              total={totalCount}
              onLoad={() => works.fetchNextPage()}
              endMessage={worksAtCacheLimit
                ? sortBy === "relevance"
                  ? `メモリ使用量を抑えるため${formatNumber(loadedItems.length)}件で停止しました。検索条件を絞ると続きへ到達できます。`
                  : `メモリ使用量を抑えるため${formatNumber(loadedItems.length)}件で停止しました。「ページ番号」に切り替えると続きへ移動できます。`
                : undefined}
              pages={{
                current: pageParam,
                size: pageSize,
                onGoTo: setPage,
                maxDirectPage,
                limitNotice: pageLimitNotice,
                unavailableReason: sortBy === "relevance"
                  ? "関連度順はページ番号で移動できません。並び順を選ぶとページ番号が使えます。"
                  : null,
              }}
            />
          </>
        ) : <EmptyState icon={Icons.search} title="一致する作品がありません" description="検索語やフィルターを減らすか、新しい作品を保存してください。" action={<Button variant="light" onClick={() => { onQueryChange(""); writeUrl({ q: "", filters: initialFilters }); }}>検索をリセット</Button>} />
      ) : entities.isLoading ? <LoadingState /> : entities.error ? <ErrorState error={entities.error} retry={() => entities.refetch()} /> : entityItems.length ? (
        <>
          <VirtualizedEntityGrid
            items={entityItems}
            kind={entityKind}
            selectionMode={selectionMode}
            selected={selectedEntityKeys}
            onSelect={toggleEntity}
            watchState={entityWatchState}
            onToggleWatch={toggleEntityWatch}
          />
          <ListPager
            hasNext={Boolean(entities.hasNextPage) && !entitiesAtCacheLimit}
            loading={entities.isFetchingNextPage || entities.isFetching}
            loaded={entityItems.length}
            total={entityTotal.data ?? null}
            onLoad={() => entities.fetchNextPage()}
            endMessage={entitiesAtCacheLimit ? `メモリ使用量を抑えるため${formatNumber(entityItems.length)}件で停止しました。「ページ番号」に切り替えると続きへ移動できます。` : undefined}
            pages={{ current: pageParam, size: pageSize, onGoTo: setPage, maxDirectPage, limitNotice: pageLimitNotice }}
          />
        </>
      ) : <EmptyState icon={Icons.people} title="一致する項目がありません" description="名前を変えて検索してください。" />}
      </>}

      <Drawer opened={filterOpened} onClose={filterDrawer.close} title="詳細フィルター" position="right" size={420} className="filter-drawer">
        <FilterForm
          value={filters}
          scope={entityScope}
          tab={tab}
          runtime={runtime}
          tags={facets.data?.tags ?? []}
          contentTypes={facets.data?.contentTypes.map((item) => ({ value: item.name, label: `${item.name} (${item.count})` })) ?? []}
          onApply={(nextFilters, nextScope) => { writeUrl({ filters: nextFilters, entityScope: nextScope }); filterDrawer.close(); }}
        />
      </Drawer>

      {/* Raised while the selection bar is up so the two never stack. */}
      <ScrollToTop offsetBottom={selectionMode ? 104 : undefined} />

      {/* コレクションには複数選択がまだ無いので、帯も出さない。 */}
      {selectionMode && tab !== "collections" && (
        <Paper className="selection-bar" shadow="xl" withBorder p="sm" role="region" aria-label="複数選択の操作">
          <div className="selection-bar__inner">
            <Group gap="sm" wrap="nowrap" className="selection-bar__summary">
              <div className="selection-bar__count" data-empty={!selectionCount || undefined}>
                <Icons.confirm size={IconSize.action} strokeWidth={2.6} aria-hidden />
                <Text fw={750}>{formatNumber(selectionCount)}{tab === "people" ? "人" : "件"}</Text>
                {/* 束ねを選ぶことは、その下の作品を選ぶこと。EPUBへ何冊
                    飛ぶのかは、押す前に分かっていなければならない。 */}
                <Text size="xs" c="dimmed">選択中{tab !== "works" && selectionCount > 0 ? ` · 作品 ${formatNumber(selectedEntityWorkCount)}件` : ""}</Text>
              </div>
              <Button variant="subtle" size="compact-sm" onClick={tab === "works" ? toggleVisibleWorkSelection : toggleVisibleEntitySelection}>{allVisibleSelected ? "表示中を解除" : "表示中をすべて選択"}</Button>
              {selectionCount > 0 && <Button variant="subtle" color="gray" size="compact-sm" onClick={() => { setSelected([]); setSelectedEntities([]); }}>すべて解除</Button>}
            </Group>
            <Group gap="xs" wrap="nowrap" className="selection-bar__actions">
              {tab === "works" ? (
                <>
                  <Button size="sm" variant="light" leftSection={<Icons.epubAdd size={IconSize.menu} />} disabled={!selected.length} onClick={() => { addToEpubQueue(selected); notifications.show({ color: "green", message: `${selected.length}件をEPUBキューに追加しました` }); }}>EPUB</Button>
                  <Menu position="top-end"><Menu.Target><Button size="sm" variant="default" rightSection={<Icons.more size={IconSize.menu} />} disabled={!selected.length}>その他</Button></Menu.Target><Menu.Dropdown><Menu.Item leftSection={<Icons.favorite size={IconSize.menu} />} onClick={() => selectionMutation.mutate({ action: "favorite" })}>お気に入りに追加</Menu.Item><Menu.Item leftSection={<Icons.watch size={IconSize.menu} />} onClick={() => selectionMutation.mutate({ action: "watch" })}>更新監視を有効化</Menu.Item>{/* 追加できて解除できないのは片手落ちだった。 */}<Menu.Item leftSection={<Icons.pause size={IconSize.menu} />} onClick={() => selectionMutation.mutate({ action: "unwatch" })}>更新監視を止める</Menu.Item><Menu.Divider /><Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} onClick={confirmDelete}>削除</Menu.Item></Menu.Dropdown></Menu>
                </>
              ) : (
                <>
                  {/* このタブの主役は監視。件数はボタン自身が名乗る - 「12件を
                      追加しました」が実際は9件だった、を無くす。 */}
                  <Tooltip label={selectedEntities.length && !entitiesToWatch.length ? `選んだ${formatNumber(selectedEntities.length)}件はすべて監視中です` : "選んだ相手の新作を追いかけます"}>
                    <Button
                      size="sm"
                      leftSection={<Icons.watch size={IconSize.menu} />}
                      disabled={!entitiesToWatch.length}
                      loading={entitySelectionMutation.isPending && entitySelectionMutation.variables === "watch"}
                      onClick={() => entitySelectionMutation.mutate("watch")}
                    >
                      {entitiesToWatch.length ? `${formatNumber(entitiesToWatch.length)}件を更新監視` : "更新監視"}
                    </Button>
                  </Tooltip>
                  <Button size="sm" variant="light" leftSection={<Icons.epubAdd size={IconSize.menu} />} disabled={!selectedEntities.length || entitySelectionMutation.isPending} loading={entitySelectionMutation.isPending && entitySelectionMutation.variables === "epub"} onClick={() => entitySelectionMutation.mutate("epub")}>EPUB</Button>
                  <Menu position="top-end">
                    <Menu.Target><Button size="sm" variant="default" rightSection={<Icons.more size={IconSize.menu} />} disabled={!selectedEntities.length}>その他</Button></Menu.Target>
                    <Menu.Dropdown>
                      <Menu.Item leftSection={<Icons.pause size={IconSize.menu} />} disabled={!entitiesToUnwatch.length} onClick={() => entitySelectionMutation.mutate("unwatch")}>
                        {entitiesToUnwatch.length ? `${formatNumber(entitiesToUnwatch.length)}件の更新監視を止める` : "更新監視を止める"}
                      </Menu.Item>
                      <Menu.Divider />
                      <Menu.Item leftSection={<Icons.archive size={IconSize.menu} />} onClick={() => entitySelectionMutation.mutate("archive")}>アーカイブを書き出す</Menu.Item>
                    </Menu.Dropdown>
                  </Menu>
                </>
              )}
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
      color={strict ? "piep" : "gray"}
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

function FilterChip({ label, onRemove, color = "piep" }: { label: string; onRemove: () => void; color?: string }) {
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
          {suggestions.isLoading ? <Combobox.Empty>候補を検索中…</Combobox.Empty> : items.length ? items.map((item, index) => <Combobox.Option value={`suggestion:${index}`} key={`${item.kind}:${item.source ?? ""}:${item.sourceKey ?? item.value}`}><Group justify="space-between" wrap="nowrap"><Text size="sm" lineClamp={1}>{item.label}</Text><Group gap={5} wrap="nowrap">{item.exactMatch && <Badge size="xs" variant="filled" color="piep">完全一致</Badge>}<Badge size="xs" variant="light" color={item.kind === "tag" ? "piep" : "piep"}>{SUGGESTION_KIND_LABEL[item.kind] ?? item.kind}</Badge>{typeof item.count === "number" && <Text size="xs" c="dimmed">{formatNumber(item.count)}件</Text>}</Group></Group></Combobox.Option>) : <Combobox.Empty>Enterで全文検索</Combobox.Empty>}
        </Combobox.Options>
        <Combobox.Footer><Text size="xs" c="dimmed">例: tag:創作 -成人向け &quot;完全一致&quot;</Text></Combobox.Footer>
      </Combobox.Dropdown>
    </Combobox>
  );
}

function FilterForm({ value, scope, tab, runtime, tags, contentTypes, onApply }: {
  value: Filters;
  scope: EntityScopeFilters;
  tab: LibraryTab;
  runtime: boolean;
  tags: FacetCount[];
  contentTypes: { value: string; label: string }[];
  onApply: (value: Filters, scope: EntityScopeFilters) => void;
}) {
  const [draft, setDraft] = useState(() => normalizeFilters(value));
  const [scopeDraft, setScopeDraft] = useState(scope);
  // Compared by value: the applied filters are rebuilt from the address on
  // every navigation, and a fresh object each time reset the half-filled form.
  const applied = JSON.stringify(value);
  useEffect(() => setDraft(normalizeFilters(JSON.parse(applied) as Filters)), [applied]);
  const appliedScope = JSON.stringify(scope);
  useEffect(() => setScopeDraft(JSON.parse(appliedScope) as EntityScopeFilters), [appliedScope]);
  const entityTab = tab === "people" || tab === "series";
  const entityNoun = tab === "series" ? "シリーズ" : "作者";
  const min = numericFilterOrNull(draft.minChars);
  const max = numericFilterOrNull(draft.maxChars);
  const invalidRange = min !== null && max !== null && min > max;
  // The two buttons that make any of this take effect used to be the last thing
  // in a long scrolling form, so on an ordinary window they opened below the
  // bottom edge - the drawer looked like it had no way to apply anything.
  return (
    <div className="filter-form">
    <Stack gap="lg" className="filter-form__fields">
      {/* At the top, and as a line rather than a boxed notice. It qualifies
          everything below it, so it is the one thing worth reading before any
          of this is set - and at the end of the form it was permanently half
          cut off by the bottom edge, which reads as something being broken. */}
      {entityTab ? (
        <>
          {/* 束ね自身の条件が先。いま見ている一覧そのものの話なので、
              配下の作品の話より手前にある。 */}
          <Box>
            <Text fw={700} size="sm" mb={4}>この{entityNoun}たち自身の条件</Text>
            <Text size="xs" c="dimmed" mb="xs">一覧に並ぶかどうかを、{entityNoun}の側で決めます。</Text>
            <Stack gap="sm">
              <Select
                label="更新監視"
                clearable
                placeholder="すべて"
                data={[
                  { value: "watched", label: "監視中" },
                  { value: "paused", label: "停止中" },
                  { value: "unwatched", label: "未登録" },
                ]}
                value={scopeDraft.watch}
                onChange={(watch) => setScopeDraft({ ...scopeDraft, watch: parseEntityWatch(watch) })}
              />
              <NumberInput
                label="作品数の下限"
                description="これ以上の作品を持つものだけ"
                placeholder="指定なし"
                hideControls
                min={0}
                max={Number.MAX_SAFE_INTEGER}
                allowDecimal={false}
                allowNegative={false}
                value={scopeDraft.minWorkCount}
                onChange={(minWorkCount) => setScopeDraft({ ...scopeDraft, minWorkCount: numericFilter(typeof minWorkCount === "string" ? Number.parseInt(minWorkCount, 10) : minWorkCount) })}
              />
              {tab === "series" && (
                <Select
                  label="連載の状態"
                  clearable
                  placeholder="すべて"
                  description="取得元がまだ何も言っていないシリーズは、どちらにも入りません"
                  data={[{ value: "concluded", label: "完結" }, { value: "ongoing", label: "連載中" }]}
                  value={scopeDraft.concluded === null ? null : scopeDraft.concluded ? "concluded" : "ongoing"}
                  onChange={(state) => setScopeDraft({ ...scopeDraft, concluded: state === "concluded" ? true : state === "ongoing" ? false : null })}
                />
              )}
            </Stack>
          </Box>
          <Divider />
          <Text size="xs" c="dimmed" className="filter-form__scope">ここから下は配下の作品の条件です。その作品を持つ{entityNoun}だけが残ります。</Text>
        </>
      ) : (
        <Text size="xs" c="dimmed" className="filter-form__scope">これらは作品タブに適用されます。</Text>
      )}
      <Box><Text fw={700} size="sm" mb="xs">ソース</Text><Checkbox.Group value={draft.sources} onChange={(sources) => setDraft({ ...draft, sources })}><Group><Checkbox value="pixiv" label="pixiv" /><Checkbox value="fanbox" label="FANBOX" /></Group></Checkbox.Group></Box>
      <Select label="コンテンツ種別" placeholder="すべて" clearable data={contentTypes} value={draft.contentType} onChange={(contentType) => setDraft({ ...draft, contentType })} />
      <Divider />
      <TagFilterCombobox runtime={runtime} label="含めるタグ" tags={tags} value={draft.tagsInclude} onChange={(tagsInclude) => setDraft({ ...draft, tagsInclude, tagsExclude: draft.tagsExclude.filter((tag) => !tagsInclude.includes(tag)) })} />
      <TagFilterCombobox runtime={runtime} label="除外するタグ" tags={tags} value={draft.tagsExclude} onChange={(tagsExclude) => setDraft({ ...draft, tagsExclude, tagsInclude: draft.tagsInclude.filter((tag) => !tagsExclude.includes(tag)) })} />
      <Radio.Group label="複数タグの条件" value={draft.tagMode} onChange={(tagMode) => setDraft({ ...draft, tagMode: tagMode === "or" ? "or" : "and" })}><Group mt="xs"><Radio value="and" label="すべて含む" /><Radio value="or" label="いずれかを含む" /></Group></Radio.Group>
      <Divider />
      <SimpleGrid cols={2}><NumberInput label="最小文字数" hideControls min={0} max={Number.MAX_SAFE_INTEGER} allowDecimal={false} allowNegative={false} thousandSeparator="," value={draft.minChars} onChange={(minChars) => setDraft({ ...draft, minChars })} /><NumberInput label="最大文字数" hideControls min={0} max={Number.MAX_SAFE_INTEGER} allowDecimal={false} allowNegative={false} thousandSeparator="," value={draft.maxChars} onChange={(maxChars) => setDraft({ ...draft, maxChars })} /></SimpleGrid>
      {invalidRange && <Alert color="red">最大文字数は最小文字数以上にしてください。</Alert>}
      {/* 同じ引き出しに「更新監視」が二つ並ぶ。上は作者・シリーズ自身の登録、
          こちらは作品ごとの監視。名前で区別しないと、どちらを触ったのか
          分からなくなる。 */}
      <Select label={entityTab ? "作品の更新監視" : "更新監視"} clearable placeholder="すべて" data={[{ value: "watched", label: "監視中" }, { value: "unwatched", label: "未監視" }]} value={draft.watch} onChange={(watch) => setDraft({ ...draft, watch: parseWatchFilter(watch) })} />
      <Checkbox label="お気に入りのみ" checked={draft.favorite} onChange={(event) => setDraft({ ...draft, favorite: event.currentTarget.checked })} />
    </Stack>
    <Group grow className="filter-form__actions"><Button variant="default" onClick={() => { setDraft(initialFilters); setScopeDraft(initialEntityScope); }}>リセット</Button><Button disabled={invalidRange} onClick={() => onApply(normalizeFilters(draft), scopeDraft)}>適用</Button></Group>
    </div>
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
