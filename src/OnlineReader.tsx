import { useEffect, useLayoutEffect, useRef, useState } from "react";
import DOMPurify from "dompurify";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ProviderMark } from "./ProviderMark";
import { WorkThumbnail } from "./WorkThumbnail";
import { AuthorAvatar } from "./WorkCard";
import { handleReaderKeyboard } from "./readerKeyboard";
import { readRemotePosition, recordReading, refreshReading, refreshReadingWithin, remoteStart, sendReading, type ReadingWrite, type RemoteStart } from "./readingSync";
import { getOfflineAsset, getOfflineOutline, getOfflinePage, getPcAsset, getPcMetadata, getPcOutline, getPcPage, getPcWorkCollections, pcErrorMessage, saveOfflineWork, type PcCollection, type PcMetadata, type PcOutlineEntry, type PcPage, type PcWork } from "./pcApi";

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
const SYNC_WAIT_MS = 1500;
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

type SavedPosition = { page: number; top: number; ratio: number; fontSize: number; lineHeight: number; readerWidth: number; fontFamily: "serif" | "sans" };

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

function positionFromRemote(start: RemoteStart, fontFamily: "serif" | "sans"): SavedPosition {
  // 別の端末とは文字の大きさが違うので、割合で戻す。先頭から順に画像が埋まるのを待つよう top は末尾にする。
  return { page: start.page, top: Number.MAX_SAFE_INTEGER, ratio: start.ratio, fontSize: 0, lineHeight: 0, readerWidth: 0, fontFamily };
}

function sameRevision(metadata: PcMetadata, work: PcWork, editHash: string | null): boolean {
  return metadata.work.currentVersion === work.currentVersion
    && metadata.work.contentHash === work.contentHash
    && metadata.activeEditHash === editHash;
}

function safeHtml(source: string): string {
  return DOMPurify.sanitize(source, {
    ALLOWED_TAGS: ["p", "br", "h1", "h2", "h3", "ruby", "rt", "rp", "span", "strong", "em", "b", "i", "u", "s", "blockquote", "figure", "figcaption", "img", "hr", "a", "div", "ul", "ol", "li", "code", "pre"],
    ALLOWED_ATTR: ["class", "alt", "data-asset-id", "data-page", "href"],
    FORBID_TAGS: ["script", "style", "iframe", "object", "embed", "form", "svg", "math", "template"],
  });
}

function descriptionText(source: string): string {
  const safe = DOMPurify.sanitize(source, { ALLOWED_TAGS: ["br"], ALLOWED_ATTR: [] });
  const holder = document.createElement("div");
  holder.innerHTML = safe.replace(/<br\s*\/?\s*>/gi, "\n");
  return holder.textContent?.trim() ?? "";
}

function displayDate(value: string | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleDateString("ja-JP");
}

function safePageHtml(source: string): string {
  const holder = document.createElement("div");
  holder.innerHTML = safeHtml(source);
  for (const anchor of holder.querySelectorAll<HTMLAnchorElement>("a")) {
    const page = anchor.dataset.page;
    if (anchor.classList.contains("jump-link") && page && /^\d+$/.test(page)) {
      anchor.setAttribute("href", "#");
      continue;
    }
    const href = anchor.getAttribute("href");
    if (!href || !/^https?:\/\//i.test(href)) anchor.removeAttribute("href");
    anchor.removeAttribute("data-page");
  }
  return holder.innerHTML;
}

export function OnlineReader({ work, fontSize, lineHeight, readerWidth, fontFamily, readerTheme, onBack, onFontSizeChange, onLineHeightChange, onReaderWidthChange, onFontFamilyChange, onReaderThemeChange, backLabel = "作品一覧に戻る", onSaved, offline = false, offlineCollections, offlineAuthorCover = false, offlineEditHash = null, snapshotGeneration }: { work: PcWork; fontSize: number; lineHeight: number; readerWidth: number; fontFamily: "serif" | "sans"; readerTheme: "white" | "paper" | "night"; onBack: () => void; onFontSizeChange: (value: number) => void; onLineHeightChange: (value: number) => void; onReaderWidthChange: (value: number) => void; onFontFamilyChange: (value: "serif" | "sans") => void; onReaderThemeChange: (value: "white" | "paper" | "night") => void; backLabel?: string; onSaved?: () => void; offline?: boolean; offlineCollections?: PcCollection[]; offlineAuthorCover?: boolean; offlineEditHash?: string | null; snapshotGeneration?: number }) {
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
  const [outlineOpen, setOutlineOpen] = useState(false);
  const [typeOpen, setTypeOpen] = useState(false);
  const [readerTitleExpanded, setReaderTitleExpanded] = useState(false);
  const [sidebarTitleExpanded, setSidebarTitleExpanded] = useState(false);
  const [readerTitleOverflow, setReaderTitleOverflow] = useState(false);
  const [sidebarTitleOverflow, setSidebarTitleOverflow] = useState(false);
  const [detailWork, setDetailWork] = useState<PcWork | null>(null);
  const [isEdited, setIsEdited] = useState(false);
  const [collections, setCollections] = useState<PcCollection[] | null>(offlineCollections ?? null);
  const [jumpPage, setJumpPage] = useState(1);
  const touchStart = useRef<{ x: number; y: number } | null>(null);
  const scrollRef = useRef<HTMLElement>(null);
  const keyboardPageChange = useRef(false);
  const outlineTriggerRef = useRef<HTMLButtonElement>(null);
  const typeTriggerRef = useRef<HTMLButtonElement>(null);
  const outlineWasOpen = useRef(false);
  const typeWasOpen = useRef(false);
  const readerTitleRef = useRef<HTMLHeadingElement>(null);
  const sidebarTitleRef = useRef<HTMLElement>(null);
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
    setOutline([]);
    setDetailWork(metadata.work);
    setIsEdited(metadata.isEdited);
    setPageIndex(savedPage(metadata.work, metadata.activeEditHash));
    setActiveEditHash(metadata.activeEditHash);
    setRetry((current) => current + 1);
    return true;
  };

  useEffect(() => {
    if (offline) return;
    let alive = true;
    void Promise.all([getPcMetadata(work.id), refreshReadingWithin(SYNC_WAIT_MS)]).then(async ([metadata]) => {
      const start = await remoteStartFor(metadata.work, metadata.activeEditHash);
      if (!alive) return;
      setDetailWork(metadata.work);
      setIsEdited(metadata.isEdited);
      if (start) {
        markLocalReading(readingKey(metadata.work, metadata.activeEditHash), start.deviceTime);
        guidedPage.current = start.guide;
        pendingRestore.current = start.guide ? null : positionFromRemote(start, fontFamily);
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
    if (!viewport || loading || problem || !page) return;
    const length = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
    pendingRestore.current = {
      page: pageIndex,
      top: viewport.scrollTop,
      ratio: length > 0 ? viewport.scrollTop / length : 0,
      fontSize,
      lineHeight,
      readerWidth,
      fontFamily,
    };
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
  };

  const acceptRemote = () => {
    if (!remoteOffer || !positionKey) return;
    markLocalReading(positionKey, remoteOffer.deviceTime);
    syncedPage.current = remoteOffer.page;
    guidedPage.current = remoteOffer.guide;
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
        const viewport = scrollRef.current;
        const length = viewport ? Math.max(0, viewport.scrollHeight - viewport.clientHeight) : 0;
        const ratio = viewport && length > 0 ? viewport.scrollTop / length : 0;
        if (start.page === currentPage.current && Math.abs(start.ratio - ratio) < 0.02) return;
        // 読んでいる最中に画面を動かさない。移動するかは読者に任せる。
        setRemoteOffer(start);
      });
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") check();
      else sendQueuedWrite();
    };
    check();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      alive = false;
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [positionKey]);

  useEffect(() => {
    if (!imageResolutionDone || loading || problem) return;
    const viewport = scrollRef.current;
    if (!viewport) return;
    const images = [...viewport.querySelectorAll<HTMLImageElement>(".online-document img")];
    const outlineTarget = pendingOutline.current?.page === pageIndex
      ? viewport.querySelectorAll<HTMLElement>(".online-document h2")[pendingOutline.current.index]
      : null;
    const saved = pendingRestore.current?.page === pageIndex ? pendingRestore.current : null;
    const threshold = (saved?.top ?? 0) + viewport.clientHeight;
    // Only images before the restored reading position (or outline heading)
    // need to settle. Later illustrations may continue loading while reading.
    const critical = images.filter((image) => outlineTarget
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
  }, [html, imageResolutionDone, loading, problem, pageIndex]);

  useLayoutEffect(() => {
    const titles = [readerTitleRef.current, sidebarTitleRef.current].filter((node): node is HTMLElement => Boolean(node));
    const check = () => {
      if (!readerTitleExpanded) setReaderTitleOverflow(Boolean(readerTitleRef.current && readerTitleRef.current.scrollHeight > readerTitleRef.current.clientHeight + 1));
      if (!sidebarTitleExpanded) setSidebarTitleOverflow(Boolean(sidebarTitleRef.current && sidebarTitleRef.current.scrollHeight > sidebarTitleRef.current.clientHeight + 1));
    };
    check();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(check);
    titles.forEach((node) => observer?.observe(node));
    return () => observer?.disconnect();
  }, [readerTitleExpanded, sidebarTitleExpanded, outlineOpen, pageIndex, fontSize, work.title]);

  useEffect(() => {
    const backWindow = window as Window & { piepAndroidReaderLayerBack?: () => boolean };
    backWindow.piepAndroidReaderLayerBack = () => {
      if (outlineOpen) { setOutlineOpen(false); return true; }
      if (typeOpen) { setTypeOpen(false); return true; }
      return false;
    };
    return () => { delete backWindow.piepAndroidReaderLayerBack; };
  }, [outlineOpen, typeOpen]);

  useEffect(() => {
    if (outlineOpen) {
      outlineWasOpen.current = true;
      document.querySelector<HTMLElement>(".reader-outline[role='dialog'] button")?.focus();
    } else if (outlineWasOpen.current) {
      outlineWasOpen.current = false;
      outlineTriggerRef.current?.focus();
    }
  }, [outlineOpen]);

  useEffect(() => {
    if (typeOpen) {
      typeWasOpen.current = true;
      document.querySelector<HTMLElement>(".reader-type-panel[role='dialog'] button")?.focus();
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
    if (!viewport || !positionKey || loading || problem || !page || page.page !== pageIndex) return;
    let restored = false;
    let changed = false;
    let timer: number | null = null;
    let frame = 0;
    const saved = pendingRestore.current?.page === pageIndex ? pendingRestore.current : null;
    const pageCount = page.pageCount;
    let restoredTop: number | null = null;
    if (syncedPage.current === null) syncedPage.current = pageIndex;
    else if (syncedPage.current !== pageIndex) {
      syncedPage.current = pageIndex;
      queueReadingWrite(pageIndex, saved?.ratio ?? 0, pageCount);
    }
    const flush = () => {
      if (!changed || !restored) return;
      changed = false;
      const length = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
      const position: SavedPosition = {
        page: pageIndex,
        top: viewport.scrollTop,
        ratio: length > 0 ? viewport.scrollTop / length : 0,
        fontSize,
        lineHeight,
        readerWidth,
        fontFamily,
      };
      try { window.localStorage.setItem(`${positionKey}.position`, JSON.stringify(position)); } catch { /* storage unavailable */ }
      // 位置を戻しただけのスクロールは、読み進めた印ではない。
      if (restoredTop === null || Math.abs(viewport.scrollTop - restoredTop) > 2) queueReadingWrite(pageIndex, position.ratio, pageCount);
    };
    const onScroll = () => {
      if (!restored) return;
      changed = true;
      if (timer === null) timer = window.setTimeout(() => { timer = null; flush(); }, 400);
    };
    const onUserInput = () => {
      if (!saved || assetsReady) return;
      pendingRestore.current = null;
      restored = true;
    };
    viewport.addEventListener("scroll", onScroll, { passive: true });
    viewport.addEventListener("touchstart", onUserInput, { passive: true });
    viewport.addEventListener("wheel", onUserInput, { passive: true });
    viewport.addEventListener("pointerdown", onUserInput, { passive: true });
    if (saved) {
      const sameType = saved.fontSize === fontSize && saved.lineHeight === lineHeight && saved.readerWidth === readerWidth && saved.fontFamily === fontFamily;
      const deadline = performance.now() + 2000;
      const restore = () => {
        const length = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
        const target = sameType ? Math.min(saved.top, length) : saved.ratio * length;
        viewport.scrollTop = target;
        if (Math.abs(viewport.scrollTop - target) < 2 || performance.now() >= deadline) {
          restored = true;
          restoredTop = viewport.scrollTop;
          pendingRestore.current = null;
        } else frame = requestAnimationFrame(restore);
      };
      if (assetsReady) frame = requestAnimationFrame(restore);
    } else {
      pendingRestore.current = null;
      restored = true;
    }
    return () => {
      viewport.removeEventListener("scroll", onScroll);
      viewport.removeEventListener("touchstart", onUserInput);
      viewport.removeEventListener("wheel", onUserInput);
      viewport.removeEventListener("pointerdown", onUserInput);
      cancelAnimationFrame(frame);
      if (timer !== null) window.clearTimeout(timer);
      flush();
    };
  }, [positionKey, pageIndex, page, html, loading, problem, assetsReady, fontSize, lineHeight, readerWidth, fontFamily]);

  // 上の片付けが最後の位置を積むので、この片付けはその後に走らせる。
  useEffect(() => () => sendQueuedWrite(), []);

  useEffect(() => {
    const target = pendingOutline.current;
    const viewport = scrollRef.current;
    if (!target || !viewport || outlineOpen || loading || !assetsReady || page?.page !== target.page) return;
    const heading = viewport.querySelectorAll<HTMLElement>(".online-document h2")[target.index];
    pendingOutline.current = null;
    if (!heading) return;
    const offset = heading.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
    viewport.scrollTop += offset - 16;
  }, [page, html, pageIndex, outlineOpen, loading, assetsReady]);

  useEffect(() => {
    if (!offline && activeEditHash === undefined) return;
    if (offline && !offlineGeneration) return;
    let alive = true;
    void (async () => {
      try {
        if (!offline) {
          const before = await getPcMetadata(work.id);
          if (!alive || adoptRevision(before)) return;
        }
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
  }, [offline, work.id, positionKey, outlineOpen, offlineGeneration]);

  useEffect(() => {
    if (!outlineOpen || offline) return;
    let alive = true;
    void getPcWorkCollections(work.id).then((memberships) => { if (alive) setCollections(memberships); }).catch(() => {});
    return () => { alive = false; };
  }, [outlineOpen, offline, work.id]);

  useEffect(() => {
    if (activeEditHash === undefined) return;
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
        if (!offline) {
          const before = await getPcMetadata(work.id);
          if (cancelled || adoptRevision(before)) return;
        }
        const next = await (offline ? getOfflinePage(work.id, pageIndex) : getPcPage(work.id, pageIndex));
        if (cancelled) return;
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
        if (!offline) {
          const after = await getPcMetadata(work.id);
          if (cancelled || adoptRevision(after)) return;
        }
        setPage(next);
        setJumpPage(next.page + 1);
        if (next.page !== pageIndex) setPageIndex(next.page);
        const clean = safePageHtml(next.html);
        setHtml(clean);
        setLoading(false);
        // Let React mount the sanitized document before filling its images.
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        if (cancelled) return;
        const viewport = scrollRef.current;
        const images = [...(viewport?.querySelectorAll<HTMLImageElement>(".online-document img") ?? [])];
        setImageResolutionDone(true);
        if (!images.length) return;
        const uniqueIds = [...new Set(images.map((image) => Number(image.dataset.assetId)).filter((id) => Number.isSafeInteger(id) && id > 0))];
        for (const image of images) {
          const id = Number(image.dataset.assetId);
          if (Number.isSafeInteger(id) && id > 0) continue;
          const message = document.createElement("span");
          message.className = "reader-missing-image";
          message.setAttribute("role", "alert");
          message.textContent = `画像を読み込めません（${image.alt || "本文画像"}）`;
          image.replaceWith(message);
        }
        let nextAsset = 0;
        let snapshotChanged = false;
        const fetchAsset = async () => {
          while (!cancelled && !snapshotChanged && nextAsset < uniqueIds.length) {
            const assetId = uniqueIds[nextAsset++];
            let source: string | null = null;
            try {
              const asset = await (offline ? getOfflineAsset(work.id, assetId, pageGeneration!) : getPcAsset(work.id, assetId));
              source = `data:${asset.mimeType};base64,${asset.base64}`;
            } catch (error) {
              if (offline && offlineSnapshotChanged(error)) { snapshotChanged = true; return; }
              // Other image failures leave a visible placeholder.
            }
            if (cancelled || snapshotChanged) return;
            for (const image of viewport?.querySelectorAll<HTMLImageElement>(`.online-document img[data-asset-id="${assetId}"]`) ?? []) {
              if (source) {
                image.src = source;
                image.removeAttribute("data-asset-id");
              } else {
                const message = document.createElement("span");
                message.className = "reader-missing-image";
                message.setAttribute("role", "alert");
                message.textContent = `画像を読み込めません（${image.alt || "本文画像"}）`;
                image.replaceWith(message);
              }
            }
            viewport?.dispatchEvent(new Event("piep-asset-change"));
          }
        };
        // Fill each illustration as it arrives while four requests share the connection.
        await Promise.all(Array.from({ length: Math.min(4, uniqueIds.length) }, fetchAsset));
        if (cancelled) return;
        if (snapshotChanged) {
          setPage(null);
          setHtml("");
          setOfflineGeneration(null);
          setImageResolutionDone(false);
          setAssetsReady(false);
          setLoading(true);
          setRetry((current) => current + 1);
          return;
        }
        if (!cancelled && !offline) {
          const afterAssets = await getPcMetadata(work.id);
          if (cancelled || adoptRevision(afterAssets)) return;
        }
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
  }, [work.id, pageIndex, retry, offline, positionKey]);

  const openContentLink = (event: React.MouseEvent<HTMLDivElement>) => {
    const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
    if (!anchor || !event.currentTarget.contains(anchor)) return;
    event.preventDefault();
    const sourcePage = Number(anchor.dataset.page);
    if (anchor.classList.contains("jump-link") && Number.isSafeInteger(sourcePage) && sourcePage >= 1) {
      const target = page?.sourcePageStarts[sourcePage - 1];
      if (target !== undefined) setPageIndex(target);
      return;
    }
    const href = anchor.getAttribute("href");
    if (href && /^https?:\/\//i.test(href)) {
      void openUrl(href).catch(() => setSaveMessage("リンクを開けませんでした"));
    }
  };

  const jumpTo = (index: number) => {
    if (!page || !Number.isSafeInteger(index)) return;
    setPageIndex(Math.min(Math.max(index, 0), page.pageCount - 1));
    setOutlineOpen(false);
  };

  const jumpToOutline = (entry: PcOutlineEntry) => {
    // 目次の page/index は転送後の読書ページと、そのページ内の h2 の順番。
    pendingRestore.current = null;
    pendingOutline.current = { page: entry.page - 1, index: entry.index };
    setPageIndex(entry.page - 1);
    setOutlineOpen(false);
  };

  const sidebarWork = detailWork ?? work;
  const description = sidebarWork.excerpt ? descriptionText(sidebarWork.excerpt) : "";
  const date = displayDate(sidebarWork.sourceCreatedAt ?? sidebarWork.downloadedAt);

  const finishHorizontalSwipe = (event: React.TouchEvent<HTMLElement>) => {
    const start = touchStart.current;
    touchStart.current = null;
    const viewport = scrollRef.current;
    if (!start || !viewport || !page || loading || event.changedTouches.length !== 1) return;
    const dx = event.changedTouches[0].clientX - start.x;
    const dy = event.changedTouches[0].clientY - start.y;
    if (Math.abs(dx) < 85 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
    const direction = dx < 0 ? 1 : -1;
    const end = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
    // A transfer page can contain many screens of text. A swipe advances one
    // screen until its edge; only then does it switch transfer pages.
    if ((direction > 0 && viewport.scrollTop < end - 2) || (direction < 0 && viewport.scrollTop > 2)) {
      viewport.scrollBy({ top: direction * Math.max(1, viewport.clientHeight - 48), behavior: "smooth" });
      return;
    }
    const next = pageIndex + direction;
    if (next >= 0 && next < page.pageCount) setPageIndex(next);
  };

  return <div className={`reader-screen reader-theme--${readerTheme}`}>
    <header className="reader-toolbar"><button className="text-button back-button" onClick={onBack} aria-label={backLabel}>← <span>戻る</span></button><span className="reader-toolbar-title">{work.title}</span><div className="reader-toolbar-actions"><button ref={outlineTriggerRef} className="text-button" onClick={() => setOutlineOpen(true)}>{outline.length > 0 ? "目次・情報" : "作品情報"}</button><button ref={typeTriggerRef} className="text-button reader-type-button" onClick={() => setTypeOpen(true)} aria-label="文字の表示設定">文字</button></div></header>
    {saveMessage && <div className="reader-notice" role="status">{saveMessage}</div>}
    {remoteOffer && <div className="reader-notice reader-sync-offer" role="status"><span>{remoteOffer.guide ? "別の版を" : "別の端末で"} {remoteOffer.page + 1} ページまで読んでいます</span><button onClick={acceptRemote}>そこへ移動</button><button onClick={dismissRemote}>閉じる</button></div>}
    {typeOpen && <div className="reader-type-layer">
      <button className="reader-type-scrim" onClick={() => setTypeOpen(false)} aria-label="表示設定を閉じる" />
      <div className="reader-type-panel" role="dialog" aria-modal="true" aria-label="文字の表示設定" onClickCapture={(event) => { if ((event.target as Element).closest(".reader-type-row button")) capturePosition(); }}>
        <div className="reader-type-heading"><strong>文字の表示</strong><button onClick={() => setTypeOpen(false)} aria-label="閉じる">×</button></div>
        <div className="reader-type-row"><span>文字サイズ</span><div><button disabled={fontSize <= 14} onClick={() => onFontSizeChange(fontSize - 1)} aria-label="文字を小さくする">−</button><strong>{fontSize}px</strong><button disabled={fontSize >= 28} onClick={() => onFontSizeChange(fontSize + 1)} aria-label="文字を大きくする">＋</button></div></div>
        <div className="reader-type-row"><span>行間</span><div><button disabled={lineHeight <= 1.4} onClick={() => onLineHeightChange(Math.max(1.4, Math.round((lineHeight - .1) * 100) / 100))} aria-label="行間を狭くする">−</button><strong>{lineHeight.toFixed(2)}</strong><button disabled={lineHeight >= 2.4} onClick={() => onLineHeightChange(Math.min(2.4, Math.round((lineHeight + .1) * 100) / 100))} aria-label="行間を広くする">＋</button></div></div>
        <div className="reader-type-row"><span>本文幅</span><div><button disabled={readerWidth <= 520} onClick={() => onReaderWidthChange(Math.max(520, readerWidth - 20))} aria-label="本文幅を狭くする">−</button><strong>{readerWidth}px</strong><button disabled={readerWidth >= 900} onClick={() => onReaderWidthChange(Math.min(900, readerWidth + 20))} aria-label="本文幅を広くする">＋</button></div></div>
        <p className="reader-type-help">広い画面では本文の左右余白を調整します。</p>
        <div className="reader-type-row reader-type-choice-row"><span>書体</span><div role="group" aria-label="書体">
          <button type="button" aria-pressed={fontFamily === "serif"} onClick={() => onFontFamilyChange("serif")}>明朝</button>
          <button type="button" aria-pressed={fontFamily === "sans"} onClick={() => onFontFamilyChange("sans")}>ゴシック</button>
        </div></div>
        <div className="reader-type-row reader-type-choice-row"><span>読書背景</span><div role="group" aria-label="読書背景">
          <button type="button" aria-pressed={readerTheme === "white"} onClick={() => onReaderThemeChange("white")}>白</button>
          <button type="button" aria-pressed={readerTheme === "paper"} onClick={() => onReaderThemeChange("paper")}>紙の色</button>
          <button type="button" aria-pressed={readerTheme === "night"} onClick={() => onReaderThemeChange("night")}>ナイト</button>
        </div></div>
      </div>
    </div>}
    <main className="reader-scroll" ref={scrollRef} key={pageIndex} tabIndex={0} aria-label="本文。Page Up、Page Down、左右矢印、Spaceで移動" onKeyDown={(event) => {
      if (loading || problem || outlineOpen || typeOpen || !page) return;
      handleReaderKeyboard(event, pageIndex, page.pageCount, (next, edge) => {
        keyboardPageChange.current = true;
        if (edge === "end") pendingRestore.current = {
          page: next,
          top: Number.MAX_SAFE_INTEGER,
          ratio: 1,
          fontSize,
          lineHeight,
          readerWidth,
          fontFamily,
        };
        setPageIndex(next);
      });
    }} onTouchStart={(event) => { if (event.touches.length === 1) touchStart.current = { x: event.touches[0].clientX, y: event.touches[0].clientY }; }} onTouchEnd={finishHorizontalSwipe}><article className={`reading-paper online-paper reader-font--${fontFamily}`} style={{ maxWidth: `${readerWidth}px`, fontSize: `${fontSize}px`, lineHeight }}>
      {pageIndex === 0 && <><h1 ref={readerTitleRef} className={readerTitleExpanded ? "reader-work-title expanded" : "reader-work-title"}>{work.title}</h1>{readerTitleOverflow && <button className="reader-title-toggle" onClick={() => setReaderTitleExpanded((expanded) => !expanded)} aria-expanded={readerTitleExpanded}>{readerTitleExpanded ? "タイトルを短く表示" : "タイトルをすべて表示"}</button>}<div className="reader-rule" /></>}
      {loading ? <p>本文を読み込んでいます…</p> : problem ? <div className="reader-error" role="alert"><p>{problem}</p><button onClick={() => setRetry((current) => current + 1)}>もう一度読み込む</button></div> : <div className="online-document" onClick={openContentLink} dangerouslySetInnerHTML={{ __html: html }} />}
    </article></main>
    {page && page.pageCount > 1 && <footer className="reader-footer"><button onClick={() => setPageIndex((current) => Math.max(0, current - 1))} disabled={loading || pageIndex === 0}>前のページ</button><span aria-live="polite">{`${page.page + 1} / ${page.pageCount}`}</span><button onClick={() => setPageIndex((current) => current + 1)} disabled={loading || pageIndex >= page.pageCount - 1}>次のページ</button></footer>}
    {outlineOpen && <div className="reader-outline-layer">
      <button className="reader-outline-scrim" onClick={() => setOutlineOpen(false)} aria-label="目次と情報を閉じる" />
      <aside className="reader-outline" role="dialog" aria-modal="true" aria-label="目次と作品情報">
        <div className="reader-outline-heading"><h2>{outline.length > 0 ? "目次・作品情報" : "作品情報"}</h2><button onClick={() => setOutlineOpen(false)} aria-label="閉じる">×</button></div>
        <div className="reader-outline-work"><span className="reader-outline-cover"><WorkThumbnail work={sidebarWork} offline={offline} snapshotGeneration={snapshotGeneration} /></span><div className="reader-outline-work-text"><div className="reader-outline-labels"><ProviderMark source={sidebarWork.source} compact /><span>{sidebarWork.contentType === "novel" ? "小説" : "作品"}</span>{isEdited && <span>編集版</span>}</div><strong ref={sidebarTitleRef} className={sidebarTitleExpanded ? "expanded" : ""}>{sidebarWork.title}</strong>{sidebarTitleOverflow && <button className="reader-outline-title-toggle" onClick={() => setSidebarTitleExpanded((expanded) => !expanded)} aria-expanded={sidebarTitleExpanded}>{sidebarTitleExpanded ? "タイトルを短く表示" : "タイトルをすべて表示"}</button>}<span className="reader-outline-author"><AuthorAvatar work={sidebarWork} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} />{sidebarWork.authorName}</span>{sidebarWork.seriesTitle && <small>{sidebarWork.seriesTitle}</small>}</div></div>
        {!offline && <button className="reader-outline-save" disabled={saving} onClick={() => void save()}>{saving ? "保存中…" : "端末に保存"}</button>}
        {saveMessage && <p className="reader-outline-save-status" role="status">{saveMessage}</p>}
        {page && page.pageCount > 1 && <div className="reader-outline-progress"><div><span>{page.page + 1} / {page.pageCount} ページ</span><span>{Math.round((page.page + 1) / page.pageCount * 100)}%</span></div><progress max={100} value={(page.page + 1) / page.pageCount * 100} aria-label="ページ位置" /></div>}
        {page && page.pageCount > 1 && <div className="reader-jump" aria-label="ページ移動"><div className="reader-jump-heading"><strong>ページを移動</strong><span>{jumpPage} / {page.pageCount}</span></div><input type="range" min="1" max={page.pageCount} step="1" value={jumpPage} aria-label="移動先のページ" onChange={(event) => setJumpPage(Number(event.target.value))} /><div className="reader-jump-actions"><button type="button" disabled={page.page === 0} onClick={() => jumpTo(page.page - 1)}>前のページ</button><button type="button" className="reader-jump-go" disabled={jumpPage === page.page + 1} onClick={() => jumpTo(jumpPage - 1)}>選んだページを開く</button><button type="button" disabled={page.page >= page.pageCount - 1} onClick={() => jumpTo(page.page + 1)}>次のページ</button></div></div>}
        {outline.length > 0 && <div className="reader-outline-list"><h3>見出し</h3>{outline.map((entry) => <button key={`${entry.page}:${entry.index}`} onClick={() => jumpToOutline(entry)}><strong>{entry.title}</strong><small>{entry.page} ページ</small></button>)}</div>}
        <section className="reader-outline-details" aria-label="作品情報"><div className="reader-outline-facts">{date && <span>公開 {date}</span>}<span>{sidebarWork.textLength.toLocaleString("ja-JP")} 字</span><span>{sidebarWork.assetCount.toLocaleString("ja-JP")} 画像</span>{sidebarWork.favorite && <span>お気に入り</span>}</div>{description && <p className="reader-outline-description">{description}</p>}{sidebarWork.tags.length > 0 && <div className="reader-outline-tags" aria-label="タグ">{sidebarWork.tags.map((tag, index) => <span key={`${tag}:${index}`}>#{tag}</span>)}</div>}{collections && collections.length > 0 && <div className="reader-outline-collections"><strong>コレクション</strong>{collections.map((collection) => <span key={collection.id}>{collection.name}</span>)}</div>}</section>
      </aside>
    </div>}
  </div>;
}
