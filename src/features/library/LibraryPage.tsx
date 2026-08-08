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
  Radio,
  ScrollArea,
  SegmentedControl,
  Select,
  SimpleGrid,
  Stack,
  Tabs,
  Text,
  Tooltip,
  useCombobox,
} from "@mantine/core";
import { useDebouncedValue, useDisclosure, useLocalStorage } from "@mantine/hooks";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDownUp,
  BookOpen,
  BookmarkPlus,
  Check,
  Filter,
  Grid2X2,
  Heart,
  LibraryBig,
  List,
  MoreHorizontal,
  RefreshCw,
  Search,
  Trash2,
  Users,
  X,
} from "lucide-react";
import { useAppSearchParams } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { EntityCard } from "@/components/EntityCard";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { WorkCard } from "@/components/WorkCard";
import { demoFacets, searchDemoWorks } from "@/mocks/demoData";
import { errorMessage, formatNumber } from "@/lib/format";
import {
  deleteDownloads,
  getFilterFacets,
  isTauriRuntime,
  searchDownloadsV2,
  searchEntityFacets,
  setFavorite,
  setFlagsForIds,
} from "@/services/dbApi";
import { searchSuggest } from "@/services/searchApi";
import type { FacetCount, SearchSuggestion, SearchV2Params } from "@/types/library";

type LibraryTab = "works" | "people" | "series";
type ViewMode = "gallery" | "compact";

interface Filters {
  sources: string[];
  contentType: string | null;
  favorite: boolean;
  watch: string | null;
  tagsInclude: string[];
  tagsExclude: string[];
  tagMode: "and" | "or";
  minChars: number | string;
  maxChars: number | string;
}

interface SavedSearch {
  id: string;
  name: string;
  query: string;
  tab: LibraryTab;
  filters: Filters;
  sortBy: string;
}

const initialFilters: Filters = {
  sources: [], contentType: null, favorite: false, watch: null,
  tagsInclude: [], tagsExclude: [], tagMode: "and", minChars: "", maxChars: "",
};

const PAGE_SIZE = 60;

export default function LibraryPage() {
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [urlParams, setUrlParams] = useAppSearchParams();
  // Query, tab and the favourite flag live in the URL so history navigation and
  // deep links such as /library?favorite=1 actually change what is shown.
  // Mirroring them into state as well would let the writer and the reader
  // overwrite each other on every change.
  const searchText = urlParams.get("q") ?? "";
  const tab = ((urlParams.get("tab") as LibraryTab) || "works");
  const favorite = urlParams.get("favorite") === "1";
  // Deep-linkable so the home tiles can open the library already filtered to
  // exactly what their number counts.
  const watch = urlParams.get("watch");
  const writeUrl = useCallback((patch: { q?: string; tab?: LibraryTab; favorite?: boolean; watch?: string | null }) => {
    const next = new URLSearchParams(urlParams);
    const q = patch.q ?? searchText;
    const nextTab = patch.tab ?? tab;
    const nextFavorite = patch.favorite ?? favorite;
    const nextWatch = patch.watch === undefined ? watch : patch.watch;
    if (q) next.set("q", q); else next.delete("q");
    if (nextTab !== "works") next.set("tab", nextTab); else next.delete("tab");
    if (nextFavorite) next.set("favorite", "1"); else next.delete("favorite");
    if (nextWatch) next.set("watch", nextWatch); else next.delete("watch");
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

  const [otherFilters, setOtherFilters] = useState<Filters>(initialFilters);
  const [sortBy, setSortBy] = useState(() => urlParams.get("sort") ?? "downloaded_at");
  const [view, setView] = useLocalStorage<ViewMode>({ key: "piep.library-view", defaultValue: "gallery" });
  const [savedSearches, setSavedSearches] = useLocalStorage<SavedSearch[]>({ key: "piep.saved-searches.v2", defaultValue: [] });
  const [filterOpened, filterDrawer] = useDisclosure(false);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  const { addToEpubQueue } = useWorkspace();

  // `favorite` is deep-linkable, so the URL owns it outright. Keeping a copy in
  // the filters state as well made the two writers overwrite each other every
  // render until React gave up on the update depth.
  const filters = useMemo<Filters>(() => ({ ...otherFilters, favorite, watch: watch ?? otherFilters.watch }), [favorite, otherFilters, watch]);
  const setFilters = useCallback((next: Filters) => {
    setOtherFilters(next);
    if (next.favorite !== favorite || (next.watch ?? null) !== watch) writeUrl({ favorite: next.favorite, watch: next.watch ?? null });
  }, [favorite, watch, writeUrl]);
  useEffect(() => { setSelected([]); }, [searchText, filters, sortBy, tab]);

  const params = useMemo<SearchV2Params>(() => ({
    text: searchText || null,
    // Selecting both providers is the same as no provider filter; the backend
    // takes a single source, and post-filtering a page would break paging.
    source: filters.sources.length === 1 ? filters.sources[0] : null,
    contentType: filters.contentType,
    favorite: filters.favorite || null,
    tagsInclude: filters.tagsInclude.length ? filters.tagsInclude : null,
    tagsExclude: filters.tagsExclude.length ? filters.tagsExclude : null,
    tagFilterMode: filters.tagMode,
    minCharCount: typeof filters.minChars === "number" ? filters.minChars : null,
    maxCharCount: typeof filters.maxChars === "number" ? filters.maxChars : null,
    watchFilter: filters.watch,
    sortBy,
    sortOrder: sortBy === "title" || sortBy === "author_name" ? "asc" : "desc",
    limit: PAGE_SIZE,
    projection: view === "compact" ? "libraryCompact" : "libraryGallery",
  }), [searchText, filters, sortBy, view]);

  // Keyset pagination: fetching a fixed slab and paging it in the browser
  // capped the library at that slab and reported the wrong total.
  const works = useInfiniteQuery({
    queryKey: ["library", params],
    queryFn: ({ pageParam }) => runtime
      ? searchDownloadsV2({ ...params, cursor: pageParam })
      : Promise.resolve(searchDemoWorks(searchText, filters.sources.length === 1 ? filters.sources[0] : null)),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
    enabled: tab === "works",
  });
  const facets = useQuery({ queryKey: ["library-facets"], queryFn: () => runtime ? getFilterFacets() : Promise.resolve(demoFacets) });
  const loadedItems = useMemo(() => works.data?.pages.flatMap((page) => page.items) ?? [], [works.data]);
  const totalCount = works.data?.pages[0]?.totalEstimate ?? null;
  const activeFilterCount = filters.sources.length + Number(Boolean(filters.contentType)) + Number(filters.favorite) + Number(Boolean(filters.watch)) + filters.tagsInclude.length + filters.tagsExclude.length + Number(filters.minChars !== "") + Number(filters.maxChars !== "");
  const loadedIds = useMemo(() => loadedItems.map((item) => item.id), [loadedItems]);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const allLoadedSelected = loadedIds.length > 0 && loadedIds.every((id) => selectedSet.has(id));

  const favoriteMutation = useMutation({
    mutationFn: ({ id, favorite }: { id: number; favorite: boolean }) => runtime ? setFavorite(id, favorite) : Promise.resolve(),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["library"] }),
    onError: (error) => notifications.show({ color: "red", title: "お気に入りを更新できません", message: errorMessage(error) }),
  });
  const deleteMutation = useMutation({
    mutationFn: (ids: number[]) => runtime ? deleteDownloads(ids) : Promise.resolve({ matchedCount: ids.length, changedCount: ids.length }),
    onSuccess: (result) => {
      notifications.show({ color: "green", title: "ライブラリから削除しました", message: `${formatNumber(result.changedCount)}件を削除しました` });
      setSelected([]); setSelectionMode(false);
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["library-facets"] });
      queryClient.invalidateQueries({ queryKey: ["library-entities"] });
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
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["dashboard"] });
    },
    onError: (error) => notifications.show({ color: "red", title: "一括操作に失敗しました", message: errorMessage(error) }),
  });

  const saveCurrentSearch = () => {
    const summary = query.trim() || [filters.sources.join("・"), filters.tagsInclude.map((tag) => `#${tag}`).join(" ")].filter(Boolean).join(" / ") || "すべての作品";
    const saved: SavedSearch = { id: crypto.randomUUID(), name: summary, query, tab, filters, sortBy };
    setSavedSearches((current) => [saved, ...current].slice(0, 12));
    notifications.show({ title: "検索を保存しました", message: summary, color: "green" });
  };
  const applySavedSearch = (saved: SavedSearch) => {
    onQueryChange(saved.query);
    setOtherFilters(saved.filters);
    setSortBy(saved.sortBy);
    writeUrl({ q: saved.query, tab: saved.tab, favorite: saved.filters.favorite });
  };

  // Stable across renders so the memoised cards are not all invalidated by a
  // fresh closure every time the list re-renders.
  const toggleSelected = useCallback((id: number, checked: boolean) => setSelected((current) => checked ? [...new Set([...current, id])] : current.filter((item) => item !== id)), []);
  const toggleFavorite = useCallback((id: number, favorite: boolean) => favoriteMutation.mutate({ id, favorite }), [favoriteMutation]);
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
    queryKey: ["library-entities", entityKind, searchText],
    queryFn: ({ pageParam }) => runtime
      ? searchEntityFacets(entityKind, searchText || null, PAGE_SIZE, pageParam)
      : Promise.resolve((entityKind === "person" ? demoFacets.authorEntities : demoFacets.series)
        .filter((entity) => !searchText || `${entity.displayName} ${entity.description ?? ""}`.toLocaleLowerCase("ja-JP").includes(searchText.toLocaleLowerCase("ja-JP")))),
    initialPageParam: 0,
    getNextPageParam: (lastPage, allPages) => lastPage.length < PAGE_SIZE ? undefined : allPages.length * PAGE_SIZE,
    enabled: tab !== "works",
  });
  const entityItems = useMemo(() => entities.data?.pages.flat() ?? [], [entities.data]);

  return (
    <div className="page page--contained library-page">
      <Paper p="md" className="library-toolbar" withBorder>
        <Stack gap="sm">
          <Group wrap="nowrap" align="flex-start">
            <Box flex={1}><LibrarySearch value={query} onChange={onQueryChange} runtime={runtime} /></Box>
            <Tooltip label="詳細フィルター">
              <Indicator label={activeFilterCount} size={16} disabled={!activeFilterCount}>
                <Button variant="default" leftSection={<Filter size={16} />} onClick={filterDrawer.open}>絞り込み</Button>
              </Indicator>
            </Tooltip>
            <Select
              value={sortBy}
              onChange={(value) => setSortBy(value ?? "downloaded_at")}
              data={[{ value: "downloaded_at", label: "保存日（新しい順）" }, { value: "source_updated_at", label: "公開更新日" }, { value: "title", label: "タイトル" }, { value: "author_name", label: "作者" }, { value: "text_length", label: "文字数" }, { value: "file_size_bytes", label: "容量" }]}
              leftSection={<ArrowDownUp size={15} />}
              w={206}
              aria-label="並び順"
            />
            <SegmentedControl
              value={view}
              onChange={(value) => setView(value as ViewMode)}
              data={[{ value: "gallery", label: <Tooltip label="ギャラリー"><Grid2X2 size={15} aria-label="ギャラリー表示" /></Tooltip> }, { value: "compact", label: <Tooltip label="リスト"><List size={15} aria-label="リスト表示" /></Tooltip> }]}
            />
            <Menu position="bottom-end" width={280} withinPortal>
              <Menu.Target><Tooltip label="保存した検索"><ActionIcon variant="default" size={36} aria-label="保存した検索"><BookmarkPlus size={16} /></ActionIcon></Tooltip></Menu.Target>
              <Menu.Dropdown>
                <Menu.Item leftSection={<BookmarkPlus size={15} />} onClick={saveCurrentSearch}>現在の検索条件を保存</Menu.Item>
                {savedSearches.length > 0 && <><Menu.Divider /><Menu.Label>保存済みの検索</Menu.Label>{savedSearches.map((saved) => <Menu.Item key={saved.id} leftSection={<Search size={14} />} onClick={() => applySavedSearch(saved)}>{saved.name}</Menu.Item>)}<Menu.Divider /><Menu.Item color="red" leftSection={<Trash2 size={14} />} onClick={() => setSavedSearches([])}>保存済み検索を消去</Menu.Item></>}
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
          <Tabs.Tab value="works" leftSection={<BookOpen size={15} />}>作品</Tabs.Tab>
          <Tabs.Tab value="people" leftSection={<Users size={15} />}>作者・クリエイター</Tabs.Tab>
          <Tabs.Tab value="series" leftSection={<LibraryBig size={15} />}>シリーズ</Tabs.Tab>
        </Tabs.List>
      </Tabs>

      <Group justify="space-between" my="md">
        <Text size="sm" c="dimmed">{tab === "works"
          ? `${formatNumber(totalCount ?? loadedItems.length)}件${totalCount !== null && loadedItems.length < totalCount ? `（${formatNumber(loadedItems.length)}件を表示中）` : ""}${works.data?.pages[0]?.searchMeta.engine ? ` · ${works.data.pages[0].searchMeta.engine}` : ""}`
          : `${formatNumber(entityItems.length)}件${entities.hasNextPage ? "以上" : ""}`}</Text>
        {tab === "works" && !selectionMode && <Button size="xs" variant="subtle" color="gray" leftSection={<Check size={14} />} onClick={() => setSelectionMode(true)}>複数選択</Button>}
      </Group>

      {tab === "works" ? (
        works.isLoading ? <LoadingState label="ライブラリを検索しています" /> : works.error ? <ErrorState error={works.error} retry={() => works.refetch()} /> : loadedItems.length ? (
          <>
            {view === "gallery"
              ? <div className="library-grid">{loadedItems.map((work) => <WorkCard key={work.id} work={work} selectionMode={selectionMode} selected={selectedSet.has(work.id)} onSelect={toggleSelected} onToggleFavorite={toggleFavorite} />)}</div>
              : <Stack gap="xs">{loadedItems.map((work) => <WorkCard compact key={work.id} work={work} selectionMode={selectionMode} selected={selectedSet.has(work.id)} onSelect={toggleSelected} onToggleFavorite={toggleFavorite} />)}</Stack>}
            <LoadMore hasNext={works.hasNextPage} loading={works.isFetchingNextPage} onLoad={() => works.fetchNextPage()} />
          </>
        ) : <EmptyState icon={Search} title="一致する作品がありません" description="検索語やフィルターを減らすか、新しい作品を保存してください。" action={<Button variant="light" onClick={() => { onQueryChange(""); setOtherFilters(initialFilters); writeUrl({ q: "", favorite: false }); }}>検索をリセット</Button>} />
      ) : entities.isLoading ? <LoadingState /> : entities.error ? <ErrorState error={entities.error} retry={() => entities.refetch()} /> : entityItems.length ? (
        <>
          <SimpleGrid cols={{ base: 1, sm: 2, xl: 3 }}>{entityItems.map((entity) => <EntityCard key={`${entity.source}:${entity.sourceKey}`} entity={entity} kind={entityKind} />)}</SimpleGrid>
          <LoadMore hasNext={entities.hasNextPage} loading={entities.isFetchingNextPage} onLoad={() => entities.fetchNextPage()} />
        </>
      ) : <EmptyState icon={Users} title="一致する項目がありません" description="名前を変えて検索してください。" />}

      <Drawer opened={filterOpened} onClose={filterDrawer.close} title="詳細フィルター" position="right" size={420} scrollAreaComponent={ScrollArea.Autosize}>
        <FilterForm value={filters} onChange={setFilters} tags={facets.data?.tags ?? []} contentTypes={facets.data?.contentTypes.map((item) => ({ value: item.name, label: `${item.name} (${item.count})` })) ?? []} onApply={filterDrawer.close} />
      </Drawer>

      {selectionMode && (
        <Paper className="selection-bar" shadow="xl" withBorder p="sm">
          <Group justify="space-between" wrap="nowrap">
            <Group gap="sm" wrap="nowrap"><Text fw={700}>{selected.length}件を選択</Text><Button variant="subtle" size="compact-sm" onClick={toggleVisibleSelection}>{allLoadedSelected ? "表示中を選択解除" : "表示中をすべて選択"}</Button>{selected.length > 0 && <Button variant="subtle" color="gray" size="compact-sm" onClick={() => setSelected([])}>選択をすべて解除</Button>}</Group>
            <Group gap="xs" wrap="nowrap">
              <Button size="sm" variant="light" leftSection={<BookOpen size={15} />} disabled={!selected.length} onClick={() => { addToEpubQueue(selected); notifications.show({ color: "green", message: `${selected.length}件をEPUBキューに追加しました` }); }}>EPUB</Button>
              <Menu position="top-end"><Menu.Target><Button size="sm" variant="default" rightSection={<MoreHorizontal size={15} />} disabled={!selected.length}>その他</Button></Menu.Target><Menu.Dropdown><Menu.Item leftSection={<Heart size={15} />} onClick={() => selectionMutation.mutate({ action: "favorite" })}>お気に入りに追加</Menu.Item><Menu.Item leftSection={<RefreshCw size={15} />} onClick={() => selectionMutation.mutate({ action: "watch" })}>更新監視を有効化</Menu.Item><Menu.Divider /><Menu.Item color="red" leftSection={<Trash2 size={15} />} onClick={confirmDelete}>削除</Menu.Item></Menu.Dropdown></Menu>
              <Divider orientation="vertical" h={26} mx={2} />
              <Tooltip label="複数選択を終了"><ActionIcon variant="light" color="red" aria-label="複数選択を終了" onClick={leaveSelection}><X size={18} /></ActionIcon></Tooltip>
            </Group>
          </Group>
        </Paper>
      )}
    </div>
  );
}

/**
 * Keyset pagination has no page numbers, so the next page is pulled in as the
 * reader reaches the end of the list, with an explicit button as the fallback.
 */
function LoadMore({ hasNext, loading, onLoad }: { hasNext: boolean; loading: boolean; onLoad: () => void }) {
  const sentinelRef = useRef<HTMLDivElement>(null);
  const loadRef = useRef(onLoad);
  loadRef.current = onLoad;

  useEffect(() => {
    const node = sentinelRef.current;
    if (!node || !hasNext) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) loadRef.current();
    }, { rootMargin: "480px" });
    observer.observe(node);
    return () => observer.disconnect();
  }, [hasNext]);

  if (!hasNext) return null;
  return (
    <Group justify="center" mt="xl" ref={sentinelRef}>
      <Button variant="default" loading={loading} onClick={onLoad}>さらに読み込む</Button>
    </Group>
  );
}

function FilterChip({ label, onRemove, color = "blue" }: { label: string; onRemove: () => void; color?: string }) {
  return <Badge variant="light" color={color} rightSection={<ActionIcon size="xs" variant="transparent" color={color} aria-label={`${label}を解除`} onClick={onRemove}><X size={11} /></ActionIcon>}>{label}</Badge>;
}

function LibrarySearch({ value, onChange, runtime }: { value: string; onChange: (value: string) => void; runtime: boolean }) {
  const [composing, setComposing] = useState(false);
  const [debounced] = useDebouncedValue(value, 180);
  const combobox = useCombobox({ onDropdownClose: () => combobox.resetSelectedOption(), onDropdownOpen: () => combobox.selectFirstOption() });
  const suggestions = useQuery({
    queryKey: ["search-suggest", debounced],
    queryFn: async (): Promise<SearchSuggestion[]> => {
      if (!debounced.trim()) return [];
      if (runtime) return (await searchSuggest({ text: debounced, limit: 8 })).items;
      return ["創作", "ファンタジー", "青葉しおり", "季節の栞"].filter((item) => item.includes(debounced)).map((item, index) => ({ kind: index < 2 ? "tag" : "author", label: item, value: item }));
    },
  });
  const items = suggestions.data ?? [];
  return (
    <Combobox store={combobox} onOptionSubmit={(selected) => { onChange(selected); combobox.closeDropdown(); }} withinPortal>
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
          leftSection={<Search size={17} />}
          rightSection={value ? <Combobox.ClearButton onClear={() => onChange("")} /> : null}
          rightSectionPointerEvents={value ? "all" : "none"}
          placeholder="タイトル、作者、タグ、本文を検索"
          aria-label="ライブラリを検索"
          size="md"
        />
      </Combobox.Target>
      <Combobox.Dropdown hidden={!value.trim()}>
        <Combobox.Options>
          {suggestions.isLoading ? <Combobox.Empty>候補を検索中…</Combobox.Empty> : items.length ? items.map((item) => <Combobox.Option value={item.value} key={`${item.kind}:${item.value}`}><Group justify="space-between"><Text size="sm">{item.label}</Text><Badge size="xs" variant="light" color="gray">{item.kind}</Badge></Group></Combobox.Option>) : <Combobox.Empty>Enterで全文検索</Combobox.Empty>}
        </Combobox.Options>
        <Combobox.Footer><Text size="xs" c="dimmed">例: tag:創作 -成人向け &quot;完全一致&quot;</Text></Combobox.Footer>
      </Combobox.Dropdown>
    </Combobox>
  );
}

function FilterForm({ value, onChange, tags, contentTypes, onApply }: { value: Filters; onChange: (value: Filters) => void; tags: FacetCount[]; contentTypes: { value: string; label: string }[]; onApply: () => void }) {
  return (
    <Stack gap="lg">
      <Box><Text fw={700} size="sm" mb="xs">ソース</Text><Checkbox.Group value={value.sources} onChange={(sources) => onChange({ ...value, sources })}><Group><Checkbox value="pixiv" label="pixiv" /><Checkbox value="fanbox" label="FANBOX" /></Group></Checkbox.Group></Box>
      <Select label="コンテンツ種別" placeholder="すべて" clearable data={contentTypes} value={value.contentType} onChange={(contentType) => onChange({ ...value, contentType })} />
      <Divider />
      <TagFilterCombobox label="含めるタグ" tags={tags} value={value.tagsInclude} onChange={(tagsInclude) => onChange({ ...value, tagsInclude })} />
      <TagFilterCombobox label="除外するタグ" tags={tags} value={value.tagsExclude} onChange={(tagsExclude) => onChange({ ...value, tagsExclude })} />
      <Radio.Group label="複数タグの条件" value={value.tagMode} onChange={(tagMode) => onChange({ ...value, tagMode: tagMode as "and" | "or" })}><Group mt="xs"><Radio value="and" label="すべて含む" /><Radio value="or" label="いずれかを含む" /></Group></Radio.Group>
      <Divider />
      <SimpleGrid cols={2}><NumberInput label="最小文字数" min={0} thousandSeparator="," value={value.minChars} onChange={(minChars) => onChange({ ...value, minChars })} /><NumberInput label="最大文字数" min={0} thousandSeparator="," value={value.maxChars} onChange={(maxChars) => onChange({ ...value, maxChars })} /></SimpleGrid>
      <Select label="更新監視" clearable placeholder="すべて" data={[{ value: "watched", label: "監視中" }, { value: "unwatched", label: "未監視" }]} value={value.watch} onChange={(watch) => onChange({ ...value, watch })} />
      <Checkbox label="お気に入りのみ" checked={value.favorite} onChange={(event) => onChange({ ...value, favorite: event.currentTarget.checked })} />
      <Alert color="gray" variant="light">フィルターは作品タブに適用されます。作者・シリーズは上部の検索語で絞り込めます。</Alert>
      <Group grow><Button variant="default" onClick={() => onChange(initialFilters)}>リセット</Button><Button onClick={onApply}>適用</Button></Group>
    </Stack>
  );
}

function TagFilterCombobox({ label, tags, value, onChange }: { label: string; tags: FacetCount[]; value: string[]; onChange: (value: string[]) => void }) {
  const [search, setSearch] = useState("");
  const combobox = useCombobox({ onDropdownClose: () => { combobox.resetSelectedOption(); setSearch(""); } });
  const normalized = search.trim().toLocaleLowerCase("ja-JP");
  const visible = tags
    .filter((tag) => !normalized || tag.name.toLocaleLowerCase("ja-JP").includes(normalized))
    .slice(0, 80);
  const toggle = (name: string) => onChange(value.includes(name) ? value.filter((item) => item !== name) : [...value, name]);
  return <Combobox store={combobox} onOptionSubmit={toggle} withinPortal>
    <Combobox.DropdownTarget>
      <PillsInput label={label} onClick={() => combobox.openDropdown()} rightSection={<Combobox.Chevron />}>
        <Pill.Group>
          {value.map((tag) => <Pill key={tag} withRemoveButton onRemove={() => onChange(value.filter((item) => item !== tag))}>{tag}</Pill>)}
          <Combobox.EventsTarget>
            <PillsInput.Field value={search} onChange={(event) => { setSearch(event.currentTarget.value); combobox.openDropdown(); combobox.updateSelectedOptionIndex(); }} onFocus={() => combobox.openDropdown()} onKeyDown={(event) => { if (event.key === "Backspace" && !search && value.length) onChange(value.slice(0, -1)); }} placeholder={value.length ? "タグを追加" : "タグを検索して選択"} />
          </Combobox.EventsTarget>
        </Pill.Group>
      </PillsInput>
    </Combobox.DropdownTarget>
    <Combobox.Dropdown>
      <Combobox.Options mah={300} style={{ overflowY: "auto" }}>
        {visible.length ? visible.map((tag) => <Combobox.Option value={tag.name} key={tag.name} active={value.includes(tag.name)}>
          <Group justify="space-between" wrap="nowrap"><Group gap="xs" wrap="nowrap"><Check size={14} opacity={value.includes(tag.name) ? 1 : 0} /><Text size="sm" className="line-clamp-1">{tag.name}</Text></Group><Badge size="xs" variant="light" color="gray">{formatNumber(tag.count)}</Badge></Group>
        </Combobox.Option>) : <Combobox.Empty>一致するタグがありません</Combobox.Empty>}
      </Combobox.Options>
      {tags.length > visible.length && <Combobox.Footer><Text size="xs" c="dimmed">{formatNumber(tags.length)}件から検索中 · 上位{visible.length}件を表示</Text></Combobox.Footer>}
    </Combobox.Dropdown>
  </Combobox>;
}
