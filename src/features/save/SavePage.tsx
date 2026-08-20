import { useCallback, useEffect, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  Loader,
  Menu,
  Progress,
  ScrollArea,
  SegmentedControl,
  Stack,
  Text,
  TextInput,
  ThemeIcon,
  Title,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useLocalStorage } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate, useAppSearchParams, useRouteParams } from "@/app/router";
import {
  describeDownloadTarget,
  downloadTargetKey,
  type DownloadTargetKind,
  type FanboxPost,
  type PixivNovel,
  type SidebarDownloadType,
  type SidebarItem,
} from "@/features/browser/downloadCandidates";
import { normalizeFanboxPostPayload, normalizeFanboxSaveMetadata, normalizePixivSaveMetadata } from "@/features/browser/downloadMetadata";
import { refreshEntityProfilesForEntries, type UpdateDownloadEntry } from "@/features/updates/updateWorkflow";
import { errorMessage } from "@/lib/format";
import { getProvider, ProviderMark } from "@/lib/providers";
import { registerUnsavedGuard } from "@/lib/unsavedGuard";
import { createSourcePacer, isRateLimited, MAX_RATE_LIMIT_RETRIES } from "@/lib/sourcePacing";
import { useEmbeddedBrowserOverlay } from "@/features/browser/useEmbeddedBrowserOverlay";
import {
  closeEmbeddedBrowser,
  closeStandaloneBrowser,
  destroyEmbeddedBrowser,
  getEmbeddedBrowserUrl,
  getStandaloneBrowserUrl,
  goBackEmbeddedBrowser,
  goForwardEmbeddedBrowser,
  navigateEmbeddedBrowser,
  openEmbeddedBrowser,
  openStandaloneBrowser,
  reloadEmbeddedBrowser,
  setEmbeddedBrowserBounds,
  setEmbeddedBrowserVisible,
  type BrowserAcceleratorEvent,
  type StandaloneBrowserClosedEvent,
  type StandaloneBrowserUrlEvent,
} from "@/services/browserApi";
import { getDownloadBySource, isTauriRuntime, setWatchUpdates } from "@/services/dbApi";
import { loadSchedule } from "@/features/updates/updateSchedule";
import {
  downloadAndSave,
  fetchFanboxCreatorPosts,
  fetchFanboxPost,
  fetchPixivNovel,
  fetchPixivNovelByUrl,
  fetchPixivSeriesNovels,
  fetchPixivUserNovels,
} from "@/services/downloadApi";
import { subscribeTauriEvent } from "@/services/eventBus";
import { store } from "@/store";
import type { DownloadEntry } from "@/types/library";
import { requestOperationCancel, startOperation, type OperationController } from "@/features/jobs/operationJobs";

type SaveSource = "pixiv" | "fanbox";

export default function SavePage() {
  const { source: routeSource } = useRouteParams("/save/:source?");
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [searchParams] = useAppSearchParams();
  const runtime = isTauriRuntime();
  const source: SaveSource = routeSource === "fanbox" ? "fanbox" : "pixiv";
  const initialUrl = searchParams.get("url") || getProvider(source).homeUrl || "https://www.pixiv.net/";
  const [currentUrl, setCurrentUrl] = useState(initialUrl);
  const [address, setAddress] = useState(initialUrl);
  const [items, setItems] = useState<SidebarItem[]>([]);
  const [downloadType, setDownloadType] = useState<SidebarDownloadType | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [saving, setSaving] = useState(false);
  // 中止は押した瞬間に見えなければならない。取り消しが実際に効くのは次の
  // 待ちの切れ目なので、その間ボタンが何も言わないと押せていないと読まれる。
  const [canceling, setCanceling] = useState(false);
  const [progress, setProgress] = useState<{ current: number; total: number; text: string } | null>(null);
  const [lastAnalysisUrl, setLastAnalysisUrl] = useState<string | null>(null);
  // 一覧がどのページのものかは、URLではなく「相手」で覚える。
  const [lastAnalysisKey, setLastAnalysisKey] = useState<string | null>(null);
  const [authConnected, setAuthConnected] = useState<boolean | null>(null);
  const [candidateWidth, setCandidateWidth] = useLocalStorage({ key: "piep.save-candidate-width", defaultValue: 360 });
  const [candidateCollapsed, setCandidateCollapsed] = useState(false);
  // While the large window is open the in-app pane hands its job over to it:
  // two live sessions on the same provider would fight over the login state,
  // and the native child WebView would still paint over the app.
  const [detached, setDetached] = useState(false);
  const [history, setHistory] = useState<string[]>([]);
  const browserViewportRef = useRef<HTMLDivElement>(null);
  const layoutRef = useRef<HTMLDivElement>(null);
  const addressFocusedRef = useRef(false);
  const acceleratorHandlerRef = useRef<(payload: BrowserAcceleratorEvent) => void>(() => undefined);
  const saveOperationRef = useRef<OperationController | null>(null);
  // 保存中かどうかは、描画の外からも即座に読めなければならない - 終了ガードと
  // 再入の防止が、どちらも state の反映を待てないため。
  const savingRef = useRef(false);
  // 取得も同じ。Ctrl+S と取得ボタンが同じ瞬間に入ると、state の反映を待たずに
  // もう一周始まり、同じ知らせが二度出る。
  const analyzingRef = useRef(false);
  const executeRef = useRef<() => void>(() => undefined);

  useEmbeddedBrowserOverlay(browserViewportRef, runtime && !detached);

  // A page is only worth remembering once, and only the most recent handful are
  // worth offering back.
  const rememberVisit = useCallback((url: string) => {
    if (!url || url === "about:blank") return;
    setHistory((current) => [url, ...current.filter((item) => item !== url)].slice(0, 24));
  }, []);

  const readBounds = useCallback(() => {
    const element = browserViewportRef.current;
    if (!runtime || !element) return null;
    const rect = element.getBoundingClientRect();
    if (rect.width < 100 || rect.height < 100) return null;
    return { x: Math.round(rect.left), y: Math.round(rect.top), width: Math.round(rect.width), height: Math.round(rect.height) };
  }, [runtime]);

  const positionBrowser = useCallback(async (url: string) => {
    const userAgent = source === "fanbox" ? await store.get<string>("fanbox_user_agent") || undefined : undefined;
    // Measured after the await: the layout can settle while the store is read.
    const bounds = readBounds();
    if (!bounds) return;
    await openEmbeddedBrowser(url, { ...bounds, userAgent });
    // Let the reconciler re-apply, since creation may land after another pass.
    appliedBoundsRef.current = "";
  }, [readBounds, source]);

  // Resizing must never pass a URL: the tracked URL lags behind in-page (SPA)
  // navigation, so reusing the open path here would yank the user back to a
  // stale page mid-drag.
  //
  // This is a reconciliation, not an event handler. The native view drifts out
  // of step with its placeholder for more reasons than can be caught one by one
  // - the view being created after a layout pass, an observer callback landing
  // before it exists, a scale change - and once it drifts nothing puts it back.
  // Comparing against the last applied rectangle makes every call cheap enough
  // to also run on a timer, so any drift corrects itself.
  const appliedBoundsRef = useRef<string>("");
  const initializedSourceRef = useRef<SaveSource | null>(null);
  const detachedRef = useRef(detached);
  detachedRef.current = detached;
  const currentUrlRef = useRef(currentUrl);
  currentUrlRef.current = currentUrl;
  const syncBrowserBounds = useCallback(() => {
    if (detachedRef.current) return;
    const bounds = readBounds();
    if (!bounds) return;
    const key = `${bounds.x},${bounds.y},${bounds.width},${bounds.height}`;
    if (key === appliedBoundsRef.current) return;
    appliedBoundsRef.current = key;
    setEmbeddedBrowserBounds(bounds)
      .then((applied) => { if (!applied) appliedBoundsRef.current = ""; })
      .catch(() => { appliedBoundsRef.current = ""; });
  }, [readBounds]);

  useEffect(() => {
    let cancelled = false;
    const home = searchParams.get("url") || getProvider(source).homeUrl || "https://www.pixiv.net/";
    setCurrentUrl(home); setAddress(home); setItems([]); setDownloadType(null); setLastAnalysisUrl(null); setLastAnalysisKey(null);
    if (runtime) {
      const sourceChanged = initializedSourceRef.current !== source;
      initializedSourceRef.current = source;
      const initialize = async () => {
        // Tear down the previous provider only after guarded navigation has
        // committed. Destroying it in switchSource leaves this page with a
        // dead WebView when the user chooses to keep unsaved work.
        if (sourceChanged) await destroyEmbeddedBrowser().catch(() => undefined);
        const value = await (source === "pixiv"
          ? store.get<string>("pixiv_refresh_token")
          : store.get<string>("fanbox_session_id"));
        if (cancelled) return;
        setAuthConnected(Boolean(value));
        await positionBrowser(home);
      };
      initialize().catch((error) => {
        if (cancelled) return;
        setAuthConnected(false);
        notifications.show({ color: "red", title: "内蔵ブラウザを開けません", message: errorMessage(error) });
      });
    } else {
      setAuthConnected(false);
    }
    return () => { cancelled = true; };
  }, [positionBrowser, runtime, searchParams, source]);
  useEffect(() => {
    if (!runtime) return;
    const element = browserViewportRef.current;
    if (!element) return;
    // Bounds updates are coalesced to one per frame; a drag-resize otherwise
    // fires an IPC call for every intermediate pixel.
    let frame = 0;
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(syncBrowserBounds); };
    const observer = new ResizeObserver(schedule);
    observer.observe(element);
    // The whole save layout matters, not just the viewport box: collapsing the
    // candidate pane or dragging the splitter moves the browser pane too.
    if (layoutRef.current) observer.observe(layoutRef.current);
    window.addEventListener("resize", schedule);
    // Backstop for anything the observers miss; a no-op unless the rectangle
    // actually moved.
    const reconcile = window.setInterval(syncBrowserBounds, 700);
    // The address field is only refreshed while the user is not editing it,
    // otherwise a background URL refresh wipes what they are typing.
    const applyUrl = (url: string) => {
      setCurrentUrl(url);
      rememberVisit(url);
      if (!addressFocusedRef.current) setAddress(url);
    };
    const dispose = subscribeTauriEvent<string>("url-changed", (event) => applyUrl(event.payload));
    const disposeAccelerator = subscribeTauriEvent<BrowserAcceleratorEvent>("browser-accelerator", (event) => acceleratorHandlerRef.current(event.payload));
    // Tracking the large window's page live is what lets the candidate sidebar
    // work against it: "候補を取得" reads the same currentUrl either way.
    const disposeStandaloneUrl = subscribeTauriEvent<StandaloneBrowserUrlEvent>("standalone-browser-url-changed", (event) => {
      if (event.payload.source !== source) return;
      applyUrl(event.payload.url);
    });
    const disposeStandaloneClosed = subscribeTauriEvent<StandaloneBrowserClosedEvent>("standalone-browser-closed", (event) => {
      if (event.payload.source !== source) return;
      reattachRef.current();
    });
    // In-page navigation cannot reach us as an event: this WebView loads a
    // remote origin, so Tauri's IPC is deliberately not injected into it.
    // While the large window has the session, that window is the one to poll.
    const poll = window.setInterval(() => {
      const read = detachedRef.current ? getStandaloneBrowserUrl(source) : getEmbeddedBrowserUrl();
      read.then((url) => { if (url) applyUrl(url); }).catch(() => undefined);
    }, 2500);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      window.clearInterval(poll);
      window.clearInterval(reconcile);
      appliedBoundsRef.current = "";
      dispose();
      disposeAccelerator();
      disposeStandaloneUrl();
      disposeStandaloneClosed();
      closeEmbeddedBrowser().catch(() => undefined);
    };
  }, [rememberVisit, runtime, source, syncBrowserBounds]);
  useEffect(() => {
    const guard = (event: BeforeUnloadEvent) => { if (saving) { event.preventDefault(); event.returnValue = ""; } };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [saving]);
  // Closing the desktop window mid-download would abandon the batch silently.
  // 移動のほうは止めない - この画面を離れても保存は走り続け、進行状況も中止も
  // アクティビティに残る。閉じるときだけは本当に消えるので、そこでは訊く。
  useEffect(() => {
    const unregister = registerUnsavedGuard(() => savingRef.current, ["close"]);
    return () => {
      unregister();
      // 黙って居なくなると、止まったのか続いているのか分からない。
      if (savingRef.current) {
        notifications.show({ color: "piep", title: "保存はこのまま続きます", message: "進行状況と中止は、左下のアクティビティから操作できます" });
      }
    };
  }, []);

  const selectedCount = items.filter((item) => item.selected).length;
  // 保存ボタンが名乗る件数は、実際に取りに行く件数と同じでなければならない。
  // 済んだものは対象から外れるので、失敗が混じったあとは選択数と食い違う。
  const isPendingSave = (item: SidebarItem) => item.selected && item.status !== "success" && item.status !== "skipped";
  const pendingCount = items.filter(isPendingSave).length;
  const retryCount = items.filter((item) => isPendingSave(item) && item.status === "failed").length;
  const targetKind = describeDownloadTarget(currentUrl).kind;
  // 古いのは一覧のほう。一覧が無いうちは、古くなりようがない。
  const analysisStale = items.length > 0 && Boolean(lastAnalysisKey) && lastAnalysisKey !== downloadTargetKey(currentUrl);
  const navigateBrowser = async (url: string) => {
    const normalized = /^https?:\/\//i.test(url) ? url : `https://${url}`;
    setCurrentUrl(normalized); setAddress(normalized);
    if (!runtime) return;
    // The address bar always drives whichever browser currently holds the
    // session, so it keeps working after the handover.
    if (detachedRef.current) {
      const userAgent = source === "fanbox" ? await store.get<string>("fanbox_user_agent") || undefined : undefined;
      await openStandaloneBrowser(normalized, { source, userAgent }).catch((error) =>
        notifications.show({ color: "red", title: "大きいウィンドウを操作できません", message: errorMessage(error) }));
      return;
    }
    try { await navigateEmbeddedBrowser(normalized); } catch { await positionBrowser(normalized); }
  };
  const switchSource = async (next: string) => {
    if (next === source) return;
    navigate(`/save/${next}`);
  };
  const openSourceInWindow = async () => {
    try {
      if (!runtime) {
        window.open(currentUrl, "_blank", "noopener,noreferrer");
        return;
      }
      const userAgent = source === "fanbox" ? await store.get<string>("fanbox_user_agent") || undefined : undefined;
      const reused = await openStandaloneBrowser(currentUrl, { source, userAgent });
      // Hidden rather than destroyed: the session, the cookies and the page it
      // was on all survive, so coming back is instant.
      setDetached(true);
      await setEmbeddedBrowserVisible(false).catch(() => undefined);
      notifications.show({
        color: "piep",
        title: reused ? "大きいウィンドウへ切り替えました" : "大きいウィンドウで開きました",
        message: "右の保存候補はそのまま使えます。ウィンドウを閉じるとアプリ内に戻ります。",
      });
    } catch (error) {
      notifications.show({ color: "red", title: "大きいウィンドウを開けません", message: errorMessage(error) });
    }
  };

  /** Brings the page the large window ended on back into the in-app pane. */
  const reattachBrowser = useCallback(async (url: string) => {
    // The close event and the watchdog below can both fire for one closure.
    if (!detachedRef.current) return;
    detachedRef.current = false;
    setDetached(false);
    if (!runtime) return;
    appliedBoundsRef.current = "";
    try {
      await navigateEmbeddedBrowser(url);
      await setEmbeddedBrowserVisible(true);
      syncBrowserBounds();
    } catch {
      // The child view may have been torn down while detached; recreate it at
      // the page the large window was showing.
      await positionBrowser(url).catch(() => undefined);
    }
  }, [positionBrowser, runtime, syncBrowserBounds]);

  const returnToApp = async () => {
    const url = currentUrl;
    await closeStandaloneBrowser(source).catch(() => undefined);
    await reattachBrowser(url);
  };
  // The listener is installed once, so it reaches the current reattach through
  // a ref rather than re-subscribing on every URL change.
  const reattachRef = useRef<() => void>(() => undefined);
  reattachRef.current = () => { void reattachBrowser(currentUrlRef.current); };

  // The close event is the fast path, but the pane must come back even if it
  // never arrives - a window manager can tear a window down without one, and a
  // browser pane stuck behind a placeholder is unusable. Asking the backend
  // whether the window still exists needs no event at all.
  useEffect(() => {
    if (!runtime || !detached) return;
    const watchdog = window.setInterval(() => {
      getStandaloneBrowserUrl(source)
        .then((url) => {
          if (url) return;
          reattachRef.current();
        })
        .catch(() => undefined);
    }, 700);
    return () => window.clearInterval(watchdog);
  }, [detached, runtime, source]);

  // On remount the large window may still be open; pick the handover back up
  // instead of showing an in-app pane that nothing is driving.
  useEffect(() => {
    if (!runtime) return;
    let cancelled = false;
    getStandaloneBrowserUrl(source)
      .then((url) => {
        if (cancelled || !url) return;
        setDetached(true);
        setCurrentUrl(url);
        setAddress(url);
        rememberVisit(url);
        void setEmbeddedBrowserVisible(false).catch(() => undefined);
      })
      .catch(() => undefined);
    return () => { cancelled = true; };
  }, [rememberVisit, runtime, source]);
  const pasteUrl = async () => {
    try {
      const value = (await navigator.clipboard.readText()).trim();
      if (!value) return;
      setAddress(value);
      await navigateBrowser(value);
    } catch (error) {
      notifications.show({ color: "red", title: "クリップボードを読めません", message: errorMessage(error) });
    }
  };
  const startCandidateResize = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const startWidth = candidateWidth;
    // Cap against the pane's own row so the candidate column can never be
    // pushed past the edge of the clipped, unscrollable save page.
    const available = layoutRef.current?.clientWidth ?? window.innerWidth;
    const maxWidth = Math.max(260, Math.round(available * 0.55));
    const move = (moveEvent: PointerEvent) => setCandidateWidth(Math.round(Math.max(260, Math.min(maxWidth, startWidth + startX - moveEvent.clientX))));
    const stop = () => {
      handle.releasePointerCapture?.(event.pointerId);
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", stop);
      handle.removeEventListener("pointercancel", stop);
      document.body.style.removeProperty("user-select");
    };
    document.body.style.userSelect = "none";
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", stop, { once: true });
    handle.addEventListener("pointercancel", stop, { once: true });
  };
  const resizeCandidateWithKeyboard = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const available = layoutRef.current?.clientWidth ?? window.innerWidth;
    const maxWidth = Math.max(260, Math.round(available * 0.55));
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = candidateWidth + 24;
    if (event.key === "ArrowRight") next = candidateWidth - 24;
    if (event.key === "Home") next = 260;
    if (event.key === "End") next = maxWidth;
    if (next === null) return;
    event.preventDefault();
    setCandidateWidth(Math.round(Math.max(260, Math.min(maxWidth, next))));
  };
  const analyze = async (analysisUrl = currentUrl) => {
    if (!runtime) return notifications.show({ color: "piep", message: "候補取得はデスクトップアプリで利用できます" });
    if (!authConnected) return notifications.show({ color: "yellow", title: `${getProvider(source).label}に接続してください`, message: "設定画面からログインすると候補を取得できます" });
    const { kind: target, id: targetId } = describeDownloadTarget(analysisUrl);
    if (target === "unsupported") return notifications.show({ color: "yellow", title: "対応ページではありません", message: "作品、シリーズ、作者・クリエイターページを開いてください" });
    // 同じ取得が二重に走れば、同じ知らせも二度出る。
    if (analyzingRef.current) return;
    analyzingRef.current = true;
    setAnalyzing(true); setItems([]); setProgress(null);
    try {
      const pixivToken = await store.get<string>("pixiv_refresh_token") || "";
      const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
      const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
      let next: SidebarItem[] = [];
      if (target === "pixiv_single") {
        const data = await fetchPixivNovelByUrl<PixivNovel>(analysisUrl, pixivToken);
        const novelId = String(data.id ?? data.detail?.id ?? "");
        next = [{ id: novelId, title: data.title || data.detail?.title || "Pixiv novel", subtitle: data.user?.name || data.detail?.user?.name, selected: true, originalData: data }];
      } else if (target === "pixiv_series") {
        next = (await fetchPixivSeriesNovels<PixivNovel[]>(targetId, pixivToken)).map((novel) => ({ id: String(novel.id), title: novel.title, subtitle: novel.user?.name, selected: true, originalData: novel }));
      } else if (target === "pixiv_user") {
        next = (await fetchPixivUserNovels<PixivNovel[]>(targetId, pixivToken)).map((novel) => ({ id: String(novel.id), title: novel.title, subtitle: novel.user?.name, selected: true, originalData: novel }));
      } else if (target === "fanbox_single") {
        const data = normalizeFanboxPostPayload<FanboxPost>(await fetchFanboxPost<unknown>(targetId, fanboxCookie, fanboxUserAgent));
        next = [{ id: targetId, title: data.title || "FANBOX投稿", subtitle: data.user?.name, selected: true, originalData: data }];
      } else if (target === "fanbox_creator") {
        next = (await fetchFanboxCreatorPosts<FanboxPost[]>(targetId, fanboxCookie, fanboxUserAgent)).map((post) => ({ id: String(post.id), title: post.title, subtitle: post.user?.name, selected: true, originalData: post }));
      }
      setItems(next); setDownloadType(target); setLastAnalysisUrl(analysisUrl); setLastAnalysisKey(downloadTargetKey(analysisUrl));
      notifications.show({ color: "green", title: `${next.length}件の候補を取得しました`, message: "保存する項目を確認してください" });
    } catch (error) {
      notifications.show({ color: "red", title: "候補を取得できません", message: errorMessage(error) });
    } finally { analyzingRef.current = false; setAnalyzing(false); }
  };
  acceleratorHandlerRef.current = (payload) => {
    // A detached browser is provider-scoped. The embedded view belongs to the
    // currently mounted Save page even when it temporarily visits a neutral
    // domain, so do not discard its close/save shortcut based on URL inference.
    if (payload.browser === "standalone" && payload.source !== source) return;
    if (payload.action === "close") {
      if (payload.browser === "embedded") navigate("/library");
      return;
    }
    if (payload.action === "save") {
      void (async () => {
        setCurrentUrl(payload.url);
        setAddress(payload.url);
        rememberVisit(payload.url);
        // The hidden in-app view is no longer dragged along: the candidate
        // sidebar reads the same URL either browser reports, so pulling the
        // background view to that page only cost a second page load.
        await analyze(payload.url);
      })();
    }
  };
  const execute = async () => {
    // 済んだものは対象から外す。再試行は「残りをもう一度」であって、
    // 保存できたものを取り直しに行くことではない - 監視中の作品は
    // 保存済みでも取り直す作りなので、素通しにすると本当に再取得される。
    const selected = items.filter((item) => item.selected && item.status !== "success" && item.status !== "skipped");
    if (!selected.length || !downloadType || !runtime || savingRef.current) return;
    // 取得元は続けざまに叩けば断ってくる。1件ずつ間を空け、断られたら
    // 引き下がって同じ項目をやり直す。ここに間隔が無かったころは、89件
    // 選ぶと途中から全部が取得制限で落ちていた。
    //
    // 中止より先に作るのは、中止がこの待ちを断ち切る相手だから。取得制限の
    // あとの待ちは30秒近くまで伸び、そこで止まっているあいだ中止を握り潰すと
    // 「押しても何も起きない」ことになる。
    const pacer = createSourcePacer();
    const operation = startOperation({
      kind: "save",
      label: `${selected.length}件をライブラリに保存`,
      // 一覧の出どころは、いま開いているページではなく取ってきたページ。
      detail: `${getProvider(source).label} · ${lastAnalysisUrl || currentUrl}`,
      total: selected.length,
      // 待ちをその場で終わらせ、押されたことを画面にも即座に出す。実際に
      // 止まるのは次の切れ目だが、返事はここで返す。
      onCancel: () => { pacer.abort(); setCanceling(true); },
      // 再試行はいまの画面に対して走らせる。開始時の execute を捕まえたままだと、
      // その描画の items を見て、成功済みも含む古い一覧を回し直してしまう。
      onRetry: () => executeRef.current(),
    });
    saveOperationRef.current = operation;
    // state の反映を待たずに閉める。二連打や再試行が同じ瞬間に入ると、
    // setSaving の反映前にもう一周始まってしまう。
    savingRef.current = true;
    setSaving(true);
    setCanceling(false);
    let saved = 0, skipped = 0, failed = 0;
    let firstError = "";
    const savedEntries: UpdateDownloadEntry[] = [];
    try {
      const pixivToken = await store.get<string>("pixiv_refresh_token") || "";
      const fanboxCookie = await store.get<string>("fanbox_session_id") || "";
      const fanboxUserAgent = await store.get<string>("fanbox_user_agent") || "Mozilla/5.0";
      for (let index = 0; index < selected.length; index += 1) {
        if (operation.isCancelRequested()) break;
        const item = selected[index];
        setProgress({ current: index + 1, total: selected.length, text: item.title });
        operation.progress(index, selected.length, `「${item.title}」を処理しています`);
        setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "downloading" } : row));
        let attempt = 0;
        for (;;) {
          try {
            const itemSource = downloadType.startsWith("pixiv") ? "pixiv" : "fanbox";
            const existing = await getDownloadBySource<DownloadEntry>(itemSource, item.id);
            if (existing && !existing.watchUpdates) { skipped += 1; operation.log(`「${item.title}」は保存済みのためスキップしました`, "info"); setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "skipped" } : row)); break; }
            if (itemSource === "pixiv") {
              const data = await fetchPixivNovel<PixivNovel>(item.id, pixivToken);
              const metadata = normalizePixivSaveMetadata(data, item.title, item.subtitle);
              savedEntries.push(await downloadAndSave<UpdateDownloadEntry>({ data, source: "pixiv", sourceId: item.id, ...metadata, cookie: null, userAgent: null }));
            } else {
              const data = normalizeFanboxPostPayload<FanboxPost>(await fetchFanboxPost<unknown>(item.id, fanboxCookie, fanboxUserAgent));
              const metadata = normalizeFanboxSaveMetadata(data, item.title, item.subtitle);
              savedEntries.push(await downloadAndSave<UpdateDownloadEntry>({ data, source: "fanbox", sourceId: item.id, ...metadata, cookie: fanboxCookie, userAgent: fanboxUserAgent }));
            }
            saved += 1; operation.progress(index + 1, selected.length, `「${item.title}」を保存しました`); setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "success" } : row));
            // 通ったぶんだけ元の速さへ戻す。制限が解けたあとも遅いままにしない。
            pacer.relax();
            break;
          } catch (error) {
            // 「今は無理」と言われただけなら、間を空けて同じ項目をやり直す。
            if (isRateLimited(error) && attempt < MAX_RATE_LIMIT_RETRIES && !operation.isCancelRequested()) {
              attempt += 1;
              const waited = Math.round(pacer.currentDelayMs() * 2 / 1000);
              operation.log(`「${item.title}」: 取得が制限されています。${waited}秒あけてやり直します`, "warn");
              setProgress({ current: index + 1, total: selected.length, text: `取得制限のため待機中… ${item.title}` });
              await pacer.backOff();
              // 待っているあいだに中止が入っていたら、やり直しには行かない。
              // この項目はまだ失敗ではないので、失敗としても数えない。
              if (operation.isCancelRequested()) {
                operation.log(`「${item.title}」の待機を中止しました`, "warn");
                setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "pending" } : row));
                break;
              }
              continue;
            }
            const message = errorMessage(error);
            if (!firstError) firstError = message;
            operation.log(`「${item.title}」: ${message}`, "error");
            failed += 1; setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, status: "failed", error: message } : row));
            break;
          }
        }
        // 最後の1件のあとまで待つ必要はない。
        if (index < selected.length - 1 && !operation.isCancelRequested()) await pacer.wait();
      }
      // 仕上げの作者・シリーズ取得も、相手は同じ取得元。中止のあとにここで
      // 何十回も叩けば、止めたはずのものが止まって見えない。作品はもう
      // 保存できていて、作者名も入っている - 足りないのは横顔だけなので、
      // 次の保存や更新確認のときに埋まる。
      if (savedEntries.length > 0 && operation.isCancelRequested()) {
        operation.log("中止したため、作者・シリーズ情報の取得は見送りました", "warn");
      }
      if (savedEntries.length > 0 && !operation.isCancelRequested()) {
        setProgress({ current: selected.length, total: selected.length, text: "作者・シリーズ情報を取得しています" });
        await refreshEntityProfilesForEntries(savedEntries, { refreshToken: pixivToken, fanboxCookie, fanboxUserAgent });
        // 「保存したものは追いかける」設定のときだけ、保存直後に監視へ載せる。
        // 失敗しても保存は済んでいるので、ここで処理を止めない。
        const schedule = await loadSchedule();
        if (schedule.watchSaved) {
          await Promise.allSettled(savedEntries.map((entry) => setWatchUpdates(entry.id, true)));
        }
      }
      queryClient.invalidateQueries({ queryKey: ["library"] }); queryClient.invalidateQueries({ queryKey: ["dashboard"] }); queryClient.invalidateQueries({ queryKey: ["library-facets"] });
      queryClient.invalidateQueries({ queryKey: ["entity"] }); queryClient.invalidateQueries({ queryKey: ["entity-works"] });
      if (operation.isCancelRequested()) {
        operation.cancel(`保存 ${saved} · スキップ ${skipped} · 失敗 ${failed} の時点で中止しました`);
      } else if (failed === selected.length) {
        operation.fail(new Error(firstError || "すべての作品を保存できませんでした"));
      } else {
        operation.complete(`保存 ${saved} · スキップ ${skipped} · 失敗 ${failed}`);
      }
      notifications.show({ color: operation.isCancelRequested() ? "gray" : failed ? "yellow" : "green", title: operation.isCancelRequested() ? "保存を中止しました" : "保存が完了しました", message: `保存 ${saved} · スキップ ${skipped} · 失敗 ${failed}` });
      if (firstError) notifications.show({ color: "red", title: "保存できなかった項目があります", message: firstError });
    } catch (error) {
      operation.fail(error);
      notifications.show({ color: "red", title: "保存処理を完了できません", message: errorMessage(error) });
    } finally { saveOperationRef.current = null; savingRef.current = false; setSaving(false); setCanceling(false); setProgress(null); }
  };
  // 最新の execute を再試行へ届けるための一段。render ごとに差し替わる。
  executeRef.current = execute;

  return (
    <div className="save-page">
      <div className="save-page__header">
        <Group justify="space-between" h="100%" px="md" wrap="nowrap">
          <Group gap="md" wrap="nowrap"><Title order={1} fz="h2">Webから保存</Title><SegmentedControl aria-label="保存元サービス" value={source} onChange={switchSource} data={[{ value: "pixiv", label: <Group gap={6} wrap="nowrap"><ProviderMark provider="pixiv" compact /><Text size="xs" fw={700}>pixiv</Text></Group> }, { value: "fanbox", label: <Group gap={6} wrap="nowrap"><ProviderMark provider="fanbox" compact /><Text size="xs" fw={700}>FANBOX</Text></Group> }]} /></Group>
          <Group gap="xs"><Badge color={authConnected ? "green" : "gray"} variant="light" leftSection={<span className="status-dot" />}>{authConnected ? "接続済み" : "未接続"}</Badge><Button variant="subtle" size="xs" onClick={() => navigate("/settings")}>接続設定</Button></Group>
        </Group>
      </div>
      <div
        ref={layoutRef}
        className="save-layout"
        data-candidate-collapsed={candidateCollapsed || undefined}
        style={{ "--piep-candidate-width": `${candidateWidth}px` } as React.CSSProperties}
      >
        <section className="browser-pane">
          <div className="browser-toolbar">
            <Group gap={4} wrap="nowrap">
              <Tooltip label="戻る"><ActionIcon variant="subtle" color="gray" aria-label="ブラウザで戻る" disabled={!runtime || detached} onClick={() => goBackEmbeddedBrowser()}><Icons.back size={IconSize.action} /></ActionIcon></Tooltip>
              <Tooltip label="進む"><ActionIcon variant="subtle" color="gray" aria-label="ブラウザで進む" disabled={!runtime || detached} onClick={() => goForwardEmbeddedBrowser()}><Icons.forward size={IconSize.action} /></ActionIcon></Tooltip>
              <Tooltip label="再読み込み"><ActionIcon variant="subtle" color="gray" aria-label="ページを再読み込み" disabled={!runtime || detached} onClick={() => reloadEmbeddedBrowser()}><Icons.retry size={IconSize.action} /></ActionIcon></Tooltip>
              <Tooltip label="ホーム"><ActionIcon variant="subtle" color="gray" aria-label="サービスのホームを開く" onClick={() => navigateBrowser(getProvider(source).homeUrl!)}><Icons.home size={IconSize.action} /></ActionIcon></Tooltip>
              {history.length > 1 && (
                <Menu position="bottom-start" width={420} withinPortal>
                  <Menu.Target><Tooltip label="このセッションで開いたページ"><ActionIcon variant="subtle" color="gray" aria-label="このセッションで開いたページ"><Icons.versionHistory size={IconSize.action} /></ActionIcon></Tooltip></Menu.Target>
                  <Menu.Dropdown>
                    <Menu.Label>このセッションで開いたページ</Menu.Label>
                    {history.map((url) => <Menu.Item key={url} onClick={() => navigateBrowser(url)}><Text size="xs" className="line-clamp-1">{url}</Text></Menu.Item>)}
                  </Menu.Dropdown>
                </Menu>
              )}
              <form onSubmit={(event) => { event.preventDefault(); navigateBrowser(address); }} style={{ flex: 1 }}>
                <TextInput value={address} onChange={(event) => setAddress(event.currentTarget.value)} onFocus={() => { addressFocusedRef.current = true; }} onBlur={() => { addressFocusedRef.current = false; }} leftSection={<Icons.secureConnection size={IconSize.inline} />} rightSection={<Group gap={0} wrap="nowrap"><Tooltip label="クリップボードのURLを開く"><ActionIcon type="button" variant="subtle" size="sm" aria-label="クリップボードのURLを開く" onClick={pasteUrl}><Icons.paste size={IconSize.menu} /></ActionIcon></Tooltip><ActionIcon type="submit" variant="subtle" size="sm" aria-label="URLを開く"><Icons.forward size={IconSize.menu} /></ActionIcon></Group>} rightSectionWidth={58} size="sm" aria-label="ブラウザのアドレス" />
              </form>
              {detached
                ? <Tooltip label="アプリ内の表示に戻す"><ActionIcon variant="light" color="piep" aria-label="アプリ内の表示に戻す" onClick={returnToApp}><Icons.minimize size={IconSize.action} /></ActionIcon></Tooltip>
                : <Tooltip label="大きいウィンドウで開く（Ctrl+Sで候補取得）"><ActionIcon variant="subtle" color="gray" aria-label="大きいウィンドウで開く" onClick={openSourceInWindow}><Icons.maximize size={IconSize.action} /></ActionIcon></Tooltip>}
            </Group>
          </div>
          <div ref={browserViewportRef} className="browser-viewport" data-detached={detached || undefined}>
            {runtime && detached && (
              <Stack align="center" justify="center" h="100%" p="xl" ta="center" gap="sm">
                <ThemeIcon size={64} radius="xl" variant="light"><Icons.externalLink size={IconSize.hero} /></ThemeIcon>
                <Title order={3} fz="h4">大きいウィンドウで表示中</Title>
                <Text size="sm" c="dimmed" maw={460}>
                  {getProvider(source).label}は別ウィンドウで開いています。右の保存候補はそのまま使えます。ウィンドウを閉じると、このページがここに戻ります。
                </Text>
                <Text size="xs" c="dimmed" maw={460} className="line-clamp-2">{currentUrl}</Text>
                <Group mt="xs">
                  <Button variant="light" leftSection={<Icons.minimize size={IconSize.menu} />} onClick={returnToApp}>アプリ内に戻す</Button>
                  <Button variant="default" leftSection={<Icons.externalLink size={IconSize.menu} />} onClick={openSourceInWindow}>ウィンドウを前面に</Button>
                </Group>
              </Stack>
            )}
            {!runtime && <Stack align="center" justify="center" h="100%" p="xl" ta="center"><ThemeIcon size={72} radius="xl" variant="light"><Icons.browser size={IconSize.hero} /></ThemeIcon><Title order={3}>内蔵ブラウザ</Title><Text size="sm" c="dimmed" maw={430}>Tauriアプリでは、この領域に{getProvider(source).label}が表示されます。ページを移動して右側の「候補を取得」を押します。</Text><Button component="a" href={getProvider(source).homeUrl!} target="_blank" rel="noopener noreferrer" variant="light" rightSection={<Icons.externalLink size={IconSize.menu} />}>公式サイトを開く</Button></Stack>}
          </div>
        </section>

        <div className="save-resizer" role="separator" tabIndex={0} aria-orientation="vertical" aria-label="保存候補パネルの幅を変更" aria-valuemin={260} aria-valuenow={candidateWidth} aria-valuetext={`${candidateWidth}px。左右の矢印キーで変更`} onPointerDown={startCandidateResize} onKeyDown={resizeCandidateWithKeyboard}><Icons.drag size={IconSize.menu} /></div>

        <aside className="candidate-pane" data-collapsed={candidateCollapsed || undefined}>
          {candidateCollapsed ? <Tooltip label="保存候補を開く" position="left"><ActionIcon variant="subtle" color="gray" size="lg" mt="sm" mx="auto" aria-label="保存候補を開く" onClick={() => setCandidateCollapsed(false)}><Icons.panelOpen size={IconSize.nav} /></ActionIcon></Tooltip> :
          <Stack h="100%" gap={0}>
            <Box p="md">
              <Group justify="space-between" mb="xs"><Box><Text fw={750}>保存候補</Text><Text size="xs" c="dimmed">開いているページから自動判定</Text></Box><Group gap={4}><ThemeIcon variant="light"><Icons.select size={IconSize.action} /></ThemeIcon><Tooltip label="保存候補をたたむ"><ActionIcon variant="subtle" color="gray" aria-label="保存候補をたたむ" onClick={() => setCandidateCollapsed(true)}><Icons.panelClose size={IconSize.action} /></ActionIcon></Tooltip></Group></Group>
              <Alert color={targetKind === "unsupported" ? "gray" : "piep"} variant="light" icon={targetKind === "unsupported" ? <Icons.link size={IconSize.action} /> : <Icons.confirm size={IconSize.action} />} py="xs">
                <Text size="xs">{targetLabel(targetKind)}</Text>
              </Alert>
              {/* 帯を割り込ませない。取り直しを頼む相手は、取り直すボタンである。
                  独立した警告として一段挟むと、押す場所と読む場所が離れるうえ、
                  ページを移るたびにパネル全体が上下に飛ぶ。 */}
              <Button
                fullWidth
                mt="sm"
                color={analysisStale ? "yellow" : undefined}
                leftSection={analyzing ? undefined : analysisStale ? <Icons.retry size={IconSize.menu} /> : <Icons.search size={IconSize.menu} />}
                loading={analyzing}
                disabled={!runtime || saving || targetKind === "unsupported"}
                onClick={() => analyze()}
              >
                {analysisStale ? "候補を再取得" : "候補を取得"}
              </Button>
            </Box>
            <Divider />
            <Group justify="space-between" px="md" py="sm" gap="xs" wrap="nowrap">
              <Group gap={6} miw={0} wrap="nowrap">
                <Text size="xs" c="dimmed">{items.length ? `${selectedCount} / ${items.length}件を選択` : "候補はまだありません"}</Text>
                {/* 古いのは一覧そのものなので、印は一覧の見出しに付く。読み上げ
                    にも出るよう status にしてある - 見えている人には色が、
                    見えていない人には言葉が要る。 */}
                {analysisStale && items.length > 0 && (
                  <Tooltip label="取得したときのページから移動しています" multiline w={220}>
                    <Badge color="yellow" variant="light" size="sm" role="status" style={{ flexShrink: 0 }}>古い</Badge>
                  </Tooltip>
                )}
              </Group>
              {items.length > 0 && <Group gap={6} wrap="nowrap"><Button variant="subtle" size="compact-xs" onClick={() => setItems((rows) => rows.map((item) => ({ ...item, selected: true })))}>すべて</Button><Button variant="subtle" color="gray" size="compact-xs" onClick={() => setItems((rows) => rows.map((item) => ({ ...item, selected: false })))}>解除</Button></Group>}
            </Group>
            <ScrollArea flex={1} px="md" type="auto" className="candidate-list" data-stale={analysisStale && items.length > 0 || undefined}>
              <Stack gap="xs" pb="md">
                {!items.length && !analyzing && <EmptyCandidates source={source} />}
                {items.map((item) => <CandidateRow key={item.id} item={item} disabled={saving} onToggle={() => setItems((rows) => rows.map((row) => row.id === item.id ? { ...row, selected: !row.selected } : row))} />)}
              </Stack>
            </ScrollArea>
            <Divider />
            <Box p="md">
              {progress && <Stack gap={5} mb="sm" role="status" aria-live="polite"><Group justify="space-between"><Text size="xs" className="line-clamp-1">{canceling ? `中止しています… ${progress.text}` : progress.text}</Text><Text size="xs" c="dimmed">{progress.current}/{progress.total}</Text></Group><Progress value={progress.current / progress.total * 100} animated={!canceling} color={canceling ? "gray" : undefined} aria-label={`保存進捗 ${progress.current}/${progress.total}`} /></Stack>}
              {saving
                ? <Group grow>
                    <Button size="md" leftSection={<Icons.collect size={IconSize.action} />} loading>{canceling ? "中止待ち" : "保存中"}</Button>
                    {/* 押したことが返らないボタンは、押せなかったのと同じ。
                        実際に止まるのは次の切れ目でも、返事はその場で返す。 */}
                    <Button size="md" variant="light" color="red" leftSection={<Icons.cancel size={IconSize.action} />} loading={canceling} disabled={canceling} onClick={() => saveOperationRef.current && requestOperationCancel(saveOperationRef.current.id)}>{canceling ? "中止しています" : "中止"}</Button>
                  </Group>
                : <Button fullWidth size="md" leftSection={<Icons.collect size={IconSize.action} />} disabled={!runtime || !pendingCount} onClick={execute}>{!pendingCount ? (selectedCount ? "選択したものは保存済みです" : "保存する項目を選択") : retryCount === pendingCount ? `失敗した${pendingCount}件をやり直す` : `${pendingCount}件をライブラリに保存`}</Button>}
              <Button fullWidth mt="xs" variant="subtle" color="gray" leftSection={<Icons.library size={IconSize.menu} />} onClick={() => navigate("/library")}>ライブラリを開く</Button>
            </Box>
          </Stack>}
        </aside>
      </div>
    </div>
  );
}

function targetLabel(kind: DownloadTargetKind): string {
  const labels: Record<string, string> = { pixiv_single: "pixiv小説を保存できます", pixiv_series: "シリーズ内の小説を選択できます", pixiv_user: "作者の小説を選択できます", fanbox_single: "FANBOX投稿を保存できます", fanbox_creator: "クリエイターの投稿を選択できます", unsupported: "作品・シリーズ・作者ページを開いてください" };
  return labels[kind];
}

function CandidateRow({ item, disabled, onToggle }: { item: SidebarItem; disabled: boolean; onToggle: () => void }) {
  const status = item.status;
  return (
    <Card p="sm" className="candidate-row" data-selected={item.selected || undefined}>
      <Group wrap="nowrap" align="flex-start">
        <Checkbox checked={item.selected} disabled={disabled || status === "success" || status === "skipped"} onChange={onToggle} aria-label={`${item.title}を保存対象にする`} mt={3} />
        <Stack gap={3} flex={1} miw={0}><Text size="sm" fw={650} className="line-clamp-2">{item.title}</Text>{item.subtitle && <Text size="xs" c="dimmed" className="line-clamp-1">{item.subtitle}</Text>}{item.error && <Text size="xs" c="red" className="line-clamp-3">{item.error}</Text>}<Text size="10px" c="dimmed">ID {item.id}</Text></Stack>
        <StatusIcon status={status} />
      </Group>
    </Card>
  );
}

function StatusIcon({ status }: { status: SidebarItem["status"] }) {
  if (status === "downloading") return <Loader size="xs" />;
  if (status === "success") return <ThemeIcon color="green" size="sm" radius="xl"><Icons.confirm size={IconSize.inline} /></ThemeIcon>;
  if (status === "skipped") return <Badge color="gray" size="xs">保存済み</Badge>;
  if (status === "failed") return <ThemeIcon color="red" size="sm" radius="xl"><Icons.cancel size={IconSize.inline} /></ThemeIcon>;
  return null;
}

function EmptyCandidates({ source }: { source: SaveSource }) {
  return <Stack align="center" ta="center" py={48} px="md"><ThemeIcon size={48} radius="xl" variant="light" color="gray"><Icons.link size={IconSize.feature} /></ThemeIcon><Text size="sm" fw={700}>ページを開いて候補を取得</Text><Text size="xs" c="dimmed">{source === "pixiv" ? "小説、シリーズ、作者ページ" : "投稿またはクリエイターページ"}に対応しています。</Text></Stack>;
}
