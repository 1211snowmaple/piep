import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { BookOpen, LibraryBig, Menu, Settings } from "lucide-react";
import "./App.css";
import "./App.desktop.css";
import { PiepLockup } from "./PiepLockup";
import { PcLibrary } from "./PcLibrary";
import { PcConnectionSettings } from "./PcConnectionSettings";
import { OnlineReader } from "./OnlineReader";
import { OfflineLibrary } from "./OfflineLibrary";
import { WorkDetail, type SelectedWork } from "./WorkDetail";
import { defaultPcSearch, getPcStatus, type OfflineWork, type PcConnectionStatus, type PcSearchCriteria, type PcWork } from "./pcApi";
import { handleReaderKeyboard } from "./readerKeyboard";

type Section = "pc" | "offline" | "settings";
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
const fontFamilyKey = "piep.android.reader.font-family";
const readerThemeKey = "piep.android.reader.theme";
const appearanceKey = "piep.android.appearance";
const pagingModeKey = "piep.android.library.paging-mode";

type ReaderFont = "serif" | "sans";
type ReaderTheme = "white" | "paper" | "night";
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

function Icon({ name }: { name: IconName }) {
  const Component = { computer: LibraryBig, book: BookOpen, settings: Settings, menu: Menu }[name];
  return <Component className="icon" aria-hidden="true" />;
}

const sections: { id: Section; label: string; icon: IconName }[] = [
  { id: "pc", label: "ライブラリ", icon: "computer" },
  { id: "offline", label: "端末内", icon: "book" },
  { id: "settings", label: "設定", icon: "settings" },
];

function isOnlyShelf(search: PcSearchCriteria, favorite: boolean) {
  return search.favorite === favorite && Object.entries(defaultPcSearch).every(([key, value]) =>
    ["favorite", "sort", "tagMode", "dateField"].includes(key) || search[key as keyof PcSearchCriteria] === value);
}

function App() {
  const [section, setSection] = useState<Section>("pc");
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
  const [selectedWork, setSelectedWork] = useState<SelectedWork | null>(null);
  const [savedRevision, setSavedRevision] = useState(0);
  const [reading, setReading] = useState(false);
  const [onlineWork, setOnlineWork] = useState<PcWork | null>(null);
  const [offlineWork, setOfflineWork] = useState<OfflineWork | null>(null);
  const [pcConnection, setPcConnection] = useState<PcConnectionStatus | null>(null);
  const [pcSearch, setPcSearch] = useState<PcSearchCriteria>(defaultPcSearch);
  const [pcNavigationKey, setPcNavigationKey] = useState(0);
  const [sidebarSearch, setSidebarSearch] = useState<PcSearchCriteria>(defaultPcSearch);
  const [sidebarSearchError, setSidebarSearchError] = useState<string | null>(null);
  const [page, setPage] = useState(() => storedNumber(positionKey, 0, 0, preview.pages.length - 1));
  const [fontSize, setFontSize] = useState(() => storedNumber(fontKey, 18, 14, 28));
  const [lineHeight, setLineHeight] = useState(() => storedNumber(lineHeightKey, 1.72, 1.4, 2.4));
  const [readerWidth, setReaderWidth] = useState(() => storedNumber(readerWidthKey, 680, 520, 900));
  const [fontFamily, setFontFamily] = useState<ReaderFont>(() => storedChoice(fontFamilyKey, "serif", ["serif", "sans"]));
  const [readerTheme, setReaderTheme] = useState<ReaderTheme>(() => storedChoice(readerThemeKey, "white", ["white", "paper", "night"]));
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

  useEffect(() => {
    const backWindow = window as Window & { piepAndroidBack?: () => boolean; piepAndroidReaderLayerBack?: () => boolean };
    backWindow.piepAndroidBack = () => {
      if (readerOpen) {
        if (backWindow.piepAndroidReaderLayerBack?.()) return true;
        setOnlineWork(null);
        setOfflineWork(null);
        setReading(false);
        return true;
      }
      if (detailOpen) { setSelectedWork(null); return true; }
      if (sidebarOpen) { setSidebarOpen(false); return true; }
      return false;
    };
    return () => { delete backWindow.piepAndroidBack; };
  }, [readerOpen, detailOpen, sidebarOpen]);

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
    const sidebar = document.getElementById("app-navigation");
    const viewport = window.visualViewport;
    if (!sidebar) return;
    const androidWindow = window as Window & {
      piepAndroidAppearance?: { imeInsetPx?: () => number };
      piepAndroidImeInset?: (insetPx: number) => void;
    };

    const root = document.documentElement;
    let frame = 0;
    const revealFocusedField = () => {
      const active = document.activeElement;
      if (!(active instanceof HTMLInputElement || active instanceof HTMLSelectElement || active instanceof HTMLTextAreaElement)
        || !sidebar.contains(active)) return;
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        active.scrollIntoView({ block: "nearest", inline: "nearest" });
      });
    };
    const updateKeyboardInset = () => {
      const viewportInset = viewport ? Math.max(0, window.innerHeight - viewport.offsetTop - viewport.height) : 0;
      const nativeInsetPx = androidWindow.piepAndroidAppearance?.imeInsetPx?.() ?? 0;
      const nativeInset = Number.isFinite(nativeInsetPx) ? Math.max(0, nativeInsetPx / window.devicePixelRatio) : 0;
      const obscuredBottom = Math.max(viewportInset, nativeInset);
      root.style.setProperty("--piep-visual-viewport-bottom-inset", `${obscuredBottom}px`);
      if (obscuredBottom > 80) revealFocusedField();
    };

    sidebar.addEventListener("focusin", revealFocusedField);
    androidWindow.piepAndroidImeInset = updateKeyboardInset;
    viewport?.addEventListener("resize", updateKeyboardInset);
    viewport?.addEventListener("scroll", updateKeyboardInset);
    updateKeyboardInset();
    return () => {
      sidebar.removeEventListener("focusin", revealFocusedField);
      viewport?.removeEventListener("resize", updateKeyboardInset);
      viewport?.removeEventListener("scroll", updateKeyboardInset);
      delete androidWindow.piepAndroidImeInset;
      window.cancelAnimationFrame(frame);
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

  const showSection = (next: Section) => {
    setSection(next);
    setSidebarOpen(false);
  };

  useEffect(() => { setSidebarSearch(pcSearch); }, [pcSearch]);

  const applySidebarSearch = () => {
    if (sidebarSearch.minChars && sidebarSearch.maxChars && Number(sidebarSearch.minChars) > Number(sidebarSearch.maxChars)) {
      setSidebarSearchError("文字数の下限は上限以下にしてください。");
      return;
    }
    if (sidebarSearch.minAssets && sidebarSearch.maxAssets && Number(sidebarSearch.minAssets) > Number(sidebarSearch.maxAssets)) {
      setSidebarSearchError("画像数の下限は上限以下にしてください。");
      return;
    }
    if (sidebarSearch.dateFrom && sidebarSearch.dateTo && sidebarSearch.dateFrom > sidebarSearch.dateTo) {
      setSidebarSearchError("日付の開始日は終了日以前にしてください。");
      return;
    }
    for (const value of [sidebarSearch.author, sidebarSearch.authorExclude, sidebarSearch.tag, sidebarSearch.tagExclude]) {
      if (value.split(/[\n,、]+/).filter((part) => part.trim()).length > 10) {
        setSidebarSearchError("作者とタグはそれぞれ 10 件まで指定できます。");
        return;
      }
    }
    setSidebarSearchError(null);
    setPcSearch({ ...sidebarSearch, text: sidebarSearch.text.trim(), author: sidebarSearch.author.trim(), authorExclude: sidebarSearch.authorExclude.trim(), tag: sidebarSearch.tag.trim(), tagExclude: sidebarSearch.tagExclude.trim(), minChars: sidebarSearch.minChars.trim(), maxChars: sidebarSearch.maxChars.trim(), minAssets: sidebarSearch.minAssets.trim(), maxAssets: sidebarSearch.maxAssets.trim() });
    setPcNavigationKey((key) => key + 1);
    showSection("pc");
    document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
  };

  const showPcShelf = (favorite: boolean) => {
    setPcSearch({ ...defaultPcSearch, favorite });
    setPcNavigationKey((key) => key + 1);
    showSection("pc");
    document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
  };

  const showPreview = () => {
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSidebarOpen(false);
    setReading(true);
  };

  const openOnlineWork = (work: PcWork) => {
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setOnlineWork(work);
  };

  const openOnlineDetail = (work: PcWork) => {
    detailReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSelectedWork({ work, offline: false });
  };

  const openOfflineWork = (work: OfflineWork) => {
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setOfflineWork(work);
  };

  const openOfflineDetail = (work: OfflineWork) => {
    detailReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSelectedWork({ work: work.work, offline: true, snapshot: work });
  };

  const startReading = () => {
    if (!selectedWork) return;
    readerReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (selectedWork.offline) setOfflineWork(selectedWork.snapshot);
    else setOnlineWork(selectedWork.work);
  };

  const closeDetail = () => setSelectedWork(null);
  const savedWorkChanged = () => setSavedRevision((revision) => revision + 1);

  const closeReader = () => {
    setOnlineWork(null);
    setOfflineWork(null);
    setReading(false);
  };

  useEffect(() => { try { window.localStorage.setItem(positionKey, String(page)); } catch { /* storage unavailable */ } }, [page]);
  useEffect(() => { try { window.localStorage.setItem(fontKey, String(fontSize)); } catch { /* storage unavailable */ } }, [fontSize]);
  useEffect(() => { try { window.localStorage.setItem(pagingModeKey, pagingMode); } catch { /* storage unavailable */ } }, [pagingMode]);
  useEffect(() => { try { window.localStorage.setItem(lineHeightKey, String(lineHeight)); } catch { /* storage unavailable */ } }, [lineHeight]);
  useEffect(() => { try { window.localStorage.setItem(readerWidthKey, String(readerWidth)); } catch { /* storage unavailable */ } }, [readerWidth]);
  useEffect(() => { try { window.localStorage.setItem(fontFamilyKey, fontFamily); } catch { /* storage unavailable */ } }, [fontFamily]);
  useEffect(() => { try { window.localStorage.setItem(readerThemeKey, readerTheme); } catch { /* storage unavailable */ } }, [readerTheme]);
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

  const activeReader = offlineWork
    ? <OnlineReader key={`saved:${offlineWork.work.id}`} work={offlineWork.work} offline offlineCollections={offlineWork.collections ?? []} offlineEditHash={offlineWork.savedEditHash} offlineAuthorCover={Boolean(offlineWork.hasAuthorCover)} snapshotGeneration={offlineWork.savedAtMs} fontSize={fontSize} lineHeight={lineHeight} readerWidth={readerWidth} fontFamily={fontFamily} readerTheme={readerTheme} onBack={closeReader} onFontSizeChange={setFontSize} onLineHeightChange={setLineHeight} onReaderWidthChange={setReaderWidth} onFontFamilyChange={setFontFamily} onReaderThemeChange={setReaderTheme} backLabel={selectedWork ? "作品詳細に戻る" : "作品一覧に戻る"} />
    : onlineWork
      ? <OnlineReader key={`pc:${onlineWork.id}`} work={onlineWork} fontSize={fontSize} lineHeight={lineHeight} readerWidth={readerWidth} onReaderWidthChange={setReaderWidth} fontFamily={fontFamily} readerTheme={readerTheme} onBack={closeReader} onFontSizeChange={setFontSize} onLineHeightChange={setLineHeight} onFontFamilyChange={setFontFamily} onReaderThemeChange={setReaderTheme} backLabel={selectedWork ? "作品詳細に戻る" : "作品一覧に戻る"} onSaved={savedWorkChanged} />
      : reading ? (
      <div className={`reader-screen reader-theme--${readerTheme}`}>
        <header className="reader-toolbar">
          <button className="text-button back-button" onClick={closeReader} aria-label="本棚に戻る">← <span>戻る</span></button>
          <span className="reader-toolbar-title">表示サンプル</span>
          <button className="text-button reader-setting-button" onClick={() => setFontSize((value) => value >= 28 ? 14 : value + 2)} aria-label="文字を大きくする">あ+</button>
        </header>
        <main className="reader-scroll" key={page} ref={previewScrollRef} tabIndex={0} aria-label="表示サンプルの本文。Page Up、Page Down、左右矢印、Spaceで移動" onKeyDown={(event) => handleReaderKeyboard(event, page, preview.pages.length, (next, edge) => { previewKeyboardPageChange.current = true; previewKeyboardEnd.current = edge === "end"; setPage(next); })}>
            <article className={`reading-paper reader-font--${fontFamily}`} style={{ maxWidth: `${readerWidth}px`, fontSize: `${fontSize}px`, lineHeight }}>
            <div className="reader-eyebrow">PIEP · READING PREVIEW</div>
            {page === 0 && <><h1>{preview.title}</h1><p className="reader-author">{preview.author}</p><div className="reader-rule" /></>}
            <h2>{preview.pages[page].heading}</h2>
            {preview.pages[page].paragraphs.map((paragraph) => <p key={paragraph}>{paragraph}</p>)}
          </article>
        </main>
        <footer className="reader-footer">
          <button onClick={() => setPage((value) => Math.max(0, value - 1))} disabled={page === 0}>前のページ</button>
          <span aria-live="polite">{page + 1} / {preview.pages.length}</span>
          <button onClick={() => setPage((value) => Math.min(preview.pages.length - 1, value + 1))} disabled={page === preview.pages.length - 1}>次のページ</button>
        </footer>
      </div>
    ) : null;

  return (
    <>
    <div className={`app-shell app-shell--${section}`} aria-hidden={readerOpen || detailOpen} inert={readerOpen || detailOpen}>
      <aside className={sidebarOpen ? "sidebar mobile-open" : "sidebar"} id="app-navigation" aria-label="サイドバー">
        <div className="brand"><PiepLockup size={24} /></div>
        <nav className="side-nav" aria-label="画面の移動">{sections.map((item) => <button key={item.id} type="button" ref={item.id === "pc" ? sidebarFirstControlRef : undefined} className={section === item.id ? "nav-item active" : "nav-item"} onClick={() => showSection(item.id)} aria-current={section === item.id ? "page" : undefined}><Icon name={item.icon} /><span>{item.label}</span></button>)}</nav>
        <div className="sidebar-search">
          <span className="sidebar-group-label">PC の本棚</span>
          <div className="sidebar-shelves"><button type="button" className={isOnlyShelf(pcSearch, false) ? "shelf-choice active" : "shelf-choice"} onClick={() => showPcShelf(false)}>すべて</button><button type="button" className={isOnlyShelf(pcSearch, true) ? "shelf-choice active" : "shelf-choice"} onClick={() => showPcShelf(true)}>お気に入り</button></div>
          <form className="sidebar-filter-form" onSubmit={(event) => { event.preventDefault(); applySidebarSearch(); }}>
            <label>取得元<select value={sidebarSearch.source} onChange={(event) => setSidebarSearch((old) => ({ ...old, source: event.target.value as PcSearchCriteria["source"] }))}><option value="">すべて</option><option value="pixiv">pixiv</option><option value="fanbox">FANBOX</option></select></label>
            <label>種類<select value={sidebarSearch.contentType} onChange={(event) => setSidebarSearch((old) => ({ ...old, contentType: event.target.value as PcSearchCriteria["contentType"] }))}><option value="">すべて</option><option value="novel">小説</option><option value="article">記事</option><option value="text">テキスト</option><option value="image">画像</option><option value="file">ファイル</option></select></label>
            <label>作者名<input value={sidebarSearch.author} maxLength={1000} onChange={(event) => setSidebarSearch((old) => ({ ...old, author: event.target.value }))} placeholder="読点区切り" /></label>
            <label>タグ<input value={sidebarSearch.tag} maxLength={1000} onChange={(event) => setSidebarSearch((old) => ({ ...old, tag: event.target.value }))} placeholder="読点区切り" /></label>
            <details className="sidebar-advanced"><summary>詳細条件</summary><div className="sidebar-advanced-fields">
              <label>タグの組み合わせ<select value={sidebarSearch.tagMode} onChange={(event) => setSidebarSearch((old) => ({ ...old, tagMode: event.target.value as PcSearchCriteria["tagMode"] }))}><option value="and">すべて含む</option><option value="or">いずれかを含む</option></select></label>
              <label>除外する作者<input value={sidebarSearch.authorExclude} maxLength={1000} onChange={(event) => setSidebarSearch((old) => ({ ...old, authorExclude: event.target.value }))} placeholder="読点区切り" /></label>
              <label>除外するタグ<input value={sidebarSearch.tagExclude} maxLength={1000} onChange={(event) => setSidebarSearch((old) => ({ ...old, tagExclude: event.target.value }))} placeholder="読点区切り" /></label>
              <div className="sidebar-number-range"><label>文字数の下限<input type="number" inputMode="numeric" min="0" max="1000000000" value={sidebarSearch.minChars} onChange={(event) => setSidebarSearch((old) => ({ ...old, minChars: event.target.value }))} placeholder="指定なし" /></label><label>上限<input type="number" inputMode="numeric" min="0" max="1000000000" value={sidebarSearch.maxChars} onChange={(event) => setSidebarSearch((old) => ({ ...old, maxChars: event.target.value }))} placeholder="指定なし" /></label></div>
              <div className="sidebar-number-range"><label>画像数の下限<input type="number" inputMode="numeric" min="0" max="1000000" value={sidebarSearch.minAssets} onChange={(event) => setSidebarSearch((old) => ({ ...old, minAssets: event.target.value }))} placeholder="指定なし" /></label><label>上限<input type="number" inputMode="numeric" min="0" max="1000000" value={sidebarSearch.maxAssets} onChange={(event) => setSidebarSearch((old) => ({ ...old, maxAssets: event.target.value }))} placeholder="指定なし" /></label></div>
              <label>改稿<select value={sidebarSearch.revision} onChange={(event) => setSidebarSearch((old) => ({ ...old, revision: event.target.value as PcSearchCriteria["revision"] }))}><option value="">すべて</option><option value="revised">改稿あり</option><option value="first_version">初版のみ</option></select></label>
              <label>ローカル編集<select value={sidebarSearch.edit} onChange={(event) => setSidebarSearch((old) => ({ ...old, edit: event.target.value as PcSearchCriteria["edit"] }))}><option value="">すべて</option><option value="edited">編集あり</option><option value="unedited">編集なし</option></select></label>
              <label>表紙<select value={sidebarSearch.cover} onChange={(event) => setSidebarSearch((old) => ({ ...old, cover: event.target.value as PcSearchCriteria["cover"] }))}><option value="">すべて</option><option value="has_cover">表紙あり</option><option value="no_cover">表紙なし</option></select></label>
              <label>日付の種類<select value={sidebarSearch.dateField} onChange={(event) => setSidebarSearch((old) => ({ ...old, dateField: event.target.value as PcSearchCriteria["dateField"] }))}><option value="downloaded_at">保存日</option><option value="source_created_at">公開日</option><option value="source_updated_at">更新日</option></select></label>
              <div className="sidebar-number-range"><label>開始日<input type="date" value={sidebarSearch.dateFrom} onChange={(event) => setSidebarSearch((old) => ({ ...old, dateFrom: event.target.value }))} /></label><label>終了日<input type="date" value={sidebarSearch.dateTo} onChange={(event) => setSidebarSearch((old) => ({ ...old, dateTo: event.target.value }))} /></label></div>
              <p>作者・タグは読点、コンマ、改行で最大10件。検索語には「語句」や -語句も使えます。</p>
            </div></details>
            {sidebarSearchError && <p className="sidebar-filter-error" role="alert">{sidebarSearchError}</p>}
            <label>並び順<select value={sidebarSearch.sort} onChange={(event) => setSidebarSearch((old) => ({ ...old, sort: event.target.value as PcSearchCriteria["sort"] }))}><option value="standard">自動</option><optgroup label="日付"><option value="dateDesc">保存日：新しい順</option><option value="dateAsc">保存日：古い順</option><option value="publishedDesc">公開日：新しい順</option><option value="publishedAsc">公開日：古い順</option></optgroup><optgroup label="名前"><option value="titleAsc">タイトル：昇順</option><option value="titleDesc">タイトル：降順</option><option value="authorAsc">作者名：昇順</option><option value="authorDesc">作者名：降順</option></optgroup><optgroup label="内容"><option value="lengthDesc">文字数：多い順</option><option value="lengthAsc">文字数：少ない順</option><option value="assetsDesc">画像数：多い順</option><option value="assetsAsc">画像数：少ない順</option><option value="versionDesc">版番号：大きい順</option><option value="versionAsc">版番号：小さい順</option></optgroup></select></label>
            <p className="sidebar-sort-hint">自動では、検索語がなければ保存が新しい順に並べます。</p>
            <label className="sidebar-check"><input type="checkbox" checked={sidebarSearch.favorite} onChange={(event) => setSidebarSearch((old) => ({ ...old, favorite: event.target.checked }))} />この条件でお気に入りのみ</label>
            <div className="sidebar-filter-actions"><button type="submit">絞り込む</button><button type="button" onClick={() => { setSidebarSearch(defaultPcSearch); setPcSearch(defaultPcSearch); setPcNavigationKey((key) => key + 1); showSection("pc"); }}>解除</button></div>
          </form>
        </div>
        <div className={pcConnection?.reachable ? "sidebar-bottom connected" : "sidebar-bottom"}><span className="status-dot" /> PC {pcConnection?.reachable ? "接続中" : "未接続"}</div>
      </aside>
      <div className="main-column">
        <header className="mobile-header"><button ref={sidebarToggleRef} className="sidebar-toggle" type="button" onClick={() => setSidebarOpen((open) => !open)} aria-label={sidebarOpen ? "サイドバーを閉じる" : "サイドバーを開く"} aria-expanded={sidebarOpen} aria-controls="app-navigation"><Icon name="menu" /></button><PiepLockup size={22} /><span className="mobile-section-title">{sections.find((item) => item.id === section)?.label}</span></header>
        {sidebarOpen && <button className="mobile-sidebar-scrim" type="button" onClick={() => setSidebarOpen(false)} aria-label="サイドバーを閉じる" />}
        <main className="main-content">
          {section === "pc" && <PcLibrary criteria={pcSearch} onCriteriaChange={setPcSearch} onOpenWork={openOnlineWork} onOpenDetail={openOnlineDetail} onStatusChange={setPcConnection} onGoToSettings={() => showSection("settings")} navigationKey={pcNavigationKey} savedRevision={savedRevision} pagingMode={pagingMode} />}
          {section === "offline" && <OfflineLibrary onOpenWork={openOfflineWork} onOpenDetail={openOfflineDetail} onShowPc={() => showSection("pc")} savedRevision={savedRevision} pagingMode={pagingMode} />}
          {section === "settings" && <>
            <div className="page-heading"><div><p className="eyebrow">PREFERENCES</p><h1>設定</h1><p>この端末での読み方を整えます。</p></div></div>
            <section className="settings-card"><div className="setting-title"><div><h2>読書の表示</h2><p>初期値はデスクトップ版と同じです。変更はこの端末の読書画面に反映されます。</p></div><span className="setting-icon">あ</span></div><label className="slider-row"><span>文字サイズ <strong>{fontSize}px</strong></span><input type="range" min="14" max="28" step="1" value={fontSize} onChange={(event) => setFontSize(Number(event.target.value))} /></label><label className="slider-row"><span>行間 <strong>{lineHeight.toFixed(2)}</strong></span><input type="range" min="1.4" max="2.4" step="0.01" value={lineHeight} onChange={(event) => setLineHeight(Number(event.target.value))} /></label><label className="slider-row"><span>本文幅 <strong>{readerWidth}px</strong></span><input type="range" min="520" max="900" step="20" value={readerWidth} onChange={(event) => setReaderWidth(Number(event.target.value))} /></label><p className="settings-help reader-width-help">広い画面では本文の左右余白を調整します。</p><div className="setting-choice"><span>書体</span><div role="group" aria-label="書体"><button className={fontFamily === "serif" ? "selected" : ""} onClick={() => setFontFamily("serif")}>明朝</button><button className={fontFamily === "sans" ? "selected" : ""} onClick={() => setFontFamily("sans")}>ゴシック</button></div></div><div className="setting-choice"><span>読書背景</span><div role="group" aria-label="読書背景"><button className={readerTheme === "white" ? "selected" : ""} onClick={() => setReaderTheme("white")}>白</button><button className={readerTheme === "paper" ? "selected" : ""} onClick={() => setReaderTheme("paper")}>紙の色</button><button className={readerTheme === "night" ? "selected" : ""} onClick={() => setReaderTheme("night")}>ナイト</button></div></div><button className="secondary-button reset-reading" onClick={() => { setFontSize(18); setLineHeight(1.72); setReaderWidth(680); setFontFamily("serif"); setReaderTheme("white"); }}>読書設定を初期値に戻す</button><button className="secondary-button preview-setting-button" onClick={showPreview}>表示サンプルで試す</button></section>
            <section className="settings-card"><div className="setting-title"><div><h2>一覧の読み込み方</h2><p>PC と端末内の作品一覧で、続きをどう表示するか選べます。</p></div></div><div className="setting-choice"><span>作品一覧</span><div role="group" aria-label="一覧の読み込み方"><button className={pagingMode === "auto" ? "selected" : ""} onClick={() => setPagingMode("auto")}>スクロールで自動</button><button className={pagingMode === "pages" ? "selected" : ""} onClick={() => setPagingMode("pages")}>ページ番号</button></div></div><p className="settings-help">関連度順の検索は自動読み込みになります。PC のページ番号は先頭 251 ページまで選べます。全作品をたどるときは自動読み込みを使ってください。</p></section>
            <section className="settings-card"><div className="setting-title"><div><h2>カラーテーマ</h2><p>デスクトップ版と同じく、初期状態はシステム設定に従います。</p></div></div><div className="setting-choice"><span>アプリの表示</span><div role="group" aria-label="アプリのカラーテーマ"><button className={appearance === "auto" ? "selected" : ""} onClick={() => setAppearance("auto")}>システム</button><button className={appearance === "light" ? "selected" : ""} onClick={() => setAppearance("light")}>ライト</button><button className={appearance === "dark" ? "selected" : ""} onClick={() => setAppearance("dark")}>ダーク</button></div></div></section>
            <PcConnectionSettings status={pcConnection} onStatusChange={setPcConnection} />
          </>}
        </main>
        <nav className="bottom-nav" aria-label="下部ナビゲーション">{sections.map((item) => <button key={item.id} className={section === item.id ? "bottom-item active" : "bottom-item"} onClick={() => showSection(item.id)} aria-current={section === item.id ? "page" : undefined}><Icon name={item.icon} /><span>{item.label}</span></button>)}</nav>
      </div>
    </div>
    {selectedWork && <div className="work-detail-overlay" ref={detailOverlayRef} aria-hidden={readerOpen} inert={readerOpen} onClick={(event) => { if (event.target === event.currentTarget && !readerOpen) closeDetail(); }}><WorkDetail key={`${selectedWork.offline ? "saved" : "pc"}:${selectedWork.work.id}`} selected={selectedWork} onBack={closeDetail} onRead={startReading} onSaved={savedWorkChanged} onRemoved={() => { savedWorkChanged(); if (selectedWork.offline) closeDetail(); }} savedRevision={savedRevision} /></div>}
    {activeReader && <div className="reader-overlay" ref={readerOverlayRef}>{activeReader}</div>}
    </>
  );
}

export default App;
