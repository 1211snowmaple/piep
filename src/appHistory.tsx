import { createContext, useCallback, useContext, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
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
  shelf: "reading" | null;
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
};

export type OfflineTab = "works" | "people" | "series" | "collections";
export type OfflinePageState = { name: "offline"; tab: OfflineTab; groupId: string | null };
export type AppPage = LibraryPage | EntityPageState | OfflinePageState | { name: "settings" };
export type ReaderOverlay = { kind: "online"; work: PcWork } | { kind: "offline"; work: OfflineWork } | { kind: "preview" };
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

/**
 * 画面の履歴。デスクトップ版のルーターと同じ考えで、場所の移動を一歩として積み、
 * 絞り込みや並べ替えは同じ一歩の中で書き換える。検索欄の一打ごとに履歴が増えると、
 * 戻るが何度押しても前の場所へ帰れなくなるため。
 *
 * スクロール位置は場所ではなく履歴の一歩ごとに持つ。同じ一覧を別の経路で二度開いても、
 * それぞれ見ていた場所へ戻れる。
 */
export function useAppHistory(scroller: RefObject<HTMLElement | null>) {
  const nextId = useRef(1);
  const [stack, setStack] = useState<AppEntry[]>(() => [{ id: 0, page: homePage, detail: null, reader: null }]);
  const stackRef = useRef(stack);
  stackRef.current = stack;
  const positions = useRef(new Map<number, number>());
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
    const move = pending.current;
    pending.current = null;
    const element = scroller.current;
    if (!move || !element) { recording.current = true; return; }
    if (placeOf(move.from.page) === placeOf(current.page)) {
      positions.current.set(current.id, positions.current.get(move.from.id) ?? element.scrollTop);
      recording.current = true;
      return;
    }
    if (move.type === "push") {
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

  return { entry: current, depth: stack.length, push, replace, back };
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
    openTag: (tag) => push({ page: { ...homePage, criteria: { ...defaultPcSearch, tag } }, detail: null, reader: null }),
  }), [value, push]);
  return <NavigationContext.Provider value={full}>{children}</NavigationContext.Provider>;
}

export function useAppNavigation(): AppNavigation {
  const value = useContext(NavigationContext);
  if (!value) throw new Error("NavigationProvider is required");
  return value;
}
