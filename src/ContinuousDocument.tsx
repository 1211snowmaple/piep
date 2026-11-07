import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { fillDocumentImages } from "./readerDocument";
import type { PcPage } from "./pcApi";

export type ContinuousTarget = {
  page: number;
  /** 区切りの先頭からの位置。文字の設定が同じときだけ使う。 */
  top?: number;
  /** 読書位置の割合。別の端末から来た位置はこれで戻す。 */
  ratio?: number;
  /** 区切りの高さに対する割合。文字の設定や画面の向きが変わったときに使う。 */
  fraction?: number;
  heading?: number;
  nonce: number;
};

export type LivePosition = { page: number; top: number; ratio: number; pageCount: number };

type Section = { page: number; html: string; generation: string | null };
export type LoadedSection = { page: PcPage; html: string; generation: string | null };

// 区切りをいくつ持っていても、遠くのものだけを捨てる。短い区切りが続く作品で読み込みと破棄を繰り返さない。
const MAX_SECTIONS = 5;
const EXTEND_MARGIN = 1.5;
const EVICT_DISTANCE = 3;

const SectionBody = memo(function SectionBody({ html, onClick }: { html: string; onClick: (event: React.MouseEvent<HTMLDivElement>) => void }) {
  return <div className="online-document" onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />;
});

/**
 * ページの区切りなしで本文を一続きに見せる。
 *
 * 全ページを一度に持つと長い作品で画像の分だけ膨らむので、読んでいる場所の周りだけを
 * 読み込み、遠くなった区切りは捨てる。読書位置は区切りごとの割合で表すので、
 * ページごとの読み方と同じ形でサーバーへ送れる。
 */
export function ContinuousDocument({ viewportRef, load, imageSource, isSnapshotChange, onSnapshotChanged, start, target, typeKey, header, onInfo, onPosition, onCommit, onLink, onStartRejected, flushRef }: {
  viewportRef: RefObject<HTMLElement | null>;
  load: (page: number) => Promise<LoadedSection | null>;
  imageSource: (assetId: number, width: number, generation: string | null) => string | Promise<string>;
  isSnapshotChange: (error: unknown) => boolean;
  onSnapshotChanged: () => void;
  start: () => ContinuousTarget;
  target: ContinuousTarget | null;
  typeKey: string;
  header: ReactNode;
  onInfo: (page: PcPage, generation: string | null) => void;
  onPosition: (position: LivePosition) => void;
  onCommit: (position: LivePosition) => void;
  onLink: (event: React.MouseEvent<HTMLDivElement>) => void;
  /** 開始位置のページが今の版に無いとき true を返すと、最初から開き直す。 */
  onStartRejected: (loaded: PcPage | null) => boolean;
  flushRef: RefObject<(() => void) | null>;
}) {
  const [sections, setSections] = useState<Section[]>([]);
  const [failure, setFailure] = useState<{ page: number; message: string } | null>(null);
  const [pageCount, setPageCount] = useState<number | null>(null);
  const [seed, setSeed] = useState(0);
  const sectionsRef = useRef(sections);
  sectionsRef.current = sections;
  const pageCountRef = useRef<number | null>(null);
  const seedRef = useRef(0);
  const snapshotGeneration = useRef<string | null | undefined>(undefined);
  const inFlight = useRef(new Set<number>());
  const filled = useRef(new Set<number>());
  const nodes = useRef(new Map<number, HTMLElement>());
  const pending = useRef<ContinuousTarget | null>(null);
  // 画像が埋まるまでは、読者が触らないかぎり目標の位置へ合わせ直す。
  const settling = useRef<ContinuousTarget | null>(null);
  const anchor = useRef<{ page: number; offset: number } | null>(null);
  const live = useRef<{ page: number; fraction: number; width: number; height: number } | null>(null);
  const latest = useRef<LivePosition | null>(null);
  const restoredAt = useRef<{ page: number; offset: number } | null>(null);
  const dirty = useRef(false);
  const commitTimer = useRef<number | null>(null);
  const frame = useRef(0);
  const startRef = useRef(start);
  startRef.current = start;
  const callbacks = useRef({ load, imageSource, isSnapshotChange, onSnapshotChanged, onInfo, onPosition, onCommit, onStartRejected });
  callbacks.current = { load, imageSource, isSnapshotChange, onSnapshotChanged, onInfo, onPosition, onCommit, onStartRejected };

  const flush = useCallback(() => {
    if (commitTimer.current !== null) { window.clearTimeout(commitTimer.current); commitTimer.current = null; }
    if (dirty.current && latest.current) {
      dirty.current = false;
      callbacks.current.onCommit(latest.current);
    }
  }, []);
  flushRef.current = flush;

  const measure = useCallback(() => {
    const viewport = viewportRef.current;
    const count = pageCountRef.current;
    if (!viewport || count === null || !sectionsRef.current.length) return;
    const top = viewport.getBoundingClientRect().top;
    let current: { page: number; node: HTMLElement; rect: DOMRect } | null = null;
    for (const section of sectionsRef.current) {
      const node = nodes.current.get(section.page);
      if (!node) continue;
      const rect = node.getBoundingClientRect();
      current = { page: section.page, node, rect };
      if (rect.bottom > top + 1) break;
    }
    if (!current) return;
    const offset = Math.max(0, top - current.rect.top);
    const height = current.rect.height;
    const length = Math.max(0, height - viewport.clientHeight);
    live.current = { page: current.page, fraction: height > 0 ? offset / height : 0, width: viewport.clientWidth, height: viewport.clientHeight };
    const lastSection = sectionsRef.current[sectionsRef.current.length - 1];
    const atEnd = lastSection.page === count - 1 && viewport.scrollTop >= viewport.scrollHeight - viewport.clientHeight - 2;
    const position: LivePosition = atEnd
      ? { page: count - 1, top: offset, ratio: 1, pageCount: count }
      : { page: current.page, top: offset, ratio: length > 0 ? Math.min(1, offset / length) : 0, pageCount: count };
    latest.current = position;
    callbacks.current.onPosition(position);
    const origin = restoredAt.current;
    if (!origin || origin.page !== current.page || Math.abs(origin.offset - offset) > 2) {
      dirty.current = true;
      if (commitTimer.current === null) commitTimer.current = window.setTimeout(() => { commitTimer.current = null; flush(); }, 400);
    }
  }, [flush, viewportRef]);

  const captureAnchor = useCallback(() => {
    const viewport = viewportRef.current;
    const page = latest.current?.page;
    const node = page === undefined ? null : nodes.current.get(page);
    anchor.current = viewport && node ? { page: page!, offset: node.getBoundingClientRect().top - viewport.getBoundingClientRect().top } : null;
  }, [viewportRef]);

  const fetchPage = useCallback(async (page: number) => {
    const currentSeed = seedRef.current;
    if (inFlight.current.has(page) || sectionsRef.current.some((section) => section.page === page)) return;
    inFlight.current.add(page);
    setFailure((old) => old?.page === page ? null : old);
    try {
      const loaded = await callbacks.current.load(page);
      if (currentSeed !== seedRef.current || !loaded) return;
      if (snapshotGeneration.current !== undefined && snapshotGeneration.current !== loaded.generation) {
        seedRef.current += 1;
        sectionsRef.current = [];
        setSections([]);
        pageCountRef.current = null;
        setPageCount(null);
        callbacks.current.onSnapshotChanged();
        return;
      }
      snapshotGeneration.current = loaded.generation;
      if (!sectionsRef.current.length && callbacks.current.onStartRejected(loaded.page)) return;
      // 版が変わってページが減ると、頼んだのと別のページが返る。目標を合わせないと位置が決まらない。
      if (pending.current?.page === page && loaded.page.page !== page) {
        pending.current = { ...pending.current, page: loaded.page.page };
        if (settling.current?.page === page) settling.current = pending.current;
      }
      pageCountRef.current = loaded.page.pageCount;
      setPageCount(loaded.page.pageCount);
      callbacks.current.onInfo(loaded.page, loaded.generation);
      captureAnchor();
      setSections((old) => {
        if (old.some((section) => section.page === loaded.page.page)) return old;
        return [...old, { page: loaded.page.page, html: loaded.html, generation: loaded.generation }].sort((a, b) => a.page - b.page);
      });
    } catch (error) {
      if (currentSeed !== seedRef.current) return;
      if (!sectionsRef.current.length && callbacks.current.onStartRejected(null)) return;
      setFailure({ page, message: typeof error === "object" && error && "message" in error && typeof error.message === "string" ? error.message : "本文を読み込めませんでした" });
    } finally {
      if (currentSeed === seedRef.current) inFlight.current.delete(page);
    }
  }, [captureAnchor]);

  const applyTarget = useCallback((goal: ContinuousTarget) => {
    const viewport = viewportRef.current;
    const node = nodes.current.get(goal.page);
    if (!viewport || !node) return false;
    const base = node.getBoundingClientRect().top - viewport.getBoundingClientRect().top + viewport.scrollTop;
    const height = node.getBoundingClientRect().height;
    let offset = 0;
    if (goal.heading !== undefined) {
      const heading = node.querySelectorAll<HTMLElement>(".online-document h2")[goal.heading];
      offset = heading ? heading.getBoundingClientRect().top - node.getBoundingClientRect().top - 16 : 0;
    } else if (goal.fraction !== undefined) offset = goal.fraction * height;
    else if (goal.top !== undefined) offset = Math.min(goal.top, height);
    else if (goal.ratio !== undefined) offset = goal.ratio * Math.max(0, height - viewport.clientHeight);
    viewport.scrollTop = base + Math.max(0, offset);
    restoredAt.current = { page: goal.page, offset: Math.max(0, viewport.getBoundingClientRect().top - node.getBoundingClientRect().top) };
    return true;
  }, [viewportRef]);

  const beginAt = useCallback((goal: ContinuousTarget) => {
    flush();
    seedRef.current += 1;
    snapshotGeneration.current = undefined;
    inFlight.current.clear();
    filled.current.clear();
    nodes.current.clear();
    pending.current = goal;
    settling.current = goal;
    anchor.current = null;
    latest.current = null;
    setFailure(null);
    setSections([]);
    sectionsRef.current = [];
    setSeed(seedRef.current);
  }, [flush]);

  const extend = useCallback(() => {
    const viewport = viewportRef.current;
    const count = pageCountRef.current;
    const list = sectionsRef.current;
    if (!viewport || count === null || !list.length) return;
    const margin = viewport.clientHeight * EXTEND_MARGIN;
    const first = list[0].page;
    const last = list[list.length - 1].page;
    if (last < count - 1 && viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop < margin) void fetchPage(last + 1);
    if (first > 0 && viewport.scrollTop < margin) void fetchPage(first - 1);
    if (list.length <= MAX_SECTIONS) return;
    const far = viewport.clientHeight * EVICT_DISTANCE;
    const top = viewport.getBoundingClientRect().top;
    const firstNode = nodes.current.get(first);
    const lastNode = nodes.current.get(last);
    let drop: number | null = null;
    if (firstNode && firstNode.getBoundingClientRect().bottom < top - far) drop = first;
    else if (lastNode && lastNode.getBoundingClientRect().top > top + viewport.clientHeight + far) drop = last;
    if (drop === null) return;
    captureAnchor();
    filled.current.delete(drop);
    setSections((old) => old.filter((section) => section.page !== drop));
  }, [captureAnchor, fetchPage, viewportRef]);

  // 開いた直後に開始位置から読み始める。親は版が変わると key で作り直す。
  const startGoal = useRef<ContinuousTarget | null>(null);
  useEffect(() => {
    // 開始位置は一度だけ決める。開き直しの処理が重なっても別の位置から始めない。
    startGoal.current ??= startRef.current();
    beginAt(startGoal.current);
  }, [beginAt]);

  useEffect(() => {
    if (seed) void fetchPage(pending.current?.page ?? 0);
  }, [fetchPage, seed]);

  const lastTarget = useRef<number | null>(null);
  useEffect(() => {
    if (!target || target.nonce === lastTarget.current) return;
    lastTarget.current = target.nonce;
    if (sectionsRef.current.some((section) => section.page === target.page)) {
      settling.current = null;
      applyTarget(target);
      measure();
      extend();
    } else beginAt(target);
  }, [applyTarget, beginAt, extend, measure, target]);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport || !sections.length) return;
    if (pending.current && nodes.current.has(pending.current.page)) {
      applyTarget(pending.current);
      pending.current = null;
    } else if (anchor.current) {
      const node = nodes.current.get(anchor.current.page);
      if (node) viewport.scrollTop += node.getBoundingClientRect().top - viewport.getBoundingClientRect().top - anchor.current.offset;
    }
    anchor.current = null;
    measure();
  }, [applyTarget, measure, sections, viewportRef]);

  // 区切りが増えたら画像を埋める。先に読み込んだものから順に出す。
  useEffect(() => {
    for (const section of sections) {
      if (filled.current.has(section.page)) continue;
      const node = nodes.current.get(section.page);
      if (!node) continue;
      filled.current.add(section.page);
      const currentSeed = seedRef.current;
      const page = section.page;
      const settle = () => {
        const goal = settling.current;
        if (goal?.page === page && seedRef.current === currentSeed) applyTarget(goal);
      };
      void fillDocumentImages({
        root: node,
        scroller: viewportRef.current,
        source: (assetId, width) => callbacks.current.imageSource(assetId, width, section.generation),
        isCancelled: () => seedRef.current !== currentSeed || !node.isConnected,
        isSnapshotChange: (error) => callbacks.current.isSnapshotChange(error),
        onSnapshotChanged: () => callbacks.current.onSnapshotChanged(),
        onChange: settle,
      }).then(settle);
    }
    extend();
  }, [applyTarget, extend, sections]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const stopSettling = () => { settling.current = null; };
    const onScroll = () => {
      // 回転の直後は高さが変わっただけで、読み進めたわけではない。
      if (live.current && (live.current.width !== viewport.clientWidth || live.current.height !== viewport.clientHeight)) return;
      if (frame.current) return;
      frame.current = requestAnimationFrame(() => {
        frame.current = 0;
        measure();
        extend();
      });
    };
    const resize = new ResizeObserver(() => {
      const state = live.current;
      if (!state || (state.width === viewport.clientWidth && state.height === viewport.clientHeight)) return;
      applyTarget({ page: state.page, fraction: state.fraction, nonce: 0 });
      measure();
    });
    viewport.addEventListener("scroll", onScroll, { passive: true });
    viewport.addEventListener("touchstart", stopSettling, { passive: true });
    viewport.addEventListener("wheel", stopSettling, { passive: true });
    viewport.addEventListener("pointerdown", stopSettling, { passive: true });
    resize.observe(viewport);
    return () => {
      viewport.removeEventListener("scroll", onScroll);
      viewport.removeEventListener("touchstart", stopSettling);
      viewport.removeEventListener("wheel", stopSettling);
      viewport.removeEventListener("pointerdown", stopSettling);
      resize.disconnect();
      if (frame.current) cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [applyTarget, extend, measure, viewportRef]);

  // 文字の設定が変わったら、読んでいた場所を割合で保つ。
  const lastType = useRef(typeKey);
  useLayoutEffect(() => {
    if (lastType.current === typeKey) return;
    lastType.current = typeKey;
    const state = live.current;
    if (!state) return;
    settling.current = null;
    applyTarget({ page: state.page, fraction: state.fraction, nonce: 0 });
    measure();
  }, [applyTarget, measure, typeKey]);

  useEffect(() => () => flush(), [flush]);

  const lastLoaded = sections[sections.length - 1]?.page;
  return <>
    {sections.map((section) => <section key={section.page} data-page={section.page} ref={(node) => { if (node) nodes.current.set(section.page, node); else nodes.current.delete(section.page); }}>
      {section.page === 0 && header}
      <SectionBody html={section.html} onClick={onLink} />
    </section>)}
    {!sections.length && !failure && <p>本文を読み込んでいます…</p>}
    {failure && <div className="reader-error" role="alert"><p>{failure.message}</p><button type="button" onClick={() => void fetchPage(failure.page)}>もう一度読み込む</button></div>}
    {pageCount !== null && lastLoaded === pageCount - 1 && !failure && <p className="reader-end" aria-hidden="true">おわり</p>}
  </>;
}
