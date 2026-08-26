import { useCallback } from "react";
import { useLocalStorage } from "@mantine/hooks";

/**
 * 作品の一覧を「どう見るか」。
 *
 * これは画面ごとの設定ではなく、**利用者の好み**である。棚・作者ページ・
 * 束の中身がそれぞれ別の鍵で覚えていた頃は、同じ作品の一覧が画面によって
 * 違う顔で出ていた。作者ページに至っては覚えることすらせず、常にカードだった。
 *
 * 覚える場所を一つにして、どこで切り替えてもどこでも効くようにする。
 * → [設計原則 2](../../docs/policy/02-principles.md)
 */
export type ViewMode = "gallery" | "compact";

export function parseViewMode(value: unknown): ViewMode {
  return value === "compact" ? "compact" : "gallery";
}

export function useViewMode(): [ViewMode, (next: ViewMode) => void] {
  // 初回の描画で読む。1フレーム遅れて一覧へ変わると、仮想化した並びを
  // 読み手の目の前で組み直すことになる。
  const [stored, setStored] = useLocalStorage<unknown>({
    key: "piep.library-view",
    defaultValue: "gallery",
    getInitialValueInEffect: false,
  });
  const setView = useCallback((next: ViewMode) => setStored(next), [setStored]);
  return [parseViewMode(stored), setView];
}
