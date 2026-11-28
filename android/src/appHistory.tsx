import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { defaultPcSearch, type OfflineWork, type PcSearchCriteria, type PcWork } from "./pcApi";
import type { SelectedWork } from "./WorkDetail";

export type LibraryTab = "works" | "people" | "series" | "collections";
export type EntityTab = "works" | "series" | "collections";
export type EntityKind = "person" | "series";

export type LibraryPage = {
  name: "library";
  tab: LibraryTab;
  criteria: PcSearchCriteria;
  /** 作者・シリーズ・コレクションの一覧それぞれの検索語。 */
  queries: { people: string; series: string; collections: string; members: string };
  listPage: number;
  collectionId: string | null;
  /** 読みかけは検索条件ではなく、この端末の読書記録から並べる。 */
  shelf: "reading" | "finished" | null;
};

export type EntityPageState = {
  name: "entity";
  kind: EntityKind;
  source: string;
  key: string;
  displayName: string;
  tab: EntityTab;
  query: string;
  tags: string[];
  seriesQuery: string;
  listPage: number;
  /** 絞り込み（取得元、種類、文字数など）。検索語・含めるタグ・並び順はほかの欄で持つ。 */
  filters?: PcSearchCriteria;
};

export type OfflineTab = "works" | "people" | "series" | "collections";
export type OfflinePageState = { name: "offline"; tab: OfflineTab; groupId: string | null };
export type AppPage = LibraryPage | EntityPageState | OfflinePageState | { name: "settings" };
export type ReaderOverlay = { kind: "online"; work: PcWork; version?: number } | { kind: "offline"; work: OfflineWork; page?: number } | { kind: "preview" };
export type AppEntry = { id: number; page: AppPage; detail: SelectedWork | null; reader: ReaderOverlay | null };
export type EntryPatch = Partial<Omit<AppEntry, "id">>;

export const homePage: LibraryPage = { name: "library", tab: "works", criteria: defaultPcSearch, queries: { people: "", series: "", collections: "", members: "" }, listPage: 1, collectionId: null, shelf: null };

export function entityPage(kind: EntityKind, source: string, key: string, displayName: string): EntityPageState {
  return { name: "entity", kind, source, key, displayName, tab: "works", query: "", tags: [], seriesQuery: "", listPage: 1 };
}

/** 別の場所かどうか。同じ場所の絞り込みや上に重ねた面では、スクロール位置を動かさない。 */
function placeOf(page: AppPage): string {
  if (page.name === "library") return `library:${page.shelf ?? ""}:${page.tab}:${page.collectionId ?? ""}:${page.listPage}:${JSON.stringify(page.criteria)}:${JSON.stringify(page.queries)}`;
  if (page.name === "offline") return `offline:${page.tab}:${page.groupId ?? ""}`;
  if (page.name === "entity") return `entity:${page.kind}:${page.source}:${page.key}:${page.tab}:${page.listPage}`;
  return page.name;
}

const MAX_ENTRIES = 60;
const RESTORE_LIMIT_MS = 1500;
const SAVED_PLACE_KEY = "piep.place";
const PAGE_NAMES = new Set(["library", "entity", "offline", "settings"]);

type SavedPlace = { stack: AppEntry[]; positions: [number, number][] };

/**
 * OS がアプリを終わらせたあとの起動か。Android は、利用者が閉じたあとは最初の画面から、
 * OS が裏で終わらせたあとは元の画面から始めることを求める。その区別は Activity が受け取る
 * 保存状態の有無で分かり、ネイティブの側がそれを伝える。
 */
function launchedAfterSystemStop(): boolean {
  try {
    return (window as Window & { piepAndroidLaunch?: { restoredBySystem?: () => boolean } }).piepAndroidLaunch?.restoredBySystem?.() === true;
  } catch {
    return false;
  }
}

function isEntry(value: unknown): value is AppEntry {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<AppEntry>;
  return typeof entry.id === "number" && Number.isSafeInteger(entry.id)
    && !!entry.page && typeof entry.page === "object" && PAGE_NAMES.has((entry.page as { name?: unknown }).name as string)
    && (entry.detail === null || typeof entry.detail === "object")
    && (entry.reader === null || typeof entry.reader === "object");
}

/** 書き留めた場所を読む。OS に終わらされたあとの起動でなければ捨てる。 */
function readSavedPlace(): SavedPlace | null {
  const restore = launchedAfterSystemStop();
  try {
    const raw: unknown = restore ? JSON.parse(window.localStorage.getItem(SAVED_PLACE_KEY) ?? "null") : null;
    if (!restore) window.localStorage.removeItem(SAVED_PLACE_KEY);
    if (!raw || typeof raw !== "object") return null;
    const { stack, positions } = raw as Partial<SavedPlace>;
    if (!Array.isArray(stack) || !stack.length || stack.length > MAX_ENTRIES || !stack.every(isEntry)) return null;
    const kept = Array.isArray(positions)
      ? positions.filter((item): item is [number, number] => Array.isArray(item) && Number.isFinite(item[0]) && Number.isFinite(item[1]))
      : [];
    return { stack, positions: kept };
  } catch {
    return null;
  }
}

function writeSavedPlace(place: SavedPlace) {
  try { window.localStorage.setItem(SAVED_PLACE_KEY, JSON.stringify(place)); } catch { /* storage unavailable */ }
}

/**
 * 画面の履歴。デスクトップ版のルーターと同じ考えで、場所の移動を一歩として積み、
 * 絞り込みや並べ替えは同じ一歩の中で書き換える。検索欄の一打ごとに履歴が増えると、
 * 戻るが何度押しても前の場所へ帰れなくなるため。
 *
 * スクロール位置は場所ではなく履歴の一歩ごとに持つ。同じ一覧を別の経路で二度開いても、
 * それぞれ見ていた場所へ戻れる。
 */
export function useAppHistory(scroller: RefObject<HTMLElement | null>) {
  // OS に終わらされたあとの起動なら、見ていた場所とスクロール位置から始める。
  const [saved] = useState(readSavedPlace);
  const nextId = useRef(saved ? Math.max(...saved.stack.map((entry) => entry.id)) + 1 : 1);
  const [stack, setStack] = useState<AppEntry[]>(() => saved?.stack ?? [{ id: 0, page: homePage, detail: null, reader: null }]);
  const stackRef = useRef(stack);
  stackRef.current = stack;
  const positions = useRef(new Map<number, number>(saved?.positions));
  const resumeScroll = useRef(Boolean(saved));
  const pending = useRef<{ type: "push" | "pop"; from: AppEntry } | null>(null);
  const recording = useRef(true);
  const stopRestore = useRef<() => void>(() => undefined);
  const current = stack[stack.length - 1];
  const currentId = useRef(current.id);

  const remember = useCallback(() => {
    const element = scroller.current;
    if (element) positions.current.set(currentId.current, element.scrollTop);
  }, [scroller]);

  const push = useCallback((patch: EntryPatch) => {
    const top = stackRef.current[stackRef.current.length - 1];
    const next = { ...top, ...patch };
    if (JSON.stringify(next.page) === JSON.stringify(top.page) && next.detail === top.detail && next.reader === top.reader) return;
    stopRestore.current();
    remember();
    recording.current = false;
    pending.current = { type: "push", from: top };
    const entry = { ...next, id: nextId.current++ };
    const kept = [...stackRef.current, entry];
    // 古い履歴を落とすときも、起点は残す。
    if (kept.length > MAX_ENTRIES) {
      for (const dropped of kept.splice(1, kept.length - MAX_ENTRIES)) positions.current.delete(dropped.id);
    }
    stackRef.current = kept;
    setStack(kept);
  }, [remember]);

  const replace = useCallback((patch: EntryPatch) => {
    const list = stackRef.current;
    const kept = [...list.slice(0, -1), { ...list[list.length - 1], ...patch }];
    stackRef.current = kept;
    setStack(kept);
  }, []);

  /** 接続先が変わったとき。前のサーバーの作品を指す履歴を残さない。設定と端末内の画面はそのまま。 */
  const restart = useCallback(() => {
    stopRestore.current();
    positions.current.clear();
    const { page } = stackRef.current[stackRef.current.length - 1];
    const home: AppEntry = { id: nextId.current++, page: homePage, detail: null, reader: null };
    const kept = page.name === "settings" || page.name === "offline" ? [home, { id: nextId.current++, page, detail: null, reader: null }] : [home];
    stackRef.current = kept;
    setStack(kept);
  }, []);

  const back = useCallback(() => {
    const list = stackRef.current;
    if (list.length < 2) return false;
    stopRestore.current();
    recording.current = false;
    pending.current = { type: "pop", from: list[list.length - 1] };
    positions.current.delete(list[list.length - 1].id);
    const kept = list.slice(0, -1);
    stackRef.current = kept;
    setStack(kept);
    return true;
  }, []);

  useLayoutEffect(() => {
    currentId.current = current.id;
    const resuming = resumeScroll.current;
    resumeScroll.current = false;
    const move = pending.current;
    pending.current = null;
    const element = scroller.current;
    if (!element || (!move && !resuming)) { recording.current = true; return; }
    if (move && placeOf(move.from.page) === placeOf(current.page)) {
      positions.current.set(current.id, positions.current.get(move.from.id) ?? element.scrollTop);
      recording.current = true;
      return;
    }
    if (move?.type === "push") {
      element.scrollTop = 0;
      recording.current = true;
      return;
    }
    // 戻った先の行はまだ描かれていないことがある。高さが落ち着くまで合わせ続ける。
    const target = positions.current.get(current.id) ?? 0;
    const started = performance.now();
    let frame = 0;
    let settled = 0;
    let height = -1;
    const finish = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      recording.current = true;
      window.removeEventListener("touchstart", finish, true);
      window.removeEventListener("wheel", finish, true);
      stopRestore.current = () => undefined;
    };
    const step = () => {
      element.scrollTop = target;
      settled = Math.abs(element.scrollTop - target) <= 1 && element.scrollHeight === height ? settled + 1 : 0;
      height = element.scrollHeight;
      if (settled >= 4 || performance.now() - started > RESTORE_LIMIT_MS) { finish(); return; }
      frame = requestAnimationFrame(step);
    };
    // 指で動かしたら、その人の操作を優先する。
    window.addEventListener("touchstart", finish, true);
    window.addEventListener("wheel", finish, true);
    stopRestore.current = finish;
    step();
    return finish;
  }, [current, scroller]);

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const onScroll = () => { if (recording.current) remember(); };
    element.addEventListener("scroll", onScroll, { passive: true });
    return () => element.removeEventListener("scroll", onScroll);
  }, [remember, scroller]);

  // 裏へ回るときに書き留める。OS はそのあと知らせずにアプリを終わらせることがある。
  useEffect(() => {
    const save = () => writeSavedPlace({ stack: stackRef.current, positions: [...positions.current] });
    save();
    const onHide = () => {
      if (document.visibilityState !== "hidden") return;
      remember();
      save();
    };
    document.addEventListener("visibilitychange", onHide);
    return () => document.removeEventListener("visibilitychange", onHide);
  }, [stack, remember]);

  return { restart, entry: current, depth: stack.length, push, replace, back };
}

export type AppNavigation = {
  entry: AppEntry;
  depth: number;
  push: (patch: EntryPatch) => void;
  replace: (patch: EntryPatch) => void;
  back: () => boolean;
  /** 上に重ねた面を閉じて、作者やシリーズの画面へ。 */
  openEntity: (kind: EntityKind, source: string, key: string, displayName: string) => void;
  /** タグで絞った本棚へ。 */
  openTag: (tag: string) => void;
  /** サーバーの本棚へ移動できるときだけ、作者やシリーズへの導線を出す。 */
  canBrowseServer: boolean;
};

const NavigationContext = createContext<AppNavigation | null>(null);

export function NavigationProvider({ value, children }: { value: Omit<AppNavigation, "openEntity" | "openTag">; children: React.ReactNode }) {
  const { push } = value;
  const full = useMemo<AppNavigation>(() => ({
    ...value,
    openEntity: (kind, source, key, displayName) => push({ page: entityPage(kind, source, key, displayName), detail: null, reader: null }),
    openTag: (tag) => push({ page: { ...homePage, criteria: { ...defaultPcSearch, tag: [tag] } }, detail: null, reader: null }),
  }), [value, push]);
  return <NavigationContext.Provider value={full}>{children}</NavigationContext.Provider>;
}

export function useAppNavigation(): AppNavigation {
  const value = useContext(NavigationContext);
  if (!value) throw new Error("NavigationProvider is required");
  return value;
}
