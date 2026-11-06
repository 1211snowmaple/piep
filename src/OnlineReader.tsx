import { rememberReading } from "./recentReading";
import { progressLabel } from "./workText";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ALargeSmall, ChevronLeft, TableOfContents, X } from "lucide-react";
import { Icons } from "./icons";
import { WorkFavoriteButton } from "./FavoriteButton";
import { ContinuousDocument, type ContinuousTarget, type LivePosition, type LoadedSection } from "./ContinuousDocument";
import { ReaderInfoSheet, type InfoTab } from "./ReaderInfoSheet";
import { ReaderPager } from "./ReaderPager";
import { ReaderBehaviorControls, ReaderTypeControls } from "./ReaderSettings";
import { fillDocumentImages, safePageHtml } from "./readerDocument";
import { readerKeyDirection } from "./readerKeyboard";
import { readRemotePosition, recordReading, refreshReading, refreshReadingWithin, remoteStart, sendReading, type ReadingWrite, type RemoteStart } from "./readingSync";
import { forgetWorkPages, fetchPage, prefetchPage } from "./pageCache";
import { illustrationUrl } from "./images";
import { getOfflineAsset, getOfflineOutline, getOfflinePage, getPcMetadata, getPcOutline, getPcWorkCollections, pcErrorMessage, saveOfflineWork, type PcCollection, type PcMetadata, type PcOutlineEntry, type PcPage, type PcWork } from "./pcApi";
import type { ReaderBehavior, ReaderFont, ReaderLayout, ReaderType } from "./readerPrefs";

function offlineSnapshotChanged(error: unknown): boolean {
  return typeof error === "object" && error !== null && "code" in error && error.code === "offline_snapshot_changed";
}

function readingKey(work: PcWork, editHash: string | null): string {
  return `piep.android.pc.${work.id}.${work.currentVersion}.${work.contentHash ?? ""}.${editHash ?? "original"}`;
}

function savedPage(work: PcWork, editHash: string | null): number {
  try {
    const raw = window.localStorage.getItem(`${readingKey(work, editHash)}.page`);
    if (raw === null) return 0;
    const value = Number(raw);
    return Number.isSafeInteger(value) && value >= 0 && value <= 1_000_000 ? value : 0;
  } catch { return 0; }
}

// サーバーの位置を待つのは開くときの一瞬だけ。遅ければ手元の位置で開き、あとで案内する。
// 同期を待つのは短く。間に合わなければ手元の位置で開き、本文を先に出す。
const SYNC_WAIT_MS = 600;
// 今のページを描き終えてから、次のページを受け取り始める。
const PREFETCH_DELAY_MS = 300;
const WRITE_DELAY_MS = 3000;

function localReadingTime(work: PcWork, editHash: string | null): number {
  try {
    const value = Number(window.localStorage.getItem(`${readingKey(work, editHash)}.at`));
    return Number.isSafeInteger(value) && value > 0 ? value : 0;
  } catch { return 0; }
}

function markLocalReading(key: string, time: number): void {
  try { window.localStorage.setItem(`${key}.at`, String(time)); } catch { /* storage unavailable */ }
}

async function remoteStartFor(work: PcWork, editHash: string | null): Promise<RemoteStart | null> {
  return remoteStart(await readRemotePosition(work.source, work.sourceId), localReadingTime(work, editHash), work.currentVersion, editHash);
}

// layout が違う位置は、割合だけを使って戻す。ページごとと一続きでは top の基準が違う。
type SavedPosition = { page: number; top: number; ratio: number; fontSize: number; lineHeight: number; readerWidth: number; fontFamily: ReaderFont; layout?: ReaderLayout };

function readSavedPosition(work: PcWork, editHash: string | null): SavedPosition | null {
  try {
    const raw = window.localStorage.getItem(`${readingKey(work, editHash)}.position`);
    if (raw === null) return null;
    const value = JSON.parse(raw) as Partial<SavedPosition> | null;
    if (!value || !Number.isSafeInteger(value.page) || !Number.isFinite(value.top) || !Number.isFinite(value.ratio)) return null;
    if (value.page! < 0 || value.top! < 0 || value.ratio! < 0 || value.ratio! > 1) return null;
    return value as SavedPosition;
  } catch { return null; }
}

/** 詳細画面の「続きから読む」のため。開いただけのものは続きとして扱わない。 */
export function hasReadingPosition(work: PcWork, editHash: string | null): boolean {
  const saved = readSavedPosition(work, editHash);
  return saved !== null && (saved.page > 0 || (saved.ratio ?? 0) > 0.01);
}

function positionFromRemote(start: RemoteStart, fontFamily: ReaderFont): SavedPosition {
  // 別の端末とは文字の大きさが違うので、割合で戻す。先頭から順に画像が埋まるのを待つよう top は末尾にする。
  return { page: start.page, top: Number.MAX_SAFE_INTEGER, ratio: start.ratio, fontSize: 0, lineHeight: 0, readerWidth: 0, fontFamily };
}

function sameRevision(metadata: PcMetadata, work: PcWork, editHash: string | null): boolean {
  return metadata.work.currentVersion === work.currentVersion
    && metadata.work.contentHash === work.contentHash
    && metadata.activeEditHash === editHash;
}

/** 画面を送るときに一行だけ残す。読んでいた行を見失わないため。 */
function stepHeight(viewport: HTMLElement): number {
  const paper = viewport.querySelector<HTMLElement>(".reading-paper");
  const line = paper ? Number.parseFloat(getComputedStyle(paper).lineHeight) : 0;
  const keep = Number.isFinite(line) && line > 0 ? line * 1.2 : 48;
  return Math.max(1, viewport.clientHeight - keep);
}

export function OnlineReader({ work, type, behavior, onBack, onTypeChange, onBehaviorChange, backLabel = "作品一覧に戻る", onSaved, offline = false, offlineCollections, offlineAuthorCover = false, offlineEditHash = null, snapshotGeneration }: { work: PcWork; type: ReaderType; behavior: ReaderBehavior; onBack: () => void; onTypeChange: (patch: Partial<ReaderType>) => void; onBehaviorChange: (patch: Partial<ReaderBehavior>) => void; backLabel?: string; onSaved?: () => void; offline?: boolean; offlineCollections?: PcCollection[]; offlineAuthorCover?: boolean; offlineEditHash?: string | null; snapshotGeneration?: number }) {
  const { fontSize, lineHeight, readerWidth, fontFamily, readerTheme } = type;
  const continuous = behavior.layout === "continuous";
  const [activeEditHash, setActiveEditHash] = useState<string | null | undefined>(offline ? offlineEditHash : undefined);
  const [pageIndex, setPageIndex] = useState(() => offline ? savedPage(work, offlineEditHash) : 0);
  const [page, setPage] = useState<PcPage | null>(null);
  const [html, setHtml] = useState("");
  const [loading, setLoading] = useState(true);
  const [imageResolutionDone, setImageResolutionDone] = useState(false);
  const [assetsReady, setAssetsReady] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const [saving, setSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState<string | null>(null);
  const [outline, setOutline] = useState<PcOutlineEntry[]>([]);
  const [offlineGeneration, setOfflineGeneration] = useState<string | null>(null);
  const [infoOpen, setInfoOpen] = useState(false);
  const [infoTab, setInfoTab] = useState<InfoTab>("info");
  const [typeOpen, setTypeOpen] = useState(false);
  const [typeTab, setTypeTab] = useState<"type" | "turn">("type");
  const [readerTitleExpanded, setReaderTitleExpanded] = useState(false);
  const [readerTitleOverflow, setReaderTitleOverflow] = useState(false);
  const [barsShown, setBarsShown] = useState(true);
  const [progress, setProgress] = useState(0);
  const [dragPosition, setDragPosition] = useState<number | null>(null);
  const [enterDirection, setEnterDirection] = useState<"next" | "prev" | null>(null);
  const [continuousTarget, setContinuousTarget] = useState<ContinuousTarget | null>(null);
  const [detailWork, setDetailWork] = useState<PcWork | null>(null);
  const [isEdited, setIsEdited] = useState(false);
  const [collections, setCollections] = useState<PcCollection[] | null>(offlineCollections ?? null);
  const touchStart = useRef<{ x: number; y: number } | null>(null);
  const scrollRef = useRef<HTMLElement>(null);
  const keyboardPageChange = useRef(false);
  const infoTriggerRef = useRef<HTMLButtonElement>(null);
  const typeTriggerRef = useRef<HTMLButtonElement>(null);
  const infoWasOpen = useRef(false);
  const typeWasOpen = useRef(false);
  const readerTitleRef = useRef<HTMLHeadingElement>(null);
  const touching = useRef(false);
  const swallowClick = useRef(false);
  const stepGoal = useRef<{ top: number; timer: number } | null>(null);
  const latestPosition = useRef<LivePosition | null>(null);
  const flushContinuous = useRef<(() => void) | null>(null);
  const targetNonce = useRef(0);
  const pendingRestore = useRef<SavedPosition | null>(offline ? readSavedPosition(work, offlineEditHash) : null);
  const pendingOutline = useRef<{ page: number; index: number } | null>(null);
  const [remoteOffer, setRemoteOffer] = useState<RemoteStart | null>(null);
  // 最初に開いたページは読み進めた印ではないので、送らない。
  const syncedPage = useRef<number | null>(null);
  const guidedPage = useRef(false);
  const queuedWrite = useRef<ReadingWrite | null>(null);
  const writeTimer = useRef<number | null>(null);
  const dismissedRemote = useRef(0);
  const currentPage = useRef(pageIndex);
  currentPage.current = pageIndex;
  const readingWork = detailWork ?? work;
  const positionKey = activeEditHash === undefined ? null : readingKey(readingWork, activeEditHash);

  useLayoutEffect(() => {
    if (!keyboardPageChange.current) return;
    keyboardPageChange.current = false;
    scrollRef.current?.focus();
  }, [pageIndex]);

  const reportPosition = (position: LivePosition) => {
    latestPosition.current = position;
    const next = position.pageCount > 0 ? Math.min(1, (position.page + position.ratio) / position.pageCount) : 0;
    setProgress((old) => Math.abs(old - next) < 0.002 ? old : next);
  };

  const adoptRevision = (metadata: PcMetadata): boolean => {
    if (activeEditHash !== undefined && sameRevision(metadata, readingWork, activeEditHash)) {
      setDetailWork(metadata.work);
      setIsEdited(metadata.isEdited);
      return false;
    }
    // A different edit or source version must never inherit the prior page or outline.
    pendingRestore.current = readSavedPosition(metadata.work, metadata.activeEditHash);
    pendingOutline.current = null;
    syncedPage.current = null;
    forgetWorkPages(work.id);
    setOutline([]);
    setDetailWork(metadata.work);
    setIsEdited(metadata.isEdited);
    setPageIndex(savedPage(metadata.work, metadata.activeEditHash));
    setActiveEditHash(metadata.activeEditHash);
    setRetry((current) => current + 1);
    return true;
  };

  /**
   * 本文を受け取り、今の版のものか確かめる。別の版だったら null を返す（読み直しは adoptRevision が始める）。
   * サーバーが版を本文に添えるので、確かめるための往復は増えない。添えない古いサーバーにだけ一度尋ねる。
   */
  const readPage = async (target: number): Promise<PcPage | null> => {
    const next = await fetchPage(work.id, target);
    const revision = next.revision;
    if (revision && revision.currentVersion === readingWork.currentVersion && revision.contentHash === readingWork.contentHash && revision.activeEditHash === activeEditHash) return next;
    const metadata = await getPcMetadata(work.id);
    if (adoptRevision(metadata)) return null;
    if (!revision) return next;
    // 版は変わっていない。覚えていた本文のほうが古かった。
    forgetWorkPages(work.id);
    return fetchPage(work.id, target);
  };

  /** 次に読まれるページを先に受け取る。今のページの描画が済んでから。 */
  const prefetchNext = (shown: PcPage) => {
    if (offline || shown.page + 1 >= shown.pageCount) return;
    window.setTimeout(() => prefetchPage(work.id, shown.page + 1), PREFETCH_DELAY_MS);
  };

  const imageSource = (assetId: number, width: number, generation: string | null): string | Promise<string> => {
    if (offline) return getOfflineAsset(work.id, assetId, generation!).then((asset) => `data:${asset.mimeType};base64,${asset.base64}`);
    const url = illustrationUrl(work.id, assetId, width, readingWork.currentVersion, readingWork.contentHash);
    if (!url) throw new Error("画像を読み込めません");
    return url;
  };

  useEffect(() => {
    if (offline) return;
    let alive = true;
    // 版と同期の確認を待つあいだに本文も受け取っておく。位置が同じなら待ち時間がなくなる。
    prefetchPage(work.id, savedPage(work, null));
    void Promise.all([getPcMetadata(work.id), refreshReadingWithin(SYNC_WAIT_MS)]).then(async ([metadata]) => {
      const start = await remoteStartFor(metadata.work, metadata.activeEditHash);
      if (!alive) return;
      setDetailWork(metadata.work);
      setIsEdited(metadata.isEdited);
      if (start) {
        // 時刻だけ新しくなると、次に開いたとき古い位置を「最新」と扱ってしまう。位置も同時に書く。
        const key = readingKey(metadata.work, metadata.activeEditHash);
        const adopted = positionFromRemote(start, fontFamily);
        try {
          window.localStorage.setItem(`${key}.page`, String(start.page));
          window.localStorage.setItem(`${key}.position`, JSON.stringify({ ...adopted, top: 0 }));
        } catch { /* storage unavailable */ }
        markLocalReading(key, start.deviceTime);
        guidedPage.current = start.guide;
        pendingRestore.current = start.guide ? null : adopted;
        setPageIndex(start.page);
      } else {
        pendingRestore.current = readSavedPosition(metadata.work, metadata.activeEditHash);
        setPageIndex(savedPage(metadata.work, metadata.activeEditHash));
      }
      setActiveEditHash(metadata.activeEditHash);
    }).catch(() => {
      if (!alive) return;
      // 本文が取得できる場合は読書を続ける。未知の編集状態で古い位置は流用しない。
      setActiveEditHash("unknown");
    });
    return () => { alive = false; };
  }, [offline, work.id]);

  const capturePosition = () => {
    const viewport = scrollRef.current;
    if (continuous || !viewport || loading || problem || !page) return;
    const length = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
    pendingRestore.current = {
      page: pageIndex,
      top: viewport.scrollTop,
      ratio: length > 0 ? viewport.scrollTop / length : 0,
      fontSize,
      lineHeight,
      readerWidth,
      fontFamily,
      layout: "pages",
    };
  };

  const continuousJump = (goal: Omit<ContinuousTarget, "nonce">) => {
    targetNonce.current += 1;
    setContinuousTarget({ ...goal, nonce: targetNonce.current });
  };

  /** 開始位置を、一続きの読み方の形へ直す。同じ設定で読んでいたなら正確な位置へ戻す。 */
  const continuousStart = (): ContinuousTarget => {
    const saved = pendingRestore.current?.page === pageIndex ? pendingRestore.current : null;
    pendingRestore.current = null;
    if (!saved) return { page: pageIndex, top: 0, nonce: 0 };
    const sameType = saved.layout === "continuous" && saved.fontSize === fontSize && saved.lineHeight === lineHeight && saved.readerWidth === readerWidth && saved.fontFamily === fontFamily;
    return sameType ? { page: saved.page, top: saved.top, nonce: 0 } : { page: saved.page, ratio: saved.ratio, nonce: 0 };
  };

  const sendQueuedWrite = () => {
    if (writeTimer.current !== null) window.clearTimeout(writeTimer.current);
    writeTimer.current = null;
    const write = queuedWrite.current;
    queuedWrite.current = null;
    if (write) void recordReading(write).then(() => sendReading());
  };

  const queueReadingWrite = (pageNumber: number, ratio: number, pageCount: number) => {
    if (!positionKey || activeEditHash === undefined || activeEditHash === "unknown") return;
    const time = Date.now();
    markLocalReading(positionKey, time);
    setRemoteOffer(null);
    queuedWrite.current = {
      source: readingWork.source,
      sourceId: readingWork.sourceId,
      version: readingWork.currentVersion,
      editHash: activeEditHash,
      page: pageNumber,
      position: Math.min(1, Math.max(0, ratio)),
      finished: pageNumber === pageCount - 1 && ratio >= 0.98,
      deviceTime: time,
    };
    if (writeTimer.current === null) writeTimer.current = window.setTimeout(sendQueuedWrite, WRITE_DELAY_MS);
    rememberReading(readingWork, pageCount > 0 ? (pageNumber + Math.min(1, Math.max(0, ratio))) / pageCount : 0, pageNumber === pageCount - 1 && ratio >= 0.98);
  };

  const acceptRemote = () => {
    if (!remoteOffer || !positionKey) return;
    markLocalReading(positionKey, remoteOffer.deviceTime);
    syncedPage.current = remoteOffer.page;
    guidedPage.current = remoteOffer.guide;
    if (continuous) {
      continuousJump({ page: remoteOffer.page, ratio: remoteOffer.guide ? 0 : remoteOffer.ratio });
      guidedPage.current = false;
      setRemoteOffer(null);
      return;
    }
    pendingRestore.current = remoteOffer.guide ? null : positionFromRemote(remoteOffer, fontFamily);
    if (remoteOffer.page === pageIndex) setRetry((current) => current + 1);
    else setPageIndex(remoteOffer.page);
    setRemoteOffer(null);
  };

  const dismissRemote = () => {
    if (remoteOffer) dismissedRemote.current = remoteOffer.deviceTime;
    setRemoteOffer(null);
  };

  useEffect(() => {
    if (!positionKey || activeEditHash === undefined || activeEditHash === "unknown") return;
    let alive = true;
    const check = () => {
      void refreshReading().then(() => remoteStartFor(readingWork, activeEditHash)).then((start) => {
        if (!alive || !start || start.deviceTime <= dismissedRemote.current) return;
        const ratio = latestPosition.current?.page === currentPage.current ? latestPosition.current.ratio : 0;
        if (start.page === currentPage.current && Math.abs(start.ratio - ratio) < 0.02) return;
        // 読んでいる最中に画面を動かさない。移動するかは読者に任せる。
        setRemoteOffer(start);
      });
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") check();
      else { flushContinuous.current?.(); sendQueuedWrite(); }
    };
    check();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      alive = false;
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [positionKey]);

  useEffect(() => {
    if (continuous || !imageResolutionDone || loading || problem) return;
    const viewport = scrollRef.current;
    if (!viewport) return;
    const images = [...viewport.querySelectorAll<HTMLImageElement>(".online-document img")];
    const outlineTarget = pendingOutline.current?.page === pageIndex
      ? viewport.querySelectorAll<HTMLElement>(".online-document h2")[pendingOutline.current.index]
      : null;
    const saved = pendingRestore.current?.page === pageIndex ? pendingRestore.current : null;
    const threshold = (saved?.top ?? 0) + viewport.clientHeight;
    // Only images before the restored reading position (or outline heading)
    // need to settle, and only when their height is unknown. The server sends
    // width and height, so sized images already hold their final space.
    const critical = images.filter((image) => !image.hasAttribute("width")).filter((image) => outlineTarget
      ? Boolean(image.compareDocumentPosition(outlineTarget) & Node.DOCUMENT_POSITION_FOLLOWING)
      : image.getBoundingClientRect().top - viewport.getBoundingClientRect().top + viewport.scrollTop <= threshold);
    let frame = 0;
    const settle = () => {
      if (critical.every((image) => !image.isConnected || (!image.hasAttribute("data-asset-id") && image.complete))) {
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(() => setAssetsReady(true));
      }
    };
    viewport.addEventListener("load", settle, true);
    viewport.addEventListener("error", settle, true);
    viewport.addEventListener("piep-asset-change", settle);
    settle();
    return () => {
      viewport.removeEventListener("load", settle, true);
      viewport.removeEventListener("error", settle, true);
      viewport.removeEventListener("piep-asset-change", settle);
      cancelAnimationFrame(frame);
    };
  }, [continuous, html, imageResolutionDone, loading, problem, pageIndex]);

  useLayoutEffect(() => {
    const title = readerTitleRef.current;
    if (!title) return;
    const check = () => { if (!readerTitleExpanded) setReaderTitleOverflow(title.scrollHeight > title.clientHeight + 1); };
    check();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(check);
    observer?.observe(title);
    return () => observer?.disconnect();
  }, [readerTitleExpanded, pageIndex, fontSize, work.title, html, continuous, page]);

  useEffect(() => {
    const backWindow = window as Window & { piepAndroidReaderLayerBack?: () => boolean };
    backWindow.piepAndroidReaderLayerBack = () => {
      if (infoOpen) { setInfoOpen(false); return true; }
      if (typeOpen) { setTypeOpen(false); return true; }
      return false;
    };
    return () => { delete backWindow.piepAndroidReaderLayerBack; };
  }, [infoOpen, typeOpen]);

  useEffect(() => {
    if (infoOpen) {
      infoWasOpen.current = true;
      document.querySelector<HTMLElement>(".reader-info-sheet[role='dialog'] button")?.focus();
    } else if (infoWasOpen.current) {
      infoWasOpen.current = false;
      infoTriggerRef.current?.focus();
    }
  }, [infoOpen]);

  useEffect(() => {
    if (typeOpen) {
      typeWasOpen.current = true;
      document.querySelector<HTMLElement>(".reader-type-sheet[role='dialog'] button")?.focus();
    } else if (typeWasOpen.current) {
      typeWasOpen.current = false;
      typeTriggerRef.current?.focus();
    }
  }, [typeOpen]);

  const save = async () => {
    setSaving(true);
    setSaveMessage(null);
    try { await saveOfflineWork(work.id); onSaved?.(); setSaveMessage("端末に保存しました"); }
    catch (error) { setSaveMessage(pcErrorMessage(error)); }
    finally { setSaving(false); }
  };

  useEffect(() => {
    if (!positionKey) return;
    try { window.localStorage.setItem(`${positionKey}.page`, String(pageIndex)); } catch { /* storage unavailable */ }
  }, [positionKey, pageIndex]);

  useEffect(() => {
    const viewport = scrollRef.current;
    if (continuous || !viewport || !positionKey || loading || problem || !page || page.page !== pageIndex) return;
    let restored = false;
    let changed = false;
    let timer: number | null = null;
    let frame = 0;
    const saved = pendingRestore.current?.page === pageIndex ? pendingRestore.current : null;
    const pageCount = page.pageCount;
    let restoredTop: number | null = null;
    let size = { width: viewport.clientWidth, height: viewport.clientHeight };
    let fraction = 0;
    let latest: SavedPosition | null = null;
    if (syncedPage.current === null) syncedPage.current = pageIndex;
    else if (syncedPage.current !== pageIndex) {
      syncedPage.current = pageIndex;
      queueReadingWrite(pageIndex, saved?.ratio ?? 0, pageCount);
    }
    const readPosition = () => {
      const length = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
      fraction = viewport.scrollHeight > 0 ? viewport.scrollTop / viewport.scrollHeight : 0;
      const position: SavedPosition = {
        page: pageIndex,
        top: viewport.scrollTop,
        ratio: length > 0 ? viewport.scrollTop / length : 0,
        fontSize,
        lineHeight,
        readerWidth,
        fontFamily,
        layout: "pages",
      };
      latest = position;
      reportPosition({ page: pageIndex, top: position.top, ratio: position.ratio, pageCount });
      return position;
    };
    const flush = () => {
      if (!changed || !restored || !latest) return;
      changed = false;
      try { window.localStorage.setItem(`${positionKey}.position`, JSON.stringify(latest)); } catch { /* storage unavailable */ }
      // 位置を戻しただけのスクロールは、読み進めた印ではない。
      if (restoredTop === null || Math.abs(latest.top - restoredTop) > 2) queueReadingWrite(pageIndex, latest.ratio, pageCount);
    };
    const onScroll = () => {
      if (!restored) return;
      // 回転の直後は高さが変わっただけで、読み進めたわけではない。
      if (size.width !== viewport.clientWidth || size.height !== viewport.clientHeight) return;
      readPosition();
      changed = true;
      if (timer === null) timer = window.setTimeout(() => { timer = null; flush(); }, 400);
    };
    const onUserInput = () => {
      if (!saved || assetsReady) return;
      pendingRestore.current = null;
      restored = true;
    };
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => {
      if (size.width === viewport.clientWidth && size.height === viewport.clientHeight) return;
      size = { width: viewport.clientWidth, height: viewport.clientHeight };
      if (restored) { viewport.scrollTop = fraction * viewport.scrollHeight; restoredTop = viewport.scrollTop; readPosition(); }
    });
    resize?.observe(viewport);
    viewport.addEventListener("scroll", onScroll, { passive: true });
    viewport.addEventListener("touchstart", onUserInput, { passive: true });
    viewport.addEventListener("wheel", onUserInput, { passive: true });
    viewport.addEventListener("pointerdown", onUserInput, { passive: true });
    if (saved) {
      const sameType = (saved.layout ?? "pages") === "pages" && saved.fontSize === fontSize && saved.lineHeight === lineHeight && saved.readerWidth === readerWidth && saved.fontFamily === fontFamily;
      const deadline = performance.now() + 2000;
      const restore = () => {
        const length = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
        const target = sameType ? Math.min(saved.top, length) : saved.ratio * length;
        viewport.scrollTop = target;
        if (Math.abs(viewport.scrollTop - target) < 2 || performance.now() >= deadline) {
          restored = true;
          restoredTop = viewport.scrollTop;
          pendingRestore.current = null;
          readPosition();
        } else frame = requestAnimationFrame(restore);
      };
      if (assetsReady) frame = requestAnimationFrame(restore);
    } else {
      pendingRestore.current = null;
      restored = true;
      readPosition();
    }
    return () => {
      viewport.removeEventListener("scroll", onScroll);
      viewport.removeEventListener("touchstart", onUserInput);
      viewport.removeEventListener("wheel", onUserInput);
      viewport.removeEventListener("pointerdown", onUserInput);
      resize?.disconnect();
      cancelAnimationFrame(frame);
      if (timer !== null) window.clearTimeout(timer);
      flush();
    };
  }, [continuous, positionKey, pageIndex, page, html, loading, problem, assetsReady, fontSize, lineHeight, readerWidth, fontFamily]);

  // 上の片付けが最後の位置を積むので、この片付けはその後に走らせる。
  useEffect(() => () => { flushContinuous.current?.(); sendQueuedWrite(); }, []);

  useEffect(() => {
    const target = pendingOutline.current;
    const viewport = scrollRef.current;
    if (continuous || !target || !viewport || infoOpen || loading || !assetsReady || page?.page !== target.page) return;
    const heading = viewport.querySelectorAll<HTMLElement>(".online-document h2")[target.index];
    pendingOutline.current = null;
    if (!heading) return;
    const offset = heading.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
    viewport.scrollTop += offset - 16;
  }, [continuous, page, html, pageIndex, infoOpen, loading, assetsReady]);

  useEffect(() => {
    if (!offline && activeEditHash === undefined) return;
    if (offline && !offlineGeneration) return;
    let alive = true;
    void (async () => {
      try {
        const entries = await (offline ? getOfflineOutline(work.id, offlineGeneration!) : getPcOutline(work.id));
        if (!alive) return;
        if (!offline) {
          const after = await getPcMetadata(work.id);
          if (!alive || adoptRevision(after)) return;
        }
        setOutline(entries);
      } catch (error) {
        if (!alive) return;
        setOutline([]);
        if (offline && offlineSnapshotChanged(error)) setRetry((current) => current + 1);
      }
    })();
    return () => { alive = false; };
  }, [offline, work.id, positionKey, infoOpen, offlineGeneration]);

  useEffect(() => {
    if (!infoOpen || offline) return;
    let alive = true;
    void getPcWorkCollections(work.id).then((memberships) => { if (alive) setCollections(memberships); }).catch(() => {});
    return () => { alive = false; };
  }, [infoOpen, offline, work.id]);

  useEffect(() => {
    if (activeEditHash === undefined || continuous) return;
    let cancelled = false;
    setLoading(true);
    setProblem(null);
    setPage(null);
    setHtml("");
    if (offline) { setOfflineGeneration(null); setOutline([]); }
    setImageResolutionDone(false);
    setAssetsReady(false);
    void (async () => {
      try {
        const next = await (offline ? getOfflinePage(work.id, pageIndex) : readPage(pageIndex));
        if (cancelled || !next) return;
        if (guidedPage.current) {
          guidedPage.current = false;
          if (pageIndex !== 0 && (pageIndex >= next.pageCount || next.page !== pageIndex)) {
            syncedPage.current = null;
            setPageIndex(0);
            return;
          }
        }
        const pageGeneration = "generation" in next && typeof next.generation === "string" ? next.generation : null;
        if (offline) {
          if (!pageGeneration) throw new Error("保存済み作品の版を確認できませんでした。もう一度読み込んでください");
          setOfflineGeneration(pageGeneration);
        }
        setPage(next);
        if (next.page !== pageIndex) setPageIndex(next.page);
        const clean = safePageHtml(next.html);
        setHtml(clean);
        setLoading(false);
        prefetchNext(next);
        // Let React mount the sanitized document before filling its images.
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        if (cancelled) return;
        const viewport = scrollRef.current;
        setImageResolutionDone(true);
        if (!viewport) return;
        await fillDocumentImages({
          root: viewport,
          scroller: viewport,
          source: (assetId, width) => imageSource(assetId, width, pageGeneration),
          isCancelled: () => cancelled,
          isSnapshotChange: (error) => offline && offlineSnapshotChanged(error),
          onSnapshotChanged: () => {
            setPage(null);
            setHtml("");
            setOfflineGeneration(null);
            setImageResolutionDone(false);
            setAssetsReady(false);
            setLoading(true);
            setRetry((current) => current + 1);
          },
          onChange: () => viewport.dispatchEvent(new Event("piep-asset-change")),
        });
      } catch (error) {
        if (cancelled) return;
        if (guidedPage.current && pageIndex !== 0) {
          // 目安のページが今の版に無い。最初から読む。
          guidedPage.current = false;
          syncedPage.current = null;
          setPageIndex(0);
          return;
        }
        setProblem(pcErrorMessage(error));
        setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [work.id, pageIndex, retry, offline, positionKey, continuous]);

  const goToPage = (next: number, edge: "start" | "end" = "start") => {
    if (!page || !Number.isSafeInteger(next)) return;
    const target = Math.min(Math.max(next, 0), page.pageCount - 1);
    if (continuous) {
      continuousJump({ page: target, top: 0 });
      return;
    }
    if (target === pageIndex) return;
    setEnterDirection(behavior.motion === "slide" ? (target > pageIndex ? "next" : "prev") : null);
    pendingRestore.current = edge === "end" ? { page: target, top: Number.MAX_SAFE_INTEGER, ratio: 1, fontSize, lineHeight, readerWidth, fontFamily, layout: "pages" } : null;
    setPageIndex(target);
  };

  const openContentLink = (event: React.MouseEvent<HTMLDivElement>) => {
    const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
    if (!anchor || !event.currentTarget.contains(anchor)) return;
    event.preventDefault();
    const sourcePage = Number(anchor.dataset.page);
    if (anchor.classList.contains("jump-link") && Number.isSafeInteger(sourcePage) && sourcePage >= 1) {
      const target = page?.sourcePageStarts[sourcePage - 1];
      if (target !== undefined) goToPage(target);
      return;
    }
    const href = anchor.getAttribute("href");
    if (href && /^https?:\/\//i.test(href)) {
      void openUrl(href).catch(() => setSaveMessage("リンクを開けませんでした"));
    }
  };

  const jumpToOutline = (entry: PcOutlineEntry) => {
    // 目次の page/index は転送後の読書ページと、そのページ内の h2 の順番。
    setInfoOpen(false);
    if (continuous) {
      continuousJump({ page: entry.page - 1, heading: entry.index });
      return;
    }
    pendingRestore.current = null;
    pendingOutline.current = { page: entry.page - 1, index: entry.index };
    setPageIndex(entry.page - 1);
  };

  /** 作品全体に対する割合の位置へ移動する。 */
  const goToProgress = (value: number) => {
    const count = page?.pageCount ?? 1;
    const scaled = Math.min(0.9999, Math.max(0, value)) * count;
    const targetPage = Math.floor(scaled);
    const ratio = scaled - targetPage;
    if (continuous) {
      continuousJump({ page: targetPage, ratio });
      return;
    }
    if (targetPage === pageIndex) {
      const viewport = scrollRef.current;
      if (viewport) viewport.scrollTop = ratio * Math.max(0, viewport.scrollHeight - viewport.clientHeight);
      return;
    }
    pendingRestore.current = { page: targetPage, top: Number.MAX_SAFE_INTEGER, ratio, fontSize: 0, lineHeight: 0, readerWidth: 0, fontFamily };
    setPageIndex(targetPage);
  };

  /** 一画面ぶん送る。ページの端に着いていれば次のページへ。タップ・スワイプ・キーで共通。 */
  const turn = (direction: 1 | -1) => {
    const viewport = scrollRef.current;
    if (!viewport || infoOpen || typeOpen) return;
    if (!continuous && (loading || problem || !page)) return;
    if (behavior.bars === "auto") setBarsShown(false);
    const end = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
    const smooth = behavior.motion === "slide" && !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const from = stepGoal.current?.top ?? viewport.scrollTop;
    if ((direction > 0 && from < end - 2) || (direction < 0 && from > 2)) {
      const top = Math.min(end, Math.max(0, from + direction * stepHeight(viewport)));
      if (stepGoal.current) window.clearTimeout(stepGoal.current.timer);
      stepGoal.current = { top, timer: window.setTimeout(() => { stepGoal.current = null; }, 450) };
      viewport.scrollTo({ top, behavior: smooth ? "smooth" : "instant" });
      return;
    }
    if (stepGoal.current) { window.clearTimeout(stepGoal.current.timer); stepGoal.current = null; }
    if (continuous || !page) return;
    const next = pageIndex + direction;
    if (next >= 0 && next < page.pageCount) goToPage(next, direction < 0 ? "end" : "start");
  };
  const turnRef = useRef(turn);
  turnRef.current = turn;

  useEffect(() => {
    const native = window as Window & { piepAndroidReader?: { setVolumeKeys: (enabled: boolean) => void }; piepAndroidVolumeKey?: (direction: number) => void };
    native.piepAndroidReader?.setVolumeKeys(behavior.volumeKeys);
    native.piepAndroidVolumeKey = (direction) => turnRef.current(direction > 0 ? 1 : -1);
    return () => {
      native.piepAndroidReader?.setVolumeKeys(false);
      delete native.piepAndroidVolumeKey;
    };
  }, [behavior.volumeKeys]);

  const finishHorizontalSwipe = (event: React.TouchEvent<HTMLElement>) => {
    const start = touchStart.current;
    touchStart.current = null;
    touching.current = false;
    if (!behavior.swipe || !start || event.changedTouches.length !== 1) return;
    const dx = event.changedTouches[0].clientX - start.x;
    const dy = event.changedTouches[0].clientY - start.y;
    if (Math.abs(dx) < 85 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
    const forward = behavior.reverse ? dx > 0 : dx < 0;
    turn(forward ? 1 : -1);
  };

  const tapReader = (event: React.MouseEvent<HTMLElement>) => {
    if (swallowClick.current) { swallowClick.current = false; return; }
    if ((event.target as Element).closest("a[href], button, input, summary, label")) return;
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const x = (event.clientX - rect.left) / rect.width;
    const y = (event.clientY - rect.top) / rect.height;
    let zone = 0;
    if (behavior.tap === "sides") zone = (x < 0.3 ? -1 : x > 0.7 ? 1 : 0) * (behavior.reverse ? -1 : 1);
    else if (behavior.tap === "updown") zone = y < 0.3 ? -1 : y > 0.7 ? 1 : 0;
    if (zone === 0) { setBarsShown((shown) => !shown); return; }
    turn(zone > 0 ? 1 : -1);
  };

  const changeType = (patch: Partial<ReaderType>) => {
    // 文字の設定が変わると高さが変わるので、読んでいた場所を覚えてから反映する。
    if (patch.fontSize !== undefined || patch.lineHeight !== undefined || patch.readerWidth !== undefined || patch.fontFamily !== undefined) capturePosition();
    onTypeChange(patch);
  };

  const changeBehavior = (patch: Partial<ReaderBehavior>) => {
    if (patch.layout && patch.layout !== behavior.layout) {
      const position = latestPosition.current;
      flushContinuous.current?.();
      if (position) {
        // 読み方を変えても同じ場所から続ける。top の基準が違うので割合で戻す。
        pendingRestore.current = { page: position.page, top: Number.MAX_SAFE_INTEGER, ratio: position.ratio, fontSize: 0, lineHeight: 0, readerWidth: 0, fontFamily };
        syncedPage.current = position.page;
        setPageIndex(position.page);
      }
      setContinuousTarget(null);
      setEnterDirection(null);
    }
    onBehaviorChange(patch);
  };

  const loadSection = async (target: number): Promise<LoadedSection | null> => {
    const next = await (offline ? getOfflinePage(work.id, target) : readPage(target));
    if (!next) return null;
    const generation = "generation" in next && typeof next.generation === "string" ? next.generation : null;
    if (offline && !generation) throw new Error("保存済み作品の版を確認できませんでした。もう一度読み込んでください");
    prefetchNext(next);
    return { page: next, html: safePageHtml(next.html), generation };
  };

  const continuousInfo = (next: PcPage, generation: string | null) => {
    setPage((old) => old && old.pageCount === next.pageCount && old.page === currentPage.current ? old : { ...next, page: currentPage.current });
    if (offline && generation) setOfflineGeneration(generation);
    setLoading(false);
    setProblem(null);
  };

  const continuousPosition = (position: LivePosition) => {
    reportPosition(position);
    if (position.page !== currentPage.current) {
      currentPage.current = position.page;
      setPageIndex(position.page);
      setPage((old) => old ? { ...old, page: position.page } : old);
    }
  };

  const continuousCommit = (position: LivePosition) => {
    if (!positionKey) return;
    const saved: SavedPosition = { page: position.page, top: position.top, ratio: position.ratio, fontSize, lineHeight, readerWidth, fontFamily, layout: "continuous" };
    try { window.localStorage.setItem(`${positionKey}.position`, JSON.stringify(saved)); } catch { /* storage unavailable */ }
    queueReadingWrite(position.page, position.ratio, position.pageCount);
  };

  const rejectContinuousStart = (loaded: PcPage | null): boolean => {
    if (!guidedPage.current) return false;
    guidedPage.current = false;
    const start = currentPage.current;
    if (start !== 0 && (!loaded || start >= loaded.pageCount || loaded.page !== start)) {
      // 目安のページが今の版に無い。最初から読む。
      syncedPage.current = null;
      setPageIndex(0);
      continuousJump({ page: 0, top: 0 });
      return true;
    }
    return false;
  };

  const sidebarWork = detailWork ?? work;
  const pageCount = page?.pageCount ?? 1;
  const pinned = behavior.bars === "pinned";
  const barsVisible = pinned || barsShown || (!continuous && (loading || Boolean(problem)));
  const multiPage = pageCount > 1;
  const showPager = !continuous && multiPage;
  const shownProgress = dragPosition ?? progress;
  const percentLabel = progressLabel(shownProgress, pageCount);
  const dragPage = Math.min(pageCount - 1, Math.floor(Math.min(0.9999, shownProgress) * pageCount));
  const statusLabel = !continuous && multiPage && !(showPager && behavior.pageButtons === "always") ? `${(dragPosition === null ? pageIndex : dragPage) + 1} / ${pageCount}` : null;
  const titleBlock = <><h1 ref={readerTitleRef} className={readerTitleExpanded ? "reader-work-title expanded" : "reader-work-title"}>{work.title}</h1>{readerTitleOverflow && <button className="reader-title-toggle" onClick={() => setReaderTitleExpanded((expanded) => !expanded)} aria-expanded={readerTitleExpanded}>{readerTitleExpanded ? "タイトルを短く表示" : "タイトルをすべて表示"}</button>}<div className="reader-rule" /></>;
  const commitDrag = () => {
    if (dragPosition === null) return;
    goToProgress(dragPosition);
    setDragPosition(null);
  };

  return <div className={`reader-screen reader-theme--${readerTheme} ${pinned ? "reader-bars--pinned" : "reader-bars--auto"}`} data-bars={barsVisible ? "shown" : "hidden"}>
    <span className="reader-safe-top" aria-hidden="true" />
    <header className="reader-toolbar">
      <button className="reader-icon-button" onClick={onBack} aria-label={backLabel}><ChevronLeft aria-hidden="true" /></button>
      <span className="reader-toolbar-title">{work.title}</span>
    </header>
    <div className="reader-toasts">
      {saveMessage && !infoOpen && <div className="reader-notice" role="status"><span>{saveMessage}</span><button type="button" onClick={() => setSaveMessage(null)} aria-label="閉じる"><X aria-hidden="true" /></button></div>}
      {remoteOffer && <div className="reader-notice reader-sync-offer" role="status"><span>{remoteOffer.guide ? "別の版を" : "別の端末で"} {remoteOffer.page + 1} ページまで読んでいます</span><button onClick={acceptRemote}>そこへ移動</button><button onClick={dismissRemote}>閉じる</button></div>}
    </div>
    <main className="reader-scroll" ref={scrollRef} key={continuous ? "continuous" : pageIndex} tabIndex={0} aria-label="本文。Page Up、Page Down、左右矢印、Spaceで移動" onClick={tapReader} onKeyDown={(event) => {
      const direction = readerKeyDirection(event);
      if (!direction || infoOpen || typeOpen) return;
      event.preventDefault();
      keyboardPageChange.current = !continuous;
      turn(direction);
    }} onTouchStart={(event) => {
      touching.current = true;
      if (event.touches.length === 1) touchStart.current = { x: event.touches[0].clientX, y: event.touches[0].clientY };
    }} onTouchMove={() => { if (behavior.bars === "auto" && barsShown) setBarsShown(false); }} onTouchEnd={finishHorizontalSwipe}><article className={`reading-paper online-paper reader-font--${fontFamily}${!continuous && enterDirection ? ` reader-enter--${enterDirection}` : ""}`} style={{ maxWidth: `${readerWidth}px`, fontSize: `${fontSize}px`, lineHeight }}>
      {continuous
        ? activeEditHash !== undefined && <ContinuousDocument key={`${positionKey}:${retry}`} viewportRef={scrollRef} load={loadSection} imageSource={imageSource} isSnapshotChange={(error) => offline && offlineSnapshotChanged(error)} onSnapshotChanged={() => setRetry((current) => current + 1)} start={continuousStart} target={continuousTarget} typeKey={`${fontSize}:${lineHeight}:${readerWidth}:${fontFamily}`} header={titleBlock} onInfo={continuousInfo} onPosition={continuousPosition} onCommit={continuousCommit} onLink={openContentLink} onStartRejected={rejectContinuousStart} flushRef={flushContinuous} />
        : <>
          {pageIndex === 0 && titleBlock}
          {loading ? <p>本文を読み込んでいます…</p> : problem ? <div className="reader-error" role="alert"><p>{problem}</p><button onClick={() => setRetry((current) => current + 1)}>もう一度読み込む</button></div> : <div className="online-document" onClick={openContentLink} dangerouslySetInnerHTML={{ __html: html }} />}
        </>}
    </article></main>
    <div className="reader-bottom">
      <div className="reader-panel" aria-hidden={!barsVisible} inert={!barsVisible}>
        {/* 進み具合と、読みながら触るものを一行に。字の説明は付けず、印だけを親指の届く下に置く。 */}
        <div className="reader-progress-row">
          <input type="range" className="reader-progress-slider" min={0} max={1000} step={1} value={Math.round(shownProgress * 1000)} aria-label="読書位置" onChange={(event) => setDragPosition(Number(event.target.value) / 1000)} onPointerUp={commitDrag} onTouchEnd={commitDrag} onKeyUp={commitDrag} />
          <span className="reader-progress-label">{percentLabel}</span>
          <div className="reader-actions">
            {outline.length > 0 && <button type="button" className="reader-icon-button" onClick={() => { setInfoTab("outline"); setInfoOpen(true); }} aria-label="目次" title="目次"><TableOfContents aria-hidden="true" /></button>}
            <button ref={typeTriggerRef} type="button" className="reader-icon-button" onClick={() => setTypeOpen(true)} aria-label="文字とページ送りの設定" title="表示"><ALargeSmall aria-hidden="true" /></button>
            <button ref={infoTriggerRef} type="button" className="reader-icon-button" onClick={() => { setInfoTab("info"); setInfoOpen(true); }} aria-label="作品情報" title="作品情報"><Icons.workInfo aria-hidden="true" /></button>
            <WorkFavoriteButton work={sidebarWork} className="reader-icon-button" readOnly={offline} />
          </div>
        </div>
        {showPager && behavior.pageButtons === "menu" && <ReaderPager page={pageIndex} pageCount={pageCount} disabled={loading} onChange={goToPage} />}
      </div>
      {showPager && behavior.pageButtons === "always" && <div className="reader-pagebar"><ReaderPager page={pageIndex} pageCount={pageCount} disabled={loading} onChange={goToPage} /></div>}
      <div className="reader-status" aria-live="off"><span>{statusLabel ?? ""}</span><span>{percentLabel}</span></div>
    </div>
    {typeOpen && <div className="reader-sheet-layer reader-sheet-layer--clear">
      <button className="reader-sheet-scrim" type="button" onClick={() => setTypeOpen(false)} aria-label="設定を閉じる" />
      <aside className="reader-sheet reader-type-sheet" role="dialog" aria-modal="true" aria-label="文字とページ送りの設定">
        <span className="reader-sheet-grab" aria-hidden="true" />
        <header className="reader-sheet-header">
          <div className="reader-sheet-tabs" role="tablist" aria-label="設定の種類">
            <button type="button" role="tab" aria-selected={typeTab === "type"} onClick={() => setTypeTab("type")}>文字と色</button>
            <button type="button" role="tab" aria-selected={typeTab === "turn"} onClick={() => setTypeTab("turn")}>ページ送り</button>
          </div>
          <button type="button" className="reader-icon-button" onClick={() => setTypeOpen(false)} aria-label="閉じる"><X aria-hidden="true" /></button>
        </header>
        <div className="reader-sheet-body">{typeTab === "type" ? <ReaderTypeControls value={type} onChange={changeType} /> : <ReaderBehaviorControls value={behavior} onChange={changeBehavior} />}</div>
      </aside>
    </div>}
    {infoOpen && <ReaderInfoSheet tab={infoTab} onTabChange={setInfoTab} onClose={() => setInfoOpen(false)} work={sidebarWork} isEdited={isEdited} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} outline={outline} currentPage={pageIndex} pageCount={pageCount} progress={progress} collections={collections} saving={saving} saveMessage={saveMessage} onSave={() => void save()} onOutline={jumpToOutline} />}
  </div>;
}
