import { useCallback, useSyncExternalStore } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { pcErrorMessage, setPcFavorite, type PcWork } from "./pcApi";
import { useAppNavigation } from "./appHistory";

/**
 * 端末で付け外ししたお気に入り。サーバーの返事を待たずに画面へ出し、失敗したら戻す。
 *
 * 一覧・詳細・読書画面はそれぞれ別の時点の作品を持っているので、ここを通して同じ値を見る。
 * 鍵は取得元の番号。サーバーの作品番号は同じ作品でも機械ごとに違う。
 */
type Entry = { favorite: boolean; busy: boolean };

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
  const favorite = entry?.favorite ?? work.favorite;
  const busy = entry?.busy ?? false;

  const toggle = useCallback((next: boolean) => {
    const before = entries.get(key)?.favorite ?? work.favorite;
    if (entries.get(key)?.busy) return;
    const pending: Entry = { favorite: next, busy: true };
    entries.set(key, pending);
    emit();
    setPcFavorite(work.source, work.sourceId, next)
      .then((stored) => {
        // 接続変更後に届いた古いサーバーの返事は、新しい本棚へ当てない。
        if (entries.get(key) !== pending) return;
        entries.set(key, { favorite: stored.favorite, busy: false });
        emit();
        // サイドバーの件数と、お気に入りで絞った一覧を次に開いたときに合わせる。
        void queryClient.invalidateQueries({ queryKey: ["shelf-count"] });
        void queryClient.invalidateQueries({ queryKey: ["metadata"] });
      })
      .catch((error: unknown) => {
        if (entries.get(key) !== pending) return;
        entries.set(key, { favorite: before, busy: false });
        emit();
        showNotice(pcErrorMessage(error));
      });
  }, [key, queryClient, work.favorite, work.source, work.sourceId]);

  return { favorite, busy, onToggle: navigation.canWriteFavorite ? toggle : undefined };
}
