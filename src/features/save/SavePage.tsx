import { memo, useCallback, useEffect, useRef, useState } from "react";
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
import {
  useAppNavigate,
  useAppSearchParams,
  useRouteParams,
} from "@/app/router";
import {
  describeDownloadTarget,
  downloadTargetKey,
  type DownloadTargetKind,
  type FanboxPost,
  type PixivNovel,
  type SidebarDownloadType,
  type SidebarItem,
} from "@/features/browser/downloadCandidates";
import { normalizeFanboxPostPayload } from "@/features/browser/downloadMetadata";
import { errorMessage } from "@/lib/format";
import { getProvider, ProviderMark } from "@/lib/providers";
import { registerUnsavedGuard } from "@/lib/unsavedGuard";
import { useEmbeddedBrowserOverlay } from "@/features/browser/useEmbeddedBrowserOverlay";
import {
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
import { isTauriRuntime } from "@/services/dbApi";
import { loadSchedule } from "@/features/updates/updateSchedule";
import { invalidateWorkSetViews } from "@/features/library/workSetInvalidation";
import {
  fetchFanboxCreatorPosts,
  fetchFanboxPost,
  fetchPixivNovelByUrl,
  fetchPixivSeriesNovels,
  fetchPixivUserNovels,
} from "@/services/downloadApi";
import { subscribeTauriEvent } from "@/services/eventBus";
import { store } from "@/store";
import {
  requestOperationCancel,
  startOperation,
  type OperationController,
} from "@/features/jobs/operationJobs";
import { waitForUpdateJob } from "@/features/updates/updateJobs";
import {
  cancelUpdateJobCommand,
  listUpdateJobItemStatesCommand,
  startSaveJobCommand,
  type UpdateJobItemState,
  type UpdateJobSnapshot,
} from "@/services/updateJobApi";

type SaveSource = "pixiv" | "fanbox";

const SAVE_JOB_ROW_STATUS: Record<string, SidebarItem["status"]> = {
  queued: "pending",
  running: "downloading",
  saved: "success",
  done: "success",
  skipped: "skipped",
  failed: "failed",
};

const SAVE_JOB_STATUS_RANK: Partial<
  Record<Exclude<SidebarItem["status"], undefined>, number>
> = {
  pending: 0,
  downloading: 1,
  success: 2,
  skipped: 2,
  failed: 2,
};

export default function SavePage() {
  const { source: routeSource } = useRouteParams("/save/:source?");
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [searchParams] = useAppSearchParams();
  const runtime = isTauriRuntime();
  const source: SaveSource = routeSource === "fanbox" ? "fanbox" : "pixiv";
  const initialUrl =
    searchParams.get("url") ||
    getProvider(source).homeUrl ||
    "https://www.pixiv.net/";
  const [currentUrl, setCurrentUrl] = useState(initialUrl);
  const [address, setAddress] = useState(initialUrl);
  const [items, setItems] = useState<SidebarItem[]>([]);
  const [downloadType, setDownloadType] = useState<SidebarDownloadType | null>(
    null,
  );
  const [analyzing, setAnalyzing] = useState(false);
  const [saving, setSaving] = useState(false);
  // 中止は押した瞬間に見えなければならない。取り消しが実際に効くのは次の
  // 待ちの切れ目なので、その間ボタンが何も言わないと押せていないと読まれる。
  const [canceling, setCanceling] = useState(false);
  const [progress, setProgress] = useState<{
    current: number;
    total: number;
    text: string;
  } | null>(null);
  const [lastAnalysisUrl, setLastAnalysisUrl] = useState<string | null>(null);
  // 一覧がどのページのものかは、URLではなく「相手」で覚える。
  const [lastAnalysisKey, setLastAnalysisKey] = useState<string | null>(null);
  const [authConnected, setAuthConnected] = useState<boolean | null>(null);
  const [candidateWidth, setCandidateWidth] = useLocalStorage({
    key: "piep.save-candidate-width",
    defaultValue: 360,
  });
  const [candidateCollapsed, setCandidateCollapsed] = useState(false);
  // While the large window is open the in-app pane hands its job over to it:
  // two live sessions on the same provider would fight over the login state,
  // and the native child WebView would still paint over the app.
  const [detached, setDetached] = useState(false);
  const [history, setHistory] = useState<string[]>([]);
  const browserViewportRef = useRef<HTMLDivElement>(null);
  const layoutRef = useRef<HTMLDivElement>(null);
  const addressFocusedRef = useRef(false);
  const acceleratorHandlerRef = useRef<
    (payload: BrowserAcceleratorEvent) => void
  >(() => undefined);
  const saveOperationRef = useRef<OperationController | null>(null);
  const saveJobIdRef = useRef<string | null>(null);
  // 保存中かどうかは、描画の外からも即座に読めなければならない - 終了ガードと
  // 再入の防止が、どちらも state の反映を待てないため。
  const savingRef = useRef(false);
  // 終了ガードの解除。**保存が終わったときに呼ぶ**もので、画面を離れたときでは
  // ない。保存は画面より長生きするので、寿命は保存に合わせる。
  const closeGuardRef = useRef<(() => void) | null>(null);
  // 取得も同じ。Ctrl+S と取得ボタンが同じ瞬間に入ると、state の反映を待たずに
  // もう一周始まり、同じ知らせが二度出る。
  const analyzingRef = useRef(false);
  const executeRef = useRef<() => void>(() => undefined);

  useEmbeddedBrowserOverlay(browserViewportRef, runtime && !detached);

  // 一行ごとの押し先。**毎回作り直さない。**
  //
  // ここで新しい関数を作ると、行に渡す props が毎回変わってメモ化が素通しに
  // なる。保存中は1件ごとに `items` を組み直すので、素通しのままだと全行が
  // そのたびに描き直される。
  const toggleCandidate = useCallback((id: string) => {
    setItems((rows) =>
      rows.map((row) =>
        row.id === id ? { ...row, selected: !row.selected } : row,
      ),
    );
  }, []);

  // A page is only worth remembering once, and only the most recent handful are
  // worth offering back.
  const rememberVisit = useCallback((url: string) => {
    if (!url || url === "about:blank") return;
    setHistory((current) => {
      // 2.5秒ごとの見回りがここを通る。同じ場所に居るあいだも毎回新しい配列を
      // 返していたので、URL が1文字も動いていないのに画面が作り直されていた。
      if (current[0] === url) return current;
      return [url, ...current.filter((item) => item !== url)].slice(0, 24);
    });
  }, []);

  const readBounds = useCallback(() => {
    const element = browserViewportRef.current;
    if (!runtime || !element) return null;
    const rect = element.getBoundingClientRect();
    if (rect.width < 100 || rect.height < 100) return null;
    return {
      x: Math.round(rect.left),
      y: Math.round(rect.top),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    };
  }, [runtime]);

  const positionBrowser = useCallback(
    async (url: string) => {
      const userAgent =
        source === "fanbox"
          ? (await store.get<string>("fanbox_user_agent")) || undefined
          : undefined;
      // Measured after the await: the layout can settle while the store is read.
      const bounds = readBounds();
      if (!bounds) return;
      await openEmbeddedBrowser(url, { ...bounds, userAgent });
      // Let the reconciler re-apply, since creation may land after another pass.
      appliedBoundsRef.current = "";
    },
    [readBounds, source],
  );

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
  // WebView creation is not cancellable once it has crossed IPC. Serialize
  // provider initializations so a slow, stale open can never finish after the
  // current provider and take the pane back to the wrong site.
  const browserInitQueueRef = useRef<Promise<void>>(Promise.resolve());
  const detachedRef = useRef(detached);
  detachedRef.current = detached;
  // Which provider currently owns the large browser. This is separate from
  // the route source during a provider handover, when the route has changed
  // but the replacement large window is still being created.
  const detachedSourceRef = useRef<SaveSource | null>(null);
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
      .then((applied) => {
        if (!applied) appliedBoundsRef.current = "";
      })
      .catch(() => {
        appliedBoundsRef.current = "";
      });
  }, [readBounds]);

  useEffect(() => {
    let cancelled = false;
    const home =
      searchParams.get("url") ||
      getProvider(source).homeUrl ||
      "https://www.pixiv.net/";
    setCurrentUrl(home);
    setAddress(home);
    setItems([]);
    setDownloadType(null);
    setLastAnalysisUrl(null);
    setLastAnalysisKey(null);
    if (runtime) {
      const sourceChanged = initializedSourceRef.current !== source;
      initializedSourceRef.current = source;
      const initialize = async () => {
        if (cancelled) return;
        // Tear down the previous provider only after guarded navigation has
        // committed. Destroying it in switchSource leaves this page with a
        // dead WebView when the user chooses to keep unsaved work.
        if (sourceChanged)
          await destroyEmbeddedBrowser().catch(() => undefined);
        if (cancelled) return;
        const value = await (source === "pixiv"
          ? store.get<string>("pixiv_refresh_token")
          : store.get<string>("fanbox_session_id"));
        if (cancelled) return;
        setAuthConnected(Boolean(value));
        // **大きいウィンドウは、この画面より長生きする。**
        // 切り離したまま別の画面へ行くと、片付けで閉じるのは埋め込みだけで、
        // 大きいウィンドウはそのまま残る。切り離していたという記憶は
        // 画面と一緒に消えるので、戻ってきたときに埋め込みも開き、**同じ
        // サイトが二つの窓に出ていた**。残っているなら、その状態を引き継ぐ。
        const otherSource: SaveSource = source === "pixiv" ? "fanbox" : "pixiv";
        const [standing, otherStanding] = await Promise.all([
          getStandaloneBrowserUrl(source).catch(() => null),
          getStandaloneBrowserUrl(otherSource).catch(() => null),
        ]);
        if (cancelled) return;
        if (standing) {
          detachedRef.current = true;
          detachedSourceRef.current = source;
          setDetached(true);
          setCurrentUrl(standing);
          setAddress(standing);
          rememberVisit(standing);
          // A detached page must have a single renderer. This also clears an
          // embedded view left behind by an interrupted previous mount.
          await destroyEmbeddedBrowser().catch(() => undefined);
          // Older builds allowed one large window per provider to remain on
          // screen. Collapse that stale state back to the single workspace.
          if (otherStanding)
            await closeStandaloneBrowser(otherSource).catch(() => undefined);
          return;
        }
        const previousDetachedSource =
          detachedSourceRef.current && detachedSourceRef.current !== source
            ? detachedSourceRef.current
            : otherStanding
              ? otherSource
              : null;
        if (previousDetachedSource) {
          const userAgent =
            source === "fanbox"
              ? (await store.get<string>("fanbox_user_agent")) || undefined
              : undefined;
          await openStandaloneBrowser(home, { source, userAgent });
          // Opening a native WebView cannot be cancelled. If another switch
          // overtook this one, remove the stale window before its successor.
          if (cancelled) {
            await closeStandaloneBrowser(source).catch(() => undefined);
            return;
          }
          detachedRef.current = true;
          detachedSourceRef.current = source;
          setDetached(true);
          setCurrentUrl(home);
          setAddress(home);
          rememberVisit(home);
          await destroyEmbeddedBrowser().catch(() => undefined);
          await closeStandaloneBrowser(previousDetachedSource).catch(
            () => undefined,
          );
          return;
        }
        detachedRef.current = false;
        detachedSourceRef.current = null;
        setDetached(false);
        await positionBrowser(home);
        // `openEmbeddedBrowser` itself cannot be cancelled. If this task became
        // stale while IPC was in flight, tear down its late result before the
        // serialized current-provider task proceeds.
        if (cancelled) await destroyEmbeddedBrowser().catch(() => undefined);
      };
      const queued = browserInitQueueRef.current.then(initialize);
      browserInitQueueRef.current = queued.catch(() => undefined);
      queued.catch((error) => {
        if (cancelled) return;
        setAuthConnected(false);
        notifications.show({
          color: "red",
          title: "内蔵ブラウザを開けません",
          message: errorMessage(error),
        });
      });
    } else {
      setAuthConnected(false);
    }
    return () => {
      cancelled = true;
    };
  }, [positionBrowser, rememberVisit, runtime, searchParams, source]);
  useEffect(() => {
    if (!runtime) return;
    const element = browserViewportRef.current;
    if (!element) return;
    // Bounds updates are coalesced to one per frame; a drag-resize otherwise
    // fires an IPC call for every intermediate pixel.
    let frame = 0;
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(syncBrowserBounds);
    };
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
    const dispose = subscribeTauriEvent<string>("url-changed", (event) =>
      applyUrl(event.payload),
    );
    const disposeAccelerator = subscribeTauriEvent<BrowserAcceleratorEvent>(
      "browser-accelerator",
      (event) => acceleratorHandlerRef.current(event.payload),
    );
    // Tracking the large window's page live is what lets the candidate sidebar
    // work against it: "候補を取得" reads the same currentUrl either way.
    const disposeStandaloneUrl = subscribeTauriEvent<StandaloneBrowserUrlEvent>(
      "standalone-browser-url-changed",
      (event) => {
        if (event.payload.source !== source) return;
        applyUrl(event.payload.url);
      },
    );
    const disposeStandaloneClosed =
      subscribeTauriEvent<StandaloneBrowserClosedEvent>(
        "standalone-browser-closed",
        (event) => {
          if (event.payload.source !== source) return;
          if (detachedSourceRef.current !== event.payload.source) return;
          reattachRef.current();
        },
      );
    // In-page navigation cannot reach us as an event: this WebView loads a
    // remote origin, so Tauri's IPC is deliberately not injected into it.
    // While the large window has the session, that window is the one to poll.
    const poll = window.setInterval(() => {
      const read = detachedRef.current
        ? getStandaloneBrowserUrl(source)
        : getEmbeddedBrowserUrl();
      read
        .then((url) => {
          if (url) applyUrl(url);
        })
        .catch(() => undefined);
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
      // A hidden WebView keeps its renderer, decoded images and GPU surfaces.
      // Leaving the workspace is a durable boundary; recreate it on return.
      destroyEmbeddedBrowser().catch(() => undefined);
    };
  }, [rememberVisit, runtime, source, syncBrowserBounds]);
  useEffect(() => {
    const guard = (event: BeforeUnloadEvent) => {
      if (saving) {
        event.preventDefault();
        event.returnValue = "";
      }
    };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [saving]);
  // Closing the desktop window mid-download would abandon the batch silently.
  // 移動のほうは止めない - この画面を離れても保存は走り続け、進行状況も中止も
  // アクティビティに残る。閉じるときだけは本当に消えるので、そこでは訊く。
  // ガードはここでは外さない。以前はこの後片付けで解除していたが、保存は画面を
  // 離れても走り続ける設計なので、**守るべきものが残っているのに守りだけが消え**
  // ていた。保存中にライブラリへ移ってウィンドウを閉じると、何も訊かれずに終了し、
  // 走っていた保存が消える。登録と解除は execute の開始と finally に置いてある。
  useEffect(
    () => () => {
      // 黙って居なくなると、止まったのか続いているのか分からない。
      if (savingRef.current) {
        notifications.show({
          color: "piep",
          title: "保存はこのまま続きます",
          message: "進行状況と中止は、左下のアクティビティから操作できます",
        });
      }
    },
    [],
  );

  const selectedCount = items.filter((item) => item.selected).length;
  // 保存ボタンが名乗る件数は、実際に取りに行く件数と同じでなければならない。
  // 済んだものは対象から外れるので、失敗が混じったあとは選択数と食い違う。
  const isPendingSave = (item: SidebarItem) =>
    item.selected && item.status !== "success" && item.status !== "skipped";
  const pendingCount = items.filter(isPendingSave).length;
  const retryCount = items.filter(
    (item) => isPendingSave(item) && item.status === "failed",
  ).length;
  const targetKind = describeDownloadTarget(currentUrl).kind;
  // 古いのは一覧のほう。一覧が無いうちは、古くなりようがない。
  const analysisStale =
    items.length > 0 &&
    Boolean(lastAnalysisKey) &&
    lastAnalysisKey !== downloadTargetKey(currentUrl);
  const navigateBrowser = async (url: string) => {
    const normalized = /^https?:\/\//i.test(url) ? url : `https://${url}`;
    setCurrentUrl(normalized);
    setAddress(normalized);
    if (!runtime) return;
    // The address bar always drives whichever browser currently holds the
    // session, so it keeps working after the handover.
    if (detachedRef.current) {
      const userAgent =
        source === "fanbox"
          ? (await store.get<string>("fanbox_user_agent")) || undefined
          : undefined;
      await openStandaloneBrowser(normalized, { source, userAgent }).catch(
        (error) =>
          notifications.show({
            color: "red",
            title: "大きいウィンドウを操作できません",
            message: errorMessage(error),
          }),
      );
      return;
    }
    try {
      await navigateEmbeddedBrowser(normalized);
    } catch {
      await positionBrowser(normalized);
    }
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
      const userAgent =
        source === "fanbox"
          ? (await store.get<string>("fanbox_user_agent")) || undefined
          : undefined;
      const reused = await openStandaloneBrowser(currentUrl, {
        source,
        userAgent,
      });
      // The standalone window owns the live page now. Keeping an additional
      // hidden renderer doubles the expensive part of the browser workspace.
      detachedRef.current = true;
      detachedSourceRef.current = source;
      setDetached(true);
      await destroyEmbeddedBrowser().catch(() => undefined);
      notifications.show({
        color: "piep",
        title: reused
          ? "大きいウィンドウへ切り替えました"
          : "大きいウィンドウで開きました",
        message:
          "右の保存候補はそのまま使えます。ウィンドウを閉じるとアプリ内に戻ります。",
      });
    } catch (error) {
      notifications.show({
        color: "red",
        title: "大きいウィンドウを開けません",
        message: errorMessage(error),
      });
    }
  };

  /** Brings the page the large window ended on back into the in-app pane. */
  const reattachBrowser = useCallback(
    async (url: string) => {
      // The close event and the watchdog below can both fire for one closure.
      if (!detachedRef.current) return;
      detachedRef.current = false;
      detachedSourceRef.current = null;
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
    },
    [positionBrowser, runtime, syncBrowserBounds],
  );

  const returnToApp = async () => {
    const url = currentUrl;
    await closeStandaloneBrowser(source).catch(() => undefined);
    await reattachBrowser(url);
  };
  // The listener is installed once, so it reaches the current reattach through
  // a ref rather than re-subscribing on every URL change.
  const reattachRef = useRef<() => void>(() => undefined);
  reattachRef.current = () => {
    void reattachBrowser(currentUrlRef.current);
  };

  // The close event is the fast path, but the pane must come back even if it
  // never arrives - a window manager can tear a window down without one, and a
  // browser pane stuck behind a placeholder is unusable. Asking the backend
  // whether the window still exists needs no event at all.
  useEffect(() => {
    if (!runtime || !detached) return;
    const watchdog = window.setInterval(() => {
      // During a provider handover the old window is intentionally closing and
      // the new route is already mounted. It must not resurrect a child view.
      if (detachedSourceRef.current !== source) return;
      getStandaloneBrowserUrl(source)
        .then((url) => {
          if (url) return;
          reattachRef.current();
        })
        .catch(() => undefined);
    }, 700);
    return () => window.clearInterval(watchdog);
  }, [detached, runtime, source]);

  // 開いたままの大きいウィンドウを引き継ぐ判断は、**埋め込みを作る前**の
  // 初期化で行う（この上の `useEffect`）。ここにも同じ判断を置いていたころ、
  // 作る側（`openEmbeddedBrowser` は作ると同時に表示する）と隠す側が競争し、
  // 順番が入れ替わると**同じサイトが二つの窓に出た**。判断はひとつの場所で。
  const pasteUrl = async () => {
    try {
      const value = (await navigator.clipboard.readText()).trim();
      if (!value) return;
      setAddress(value);
      await navigateBrowser(value);
    } catch (error) {
      notifications.show({
        color: "red",
        title: "クリップボードを読めません",
        message: errorMessage(error),
      });
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
    const move = (moveEvent: PointerEvent) =>
      setCandidateWidth(
        Math.round(
          Math.max(
            260,
            Math.min(maxWidth, startWidth + startX - moveEvent.clientX),
          ),
        ),
      );
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
  const resizeCandidateWithKeyboard = (
    event: React.KeyboardEvent<HTMLDivElement>,
  ) => {
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
    if (!runtime)
      return notifications.show({
        color: "piep",
        message: "候補取得はデスクトップアプリで利用できます",
      });
    if (!authConnected)
      return notifications.show({
        color: "yellow",
        title: `${getProvider(source).label}に接続してください`,
        message: "設定画面からログインすると候補を取得できます",
      });
    const { kind: target, id: targetId } = describeDownloadTarget(analysisUrl);
    if (target === "unsupported")
      return notifications.show({
        color: "yellow",
        title: "対応ページではありません",
        message: "作品、シリーズ、作者・クリエイターページを開いてください",
      });
    // 同じ取得が二重に走れば、同じ知らせも二度出る。
    if (analyzingRef.current) return;
    analyzingRef.current = true;
    setAnalyzing(true);
    setItems([]);
    setProgress(null);
    try {
      const pixivToken = (await store.get<string>("pixiv_refresh_token")) || "";
      const fanboxCookie = (await store.get<string>("fanbox_session_id")) || "";
      const fanboxUserAgent =
        (await store.get<string>("fanbox_user_agent")) || "Mozilla/5.0";
      let next: SidebarItem[] = [];
      if (target === "pixiv_single") {
        const data = await fetchPixivNovelByUrl<PixivNovel>(
          analysisUrl,
          pixivToken,
        );
        const novelId = String(data.id ?? data.detail?.id ?? "");
        next = [
          {
            id: novelId,
            title: data.title || data.detail?.title || "Pixiv novel",
            subtitle: data.user?.name || data.detail?.user?.name,
            selected: true,
            originalData: data,
          },
        ];
      } else if (target === "pixiv_series") {
        next = (
          await fetchPixivSeriesNovels<PixivNovel[]>(targetId, pixivToken)
        ).map((novel) => ({
          id: String(novel.id),
          title: novel.title,
          subtitle: novel.user?.name,
          selected: true,
          originalData: novel,
        }));
      } else if (target === "pixiv_user") {
        next = (
          await fetchPixivUserNovels<PixivNovel[]>(targetId, pixivToken)
        ).map((novel) => ({
          id: String(novel.id),
          title: novel.title,
          subtitle: novel.user?.name,
          selected: true,
          originalData: novel,
        }));
      } else if (target === "fanbox_single") {
        const data = normalizeFanboxPostPayload<FanboxPost>(
          await fetchFanboxPost<unknown>(
            targetId,
            fanboxCookie,
            fanboxUserAgent,
          ),
        );
        next = [
          {
            id: targetId,
            title: data.title || "FANBOX投稿",
            subtitle: data.user?.name,
            selected: true,
            originalData: data,
          },
        ];
      } else if (target === "fanbox_creator") {
        next = (
          await fetchFanboxCreatorPosts<FanboxPost[]>(
            targetId,
            fanboxCookie,
            fanboxUserAgent,
          )
        ).map((post) => ({
          id: String(post.id),
          title: post.title,
          subtitle: post.user?.name,
          selected: true,
          originalData: post,
        }));
      }
      setItems(next);
      setDownloadType(target);
      setLastAnalysisUrl(analysisUrl);
      setLastAnalysisKey(downloadTargetKey(analysisUrl));
      notifications.show({
        color: "green",
        title: `${next.length}件の候補を取得しました`,
        message: "保存する項目を確認してください",
      });
    } catch (error) {
      notifications.show({
        color: "red",
        title: "候補を取得できません",
        message: errorMessage(error),
      });
    } finally {
      analyzingRef.current = false;
      setAnalyzing(false);
    }
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
  // ジョブの様子を、行と進捗に写し取る。**保存はこの関数の中では起きない** -
  // 走っているのは Rust の中で、ここはそれを覗いているだけ。だからこの画面を
  // 離れても、描画側が落ちても、保存は最後まで進む。
  const followSaveJob = async (
    jobId: string,
    itemSource: "pixiv" | "fanbox",
    operation: OperationController,
    total: number,
  ) => {
    let forwardedLogId = 0;
    let lastStatesAt = 0;
    let statesPromise: Promise<void> | null = null;
    const applyItemStates = async (force = false) => {
      // A progress event arrives for every item. Item-state lookup is a full
      // IPC query, so never let those events start overlapping queries.
      if (statesPromise) {
        if (!force) return;
        await statesPromise;
      }
      const now = Date.now();
      if (!force && now - lastStatesAt < 200) return;
      lastStatesAt = now;
      const request = (async () => {
        const states = await listUpdateJobItemStatesCommand(jobId).catch(
          () => null,
        );
        if (!states) return;
        const byId = new Map(
          states
            .filter((state) => state.source === itemSource && state.sourceId)
            .map((state) => [state.sourceId as string, state]),
        );
        setItems((rows) =>
          rows.map((row) => {
            const state = byId.get(row.id);
            if (!state) return row;
            const status = SAVE_JOB_ROW_STATUS[state.status];
            if (!status) return row;
            // 状態取得は複数回が重なりうる。古い応答があとから戻っても、保存済みの
            // 行を「処理中」へ巻き戻さない。
            const currentRank = row.status
              ? (SAVE_JOB_STATUS_RANK[row.status] ?? 0)
              : 0;
            if (currentRank > (SAVE_JOB_STATUS_RANK[status] ?? 0)) return row;
            if (row.status === status && (row.error ?? null) === state.error)
              return row;
            return { ...row, status, error: state.error ?? undefined };
          }),
        );
      })();
      statesPromise = request;
      try {
        await request;
      } finally {
        if (statesPromise === request) statesPromise = null;
      }
    };
    const applyItemState = (state: UpdateJobItemState) => {
      if (state.source !== itemSource || !state.sourceId) return;
      const status = SAVE_JOB_ROW_STATUS[state.status];
      if (!status) return;
      setItems((rows) =>
        rows.map((row) => {
          if (row.id !== state.sourceId) return row;
          const currentRank = row.status
            ? (SAVE_JOB_STATUS_RANK[row.status] ?? 0)
            : 0;
          if (currentRank > (SAVE_JOB_STATUS_RANK[status] ?? 0)) return row;
          if (row.status === status && (row.error ?? null) === state.error)
            return row;
          return { ...row, status, error: state.error ?? undefined };
        }),
      );
    };
    let lastProgressAt = 0;
    let lastProgressDone = -1;
    const applySnapshot = (snapshot: UpdateJobSnapshot) => {
      const done = Math.min(snapshot.processed, total);
      const now = Date.now();
      const terminal =
        snapshot.status === "completed" ||
        snapshot.status === "failed" ||
        snapshot.status === "canceled";
      // Keep the progress indicator responsive without forcing a React render
      // and an operation-history write for every worker event.
      if (!terminal && done === lastProgressDone && now - lastProgressAt < 250)
        return;
      // At most four visual/history updates per second, with a count-based
      // fallback so a slow worker still reports meaningful progress. This
      // keeps both React rendering and localStorage writes bounded for fast
      // 200–800 item saves.
      if (
        !terminal &&
        done - lastProgressDone < 10 &&
        now - lastProgressAt < 250
      )
        return;
      lastProgressAt = now;
      lastProgressDone = done;
      setProgress({
        current: done,
        total,
        text: snapshot.activeLabel || `${done}/${total} 件を処理しました`,
      });
      operation.progress(
        done,
        total,
        snapshot.activeLabel || `${done}/${total} 件を処理しました`,
      );
      // ジョブのログはそのまま作業の記録へ流す。同じ行を二度出さないよう、
      // どこまで送ったかを覚えておく。
      const fresh = snapshot.logs
        .filter((log) => log.id > forwardedLogId)
        .sort((a, b) => a.id - b.id);
      for (const log of fresh) {
        operation.log(
          log.message,
          log.logType === "success" ? "info" : log.logType,
        );
        forwardedLogId = log.id;
      }
    };

    // One full read establishes the state if the worker progressed before the
    // delta listener was registered. Live events update one row at a time.
    await applyItemStates(true);
    const final = await waitForUpdateJob(
      jobId,
      (snapshot) => {
        applySnapshot(snapshot);
      },
      applyItemState,
    );
    // 最後の1件ぶんは、終わりの様子が届いたあとで書き戻す。
    await applyItemStates(true);
    return final;
  };

  const execute = async () => {
    // 済んだものは対象から外す。再試行は「残りをもう一度」であって、
    // 保存できたものを取り直しに行くことではない - 監視中の作品は
    // 保存済みでも取り直す作りなので、素通しにすると本当に再取得される。
    const selected = items.filter(
      (item) =>
        item.selected && item.status !== "success" && item.status !== "skipped",
    );
    if (!selected.length || !downloadType || !runtime || savingRef.current)
      return;
    const itemSource: "pixiv" | "fanbox" = downloadType.startsWith("pixiv")
      ? "pixiv"
      : "fanbox";
    const operation = startOperation({
      kind: "save",
      label: `${selected.length}件をライブラリに保存`,
      // 一覧の出どころは、いま開いているページではなく取ってきたページ。
      detail: `${getProvider(source).label} · ${lastAnalysisUrl || currentUrl}`,
      total: selected.length,
      // 中止はジョブへ伝える。押されたことは画面にも即座に出す - 実際に
      // 止まるのは次の切れ目だが、返事はここで返す。
      onCancel: async () => {
        setCanceling(true);
        const jobId = saveJobIdRef.current;
        if (!jobId) return;
        try {
          await cancelUpdateJobCommand(jobId);
        } catch (error) {
          // 握りつぶしていた。中止が届かなかったのに「中止しています」の
          // ままボタンが沈み、保存はそのまま最後まで走って成功の報せが出る。
          // 押した人には、自分の中止がどこへ行ったのか分からない。
          setCanceling(false);
          throw error;
        }
      },
      // 再試行はいまの画面に対して走らせる。開始時の execute を捕まえたままだと、
      // その描画の items を見て、成功済みも含む古い一覧を回し直してしまう。
      onRetry: () => executeRef.current(),
    });
    saveOperationRef.current = operation;
    // state の反映を待たずに閉める。二連打や再試行が同じ瞬間に入ると、
    // setSaving の反映前にもう一周始まってしまう。
    savingRef.current = true;
    closeGuardRef.current?.();
    closeGuardRef.current = registerUnsavedGuard(
      () => savingRef.current,
      ["close"],
    );
    setSaving(true);
    setCanceling(false);
    setProgress({
      current: 0,
      total: selected.length,
      text: "保存の準備をしています",
    });
    try {
      const schedule = await loadSchedule();
      const snapshot = await startSaveJobCommand(
        selected.map((item) => ({
          source: itemSource,
          sourceId: item.id,
          title: item.title,
        })),
        schedule.watchSaved ?? false,
      );
      saveJobIdRef.current = snapshot.jobId;
      operation.linkExternalJob(snapshot.jobId);
      if (operation.isCancelRequested()) {
        await cancelUpdateJobCommand(snapshot.jobId).catch(() => undefined);
      }
      const final = await followSaveJob(
        snapshot.jobId,
        itemSource,
        operation,
        selected.length,
      );
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["entity"] });
      invalidateWorkSetViews(queryClient);
      const saved = final.savedCount;
      const failed = final.errorCount;
      const skipped = Math.max(0, final.processed - saved - failed);
      const tally = `保存 ${saved} · 保存済み ${skipped} · 失敗 ${failed}`;
      if (final.status === "canceled" || final.status === "canceling") {
        operation.cancel(`${tally} の時点で中止しました`);
        notifications.show({
          color: "gray",
          title: "保存を中止しました",
          message: tally,
        });
      } else if (
        final.status === "paused" ||
        final.status === "auth_required"
      ) {
        // 止まっただけで、失敗ではない。ジョブは残っているので続きから再開できる。
        operation.complete(
          `${tally}（${final.activeLabel || "中断しました"}）`,
        );
        notifications.show({
          color: "yellow",
          title: "保存を中断しました",
          message: `${tally} · 更新の画面から再開できます`,
        });
      } else if (saved === 0 && failed > 0) {
        operation.fail(
          new Error(final.activeLabel || "すべての作品を保存できませんでした"),
        );
        notifications.show({
          color: "red",
          title: "保存できませんでした",
          message: tally,
        });
      } else {
        operation.complete(tally);
        notifications.show({
          color: failed ? "yellow" : "green",
          title: "保存が完了しました",
          message: tally,
        });
      }
    } catch (error) {
      operation.fail(error);
      // 見失っただけで、保存そのものは保存側で走り続けている。ここで棚の
      // 取り置きを捨てないと、実際には増えているのに古い一覧を出したまま
      // 「失敗しました」と言うことになる。
      queryClient.invalidateQueries({ queryKey: ["library"] });
      queryClient.invalidateQueries({ queryKey: ["entity"] });
      invalidateWorkSetViews(queryClient);
      notifications.show({
        color: "red",
        title: "保存処理を完了できません",
        message: errorMessage(error),
      });
    } finally {
      saveJobIdRef.current = null;
      saveOperationRef.current = null;
      savingRef.current = false;
      closeGuardRef.current?.();
      closeGuardRef.current = null;
      setSaving(false);
      setCanceling(false);
      setProgress(null);
    }
  };
  // 最新の execute を再試行へ届けるための一段。render ごとに差し替わる。
  executeRef.current = execute;

  return (
    <div className="save-page">
      <div className="save-page__header">
        <Group justify="space-between" h="100%" px="md" wrap="nowrap">
          <Group gap="md" wrap="nowrap">
            <Title order={1} fz="h2">
              Webから保存
            </Title>
            <SegmentedControl
              aria-label="保存元サービス"
              value={source}
              onChange={switchSource}
              data={[
                {
                  value: "pixiv",
                  label: (
                    <Group gap={6} wrap="nowrap">
                      <ProviderMark provider="pixiv" compact />
                      <Text size="xs" fw={700}>
                        pixiv
                      </Text>
                    </Group>
                  ),
                },
                {
                  value: "fanbox",
                  label: (
                    <Group gap={6} wrap="nowrap">
                      <ProviderMark provider="fanbox" compact />
                      <Text size="xs" fw={700}>
                        FANBOX
                      </Text>
                    </Group>
                  ),
                },
              ]}
            />
          </Group>
          <Group gap="xs">
            <Badge
              color={authConnected ? "green" : "gray"}
              variant="light"
              leftSection={<span className="status-dot" />}
            >
              {authConnected ? "接続済み" : "未接続"}
            </Badge>
            <Button
              variant="subtle"
              size="xs"
              onClick={() => navigate("/settings")}
            >
              接続設定
            </Button>
          </Group>
        </Group>
      </div>
      <div
        ref={layoutRef}
        className="save-layout"
        data-candidate-collapsed={candidateCollapsed || undefined}
        style={
          {
            "--piep-candidate-width": `${candidateWidth}px`,
          } as React.CSSProperties
        }
      >
        <section className="browser-pane">
          <div className="browser-toolbar">
            <Group gap={4} wrap="nowrap">
              <Tooltip label="戻る">
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  aria-label="ブラウザで戻る"
                  disabled={!runtime || detached}
                  onClick={() => goBackEmbeddedBrowser()}
                >
                  <Icons.back size={IconSize.action} />
                </ActionIcon>
              </Tooltip>
              <Tooltip label="進む">
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  aria-label="ブラウザで進む"
                  disabled={!runtime || detached}
                  onClick={() => goForwardEmbeddedBrowser()}
                >
                  <Icons.forward size={IconSize.action} />
                </ActionIcon>
              </Tooltip>
              <Tooltip label="再読み込み">
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  aria-label="ページを再読み込み"
                  disabled={!runtime || detached}
                  onClick={() => reloadEmbeddedBrowser()}
                >
                  <Icons.retry size={IconSize.action} />
                </ActionIcon>
              </Tooltip>
              <Tooltip label="ホーム">
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  aria-label="サービスのホームを開く"
                  onClick={() => navigateBrowser(getProvider(source).homeUrl!)}
                >
                  <Icons.home size={IconSize.action} />
                </ActionIcon>
              </Tooltip>
              {history.length > 1 && (
                <Menu position="bottom-start" width={420} withinPortal>
                  <Menu.Target>
                    <Tooltip label="このセッションで開いたページ">
                      <ActionIcon
                        variant="subtle"
                        color="gray"
                        aria-label="このセッションで開いたページ"
                      >
                        <Icons.versionHistory size={IconSize.action} />
                      </ActionIcon>
                    </Tooltip>
                  </Menu.Target>
                  <Menu.Dropdown>
                    <Menu.Label>このセッションで開いたページ</Menu.Label>
                    {history.map((url) => (
                      <Menu.Item key={url} onClick={() => navigateBrowser(url)}>
                        <Text size="xs" className="line-clamp-1">
                          {url}
                        </Text>
                      </Menu.Item>
                    ))}
                  </Menu.Dropdown>
                </Menu>
              )}
              <form
                onSubmit={(event) => {
                  event.preventDefault();
                  navigateBrowser(address);
                }}
                style={{ flex: 1 }}
              >
                <TextInput
                  value={address}
                  onChange={(event) => setAddress(event.currentTarget.value)}
                  onFocus={() => {
                    addressFocusedRef.current = true;
                  }}
                  onBlur={() => {
                    addressFocusedRef.current = false;
                  }}
                  leftSection={
                    <Icons.secureConnection size={IconSize.inline} />
                  }
                  rightSection={
                    <Group gap={0} wrap="nowrap">
                      <Tooltip label="クリップボードのURLを開く">
                        <ActionIcon
                          type="button"
                          variant="subtle"
                          size="sm"
                          aria-label="クリップボードのURLを開く"
                          onClick={pasteUrl}
                        >
                          <Icons.paste size={IconSize.menu} />
                        </ActionIcon>
                      </Tooltip>
                      <ActionIcon
                        type="submit"
                        variant="subtle"
                        size="sm"
                        aria-label="URLを開く"
                      >
                        <Icons.forward size={IconSize.menu} />
                      </ActionIcon>
                    </Group>
                  }
                  rightSectionWidth={58}
                  size="sm"
                  aria-label="ブラウザのアドレス"
                />
              </form>
              {detached ? (
                <Tooltip label="アプリ内の表示に戻す">
                  <ActionIcon
                    variant="light"
                    color="piep"
                    aria-label="アプリ内の表示に戻す"
                    onClick={returnToApp}
                  >
                    <Icons.minimize size={IconSize.action} />
                  </ActionIcon>
                </Tooltip>
              ) : (
                <Tooltip label="大きいウィンドウで開く（Ctrl+Sで候補取得）">
                  <ActionIcon
                    variant="subtle"
                    color="gray"
                    aria-label="大きいウィンドウで開く"
                    onClick={openSourceInWindow}
                  >
                    <Icons.maximize size={IconSize.action} />
                  </ActionIcon>
                </Tooltip>
              )}
            </Group>
          </div>
          <div
            ref={browserViewportRef}
            className="browser-viewport"
            data-detached={detached || undefined}
          >
            {runtime && detached && (
              <Stack
                align="center"
                justify="center"
                h="100%"
                p="xl"
                ta="center"
                gap="sm"
              >
                <ThemeIcon size={64} radius="xl" variant="light">
                  <Icons.externalLink size={IconSize.hero} />
                </ThemeIcon>
                <Title order={3} fz="h4">
                  大きいウィンドウで表示中
                </Title>
                <Text size="sm" c="dimmed" maw={460}>
                  {getProvider(source).label}
                  は別ウィンドウで開いています。右の保存候補はそのまま使えます。ウィンドウを閉じると、このページがここに戻ります。
                </Text>
                <Text size="xs" c="dimmed" maw={460} className="line-clamp-2">
                  {currentUrl}
                </Text>
                <Group mt="xs">
                  <Button
                    variant="light"
                    leftSection={<Icons.minimize size={IconSize.menu} />}
                    onClick={returnToApp}
                  >
                    アプリ内に戻す
                  </Button>
                  <Button
                    variant="default"
                    leftSection={<Icons.externalLink size={IconSize.menu} />}
                    onClick={openSourceInWindow}
                  >
                    ウィンドウを前面に
                  </Button>
                </Group>
              </Stack>
            )}
            {!runtime && (
              <Stack
                align="center"
                justify="center"
                h="100%"
                p="xl"
                ta="center"
              >
                <ThemeIcon size={72} radius="xl" variant="light">
                  <Icons.browser size={IconSize.hero} />
                </ThemeIcon>
                <Title order={3}>内蔵ブラウザ</Title>
                <Text size="sm" c="dimmed" maw={430}>
                  Tauriアプリでは、この領域に{getProvider(source).label}
                  が表示されます。ページを移動して右側の「候補を取得」を押します。
                </Text>
                <Button
                  component="a"
                  href={getProvider(source).homeUrl!}
                  target="_blank"
                  rel="noopener noreferrer"
                  variant="light"
                  rightSection={<Icons.externalLink size={IconSize.menu} />}
                >
                  公式サイトを開く
                </Button>
              </Stack>
            )}
          </div>
        </section>

        <div
          className="save-resizer"
          role="separator"
          tabIndex={0}
          aria-orientation="vertical"
          aria-label="保存候補パネルの幅を変更"
          aria-valuemin={260}
          aria-valuenow={candidateWidth}
          aria-valuetext={`${candidateWidth}px。左右の矢印キーで変更`}
          onPointerDown={startCandidateResize}
          onKeyDown={resizeCandidateWithKeyboard}
        >
          <Icons.drag size={IconSize.menu} />
        </div>

        <aside
          className="candidate-pane"
          data-collapsed={candidateCollapsed || undefined}
        >
          {candidateCollapsed ? (
            <Tooltip label="保存候補を開く" position="left">
              <ActionIcon
                variant="subtle"
                color="gray"
                size="lg"
                mt="sm"
                mx="auto"
                aria-label="保存候補を開く"
                onClick={() => setCandidateCollapsed(false)}
              >
                <Icons.panelOpen size={IconSize.nav} />
              </ActionIcon>
            </Tooltip>
          ) : (
            <Stack h="100%" gap={0}>
              <Box p="md">
                <Group justify="space-between" mb="xs">
                  <Box>
                    <Text fw={750}>保存候補</Text>
                    <Text size="xs" c="dimmed">
                      開いているページから自動判定
                    </Text>
                  </Box>
                  <Group gap={4}>
                    <ThemeIcon variant="light">
                      <Icons.select size={IconSize.action} />
                    </ThemeIcon>
                    <Tooltip label="保存候補をたたむ">
                      <ActionIcon
                        variant="subtle"
                        color="gray"
                        aria-label="保存候補をたたむ"
                        onClick={() => setCandidateCollapsed(true)}
                      >
                        <Icons.panelClose size={IconSize.action} />
                      </ActionIcon>
                    </Tooltip>
                  </Group>
                </Group>
                <Alert
                  color={targetKind === "unsupported" ? "gray" : "piep"}
                  variant="light"
                  icon={
                    targetKind === "unsupported" ? (
                      <Icons.link size={IconSize.action} />
                    ) : (
                      <Icons.confirm size={IconSize.action} />
                    )
                  }
                  py="xs"
                >
                  <Text size="xs">{targetLabel(targetKind)}</Text>
                </Alert>
                {/* 帯を割り込ませない。取り直しを頼む相手は、取り直すボタンである。
                  独立した警告として一段挟むと、押す場所と読む場所が離れるうえ、
                  ページを移るたびにパネル全体が上下に飛ぶ。 */}
                <Button
                  fullWidth
                  mt="sm"
                  color={analysisStale ? "yellow" : undefined}
                  leftSection={
                    analyzing ? undefined : analysisStale ? (
                      <Icons.retry size={IconSize.menu} />
                    ) : (
                      <Icons.search size={IconSize.menu} />
                    )
                  }
                  loading={analyzing}
                  disabled={!runtime || saving || targetKind === "unsupported"}
                  onClick={() => analyze()}
                >
                  {analysisStale ? "候補を再取得" : "候補を取得"}
                </Button>
              </Box>
              <Divider />
              <Group
                justify="space-between"
                px="md"
                py="sm"
                gap="xs"
                wrap="nowrap"
              >
                <Group gap={6} miw={0} wrap="nowrap">
                  <Text size="xs" c="dimmed">
                    {items.length
                      ? `${selectedCount} / ${items.length}件を選択`
                      : "候補はまだありません"}
                  </Text>
                  {/* 古いのは一覧そのものなので、印は一覧の見出しに付く。読み上げ
                    にも出るよう status にしてある - 見えている人には色が、
                    見えていない人には言葉が要る。 */}
                  {analysisStale && items.length > 0 && (
                    <Tooltip
                      label="取得したときのページから移動しています"
                      multiline
                      w={220}
                    >
                      <Badge
                        color="yellow"
                        variant="light"
                        size="sm"
                        role="status"
                        style={{ flexShrink: 0 }}
                      >
                        古い
                      </Badge>
                    </Tooltip>
                  )}
                </Group>
                {items.length > 0 && (
                  <Group gap={6} wrap="nowrap">
                    <Button
                      variant="subtle"
                      size="compact-xs"
                      onClick={() =>
                        setItems((rows) =>
                          rows.map((item) => ({ ...item, selected: true })),
                        )
                      }
                    >
                      すべて
                    </Button>
                    <Button
                      variant="subtle"
                      color="gray"
                      size="compact-xs"
                      onClick={() =>
                        setItems((rows) =>
                          rows.map((item) => ({ ...item, selected: false })),
                        )
                      }
                    >
                      解除
                    </Button>
                  </Group>
                )}
              </Group>
              <ScrollArea
                flex={1}
                px="md"
                type="auto"
                className="candidate-list"
                data-stale={(analysisStale && items.length > 0) || undefined}
              >
                <Stack gap="xs" pb="md">
                  {!items.length && !analyzing && (
                    <EmptyCandidates source={source} />
                  )}
                  {items.map((item) => (
                    <CandidateRow
                      key={item.id}
                      item={item}
                      disabled={saving}
                      onToggle={toggleCandidate}
                    />
                  ))}
                </Stack>
              </ScrollArea>
              <Divider />
              <Box p="md">
                {progress && (
                  <Stack gap={5} mb="sm" role="status" aria-live="polite">
                    <Group justify="space-between" wrap="nowrap">
                      <Text size="xs" className="line-clamp-1" flex={1} miw={0}>
                        {canceling
                          ? `中止しています… ${progress.text}`
                          : progress.text}
                      </Text>
                      <Text size="xs" c="dimmed" style={{ flexShrink: 0 }}>
                        {progress.current}/{progress.total}
                      </Text>
                    </Group>
                    <Progress
                      value={(progress.current / progress.total) * 100}
                      animated={!canceling}
                      color={canceling ? "gray" : undefined}
                      aria-label={`保存進捗 ${progress.current}/${progress.total}`}
                    />
                  </Stack>
                )}
                {saving ? (
                  <Group grow>
                    <Button
                      size="md"
                      leftSection={<Icons.collect size={IconSize.action} />}
                      loading
                    >
                      {canceling ? "中止待ち" : "保存中"}
                    </Button>
                    {/* 押したことが返らないボタンは、押せなかったのと同じ。
                        実際に止まるのは次の切れ目でも、返事はその場で返す。 */}
                    <Button
                      size="md"
                      variant="light"
                      color="red"
                      leftSection={<Icons.cancel size={IconSize.action} />}
                      loading={canceling}
                      disabled={canceling}
                      onClick={() =>
                        saveOperationRef.current &&
                        requestOperationCancel(saveOperationRef.current.id)
                      }
                    >
                      {canceling ? "中止しています" : "中止"}
                    </Button>
                  </Group>
                ) : (
                  <Button
                    fullWidth
                    size="md"
                    leftSection={<Icons.collect size={IconSize.action} />}
                    disabled={!runtime || !pendingCount}
                    onClick={execute}
                  >
                    {!pendingCount
                      ? selectedCount
                        ? "選択したものは保存済みです"
                        : "保存する項目を選択"
                      : retryCount === pendingCount
                        ? `失敗した${pendingCount}件をやり直す`
                        : `${pendingCount}件をライブラリに保存`}
                  </Button>
                )}
                <Button
                  fullWidth
                  mt="xs"
                  variant="subtle"
                  color="gray"
                  leftSection={<Icons.library size={IconSize.menu} />}
                  onClick={() => navigate("/library")}
                >
                  ライブラリを開く
                </Button>
              </Box>
            </Stack>
          )}
        </aside>
      </div>
    </div>
  );
}

function targetLabel(kind: DownloadTargetKind): string {
  const labels: Record<string, string> = {
    pixiv_single: "pixiv小説を保存できます",
    pixiv_series: "シリーズ内の小説を選択できます",
    pixiv_user: "作者の小説を選択できます",
    fanbox_single: "FANBOX投稿を保存できます",
    fanbox_creator: "クリエイターの投稿を選択できます",
    unsupported: "作品・シリーズ・作者ページを開いてください",
  };
  return labels[kind];
}

/**
 * 一行。**メモ化しておく。**
 *
 * 保存は1件ごとに `items` を組み直すので、素のままだと**そのたびに全行が
 * 描き直される**。818件の保存なら 818行 x 2回 x 818件で百万回を超え、
 * その確保が積み上がって画面のプロセスがメモリ不足で落ちた（実際に落ちた）。
 * 変わるのは1行だけなので、他の行は前の結果を使い回す。
 *
 * そのためには押したときの関数も毎回作り直さないことが要る。作り直すと
 * props が毎回変わり、メモ化は素通しになる。
 */
const CandidateRow = memo(function CandidateRow({
  item,
  disabled,
  onToggle,
}: {
  item: SidebarItem;
  disabled: boolean;
  onToggle: (id: string) => void;
}) {
  const status = item.status;
  return (
    <Card
      p="sm"
      className="candidate-row"
      data-selected={item.selected || undefined}
    >
      <Group wrap="nowrap" align="flex-start">
        <Checkbox
          checked={item.selected}
          disabled={disabled || status === "success" || status === "skipped"}
          onChange={() => onToggle(item.id)}
          aria-label={`${item.title}を保存対象にする`}
          mt={3}
        />
        <Stack gap={3} flex={1} miw={0}>
          <Text size="sm" fw={650} className="line-clamp-2">
            {item.title}
          </Text>
          {item.subtitle && (
            <Text size="xs" c="dimmed" className="line-clamp-1">
              {item.subtitle}
            </Text>
          )}
          {item.error && (
            <Text size="xs" c="red" className="line-clamp-3">
              {item.error}
            </Text>
          )}
          <Text size="10px" c="dimmed">
            ID {item.id}
          </Text>
        </Stack>
        <StatusIcon status={status} />
      </Group>
    </Card>
  );
});

function StatusIcon({ status }: { status: SidebarItem["status"] }) {
  if (status === "downloading") return <Loader size="xs" />;
  if (status === "success")
    return (
      <ThemeIcon color="green" size="sm" radius="xl">
        <Icons.confirm size={IconSize.inline} />
      </ThemeIcon>
    );
  if (status === "skipped")
    return (
      <Badge color="gray" size="xs">
        保存済み
      </Badge>
    );
  if (status === "failed")
    return (
      <ThemeIcon color="red" size="sm" radius="xl">
        <Icons.cancel size={IconSize.inline} />
      </ThemeIcon>
    );
  return null;
}

function EmptyCandidates({ source }: { source: SaveSource }) {
  return (
    <Stack align="center" ta="center" py={48} px="md">
      <ThemeIcon size={48} radius="xl" variant="light" color="gray">
        <Icons.link size={IconSize.feature} />
      </ThemeIcon>
      <Text size="sm" fw={700}>
        ページを開いて候補を取得
      </Text>
      <Text size="xs" c="dimmed">
        {source === "pixiv"
          ? "小説、シリーズ、作者ページ"
          : "投稿またはクリエイターページ"}
        に対応しています。
      </Text>
    </Stack>
  );
}
