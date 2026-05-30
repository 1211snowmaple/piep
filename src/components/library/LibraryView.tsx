import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save, open, ask } from "@tauri-apps/plugin-dialog";
import { SearchIcon, TrashIcon, ExportIcon, ArchiveIcon, ImageIcon, RefreshIcon, SyncIcon, FunnelIcon, TemplateIcon, LibraryIcon, FileIcon, SaveIcon, PanelRightIcon, BookIcon } from "../icons/Icons";
import { store } from "../../store";

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
  tags: string | null;
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
  matchSnippet?: string | null;
  matchFields?: string[];
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

interface FilterFacets {
  tags: FacetCount[];
  authors: FacetCount[];
  contentTypes: FacetCount[];
  assetTypes: FacetCount[];
}

type FilterModeValue = "include" | "exclude";
type AssetFilter = "all" | "no_assets" | "has_images" | "has_files" | "has_images_and_files" | "has_assets";
type WatchFilter = "all" | "watched" | "unwatched";

const LIBRARY_PAGE_SIZE = 120;
const LIBRARY_SELECT_PAGE_SIZE = 1000;
const COVER_PRELOAD_LIMIT = 80;

interface DownloadSearchParams {
  query: string | null;
  source: string | null;
  contentType: string | null;
  sortBy: string;
  sortOrder: "asc" | "desc";
  limit: number;
  offset: number;
  favorite: boolean | null;
  tagsInclude: string[] | null;
  tagsExclude: string[] | null;
  tagFilterMode: "and" | "or";
  authorsInclude: string[] | null;
  authorsExclude: string[] | null;
  minCharCount: number | null;
  maxCharCount: number | null;
  assetFilter: string | null;
  watchFilter: string | null;
  personSource?: string | null;
  personKey?: string | null;
  seriesSource?: string | null;
  seriesKey?: string | null;
  searchMode?: "smart" | "sort";
}

interface SearchIndexStatus {
  totalDownloads: number;
  indexedDownloads: number;
  pendingDownloads: number;
  isComplete: boolean;
}

interface LibraryDataCacheEntry {
  downloads: DownloadEntry[];
  stats: DbStats | null;
  coverCache: Record<number, string>;
  hasMore: boolean;
}

const libraryDataCache = new Map<string, LibraryDataCacheEntry>();

export interface LibraryUiState {
  searchVal: string;
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
}

interface Props {
  mode?: "library" | "epub" | "update";
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

function parseTags(tagsStr: string | null): string[] {
  if (!tagsStr) return [];
  const trimmed = tagsStr.trim();
  if (trimmed.startsWith("[") && trimmed.endsWith("]")) {
    try {
      const parsed = JSON.parse(trimmed);
      if (Array.isArray(parsed)) {
        return parsed.map(t => String(t).trim()).filter(Boolean);
      }
    } catch {}
  }
  return trimmed
    .replace(/[\[\]"']/g, "")
    .split(",")
    .map(t => t.trim())
    .filter(Boolean);
}

interface UpdateTarget {
  id: number;
  targetType: "work" | "author" | "series";
  source: "pixiv" | "fanbox";
  sourceKey: string;
  displayName: string;
  enabled: boolean;
  lastCheckedAt: string | null;
  lastSeenSourceId: string | null;
  lastSeenSourceUpdatedAt: string | null;
  metadataJson: string | null;
}

function normalizePixivNovelId(item: any): string {
  return String(item.id ?? item.detail?.id ?? "");
}

function normalizePixivSeries(item: any): { id: string; title: string } | null {
  const series = item.series && "id" in item.series ? item.series : null;
  const nav = item.seriesNavigation ?? item.series_navigation ?? item.detail?.seriesNavigation ?? item.detail?.series_navigation;
  const id = String(item.seriesId ?? item.series_id ?? item.detail?.seriesId ?? item.detail?.series_id ?? series?.id ?? nav?.seriesId ?? nav?.series_id ?? "");
  const title = String(item.seriesTitle ?? item.series_title ?? item.detail?.seriesTitle ?? item.detail?.series_title ?? series?.title ?? nav?.seriesTitle ?? nav?.series_title ?? "");
  return id ? { id, title: title || "シリーズ" } : null;
}

function pixivTags(data: any): string[] {
  const tags = data.detail?.tags ?? data.tags ?? [];
  if (Array.isArray(tags)) return tags.map((t: any) => t.name || t).filter(Boolean);
  if (Array.isArray(tags.tags)) return tags.tags.map((t: any) => t.name || t).filter(Boolean);
  return [];
}

export function LibraryView({ mode = "library", onViewDetail, onViewPerson, onViewSeries, showToast, initialFilter = "", epubSelectedIds, onEpubSelectionChange, onToggleEpubSidebar, epubSidebarOpen, onOpenTemplateManager, updateSidebarOpen, onToggleUpdateSidebar, onUpdateWorkTargetChange, initialTagFilters, initialAuthorFilters, initialState, restoreKey = 0, onUiStateChange }: Props) {
  const tauriAvailable = isTauriRuntime();
  const isEpubMode = mode === "epub";
  const isUpdatePageMode = mode === "update";
  const [downloads, setDownloads] = useState<DownloadEntry[]>([]);
  const [stats, setStats] = useState<DbStats | null>(null);
  const [facets, setFacets] = useState<FilterFacets>({ tags: [], authors: [], contentTypes: [], assetTypes: [] });
  const [searchVal, setSearchVal] = useState(initialState?.searchVal ?? "");
  const [query, setQuery] = useState(initialState?.searchVal ?? "");

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
  const [isUpdateMode, setIsUpdateMode] = useState(false);
  const [showFilters, setShowFilters] = useState(initialState?.showFilters ?? false);
  const [showBackupDropdown, setShowBackupDropdown] = useState(false);
  const backupRef = useRef<HTMLDivElement>(null);
  const filterPanelRef = useRef<HTMLDivElement>(null);
  const filterToggleRef = useRef<HTMLButtonElement>(null);
  const coverCacheRef = useRef<Record<number, string>>({});
  const downloadsRef = useRef<DownloadEntry[]>([]);
  const [showScrollTop, setShowScrollTop] = useState(false);
  const [tagCandidateQuery, setTagCandidateQuery] = useState("");
  const [authorCandidateQuery, setAuthorCandidateQuery] = useState("");
  const [facetSearchResults, setFacetSearchResults] = useState<{ tags: FacetCount[]; authors: FacetCount[] }>({ tags: [], authors: [] });
  const [searchIndexStatus, setSearchIndexStatus] = useState<SearchIndexStatus | null>(null);
  const lastRestoreKeyRef = useRef(-1);
  const fetchSeqRef = useRef(0);

  useEffect(() => {
    coverCacheRef.current = coverCache;
  }, [coverCache]);

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
    };
  }, [mode, searchVal, sourceFilter, sortBy, sortOrder, tagFilters, authorFilters, filterMode, minCharCount, maxCharCount, assetFilter, watchFilter, showFilters, initialState?.scrollTop]);

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
        const nextFacets = await invoke<FilterFacets>("db_get_filter_facets");
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
            ? invoke<FacetCount[]>("db_search_filter_facets", { kind: "tags", query: tagQuery, limit: 30 })
            : Promise.resolve([]),
          authorQuery
            ? invoke<FacetCount[]>("db_search_filter_facets", { kind: "authors", query: authorQuery, limit: 30 })
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
    showToast("監視中の作品・著者・シリーズの更新を確認中...", "info");
    try {
      // 1. 監視対象の作品一覧と、著者・シリーズのターゲット一覧を取得
      const watched = await invoke<DownloadEntry[]>("db_get_watched_downloads");
      const collectionTargets = await invoke<UpdateTarget[]>("db_list_update_targets", { targetType: null, enabledOnly: true });
      const enabledCollections = collectionTargets.filter(t => t.enabled && (t.targetType === "author" || t.targetType === "series"));

      if (watched.length === 0 && enabledCollections.length === 0) {
        showToast("更新監視対象の作品、著者、シリーズがありません。", "info");
        setIsUpdating(false);
        return;
      }

      // 2. 必要なトークン類を store から取得
      const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
      const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
      const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";

      let updatedCount = 0;
      let newWorksCount = 0;
      let errorCount = 0;

      // 3. 各作品の更新確認 (2段階チェック: まず軽量な更新日時のみを比較し、変化があれば詳細DL & 精密ハッシュ判定を実行)
      if (watched.length > 0) {
        for (const dl of watched) {
          try {
            if (dl.source === "pixiv") {
              if (!refreshToken) continue;

              // 【フェーズ1】軽量なメタデータ情報のみを取得
              const metadata: any = await invoke("fetch_pixiv_novel_metadata", {
                novelId: dl.sourceId,
                refreshToken
              });

              // 最終更新日時（create_date）を比較する
              const metadataUpdatedAt = metadata.create_date || null;
              const isDateChanged = dl.sourceUpdatedAt !== metadataUpdatedAt;

              if (!isDateChanged) {
                continue;
              }

              const data: any = await invoke("fetch_pixiv_novel", {
                novelId: dl.sourceId,
                refreshToken
              });

              // 新しく download_and_save を呼ぶことでハッシュ更新＆バージョン管理を自動実行
              const tags = data.detail?.tags?.map((t: any) => t.name || t) || [];
              await invoke("download_and_save", {
                data: data,
                source: dl.source,
                sourceId: dl.sourceId,
                title: data.detail?.title || dl.title,
                authorName: data.detail?.user?.name || dl.authorName,
                authorId: data.detail?.user?.id?.toString() || dl.authorId,
                contentType: "novel",
                tags,
                excerpt: data.detail?.caption || null,
                sourceCreatedAt: data.detail?.create_date || null,
                cookie: null,
                userAgent: null
              });
              
              // 最新のDBエントリを再取得して、バージョンが上がっているか確認
              const currentDl = await invoke<DownloadEntry>("db_get_download", { id: dl.id });
              if (currentDl.currentVersion > dl.currentVersion) {
                updatedCount++;
              }
            } else if (dl.source === "fanbox") {
              if (!fanboxCookie) continue;

              // FANBOXも同様に、まず軽量な情報チェック（fetch_fanbox_post）を行い、
              // updatedDatetime を取得して前回の更新日時と比較します。
              const post: any = await invoke("fetch_fanbox_post", {
                postId: dl.sourceId,
                cookie: fanboxCookie,
                userAgent: fanboxUserAgent
              });

              const postUpdatedAt = post.updatedDatetime || null;
              const isDateChanged = dl.sourceUpdatedAt !== postUpdatedAt;

              if (!isDateChanged) {
                continue;
              }

              const tags = post.tags || [];
              await invoke("download_and_save", {
                data: post,
                source: dl.source,
                sourceId: dl.sourceId,
                title: post.title || dl.title,
                authorName: post.user?.name || dl.authorName,
                authorId: post.creatorId || dl.authorId,
                contentType: post.type || "article",
                tags,
                excerpt: post.excerpt || null,
                sourceCreatedAt: post.publishedDatetime || null,
                cookie: fanboxCookie,
                userAgent: fanboxUserAgent
              });

              // バージョンが上がっているか確認
              const currentDl = await invoke<DownloadEntry>("db_get_download", { id: dl.id });
              if (currentDl.currentVersion > dl.currentVersion) {
                updatedCount++;
              }
            }
          } catch (err) {
            console.error(`Update check error for ${dl.title}:`, err);
            errorCount++;
          }
          
          // 相手サーバーの負荷軽減とIPブロック(429)の防止のための1.5秒のウェイト
          await new Promise(r => setTimeout(r, 1500));
        }
      }

      // 4. 監視中の著者・シリーズの新作確認＆自動ダウンロード
      if (enabledCollections.length > 0) {
        for (const target of enabledCollections) {
          try {
            let items: any[] = [];
            if (target.source === "pixiv" && target.targetType === "author") {
              if (!refreshToken) continue;
              items = await invoke<any[]>("fetch_pixiv_user_novels", { userId: target.sourceKey, refreshToken });
            } else if (target.source === "pixiv" && target.targetType === "series") {
              if (!refreshToken) continue;
              items = await invoke<any[]>("fetch_pixiv_series_novels", { seriesId: target.sourceKey, refreshToken });
            } else if (target.source === "fanbox" && target.targetType === "author") {
              if (!fanboxCookie) continue;
              items = await invoke<any[]>("fetch_fanbox_creator_posts", { creatorId: target.sourceKey, cookie: fanboxCookie, userAgent: fanboxUserAgent });
            }

            for (const item of items) {
              const sourceId = target.source === "pixiv" ? normalizePixivNovelId(item) : String(item.id || "");
              if (!sourceId) continue;
              const existing = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: target.source, sourceId });
              if (existing) continue;

              // 新作の自動ダウンロードを実行
              showToast(`新作「${item.title || "無題"}」を自動保存中...`, "info");
              if (target.source === "pixiv") {
                const data: any = await invoke("fetch_pixiv_novel", { novelId: sourceId, refreshToken });
                await invoke<DownloadEntry>("download_and_save", {
                  data,
                  source: "pixiv",
                  sourceId,
                  title: data.detail?.title || item.title || "無題",
                  authorName: data.detail?.user?.name || item.user?.name || target.displayName || "unknown",
                  authorId: String(data.detail?.user?.id || item.user?.id || target.sourceKey || "0"),
                  contentType: "novel",
                  tags: pixivTags(data),
                  excerpt: data.detail?.caption || null,
                  sourceCreatedAt: data.detail?.create_date || item.create_date || null,
                  cookie: null,
                  userAgent: null,
                });
                
                // 関連シリーズの自動監視追加
                const series = normalizePixivSeries(item);
                if (series) {
                  await invoke("db_upsert_update_target", {
                    target: {
                      targetType: "series",
                      source: "pixiv",
                      sourceKey: series.id,
                      displayName: series.title,
                      enabled: true,
                      metadataJson: null,
                    }
                  });
                }
              } else {
                const post: any = await invoke("fetch_fanbox_post", { postId: sourceId, cookie: fanboxCookie, userAgent: fanboxUserAgent });
                await invoke("download_and_save", {
                  data: post,
                  source: "fanbox",
                  sourceId,
                  title: post.title || item.title || "無題",
                  authorName: post.user?.name || item.user?.name || target.displayName || "unknown",
                  authorId: post.creatorId || post.creator_id || item.creatorId || "0",
                  contentType: post.type || post.postType || "article",
                  tags: post.tags || [],
                  excerpt: post.excerpt || null,
                  sourceCreatedAt: post.publishedDatetime || post.published_datetime || null,
                  cookie: fanboxCookie,
                  userAgent: fanboxUserAgent,
                });
              }
              newWorksCount++;
              await new Promise(r => setTimeout(r, 1500));
            }

            // チェック済マークの更新
            const first = items[0];
            const firstId = target.source === "pixiv" ? normalizePixivNovelId(first || {}) : String(first?.id || "");
            const firstUpdated = target.source === "pixiv"
              ? first?.create_date || first?.createDate || null
              : first?.updatedDatetime || first?.updated_datetime || null;

            await invoke("db_mark_update_target_checked", {
              targetType: target.targetType,
              source: target.source,
              sourceKey: target.sourceKey,
              lastSeenSourceId: firstId || null,
              lastSeenSourceUpdatedAt: firstUpdated || null,
            });

            await new Promise(r => setTimeout(r, 1200));
          } catch (err) {
            console.error(`Collection check error for ${target.displayName}:`, err);
            errorCount++;
          }
        }
      }

      if (updatedCount > 0 || newWorksCount > 0) {
        let msg = "";
        if (updatedCount > 0 && newWorksCount > 0) {
          msg = `${updatedCount}件の作品更新と${newWorksCount}件の新作をダウンロードしました！`;
        } else if (updatedCount > 0) {
          msg = `${updatedCount}件の作品更新をダウンロードしました！`;
        } else {
          msg = `${newWorksCount}件の新作をダウンロードしました！`;
        }
        showToast(msg, "success");
        fetchData();
      } else {
        showToast("すべて最新状態です。更新や新作はありませんでした。", "success");
      }
      
      if (errorCount > 0) {
        showToast(`${errorCount}件のチェック中にエラーが発生しました。`, "error");
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
    tagFilters,
    authorFilters,
    filterMode,
    minCharCount,
    maxCharCount,
    assetFilter,
    watchFilter,
  }), [mode, sourceFilter, effectiveSortBy, sortOrder, query, tagFilters, authorFilters, filterMode, minCharCount, maxCharCount, assetFilter, watchFilter]);

  const buildSearchParams = useCallback((limit: number, offset: number): DownloadSearchParams => {
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
      offset,
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
      searchMode: "smart",
    };
  }, [sourceFilter, effectiveSortBy, sortOrder, query, tagFilters, authorFilters, filterMode, minCharCount, maxCharCount, assetFilter, watchFilter]);

  const fetchDataPage = useCallback(async (offset = 0, options?: { replace?: boolean; preferCache?: boolean }) => {
    const replace = options?.replace ?? offset === 0;
    const preferCache = options?.preferCache ?? replace;
    const seq = ++fetchSeqRef.current;

    if (replace && preferCache) {
      const cached = libraryDataCache.get(filterCacheKey);
      if (cached) {
        setDownloads(cached.downloads);
        setStats(cached.stats);
        setCoverCache(cached.coverCache);
        setHasMoreDownloads(cached.hasMore);
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
      setLoading(false);
      setLoadingMore(false);
      return;
    }

    try {
      const [resultsWithLookahead, dbStats] = await Promise.all([
        invoke<DownloadEntry[]>("db_search_downloads", {
          params: buildSearchParams(LIBRARY_PAGE_SIZE + 1, offset)
        }),
        invoke<DbStats>("db_get_stats"),
      ]);

      if (seq !== fetchSeqRef.current) return;

      const page = resultsWithLookahead.slice(0, LIBRARY_PAGE_SIZE);
      const nextHasMore = resultsWithLookahead.length > LIBRARY_PAGE_SIZE;
      const nextDownloads = replace ? page : [...downloadsRef.current, ...page];
      setDownloads(nextDownloads);
      setStats(dbStats);
      setHasMoreDownloads(nextHasMore);
      setLoading(false);

      const baseCoverCache = replace ? (libraryDataCache.get(filterCacheKey)?.coverCache ?? {}) : coverCacheRef.current;
      libraryDataCache.set(filterCacheKey, {
        downloads: nextDownloads,
        stats: dbStats,
        coverCache: baseCoverCache,
        hasMore: nextHasMore,
      });

      const coversToLoad = page
        .filter(dl => dl.coverPath && !baseCoverCache[dl.id])
        .slice(0, COVER_PRELOAD_LIMIT);

      if (coversToLoad.length > 0) {
        Promise.all(
          coversToLoad.map(async (dl) => {
            try {
              const b64 = await invoke<string>("read_image_base64", { path: dl.coverPath });
              return { id: dl.id, b64 };
            } catch {
              return null;
            }
          })
        ).then(covers => {
          const newCache: Record<number, string> = {};
          for (const res of covers) {
            if (res) newCache[res.id] = res.b64;
          }
          if (Object.keys(newCache).length === 0) return;
          setCoverCache(prev => {
            const merged = { ...prev, ...newCache };
            const current = libraryDataCache.get(filterCacheKey);
            if (current) {
              libraryDataCache.set(filterCacheKey, { ...current, coverCache: merged });
            }
            return merged;
          });
        });
      }
    } catch (e) {
      console.error("Library fetch error:", e);
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }, [filterCacheKey, buildSearchParams]);

  const fetchData = useCallback(() => {
    fetchDataPage(0, { replace: true, preferCache: true });
  }, [fetchDataPage]);

  useEffect(() => { fetchData(); }, [fetchData]);

  useEffect(() => {
    if (!tauriAvailable) return;
    let cancelled = false;

    const rebuild = async () => {
      try {
        let status = await invoke<SearchIndexStatus>("db_get_search_index_status");
        if (cancelled) return;
        setSearchIndexStatus(status);

        while (!cancelled && status.pendingDownloads > 0) {
          status = await invoke<SearchIndexStatus>("db_rebuild_search_index_batch", { limit: 24 });
          if (cancelled) return;
          setSearchIndexStatus(status);
          if (status.pendingDownloads === 0) {
            libraryDataCache.clear();
            fetchDataPage(0, { replace: true, preferCache: false });
            break;
          }
          await new Promise(resolve => window.setTimeout(resolve, 80));
        }
      } catch (e) {
        console.error("Search index rebuild failed:", e);
      }
    };

    rebuild();
    return () => {
      cancelled = true;
    };
  }, [tauriAvailable, fetchDataPage]);

  useEffect(() => {
    const selector = mode === "epub" ? ".epub-main-content" : mode === "update" ? ".update-main-content" : ".library-main-content";
    const container = document.querySelector(selector);
    if (!container) return;

    const handleLoadMore = () => {
      const remaining = container.scrollHeight - container.scrollTop - container.clientHeight;
      if (remaining < 900 && hasMoreDownloads && !loading && !loadingMore) {
        fetchDataPage(downloadsRef.current.length, { replace: false, preferCache: false });
      }
    };

    container.addEventListener("scroll", handleLoadMore);
    handleLoadMore();
    return () => container.removeEventListener("scroll", handleLoadMore);
  }, [mode, hasMoreDownloads, loading, loadingMore, fetchDataPage]);

  // すべての検索・絞り込みはデータベース（SQL）側に完全移譲したため、downloadsをそのまま返却
  const filteredDownloads = useMemo<DownloadEntry[]>(() => {
    return downloads;
  }, [downloads]);

  const fetchAllMatchingDownloads = useCallback(async (): Promise<DownloadEntry[]> => {
    if (!isTauriRuntime()) return filteredDownloads;

    const all: DownloadEntry[] = [];
    let offset = 0;

    while (true) {
      const page = await invoke<DownloadEntry[]>("db_search_downloads", {
        params: buildSearchParams(LIBRARY_SELECT_PAGE_SIZE, offset)
      });

      all.push(...page);
      if (page.length < LIBRARY_SELECT_PAGE_SIZE) break;
      offset += LIBRARY_SELECT_PAGE_SIZE;
    }

    return all;
  }, [buildSearchParams, filteredDownloads]);

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
  }, []);

  const isAllSelected = useMemo(() => {
    if (filteredDownloads.length === 0) return false;
    return filteredDownloads.every(dl => selectedDownloadIds.includes(dl.id));
  }, [filteredDownloads, selectedDownloadIds]);

  const handleToggleSelectAll = useCallback(async () => {
    showToast("現在の絞り込み条件に一致する作品を確認しています...", "info");
    const allMatching = await fetchAllMatchingDownloads();
    if (isAllSelected) {
      const matchingIds = new Set(allMatching.map(dl => dl.id));
      setSelectedDownloadIds(prev => prev.filter(id => !matchingIds.has(id)));
      showToast(`${allMatching.length} 件の選択を解除しました`, "success");
    } else {
      const newIds = new Set([...selectedDownloadIds, ...allMatching.map(dl => dl.id)]);
      setSelectedDownloadIds(Array.from(newIds));
      showToast(`${allMatching.length} 件を選択しました`, "success");
    }
  }, [isAllSelected, filteredDownloads, selectedDownloadIds, fetchAllMatchingDownloads, showToast]);

  const handleSetWatchAllFiltered = async (watch: boolean) => {
    if (filteredDownloads.length === 0) return;
    try {
      showToast(watch ? "絞り込み条件に一致する作品をすべて更新ONに設定中..." : "絞り込み条件に一致する作品をすべて更新OFFに設定中...", "info");
      const allMatching = await fetchAllMatchingDownloads();
      const filteredIds = allMatching.map(dl => dl.id);
      await Promise.all(
        filteredIds.map(id => 
          invoke("db_set_watch_updates", { downloadId: id, watch })
        )
      );
      setDownloads(prev => 
        prev.map(dl => 
          filteredIds.includes(dl.id) ? { ...dl, watchUpdates: watch } : dl
        )
      );
      showToast(watch ? `${filteredIds.length} 件の作品を更新ONにしました` : `${filteredIds.length} 件の作品を更新OFFにしました`, "success");
    } catch (e: any) {
      showToast(`一括設定エラー: ${e}`, "error");
    }
  };

  const handleToggleWatchCard = async (id: number, currentWatch: boolean) => {
    try {
      const nextWatch = !currentWatch;
      await invoke("db_set_watch_updates", { downloadId: id, watch: nextWatch });
      setDownloads(prev => prev.map(dl => dl.id === id ? { ...dl, watchUpdates: nextWatch } : dl));
      showToast(nextWatch ? "更新監視を有効にしました" : "更新監視を解除しました", "success");
    } catch (e: any) {
      showToast(`監視設定エラー: ${e}`, "error");
    }
  };

  const handleToggleFavorite = async (id: number, currentFav: boolean) => {
    try {
      const nextFav = !currentFav;
      await invoke("db_set_favorite", { downloadId: id, favorite: nextFav });
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
    if (selectedDownloadIds.length === 0) {
      setIsDeleteMode(false);
      return;
    }

    const isConfirmed = await ask(
      `選択された ${selectedDownloadIds.length} 件の作品を完全に削除してもよろしいですか？\nアセットファイルや関連データも完全に消去されます。`,
      { title: "一括削除の確認", kind: "warning", okLabel: "削除する", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;

    try {
      showToast("一括削除を実行中...", "info");
      await Promise.all(selectedDownloadIds.map(id => invoke("db_delete_download", { id })));
      showToast(`${selectedDownloadIds.length} 件の作品を完全に削除しました`, "success");
      setSelectedDownloadIds([]);
      setIsDeleteMode(false);
      fetchData();
    } catch (e: any) { 
      showToast(`一括削除エラー: ${e}`, "error"); 
    }
  };

  const handleExportZip = async () => {
    try {
      const path = await save({ filters: [{ name: "ZIP", extensions: ["zip"] }], defaultPath: "piep_backup.zip" });
      if (!path) return;
      showToast("バックアップを作成中...", "info");
      await invoke("export_all_zip", { zipPath: path });
      showToast("バックアップの作成が完了しました！", "success");
    } catch (e: any) { showToast(`バックアップ作成エラー: ${e}`, "error"); }
  };

  const handleImportZip = async () => {
    try {
      const file = await open({ filters: [{ name: "ZIP", extensions: ["zip"] }], multiple: false });
      if (!file) return;
      showToast("バックアップから復元中...", "info");
      const count = await invoke<number>("import_zip", { zipPath: file });
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

  return (
    <>
      <div className="library-view">
      {/* Sticky Header Wrapper */}
      <div className={`library-header-sticky ${isEpubMode ? 'epub-active' : ''} ${isUpdatePageMode ? 'update-active' : ''} ${isDeleteMode ? 'delete-active' : ''} ${isUpdateMode ? 'update-active' : ''}`}>
        <div className="library-header-top">
          {/* Micro Stats Bar (Frosted glass capsule, unified across Library & EPUB) */}
          {stats ? (
            <div className="library-stats-mini">
              <span className="stats-segment">
                <LibraryIcon style={{ width: '13px', height: '13px', stroke: 'currentColor' }} />
                <strong>{stats.totalDownloads}</strong> 作品
                <span className="stats-sub-detail">({stats.pixivCount} Pixiv, {stats.fanboxCount} FANBOX)</span>
              </span>
              <span className="stats-separator">·</span>
              <span className="stats-segment">
                <FileIcon style={{ width: '13px', height: '13px', stroke: 'currentColor' }} />
                <strong>{stats.totalAssets || 0}</strong> ファイル (アセット)
              </span>
              <span className="stats-separator">·</span>
              <span className="stats-segment">
                <SaveIcon style={{ width: '13px', height: '13px', stroke: 'currentColor' }} />
                合計 <strong>{formatBytes(stats.totalSizeBytes)}</strong>
                <span className="stats-sub-detail">(平均 {stats.totalDownloads > 0 ? formatBytes(stats.totalSizeBytes / stats.totalDownloads) : "0 B"})</span>
              </span>
            </div>
          ) : (
            <div className="library-stats-mini-spacer" />
          )}
        </div>
        {/* Search & Filter toolbar */}
        <div className="library-toolbar">
          {/* Upper Row: Massive Full-width Search Bar */}
          <div className="toolbar-row search-row">
            <div className={`search-input-wrapper ${searchIndexStatus && !searchIndexStatus.isComplete ? "has-index-status" : ""}`}>
              <SearchIcon />
              <input
                type="text"
                className="search-input"
                placeholder="タイトル、著者、タグ、本文で検索..."
                value={searchVal}
                onChange={e => setSearchVal(e.target.value)}
              />
              {searchIndexStatus && !searchIndexStatus.isComplete && (
                <span className="search-index-status">
                  本文検索を準備中 {Math.max(0, searchIndexStatus.totalDownloads - searchIndexStatus.pendingDownloads).toLocaleString()}/{searchIndexStatus.totalDownloads.toLocaleString()}
                </span>
              )}
            </div>
          </div>

          {/* Lower Row: Filter dropdowns and Action Buttons */}
          <div className="toolbar-row actions-row">
            <div className="filter-group">
              <select className="filter-select" value={sourceFilter} onChange={e => setSourceFilter(e.target.value)}>
                <option value="">すべて</option>
                <option value="pixiv">Pixiv</option>
                <option value="fanbox">FANBOX</option>
                <option value="favorite">お気に入り</option>
              </select>
              <select
                className="filter-select"
                value={sortBy}
                onChange={e => {
                  setSortBy(e.target.value);
                  setSortTouched(true);
                }}
              >
                <option value="relevance">関連度順</option>
                <option value="date">ダウンロード日順</option>
                <option value="published">投稿日順</option>
                <option value="title">タイトル順</option>
                <option value="author">著者順</option>
                <option value="size">サイズ順</option>
              </select>
              <button
                type="button"
                className="sort-order-btn"
                onClick={() => setSortOrder(prev => prev === "asc" ? "desc" : "asc")}
                title={sortOrder === "asc" ? "昇順（古い/小さい順）。クリックで降順に変更" : "降順（新しい/大きい順）。クリックで昇順に変更"}
              >
                <strong>{sortOrder === "asc" ? "↑" : "↓"}</strong>
              </button>
              <button 
                ref={filterToggleRef}
                className={`filter-toggle-btn ${showFilters ? 'expanded' : ''} ${isFilterActive ? 'active' : ''}`}
                onClick={() => setShowFilters(prev => !prev)}
                title={showFilters ? `フィルターパネルを閉じる${isFilterActive ? ' (現在フィルター適用中)' : ''}` : `フィルターパネルを開く${isFilterActive ? ' (現在フィルター適用中)' : ''}`}
              >
                <FunnelIcon />
                <span className={`toggle-arrow ${showFilters ? 'up' : 'down'}`} />
              </button>
            </div>
            <div className="toolbar-actions">
              {isEpubMode ? (
                /* EPUB Mode Actions */
                <>
                  <div className="epub-selection-group">
                    <span className={`epub-selection-badge ${(epubSelectedIds?.size || 0) === 0 ? 'empty' : ''}`}>
                      {epubSelectedIds?.size || 0} 件選択
                    </span>
                    <button
                      className={`toolbar-btn ${epubIsAllSelected ? "active" : ""}`}
                      title={epubIsAllSelected ? "すべての選択を解除" : "現在の絞り込み条件に一致する作品をすべて選択"}
                      onClick={epubIsAllSelected ? epubSelectNone : epubSelectAll}
                      disabled={!hasVisibleDownloads}
                    >
                      {epubIsAllSelected ? "全解除" : "全選択"}
                    </button>
                  </div>
                  <button
                    className="toolbar-btn"
                    title="テンプレート管理"
                    onClick={onOpenTemplateManager}
                    disabled={!tauriAvailable}
                  >
                    <TemplateIcon />
                  </button>
                  <button
                    className={`toolbar-btn epub-btn ${epubSidebarOpen ? 'active' : ''}`}
                    title={epubSidebarOpen ? "EPUBエクスポート設定を閉じる" : "EPUBエクスポート設定を開く"}
                    onClick={onToggleEpubSidebar}
                  >
                    <BookIcon /> EPUB
                  </button>
                </>
              ) : isUpdatePageMode ? (
                <>
                  <button
                    className={`toolbar-btn ${updateIsAllVisibleWatched ? "active" : ""}`}
                    title={updateIsAllVisibleWatched ? "表示中のすべての作品を更新OFFにする" : "表示中のすべての作品を更新ONにする"}
                    onClick={() => handleSetWatchAllFiltered(!updateIsAllVisibleWatched)}
                    disabled={!hasVisibleDownloads || !tauriAvailable}
                  >
                    {updateIsAllVisibleWatched ? "全解除" : "全選択"}
                  </button>
                  <button
                    className={`toolbar-btn ${isUpdating ? "updating" : ""}`}
                    title="監視中の作品の更新および著者・シリーズの新作をチェックしてダウンロード"
                    onClick={handleCheckUpdates}
                    disabled={isUpdating || !hasAnyDownloads || !tauriAvailable}
                  >
                    {isUpdating ? <div className="spinner-mini" /> : <RefreshIcon />}
                  </button>
                  <button
                    className={`toolbar-btn update-confirm ${updateSidebarOpen ? 'active' : ''}`}
                    title={updateSidebarOpen ? "更新管理パネルを閉じる" : "更新管理パネルを開く"}
                    onClick={onToggleUpdateSidebar}
                    disabled={!tauriAvailable}
                  >
                    <PanelRightIcon /> 更新管理
                  </button>
                </>
              ) : isDeleteMode ? (
                <>
                  <div className="epub-selection-group delete-selection-group">
                    <span className={`epub-selection-badge ${selectedDownloadIds.length === 0 ? 'empty' : ''}`}>
                      {selectedDownloadIds.length} 件選択
                    </span>
                    <button 
                      className={`toolbar-btn ${isAllSelected ? "active" : ""}`}
                      title={isAllSelected ? "現在の絞り込み条件に一致する作品の選択を解除" : "現在の絞り込み条件に一致する作品をすべて選択"}
                      onClick={handleToggleSelectAll}
                      disabled={!hasVisibleDownloads}
                    >
                      {isAllSelected ? "全選択解除" : "全選択"}
                    </button>
                  </div>
                  <button 
                    className="toolbar-btn danger-confirm" 
                    title={selectedDownloadIds.length > 0 ? "選択した作品を完全に削除" : "削除する作品を選択してください"} 
                    onClick={handleDeleteSelected}
                    disabled={selectedDownloadIds.length === 0}
                  >
                    <TrashIcon /> 削除確定 ({selectedDownloadIds.length})
                  </button>
                  <button 
                    className="toolbar-btn" 
                    title="削除選択をキャンセル" 
                    onClick={() => {
                      setIsDeleteMode(false);
                      setSelectedDownloadIds([]);
                    }}
                  >
                    キャンセル
                  </button>
                </>
              ) : isUpdateMode ? (
                <>
                  <button 
                    className="toolbar-btn update-confirm" 
                    title="表示中のすべての作品を更新ONにする" 
                    onClick={() => handleSetWatchAllFiltered(true)}
                    disabled={!hasVisibleDownloads || !tauriAvailable}
                  >
                    <SyncIcon /> すべて更新ON
                  </button>
                  <button 
                    className="toolbar-btn update-unwatch" 
                    title="表示中のすべての作品を更新OFFにする" 
                    onClick={() => handleSetWatchAllFiltered(false)}
                    disabled={!hasVisibleDownloads || !tauriAvailable}
                  >
                    <SyncIcon className="update-muted-icon" /> すべて更新OFF
                  </button>
                  <button 
                    className={`toolbar-btn ${isUpdating ? "updating" : ""}`}
                    title="監視中の作品の更新および著者・シリーズの新作をチェックしてダウンロード" 
                    onClick={handleCheckUpdates}
                    disabled={isUpdating || !hasAnyDownloads || !tauriAvailable}
                  >
                    {isUpdating ? <div className="spinner-mini" /> : <RefreshIcon className={isUpdating ? "spinning" : ""} />}
                  </button>
                  <button 
                    className="toolbar-btn" 
                    title="更新モードを終了" 
                    onClick={() => setIsUpdateMode(false)}
                  >
                    完了
                  </button>
                </>
              ) : (
                <>
                  <button
                    className={`toolbar-btn ${isUpdating ? "updating" : ""}`}
                    title="監視中の作品の更新および著者・シリーズの新作をチェックしてダウンロード"
                    onClick={handleCheckUpdates}
                    disabled={isUpdating || !hasAnyDownloads || !tauriAvailable}
                  >
                    <RefreshIcon className={isUpdating ? "spinning" : ""} />
                  </button>
                  <button 
                    className="toolbar-btn" 
                    title="一括削除モードに入る" 
                    onClick={() => setIsDeleteMode(true)}
                    disabled={!hasVisibleDownloads}
                  >
                    <TrashIcon />
                  </button>
                  <div className="backup-dropdown-container" ref={backupRef}>
                    <button 
                      className={`toolbar-btn backup-dropdown-btn ${showBackupDropdown ? 'active' : ''}`}
                      title="バックアップの作成と復元" 
                      onClick={() => setShowBackupDropdown(prev => !prev)}
                      disabled={!tauriAvailable}
                    >
                      <ExportIcon />
                      <span className={`toggle-arrow ${showBackupDropdown ? 'up' : 'down'}`} />
                    </button>
                    {showBackupDropdown && (
                      <div className="backup-dropdown-menu">
                        <button 
                          className="dropdown-item" 
                          title="全データと設定をZIP形式でバックアップ" 
                          onClick={() => {
                            setShowBackupDropdown(false);
                            handleExportZip();
                          }}
                        >
                          <ExportIcon /> バックアップ作成
                        </button>
                        <button 
                          className="dropdown-item" 
                          title="バックアップZIPからデータと設定を復元" 
                          onClick={() => {
                            setShowBackupDropdown(false);
                            handleImportZip();
                          }}
                        >
                          <ArchiveIcon /> バックアップ復元
                        </button>
                      </div>
                    )}
                  </div>
                </>
              )}
            </div>
          </div>
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
                <button 
                  className={`filter-mode-btn ${filterMode === 'and' ? 'active' : ''}`}
                  onClick={() => setFilterMode('and')}
                  title="選択したすべてのタグを含む作品を表示 (AND)"
                >
                  AND
                </button>
                <button 
                  className={`filter-mode-btn ${filterMode === 'or' ? 'active' : ''}`}
                  onClick={() => setFilterMode('or')}
                  title="選択したタグのいずれかを含む作品を表示 (OR)"
                >
                  OR
                </button>
              </div>
            </div>
            {(query || sourceFilter || selectedTagNames.length > 0 || selectedAuthorNames.length > 0 || minCharCount !== "" || maxCharCount !== "" || assetFilter !== "all" || watchFilter !== "all") && (
              <div className="quick-filter-row active-filter-row">
                <span className="quick-filter-label">適用中</span>
                <div className="quick-filter-items active-filter-items">
                  {query && (
                    <span className="quick-filter-chip pinned active" onClick={() => { setSearchVal(""); setQuery(""); }}>
                      検索: {query} ×
                    </span>
                  )}
                  {sourceFilter && (
                    <span className="quick-filter-chip pinned active" onClick={() => setSourceFilter("")}>
                      {sourceFilterLabels[sourceFilter] || sourceFilter} ×
                    </span>
                  )}
                  {selectedTagNames.map(tag => {
                    const mode = tagFilters[tag];
                    return (
                      <span
                        key={`active-tag-${tag}`}
                        className={`quick-filter-chip pinned ${mode}`}
                        onClick={() => handleToggleTagFilter(tag)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === "include" ? "+ " : "- "}#{tag}
                      </span>
                    );
                  })}
                  {selectedAuthorNames.map(author => {
                    const mode = authorFilters[author];
                    return (
                      <span
                        key={`active-author-${author}`}
                        className={`quick-filter-chip pinned ${mode}`}
                        onClick={() => handleToggleAuthorFilter(author)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === "include" ? "+ " : "- "}{author}
                      </span>
                    );
                  })}
                  {(minCharCount !== "" || maxCharCount !== "") && (
                    <span className="quick-filter-chip pinned active" onClick={() => { setMinCharCount(""); setMaxCharCount(""); }}>
                      {minCharCount === "" ? "0" : minCharCount.toLocaleString()}〜{maxCharCount === "" ? "上限なし" : maxCharCount.toLocaleString()}字 ×
                    </span>
                  )}
                  {assetFilter !== "all" && (
                    <span className="quick-filter-chip pinned active" onClick={() => setAssetFilter("all")}>
                      アセット: {assetFilterLabels[assetFilter]} ×
                    </span>
                  )}
                  {watchFilter !== "all" && (
                    <span className="quick-filter-chip pinned active" onClick={() => setWatchFilter("all")}>
                      更新確認: {watchFilter === "watched" ? "ON" : "OFF"} ×
                    </span>
                  )}
                </div>
              </div>
            )}

            {/* メタ属性スマートフィルター - 小説文字数 */}
            <div className="quick-filter-row metadata-filters-row" style={{ flexWrap: 'wrap', gap: '0.5rem' }}>
              <span className="quick-filter-label">小説文字数</span>
              <div className="quick-filter-items" style={{ flexWrap: 'wrap', gap: '4px' }}>
                <span 
                  className={`quick-filter-chip ${minCharCount === "" && maxCharCount === "" ? 'active' : ''}`}
                  onClick={() => { setMinCharCount(""); setMaxCharCount(""); }}
                >
                  すべて
                </span>
                <span 
                  className={`quick-filter-chip ${minCharCount === "" && maxCharCount === 10000 ? 'active' : ''}`}
                  onClick={() => { setMinCharCount(""); setMaxCharCount(10000); }}
                  title="1万字未満の短編小説"
                >
                  1万字未満
                </span>
                <span 
                  className={`quick-filter-chip ${minCharCount === 10000 && maxCharCount === 30000 ? 'active' : ''}`}
                  onClick={() => { setMinCharCount(10000); setMaxCharCount(30000); }}
                  title="1万字以上、3万字未満の中短編"
                >
                  1万〜3万字
                </span>
                <span 
                  className={`quick-filter-chip ${minCharCount === 30000 && maxCharCount === 50000 ? 'active' : ''}`}
                  onClick={() => { setMinCharCount(30000); setMaxCharCount(50000); }}
                  title="3万字以上、5万字未満の中編小説"
                >
                  3万〜5万字
                </span>
                <span 
                  className={`quick-filter-chip ${minCharCount === 50000 && maxCharCount === 100000 ? 'active' : ''}`}
                  onClick={() => { setMinCharCount(50000); setMaxCharCount(100000); }}
                  title="5万字以上、10万字未満の長編小説"
                >
                  5万〜10万字
                </span>
                <span 
                  className={`quick-filter-chip ${minCharCount === 100000 && maxCharCount === "" ? 'active' : ''}`}
                  onClick={() => { setMinCharCount(100000); setMaxCharCount(""); }}
                  title="10万字以上の大長編小説"
                >
                  10万字以上
                </span>
              </div>

              {/* 直接入力フォーム (絵文字不使用) */}
              <div className="char-count-input-group">
                <input 
                  type="number" 
                  className="char-count-number-input" 
                  placeholder="最小" 
                  min="0"
                  value={minCharCount === "" ? "" : minCharCount} 
                  onChange={e => setMinCharCount(e.target.value === "" ? "" : Number(e.target.value))}
                  title="最小文字数を指定"
                />
                <span className="char-count-input-separator">〜</span>
                <input 
                  type="number" 
                  className="char-count-number-input" 
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
            <div className="quick-filter-row metadata-filters-row" style={{ marginTop: '0.25rem' }}>
              <span className="quick-filter-label">その他属性</span>
              <div className="quick-filter-items">
                <span 
                  className={`quick-filter-chip ${assetFilter === 'all' ? 'active' : ''}`}
                  onClick={() => setAssetFilter('all')}
                >
                  アセット: すべて
                </span>
                <span 
                  className={`quick-filter-chip ${assetFilter === 'no_assets' ? 'active' : ''}`}
                  onClick={() => setAssetFilter('no_assets')}
                  title="画像や添付ファイルを含まない作品"
                >
                  テキストのみ
                </span>
                <span 
                  className={`quick-filter-chip ${assetFilter === 'has_images' ? 'active' : ''}`}
                  onClick={() => setAssetFilter('has_images')}
                  title="表紙以外の画像や挿絵を含む作品"
                >
                  画像あり
                </span>
                <span 
                  className={`quick-filter-chip ${assetFilter === 'has_files' ? 'active' : ''}`}
                  onClick={() => setAssetFilter('has_files')}
                  title="画像以外の添付ファイルを含む作品"
                >
                  その他ファイルあり
                </span>
                <span 
                  className={`quick-filter-chip ${assetFilter === 'has_images_and_files' ? 'active' : ''}`}
                  onClick={() => setAssetFilter('has_images_and_files')}
                  title="画像とその他ファイルの両方を含む作品"
                >
                  画像+その他あり
                </span>

                <span style={{ margin: '0 0.5rem', opacity: 0.3, color: 'var(--color-text-secondary)' }}>|</span>

                {/* 更新監視 */}
                <span 
                  className={`quick-filter-chip ${watchFilter === 'all' ? 'active' : ''}`}
                  onClick={() => setWatchFilter('all')}
                >
                  更新確認: すべて
                </span>
                <span 
                  className={`quick-filter-chip ${watchFilter === 'watched' ? 'active' : ''}`}
                  onClick={() => setWatchFilter('watched')}
                  title="更新確認対象（更新ON）の作品"
                >
                  更新ON
                </span>
                <span 
                  className={`quick-filter-chip ${watchFilter === 'unwatched' ? 'active' : ''}`}
                  onClick={() => setWatchFilter('unwatched')}
                  title="更新確認対象外（更新OFF）の作品"
                >
                  更新OFF
                </span>
              </div>
            </div>

            <div className="facet-picker-grid">
              <div className="facet-picker">
                <div className="facet-picker-header">
                  <span className="quick-filter-label">タグ</span>
                  <input
                    className="facet-search-input"
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
                      <span
                        key={facet.name}
                        className={`quick-filter-chip ${mode ? mode : ''}`}
                        onClick={() => handleToggleTagFilter(facet.name)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}#{facet.name}
                        <span className="facet-count">{facet.count}</span>
                      </span>
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
                          <span
                            key={`searched-${facet.name}`}
                            className={`quick-filter-chip ${mode ? mode : ''}`}
                            onClick={() => handleToggleTagFilter(facet.name)}
                            title="クリックで + -> - -> 解除"
                          >
                            {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}#{facet.name}
                            <span className="facet-count">{facet.count}</span>
                          </span>
                        );
                      }) : <span className="facet-empty">一致するタグはありません</span>}
                    </div>
                  </>
                )}
              </div>

              <div className="facet-picker">
                <div className="facet-picker-header">
                  <span className="quick-filter-label">著者</span>
                  <input
                    className="facet-search-input"
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
                      <span
                        key={facet.name}
                        className={`quick-filter-chip ${mode ? mode : ''}`}
                        onClick={() => handleToggleAuthorFilter(facet.name)}
                        title="クリックで + -> - -> 解除"
                      >
                        {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}{facet.name}
                        <span className="facet-count">{facet.count}</span>
                      </span>
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
                          <span
                            key={`searched-${facet.name}`}
                            className={`quick-filter-chip ${mode ? mode : ''}`}
                            onClick={() => handleToggleAuthorFilter(facet.name)}
                            title="クリックで + -> - -> 解除"
                          >
                            {mode === 'include' ? '+ ' : mode === 'exclude' ? '- ' : ''}{facet.name}
                            <span className="facet-count">{facet.count}</span>
                          </span>
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
                <button 
                  type="button"
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
                </button>
              )}
              <button
                type="button"
                className="quick-filter-panel-close"
                onClick={() => setShowFilters(false)}
                title="フィルターパネルを閉じる"
              >
                <span className="toggle-arrow up" />
              </button>
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
      ) : filteredDownloads.length === 0 ? (
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
          <div className="download-grid">
            {filteredDownloads.map(dl => {
            const isChecked = selectedDownloadIds.includes(dl.id);
            const isEpubChecked = isEpubMode && epubSelectedIds?.has(dl.id);
            return (
              <div 
                key={dl.id} 
                className={`download-card ${isEpubMode ? 'epub-mode' : ''} ${isDeleteMode ? 'delete-mode' : ''} ${(isUpdateMode || isUpdatePageMode) ? 'update-mode' : ''} ${isChecked ? 'checked' : ''} ${isEpubChecked ? 'epub-checked' : ''} ${dl.watchUpdates ? 'watch-active update-checked' : 'watch-inactive'}`} 
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
                  {coverCache[dl.id] ? (
                    <img src={coverCache[dl.id]} alt="" />
                  ) : (
                    <div className="cover-placeholder"><ImageIcon /></div>
                  )}
                  <span className={`source-tag ${dl.source}`}>{dl.source === "pixiv" ? "Pixiv" : "FANBOX"}</span>
                  {dl.contentType && (
                    <span className="content-type-tag">
                      {dl.contentType === "novel" ? "小説" : dl.contentType === "article" || dl.contentType === "post" ? "投稿" : dl.contentType.toUpperCase()}
                    </span>
                  )}
                </div>
                <div className="card-body">
                  {dl.seriesId && dl.seriesTitle && (
                    <button
                      type="button"
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
                    </button>
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
                  {dl.matchSnippet && (
                    <p className="card-search-snippet" title={dl.matchSnippet}>
                      {dl.matchFields?.includes("body") ? "本文" : dl.matchFields?.includes("excerpt") ? "概要" : "一致"}: {dl.matchSnippet}
                    </p>
                  )}
                  {parseTags(dl.tags).length > 0 && (
                    <div className="card-tags">
                      {parseTags(dl.tags)
                        .slice(0, 3)
                        .map((tag, idx) => {
                          const mode = tagFilters[tag];
                          return (
                            <span 
                              key={idx} 
                              className={`tag-badge ${mode === 'include' ? 'active' : mode === 'exclude' ? 'exclude' : ''}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                handleToggleTagFilter(tag);
                              }}
                              title={`タグ「#${tag}」をフィルター（+ / - / 解除）`}
                            >
                              {mode === 'include' ? '＋ ' : mode === 'exclude' ? '－ ' : ''}#{tag}
                            </span>
                          );
                        })
                      }
                      {parseTags(dl.tags).length > 3 && (
                        <span 
                          className="tag-badge more-badge"
                          onClick={(e) => e.stopPropagation()}
                        >
                          +{parseTags(dl.tags).length - 3}
                          <div className="tags-tooltip">
                            <div className="tags-tooltip-title">すべてのタグ ({parseTags(dl.tags).length})</div>
                            <div className="tags-tooltip-list">
                              {parseTags(dl.tags).map((tag, idx) => {
                                const mode = tagFilters[tag];
                                return (
                                  <span 
                                    key={idx} 
                                    className={`tooltip-tag-badge ${mode === 'include' ? 'active' : mode === 'exclude' ? 'exclude' : ''}`}
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleToggleTagFilter(tag);
                                    }}
                                    title={`タグ「#${tag}」をフィルター（+ / - / 解除）`}
                                  >
                                    {mode === 'include' ? '＋ ' : mode === 'exclude' ? '－ ' : ''}#{tag}
                                  </span>
                                );
                              })}
                            </div>
                          </div>
                        </span>
                      )}
                    </div>
                  )}
                </div>
              </div>
            );
            })}
          </div>
          {(loadingMore || hasMoreDownloads) && (
            <div className="library-load-more">
              {loadingMore ? (
                <>
                  <div className="spinner" />
                  <span>追加読み込み中...</span>
                </>
              ) : (
                <button
                  className="toolbar-btn"
                  onClick={() => fetchDataPage(filteredDownloads.length, { replace: false, preferCache: false })}
                >
                  さらに読み込む
                </button>
              )}
            </div>
          )}
        </>
      )}
      
      {showScrollTop && (
        <button 
          className={`scroll-to-top-btn ${epubSidebarOpen ? 'sidebar-open' : ''}`}
          onClick={scrollToTop}
          title="最上部へスクロール"
        >
          <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="18 15 12 9 6 15" />
          </svg>
        </button>
      )}
    </div>

    </>
  );
}
