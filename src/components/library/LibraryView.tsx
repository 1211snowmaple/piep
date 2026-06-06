import { useState, useEffect, useCallback, useMemo, useRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { TrashIcon, ExportIcon, ArchiveIcon, ImageIcon, RefreshIcon, SyncIcon, FunnelIcon, TemplateIcon, LibraryIcon, PanelRightIcon, BookIcon, GalleryIcon, CompactIcon, CheckSquaresIcon, SquaresIcon } from "../icons/Icons";
import {
  deleteDownloads,
  getAssetUrl,
  getFilterFacets,
  getSearchIndexStatus,
  getStats,
  searchDownloadsV2,
  searchFilterFacets,
  setFavorite,
  setWatchUpdates,
  setWatchUpdatesForSearch,
} from "../../services/dbApi";
import { exportAllZip, importZip } from "@/services/archiveApi";
import { askDialog, openSingleDialog, saveDialog } from "@/services/dialogApi";
import { onTauriEvent } from "@/services/eventBus";
import { startSearchRebuildIndex } from "@/services/searchApi";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { startUpdateJob, waitForUpdateJob } from "@/features/updates/updateJobs";
import { cn } from "@/lib/utils";
import { store } from "../../store";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { type LibraryViewMode, type SearchRebuildProgress, type SearchV2Params } from "../../types/library";
import { LibraryFilterPanel } from "./LibraryFilterPanel";
import { LibrarySearchBox } from "./LibrarySearchBox";
import { LibraryWorkCard } from "./LibraryWorkCard";
import { LibraryWorkGrid } from "./LibraryWorkGrid";
import { EntityFacetGridCard } from "./EntityFacetGridCard";

type LibrarySurfaceViewMode = LibraryViewMode;

function normalizeLibraryViewMode(mode: string): LibrarySurfaceViewMode {
  if (mode === "gallery" || mode === "compact") {
    return mode as LibrarySurfaceViewMode;
  }
  if (mode === "epubSelection" || mode === "updateReview") {
    return "compact";
  }
  return "gallery";
}

function defaultViewModeForSurface(_surface: "library" | "epub" | "update"): LibrarySurfaceViewMode {
  return "gallery";
}

const viewModeOptions = [
  { id: "gallery", label: "ギャラリー" },
  { id: "compact", label: "コンパクト" }
];

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface DownloadEntry {
  id: number;
  source: string;
  sourceId: string;
  title: string;
  authorName: string;
  authorId: string;
  contentType: string;
  tags: string[];
  excerpt: string | null;
  coverPath: string | null;
  jsonPath: string;
  assetCount: number;
  fileSizeBytes: number;
  downloadedAt: string;
  sourceCreatedAt: string | null;
  sourceUpdatedAt: string | null;
  currentVersion: number;
  watchUpdates: boolean;
  textLength: number;
  favorite: boolean;
  personId?: string | null;
  personName?: string | null;
  seriesId?: string | null;
  seriesTitle?: string | null;
  searchScore?: number | null;
  matchFields?: string[];
  scoreReasons?: { field: string; matchType: string; term: string; contribution: number; detail?: string | null }[];
  matchHighlights?: { field: string; text?: string; sourceChunkId?: string | null; matchType?: string | null; segments: { text: string; matched: boolean }[] }[];
}

interface DbStats {
  totalDownloads: number;
  pixivCount: number;
  fanboxCount: number;
  totalAssets: number;
  totalSizeBytes: number;
}

interface FacetCount {
  name: string;
  count: number;
}

interface EntityFacet {
  source: string;
  sourceKey: string;
  displayName: string;
  count: number;
  coverPath: string | null;
  description?: string | null;
  updatedAt?: string | null;
  latestDownloadedAt?: string | null;
  sampleTitle?: string | null;
}

interface FilterFacets {
  tags: FacetCount[];
  authors: FacetCount[];
  authorEntities: EntityFacet[];
  series: EntityFacet[];
  contentTypes: FacetCount[];
  assetTypes: FacetCount[];
}

type FilterModeValue = "include" | "exclude";
type AssetFilter = "all" | "no_assets" | "has_images" | "has_files" | "has_images_and_files" | "has_assets";
type WatchFilter = "all" | "watched" | "unwatched";
type SearchMode = "smart" | "exact" | "semantic";

const LIBRARY_PAGE_SIZE = 120;
const LIBRARY_SELECT_PAGE_SIZE = 1000;

interface SearchIndexStatus {
  totalDownloads: number;
  indexedDownloads: number;
  pendingDownloads: number;
  isComplete: boolean;
  phase: string;
  indexedChunks: number;
  semanticIndexedChunks: number;
  semanticModelReady: boolean;
  embeddingProvider: string;
  gpuEnabled: boolean;
  throughputPerSec?: number | null;
}

interface BulkMutationResult {
  matchedCount: number;
  changedCount: number;
}

interface LibraryDataCacheEntry {
  downloads: DownloadEntry[];
  stats: DbStats | null;
  coverCache: Record<number, string>;
  hasMore: boolean;
  nextCursor: string | null;
}

const libraryDataCache = new Map<string, LibraryDataCacheEntry>();

export interface LibraryUiState {
  searchVal: string;
  searchMode?: SearchMode;
  sourceFilter: string;
  sortBy: string;
  sortOrder: "asc" | "desc";
  tagFilters: Record<string, FilterModeValue>;
  authorFilters: Record<string, FilterModeValue>;
  filterMode: "and" | "or";
  minCharCount: number | "";
  maxCharCount: number | "";
  assetFilter: AssetFilter;
  watchFilter: WatchFilter;
  showFilters: boolean;
  scrollTop: number;
  viewMode?: string;
}

interface Props {
  mode?: "library" | "epub" | "update";
  onOpenLibraryMode?: (mode: "library" | "epub" | "update") => void;
  onViewDetail: (id: number) => void;
  onViewPerson?: (source: string, sourceKey: string) => void;
  onViewSeries?: (source: string, sourceKey: string) => void;
  showToast: (msg: string, type: "success" | "error" | "info") => void;
  initialFilter?: string;
  epubSelectedIds?: Set<number>;
  onEpubSelectionChange?: (ids: Set<number>, items: DownloadEntry[]) => void;
  onToggleEpubSidebar?: () => void;
  epubSidebarOpen?: boolean;
  onOpenTemplateManager?: () => void;
  updateSidebarOpen?: boolean;
  onToggleUpdateSidebar?: () => void;
  onUpdateWorkTargetChange?: () => void;
  initialTagFilters?: Record<string, "include" | "exclude">;
  initialAuthorFilters?: Record<string, "include" | "exclude">;
  initialState?: Partial<LibraryUiState>;
  restoreKey?: number;
  onUiStateChange?: (state: LibraryUiState) => void;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatDate(iso: string): string {
  try {
    return new Date(iso).toLocaleDateString("ja-JP", { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  } catch { return iso; }
}

function ToolbarButton({
  active,
  danger,
  className,
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  danger?: boolean;
  children: ReactNode;
}) {
  return (
    <Button
      type="button"
      variant={danger ? "destructive" : active ? "secondary" : "outline"}
      size="sm"
      data-active={active}
      className={cn(
        "h-8 rounded-md px-3 shadow-sm data-[active=true]:border-primary/30 data-[active=true]:bg-primary/10 data-[active=true]:text-primary",
        className,
      )}
      {...props}
    >
      {children}
    </Button>
  );
}

function FilterChip({
  active,
  mode,
  className,
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  mode?: FilterModeValue;
  children: ReactNode;
}) {
  return (
    <Button
      type="button"
      variant={active || mode ? "secondary" : "outline"}
      size="sm"
      data-active={active || Boolean(mode)}
      data-mode={mode}
      className={cn(
        "quick-filter-chip inline-flex min-h-7 items-center gap-1 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors hover:bg-accent hover:text-accent-foreground data-[active=true]:border-primary/30 data-[active=true]:bg-primary/10 data-[active=true]:text-primary data-[mode=exclude]:border-destructive/30 data-[mode=exclude]:bg-destructive/10 data-[mode=exclude]:text-destructive",
        className,
      )}
      {...props}
    >
      {children}
    </Button>
  );
}

export function LibraryView({ mode = "library", onOpenLibraryMode, onViewDetail, onViewPerson, onViewSeries, showToast, initialFilter = "", epubSelectedIds, onEpubSelectionChange, onToggleEpubSidebar, epubSidebarOpen, onOpenTemplateManager, updateSidebarOpen, onToggleUpdateSidebar, onUpdateWorkTargetChange, initialTagFilters, initialAuthorFilters, initialState, restoreKey = 0, onUiStateChange }: Props) {
  const tauriAvailable = isTauriRuntime();
  const isEpubMode = mode === "epub";
  const isUpdatePageMode = mode === "update";
  const [downloads, setDownloads] = useState<DownloadEntry[]>([]);
  const [stats, setStats] = useState<DbStats | null>(null);
  const [facets, setFacets] = useState<FilterFacets>({ tags: [], authors: [], authorEntities: [], series: [], contentTypes: [], assetTypes: [] });
  const [searchVal, setSearchVal] = useState(initialState?.searchVal ?? "");
  const [query, setQuery] = useState(initialState?.searchVal ?? "");
  const [searchMode, setSearchMode] = useState<SearchMode>(initialState?.searchMode ?? "smart");
  const [libraryViewMode, setLibraryViewMode] = useState<LibrarySurfaceViewMode>(
    initialState?.viewMode ? normalizeLibraryViewMode(initialState.viewMode) : defaultViewModeForSurface(mode),
  );
  const [librarySubTab, setLibrarySubTab] = useState<"works" | "authors" | "series">("works");

  useEffect(() => {
    store.set("library.viewPrefs", { viewMode: libraryViewMode }).catch(() => undefined);
  }, [libraryViewMode]);

  useEffect(() => {
    const handler = setTimeout(() => {
      setQuery(searchVal);
    }, 300); // 300ms デバウンス
    return () => clearTimeout(handler);
  }, [searchVal]);

  const [sourceFilter, setSourceFilter] = useState(initialState?.sourceFilter ?? (initialFilter === "all" ? "" : initialFilter));
  const [sortBy, setSortBy] = useState(initialState?.sortBy ?? "published");
  const [sortTouched, setSortTouched] = useState(Boolean(initialState?.sortBy && initialState.sortBy !== "published"));
  const [sortOrder, setSortOrder] = useState<"asc" | "desc">(initialState?.sortOrder ?? "desc");

  useEffect(() => {
    if (sortTouched) return;
    if (query.trim() && sortBy !== "relevance") {
      setSortBy("relevance");
    } else if (!query.trim() && sortBy === "relevance") {
      setSortBy("published");
    }
  }, [query, sortBy, sortTouched]);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMoreDownloads, setHasMoreDownloads] = useState(false);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [coverCache, setCoverCache] = useState<Record<number, string>>({});
  const [isUpdating, setIsUpdating] = useState(false);
  const [tagFilters, setTagFilters] = useState<Record<string, FilterModeValue>>(initialState?.tagFilters ?? {});
  const [authorFilters, setAuthorFilters] = useState<Record<string, FilterModeValue>>(initialState?.authorFilters ?? {});
  const [filterMode, setFilterMode] = useState<"and" | "or">(initialState?.filterMode ?? "and");
  const [minCharCount, setMinCharCount] = useState<number | "">(initialState?.minCharCount ?? "");
  const [maxCharCount, setMaxCharCount] = useState<number | "">(initialState?.maxCharCount ?? "");
  const [assetFilter, setAssetFilter] = useState<AssetFilter>(initialState?.assetFilter ?? "all");
  const [watchFilter, setWatchFilter] = useState<WatchFilter>(initialState?.watchFilter ?? "all");
  const [isDeleteMode, setIsDeleteMode] = useState(false);
  const [selectedDownloadIds, setSelectedDownloadIds] = useState<number[]>([]);
  const [allMatchingSelected, setAllMatchingSelected] = useState(false);
  const [isUpdateMode, setIsUpdateMode] = useState(false);
  const [showFilters, setShowFilters] = useState(initialState?.showFilters ?? false);
  const [showBackupDropdown, setShowBackupDropdown] = useState(false);
  const backupRef = useRef<HTMLDivElement>(null);
  const filterPanelRef = useRef<HTMLDivElement>(null);
  const filterToggleRef = useRef<HTMLButtonElement>(null);
  const downloadsRef = useRef<DownloadEntry[]>([]);
  const [showScrollTop, setShowScrollTop] = useState(false);
  const [tagCandidateQuery, setTagCandidateQuery] = useState("");
  const [authorCandidateQuery, setAuthorCandidateQuery] = useState("");
  const [facetSearchResults, setFacetSearchResults] = useState<{ tags: FacetCount[]; authors: FacetCount[] }>({ tags: [], authors: [] });
  const [searchIndexStatus, setSearchIndexStatus] = useState<SearchIndexStatus | null>(null);
  const lastRestoreKeyRef = useRef(-1);
  const fetchSeqRef = useRef(0);

  useEffect(() => {
    downloadsRef.current = downloads;
  }, [downloads]);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (backupRef.current && !backupRef.current.contains(event.target as Node)) {
        setShowBackupDropdown(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
    };
  }, []);

  useEffect(() => {
    if (!showFilters) return;

    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      if (filterPanelRef.current?.contains(target) || filterToggleRef.current?.contains(target)) {
        return;
      }
      setShowFilters(false);
    }

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [showFilters]);

  useEffect(() => {
    const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
    const container = document.querySelector(selector);
    if (!container) return;

    const handleScroll = () => {
      if (container.scrollTop > 0) {
        setShowScrollTop(true);
      } else {
        setShowScrollTop(false);
      }
    };

    container.addEventListener("scroll", handleScroll);
    handleScroll(); // 初回チェック

    return () => {
      container.removeEventListener("scroll", handleScroll);
    };
  }, [mode]);

  const scrollToTop = () => {
    const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
    const container = document.querySelector(selector);
    if (container) {
      container.scrollTo({ top: 0, behavior: "smooth" });
    }
  };


  const isFilterActive = useMemo(() => {
    return (
      query.trim().length > 0 ||
      Object.keys(tagFilters).length > 0 ||
      Object.keys(authorFilters).length > 0 ||
      minCharCount !== "" ||
      maxCharCount !== "" ||
      assetFilter !== "all" ||
      watchFilter !== "all" ||
      sourceFilter !== ""
    );
  }, [query, tagFilters, authorFilters, minCharCount, maxCharCount, assetFilter, watchFilter, sourceFilter]);

  const getCurrentUiState = useCallback((scrollTop?: number): LibraryUiState => {
    const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
    const container = document.querySelector(selector);
    return {
      searchVal,
      searchMode,
      sourceFilter,
      sortBy,
      sortOrder,
      tagFilters,
      authorFilters,
      filterMode,
      minCharCount,
      maxCharCount,
      assetFilter,
      watchFilter,
      showFilters,
      scrollTop: scrollTop ?? container?.scrollTop ?? initialState?.scrollTop ?? 0,
      viewMode: libraryViewMode,
    };
  }, [mode, searchVal, searchMode, sourceFilter, sortBy, sortOrder, tagFilters, authorFilters, filterMode, minCharCount, maxCharCount, assetFilter, watchFilter, showFilters, initialState?.scrollTop, libraryViewMode]);

  useEffect(() => {
    onUiStateChange?.(getCurrentUiState());
  }, [getCurrentUiState, onUiStateChange]);

  useEffect(() => {
    if (!onUiStateChange) return;
    const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
    const container = document.querySelector(selector);
    if (!container) return;
    const handleScrollState = () => onUiStateChange(getCurrentUiState(container.scrollTop));
    container.addEventListener("scroll", handleScrollState);
    return () => container.removeEventListener("scroll", handleScrollState);
  }, [mode, getCurrentUiState, onUiStateChange]);

  useEffect(() => {
    if (!initialState || restoreKey === lastRestoreKeyRef.current) return;
    lastRestoreKeyRef.current = restoreKey;

    setSearchVal(initialState.searchVal ?? "");
    setQuery(initialState.searchVal ?? "");
    setSearchMode(initialState.searchMode ?? "smart");
    setSourceFilter(initialState.sourceFilter ?? (initialFilter === "all" ? "" : initialFilter));
    setSortBy(initialState.sortBy ?? "published");
    setSortTouched(Boolean(initialState.sortBy && initialState.sortBy !== "published"));
    setSortOrder(initialState.sortOrder ?? "desc");
    setTagFilters(initialState.tagFilters ?? {});
    setAuthorFilters(initialState.authorFilters ?? {});
    setFilterMode(initialState.filterMode ?? "and");
    setMinCharCount(initialState.minCharCount ?? "");
    setMaxCharCount(initialState.maxCharCount ?? "");
    setAssetFilter(initialState.assetFilter ?? "all");
    setWatchFilter(initialState.watchFilter ?? "all");
    setShowFilters(initialState.showFilters ?? false);
    if (initialState.viewMode) setLibraryViewMode(normalizeLibraryViewMode(initialState.viewMode));

    window.setTimeout(() => {
      const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
      const container = document.querySelector(selector);
      if (container && typeof initialState.scrollTop === "number") {
        container.scrollTop = initialState.scrollTop;
      }
    }, 50);
  }, [initialState, initialFilter, mode, restoreKey]);

  useEffect(() => {
    if (initialState) return;
    setSourceFilter(initialFilter === "all" ? "" : initialFilter);
    // フィルターの初期状態クリア
    setTagFilters({});
    setAuthorFilters({});
    setMinCharCount("");
    setMaxCharCount("");
    setAssetFilter("all");
    setWatchFilter("all");
  }, [initialFilter, initialState]);

  useEffect(() => {
    const loadFacets = async () => {
      if (!isTauriRuntime()) return;
      try {
        const nextFacets = await getFilterFacets();
        setFacets(nextFacets);
      } catch (e) {
        console.error("Failed to load filter facets:", e);
      }
    };
    loadFacets();
  }, []);

  useEffect(() => {
    if (initialTagFilters) {
      setTagFilters(initialTagFilters);
    }
  }, [initialTagFilters]);

  useEffect(() => {
    if (initialAuthorFilters) {
      setAuthorFilters(initialAuthorFilters);
    }
  }, [initialAuthorFilters]);

  useEffect(() => {
    const tagQuery = tagCandidateQuery.trim();
    const authorQuery = authorCandidateQuery.trim();
    if (!tagQuery && !authorQuery) {
      setFacetSearchResults({ tags: [], authors: [] });
      return;
    }

    const handler = window.setTimeout(async () => {
      if (!isTauriRuntime()) {
        return;
      }
      try {
        const [tags, authors] = await Promise.all([
          tagQuery
            ? searchFilterFacets("tags", tagQuery, 30)
            : Promise.resolve([]),
          authorQuery
            ? searchFilterFacets("authors", authorQuery, 30)
            : Promise.resolve([]),
        ]);
        setFacetSearchResults({ tags, authors });
      } catch (e) {
        console.error("Facet search failed:", e);
      }
    }, 160);

    return () => window.clearTimeout(handler);
  }, [tagCandidateQuery, authorCandidateQuery]);

  const handleCheckUpdates = async () => {
    setIsUpdating(true);
    showToast("監視中の作品・著者・シリーズの更新ジョブを開始します...", "info");
    try {
      const started = await startUpdateJob({ scope: "all", mode: "auto_save" });
      showToast("更新ジョブを開始しました。完了までバックグラウンドで進行します。", "info");
      const completed = await waitForUpdateJob(started.jobId);

      if (completed.status === "completed") {
        showToast(`更新ジョブ完了: 保存 ${completed.savedCount} 件 / 新作候補 ${completed.candidateCount} 件`, "success");
        fetchData();
      } else if (completed.status === "auth_required") {
        showToast("認証情報が必要です。設定を確認して更新管理から再開してください。", "error");
      } else if (completed.status === "failed") {
        showToast(`更新ジョブで ${completed.errorCount} 件のエラーが発生しました。`, "error");
      } else if (completed.status === "paused") {
        showToast("更新ジョブは一時停止中です。更新管理から再開できます。", "info");
      }
    } catch (e: any) {
      showToast(`更新チェックエラー: ${e}`, "error");
    } finally {
      setIsUpdating(false);
    }
  };

  const effectiveSortBy = useMemo(() => {
    if (query.trim() && !sortTouched) return "relevance";
    if (!query.trim() && sortBy === "relevance") return "published";
    return sortBy;
  }, [query, sortBy, sortTouched]);

  const filterCacheKey = useMemo(() => JSON.stringify({
    mode,
    sourceFilter,
    sortBy: effectiveSortBy,
    sortOrder,
    query: query.trim(),
    searchMode,
    tagFilters,
    authorFilters,
    filterMode,
    minCharCount,
    maxCharCount,
    assetFilter,
    watchFilter,
    libraryViewMode,
    librarySubTab,
  }), [mode, sourceFilter, effectiveSortBy, sortOrder, query, searchMode, tagFilters, authorFilters, filterMode, minCharCount, maxCharCount, assetFilter, watchFilter, libraryViewMode, librarySubTab]);

  const buildSearchV2Params = useCallback((limit: number, cursor: string | null = null): SearchV2Params => {
    const isFavorite = sourceFilter === "favorite";
    const actualSource = isFavorite ? null : (sourceFilter || null);

    const dbMinChar: number | null = minCharCount !== "" ? Number(minCharCount) : null;
    const dbMaxChar: number | null = maxCharCount !== "" ? Number(maxCharCount) : null;
    const dbQuery = query.trim() || null;

    const tagEntries = Object.entries(tagFilters);
    const tagsInc = tagEntries.filter(([_, mode]) => mode === "include").map(([t]) => t);
    const tagsExc = tagEntries.filter(([_, mode]) => mode === "exclude").map(([t]) => t);

    const authorEntries = Object.entries(authorFilters);
    const authorsInc = authorEntries.filter(([_, mode]) => mode === "include").map(([a]) => a);
    const authorsExc = authorEntries.filter(([_, mode]) => mode === "exclude").map(([a]) => a);

    return {
      query: dbQuery,
      source: actualSource,
      contentType: null,
      sortBy: effectiveSortBy,
      sortOrder,
      limit,
      cursor,
      favorite: isFavorite ? true : null,
      tagsInclude: tagsInc.length > 0 ? tagsInc : null,
      tagsExclude: tagsExc.length > 0 ? tagsExc : null,
      tagFilterMode: filterMode,
      authorsInclude: authorsInc.length > 0 ? authorsInc : null,
      authorsExclude: authorsExc.length > 0 ? authorsExc : null,
      minCharCount: dbMinChar,
      maxCharCount: dbMaxChar,
      assetFilter: assetFilter !== "all" ? assetFilter : null,
      watchFilter: watchFilter !== "all" ? watchFilter : null,
      viewMode: libraryViewMode,
      searchMode,
      projection: librarySubTab === "works"
        ? (libraryViewMode === "compact" ? "libraryCompact" : "libraryGallery")
        : "entityFacet",
    };
  }, [sourceFilter, effectiveSortBy, sortOrder, query, searchMode, tagFilters, authorFilters, filterMode, minCharCount, maxCharCount, assetFilter, watchFilter, libraryViewMode, librarySubTab]);

  const fetchDataPage = useCallback(async (cursor: string | null = null, options?: { replace?: boolean; preferCache?: boolean }) => {
    const replace = options?.replace ?? cursor === null;
    const preferCache = options?.preferCache ?? replace;
    const seq = ++fetchSeqRef.current;

    if (librarySubTab !== "works") {
      setHasMoreDownloads(false);
      setNextCursor(null);
      setLoading(false);
      setLoadingMore(false);
      if (!isTauriRuntime()) {
        setStats({
          totalDownloads: 0,
          pixivCount: 0,
          fanboxCount: 0,
          totalAssets: 0,
          totalSizeBytes: 0,
        });
        return;
      }
      try {
        const dbStats = await getStats() as DbStats;
        if (seq === fetchSeqRef.current) {
          setStats(dbStats);
        }
      } catch (e) {
        console.error("Library stats fetch error:", e);
      }
      return;
    }

    if (replace && preferCache) {
      const cached = libraryDataCache.get(filterCacheKey);
      if (cached) {
        setDownloads(cached.downloads);
        setStats(cached.stats);
        setCoverCache(cached.coverCache);
        setHasMoreDownloads(cached.hasMore);
        setNextCursor(cached.nextCursor);
      } else {
        setLoading(true);
      }
    } else if (replace) {
      setLoading(downloadsRef.current.length === 0);
    } else {
      setLoadingMore(true);
    }

    if (!isTauriRuntime()) {
      setDownloads([]);
      setStats({
        totalDownloads: 0,
        pixivCount: 0,
        fanboxCount: 0,
        totalAssets: 0,
        totalSizeBytes: 0,
      });
      setHasMoreDownloads(false);
      setNextCursor(null);
      setLoading(false);
      setLoadingMore(false);
      return;
    }

    try {
      const [result, dbStats] = await Promise.all([
        searchDownloadsV2(buildSearchV2Params(LIBRARY_PAGE_SIZE, cursor)) as Promise<{ items: DownloadEntry[]; nextCursor: string | null }>,
        getStats() as Promise<DbStats>,
      ]);

      if (seq !== fetchSeqRef.current) return;

      const page = result.items;
      const nextHasMore = Boolean(result.nextCursor);
      const nextDownloads = replace ? page : [...downloadsRef.current, ...page];
      setDownloads(nextDownloads);
      setStats(dbStats);
      setHasMoreDownloads(nextHasMore);
      setNextCursor(result.nextCursor);
      setLoading(false);

      const existingCache = libraryDataCache.get(filterCacheKey);
      libraryDataCache.set(filterCacheKey, {
        downloads: nextDownloads,
        stats: dbStats,
        coverCache: existingCache?.coverCache ?? {},
        hasMore: nextHasMore,
        nextCursor: result.nextCursor,
      });
    } catch (e) {
      console.error("Library fetch error:", e);
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }, [filterCacheKey, buildSearchV2Params, librarySubTab]);

  const fetchData = useCallback(() => {
    fetchDataPage(null, { replace: true, preferCache: true });
  }, [fetchDataPage]);

  useEffect(() => { fetchData(); }, [fetchData]);

  const getCoverSource = useCallback((dl: DownloadEntry): string | null => {
    return coverCache[dl.id] ?? getAssetUrl(dl.coverPath);
  }, [coverCache]);

  const handleImageError = useCallback(async (dl: DownloadEntry) => {
    if (!dl.coverPath) return;
    setCoverCache(prev => {
      const merged = { ...prev, [dl.id]: "" };
      const current = libraryDataCache.get(filterCacheKey);
      if (current) {
        libraryDataCache.set(filterCacheKey, { ...current, coverCache: merged });
      }
      return merged;
    });
  }, [filterCacheKey]);

  useEffect(() => {
    if (!tauriAvailable) return;
    let cancelled = false;
    let activeJobId: string | null = null;
    let unlisten: (() => void) | undefined;

    const rebuild = async () => {
      try {
        const status = await getSearchIndexStatus() as SearchIndexStatus;
        if (cancelled) return;
        setSearchIndexStatus(status);
        if (status.pendingDownloads > 0) {
          unlisten = await onTauriEvent<SearchRebuildProgress>("search-index-progress", event => {
            if (cancelled || (activeJobId && event.payload.jobId !== activeJobId)) return;
            if (event.payload.status === "failed") {
              console.error("Search index rebuild failed:", event.payload.error);
              return;
            }
            const nextStatus: SearchIndexStatus = {
              totalDownloads: event.payload.totalDownloads,
              indexedDownloads: event.payload.indexedDownloads,
              pendingDownloads: event.payload.pendingDownloads,
              isComplete: event.payload.isComplete,
              phase: event.payload.phase ?? (event.payload.isComplete ? "ready" : "indexing"),
              indexedChunks: event.payload.indexedChunks ?? status.indexedChunks ?? 0,
              semanticIndexedChunks: event.payload.indexedChunks ?? status.semanticIndexedChunks ?? 0,
              semanticModelReady: status.semanticModelReady,
              embeddingProvider: event.payload.embeddingProvider ?? status.embeddingProvider ?? "fastembed",
              gpuEnabled: event.payload.gpuEnabled ?? status.gpuEnabled ?? false,
              throughputPerSec: event.payload.throughputPerSec ?? null,
            };
            setSearchIndexStatus(nextStatus);
            if (event.payload.isComplete) {
              libraryDataCache.clear();
              fetchDataPage(null, { replace: true, preferCache: false });
            }
          });
          activeJobId = await startSearchRebuildIndex(128);
        }
      } catch (e) {
        console.error("Search index rebuild failed:", e);
      }
    };

    rebuild();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [tauriAvailable, fetchDataPage]);

  useEffect(() => {
    const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
    const container = document.querySelector(selector);
    if (!container) return;

    const handleLoadMore = () => {
      const remaining = container.scrollHeight - container.scrollTop - container.clientHeight;
      if (remaining < 900 && hasMoreDownloads && nextCursor && !loading && !loadingMore) {
        fetchDataPage(nextCursor, { replace: false, preferCache: false });
      }
    };

    container.addEventListener("scroll", handleLoadMore);
    handleLoadMore();
    return () => container.removeEventListener("scroll", handleLoadMore);
  }, [mode, hasMoreDownloads, nextCursor, loading, loadingMore, fetchDataPage]);

  // すべての検索・絞り込みはデータベース（SQL）側に完全移譲したため、downloadsをそのまま返却
  const filteredDownloads = useMemo<DownloadEntry[]>(() => {
    return downloads;
  }, [downloads]);

  const filteredAuthors = useMemo(() => {
    const query = searchVal.trim().toLowerCase();
    if (!query) return facets.authorEntities;
    return facets.authorEntities.filter(author =>
      author.displayName.toLowerCase().includes(query) ||
      author.sourceKey.toLowerCase().includes(query)
    );
  }, [facets.authorEntities, searchVal]);

  const filteredSeriesList = useMemo(() => {
    const query = searchVal.trim().toLowerCase();
    if (!query) return facets.series;
    return facets.series.filter(series =>
      series.displayName.toLowerCase().includes(query) ||
      series.sourceKey.toLowerCase().includes(query)
    );
  }, [facets.series, searchVal]);

  const fetchAllMatchingDownloads = useCallback(async (): Promise<DownloadEntry[]> => {
    if (!isTauriRuntime()) return filteredDownloads;

    const all: DownloadEntry[] = [];
    let cursor: string | null = null;

    while (true) {
      const result = await searchDownloadsV2({ ...buildSearchV2Params(LIBRARY_SELECT_PAGE_SIZE, cursor), projection: "bulk" }) as { items: DownloadEntry[]; nextCursor: string | null };
      const page = result.items;

      all.push(...page);
      if (!result.nextCursor || page.length === 0) break;
      cursor = result.nextCursor;
    }

    return all;
  }, [buildSearchV2Params, filteredDownloads]);

  const handleToggleTagFilter = useCallback((tag: string) => {
    setTagFilters(prev => {
      const next = { ...prev };
      const current = prev[tag];
      if (!current) {
        next[tag] = "include";
      } else if (current === "include") {
        next[tag] = "exclude";
      } else {
        delete next[tag];
      }
      return next;
    });
  }, []);

  const handleToggleAuthorFilter = useCallback((author: string) => {
    setAuthorFilters(prev => {
      const next = { ...prev };
      const current = prev[author];
      if (!current) {
        next[author] = "include";
      } else if (current === "include") {
        next[author] = "exclude";
      } else {
        delete next[author];
      }
      return next;
    });
  }, []);

  const toggleSelectDownload = useCallback((id: number) => {
    setSelectedDownloadIds(prev => 
      prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]
    );
    setAllMatchingSelected(false);
  }, []);

  const isAllSelected = useMemo(() => {
    if (allMatchingSelected) return true;
    if (filteredDownloads.length === 0) return false;
    return filteredDownloads.every(dl => selectedDownloadIds.includes(dl.id));
  }, [allMatchingSelected, filteredDownloads, selectedDownloadIds]);

  const selectionCountLabel = useMemo(() => {
    if (allMatchingSelected) return "条件一致すべて";
    return selectedDownloadIds.length.toString();
  }, [allMatchingSelected, selectedDownloadIds]);

  const handleToggleSelectAll = useCallback(async () => {
    if (isAllSelected && !allMatchingSelected) {
      setSelectedDownloadIds([]);
      setAllMatchingSelected(false);
      showToast("選択を解除しました", "success");
    } else if (isAllSelected && allMatchingSelected) {
      setSelectedDownloadIds([]);
      setAllMatchingSelected(false);
      showToast("条件一致すべての選択を解除しました", "success");
    } else {
      showToast("現在の絞り込み条件に一致する作品を確認しています...", "info");
      const allMatching = await fetchAllMatchingDownloads();
      const newIds = allMatching.map(dl => dl.id);
      setSelectedDownloadIds(newIds);
      setAllMatchingSelected(true);
      showToast(`${allMatching.length} 件を選択しました`, "success");
    }
  }, [isAllSelected, allMatchingSelected, fetchAllMatchingDownloads, showToast]);

  const handleSetWatchAllFiltered = async (watch: boolean) => {
    if (filteredDownloads.length === 0) return;
    try {
      showToast(watch ? "絞り込み条件に一致する作品をすべて更新ONに設定中..." : "絞り込み条件に一致する作品をすべて更新OFFに設定中...", "info");
      const result = await setWatchUpdatesForSearch({ ...buildSearchV2Params(LIBRARY_SELECT_PAGE_SIZE, null), projection: "bulk" }, watch) as BulkMutationResult;
      setDownloads(prev => prev.map(dl => ({ ...dl, watchUpdates: watch })));
      libraryDataCache.clear();
      fetchDataPage(null, { replace: true, preferCache: false });
      const count = result.changedCount || result.matchedCount;
      showToast(watch ? `${count} 件の作品を更新ONにしました` : `${count} 件の作品を更新OFFにしました`, "success");
    } catch (e: any) {
      showToast(`一括設定エラー: ${e}`, "error");
    }
  };

  const handleToggleWatchCard = async (id: number, currentWatch: boolean) => {
    try {
      const nextWatch = !currentWatch;
      await setWatchUpdates(id, nextWatch);
      setDownloads(prev => prev.map(dl => dl.id === id ? { ...dl, watchUpdates: nextWatch } : dl));
      showToast(nextWatch ? "更新監視を有効にしました" : "更新監視を解除しました", "success");
    } catch (e: any) {
      showToast(`監視設定エラー: ${e}`, "error");
    }
  };

  const handleToggleFavorite = async (id: number, currentFav: boolean) => {
    try {
      const nextFav = !currentFav;
      await setFavorite(id, nextFav);
      setDownloads(prev => prev.map(dl => dl.id === id ? { ...dl, favorite: nextFav } : dl));
      showToast(nextFav ? "お気に入りに追加しました" : "お気に入りから削除しました", "success");
      
      if (sourceFilter === "favorite" && !nextFav) {
        setDownloads(prev => prev.filter(dl => dl.id !== id));
      }
    } catch (e: any) {
      showToast(`お気に入り設定エラー: ${e}`, "error");
    }
  };

  const handleDeleteSelected = async () => {
    const ids = allMatchingSelected ? (await fetchAllMatchingDownloads()).map(d => d.id) : selectedDownloadIds;
    if (ids.length === 0) {
      setIsDeleteMode(false);
      return;
    }

    const isConfirmed = await askDialog(
      `選択された ${ids.length} 件の作品を完全に削除してもよろしいですか？\nアセットファイルや関連データも完全に消去されます。`,
      { title: "一括削除の確認", kind: "warning", okLabel: "削除する", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;

    try {
      showToast("一括削除を実行中...", "info");
      const result = await deleteDownloads(ids) as BulkMutationResult;
      showToast(`${result.changedCount} 件の作品を完全に削除しました`, "success");
      setSelectedDownloadIds([]);
      setAllMatchingSelected(false);
      setIsDeleteMode(false);
      libraryDataCache.clear();
      fetchData();
    } catch (e: any) { 
      showToast(`一括削除エラー: ${e}`, "error"); 
    }
  };

  const handleExportZip = async () => {
    try {
      const path = await saveDialog({ filters: [{ name: "ZIP", extensions: ["zip"] }], defaultPath: "piep_backup.zip" });
      if (!path) return;
      showToast("バックアップを作成中...", "info");
      await exportAllZip(path);
      showToast("バックアップの作成が完了しました！", "success");
    } catch (e: any) { showToast(`バックアップ作成エラー: ${e}`, "error"); }
  };

  const handleImportZip = async () => {
    try {
      const file = await openSingleDialog({ filters: [{ name: "ZIP", extensions: ["zip"] }] });
      if (!file) return;
      showToast("バックアップから復元中...", "info");
      const count = await importZip(file);
      showToast(`${count} 件の作品を完全に復元しました！`, "success");
      fetchData();
    } catch (e: any) { showToast(`復元エラー: ${e}`, "error"); }
  };

  const selectedTagNames = Object.keys(tagFilters);
  const selectedTagSet = new Set(selectedTagNames);
  const normalizedTagQuery = tagCandidateQuery.trim().toLowerCase();
  const popularTags = facets.tags
    .filter(f => !selectedTagSet.has(f.name))
    .slice(0, 10);
  const searchedTags = facets.tags
    .filter(f => !selectedTagSet.has(f.name))
    .filter(f => normalizedTagQuery.length > 0 && f.name.toLowerCase().includes(normalizedTagQuery))
    .slice(0, 30);
  const smartSearchedTags = normalizedTagQuery
    ? (tauriAvailable ? facetSearchResults.tags : searchedTags).filter(f => !selectedTagSet.has(f.name))
    : [];

  // よく読む著者上位5名を動的に抽出
  const selectedAuthorNames = Object.keys(authorFilters);
  const selectedAuthorSet = new Set(selectedAuthorNames);
  const normalizedAuthorQuery = authorCandidateQuery.trim().toLowerCase();
  const popularAuthors = facets.authors
    .filter(f => !selectedAuthorSet.has(f.name))
    .slice(0, 10);
  const searchedAuthors = facets.authors
    .filter(f => !selectedAuthorSet.has(f.name))
    .filter(f => normalizedAuthorQuery.length > 0 && f.name.toLowerCase().includes(normalizedAuthorQuery))
    .slice(0, 30);
  const smartSearchedAuthors = normalizedAuthorQuery
    ? (tauriAvailable ? facetSearchResults.authors : searchedAuthors).filter(f => !selectedAuthorSet.has(f.name))
    : [];

  const assetFilterLabels: Record<AssetFilter, string> = {
    all: "すべて",
    no_assets: "テキストのみ",
    has_images: "画像あり",
    has_files: "その他ファイルあり",
    has_images_and_files: "画像+その他あり",
    has_assets: "何らかのアセットあり",
  };

  const sourceFilterLabels: Record<string, string> = {
    pixiv: "Pixiv",
    fanbox: "FANBOX",
    favorite: "お気に入り",
  };

  // --- EPUB Mode selection helpers ---
  const epubToggleSelect = useCallback((id: number) => {
    if (!onEpubSelectionChange || !epubSelectedIds) return;
    const next = new Set(epubSelectedIds);
    if (next.has(id)) next.delete(id); else next.add(id);
    const selectedItems = downloads.filter(d => next.has(d.id));
    onEpubSelectionChange(next, selectedItems);
  }, [epubSelectedIds, onEpubSelectionChange, downloads]);

  const epubSelectAll = useCallback(async () => {
    if (!onEpubSelectionChange) return;
    showToast("現在の絞り込み条件に一致する作品をすべて選択しています...", "info");
    const allMatching = await fetchAllMatchingDownloads();
    const ids = new Set(allMatching.map(d => d.id));
    onEpubSelectionChange(ids, allMatching);
    showToast(`${allMatching.length} 件を選択しました`, "success");
  }, [fetchAllMatchingDownloads, onEpubSelectionChange, showToast]);

  const epubSelectNone = useCallback(() => {
    if (!onEpubSelectionChange) return;
    onEpubSelectionChange(new Set(), []);
  }, [onEpubSelectionChange]);

  const epubIsAllSelected = useMemo(() => {
    if (!epubSelectedIds || filteredDownloads.length === 0) return false;
    return filteredDownloads.every(dl => epubSelectedIds.has(dl.id));
  }, [filteredDownloads, epubSelectedIds]);

  const updateIsAllVisibleWatched = useMemo(() => {
    return filteredDownloads.length > 0 && filteredDownloads.every(dl => dl.watchUpdates);
  }, [filteredDownloads]);

  const hasVisibleDownloads = filteredDownloads.length > 0;
  const hasAnyDownloads = (stats?.totalDownloads ?? 0) > 0;
  const workflowTabs = [
    { id: "library" as const, label: "一覧", detail: "探す・読む", icon: LibraryIcon },
    { id: "epub" as const, label: "EPUB", detail: `${epubSelectedIds?.size ?? 0} 件選択`, icon: BookIcon },
    { id: "update" as const, label: "更新", detail: "監視・新着", icon: SyncIcon },
  ];

  return (
    <>
      <div className="library-view">
      {/* Sticky Header Wrapper */}
      <div className={`library-header-sticky ${isEpubMode ? 'epub-active' : ''} ${isUpdatePageMode ? 'update-active' : ''} ${isDeleteMode ? 'delete-active' : ''} ${isUpdateMode ? 'update-active' : ''}`}>
        <div className="mb-2 flex items-center justify-between gap-3 rounded-lg border bg-card/80 p-1.5 shadow-sm backdrop-blur">
          {/* Left Column: Title label */}
          <div className="flex flex-1 items-center justify-start min-w-[76px]">
            <span className="text-[10px] font-semibold text-muted-foreground/50 px-2 tracking-wider uppercase hidden sm:inline-block">
              {isEpubMode ? "EPUB作成" : isUpdatePageMode ? "更新管理" : "作品一覧"}
            </span>
          </div>

          {/* Center Column: Workflow tabs ("一覧", "EPUB", "更新") */}
          <div className="flex items-center justify-center">
            <div className="flex gap-1 bg-muted/30 p-0.5 rounded-lg border border-border/30">
              {workflowTabs.map(tab => {
                const Icon = tab.icon;
                const active = mode === tab.id;
                return (
                  <Button
                    key={tab.id}
                    type="button"
                    variant={active ? "secondary" : "ghost"}
                    className={cn(
                      "h-8 justify-center gap-1.5 rounded-md px-4 py-1 text-xs transition-all",
                      active
                        ? "bg-primary/10 text-primary border border-primary/20 hover:bg-primary/15 font-bold"
                        : "text-muted-foreground hover:text-foreground hover:bg-muted/40",
                    )}
                    onClick={() => {
                      if (active) return;
                      onOpenLibraryMode?.(tab.id);
                    }}
                  >
                    <Icon className="h-3.5 w-3.5" />
                    <span className="font-semibold text-[12px]">{tab.label}</span>
                  </Button>
                );
              })}
            </div>
          </div>

          {/* Right Column: Gallery/Compact layout toggle */}
          <div className="flex flex-1 items-center justify-end min-w-[76px]">
            <Tabs value={libraryViewMode} onValueChange={value => setLibraryViewMode(value as LibraryViewMode)}>
            <TabsList className="h-8 p-0.5 bg-muted/40 border border-border/20 rounded-md">
              {viewModeOptions.map(option => {
                const Icon = option.id === "gallery" ? GalleryIcon : CompactIcon;
                return (
                  <TabsTrigger
                    key={option.id}
                    value={option.id}
                    className="h-7 w-8 p-0 rounded-sm"
                    title={option.label}
                    aria-label={option.label}
                  >
                    <Icon className="h-3.5 w-3.5" />
                  </TabsTrigger>
                );
              })}
            </TabsList>
            </Tabs>
          </div>
        </div>
        {/* Search & Filter toolbar */}
        <div className="library-toolbar">
          {/* Upper Row: Massive Full-width Search Bar */}
          <div className="toolbar-row search-row">
            <LibrarySearchBox
              value={searchVal}
              onChange={setSearchVal}
              searchMode={searchMode}
              onSearchModeChange={setSearchMode}
              searchIndexStatus={searchIndexStatus}
              tauriAvailable={tauriAvailable}
            />
          </div>

          {/* Lower Row: Filter dropdowns and Action Buttons */}
          <LibraryFilterPanel>
            <div className="filter-group">
              {librarySubTab === "works" && (
                <>
                  <Select value={sourceFilter || "__all__"} onValueChange={value => setSourceFilter(value === "__all__" ? "" : value)}>
                    <SelectTrigger className="w-auto min-w-32">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__all__">すべて</SelectItem>
                      <SelectItem value="pixiv">Pixiv</SelectItem>
                      <SelectItem value="fanbox">FANBOX</SelectItem>
                      <SelectItem value="favorite">お気に入り</SelectItem>
                    </SelectContent>
                  </Select>
                  <Select
                    value={sortBy}
                    onValueChange={value => {
                      setSortBy(value);
                      setSortTouched(true);
                    }}
                  >
                    <SelectTrigger className="w-auto min-w-36">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="relevance">関連度順</SelectItem>
                      <SelectItem value="date">ダウンロード日順</SelectItem>
                      <SelectItem value="published">投稿日順</SelectItem>
                      <SelectItem value="title">タイトル順</SelectItem>
                      <SelectItem value="author">著者順</SelectItem>
                      <SelectItem value="size">サイズ順</SelectItem>
                    </SelectContent>
                  </Select>
                  <ToolbarButton
                    type="button"
                    className="sort-order-btn w-9 px-0"
                    onClick={() => setSortOrder(prev => prev === "asc" ? "desc" : "asc")}
                    title={sortOrder === "asc" ? "昇順（古い/小さい順）。クリックで降順に変更" : "降順（新しい/大きい順）。クリックで昇順に変更"}
                  >
                    <strong>{sortOrder === "asc" ? "↑" : "↓"}</strong>
                  </ToolbarButton>
                  <Button
                    type="button"
                    ref={filterToggleRef}
                    variant={isFilterActive || showFilters ? "secondary" : "outline"}
                    size="sm"
                    className={cn("filter-toggle-btn h-8 gap-1", showFilters && "expanded", isFilterActive && "active border-primary/30 bg-primary/10 text-primary")}
                    onClick={() => setShowFilters(prev => !prev)}
                    title={showFilters ? `フィルターパネルを閉じる${isFilterActive ? ' (現在フィルター適用中)' : ''}` : `フィルターパネルを開く${isFilterActive ? ' (現在フィルター適用中)' : ''}`}
                  >
                    <FunnelIcon />
                    <span className={`toggle-arrow ${showFilters ? 'up' : 'down'}`} />
                  </Button>
                </>
              )}

              {/* Sub-tabs Selection for Works / Authors / Series */}
              {librarySubTab === "works" && (
                <div className="h-5 w-[1px] bg-border/60 self-center mx-2.5" />
              )}
              <div className="flex gap-1 bg-muted/40 p-0.5 rounded-lg border border-border/20">
                <Button
                  type="button"
                  variant={librarySubTab === "works" ? "secondary" : "ghost"}
                  className={cn(
                    "h-7 rounded-md px-3 text-xs font-semibold transition-all",
                    librarySubTab === "works"
                      ? "bg-background shadow-sm text-primary border border-border/40 font-bold"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted/20"
                  )}
                  onClick={() => setLibrarySubTab("works")}
                >
                  作品 ({stats?.totalDownloads ?? 0})
                </Button>
                <Button
                  type="button"
                  variant={librarySubTab === "authors" ? "secondary" : "ghost"}
                  className={cn(
                    "h-7 rounded-md px-3 text-xs font-semibold transition-all",
                    librarySubTab === "authors"
                      ? "bg-background shadow-sm text-primary border border-border/40 font-bold"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted/20"
                  )}
                  onClick={() => setLibrarySubTab("authors")}
                >
                  作者 ({facets.authorEntities.length})
                </Button>
                <Button
                  type="button"
                  variant={librarySubTab === "series" ? "secondary" : "ghost"}
                  className={cn(
                    "h-7 rounded-md px-3 text-xs font-semibold transition-all",
                    librarySubTab === "series"
                      ? "bg-background shadow-sm text-primary border border-border/40 font-bold"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted/20"
                  )}
                  onClick={() => setLibrarySubTab("series")}
                >
                  シリーズ ({facets.series.length})
                </Button>
              </div>
            </div>

            {librarySubTab === "works" && (
              <div className="toolbar-actions">
                {isEpubMode ? (
                  /* EPUB Mode Actions */
                  <>
                    <ToolbarButton
                      active={epubIsAllSelected}
                      title={epubIsAllSelected ? "すべての選択を解除" : "現在の絞り込み条件に一致する作品をすべて選択"}
                      onClick={epubIsAllSelected ? epubSelectNone : epubSelectAll}
                      disabled={!hasVisibleDownloads}
                    >
                      {epubIsAllSelected ? <CheckSquaresIcon /> : <SquaresIcon />}
                    </ToolbarButton>
                    <ToolbarButton
                      title="テンプレート管理"
                      onClick={onOpenTemplateManager}
                      disabled={!tauriAvailable}
                    >
                      <TemplateIcon />
                    </ToolbarButton>
                    <ToolbarButton
                      active={epubSidebarOpen}
                      className="gap-1"
                      title={epubSidebarOpen ? "EPUBエクスポート設定を閉じる" : "EPUBエクスポート設定を開く"}
                      onClick={onToggleEpubSidebar}
                    >
                      <BookIcon /> EPUB
                    </ToolbarButton>
                  </>
                ) : isUpdatePageMode ? (
                  <>
                    <ToolbarButton
                      active={updateIsAllVisibleWatched}
                      title={updateIsAllVisibleWatched ? "表示中のすべての作品を更新OFFにする" : "表示中のすべての作品を更新ONにする"}
                      onClick={() => handleSetWatchAllFiltered(!updateIsAllVisibleWatched)}
                      disabled={!hasVisibleDownloads || !tauriAvailable}
                    >
                      {updateIsAllVisibleWatched ? <CheckSquaresIcon /> : <SquaresIcon />}
                    </ToolbarButton>
                    <ToolbarButton
                      className={isUpdating ? "updating" : ""}
                      title="監視中の作品の更新および著者・シリーズの新作をチェックしてダウンロード"
                      onClick={handleCheckUpdates}
                      disabled={isUpdating || !hasAnyDownloads || !tauriAvailable}
                    >
                      {isUpdating ? <div className="spinner-mini" /> : <RefreshIcon />}
                    </ToolbarButton>
                    <ToolbarButton
                      active={updateSidebarOpen}
                      className="update-confirm"
                      title={updateSidebarOpen ? "更新管理パネルを閉じる" : "更新管理パネルを開く"}
                      onClick={onToggleUpdateSidebar}
                    >
                      <PanelRightIcon /> 更新管理
                    </ToolbarButton>
                  </>
                ) : isDeleteMode ? (
                  <>
                    <div className="epub-selection-group delete-selection-group">
                      <span className={`epub-selection-badge ${!allMatchingSelected && selectedDownloadIds.length === 0 ? 'empty' : ''}`}>
                        {allMatchingSelected ? "条件一致すべて" : `${selectionCountLabel} 件選択`}
                      </span>
                      <ToolbarButton
                        active={isAllSelected}
                        title={isAllSelected ? "現在の絞り込み条件に一致する作品の選択を解除" : "現在の絞り込み条件に一致する作品をすべて選択"}
                        onClick={handleToggleSelectAll}
                        disabled={!hasVisibleDownloads}
                      >
                        {isAllSelected ? "全選択解除" : "全選択"}
                      </ToolbarButton>
                    </div>
                    <ToolbarButton
                      danger
                      className="danger-confirm"
                      title={allMatchingSelected || selectedDownloadIds.length > 0 ? "選択した作品を完全に削除" : "削除する作品を選択してください"} 
                      onClick={handleDeleteSelected}
                      disabled={!allMatchingSelected && selectedDownloadIds.length === 0}
                    >
                      <TrashIcon /> 削除確定 ({selectionCountLabel})
                    </ToolbarButton>
                    <ToolbarButton
                      title="削除選択をキャンセル" 
                      onClick={() => {
                        setIsDeleteMode(false);
                        setSelectedDownloadIds([]);
                        setAllMatchingSelected(false);
                      }}
                    >
                      キャンセル
                    </ToolbarButton>
                  </>
                ) : isUpdateMode ? (
                  <>
                    <ToolbarButton
                      className="update-confirm"
                      title="表示中のすべての作品を更新ONにする" 
                      onClick={() => handleSetWatchAllFiltered(true)}
                      disabled={!hasVisibleDownloads || !tauriAvailable}
                    >
                      <SyncIcon /> すべて更新ON
                    </ToolbarButton>
                    <ToolbarButton
                      className="update-unwatch"
                      title="表示中のすべての作品を更新OFFにする" 
                      onClick={() => handleSetWatchAllFiltered(false)}
                      disabled={!hasVisibleDownloads || !tauriAvailable}
                    >
                      <SyncIcon className="update-muted-icon" /> すべて更新OFF
                    </ToolbarButton>
                    <ToolbarButton
                      className={isUpdating ? "updating" : ""}
                      title="監視中の作品の更新および著者・シリーズの新作をチェックしてダウンロード" 
                      onClick={handleCheckUpdates}
                      disabled={isUpdating || !hasAnyDownloads || !tauriAvailable}
                    >
                      {isUpdating ? <div className="spinner-mini" /> : <RefreshIcon className={isUpdating ? "spinning" : ""} />}
                    </ToolbarButton>
                    <ToolbarButton
                      title="更新モードを終了" 
                      onClick={() => setIsUpdateMode(false)}
                    >
                      完了
                    </ToolbarButton>
                  </>
                ) : (
                  <>
                    <ToolbarButton
                      className={isUpdating ? "updating" : ""}
                      title="監視中の作品の更新および著者・シリーズの新作をチェックしてダウンロード"
                      onClick={handleCheckUpdates}
                      disabled={isUpdating || !hasAnyDownloads || !tauriAvailable}
                    >
                      <RefreshIcon className={isUpdating ? "spinning" : ""} />
                    </ToolbarButton>
                    <ToolbarButton
                      title="一括削除モードに入る" 
                      onClick={() => setIsDeleteMode(true)}
                      disabled={!hasVisibleDownloads}
                    >
                      <TrashIcon />
                    </ToolbarButton>
                    <div className="backup-dropdown-container" ref={backupRef}>
                      <ToolbarButton
                        active={showBackupDropdown}
                        className="backup-dropdown-btn"
                        title="バックアップの作成と復元" 
                        onClick={() => setShowBackupDropdown(prev => !prev)}
                        disabled={!tauriAvailable}
                      >
                        <ExportIcon />
                        <span className={`toggle-arrow ${showBackupDropdown ? 'up' : 'down'}`} />
                      </ToolbarButton>
                      {showBackupDropdown && (
                        <div className="backup-dropdown-menu">
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="dropdown-item" 
                            title="全データと設定をZIP形式でバックアップ" 
                            onClick={() => {
                              setShowBackupDropdown(false);
                              handleExportZip();
                            }}
                          >
                            <ExportIcon /> バックアップ作成
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="dropdown-item" 
                            title="バックアップZIPからデータと設定を復元" 
                            onClick={() => {
                              setShowBackupDropdown(false);
                              handleImportZip();
                            }}
                          >
                            <ArchiveIcon /> バックアップ復元
                          </Button>
                        </div>
                      )}
                    </div>
                  </>
                )}
              </div>
            )}
          </LibraryFilterPanel>
        </div>

        {/* Quick Filter Panel */}
        {showFilters && (
          <div
            ref={filterPanelRef}
            className="quick-filters-panel"
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) {
                setShowFilters(false);
              }
            }}
          >
            <div className="quick-filter-panel-header">
              <span className="quick-filter-panel-title">絞り込み</span>
              <div className="filter-mode-container">
                <Button
                  type="button"
                  variant={filterMode === "and" ? "secondary" : "ghost"}
                  size="sm"
                  className={cn("filter-mode-btn h-7 px-2", filterMode === "and" && "active")}
                  onClick={() => setFilterMode('and')}
                  title="選択したすべてのタグを含む作品を表示 (AND)"
                >
                  AND
                </Button>
                <Button
                  type="button"
                  variant={filterMode === "or" ? "secondary" : "ghost"}
                  size="sm"
                  className={cn("filter-mode-btn h-7 px-2", filterMode === "or" && "active")}
                  onClick={() => setFilterMode('or')}
                  title="選択したタグのいずれかを含む作品を表示 (OR)"
                >
                  OR
                </Button>
              </div>
            </div>
            {(query || sourceFilter || selectedTagNames.length > 0 || selectedAuthorNames.length > 0 || minCharCount !== "" || maxCharCount !== "" || assetFilter !== "all" || watchFilter !== "all") && (
              <div className="quick-filter-row active-filter-row">
                <span className="quick-filter-label">適用中</span>
                <div className="quick-filter-items active-filter-items">
                  {query && (
                    <FilterChip active className="pinned" onClick={() => { setSearchVal(""); setQuery(""); }}>
                      検索: {query} ×
                    </FilterChip>
                  )}
                  {sourceFilter && (
                    <FilterChip active className="pinned" onClick={() => setSourceFilter("")}>
                      {sourceFilterLabels[sourceFilter] || sourceFilter} ×
                    </FilterChip>
                  )}
                  {selectedTagNames.map(tag => {
                    const mode = tagFilters[tag];
                    return (
                      <FilterChip
                        key={`active-tag-${tag}`}
                        className="pinned"
                        mode={mode}
                        onClick={() => handleToggleTagFilter(tag)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === "include" ? "+ " : "- "}#{tag}
                      </FilterChip>
                    );
                  })}
                  {selectedAuthorNames.map(author => {
                    const mode = authorFilters[author];
                    return (
                      <FilterChip
                        key={`active-author-${author}`}
                        className="pinned"
                        mode={mode}
                        onClick={() => handleToggleAuthorFilter(author)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === "include" ? "+ " : "- "}{author}
                      </FilterChip>
                    );
                  })}
                  {(minCharCount !== "" || maxCharCount !== "") && (
                    <FilterChip active className="pinned" onClick={() => { setMinCharCount(""); setMaxCharCount(""); }}>
                      {minCharCount === "" ? "0" : minCharCount.toLocaleString()}〜{maxCharCount === "" ? "上限なし" : maxCharCount.toLocaleString()}字 ×
                    </FilterChip>
                  )}
                  {assetFilter !== "all" && (
                    <FilterChip active className="pinned" onClick={() => setAssetFilter("all")}>
                      アセット: {assetFilterLabels[assetFilter]} ×
                    </FilterChip>
                  )}
                  {watchFilter !== "all" && (
                    <FilterChip active className="pinned" onClick={() => setWatchFilter("all")}>
                      更新確認: {watchFilter === "watched" ? "ON" : "OFF"} ×
                    </FilterChip>
                  )}
                </div>
              </div>
            )}

            {/* メタ属性スマートフィルター - 小説文字数 */}
            <div className="quick-filter-row metadata-filters-row flex-wrap gap-2">
              <span className="quick-filter-label">小説文字数</span>
              <div className="quick-filter-items flex-wrap gap-1">
                <FilterChip
                  active={minCharCount === "" && maxCharCount === ""}
                  onClick={() => { setMinCharCount(""); setMaxCharCount(""); }}
                >
                  すべて
                </FilterChip>
                <FilterChip
                  active={minCharCount === "" && maxCharCount === 10000}
                  onClick={() => { setMinCharCount(""); setMaxCharCount(10000); }}
                  title="1万字未満の短編小説"
                >
                  1万字未満
                </FilterChip>
                <FilterChip
                  active={minCharCount === 10000 && maxCharCount === 30000}
                  onClick={() => { setMinCharCount(10000); setMaxCharCount(30000); }}
                  title="1万字以上、3万字未満の中短編"
                >
                  1万〜3万字
                </FilterChip>
                <FilterChip
                  active={minCharCount === 30000 && maxCharCount === 50000}
                  onClick={() => { setMinCharCount(30000); setMaxCharCount(50000); }}
                  title="3万字以上、5万字未満の中編小説"
                >
                  3万〜5万字
                </FilterChip>
                <FilterChip
                  active={minCharCount === 50000 && maxCharCount === 100000}
                  onClick={() => { setMinCharCount(50000); setMaxCharCount(100000); }}
                  title="5万字以上、10万字未満の長編小説"
                >
                  5万〜10万字
                </FilterChip>
                <FilterChip
                  active={minCharCount === 100000 && maxCharCount === ""}
                  onClick={() => { setMinCharCount(100000); setMaxCharCount(""); }}
                  title="10万字以上の大長編小説"
                >
                  10万字以上
                </FilterChip>
              </div>

              {/* 直接入力フォーム (絵文字不使用) */}
              <div className="char-count-input-group">
                <Input
                  type="number" 
                  className="char-count-number-input h-8 w-24"
                  placeholder="最小" 
                  min="0"
                  value={minCharCount === "" ? "" : minCharCount} 
                  onChange={e => setMinCharCount(e.target.value === "" ? "" : Number(e.target.value))}
                  title="最小文字数を指定"
                />
                <span className="char-count-input-separator">〜</span>
                <Input
                  type="number" 
                  className="char-count-number-input h-8 w-24"
                  placeholder="最大" 
                  min="0"
                  value={maxCharCount === "" ? "" : maxCharCount} 
                  onChange={e => setMaxCharCount(e.target.value === "" ? "" : Number(e.target.value))}
                  title="最大文字数を指定"
                />
                <span className="char-count-input-unit">字</span>
              </div>
            </div>

            {/* メタ属性スマートフィルター - 画像アセット・更新監視 */}
            <div className="quick-filter-row metadata-filters-row mt-1">
              <span className="quick-filter-label">その他属性</span>
              <div className="quick-filter-items">
                <FilterChip
                  active={assetFilter === 'all'}
                  onClick={() => setAssetFilter('all')}
                >
                  アセット: すべて
                </FilterChip>
                <FilterChip
                  active={assetFilter === 'no_assets'}
                  onClick={() => setAssetFilter('no_assets')}
                  title="画像や添付ファイルを含まない作品"
                >
                  テキストのみ
                </FilterChip>
                <FilterChip
                  active={assetFilter === 'has_images'}
                  onClick={() => setAssetFilter('has_images')}
                  title="表紙以外の画像や挿絵を含む作品"
                >
                  画像あり
                </FilterChip>
                <FilterChip
                  active={assetFilter === 'has_files'}
                  onClick={() => setAssetFilter('has_files')}
                  title="画像以外の添付ファイルを含む作品"
                >
                  その他ファイルあり
                </FilterChip>
                <FilterChip
                  active={assetFilter === 'has_images_and_files'}
                  onClick={() => setAssetFilter('has_images_and_files')}
                  title="画像とその他ファイルの両方を含む作品"
                >
                  画像+その他あり
                </FilterChip>

                <span className="mx-2 text-muted-foreground/30">|</span>

                {/* 更新監視 */}
                <FilterChip
                  active={watchFilter === 'all'}
                  onClick={() => setWatchFilter('all')}
                >
                  更新確認: すべて
                </FilterChip>
                <FilterChip
                  active={watchFilter === 'watched'}
                  onClick={() => setWatchFilter('watched')}
                  title="更新確認対象（更新ON）の作品"
                >
                  更新ON
                </FilterChip>
                <FilterChip
                  active={watchFilter === 'unwatched'}
                  onClick={() => setWatchFilter('unwatched')}
                  title="更新確認対象外（更新OFF）の作品"
                >
                  更新OFF
                </FilterChip>
              </div>
            </div>

            <div className="facet-picker-grid">
              <div className="facet-picker">
                <div className="facet-picker-header">
                  <span className="quick-filter-label">タグ</span>
                  <Input
                    className="facet-search-input h-8"
                    value={tagCandidateQuery}
                    onChange={e => setTagCandidateQuery(e.target.value)}
                    placeholder="タグを検索"
                  />
                </div>
                <div className="facet-section-title">よく使う候補</div>
                <div className="quick-filter-items facet-chip-list">
                  {popularTags.map(facet => {
                    const mode = tagFilters[facet.name];
                    return (
                      <FilterChip
                        key={facet.name}
                        mode={mode}
                        onClick={() => handleToggleTagFilter(facet.name)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}#{facet.name}
                        <span className="facet-count">{facet.count}</span>
                      </FilterChip>
                    );
                  })}
                </div>
                {normalizedTagQuery && (
                  <>
                    <div className="facet-section-title">検索結果</div>
                    <div className="quick-filter-items facet-chip-list">
                      {smartSearchedTags.length > 0 ? smartSearchedTags.map(facet => {
                        const mode = tagFilters[facet.name];
                        return (
                          <FilterChip
                            key={`searched-${facet.name}`}
                            mode={mode}
                            onClick={() => handleToggleTagFilter(facet.name)}
                            title="クリックで + -> - -> 解除"
                          >
                            {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}#{facet.name}
                            <span className="facet-count">{facet.count}</span>
                          </FilterChip>
                        );
                      }) : <span className="facet-empty">一致するタグはありません</span>}
                    </div>
                  </>
                )}
              </div>

              <div className="facet-picker">
                <div className="facet-picker-header">
                  <span className="quick-filter-label">著者</span>
                  <Input
                    className="facet-search-input h-8"
                    value={authorCandidateQuery}
                    onChange={e => setAuthorCandidateQuery(e.target.value)}
                    placeholder="著者を検索"
                  />
                </div>
                <div className="facet-section-title">よく使う候補</div>
                <div className="quick-filter-items facet-chip-list">
                  {popularAuthors.map(facet => {
                    const mode = authorFilters[facet.name];
                    return (
                      <FilterChip
                        key={facet.name}
                        mode={mode}
                        onClick={() => handleToggleAuthorFilter(facet.name)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}{facet.name}
                        <span className="facet-count">{facet.count}</span>
                      </FilterChip>
                    );
                  })}
                </div>
                {normalizedAuthorQuery && (
                  <>
                    <div className="facet-section-title">検索結果</div>
                    <div className="quick-filter-items facet-chip-list">
                      {smartSearchedAuthors.length > 0 ? smartSearchedAuthors.map(facet => {
                        const mode = authorFilters[facet.name];
                        return (
                          <FilterChip
                            key={`searched-${facet.name}`}
                            mode={mode}
                            onClick={() => handleToggleAuthorFilter(facet.name)}
                            title="クリックで + -> - -> 解除"
                          >
                            {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}{facet.name}
                            <span className="facet-count">{facet.count}</span>
                          </FilterChip>
                        );
                      }) : <span className="facet-empty">一致する著者はありません</span>}
                    </div>
                  </>
                )}
              </div>
            </div>
            
            <div className="quick-filter-panel-actions">
              {(searchVal || query || 
                Object.keys(tagFilters).length > 0 || 
                Object.keys(authorFilters).length > 0 || 
                minCharCount !== "" || 
                maxCharCount !== "" || 
                assetFilter !== "all" || 
                watchFilter !== "all") && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="quick-filter-clear"
                  onClick={() => {
                    setSearchVal('');
                    setQuery('');
                    setTagFilters({});
                    setAuthorFilters({});
                    setMinCharCount("");
                    setMaxCharCount("");
                    setAssetFilter('all');
                    setWatchFilter('all');
                  }}
                >
                  フィルターをクリア
                </Button>
              )}
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="quick-filter-panel-close"
                onClick={() => setShowFilters(false)}
                title="フィルターパネルを閉じる"
              >
                <span className="toggle-arrow up" />
              </Button>
            </div>
          </div>
        )}
      </div>

      {/* Download cards */}
      {loading ? (
        <div className="library-loading">
          <div className="spinner" />
          <span>読み込み中...</span>
        </div>
      ) : (
        <>
          {librarySubTab === "works" && (
            filteredDownloads.length === 0 ? (
              <div className="library-empty">
                <p>{hasAnyDownloads ? "該当するダウンロードデータが見つかりません" : "ライブラリはまだ空です"}</p>
                <p className="hint">
                  {!tauriAvailable
                    ? "開発ブラウザでは保存済みデータを読み込めません。デスクトップアプリでPixiv/FANBOXから作品を保存するとここに表示されます。"
                    : hasAnyDownloads
                      ? "検索条件やフィルターを変更するか、絞り込みを解除してください。"
                      : "Pixiv または FANBOX ブラウザで作品を開き、保存候補からライブラリに追加してください。"}
                </p>
              </div>
            ) : (
              <>
                <LibraryWorkGrid
                  items={filteredDownloads}
                  scrollSelector={mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content"}
                  hasMore={hasMoreDownloads}
                  loadingMore={loadingMore}
                  onLoadMore={() => {
                    if (nextCursor) fetchDataPage(nextCursor, { replace: false, preferCache: false });
                  }}
                  viewMode={libraryViewMode}
                  renderItem={(dl) => {
                  const isChecked = selectedDownloadIds.includes(dl.id);
                  const isEpubChecked = isEpubMode && epubSelectedIds?.has(dl.id);
                  const cardTags = dl.tags;
                  return (
                    <LibraryWorkCard
                      key={dl.id} 
                      className={cn(
                        `view-${libraryViewMode}`,
                        isEpubMode && "epub-mode",
                        isDeleteMode && "delete-mode",
                        (isUpdateMode || isUpdatePageMode) && "update-mode",
                        isChecked && "checked",
                        isEpubChecked && "epub-checked",
                        dl.watchUpdates ? "watch-active update-checked" : "watch-inactive",
                      )}
                      onClick={() => {
                        if (isEpubMode) {
                          epubToggleSelect(dl.id);
                        } else if (isDeleteMode) {
                          toggleSelectDownload(dl.id);
                        } else if (isUpdateMode || isUpdatePageMode) {
                          handleToggleWatchCard(dl.id, !!dl.watchUpdates).then(() => onUpdateWorkTargetChange?.());
                        } else {
                          onViewDetail(dl.id);
                        }
                      }}
                    >
                      <div className="card-cover">
                        {isEpubMode && (
                          <div className="card-epub-overlay">
                            <div className={`card-epub-check-wrapper ${isEpubChecked ? 'active' : ''}`}>
                              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                                <polyline points="20 6 9 17 4 12" />
                              </svg>
                            </div>
                          </div>
                        )}
                        {isDeleteMode && (
                          <div className="card-delete-overlay">
                            <div className={`card-delete-trash-wrapper ${isChecked ? 'active' : ''}`}>
                              <TrashIcon />
                            </div>
                          </div>
                        )}
                        {(isUpdateMode || isUpdatePageMode) && (
                          <div className="card-update-overlay">
                            <div className={`card-update-badge-wrapper ${dl.watchUpdates ? 'watching active' : 'unwatched'}`}>
                              <SyncIcon active={dl.watchUpdates} className={!dl.watchUpdates ? "update-muted-icon" : ""} />
                            </div>
                          </div>
                        )}
                        {!isEpubMode && !isDeleteMode && !isUpdateMode && !isUpdatePageMode && (
                          <>
                            {/* Entity Facet Cards horizontal strips are removed to promote peer screen layouts */}
                            <div 
                              className={`card-watch-badge-wrapper ${dl.watchUpdates ? 'active' : ''}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                handleToggleWatchCard(dl.id, !!dl.watchUpdates);
                              }}
                              title={dl.watchUpdates ? "更新監視中 (クリックで解除)" : "更新監視を有効にする"}
                            >
                              {dl.watchUpdates ? (
                                <SyncIcon active={true} />
                              ) : (
                                <SyncIcon className="update-muted-icon" />
                              )}
                            </div>
                            <div 
                              className={`card-favorite-badge-wrapper ${dl.favorite ? 'active' : ''}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                handleToggleFavorite(dl.id, !!dl.favorite);
                              }}
                              title={dl.favorite ? "お気に入り解除" : "お気に入りに追加"}
                            >
                              <svg viewBox="0 0 24 24" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round">
                                <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
                              </svg>
                            </div>
                          </>
                        )}
                        {getCoverSource(dl) ? (
                          <img src={getCoverSource(dl)!} alt="" onError={() => handleImageError(dl)} />
                        ) : (
                          <div className="cover-placeholder"><ImageIcon /></div>
                        )}
                        <Badge className={cn("source-tag", dl.source)}>{dl.source === "pixiv" ? "Pixiv" : "FANBOX"}</Badge>
                        {dl.contentType && (
                          <Badge variant="secondary" className="content-type-tag">
                            {dl.contentType === "novel" ? "小説" : dl.contentType === "article" || dl.contentType === "post" ? "投稿" : dl.contentType.toUpperCase()}
                          </Badge>
                        )}
                      </div>
                      <div className="card-body">
                        {dl.seriesId && dl.seriesTitle && (
                          <Button
                            type="button"
                            variant="link"
                            size="sm"
                            className="card-series-link"
                            onClick={(e) => {
                              e.stopPropagation();
                              if (!isEpubMode && !isDeleteMode && !isUpdateMode && !isUpdatePageMode) {
                                onViewSeries?.(dl.source, dl.seriesId!);
                              }
                            }}
                            title={`シリーズ「${dl.seriesTitle}」を開く`}
                          >
                            {dl.seriesTitle}
                          </Button>
                        )}
                        <h4 className="card-title" title={dl.title}>{dl.title}</h4>
                        <p 
                          className={`card-author-link ${authorFilters[dl.authorName] === 'include' ? 'active' : authorFilters[dl.authorName] === 'exclude' ? 'exclude' : ''}`} 
                          onClick={(e) => { 
                            e.stopPropagation(); 
                            if (!isEpubMode && !isDeleteMode && !isUpdateMode && !isUpdatePageMode) {
                              onViewPerson?.(dl.source, dl.personId || dl.authorId);
                            }
                          }}
                          title="作者ページを開く"
                        >
                          {authorFilters[dl.authorName] === 'exclude' ? '－ ' : ''}{dl.authorName}
                        </p>
                        <div className="card-meta">
                          <span>{formatDate(dl.downloadedAt)}</span>
                          <span>{dl.assetCount} assets · {formatBytes(dl.fileSizeBytes)} · {dl.textLength.toLocaleString()} 字</span>
                        </div>
                        {dl.matchHighlights && dl.matchHighlights.length > 0 && (
                          <div className="card-search-snippets">
                            {dl.matchHighlights.slice(0, 3).map((highlight, highlightIdx) => (
                              <p
                                key={`${highlight.field}-${highlight.sourceChunkId ?? highlightIdx}`}
                                className={cn("card-search-snippet", highlight.matchType === "semantic" && "semantic")}
                                title={(highlight.text || highlight.segments.map(segment => segment.text).join("")).trim()}
                              >
                                {highlight.field === "body" ? "本文" : highlight.field === "excerpt" ? "概要" : highlight.field === "metadata" ? "意味" : "一致"}:{" "}
                                {highlight.segments.map((segment, idx) => (
                                  segment.matched
                                    ? <mark key={idx} className="search-highlight-mark">{segment.text}</mark>
                                    : <span key={idx}>{segment.text}</span>
                                ))}
                              </p>
                            ))}
                          </div>
                        )}
                        {dl.scoreReasons && dl.scoreReasons.length > 0 && (
                          <div
                            className="card-score-reasons"
                            title={dl.scoreReasons.map(reason => `${reason.field}: ${reason.matchType} "${reason.term}" +${reason.contribution.toFixed(1)}${reason.detail ? `\n${reason.detail}` : ""}`).join("\n")}
                          >
                            {dl.scoreReasons.slice(0, 3).map((reason, idx) => (
                              <span key={`${reason.field}-${reason.matchType}-${reason.term}-${idx}`} className="card-score-reason">
                                {reason.field}:{reason.matchType} +{reason.contribution.toFixed(1)}
                              </span>
                            ))}
                          </div>
                        )}
                        {cardTags.length > 0 && (
                          <div className="card-tags">
                            {cardTags
                              .slice(0, 3)
                              .map((tag, idx) => {
                                const mode = tagFilters[tag];
                                return (
                                  <Badge
                                    key={idx} 
                                    variant={mode ? "secondary" : "outline"}
                                    className={cn("tag-badge", mode === 'include' && 'active', mode === 'exclude' && 'exclude')}
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleToggleTagFilter(tag);
                                    }}
                                    title={`タグ「#${tag}」をフィルター（+ / - / 解除）`}
                                  >
                                    {mode === 'include' ? '＋ ' : mode === 'exclude' ? '－ ' : ''}#{tag}
                                  </Badge>
                                );
                              })
                            }
                            {cardTags.length > 3 && (
                              <TooltipProvider delayDuration={150}>
                                <Tooltip>
                                  <TooltipTrigger asChild>
                                    <Badge
                                      variant="outline"
                                      className="tag-badge more-badge"
                                      onClick={(e) => e.stopPropagation()}
                                    >
                                      +{cardTags.length - 3}
                                    </Badge>
                                  </TooltipTrigger>
                                  <TooltipContent className="library-tags-popover-content">
                                    <div className="library-tags-popover-title">すべてのタグ ({cardTags.length})</div>
                                    <div className="library-tags-popover-list">
                                      {cardTags.map((tag, idx) => {
                                        const mode = tagFilters[tag];
                                        return (
                                          <Badge
                                            key={idx}
                                            variant={mode ? "secondary" : "outline"}
                                            className={cn("tooltip-tag-badge", mode === 'include' && 'active', mode === 'exclude' && 'exclude')}
                                            onClick={(e) => {
                                              e.stopPropagation();
                                              handleToggleTagFilter(tag);
                                            }}
                                            title={`タグ「#${tag}」をフィルター（+ / - / 解除）`}
                                          >
                                            {mode === 'include' ? '＋ ' : mode === 'exclude' ? '－ ' : ''}#{tag}
                                          </Badge>
                                        );
                                      })}
                                    </div>
                                  </TooltipContent>
                                </Tooltip>
                              </TooltipProvider>
                            )}
                          </div>
                        )}
                      </div>
                    </LibraryWorkCard>
                  );
                }}
              />
                {(loadingMore || hasMoreDownloads) && (
                  <div className="library-load-more">
                    {loadingMore ? (
                      <>
                        <div className="spinner" />
                        <span>追加読み込み中...</span>
                      </>
                    ) : (
                      <ToolbarButton
                        onClick={() => {
                          if (nextCursor) fetchDataPage(nextCursor, { replace: false, preferCache: false });
                        }}
                      >
                        さらに読み込む
                      </ToolbarButton>
                    )}
                  </div>
                )}
              </>
            )
          )}

          {librarySubTab === "authors" && (
            filteredAuthors.length === 0 ? (
              <div className="library-empty">
                <p>該当する作者が見つかりません</p>
              </div>
            ) : (
              <div className={cn("entity-facet-peer-grid", libraryViewMode === "compact" ? "view-compact" : "view-gallery")}>
                {filteredAuthors.map(author => (
                  <EntityFacetGridCard
                    key={`${author.source}-${author.sourceKey}`}
                    facet={author}
                    type="person"
                    viewMode={libraryViewMode === "compact" ? "compact" : "gallery"}
                    onClick={() => onViewPerson?.(author.source, author.sourceKey)}
                  />
                ))}
              </div>
            )
          )}

          {librarySubTab === "series" && (
            filteredSeriesList.length === 0 ? (
              <div className="library-empty">
                <p>該当するシリーズが見つかりません</p>
              </div>
            ) : (
              <div className={cn("entity-facet-peer-grid", libraryViewMode === "compact" ? "view-compact" : "view-gallery")}>
                {filteredSeriesList.map(series => (
                  <EntityFacetGridCard
                    key={`${series.source}-${series.sourceKey}`}
                    facet={series}
                    type="series"
                    viewMode={libraryViewMode === "compact" ? "compact" : "gallery"}
                    onClick={() => onViewSeries?.(series.source, series.sourceKey)}
                  />
                ))}
              </div>
            )
          )}
        </>
      )}
      
      {showScrollTop && (
        <Button
          type="button"
          variant="secondary"
          size="icon"
          className={cn("scroll-to-top-btn", epubSidebarOpen && "sidebar-open")}
          onClick={scrollToTop}
          title="最上部へスクロール"
        >
          <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="18 15 12 9 6 15" />
          </svg>
        </Button>
      )}
    </div>

    </>
  );
}
