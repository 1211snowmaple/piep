import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { isTauri } from "@tauri-apps/api/core";
import { ArrowUp, BookOpen, ChevronLeft, LibraryBig, Menu, Plus, Settings } from "lucide-react";
import "./App.css";
import "./App.desktop.css";
import "./App.reader.css";
import "./App.library.css";
import { PiepLockup } from "./PiepLockup";
import { AppSidebar, type Section, type Shelf } from "./AppSidebar";
import { PcLibrary } from "./PcLibrary";
import { PcConnectionSettings } from "./PcConnectionSettings";
import { AppUpdateNotice, AppUpdateSettings, useAppUpdate } from "./appUpdate";
import { sendReading } from "./readingSync";
import { OnlineReader } from "./OnlineReader";
import { OfflineLibrary } from "./OfflineLibrary";
import { WorkDetail } from "./WorkDetail";
import { EntityPage } from "./EntityPage";
import { homePage, NavigationProvider, useAppHistory, type LibraryPage } from "./appHistory";
import { forgetAllPages } from "./pageCache";
import { setReadingServer } from "./recentReading";
import { statusQueryKey } from "./workList";
import { useFavoriteNotice } from "./favorites";
import { defaultPcSearch, getPcStatus, type OfflineWork, type PcConnectionStatus, type PcSearchCriteria, type PcWork } from "./pcApi";
import { handleReaderKeyboard } from "./readerKeyboard";
import { ReaderBehaviorControls, ReaderTypeControls } from "./ReaderSettings";
import { defaultReaderBehavior, defaultReaderType, loadReaderBehavior, saveReaderBehavior, typeLimits, type ReaderBehavior, type ReaderFont, type ReaderTheme, type ReaderType } from "./readerPrefs";
import { loadRememberedSort, loadSavedConditions, saveRememberedSort, saveSavedConditions, type FixedSort, type SavedCondition } from "./librarySort";

type IconName = "computer" | "book" | "settings" | "menu";

const preview = {
  title: "雨のあと、栞をはさむ",
  author: "piep 読書画面サンプル",
  pages: [
    { heading: "雨のあと", paragraphs: [
      "雨が止むころ、窓辺に置いた本の表紙が少しだけ明るくなった。読みかけのページには、昨夜は気づかなかった短い一文がある。",
      "急いで先へ進む必要はない。指先で栞の端をそろえ、もう一度、最初の行に目を戻した。",
      "外では濡れた道を自転車が走っていく。部屋の中には、ページをめくる小さな音だけが残った。",
    ] },
    { heading: "次のページ", paragraphs: [
      "物語の続きは、いつでも同じ場所で待っている。けれど読む人の気持ちは、そのたびに少しずつ違う。",
      "昨日は通り過ぎた言葉が、今日は立ち止まる理由になる。そんな変化を確かめながら、本を閉じる時間もまた読書の一部なのだと思った。",
    ] },
    { heading: "栞の場所", paragraphs: [
      "夕方になって、雲の間から細い光が差した。栞をはさみ、本を机の上へ戻す。",
      "次に開くときも、このページから始めよう。",
    ] },
  ],
} as const;

const positionKey = "piep.android.preview.page";
const fontKey = "piep.android.reader.font-size.v2";
const lineHeightKey = "piep.android.reader.line-height.v2";
const readerWidthKey = "piep.android.reader.width.v1";
// 以前は起動のたびに既定の明朝を書き込んでいたので、選んだ記録と見分けがつかない。鍵を変えて読み捨てる。
const fontFamilyKey = "piep.android.reader.font-family.v2";
const readerThemeKey = "piep.android.reader.theme";
const appearanceKey = "piep.android.appearance";
const pagingModeKey = "piep.android.library.paging-mode";

type Appearance = "auto" | "light" | "dark";
type PagingMode = "auto" | "pages";

function storedChoice<T extends string>(key: string, fallback: T, options: readonly T[]): T {
  try { const value = window.localStorage.getItem(key); return options.find((option) => option === value) ?? fallback; }
  catch { return fallback; }
}

function storedNumber(key: string, fallback: number, minimum: number, maximum: number) {
  try {
    const raw = window.localStorage.getItem(key);
    if (raw === null) return fallback;
    const value = Number(raw);
    return Number.isFinite(value) && value >= minimum && value <= maximum ? value : fallback;
  } catch {
    return fallback;
  }
}

function storeSetting(key: string, value: string): void {
  try { window.localStorage.setItem(key, value); } catch { /* storage unavailable */ }
}

function Icon({ name }: { name: IconName }) {
  const Component = { computer: LibraryBig, book: BookOpen, settings: Settings, menu: Menu }[name];
  return <Component className="icon" aria-hidden="true" />;
}

const sections: { id: Section; label: string; icon: IconName }[] = [
  { id: "pc", label: "ライブラリ", icon: "computer" },
  { id: "offline", label: "端末内", icon: "book" },
  { id: "settings", label: "設定", icon: "settings" },
];

function App() {
  const mainRef = useRef<HTMLElement>(null);
  const { entry, depth, push, replace, back, restart } = useAppHistory(mainRef);
  const currentPage = entry.page;
  const section: Section = currentPage.name === "offline" ? "offline" : currentPage.name === "settings" ? "settings" : "pc";
  // 本棚を離れても、戻ったときの条件は保つ。
  const lastLibrary = useRef<LibraryPage>(homePage);
  if (currentPage.name === "library") lastLibrary.current = currentPage;
  const selectedWork = entry.detail;
  const onlineWork = entry.reader?.kind === "online" ? entry.reader.work : null;
  const offlineWork = entry.reader?.kind === "offline" ? entry.reader.work : null;
  const reading = entry.reader?.kind === "preview";
  const [exitHint, setExitHint] = useState(false);
  const lastBackAt = useRef(0);
  const [scrolledFar, setScrolledFar] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const sidebarFirstControlRef = useRef<HTMLButtonElement>(null);
  const sidebarToggleRef = useRef<HTMLButtonElement>(null);
  const sidebarWasOpenRef = useRef(false);
  const readerOverlayRef = useRef<HTMLDivElement>(null);
  const previewScrollRef = useRef<HTMLElement>(null);
  const previewKeyboardPageChange = useRef(false);
  const previewKeyboardEnd = useRef(false);
  const readerReturnFocusRef = useRef<HTMLElement | null>(null);
  const detailOverlayRef = useRef<HTMLDivElement>(null);
  const detailReturnFocusRef = useRef<HTMLElement | null>(null);
  const [savedRevision, setSavedRevision] = useState(0);
  const queryClient = useQueryClient();
  // つながっていない間は定期的に確かめ、戻ったらそのまま本棚を出す。
  const statusQuery = useQuery({
    queryKey: statusQueryKey,
    queryFn: getPcStatus,
    enabled: isTauri(),
    staleTime: 15_000,
    retry: false,
    refetchInterval: (query) => query.state.data?.reachable ? false : 15_000,
  });
  const pcConnection = statusQuery.data ?? null;
  const setPcConnection = useCallback((next: PcConnectionStatus | null | ((old: PcConnectionStatus | null) => PcConnectionStatus | null)) => {
    queryClient.setQueryData<PcConnectionStatus | null>(statusQueryKey, (old) => typeof next === "function" ? next(old ?? null) : next);
  }, [queryClient]);
  // 接続先が変わったら、前のサーバーの一覧・カーソル・詳細・本文を引きずらない。
  const serverUrl = statusQuery.data === undefined ? undefined : statusQuery.data?.url ?? null;
  const shownServer = useRef(serverUrl);
  useEffect(() => {
    if (serverUrl === undefined) return;
    setReadingServer(serverUrl);
    const previous = shownServer.current;
    shownServer.current = serverUrl;
    if (previous === undefined || previous === serverUrl) return;
    forgetAllPages();
    restart();
    const fromServer = (query: { queryKey: readonly unknown[] }) => query.queryKey[0] !== statusQueryKey[0] && query.queryKey[0] !== "offline-works";
    void queryClient.cancelQueries({ predicate: fromServer }).then(() => queryClient.resetQueries({ predicate: fromServer }));
  }, [serverUrl, queryClient, restart]);
  const [appUpdate, setAppUpdate] = useAppUpdate();
  const [page, setPage] = useState(() => storedNumber(positionKey, 0, 0, preview.pages.length - 1));
  // 選んだときだけ保存する。保存しておかなければ、既定を変えたときにそのまま効く。
  const [readerType, setReaderType] = useState<ReaderType>(() => {
    const defaults = defaultReaderType();
    return {
      fontSize: storedNumber(fontKey, defaults.fontSize, typeLimits.fontSize.min, typeLimits.fontSize.max),
      lineHeight: storedNumber(lineHeightKey, defaults.lineHeight, typeLimits.lineHeight.min, typeLimits.lineHeight.max),
      readerWidth: storedNumber(readerWidthKey, defaults.readerWidth, typeLimits.readerWidth.min, typeLimits.readerWidth.max),
      fontFamily: storedChoice<ReaderFont>(fontFamilyKey, defaults.fontFamily, ["serif", "sans"]),
      readerTheme: storedChoice<ReaderTheme>(readerThemeKey, defaults.readerTheme, ["white", "paper", "night"]),
    };
  });
  const [readerBehavior, setReaderBehavior] = useState<ReaderBehavior>(loadReaderBehavior);
  const [rememberedSort, setRememberedSort] = useState<FixedSort>(loadRememberedSort);
  const [savedConditions, setSavedConditions] = useState<SavedCondition[]>(loadSavedConditions);
  const { fontSize, lineHeight, readerWidth, fontFamily, readerTheme } = readerType;
  const [appearance, setAppearance] = useState<Appearance>(() => storedChoice(appearanceKey, "auto", ["auto", "light", "dark"]));
  const [pagingMode, setPagingMode] = useState<PagingMode>(() => storedChoice(pagingModeKey, "auto", ["auto", "pages"]));
  const readerOpen = onlineWork !== null || offlineWork !== null || reading;
  const detailOpen = selectedWork !== null;

  useLayoutEffect(() => {
    if (!reading || !previewKeyboardPageChange.current) return;
    previewKeyboardPageChange.current = false;
    const viewport = previewScrollRef.current;
    if (previewKeyboardEnd.current && viewport) viewport.scrollTop = viewport.scrollHeight;
    previewKeyboardEnd.current = false;
    viewport?.focus();
  }, [page, reading]);

  useEffect(() => {
    const androidWindow = window as Window & {
      piepAndroidAppearance?: { systemTopInsetPx?: () => number; systemBottomInsetPx?: () => number };
      piepAndroidSystemInsets?: (topPx: number, bottomPx: number) => void;
    };
    const root = document.documentElement;
    const applyInsets = (topPx: number, bottomPx: number) => {
      const scale = window.devicePixelRatio || 1;
      root.style.setProperty("--piep-android-top-inset", `${Math.max(0, topPx / scale)}px`);
      root.style.setProperty("--piep-android-bottom-inset", `${Math.max(0, bottomPx / scale)}px`);
    };
    androidWindow.piepAndroidSystemInsets = applyInsets;
    applyInsets(androidWindow.piepAndroidAppearance?.systemTopInsetPx?.() ?? 0,
      androidWindow.piepAndroidAppearance?.systemBottomInsetPx?.() ?? 0);
    return () => {
      delete androidWindow.piepAndroidSystemInsets;
      root.style.removeProperty("--piep-android-top-inset");
      root.style.removeProperty("--piep-android-bottom-inset");
    };
  }, []);

  // 戻るの順: 開いている小さな面 → 履歴。履歴の端では、誤って閉じないよう一度知らせる。
  useEffect(() => {
    const backWindow = window as Window & { piepAndroidBack?: () => boolean; piepAndroidReaderLayerBack?: () => boolean };
    backWindow.piepAndroidBack = () => {
      if (window.piepAndroidScannerBack?.()) return true;
      if ((window as Window & { piepAndroidSortBack?: () => boolean }).piepAndroidSortBack?.()) return true;
      if (readerOpen && backWindow.piepAndroidReaderLayerBack?.()) return true;
      if (sidebarOpen) { setSidebarOpen(false); return true; }
      if (back()) return true;
      const now = Date.now();
      if (now - lastBackAt.current < 2000) return false;
      lastBackAt.current = now;
      setExitHint(true);
      return true;
    };
    return () => { delete backWindow.piepAndroidBack; };
  }, [readerOpen, sidebarOpen, back]);

  useEffect(() => {
    if (!exitHint) return;
    const timer = window.setTimeout(() => setExitHint(false), 2000);
    return () => window.clearTimeout(timer);
  }, [exitHint]);
  useEffect(() => { setExitHint(false); }, [entry.id]);

  useEffect(() => {
    const scroller = mainRef.current;
    if (!scroller) return;
    const update = () => setScrolledFar(scroller.scrollTop > 1400);
    update();
    scroller.addEventListener("scroll", update, { passive: true });
    return () => scroller.removeEventListener("scroll", update);
  }, []);

  // 回転してサイドバーが常設になったら、開いたままの状態を残さない。
  useEffect(() => {
    const drawer = window.matchMedia("(max-width: 599px), (max-width: 900px) and (orientation: portrait)");
    const closeOnChange = () => setSidebarOpen(false);
    drawer.addEventListener("change", closeOnChange);
    return () => drawer.removeEventListener("change", closeOnChange);
  }, []);

  useEffect(() => {
    const wasOpen = sidebarWasOpenRef.current;
    sidebarWasOpenRef.current = sidebarOpen;
    if (!sidebarOpen && !wasOpen) return;

    let frame = 0;
    let timeout = 0;
    let transitioningSidebar: HTMLElement | null = null;
    let onTransitionEnd: ((event: TransitionEvent) => void) | null = null;
    const focusSidebarControl = () => {
      if (transitioningSidebar?.isConnected
        && transitioningSidebar.classList.contains("mobile-open")
        && getComputedStyle(transitioningSidebar).visibility === "visible") {
        sidebarFirstControlRef.current?.focus({ preventScroll: true });
      }
    };

    frame = window.requestAnimationFrame(() => {
      if (sidebarOpen) {
        transitioningSidebar = document.getElementById("app-navigation");
        if (!transitioningSidebar?.classList.contains("mobile-open")) return;
        onTransitionEnd = (event) => {
          if (event.target !== transitioningSidebar || event.propertyName !== "transform") return;
          window.clearTimeout(timeout);
          transitioningSidebar?.removeEventListener("transitionend", onTransitionEnd!);
          onTransitionEnd = null;
          focusSidebarControl();
        };
        transitioningSidebar.addEventListener("transitionend", onTransitionEnd);
        timeout = window.setTimeout(() => {
          if (transitioningSidebar && onTransitionEnd) {
            transitioningSidebar.removeEventListener("transitionend", onTransitionEnd);
            onTransitionEnd = null;
          }
          focusSidebarControl();
        }, 300);
      } else if (section === "pc") {
        sidebarToggleRef.current?.focus({ preventScroll: true });
      } else {
        document.querySelector<HTMLButtonElement>(".bottom-item[aria-current='page']")?.focus({ preventScroll: true });
      }
    });
    return () => {
      window.cancelAnimationFrame(frame);
      window.clearTimeout(timeout);
      if (transitioningSidebar && onTransitionEnd) {
        transitioningSidebar.removeEventListener("transitionend", onTransitionEnd);
      }
    };
  }, [sidebarOpen, section]);

  useEffect(() => {
    const viewport = window.visualViewport;
    const androidWindow = window as Window & {
      piepAndroidAppearance?: { imeInsetPx?: () => number };
      piepAndroidImeInset?: (insetPx: number) => void;
    };

    const root = document.documentElement;
    const updateKeyboardInset = () => {
      const viewportInset = viewport ? Math.max(0, window.innerHeight - viewport.offsetTop - viewport.height) : 0;
      const nativeInsetPx = androidWindow.piepAndroidAppearance?.imeInsetPx?.() ?? 0;
      const nativeInset = Number.isFinite(nativeInsetPx) ? Math.max(0, nativeInsetPx / window.devicePixelRatio) : 0;
      const obscuredBottom = Math.max(viewportInset, nativeInset);
      root.style.setProperty("--piep-visual-viewport-bottom-inset", `${obscuredBottom}px`);
    };

    androidWindow.piepAndroidImeInset = updateKeyboardInset;
    viewport?.addEventListener("resize", updateKeyboardInset);
    viewport?.addEventListener("scroll", updateKeyboardInset);
    updateKeyboardInset();
    return () => {
      viewport?.removeEventListener("resize", updateKeyboardInset);
      viewport?.removeEventListener("scroll", updateKeyboardInset);
      delete androidWindow.piepAndroidImeInset;
      root.style.removeProperty("--piep-visual-viewport-bottom-inset");
    };
  }, []);

  useEffect(() => {
    if (readerOpen) readerOverlayRef.current?.querySelector("button")?.focus();
    else if (readerReturnFocusRef.current) {
      readerReturnFocusRef.current.focus();
      readerReturnFocusRef.current = null;
    }
  }, [readerOpen]);

  useEffect(() => {
    if (detailOpen) detailOverlayRef.current?.querySelector("button")?.focus();
    else if (detailReturnFocusRef.current) {
      detailReturnFocusRef.current.focus();
      detailReturnFocusRef.current = null;
    }
  }, [detailOpen]);

  useEffect(() => {
    if (!readerOpen && !detailOpen) return;
    const handleModalKeys = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (readerOpen) {
          const backWindow = window as Window & { piepAndroidReaderLayerBack?: () => boolean };
          if (!backWindow.piepAndroidReaderLayerBack?.()) closeReader();
        } else {
          closeDetail();
        }
        event.preventDefault();
        return;
      }
      if (event.key !== "Tab") return;
      const overlay = readerOpen ? readerOverlayRef.current : detailOverlayRef.current;
      if (!overlay) return;
      const scope = overlay.querySelector<HTMLElement>('[role="dialog"]') ?? overlay;
      const focusable = [...scope.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
      )].filter((element) => element.getAttribute("aria-hidden") !== "true");
      if (!focusable.length) {
        event.preventDefault();
        scope.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && (document.activeElement === first || !scope.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !scope.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleModalKeys);
    return () => window.removeEventListener("keydown", handleModalKeys);
  }, [readerOpen, detailOpen]);

  useEffect(() => {
    if (!sidebarOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSidebarOpen(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [sidebarOpen]);

  const showLibrary = (page: LibraryPage) => {
    setSidebarOpen(false);
    push({ page, detail: null, reader: null });
  };

  const showSection = (next: Section) => {
    setSidebarOpen(false);
    if (next === section && next !== "pc") { mainRef.current?.scrollTo({ top: 0, behavior: "smooth" }); return; }
    if (next === "pc") {
      if (currentPage.name === "library") mainRef.current?.scrollTo({ top: 0, behavior: "smooth" });
      else showLibrary(lastLibrary.current);
      return;
    }
    push({ page: next === "offline" ? { name: "offline", tab: "works", groupId: null } : { name: "settings" }, detail: null, reader: null });
  };

  const changeReaderType = (patch: Partial<ReaderType>) => {
    setReaderType((old) => ({ ...old, ...patch }));
    if (patch.fontSize !== undefined) storeSetting(fontKey, String(patch.fontSize));
    if (patch.lineHeight !== undefined) storeSetting(lineHeightKey, String(patch.lineHeight));
    if (patch.readerWidth !== undefined) storeSetting(readerWidthKey, String(patch.readerWidth));
    if (patch.fontFamily !== undefined) storeSetting(fontFamilyKey, patch.fontFamily);
    if (patch.readerTheme !== undefined) storeSetting(readerThemeKey, patch.readerTheme);
  };

  const changeReaderBehavior = (patch: Partial<ReaderBehavior>) => {
    setReaderBehavior((old) => {
      const next = { ...old, ...patch };
      saveReaderBehavior(next);
      return next;
    });
  };

  const changeRememberedSort = (sort: FixedSort) => {
    setRememberedSort(sort);
    saveRememberedSort(sort);
  };

  const resetReaderSettings = () => {
    const defaults = defaultReaderType();
    for (const key of [fontKey, lineHeightKey, readerWidthKey, fontFamilyKey, readerThemeKey]) {
      try { window.localStorage.removeItem(key); } catch { /* storage unavailable */ }
    }
    setReaderType(defaults);
    setReaderBehavior(defaultReaderBehavior);
    saveReaderBehavior(defaultReaderBehavior);
  };

  const changeSavedConditions = (next: SavedCondition[]) => {
    setSavedConditions(next);
    saveSavedConditions(next);
  };

  const saveCondition = (name: string, criteria: PcSearchCriteria) => {
    const existing = savedConditions.find((item) => item.name === name);
    const entry: SavedCondition = { id: existing?.id ?? `${Date.now()}`, name, criteria };
    changeSavedConditions(existing ? savedConditions.map((item) => item === existing ? entry : item) : [...savedConditions, entry]);
  };

  const showShelf = (shelf: Shelf) => {
    if (shelf === "reading") { showLibrary({ ...homePage, shelf: "reading" }); return; }
    showLibrary({ ...homePage, criteria: { ...defaultPcSearch, favorite: shelf === "favorite", revision: shelf === "revised" ? "revised" : "" } });
  };

  const showPreview = () => {
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSidebarOpen(false);
    push({ reader: { kind: "preview" } });
  };

  const openOnlineWork = (work: PcWork) => {
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    push({ reader: { kind: "online", work } });
  };

  const openOnlineDetail = (work: PcWork) => {
    detailReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    push({ detail: { work, offline: false } });
  };

  const openOfflineWork = (work: OfflineWork) => {
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    push({ reader: { kind: "offline", work } });
  };

  const openOfflineDetail = (work: OfflineWork) => {
    detailReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    push({ detail: { work: work.work, offline: true, snapshot: work } });
  };

  const startReading = () => {
    if (!selectedWork) return;
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    push({ reader: selectedWork.offline ? { kind: "offline", work: selectedWork.snapshot } : { kind: "online", work: selectedWork.work } });
  };

  const closeDetail = () => { back(); };
  const savedWorkChanged = () => setSavedRevision((revision) => revision + 1);

  const closeReader = () => { back(); };

  // 圏外で読んだ位置を、アプリへ戻るたびに届ける。
  useEffect(() => {
    if (!isTauri()) return;
    const deliver = () => { if (document.visibilityState === "visible") void sendReading(); };
    deliver();
    document.addEventListener("visibilitychange", deliver);
    return () => document.removeEventListener("visibilitychange", deliver);
  }, []);
  useEffect(() => { try { window.localStorage.setItem(positionKey, String(page)); } catch { /* storage unavailable */ } }, [page]);
  useEffect(() => { try { window.localStorage.setItem(pagingModeKey, pagingMode); } catch { /* storage unavailable */ } }, [pagingMode]);
  useEffect(() => {
    document.documentElement.dataset.piepAppearance = appearance;
    try { window.localStorage.setItem(appearanceKey, appearance); } catch { /* storage unavailable */ }
    const deviceTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const syncSystemBars = () => {
      const nativeAppearance = (window as Window & { piepAndroidAppearance?: { setDark: (dark: boolean) => void } }).piepAndroidAppearance;
      nativeAppearance?.setDark(appearance === "dark" || (appearance === "auto" && deviceTheme.matches));
    };
    syncSystemBars();
    deviceTheme.addEventListener("change", syncSystemBars);
    return () => deviceTheme.removeEventListener("change", syncSystemBars);
  }, [appearance]);

  useEffect(() => {
    if (!isTauri() || !pcConnection?.paired || section !== "offline") return;
    let active = true;
    const check = () => { void getPcStatus().then((next) => { if (active) setPcConnection(next); }).catch(() => { if (active) setPcConnection((old) => old ? { ...old, reachable: false, issue: "offline" } : old); }); };
    check();
    const timer = window.setInterval(check, 30_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [section, pcConnection?.paired]);

  const canBrowseServer = Boolean(pcConnection?.reachable);
  const canWriteFavorite = canBrowseServer && Boolean(pcConnection?.capabilities.includes("favorite-write"));
  const navigation = useMemo(() => ({ entry, depth, push, replace, back, canBrowseServer, canWriteFavorite }), [entry, depth, push, replace, back, canBrowseServer, canWriteFavorite]);
  const favoriteNotice = useFavoriteNotice();

  const readerProps = { type: readerType, behavior: readerBehavior, onBack: closeReader, onTypeChange: changeReaderType, onBehaviorChange: changeReaderBehavior, backLabel: selectedWork ? "作品詳細に戻る" : "作品一覧に戻る" };
  const activeReader = offlineWork
    ? <OnlineReader key={`saved:${offlineWork.work.id}`} work={offlineWork.work} offline offlineCollections={offlineWork.collections ?? []} offlineEditHash={offlineWork.savedEditHash} offlineAuthorCover={Boolean(offlineWork.hasAuthorCover)} snapshotGeneration={offlineWork.savedAtMs} {...readerProps} />
    : onlineWork
      ? <OnlineReader key={`pc:${onlineWork.id}`} work={onlineWork} onSaved={savedWorkChanged} {...readerProps} />
      : reading ? (
      <div className={`reader-screen reader-theme--${readerTheme} reader-bars--pinned`}>
        <header className="reader-toolbar">
          <button className="reader-icon-button" onClick={closeReader} aria-label="本棚に戻る"><ChevronLeft aria-hidden="true" /></button>
          <span className="reader-toolbar-title">表示サンプル</span>
          <button className="reader-icon-button" onClick={() => changeReaderType({ fontSize: fontSize >= typeLimits.fontSize.max ? typeLimits.fontSize.min : fontSize + 2 })} aria-label="文字を大きくする"><Plus aria-hidden="true" /></button>
        </header>
        <main className="reader-scroll" key={page} ref={previewScrollRef} tabIndex={0} aria-label="表示サンプルの本文。Page Up、Page Down、左右矢印、Spaceで移動" onKeyDown={(event) => handleReaderKeyboard(event, page, preview.pages.length, (next, edge) => { previewKeyboardPageChange.current = true; previewKeyboardEnd.current = edge === "end"; setPage(next); })}>
            <article className={`reading-paper reader-font--${fontFamily}`} style={{ maxWidth: `${readerWidth}px`, fontSize: `${fontSize}px`, lineHeight }}>
            <div className="reader-eyebrow">PIEP · READING PREVIEW</div>
            {page === 0 && <><h1>{preview.title}</h1><p className="reader-author">{preview.author}</p><div className="reader-rule" /></>}
            <h2>{preview.pages[page].heading}</h2>
            {preview.pages[page].paragraphs.map((paragraph) => <p key={paragraph}>{paragraph}</p>)}
          </article>
        </main>
        <footer className="reader-sample-footer">
          <button onClick={() => setPage((value) => Math.max(0, value - 1))} disabled={page === 0}>前のページ</button>
          <span aria-live="polite">{page + 1} / {preview.pages.length}</span>
          <button onClick={() => setPage((value) => Math.min(preview.pages.length - 1, value + 1))} disabled={page === preview.pages.length - 1}>次のページ</button>
        </footer>
      </div>
    ) : null;

  return (
    <NavigationProvider value={navigation}>
    <div className={`app-shell app-shell--${section}`} aria-hidden={readerOpen || detailOpen} inert={readerOpen || detailOpen}>
      <AppSidebar open={sidebarOpen} section={section} page={currentPage} status={pcConnection} savedConditions={savedConditions} firstControlRef={sidebarFirstControlRef} onSection={showSection} onShelf={showShelf} onCondition={(condition) => showLibrary({ ...homePage, criteria: condition.criteria })} onRemoveCondition={(condition) => changeSavedConditions(savedConditions.filter((item) => item !== condition))} />
      <div className="main-column">
        <header className="mobile-header"><button ref={sidebarToggleRef} className="sidebar-toggle" type="button" onClick={() => setSidebarOpen((open) => !open)} aria-label={sidebarOpen ? "サイドバーを閉じる" : "サイドバーを開く"} aria-expanded={sidebarOpen} aria-controls="app-navigation"><Icon name="menu" /></button><PiepLockup size={22} /></header>
        {sidebarOpen && <button className="mobile-sidebar-scrim" type="button" onClick={() => setSidebarOpen(false)} aria-label="サイドバーを閉じる" />}
        <main className="main-content" ref={mainRef}>
          {currentPage.name === "library" && <AppUpdateNotice info={appUpdate} onOpenSettings={() => showSection("settings")} />}
          {currentPage.name === "library" && <PcLibrary page={currentPage} status={pcConnection} statusLoading={isTauri() && statusQuery.isPending} onOpenWork={openOnlineWork} onOpenDetail={openOnlineDetail} onGoToSettings={() => showSection("settings")} onSaved={savedWorkChanged} onSaveCondition={saveCondition} savedRevision={savedRevision} pagingMode={pagingMode} rememberedSort={rememberedSort} onRememberedSortChange={changeRememberedSort} />}
          {currentPage.name === "entity" && <EntityPage key={`${currentPage.kind}:${currentPage.source}:${currentPage.key}`} page={currentPage} status={pcConnection} savedRevision={savedRevision} pagingMode={pagingMode} onOpenWork={openOnlineWork} onOpenDetail={openOnlineDetail} onSaved={savedWorkChanged} />}
          {currentPage.name === "offline" && <OfflineLibrary page={currentPage} onOpenWork={openOfflineWork} onOpenDetail={openOfflineDetail} onShowPc={() => showSection("pc")} savedRevision={savedRevision} pagingMode={pagingMode} rememberedSort={rememberedSort} onRememberedSortChange={changeRememberedSort} />}
          {currentPage.name === "settings" && <>
            <div className="settings-page">
              <h1 className="settings-title">設定</h1>
              <h2 className="settings-group">接続</h2>
              <PcConnectionSettings status={pcConnection} onStatusChange={setPcConnection} />
              <h2 className="settings-group">読書</h2>
              <section className="settings-card"><div className="setting-title"><div><h2>読書の表示</h2><p>読書中も右上のボタンから変えられます。</p></div></div><ReaderTypeControls value={readerType} onChange={changeReaderType} /><div className="settings-actions"><button className="secondary-button" onClick={resetReaderSettings}>初期値に戻す</button><button className="secondary-button" onClick={showPreview}>表示サンプルで試す</button></div></section>
              <section className="settings-card"><div className="setting-title"><div><h2>読み方とページ送り</h2><p>つなげて読むかどうかと、タップ、スワイプ、音量キーでの送り方を選べます。</p></div></div><ReaderBehaviorControls value={readerBehavior} onChange={changeReaderBehavior} /></section>
              <h2 className="settings-group">表示</h2>
              <section className="settings-card"><div className="setting-choice"><span>カラーテーマ</span><div role="group" aria-label="アプリのカラーテーマ"><button className={appearance === "auto" ? "selected" : ""} onClick={() => setAppearance("auto")}>システム</button><button className={appearance === "light" ? "selected" : ""} onClick={() => setAppearance("light")}>ライト</button><button className={appearance === "dark" ? "selected" : ""} onClick={() => setAppearance("dark")}>ダーク</button></div></div><div className="setting-choice"><span>作品一覧の続き</span><div role="group" aria-label="一覧の読み込み方"><button className={pagingMode === "auto" ? "selected" : ""} onClick={() => setPagingMode("auto")}>スクロールで自動</button><button className={pagingMode === "pages" ? "selected" : ""} onClick={() => setPagingMode("pages")}>ページ番号</button></div></div><p className="settings-help">関連度順の検索は自動読み込みになります。ページ番号は先頭 251 ページまで選べます。</p></section>
              <h2 className="settings-group">管理</h2>
              <AppUpdateSettings info={appUpdate} onInfo={setAppUpdate} />
            </div>
          </>}
        </main>
        {scrolledFar && <button className="scroll-top" type="button" aria-label="一番上へ" onClick={() => mainRef.current?.scrollTo({ top: 0, behavior: "smooth" })}><ArrowUp aria-hidden="true" /></button>}
        <nav className="bottom-nav" aria-label="下部ナビゲーション">{sections.map((item) => <button key={item.id} className={section === item.id ? "bottom-item active" : "bottom-item"} onClick={() => showSection(item.id)} aria-current={section === item.id ? "page" : undefined}><Icon name={item.icon} /><span>{item.label}</span></button>)}</nav>
      </div>
    </div>
    {selectedWork && <div className="work-detail-overlay" ref={detailOverlayRef} aria-hidden={readerOpen} inert={readerOpen} onClick={(event) => { if (event.target === event.currentTarget && !readerOpen) closeDetail(); }}><WorkDetail key={`${selectedWork.offline ? "saved" : "pc"}:${selectedWork.work.id}`} selected={selectedWork} onBack={closeDetail} onRead={startReading} onSaved={savedWorkChanged} onRemoved={() => { savedWorkChanged(); if (selectedWork.offline) closeDetail(); }} savedRevision={savedRevision} /></div>}
    {activeReader && <div className="reader-overlay" ref={readerOverlayRef}>{activeReader}</div>}
    {exitHint && <div className="app-toast" role="status">もう一度押すと閉じます</div>}
    {favoriteNotice && !exitHint && <div className="app-toast" role="alert">{favoriteNotice}</div>}
    </NavigationProvider>
  );
}

export default App;
