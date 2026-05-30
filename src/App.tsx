import { useState, useEffect, useRef, useCallback } from "react";
import { AuthSettings } from "./components/AuthSettings";
import { LibraryUiState, LibraryView } from "./components/library/LibraryView";
import { PersonView, SeriesView } from "./components/library/EntityViews";
import { ContentViewer } from "./components/viewer/ContentViewer";
import { EpubSidebar } from "./components/epub/EpubSidebar";
import { UpdateSidebar } from "./components/update/UpdateSidebar";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { store } from "./store";
import "./App.css";
import logo from "./assets/piep.svg";
import {
  HomeIcon, PaletteIcon, HeartIcon, SettingsIcon, LibraryIcon, BookIcon,
  ChevronLeftIcon, ChevronRightIcon, RefreshIcon, DownloadIcon,
  AlertIcon, TerminalAlertIcon,
} from "./components/icons/Icons";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type ViewMode = "home" | "pixiv" | "fanbox" | "library" | "epub" | "update" | "settings";
type LibraryEntityView = { type: "person" | "series"; source: string; sourceKey: string } | null;

interface AppHistoryState {
  piep: true;
  viewMode: ViewMode;
  viewingDownloadId: number | null;
  libraryEntityView: LibraryEntityView;
  libraryFilter: string;
  libraryUiState: LibraryUiState;
}

interface ToastMessage {
  id: number;
  text: string;
  type: "success" | "error" | "info";
}

interface DbStats {
  totalDownloads: number;
  pixivCount: number;
  fanboxCount: number;
  totalAssets: number;
  totalSizeBytes: number;
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
  watchUpdates: boolean;
  excerpt?: string | null;
  localCoverPath?: string | null;
  localAssetsDir?: string | null;
  downloadedAt?: string;
  sourceCreatedAt?: string | null;
  personId?: string | null;
  seriesId?: string | null;
}

interface SavedSourceTarget {
  source: "pixiv" | "fanbox";
  sourceId: string;
}

interface PixivUser {
  id: string;
  name: string;
}

interface PixivSeriesNavigation {
  seriesId: string;
  seriesTitle: string;
}

interface PixivTag {
  name: string;
}

interface PixivNovel {
  id: string;
  title: string;
  characterCount: number;
  createDate: string;
  user: PixivUser;
  seriesNavigation?: PixivSeriesNavigation;
  body?: string;
  cover_url?: string;
  tags?: (PixivTag | string)[];
  detail?: {
    id: string;
    title: string;
    user: PixivUser;
    cover_url?: string;
    seriesNavigation?: PixivSeriesNavigation;
    tags?: { tags: PixivTag[] } | PixivTag[] | string[];
  };
}

interface FanboxUser {
  userId: string;
  name: string;
}

interface FanboxPost {
  id: string;
  title: string;
  type: string;
  publishedDatetime: string;
  user: FanboxUser;
  body?: any;
  coverImageUrl?: string;
  tags?: string[];
  creatorId?: string;
}

interface SidebarItem {
  id: string;
  title: string;
  subtitle?: string;
  selected: boolean;
  originalData: PixivNovel | FanboxPost;
  status?: "pending" | "downloading" | "success" | "skipped" | "failed";
}

type SidebarDownloadType = "pixiv_single" | "pixiv_series" | "pixiv_user" | "fanbox_single" | "fanbox_creator";
type SidebarMode = "empty" | "loading" | "analysis" | "downloadProgress" | "downloadDone";
type DownloadTargetKind = SidebarDownloadType | "unsupported";

interface SidebarAnalysisState {
  sourceUrl: string;
  title: string;
  items: SidebarItem[];
  downloadType: SidebarDownloadType;
  analyzedAt: number;
}

const EPUB_SIDEBAR_TRANSITION_MS = 220;

const defaultLibraryUiState: LibraryUiState = {
  searchVal: "",
  sourceFilter: "",
  sortBy: "published",
  sortOrder: "desc",
  tagFilters: {},
  authorFilters: {},
  filterMode: "and",
  minCharCount: "",
  maxCharCount: "",
  assetFilter: "all",
  watchFilter: "all",
  showFilters: false,
  scrollTop: 0,
};


function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function normalizeContentLinkUrl(url: string): string {
  return url
    .replace(/pixiv:\/\/illusts?\/(\d+)/g, "https://www.pixiv.net/artworks/$1")
    .replace(/pixiv:\/\/novels?\/(\d+)/g, "https://www.pixiv.net/novel/show.php?id=$1")
    .replace(/pixiv:\/\/users?\/(\d+)/g, "https://www.pixiv.net/users/$1");
}

function extractSavedSourceTarget(url: string): SavedSourceTarget | null {
  const normalized = normalizeContentLinkUrl(url);
  const pixivNovelMatch = normalized.match(/pixiv\.net\/(?:[a-z]{2}\/)?novels\/(\d+)/)
    || normalized.match(/pixiv\.net\/(?:[a-z]{2}\/)?novel\/show\.php\?id=(\d+)/);
  if (pixivNovelMatch?.[1]) {
    return { source: "pixiv", sourceId: pixivNovelMatch[1] };
  }

  const fanboxPostMatch = normalized.match(/fanbox\.cc\/(?:@[^/]+\/)?posts\/(\d+)/);
  if (fanboxPostMatch?.[1]) {
    return { source: "fanbox", sourceId: fanboxPostMatch[1] };
  }

  return null;
}

// ---------------------------------------------------------------------------
// App Component
// ---------------------------------------------------------------------------

function App() {
  const tauriAvailable = isTauriRuntime();
  const [viewMode, setViewMode] = useState<ViewMode>("home");
  const [currentUrl, setCurrentUrl] = useState("");
  const [isDownloading, setIsDownloading] = useState(false);
  const [toasts, setToasts] = useState<ToastMessage[]>([]);
  const [isPixivAuthed, setIsPixivAuthed] = useState(false);
  const [isFanboxAuthed, setIsFanboxAuthed] = useState(false);
  const [viewingDownloadId, setViewingDownloadId] = useState<number | null>(null);
  const [libraryEntityView, setLibraryEntityView] = useState<LibraryEntityView>(null);
  const [logs, setLogs] = useState<{ time: string; type: "info" | "success" | "warn" | "error"; text: string }[]>([]);
  const [showConsole, setShowConsole] = useState(false);
  const [stats, setStats] = useState<DbStats | null>(null);
  const [libraryFilter, setLibraryFilter] = useState<string>("all");
  const [libraryUiState, setLibraryUiState] = useState<LibraryUiState>(defaultLibraryUiState);
  const [libraryRestoreKey, setLibraryRestoreKey] = useState(0);
  const [selectedEpubIds, setSelectedEpubIds] = useState<Set<number>>(new Set());
  const [selectedEpubItems, setSelectedEpubItems] = useState<any[]>([]);
  const [epubSidebarOpen, setEpubSidebarOpen] = useState(false);
  const [epubSidebarLayoutOpen, setEpubSidebarLayoutOpen] = useState(false);
  const [updateSidebarOpen, setUpdateSidebarOpen] = useState(false);
  const [updateSidebarLayoutOpen, setUpdateSidebarLayoutOpen] = useState(false);
  const [updateSidebarRefreshKey, setUpdateSidebarRefreshKey] = useState(0);
  const [showTemplateManager, setShowTemplateManager] = useState(false);

  // 詳細画面からライブラリ画面へ引き渡すための初期フィルター状態
  const [initialTagFilters, setInitialTagFilters] = useState<Record<string, "include" | "exclude"> | undefined>(undefined);
  const [initialAuthorFilters, setInitialAuthorFilters] = useState<Record<string, "include" | "exclude"> | undefined>(undefined);

  // ダウンロード選択サイドバー用ステート
  const [showDownloadSidebar, setShowDownloadSidebar] = useState(false);
  const [sidebarTitle, setSidebarTitle] = useState("");
  const [sidebarLoading, setSidebarLoading] = useState(false);
  const [sidebarItems, setSidebarItems] = useState<SidebarItem[]>([]);
  const [sidebarProgress, setSidebarProgress] = useState<{ current: number; total: number } | null>(null);
  const [sidebarDownloadType, setSidebarDownloadType] = useState<SidebarDownloadType | null>(null);
  const [sidebarStatusText, setSidebarStatusText] = useState("");
  const [sidebarMode, setSidebarMode] = useState<SidebarMode>("empty");
  const [sidebarEmptyMessage, setSidebarEmptyMessage] = useState("保存候補はありません。");
  const [lastSidebarAnalysis, setLastSidebarAnalysis] = useState<SidebarAnalysisState | null>(null);
  const [isUserScrollingUp, setIsUserScrollingUp] = useState(false);

  const browserRef = useRef<HTMLDivElement>(null);
  const prevViewRef = useRef<ViewMode>("home");
  const toastIdRef = useRef(0);
  const consoleBottomRef = useRef<HTMLDivElement>(null);
  const sharedScrollRef = useRef<number>(0);
  const [isRestoringScroll, setIsRestoringScroll] = useState(false);
  const pendingBrowserUrlRef = useRef<string | null>(null);
  const epubSidebarCloseTimerRef = useRef<number | null>(null);
  const lastMouseHistoryAtRef = useRef(0);

  const setEpubSidebarVisible = useCallback((open: boolean) => {
    if (epubSidebarCloseTimerRef.current !== null) {
      window.clearTimeout(epubSidebarCloseTimerRef.current);
      epubSidebarCloseTimerRef.current = null;
    }

    if (open) {
      setEpubSidebarLayoutOpen(true);
      setEpubSidebarOpen(true);
      return;
    }

    setEpubSidebarOpen(false);
    epubSidebarCloseTimerRef.current = window.setTimeout(() => {
      setEpubSidebarLayoutOpen(false);
      epubSidebarCloseTimerRef.current = null;
    }, EPUB_SIDEBAR_TRANSITION_MS);
  }, []);

  const setUpdateSidebarVisible = useCallback((open: boolean) => {
    if (open) {
      setUpdateSidebarLayoutOpen(true);
      setUpdateSidebarOpen(true);
      return;
    }
    setUpdateSidebarOpen(false);
    window.setTimeout(() => setUpdateSidebarLayoutOpen(false), EPUB_SIDEBAR_TRANSITION_MS);
  }, []);

  useEffect(() => {
    return () => {
      if (epubSidebarCloseTimerRef.current !== null) {
        window.clearTimeout(epubSidebarCloseTimerRef.current);
      }
    };
  }, []);

  // ---------------------------------------------------------------------------
  // Ref-based Navigation Router
  // navStateRef always holds the latest navigation state.
  // All router functions read from this ref (not closures), so they are
  // referentially stable regardless of state changes.
  // ---------------------------------------------------------------------------

  const navStateRef = useRef({
    viewMode: "home" as ViewMode,
    viewingDownloadId: null as number | null,
    libraryEntityView: null as LibraryEntityView,
    libraryFilter: "all",
    libraryUiState: defaultLibraryUiState,
  });

  // Snapshot: read current nav state from ref + live DOM scroll position
  const getSnapshot = useCallback((): AppHistoryState => {
    const s = navStateRef.current;
    const selector = s.viewMode === "epub" ? ".epub-main-content"
      : s.viewMode === "update" ? ".update-main-content"
      : ".library-main-content";
    const container = document.querySelector(selector);
    const scrollTop = container?.scrollTop ?? s.libraryUiState.scrollTop;
    return {
      piep: true,
      viewMode: s.viewMode,
      viewingDownloadId: s.viewingDownloadId,
      libraryEntityView: s.libraryEntityView,
      libraryFilter: s.libraryFilter,
      libraryUiState: { ...s.libraryUiState, scrollTop },
    };
  }, []);

  // Apply: write history state to ref + React state (triggers re-render)
  const applyNavState = useCallback((state: AppHistoryState) => {
    navStateRef.current = {
      viewMode: state.viewMode,
      viewingDownloadId: state.viewingDownloadId,
      libraryEntityView: state.libraryEntityView ?? null,
      libraryFilter: state.libraryFilter ?? "all",
      libraryUiState: { ...defaultLibraryUiState, ...state.libraryUiState },
    };
    setViewMode(state.viewMode);
    setViewingDownloadId(state.viewingDownloadId);
    setLibraryEntityView(state.libraryEntityView ?? null);
    setLibraryFilter(state.libraryFilter ?? "all");
    setLibraryUiState({ ...defaultLibraryUiState, ...state.libraryUiState });
    setLibraryRestoreKey(k => k + 1);
    sharedScrollRef.current = state.libraryUiState?.scrollTop ?? 0;
    setIsRestoringScroll(true);
  }, []);

  // Navigate: the single entry point for all forward navigation
  const navigateTo = useCallback((updates: Partial<AppHistoryState>) => {
    // Save current state (with live DOM scroll) to current history entry
    const current = getSnapshot();
    window.history.replaceState(current, "", window.location.href);
    // Build next state by merging updates over current
    const next: AppHistoryState = {
      piep: true,
      viewMode: updates.hasOwnProperty("viewMode") ? updates.viewMode! : current.viewMode,
      viewingDownloadId: updates.hasOwnProperty("viewingDownloadId") ? updates.viewingDownloadId! : current.viewingDownloadId,
      libraryEntityView: updates.hasOwnProperty("libraryEntityView") ? updates.libraryEntityView! : current.libraryEntityView,
      libraryFilter: updates.hasOwnProperty("libraryFilter") ? updates.libraryFilter! : current.libraryFilter,
      libraryUiState: updates.hasOwnProperty("libraryUiState") ? updates.libraryUiState! : current.libraryUiState,
    };
    window.history.pushState(next, "", window.location.href);
    applyNavState(next);
  }, [getSnapshot, applyNavState]);

  const handleLibraryUiStateChange = useCallback((state: LibraryUiState) => {
    setLibraryUiState(prev => {
      if (JSON.stringify(prev) === JSON.stringify(state)) {
        return prev;
      }
      navStateRef.current = { ...navStateRef.current, libraryUiState: state };
      const histState: AppHistoryState = { piep: true, ...navStateRef.current };
      window.history.replaceState(histState, "", window.location.href);
      return state;
    });
  }, []);

  useEffect(() => {
    const handlePopState = (event: PopStateEvent) => {
      const state = event.state as AppHistoryState | null;
      if (state && state.piep) {
        applyNavState(state);
      }
    };
    window.addEventListener("popstate", handlePopState);
    return () => {
      window.removeEventListener("popstate", handlePopState);
    };
  }, [applyNavState]);

  const historyInitializedRef = useRef(false);
  useEffect(() => {
    if (historyInitializedRef.current) return;
    historyInitializedRef.current = true;
    if (window.history.state && window.history.state.piep) {
      applyNavState(window.history.state);
    } else {
      window.history.replaceState(getSnapshot(), "", window.location.href);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const handleMouseHistoryButton = (event: MouseEvent) => {
      if (event.button !== 3 && event.button !== 4) return;
      event.preventDefault();
      event.stopPropagation();
      const now = Date.now();
      if (now - lastMouseHistoryAtRef.current < 120) return;
      lastMouseHistoryAtRef.current = now;
      if (event.button === 3) {
        window.history.back();
      } else {
        window.history.forward();
      }
    };

    window.addEventListener("mouseup", handleMouseHistoryButton, true);
    window.addEventListener("auxclick", handleMouseHistoryButton, true);
    return () => {
      window.removeEventListener("mouseup", handleMouseHistoryButton, true);
      window.removeEventListener("auxclick", handleMouseHistoryButton, true);
    };
  }, []);

  const handleViewDetail = useCallback((id: number) => {
    navigateTo({
      viewingDownloadId: id,
      libraryEntityView: null,
    });
  }, [navigateTo]);

  const handleBackToLibrary = useCallback(() => {
    navigateTo({
      viewingDownloadId: null,
      libraryEntityView: null,
    });
  }, [navigateTo]);

  const handleViewPerson = useCallback((source: string, sourceKey: string) => {
    navigateTo({
      viewMode: "library",
      viewingDownloadId: null,
      libraryEntityView: { type: "person" as const, source, sourceKey },
      libraryUiState: { ...navStateRef.current.libraryUiState, scrollTop: 0 },
    });
  }, [navigateTo]);

  const handleViewSeries = useCallback((source: string, sourceKey: string) => {
    navigateTo({
      viewMode: "library",
      viewingDownloadId: null,
      libraryEntityView: { type: "series" as const, source, sourceKey },
      libraryUiState: { ...navStateRef.current.libraryUiState, scrollTop: 0 },
    });
  }, [navigateTo]);

  const handleOpenEntityEpub = useCallback((works: any[]) => {
    const ids = new Set(works.map(work => work.id));
    const isSameSelection = ids.size === selectedEpubIds.size && Array.from(ids).every(id => selectedEpubIds.has(id));
    if (epubSidebarOpen && isSameSelection) {
      setEpubSidebarVisible(false);
      return;
    }
    setSelectedEpubIds(ids);
    setSelectedEpubItems(works as any[]);
    setEpubSidebarVisible(true);
  }, [epubSidebarOpen, selectedEpubIds, setEpubSidebarVisible]);

  useEffect(() => {
    if (viewMode === "library" || viewMode === "epub" || viewMode === "update") {
      const timer = setTimeout(() => {
        const selector = viewMode === "epub" ? ".epub-main-content" : viewMode === "update" ? ".update-main-content" : ".library-main-content";
        const container = document.querySelector(selector);
        if (container) {
          container.scrollTop = sharedScrollRef.current;
        }
        setIsRestoringScroll(false);
      }, 50);
      return () => clearTimeout(timer);
    } else {
      setIsRestoringScroll(false);
    }
    // libraryEntityView and libraryRestoreKey are critical here because navigating back/forward
    // between library list and entity (person/series) views does not change viewMode/viewingDownloadId,
    // but we must still restore the scroll position and clear isRestoringScroll to prevent blank pages.
  }, [viewMode, viewingDownloadId, libraryEntityView, libraryRestoreKey]);

  // Toast helper
  const showToast = useCallback((text: string, type: ToastMessage["type"] = "info") => {
    const id = ++toastIdRef.current;
    const maxLength = 300;
    const truncatedText = text.length > maxLength ? text.substring(0, maxLength) + "..." : text;
    setToasts(prev => [...prev, { id, text: truncatedText, type }]);

    // 同時に LOGS パネル（コンソールログ）にもタイムスタンプ付きで出力
    const now = new Date();
    const timeStr = `${now.getHours().toString().padStart(2, "0")}:${now.getMinutes().toString().padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
    const logType = type === "error" ? "error" : type === "success" ? "success" : "info";
    setLogs(prev => [...prev.slice(-499), { time: timeStr, type: logType, text }]);

    setTimeout(() => {
      setToasts(prev => prev.filter(t => t.id !== id));
    }, 5000);
  }, []);

  const handleNavigateContentUrl = useCallback(async (url: string) => {
    const normalizedUrl = normalizeContentLinkUrl(url);
    const savedTarget = extractSavedSourceTarget(normalizedUrl);

    if (savedTarget) {
      try {
        const savedDownload = await invoke<DownloadEntry | null>("db_get_download_by_source", {
          source: savedTarget.source,
          sourceId: savedTarget.sourceId,
        });
        if (savedDownload) {
          navigateTo({
            viewingDownloadId: savedDownload.id,
            libraryEntityView: null,
          });
          showToast("保存済みの作品を開きました", "info");
          return;
        }
      } catch (e) {
        console.warn("Failed to resolve saved content link:", e);
      }
    }

    pendingBrowserUrlRef.current = normalizedUrl;
    const isPixiv = normalizedUrl.includes("pixiv.net");
    const nextMode = isPixiv ? "pixiv" : "fanbox";

    if (navStateRef.current.viewMode === nextMode) {
      invoke("navigate_embedded_browser", { url: normalizedUrl }).catch((e) => {
        console.warn("Failed to navigate embedded browser directly:", e);
      });
      pendingBrowserUrlRef.current = null;
    }

    navigateTo({
      viewMode: nextMode,
      viewingDownloadId: null,
      libraryEntityView: null,
    });
    showToast("アプリ内ブラウザで開きます", "info");
  }, [showToast, navigateTo]);

  const handleOpenUrlInAppBrowser = useCallback((url: string) => {
    const normalizedUrl = normalizeContentLinkUrl(url);
    pendingBrowserUrlRef.current = normalizedUrl;
    const isPixiv = normalizedUrl.includes("pixiv.net");
    const nextMode = isPixiv ? "pixiv" : "fanbox";

    if (navStateRef.current.viewMode === nextMode) {
      invoke("navigate_embedded_browser", { url: normalizedUrl }).catch((e) => {
        console.warn("Failed to navigate embedded browser directly:", e);
      });
      pendingBrowserUrlRef.current = null;
    }

    navigateTo({
      viewMode: nextMode,
      viewingDownloadId: null,
      libraryEntityView: null,
    });
    showToast("アプリ内ブラウザで開きます", "info");
  }, [showToast, navigateTo]);

  const normalizeUrlForSidebar = useCallback((url: string) => {
    try {
      const parsed = new URL(url);
      parsed.hash = "";
      parsed.pathname = parsed.pathname.replace(/\/+$/, "") || "/";
      return parsed.toString();
    } catch {
      return url.trim();
    }
  }, []);

  const isSameSidebarUrl = useCallback((a?: string | null, b?: string | null) => {
    if (!a || !b) return false;
    return normalizeUrlForSidebar(a) === normalizeUrlForSidebar(b);
  }, [normalizeUrlForSidebar]);

  const detectDownloadTarget = useCallback((url: string): DownloadTargetKind => {
    if (!url) return "unsupported";
    const isPixiv = url.includes("pixiv.net");
    const isFanbox = url.includes("fanbox.cc");

    if (isPixiv) {
      if (url.match(/novel\/series\/(\d+)/) || url.match(/novel\/series\/show\.php\?id=(\d+)/)) return "pixiv_series";
      const userIdMatch = url.match(/users\/(\d+)/);
      if (userIdMatch?.[1] && !url.includes("/novels/")) return "pixiv_user";
      if (url.match(/novels\/(\d+)/) || url.match(/novel\/show\.php\?id=(\d+)/)) return "pixiv_single";
    }

    if (isFanbox) {
      if (url.match(/posts\/(\d+)/)) return "fanbox_single";
      if (getFanboxCreatorId(url)) return "fanbox_creator";
    }

    return "unsupported";
  }, []);

  const stripSidebarItemStatus = useCallback((items: SidebarItem[]) => {
    return items.map(({ status: _status, ...item }) => ({ ...item }));
  }, []);

  const openEmptyDownloadSidebar = useCallback((message: string, title = "保存候補") => {
    setSidebarTitle(title);
    setSidebarItems([]);
    setSidebarProgress(null);
    setSidebarStatusText("");
    setSidebarDownloadType(null);
    setSidebarLoading(false);
    setSidebarEmptyMessage(message);
    setSidebarMode("empty");
    setShowDownloadSidebar(true);
  }, []);

  const openCachedDownloadSidebar = useCallback((analysis: SidebarAnalysisState) => {
    setSidebarTitle(analysis.title);
    setSidebarItems(stripSidebarItemStatus(analysis.items));
    setSidebarProgress(null);
    setSidebarStatusText("");
    setSidebarDownloadType(analysis.downloadType);
    setSidebarLoading(false);
    setSidebarEmptyMessage("");
    setSidebarMode("analysis");
    setShowDownloadSidebar(true);
  }, [stripSidebarItemStatus]);

  const applyDownloadAnalysis = useCallback((
    sourceUrl: string,
    title: string,
    downloadType: SidebarDownloadType,
    items: SidebarItem[],
  ) => {
    const cleanItems = stripSidebarItemStatus(items);
    setSidebarTitle(title);
    setSidebarItems(cleanItems);
    setSidebarProgress(null);
    setSidebarStatusText("");
    setSidebarDownloadType(downloadType);
    setSidebarLoading(false);
    setSidebarEmptyMessage("");
    setSidebarMode("analysis");
    setShowDownloadSidebar(true);
    setLastSidebarAnalysis({
      sourceUrl,
      title,
      items: cleanItems,
      downloadType,
      analyzedAt: Date.now(),
    });
  }, [stripSidebarItemStatus]);

  // Check auth status on mount and view changes
  useEffect(() => {
    const checkAuth = async () => {
      if (!isTauriRuntime()) {
        setIsPixivAuthed(false);
        setIsFanboxAuthed(false);
        return;
      }

      try {
        const pixivToken = await store.get<string>("pixiv_refresh_token");
        setIsPixivAuthed(!!pixivToken);
        const fanboxToken = await store.get<string>("fanbox_session_id");
        setIsFanboxAuthed(!!fanboxToken);
      } catch (e) {
        console.error("Failed to load auth status:", e);
      }
    };
    checkAuth();
  }, [viewMode]);

  const loadStats = useCallback(async () => {
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
      const dbStats = await invoke<DbStats>("db_get_stats");
      setStats(dbStats);
    } catch (e) {
      console.error("Failed to load db stats:", e);
    }
  }, []);

  // Load db stats when returning to home dashboard
  useEffect(() => {
    if (viewMode === "home") {
      loadStats();
    }
  }, [loadStats, viewMode]);

  // Listen for URL changes from the embedded browser
  useEffect(() => {
    if (!isTauriRuntime()) return;

    const unlisten = listen<string>("url-changed", (event) => {
      setCurrentUrl(event.payload);
    });
    return () => { unlisten.then(f => f()).catch(() => {}); };
  }, []);

  // Listen for backend download logs
  useEffect(() => {
    if (!isTauriRuntime()) return;

    const unlisten = listen<string>("download-log", (event) => {
      const payload = event.payload;
      let type: "info" | "success" | "warn" | "error" = "info";
      let text = payload;
      
      if (payload.startsWith("[SUCCESS]")) {
        type = "success";
        text = payload.replace("[SUCCESS] ", "");
      } else if (payload.startsWith("[WARN]")) {
        type = "warn";
        text = payload.replace("[WARN] ", "");
      } else if (payload.startsWith("[ERROR]")) {
        type = "error";
        text = payload.replace("[ERROR] ", "");
      } else if (payload.startsWith("[INFO]")) {
        type = "info";
        text = payload.replace("[INFO] ", "");
      }
      
      const now = new Date();
      const timeStr = `${now.getHours().toString().padStart(2, "0")}:${now.getMinutes().toString().padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
      
      setLogs(prev => [...prev.slice(-499), { time: timeStr, type, text }]);
    });
    
    return () => { unlisten.then(f => f()).catch(() => {}); };
  }, []);

  // Auto-scroll console
  useEffect(() => {
    if (showConsole && consoleBottomRef.current && !isUserScrollingUp) {
      consoleBottomRef.current.scrollIntoView({ behavior: "smooth" });
    }
  }, [logs, showConsole, isUserScrollingUp]);

  // URL同期ポーリング（外部サイトのSPA遷移およびセキュリティ境界を安全に越えてURLを追従させる生命線タイマー）
  useEffect(() => {
    if (viewMode !== "pixiv" && viewMode !== "fanbox") return;
    if (!tauriAvailable) return;

    const syncUrl = async () => {
      try {
        const url = await invoke<string>("get_embedded_browser_url");
        if (url && url !== currentUrl) {
          setCurrentUrl(url);
        }
      } catch {
        // ブラウザ起動前や破棄中のエラーは安全に無視
      }
    };

    syncUrl();
    const intervalId = window.setInterval(syncUrl, 1000);
    return () => window.clearInterval(intervalId);
  }, [tauriAvailable, viewMode, currentUrl]);

  useEffect(() => {
    if (viewMode !== "pixiv" && viewMode !== "fanbox") return;
    if (!currentUrl || isDownloading) return;

    if (sidebarMode === "downloadProgress" || sidebarMode === "downloadDone") return;
    setSidebarProgress(null);
    setSidebarStatusText("");
  }, [currentUrl, viewMode, isDownloading, sidebarMode]);

  // Manage embedded browser lifecycle
  const updateBrowserBounds = useCallback(() => {
    if (!tauriAvailable) return;

    if ((viewMode === "pixiv" || viewMode === "fanbox") && browserRef.current) {
      const rect = browserRef.current.getBoundingClientRect();
      
      // 保留中のURLがあればそれを使う
      let targetUrl = pendingBrowserUrlRef.current;
      if (targetUrl) {
        const isPixivUrl = targetUrl.includes("pixiv.net");
        const isTargetMatch = (viewMode === "pixiv" && isPixivUrl) || (viewMode === "fanbox" && !isPixivUrl);
        if (!isTargetMatch) {
          targetUrl = null;
        }
      }
      
      const initialUrl = targetUrl || (viewMode === "pixiv" ? "https://www.pixiv.net" : "https://www.fanbox.cc");
      pendingBrowserUrlRef.current = null;

      invoke("open_embedded_browser", {
        url: initialUrl, x: Math.round(rect.x), y: Math.round(rect.y),
        width: Math.round(rect.width), height: Math.round(rect.height), userAgent: undefined,
      });
    }
  }, [tauriAvailable, viewMode]);

  useEffect(() => {
    let timer: number;
    const debouncedUpdate = () => { clearTimeout(timer); timer = window.setTimeout(updateBrowserBounds, 50); };

    if (viewMode === "pixiv" || viewMode === "fanbox") {
      if (!tauriAvailable) {
        prevViewRef.current = viewMode;
        return;
      }

      const prevWasBrowser = prevViewRef.current === "pixiv" || prevViewRef.current === "fanbox";
      if (prevWasBrowser && prevViewRef.current !== viewMode) {
        // 保留中のURLがあればそれを使う。なければデフォルト
        const targetUrl = pendingBrowserUrlRef.current;
        const initialUrl = targetUrl || (viewMode === "pixiv" ? "https://www.pixiv.net" : "https://www.fanbox.cc");
        pendingBrowserUrlRef.current = null;

        invoke("navigate_embedded_browser", { url: initialUrl }).catch(() => updateBrowserBounds());
        updateBrowserBounds();
      } else {
        updateBrowserBounds();
      }
      window.addEventListener("resize", debouncedUpdate);
      prevViewRef.current = viewMode;
      return () => { window.removeEventListener("resize", debouncedUpdate); clearTimeout(timer); };
    } else {
      if (tauriAvailable && (prevViewRef.current === "pixiv" || prevViewRef.current === "fanbox")) {
        invoke("destroy_embedded_browser");
      }
      setCurrentUrl("");
      prevViewRef.current = viewMode;
    }
  }, [tauriAvailable, viewMode, updateBrowserBounds]);

  // サイドバーやコンソール表示、および表示モードが変わった際にもブラウザの表示境界を再計算・追従させる
  useEffect(() => {
    if (viewMode === "pixiv" || viewMode === "fanbox") {
      // パネルのアニメーション（すべり出し）完了に合わせて正確に再追従させるため、即時および遅延を入れて3段階調整
      updateBrowserBounds();
      const t1 = setTimeout(updateBrowserBounds, 100);
      const t2 = setTimeout(updateBrowserBounds, 250);
      return () => {
        clearTimeout(t1);
        clearTimeout(t2);
      };
    }
  }, [showDownloadSidebar, showConsole, viewMode, updateBrowserBounds]);

  const getFanboxCreatorId = (url: string) => {
    try {
      const subMatch = url.match(/https:\/\/([^.]+)\.fanbox\.cc/);
      if (subMatch && subMatch[1] !== "www" && subMatch[1] !== "api") return subMatch[1];
      const dirMatch = url.match(/fanbox\.cc\/@([^/?#\s]+)/);
      if (dirMatch) return dirMatch[1];
    } catch {}
    return null;
  };

  // DB管理ダウンロード（サイドバー表示フェーズへリニューアル）
  const handleDownload = async () => {
    if (isDownloading) return;
    if (!tauriAvailable) {
      openEmptyDownloadSidebar("保存候補の取得はTauriアプリ内でのみ利用できます。", "保存候補");
      return;
    }

    let activeUrl = "";
    try {
      activeUrl = await invoke<string>("get_embedded_browser_url");
    } catch {
      activeUrl = "";
    }

    if (!activeUrl) {
      if (lastSidebarAnalysis) {
        openCachedDownloadSidebar(lastSidebarAnalysis);
        showToast("現在のURLを取得できないため、前回の保存候補を表示しました。", "info");
      } else {
        openEmptyDownloadSidebar("現在のページから保存候補を取得できませんでした。小説ページ、シリーズ、作家、投稿、またはクリエイターページで再度お試しください。");
        showToast("ブラウザのURLを取得できませんでした。", "info");
      }
      return;
    }

    const normalizedActiveUrl = normalizeUrlForSidebar(activeUrl);
    const targetKind = detectDownloadTarget(activeUrl);

    if (lastSidebarAnalysis && isSameSidebarUrl(activeUrl, lastSidebarAnalysis.sourceUrl)) {
      openCachedDownloadSidebar(lastSidebarAnalysis);
      return;
    }

    if (targetKind === "unsupported") {
      if (lastSidebarAnalysis) {
        openCachedDownloadSidebar(lastSidebarAnalysis);
        showToast("このページは解析対象外のため、前回の保存候補を表示しました。", "info");
      } else {
        openEmptyDownloadSidebar("このページには保存できる小説・投稿が見つかりません。");
      }
      return;
    }

    showToast("URLを解析中...", "info");
    setSidebarTitle("URLを解析中...");
    setSidebarItems([]);
    setSidebarProgress(null);
    setSidebarStatusText("");
    setSidebarDownloadType(null);
    setSidebarLoading(true);
    setSidebarEmptyMessage("");
    setSidebarMode("loading");
    setShowDownloadSidebar(true);

    try {
      if (!activeUrl) {
        showToast("ブラウザのURLを取得できませんでした。", "error");
        return;
      }

      const isPixiv = activeUrl.includes("pixiv.net");
      const isFanbox = activeUrl.includes("fanbox.cc");

      if (!isPixiv && !isFanbox) {
        showToast("保存可能なコンテンツ（Pixiv小説、シリーズ、作家、またはFANBOX投稿、クリエイター）が検出されませんでした。", "info");
        return;
      }

      if (isPixiv) {
        if (!isPixivAuthed) {
          throw new Error("Pixiv連携が未設定です。設定画面から連携してください。");
        }

        const refreshToken = await store.get<string>("pixiv_refresh_token") || "";

        // A. Pixiv シリーズ小説
        const seriesIdMatch = activeUrl.match(/novel\/series\/(\d+)/) || activeUrl.match(/novel\/series\/show\.php\?id=(\d+)/);
        const seriesId = seriesIdMatch?.[1];
        if (seriesId) {
          const novels = await invoke<PixivNovel[]>("fetch_pixiv_series_novels", { seriesId, refreshToken });
          if (!novels || novels.length === 0) {
            throw new Error("シリーズに小説作品が見つかりませんでした。");
          }
          const title = novels[0]?.seriesNavigation?.seriesTitle || "シリーズ作品";
          const items = novels.map(n => ({
            id: String(n.id),
            title: n.title,
            subtitle: `文字数: ${n.characterCount?.toLocaleString() || "不明"} 字 / ${n.user?.name || ""}`,
            selected: true,
            originalData: n
          }));
          applyDownloadAnalysis(normalizedActiveUrl, `シリーズ: ${title}`, "pixiv_series", items);
          return;
        }

        // B. Pixiv 作家小説一括
        const userIdMatch = activeUrl.match(/users\/(\d+)/);
        const userId = userIdMatch?.[1];
        if (userId && !activeUrl.includes("/novels/")) {
          const novels = await invoke<PixivNovel[]>("fetch_pixiv_user_novels", { userId, refreshToken });
          if (!novels || novels.length === 0) {
            throw new Error("作家の小説作品が見つかりませんでした。");
          }
          const authorName = novels[0]?.user?.name || "作家作品";
          const items = novels.map(n => ({
            id: String(n.id),
            title: n.title,
            subtitle: `文字数: ${n.characterCount?.toLocaleString() || "不明"} 字 / ${new Date(n.createDate).toLocaleDateString("ja-JP")}`,
            selected: true,
            originalData: n
          }));
          applyDownloadAnalysis(normalizedActiveUrl, `作家: ${authorName}`, "pixiv_user", items);
          return;
        }

        // C. Pixiv 単一小説
        const novelIdMatch = activeUrl.match(/novels\/(\d+)/) || activeUrl.match(/novel\/show\.php\?id=(\d+)/);
        const novelId = novelIdMatch?.[1];
        if (novelId) {
          const data = await invoke<PixivNovel>("fetch_pixiv_novel_by_url", { url: activeUrl, refreshToken });
          const title = data.detail?.title || data.title || "閲覧中の小説";
          const author = data.detail?.user?.name || data.user?.name || "不明";
          const items = [{
            id: novelId,
            title: title,
            subtitle: `著者: ${author} (ID: ${novelId})`,
            selected: true,
            originalData: data
          }];
          applyDownloadAnalysis(normalizedActiveUrl, "Pixiv 小説保存", "pixiv_single", items);
          return;
        }

        throw new Error("Pixivの小説、シリーズ、または作家ページではありません。");
      }

      if (isFanbox) {
        if (!isFanboxAuthed) {
          throw new Error("FANBOX連携が未設定です。設定画面から連携してください。");
        }

        const cookie = await store.get<string>("fanbox_session_id") || "";
        const ua = await store.get<string>("fanbox_user_agent") || "";

        // D. FANBOX クリエイター一括
        const postIdMatch = activeUrl.match(/posts\/(\d+)/);
        const postId = postIdMatch?.[1];
        const creatorId = getFanboxCreatorId(activeUrl);

        if (creatorId && !postId) {
          const posts = await invoke<FanboxPost[]>("fetch_fanbox_creator_posts", { creatorId, cookie, userAgent: ua });
          if (!posts || posts.length === 0) {
            throw new Error("クリエイターに投稿が見つかりませんでした。");
          }
          const name = posts[0]?.user?.name || creatorId;
          const items = posts.map(p => ({
            id: String(p.id),
            title: p.title,
            subtitle: `${(p.type || "post").toUpperCase()} / ${new Date(p.publishedDatetime || Date.now()).toLocaleDateString("ja-JP")}`,
            selected: true,
            originalData: p
          }));
          applyDownloadAnalysis(normalizedActiveUrl, `クリエイター: ${name}`, "fanbox_creator", items);
          return;
        }

        // E. FANBOX 単一投稿
        if (postId) {
          const data = await invoke<FanboxPost>("fetch_fanbox_post", { postId, cookie, userAgent: ua });
          const title = data.title || "閲覧中の投稿";
          const author = data.user?.name || "不明";
          const items = [{
            id: postId,
            title: title,
            subtitle: `クリエイター: ${author} (ID: ${postId})`,
            selected: true,
            originalData: data
          }];
          applyDownloadAnalysis(normalizedActiveUrl, "FANBOX 投稿保存", "fanbox_single", items);
          return;
        }

        throw new Error("FANBOXの投稿、またはクリエイターページではありません。");
      }
    } catch (e: unknown) {
      const errMsg = e instanceof Error ? e.message : String(e);
      showToast(`検出エラー: ${errMsg}`, "error");
      if (lastSidebarAnalysis) {
        openCachedDownloadSidebar(lastSidebarAnalysis);
      } else {
        openEmptyDownloadSidebar(`保存候補を取得できませんでした。${errMsg}`);
      }
    }
  };

  // 選択されたダウンロードを実行（インストール）
  const executeSelectedDownloads = async () => {
    const targets = sidebarItems.filter(item => item.selected);
    if (targets.length === 0) {
      showToast("ダウンロードする作品が選択されていません。", "info");
      return;
    }

    const addFrontLog = (type: "info" | "success" | "warn" | "error", text: string) => {
      const now = new Date();
      const timeStr = `${now.getHours().toString().padStart(2, "0")}:${now.getMinutes().toString().padStart(2, "0")}:${now.getSeconds().toString().padStart(2, "0")}`;
      setLogs(prev => [...prev.slice(-499), { time: timeStr, type, text }]);
    };

    setIsDownloading(true);
    setSidebarProgress({ current: 0, total: targets.length });
    setSidebarStatusText("ダウンロードの初期化中...");
    setSidebarMode("downloadProgress");
    showToast("ダウンロードを開始します...", "info");
    addFrontLog("info", `ダウンロードを開始します... (対象: ${targets.length} 件)`);

    // 初期状態を pending に更新
    setSidebarItems(prev => prev.map(item => item.selected ? { ...item, status: "pending" } : item));

    let savedCount = 0;
    let skippedCount = 0;
    let failedCount = 0;
    const entityRefreshQueue = new Map<string, { entityType: "person" | "series"; source: string; sourceKey: string }>();
    const enqueueEntityRefresh = (entry: DownloadEntry) => {
      const personKey = entry.personId || entry.authorId;
      if (personKey) {
        const key = `person:${entry.source}:${personKey}`;
        entityRefreshQueue.set(key, { entityType: "person", source: entry.source, sourceKey: personKey });
      }
      if (entry.seriesId) {
        const key = `series:${entry.source}:${entry.seriesId}`;
        entityRefreshQueue.set(key, { entityType: "series", source: entry.source, sourceKey: entry.seriesId });
      }
    };

    try {
      if (sidebarDownloadType?.startsWith("pixiv")) {
        const refreshToken = await store.get<string>("pixiv_refresh_token") || "";

        for (let i = 0; i < targets.length; i++) {
          const target = targets[i];
          setSidebarProgress({ current: i + 1, total: targets.length });

          // 状態を downloading に更新
          setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "downloading" } : item));
          setSidebarStatusText(`「${target.title}」を処理中...`);

          // 重複＆更新監視の事前チェック
          const existingEntry = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: "pixiv", sourceId: target.id });
          if (existingEntry) {
            if (!existingEntry.watchUpdates) {
              enqueueEntityRefresh(existingEntry);
              skippedCount++;
              setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "skipped" } : item));
              addFrontLog("info", `[SKIP] 「${target.title}」は既に保存済みです（更新確認OFFのためスキップ）`);
              continue;
            }
            setSidebarStatusText(`「${target.title}」の更新をチェック中...`);
            addFrontLog("info", `[CHECK] 「${target.title}」の更新確認を開始します...`);
          } else {
            setSidebarStatusText(`「${target.title}」を新規保存中...`);
            addFrontLog("info", `[NEW] 「${target.title}」の新規ダウンロードを開始...`);
          }

          try {
            let data: PixivNovel;
            if (sidebarDownloadType === "pixiv_single") {
              data = target.originalData as PixivNovel;
            } else {
              const novelUrl = `https://www.pixiv.net/novel/show.php?id=${target.id}`;
              data = await invoke<PixivNovel>("fetch_pixiv_novel_by_url", { url: novelUrl, refreshToken });
            }

            const title = data.detail?.title || data.title || target.title || "pixiv_novel";
            const origPixivData = target.originalData as PixivNovel;
            const author = data.detail?.user?.name || origPixivData?.user?.name || "unknown";
            const authorId = String(data.detail?.user?.id || origPixivData?.user?.id || "0");
            
            // タグの安全な抽出
            const extractTags = (novel: PixivNovel) => {
              const directTags = novel.tags || [];
              const detailTags = novel.detail?.tags;
              if (Array.isArray(detailTags)) {
                return detailTags.map(t => typeof t === "string" ? t : t.name);
              } else if (detailTags && typeof detailTags === "object" && "tags" in detailTags) {
                return (detailTags.tags as PixivTag[]).map(t => t.name);
              }
              return directTags.map(t => typeof t === "string" ? t : t.name);
            };
            const tags = extractTags(data).length > 0 ? extractTags(data) : extractTags(origPixivData);

            const savedEntry = await invoke<DownloadEntry>("download_and_save", {
              data, source: "pixiv", sourceId: target.id, title, authorName: author, authorId,
              contentType: "novel", tags, excerpt: null,
              sourceCreatedAt: (data as any).detail?.create_date || (data as any).detail?.createDate || (data as any).create_date || (data as any).createDate || (origPixivData as any).detail?.createDate || origPixivData.createDate || null,
              cookie: null, userAgent: null,
            });
            enqueueEntityRefresh(savedEntry);
            
            savedCount++;
            setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "success" } : item));
            addFrontLog("success", `[SUCCESS] 「${target.title}」の保存が完了しました`);
            await new Promise(r => setTimeout(r, 1000));
          } catch (err) {
            failedCount++;
            setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "failed" } : item));
            addFrontLog("error", `[ERROR] 「${target.title}」の保存に失敗しました: ${err}`);
            console.error(`Failed to download Pixiv novel ${target.id}:`, err);
          }
        }
      } else if (sidebarDownloadType?.startsWith("fanbox")) {
        const cookie = await store.get<string>("fanbox_session_id") || "";
        const ua = await store.get<string>("fanbox_user_agent") || "";

        for (let i = 0; i < targets.length; i++) {
          const target = targets[i];
          setSidebarProgress({ current: i + 1, total: targets.length });

          // 状態を downloading に更新
          setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "downloading" } : item));
          setSidebarStatusText(`「${target.title}」を処理中...`);

          // 重複＆更新監視の事前チェック
          const existingEntry = await invoke<DownloadEntry | null>("db_get_download_by_source", { source: "fanbox", sourceId: target.id });
          if (existingEntry) {
            if (!existingEntry.watchUpdates) {
              enqueueEntityRefresh(existingEntry);
              skippedCount++;
              setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "skipped" } : item));
              addFrontLog("info", `[SKIP] 「${target.title}」は既に保存済みです（更新確認OFFのためスキップ）`);
              continue;
            }
            setSidebarStatusText(`「${target.title}」の更新をチェック中...`);
            addFrontLog("info", `[CHECK] 「${target.title}」の更新確認を開始します...`);
          } else {
            setSidebarStatusText(`「${target.title}」を新規保存中...`);
            addFrontLog("info", `[NEW] 「${target.title}」の新規ダウンロードを開始...`);
          }

          try {
            let data: FanboxPost;
            if (sidebarDownloadType === "fanbox_single") {
              data = target.originalData as FanboxPost;
            } else {
              data = await invoke<FanboxPost>("fetch_fanbox_post", { postId: target.id, cookie, userAgent: ua });
            }

            const origFanboxData = target.originalData as FanboxPost;
            const title = data.title || target.title || "fanbox_post";
            const author = data.user?.name || origFanboxData?.user?.name || "unknown";
            const authorId = data.creatorId || origFanboxData?.creatorId || data.user?.userId || origFanboxData?.user?.userId || "0";
            const tags = data.tags || origFanboxData?.tags || [];

            const savedEntry = await invoke<DownloadEntry>("download_and_save", {
              data, source: "fanbox", sourceId: target.id, title, authorName: author, authorId,
              contentType: data.type || origFanboxData?.type || "article", tags, excerpt: null,
              sourceCreatedAt: data.publishedDatetime || origFanboxData?.publishedDatetime || null, cookie, userAgent: ua,
            });
            enqueueEntityRefresh(savedEntry);
            
            savedCount++;
            setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "success" } : item));
            addFrontLog("success", `[SUCCESS] 「${target.title}」の保存が完了しました`);
            await new Promise(r => setTimeout(r, 1000));
          } catch (err) {
            failedCount++;
            setSidebarItems(prev => prev.map(item => item.id === target.id ? { ...item, status: "failed" } : item));
            addFrontLog("error", `[ERROR] 「${target.title}」の保存に失敗しました: ${err}`);
            console.error(`Failed to download FANBOX post ${target.id}:`, err);
          }
        }
      }

      if (entityRefreshQueue.size > 0) {
        setSidebarStatusText("作者・シリーズ情報を確認中...");
        addFrontLog("info", `[PROFILE] 作者・シリーズ情報を確認します... (${entityRefreshQueue.size} 件)`);
        const refreshToken = await store.get<string>("pixiv_refresh_token") || "";
        const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
        const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
        for (const target of entityRefreshQueue.values()) {
          try {
            await invoke("refresh_entity_profile", {
              params: {
                entityType: target.entityType,
                source: target.source,
                sourceKey: target.sourceKey,
                force: false,
                refreshToken,
                cookie: fanboxCookie,
                userAgent: fanboxUserAgent,
              },
            });
          } catch (e) {
            addFrontLog("warn", `[PROFILE] ${target.entityType}:${target.source}:${target.sourceKey} の確認をスキップしました: ${e}`);
          }
        }
      }

      setSidebarStatusText(`保存完了！ (新規: ${savedCount}, スキップ: ${skippedCount}, 失敗: ${failedCount})`);
      setSidebarMode("downloadDone");
      showToast(`保存完了！ 新規保存: ${savedCount}件, スキップ: ${skippedCount}件, 失敗: ${failedCount}件`, failedCount > 0 ? "error" : "success");
      addFrontLog("success", `[COMPLETE] ダウンロード処理が完了しました。新規: ${savedCount}件, スキップ: ${skippedCount}件, 失敗: ${failedCount}件`);
    } catch (e: unknown) {
      const errMsg = e instanceof Error ? e.message : String(e);
      setSidebarStatusText("ダウンロード中に致命的なエラーが発生しました");
      setSidebarMode("downloadDone");
      showToast(`ダウンロードエラー: ${errMsg}`, "error");
      addFrontLog("error", `[FATAL] ダウンロード中に致命的なエラーが発生しました: ${errMsg}`);
    } finally {
      setIsDownloading(false);
    }
  };

  const toggleSidebarItem = (id: string) => {
    setSidebarItems(prev => {
      const next = prev.map(item => item.id === id ? { ...item, selected: !item.selected } : item);
      if (sidebarMode === "analysis") {
        setLastSidebarAnalysis(cache => cache ? { ...cache, items: stripSidebarItemStatus(next) } : cache);
      }
      return next;
    });
  };

  const selectAllSidebarItems = (selected: boolean) => {
    setSidebarItems(prev => {
      const next = prev.map(item => ({ ...item, selected }));
      if (sidebarMode === "analysis") {
        setLastSidebarAnalysis(cache => cache ? { ...cache, items: stripSidebarItemStatus(next) } : cache);
      }
      return next;
    });
  };

  const getDownloadButtonText = () => {
    if (isDownloading) return "保存中...";
    if (showDownloadSidebar) {
      if (currentUrl && (!lastSidebarAnalysis || !isSameSidebarUrl(currentUrl, lastSidebarAnalysis.sourceUrl)) && detectDownloadTarget(currentUrl) !== "unsupported") {
        return "更新";
      }
      return "閉じる";
    }
    if (lastSidebarAnalysis && (!currentUrl || detectDownloadTarget(currentUrl) === "unsupported")) return "前回の候補";
    return "候補を取得";
  };

  const handleNavClick = useCallback((mode: ViewMode) => {
    if (isDownloading) {
      showToast("ダウンロード中は他の画面に移動できません。完了までお待ちください。", "error");
      return;
    }

    // 他のナビゲーション切り替え時に初期フィルターをリセット
    setInitialTagFilters(undefined);
    setInitialAuthorFilters(undefined);

    navigateTo({
      viewMode: mode,
      viewingDownloadId: null,
      libraryEntityView: null,
      libraryFilter: "all",
      libraryUiState: { ...defaultLibraryUiState, scrollTop: 0 },
    });
  }, [isDownloading, navigateTo, showToast]);

  if (libraryEntityView !== null) {
    return (
      <div id="root">
        <aside className="sidebar">
          <div className="sidebar-header"><img src={logo} className="sidebar-logo" alt="piep" /></div>
          <nav className="sidebar-nav">
            <button type="button" className="nav-item home-nav-item" onClick={() => handleNavClick("home")}><HomeIcon /> ホーム</button>
            <button type="button" className="nav-item library-nav-item active" onClick={() => handleBackToLibrary()}><LibraryIcon /> ライブラリ</button>
            <button type="button" className="nav-item epub-nav-item" onClick={() => handleNavClick("epub")}><BookIcon /> EPUB</button>
            <button type="button" className="nav-item update-nav-item" onClick={() => handleNavClick("update")}><RefreshIcon /> 更新管理</button>
            <button type="button" className="nav-item pixiv-nav-item" onClick={() => handleNavClick("pixiv")}><PaletteIcon /> Pixiv</button>
            <button type="button" className="nav-item fanbox-nav-item" onClick={() => handleNavClick("fanbox")}><HeartIcon /> FANBOX</button>
            <button type="button" className="nav-item settings-nav-item" onClick={() => handleNavClick("settings")}><SettingsIcon /> 設定</button>
          </nav>
        </aside>
        <main className="main-content">
          <div className={`content-inner epub-layout-wrapper entity-layout-wrapper ${epubSidebarLayoutOpen ? "epub-sidebar-layout-open" : ""}`}>
          <div className="library-main-content entity-main-content">
            {libraryEntityView.type === "person" ? (
              <PersonView
                source={libraryEntityView.source}
                sourceKey={libraryEntityView.sourceKey}
                showToast={showToast}
                onViewDetail={handleViewDetail}
                onViewSeries={handleViewSeries}
                onOpenSourceUrl={handleOpenUrlInAppBrowser}
                onOpenEpubForWorks={handleOpenEntityEpub}
                onLibraryChanged={() => {
                  loadStats();
                  setLibraryRestoreKey(key => key + 1);
                }}
                epubSidebarOpen={epubSidebarOpen}
              />
            ) : (
              <SeriesView
                source={libraryEntityView.source}
                sourceKey={libraryEntityView.sourceKey}
                showToast={showToast}
                onViewDetail={handleViewDetail}
                onViewSeries={handleViewSeries}
                onOpenSourceUrl={handleOpenUrlInAppBrowser}
                onOpenEpubForWorks={handleOpenEntityEpub}
                onLibraryChanged={() => {
                  loadStats();
                  setLibraryRestoreKey(key => key + 1);
                }}
                epubSidebarOpen={epubSidebarOpen}
              />
            )}
          </div>
          <EpubSidebar
            selectedIds={selectedEpubIds}
            selectedItems={selectedEpubItems}
            showToast={showToast}
            isOpen={epubSidebarOpen}
            onClose={() => setEpubSidebarVisible(false)}
            showTemplateManager={showTemplateManager}
            onCloseTemplateManager={() => setShowTemplateManager(false)}
          />
          </div>
        </main>
      </div>
    );
  }

  if (viewingDownloadId !== null) {
    return (
      <div id="root">
        <aside className="sidebar">
          <div className="sidebar-header"><img src={logo} className="sidebar-logo" alt="piep" /></div>
          <nav className="sidebar-nav">
            <button type="button" className="nav-item home-nav-item" onClick={() => handleNavClick("home")}><HomeIcon /> ホーム</button>
            <button type="button" className="nav-item library-nav-item active" onClick={() => handleBackToLibrary()}><LibraryIcon /> ライブラリ</button>
            <button type="button" className="nav-item epub-nav-item" onClick={() => handleNavClick("epub")}><BookIcon /> EPUB</button>
            <button type="button" className="nav-item update-nav-item" onClick={() => handleNavClick("update")}><RefreshIcon /> 更新管理</button>
            <button type="button" className="nav-item pixiv-nav-item" onClick={() => handleNavClick("pixiv")}>
              <PaletteIcon /> Pixiv
              <div className={`status-dot ${isPixivAuthed ? "authed" : ""}`} title={isPixivAuthed ? "連携済み" : "未連携"} />
            </button>
            <button type="button" className="nav-item fanbox-nav-item" onClick={() => handleNavClick("fanbox")}>
              <HeartIcon /> FANBOX
              <div className={`status-dot ${isFanboxAuthed ? "authed" : ""}`} title={isFanboxAuthed ? "連携済み" : "未連携"} />
            </button>
            <button type="button" className="nav-item settings-nav-item" onClick={() => handleNavClick("settings")}><SettingsIcon /> 設定</button>
          </nav>
          <button
            type="button"
            className={`floating-console-toggle ${showConsole ? "active" : ""}`}
            onClick={() => setShowConsole(!showConsole)}
          >
            <TerminalAlertIcon />
            LOGS
          </button>
          {!showDownloadSidebar && toasts.length > 0 && (
            <div className="sidebar-toast-container">
              {toasts.map(toast => (
                <div key={toast.id} className={`sidebar-toast ${toast.type}`}>
                  {toast.type === "error" && <AlertIcon />}
                  <span className="toast-text-content">{toast.text}</span>
                </div>
              ))}
            </div>
          )}
        </aside>
        <main className="main-content">
          <div className={`content-inner epub-layout-wrapper viewer-inner ${epubSidebarLayoutOpen ? "epub-sidebar-layout-open" : ""}`}>
            <div className="viewer-main-content">
              <ContentViewer 
                downloadId={viewingDownloadId} 
                showToast={showToast} 
                onSelectTagFilter={(tag) => {
                  setInitialTagFilters({ [tag]: "include" });
                  setInitialAuthorFilters({});
                  navigateTo({
                    viewMode: "library",
                    viewingDownloadId: null,
                    libraryEntityView: null,
                    libraryFilter: "all",
                    libraryUiState: {
                      ...defaultLibraryUiState,
                      tagFilters: { [tag]: "include" },
                      authorFilters: {},
                      scrollTop: 0,
                      showFilters: true,
                    },
                  });
                }}
                onExportEpub={(download) => {
                  if (epubSidebarOpen && selectedEpubIds.has(download.id) && selectedEpubIds.size === 1) {
                    setEpubSidebarVisible(false);
                  } else {
                    setSelectedEpubIds(new Set([download.id]));
                    setSelectedEpubItems([download as any]);
                    setEpubSidebarVisible(true);
                  }
                }}
                isEpubActive={epubSidebarOpen && selectedEpubIds.has(viewingDownloadId)}
                onViewPerson={handleViewPerson}
                onViewSeries={handleViewSeries}
                onNavigateInternalUrl={handleNavigateContentUrl}
                onOpenSourceUrl={handleOpenUrlInAppBrowser}
                onDeleted={handleBackToLibrary}
              />
            </div>
            <EpubSidebar
              selectedIds={selectedEpubIds}
              selectedItems={selectedEpubItems}
              showToast={showToast}
              isOpen={epubSidebarOpen}
              onClose={() => setEpubSidebarVisible(false)}
              showTemplateManager={showTemplateManager}
              onCloseTemplateManager={() => setShowTemplateManager(false)}
            />
          </div>
        </main>
      </div>
    );
  }

  return (
    <div id="root">
      {/* Sidebar */}
      <aside className="sidebar">
        <div className="sidebar-header"><img src={logo} className="sidebar-logo" alt="piep" /></div>
        <nav className="sidebar-nav">
          <button type="button" className={`nav-item home-nav-item ${viewMode === "home" ? "active" : ""}`} onClick={() => handleNavClick("home")}><HomeIcon /> ホーム</button>
          <button type="button" className={`nav-item library-nav-item ${viewMode === "library" ? "active" : ""}`} onClick={() => {
            if (viewMode === "library") return;
            handleNavClick("library");
          }}><LibraryIcon /> ライブラリ</button>
          <button type="button" className={`nav-item epub-nav-item ${viewMode === "epub" ? "active" : ""}`} onClick={() => {
            if (viewMode === "epub") return;
            handleNavClick("epub");
          }}><BookIcon /> EPUB</button>
          <button type="button" className={`nav-item update-nav-item ${viewMode === "update" ? "active" : ""}`} onClick={() => {
            if (viewMode === "update") return;
            handleNavClick("update");
          }}><RefreshIcon /> 更新管理</button>
          <button type="button" className={`nav-item pixiv-nav-item ${viewMode === "pixiv" ? "active" : ""}`} onClick={() => handleNavClick("pixiv")}>
            <PaletteIcon /> Pixiv
            <div className={`status-dot ${isPixivAuthed ? "authed" : ""}`} title={isPixivAuthed ? "連携済み" : "未連携"} />
          </button>
          <button type="button" className={`nav-item fanbox-nav-item ${viewMode === "fanbox" ? "active" : ""}`} onClick={() => handleNavClick("fanbox")}>
            <HeartIcon /> FANBOX
            <div className={`status-dot ${isFanboxAuthed ? "authed" : ""}`} title={isFanboxAuthed ? "連携済み" : "未連携"} />
          </button>
          <button type="button" className={`nav-item settings-nav-item ${viewMode === "settings" ? "active" : ""}`} onClick={() => handleNavClick("settings")}><SettingsIcon /> 設定</button>
        </nav>
        <button
          type="button"
          className={`floating-console-toggle ${showConsole ? "active" : ""}`}
          onClick={() => setShowConsole(!showConsole)}
        >
          <TerminalAlertIcon />
          LOGS
        </button>
        {!showDownloadSidebar && toasts.length > 0 && (
          <div className="sidebar-toast-container">
            {toasts.map(toast => (
              <div key={toast.id} className={`sidebar-toast ${toast.type}`}>
                {toast.type === "error" && <AlertIcon />}
                <span className="toast-text-content">{toast.text}</span>
              </div>
            ))}
          </div>
        )}
      </aside>

      {/* Main Content */}
      <main className="main-content">
        {/* Browser Toolbar */}
        {(viewMode === "pixiv" || viewMode === "fanbox") && (
          <div className="browser-toolbar">
            <button className="toolbar-nav-btn" title="戻る" onClick={() => invoke("go_back_embedded_browser")} disabled={!tauriAvailable}><ChevronLeftIcon /></button>
            <button className="toolbar-nav-btn" title="進む" onClick={() => invoke("go_forward_embedded_browser")} disabled={!tauriAvailable}><ChevronRightIcon /></button>
            <button className="toolbar-nav-btn" title="再読み込み" onClick={() => invoke("reload_embedded_browser")} disabled={!tauriAvailable}><RefreshIcon /></button>
            <div className="url-display">
              {viewMode === "pixiv" ? <PaletteIcon /> : <HeartIcon />}
              {!tauriAvailable ? "Tauriアプリ内でブラウザを表示します" : currentUrl || "読み込み中..."}
            </div>
            {!isPixivAuthed && viewMode === "pixiv" && (
              <button type="button" className="toolbar-auth-warning" onClick={() => handleNavClick("settings")} title="Pixivが未連携です。クリックして設定画面で連携してください。">
                ⚠️ 未連携
              </button>
            )}
            {!isFanboxAuthed && viewMode === "fanbox" && (
              <button type="button" className="toolbar-auth-warning" onClick={() => handleNavClick("settings")} title="FANBOXが未連携です。クリックして設定画面で連携してください。">
                ⚠️ 未連携
              </button>
            )}
            <button
              className={`toolbar-download-btn ready ${showDownloadSidebar ? "active" : ""}`}
              onClick={() => {
                const shouldRefreshSidebar = showDownloadSidebar
                  && !isDownloading
                  && currentUrl
                  && (!lastSidebarAnalysis || !isSameSidebarUrl(currentUrl, lastSidebarAnalysis.sourceUrl))
                  && detectDownloadTarget(currentUrl) !== "unsupported";

                if (showDownloadSidebar && !shouldRefreshSidebar) {
                  setShowDownloadSidebar(false);
                } else {
                  handleDownload();
                }
              }}
              disabled={!tauriAvailable || (isDownloading && !showDownloadSidebar)}
              title={!tauriAvailable ? "保存候補の取得はTauriアプリ内で利用できます" : showDownloadSidebar ? "保存候補を閉じる、または現在のページで更新" : "現在のページから保存候補を取得"}
            >
              <DownloadIcon />{getDownloadButtonText()}
            </button>
          </div>
        )}

        {/* Home */}
        {viewMode === "home" && (
          <div className="content-inner home-main-content">
            <div className="home-container">

              {/* Dashboard Meta Ribbon (目立たない読み取り専用のステータス帯) */}
              {stats && (
                <div className="dashboard-meta-ribbon">
                  <span className="ribbon-title">DB STATUS</span>
                  <span className="ribbon-divider">|</span>
                  <span className="ribbon-value">{stats.totalAssets || 0} assets</span>
                  <span className="ribbon-dot">·</span>
                  <span className="ribbon-value">{formatBytes(stats.totalSizeBytes)} total</span>
                  <span className="ribbon-dot">·</span>
                  <span className="ribbon-value">
                    {stats.totalDownloads > 0 ? formatBytes(stats.totalSizeBytes / stats.totalDownloads) : "0 B"} avg.
                  </span>
                </div>
              )}

              {/* Dashboard Statistics Grid (遷移価値のある3大アクションのみ) */}
              <div className="dashboard-grid">
                <button type="button" className="dashboard-card" onClick={() => {
                  navigateTo({
                    viewMode: "library",
                    viewingDownloadId: null,
                    libraryEntityView: null,
                    libraryFilter: "all",
                    libraryUiState: { ...defaultLibraryUiState, sourceFilter: "", scrollTop: 0 },
                  });
                }}>
                  <span className="value">{stats ? stats.totalDownloads : "0"}</span>
                  <span className="label">ALL LIBRARY</span>
                </button>
                <button type="button" className="dashboard-card" onClick={() => {
                  navigateTo({
                    viewMode: "library",
                    viewingDownloadId: null,
                    libraryEntityView: null,
                    libraryFilter: "pixiv",
                    libraryUiState: { ...defaultLibraryUiState, sourceFilter: "pixiv", scrollTop: 0 },
                  });
                }}>
                  <span className="value" style={{ color: "var(--color-pixiv)" }}>{stats ? stats.pixivCount : "0"}</span>
                  <span className="label">PIXIV</span>
                </button>
                <button type="button" className="dashboard-card" onClick={() => {
                  navigateTo({
                    viewMode: "library",
                    viewingDownloadId: null,
                    libraryEntityView: null,
                    libraryFilter: "fanbox",
                    libraryUiState: { ...defaultLibraryUiState, sourceFilter: "fanbox", scrollTop: 0 },
                  });
                }}>
                  <span className="value" style={{ color: "var(--color-fanbox)" }}>{stats ? stats.fanboxCount : "0"}</span>
                  <span className="label">FANBOX</span>
                </button>
              </div>

              <div className="dashboard-sections">
                <div className="dashboard-quick-grid">
                  <button type="button" className="home-card library-card" style={{ "--card-accent": "var(--color-text-secondary)", "--card-accent-bg": "rgba(100, 116, 139, 0.08)" } as React.CSSProperties} onClick={() => navigateTo({ viewMode: "library", viewingDownloadId: null, libraryEntityView: null })}>
                    <div className="home-card-icon" style={{ margin: 0 }}><LibraryIcon /></div>
                    <div className="home-card-text">
                      <h3>ライブラリ</h3>
                      <p>作品の検索・閲覧・管理</p>
                    </div>
                  </button>
                  <button type="button" className="home-card" style={{ "--card-accent": "var(--color-epub)", "--card-accent-bg": "var(--color-epub-bg)" } as React.CSSProperties} onClick={() => navigateTo({ viewMode: "epub", viewingDownloadId: null, libraryEntityView: null })}>
                    <div className="home-card-icon" style={{ margin: 0 }}><BookIcon /></div>
                    <div className="home-card-text">
                      <h3>EPUB エクスポート</h3>
                      <p>作品をEPUB形式に変換</p>
                    </div>
                  </button>
                  <button type="button" className="home-card" style={{ "--card-accent": "var(--color-success)", "--card-accent-bg": "rgba(34, 197, 94, 0.08)" } as React.CSSProperties} onClick={() => navigateTo({ viewMode: "update", viewingDownloadId: null, libraryEntityView: null })}>
                    <div className="home-card-icon" style={{ margin: 0 }}><RefreshIcon /></div>
                    <div className="home-card-text">
                      <h3>更新管理</h3>
                      <p>作品更新と新作候補を確認</p>
                    </div>
                  </button>
                  <button type="button" className="home-card" style={{ "--card-accent": "var(--color-pixiv)", "--card-accent-bg": "rgba(0, 150, 250, 0.08)" } as React.CSSProperties} onClick={() => navigateTo({ viewMode: "pixiv", viewingDownloadId: null, libraryEntityView: null })}>
                    <div className="home-card-icon" style={{ margin: 0 }}><PaletteIcon /></div>
                    <div className="home-card-text">
                      <h3>Pixiv ブラウザ</h3>
                      <p>Pixivを閲覧・保存</p>
                    </div>
                  </button>
                  <button type="button" className="home-card" style={{ "--card-accent": "var(--color-fanbox)", "--card-accent-bg": "rgba(242, 198, 36, 0.08)" } as React.CSSProperties} onClick={() => navigateTo({ viewMode: "fanbox", viewingDownloadId: null, libraryEntityView: null })}>
                    <div className="home-card-icon" style={{ margin: 0 }}><HeartIcon /></div>
                    <div className="home-card-text">
                      <h3>FANBOX ブラウザ</h3>
                      <p>限定投稿を保存</p>
                    </div>
                  </button>
                </div>
                <button type="button" className="home-card settings-card" style={{ "--card-accent": "var(--color-text-secondary)", "--card-accent-bg": "rgba(100, 116, 139, 0.08)" } as React.CSSProperties} onClick={() => navigateTo({ viewMode: "settings", viewingDownloadId: null, libraryEntityView: null })}>
                  <div className="home-card-icon" style={{ margin: 0 }}><SettingsIcon /></div>
                  <div className="home-card-text">
                    <h3>アカウント連携・設定</h3>
                    <p>認証情報を管理</p>
                  </div>
                </button>
              </div>
            </div>
          </div>
        )}

        {/* Library */}
        {viewMode === "library" && (
          <div className={`library-main-content ${isRestoringScroll ? "scroll-restoring" : ""}`}>
            <LibraryView
              mode="library"
              onViewDetail={handleViewDetail}
              onViewPerson={handleViewPerson}
              onViewSeries={handleViewSeries}
              showToast={showToast}
              initialFilter={libraryFilter}
              initialTagFilters={initialTagFilters}
              initialAuthorFilters={initialAuthorFilters}
              initialState={libraryUiState}
              restoreKey={libraryRestoreKey}
              onUiStateChange={handleLibraryUiStateChange}
            />
          </div>
        )}

        {/* EPUB */}
        {viewMode === "epub" && (
          <div className={`content-inner epub-layout-wrapper ${epubSidebarLayoutOpen ? "epub-sidebar-layout-open" : ""}`}>
            <div className={`epub-main-content ${isRestoringScroll ? "scroll-restoring" : ""}`}>
              <LibraryView
                mode="epub"
                onViewDetail={handleViewDetail}
                onViewPerson={handleViewPerson}
                onViewSeries={handleViewSeries}
                showToast={showToast}
                initialFilter={libraryFilter}
                epubSelectedIds={selectedEpubIds}
                onEpubSelectionChange={(ids, items) => {
                  setSelectedEpubIds(ids);
                  setSelectedEpubItems(items);
                }}
                onToggleEpubSidebar={() => setEpubSidebarVisible(!epubSidebarOpen)}
                epubSidebarOpen={epubSidebarOpen}
                onOpenTemplateManager={() => setShowTemplateManager(true)}
                initialTagFilters={initialTagFilters}
                initialAuthorFilters={initialAuthorFilters}
                initialState={libraryUiState}
                restoreKey={libraryRestoreKey}
                onUiStateChange={handleLibraryUiStateChange}
              />
            </div>
            <EpubSidebar
              selectedIds={selectedEpubIds}
              selectedItems={selectedEpubItems}
              showToast={showToast}
              isOpen={epubSidebarOpen}
              onClose={() => setEpubSidebarVisible(false)}
              showTemplateManager={showTemplateManager}
              onCloseTemplateManager={() => setShowTemplateManager(false)}
            />
          </div>
        )}

        {/* Update Management */}
        {viewMode === "update" && (
          <div className={`content-inner epub-layout-wrapper update-layout-wrapper ${updateSidebarLayoutOpen ? "epub-sidebar-layout-open" : ""}`}>
            <div className={`update-main-content ${isRestoringScroll ? "scroll-restoring" : ""}`}>
              <LibraryView
                mode="update"
                onViewDetail={handleViewDetail}
                onViewPerson={handleViewPerson}
                onViewSeries={handleViewSeries}
                showToast={showToast}
                initialFilter={libraryFilter}
                updateSidebarOpen={updateSidebarOpen}
                onToggleUpdateSidebar={() => setUpdateSidebarVisible(!updateSidebarOpen)}
                onUpdateWorkTargetChange={() => setUpdateSidebarRefreshKey(key => key + 1)}
                initialTagFilters={initialTagFilters}
                initialAuthorFilters={initialAuthorFilters}
                initialState={libraryUiState}
                restoreKey={libraryRestoreKey}
                onUiStateChange={handleLibraryUiStateChange}
              />
            </div>
            <UpdateSidebar
              isOpen={updateSidebarOpen}
              showToast={showToast}
              onClose={() => setUpdateSidebarVisible(false)}
              refreshKey={updateSidebarRefreshKey}
              onLibraryChanged={() => {
                loadStats();
                setLibraryRestoreKey(key => key + 1);
                setUpdateSidebarRefreshKey(key => key + 1);
              }}
            />
          </div>
        )}

        {/* Settings */}
        {viewMode === "settings" && (
          <div className="content-inner settings-main-content">
            <h2 style={{ fontSize: "1.35rem", fontWeight: 700, marginBottom: "1.5rem", letterSpacing: "-0.01em" }}>アカウント連携</h2>
            <AuthSettings />
          </div>
        )}



      {/* Browser Area & Interactive Selection Sidebar */}
      {(viewMode === "pixiv" || viewMode === "fanbox") ? (
        <div className={`browser-layout-wrapper ${showDownloadSidebar ? 'sidebar-open' : ''}`}>
          <div ref={browserRef} className="browser-placeholder">
            {!tauriAvailable && (
              <div className="browser-preview-empty">
                <h3>内蔵ブラウザはTauriアプリ内で利用できます</h3>
                <p>開発ブラウザではレイアウトだけを確認できます。Pixiv/FANBOXの表示、URL追従、保存候補の取得はデスクトップアプリで動作します。</p>
              </div>
            )}
          </div>
          
          {showDownloadSidebar && (
            <aside className="download-option-sidebar">
              <div className="sidebar-option-header">
                <div className="sidebar-option-title-row">
                  <h3 className="sidebar-option-title" title={sidebarTitle}>{sidebarTitle || "読み込み中..."}</h3>
                </div>
                
                {!sidebarLoading && sidebarItems.length > 0 && !sidebarProgress && (
                  <div className="sidebar-option-actions">
                    <button className="secondary" onClick={() => selectAllSidebarItems(true)}>すべて選択</button>
                    <button className="secondary" onClick={() => selectAllSidebarItems(false)}>すべて解除</button>
                  </div>
                )}
              </div>

              <div className="sidebar-option-list-container">
                {sidebarLoading ? (
                  <div className="skeleton-list">
                    {[1, 2, 3, 4].map(n => (
                      <div className="skeleton-item" key={n}>
                        <div className="skeleton-checkbox" />
                        <div className="skeleton-info">
                          <div className="skeleton-title" />
                          <div className="skeleton-subtitle" />
                        </div>
                      </div>
                    ))}
                  </div>
                ) : sidebarItems.length === 0 ? (
                  <div className="sidebar-empty-state">
                    <p>{sidebarEmptyMessage || "作品が見つかりませんでした。"}</p>
                  </div>
                ) : (
                  sidebarItems.map(item => (
                    <div 
                      key={item.id} 
                      className={`sidebar-option-item ${item.selected ? 'checked' : ''} ${item.status ? `status-${item.status}` : ''}`}
                      onClick={() => !isDownloading && toggleSidebarItem(item.id)}
                    >
                      <div className="sidebar-option-checkbox-wrapper">
                        <input 
                          type="checkbox" 
                          className="sidebar-option-checkbox" 
                          checked={item.selected} 
                          readOnly
                          disabled={isDownloading}
                        />
                      </div>
                      <div className="sidebar-option-info">
                        <div className="sidebar-option-text">
                          <h4 className="sidebar-option-item-title">{item.title}</h4>
                          {item.subtitle && <p className="sidebar-option-item-subtitle">{item.subtitle}</p>}
                        </div>
                        {item.status && (
                          <span className={`sidebar-option-item-status status-badge-${item.status}`}>
                            {item.status === 'pending' && '待機中'}
                            {item.status === 'downloading' && '保存中'}
                            {item.status === 'success' && '保存完了'}
                            {item.status === 'skipped' && '保存済み'}
                            {item.status === 'failed' && '失敗'}
                          </span>
                        )}
                      </div>
                    </div>
                  ))
                )}
              </div>

              <div className="sidebar-option-footer">
                {isDownloading && sidebarProgress ? (
                  <div className="sidebar-progress-container">
                    <div className="sidebar-progress-label">
                      <span>{sidebarStatusText || "ダウンロード中..."}</span>
                      <span>{sidebarProgress.current} / {sidebarProgress.total}</span>
                    </div>
                    <div className="sidebar-progress-bar-bg">
                      <div 
                        className="sidebar-progress-bar-fill" 
                        style={{ width: `${(sidebarProgress.current / sidebarProgress.total) * 100}%` }}
                      />
                    </div>
                  </div>
                ) : sidebarProgress ? (
                  <button 
                    className="primary sidebar-option-install-btn success-view" 
                    onClick={() => {
                      setShowDownloadSidebar(false);
                      setSidebarProgress(null);
                      setSidebarStatusText("");
                      setSidebarMode("analysis");
                      navigateTo({
                        viewMode: "library",
                        viewingDownloadId: null,
                        libraryEntityView: null,
                        libraryFilter: "all",
                        libraryUiState: { ...defaultLibraryUiState, sourceFilter: "", scrollTop: 0 },
                      });
                    }}
                  >
                    <LibraryIcon />
                    ライブラリへ移動して読む
                  </button>
                ) : sidebarMode === "empty" ? null : (
                  <button 
                    className="primary sidebar-option-install-btn" 
                    onClick={executeSelectedDownloads}
                    disabled={sidebarLoading || sidebarItems.filter(i => i.selected).length === 0}
                  >
                    <DownloadIcon />
                    選択した {sidebarItems.filter(i => i.selected).length} 件を保存
                  </button>
                )}
              </div>
              {showDownloadSidebar && toasts.length > 0 && (
                <div className="sidebar-toast-container">
                  {toasts.map(toast => (
                    <div key={toast.id} className={`sidebar-toast ${toast.type}`}>
                      {toast.type === "error" && <AlertIcon />}
                      <span className="toast-text-content">{toast.text}</span>
                    </div>
                  ))}
                </div>
              )}
            </aside>
          )}
        </div>
      ) : null}
      </main>



      {/* Debug Console Panel */}
      <div className={`debug-console-panel ${showConsole ? "open" : ""}`}>
        <div className="console-header">
          <div className="console-title">
            <span className="pulse-dot" />
            <h3>デバッグログコンソール</h3>
            <span className="console-count">({logs.length} 件のログ)</span>
          </div>
          <div className="console-actions">
            <button className="console-clear-btn" onClick={() => setLogs([])}>クリア</button>
            <button className="console-close-btn" onClick={() => setShowConsole(false)}>閉じる</button>
          </div>
        </div>
        <div 
          className="console-body"
          onScroll={(e) => {
            const target = e.currentTarget;
            const isUp = target.scrollHeight - target.scrollTop - target.clientHeight > 50;
            setIsUserScrollingUp(isUp);
          }}
        >
          {logs.length === 0 ? (
            <div className="console-empty">
              ログはありません。ダウンロードを開始するとここに詳細な進捗が表示されます。
            </div>
          ) : (
            <div className="console-logs-list">
              {logs.map((log, idx) => (
                <div key={idx} className={`console-row ${log.type}`}>
                  {log.type === "success" && (
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" style={{ color: "var(--color-success)", flexShrink: 0 }}>
                      <polyline points="20 6 9 17 4 12" />
                    </svg>
                  )}
                  {log.type === "error" && (
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" style={{ color: "var(--color-error)", flexShrink: 0 }}>
                      <circle cx="12" cy="12" r="10" />
                      <line x1="12" y1="8" x2="12" y2="12" />
                      <line x1="12" y1="16" x2="12.01" y2="16" />
                    </svg>
                  )}
                  {log.type === "warn" && (
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" style={{ color: "var(--color-warn)", flexShrink: 0 }}>
                      <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
                      <line x1="12" y1="9" x2="12" y2="13" />
                      <line x1="12" y1="17" x2="12.01" y2="17" />
                    </svg>
                  )}
                  {log.type === "info" && (
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" style={{ color: "var(--color-text-secondary)", flexShrink: 0 }}>
                      <circle cx="12" cy="12" r="10" />
                      <line x1="12" y1="16" x2="12" y2="12" />
                      <line x1="12" y1="8" x2="12.01" y2="8" />
                    </svg>
                  )}
                  <span className="log-time">[{log.time}]</span>
                  <span className="log-badge">{log.type.toUpperCase()}</span>
                  <span className="log-text">{log.text}</span>
                </div>
              ))}
              <div ref={consoleBottomRef} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default App;
