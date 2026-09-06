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
 *
 * ただし一つに縛り切るのもやり過ぎだった。表紙で選びたいのは棚で、束の中身は
 * 順番を追うので行のほうが読みやすい、という向き不向きが実際にある。
 * 読み込み方（`PagingScope`）と同じ二段にする - **全体の既定**があり、
 * 一覧ごとの**上書き**は持ちたいものだけが持つ。上書きを持たない一覧は
 * 既定に従うので、設定でまとめて変えれば今までどおり全部が追いついてくる。
 */
export type ViewMode = "gallery" | "compact";

/** 個別の設定。`inherit` は「全体に従う」。 */
export type ViewPreference = ViewMode | "inherit";

/**
 * 見え方を別々に覚える単位。
 *
 * 増やしたときは `VIEW_SCOPES` にも足す - 設定画面はそこから作る。
 */
export type ViewScope = "library-works" | "entity" | "collection-members";

/** 設定画面が個別に並べる一覧。名前は画面上の呼び方に合わせる。 */
export const VIEW_SCOPES: { value: ViewScope; label: string }[] = [
  { value: "library-works", label: "ライブラリ · 作品" },
  { value: "entity", label: "作者・シリーズのページ" },
  { value: "collection-members", label: "コレクションの中身" },
];

/** 全体の既定。今まで一つだけあった鍵をそのまま使う - 好みは引き継がれる。 */
const STORAGE_KEY = "piep.library-view";

/**
 * 初回の描画で読む。1フレーム遅れて一覧へ変わると、仮想化した並びを
 * 読み手の目の前で組み直すことになる。
 */
const READ_ON_FIRST_RENDER = { getInitialValueInEffect: false } as const;

export function parseViewMode(value: unknown): ViewMode {
  return value === "compact" ? "compact" : "gallery";
}

export function parseViewPreference(value: unknown): ViewPreference {
  return value === "compact" || value === "gallery" ? value : "inherit";
}

/** 全体の既定。個別に決めていない一覧は、これに従う。 */
export function useDefaultViewMode(): [ViewMode, (next: ViewMode) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({
    key: STORAGE_KEY,
    defaultValue: "gallery",
    ...READ_ON_FIRST_RENDER,
  });
  const setView = useCallback((next: ViewMode) => setStored(next), [setStored]);
  return [parseViewMode(stored), setView];
}

/** 一覧ひとつぶんの上書き。`inherit` は「全体に従う」。 */
export function useScopedViewPreference(
  scope: ViewScope,
): [ViewPreference, (next: ViewPreference) => void] {
  const [stored, setStored] = useLocalStorage<unknown>({
    key: `${STORAGE_KEY}.${scope}`,
    defaultValue: "inherit",
    ...READ_ON_FIRST_RENDER,
  });
  const setPreference = useCallback((next: ViewPreference) => setStored(next), [setStored]);
  return [parseViewPreference(stored), setPreference];
}

/**
 * その一覧で実際に使う見え方。
 *
 * **ボタンで変えると、その一覧の上書きとして残る。** 読み込み方の切り替えと
 * 同じ作法で、同じ場所に並んでいるので、片方だけ挙動が違うと説明が付かない。
 */
export function useViewMode(scope: ViewScope): [ViewMode, (next: ViewMode) => void] {
  const [fallback] = useDefaultViewMode();
  const [preference, setPreference] = useScopedViewPreference(scope);
  const setView = useCallback((next: ViewMode) => setPreference(next), [setPreference]);
  return [preference === "inherit" ? fallback : preference, setView];
}
