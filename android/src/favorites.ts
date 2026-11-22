import { useCallback, useEffect, useSyncExternalStore } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { pcErrorMessage, type PcWork } from "./pcApi";
import { connectedShelf, setShelfFavorite } from "./shelf";
import { useAppNavigation } from "./appHistory";
import { favoriteReadRevision, nextFavoriteRevision } from "./favoriteFreshness";

/**
 * 端末で付け外ししたお気に入り。サーバーの返事を待たずに画面へ出し、失敗したら戻す。
 *
 * 一覧・詳細・読書画面はそれぞれ別の時点の作品を持っているので、ここを通して同じ値を見る。
 * 鍵は取得元の番号。サーバーの作品番号は同じ作品でも機械ごとに違う。
 * 棚の作品は更新世代（`generation`）も覚え、次の付け外しの `expected` に使う。
 */
type Entry = { favorite: boolean; busy: boolean; revision: number; generation?: string };

const CONFLICT_NOTICE = "ほかの端末かデスクトップで先に変わっていたので、今の値に合わせました";

const entries = new Map<string, Entry>();
const listeners = new Set<() => void>();
let version = 0;
let notice: string | null = null;
let noticeTimer: number | null = null;

function emit() {
  version += 1;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** 接続先ごとの値なので、登録を変えたら前の本棚の変更を忘れる。 */
export function forgetFavoriteChanges(): void {
  entries.clear();
  notice = null;
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  noticeTimer = null;
  emit();
}

const keyOf = (work: Pick<PcWork, "source" | "sourceId">) => `${work.source}:${work.sourceId}`;

function showNotice(message: string) {
  notice = message;
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => { notice = null; noticeTimer = null; emit(); }, 3_500);
  emit();
}

/** 失敗を一言だけ知らせる。どの画面から押しても同じ場所に出す。 */
export function useFavoriteNotice(): string | null {
  useSyncExternalStore(subscribe, () => version);
  return notice;
}

/**
 * 作品のお気に入りの今の値と、押せるときの切り替え口。
 * サーバーが `favorite-write` を持ち、つながっているときだけ `onToggle` を返す。
 */
export function useFavorite(work: PcWork): { favorite: boolean; busy: boolean; onToggle?: (next: boolean) => void } {
  useSyncExternalStore(subscribe, () => version);
  const navigation = useAppNavigation();
  const queryClient = useQueryClient();
  const key = keyOf(work);
  const entry = entries.get(key);
  const remoteRevision = favoriteReadRevision(work);
  const refreshed = entry && !entry.busy && remoteRevision > entry.revision;
  const favorite = refreshed ? work.favorite : entry?.favorite ?? work.favorite;
  const busy = entry?.busy ?? false;

  const remoteGeneration = work.shelf?.favoriteGeneration;
  useEffect(() => {
    if (!refreshed || entries.get(key) !== entry) return;
    entries.set(key, { favorite: work.favorite, busy: false, revision: remoteRevision, generation: remoteGeneration });
    emit();
  }, [entry, key, refreshed, remoteRevision, remoteGeneration, work.favorite]);

  const refreshQueries = useCallback(() => {
    for (const name of ["shelf-count", "metadata", "works", "works-page", "collection"]) {
      void queryClient.invalidateQueries({ queryKey: [name] });
    }
  }, [queryClient]);

  // 棚に届いているときだけ押せる。
  const shelf = navigation.canBrowseServer ? connectedShelf() : null;
  const writable = shelf !== null && work.shelf !== undefined;
  const generation = (refreshed ? undefined : entry?.generation) ?? remoteGeneration ?? "0";

  const toggle = useCallback((next: boolean) => {
    const before = entries.get(key);
    const previous = { favorite, generation: before?.generation };
    if (before?.busy || !shelf) return;
    const pending: Entry = { favorite: next, busy: true, revision: nextFavoriteRevision(), generation: previous.generation };
    entries.set(key, pending);
    emit();
    setShelfFavorite(shelf, work.id, generation, next)
      .then((stored) => {
        // 接続変更後に届いた古いサーバーの返事は、新しい本棚へ当てない。
        if (entries.get(key) !== pending) return;
        entries.set(key, { favorite: stored.favorite, busy: false, revision: nextFavoriteRevision(), generation: stored.generation });
        emit();
        if (stored.conflict) showNotice(CONFLICT_NOTICE);
        // サイドバーの件数と、お気に入りで絞った一覧を次に開いたときに合わせる。
        refreshQueries();
      })
      .catch((error: unknown) => {
        if (entries.get(key) !== pending) return;
        entries.set(key, { favorite: previous.favorite, busy: false, revision: nextFavoriteRevision(), generation: previous.generation });
        emit();
        showNotice(pcErrorMessage(error));
        refreshQueries();
      });
  }, [favorite, generation, key, refreshQueries, shelf, work.id]);

  return { favorite, busy, onToggle: writable ? toggle : undefined };
}
