/** The shelves of the library, in the order the sidebar lists them. */
export type LibraryShelf = "all" | "favorite" | "reading" | "revised";

export interface LibraryShelfDefinition {
  value: LibraryShelf;
  label: string;
  /** The library URL this shelf corresponds to. */
  search: string;
}

export const LIBRARY_SHELVES: LibraryShelfDefinition[] = [
  { value: "all", label: "すべて", search: "" },
  { value: "favorite", label: "お気に入り", search: "favorite=1" },
  { value: "reading", label: "読みかけ", search: "shelf=reading" },
  // かつてここは「更新監視」だった。作品ごとの監視フラグを集める棚だったが、
  // 監視の単位は作者・シリーズであって作品ではないので、実際には誰も作品を
  // 立てず、常に空の棚がひとつ置かれていた。棚が答えるべき問いは
  // 「どれを取り直すべきか」である。watch=watched は絞り込みとしては残る。
  { value: "revised", label: "改稿あり", search: "revised=1" },
];

/**
 * Which shelf a library URL is showing.
 *
 * Only an otherwise unfiltered listing counts as a shelf: once a search term or
 * a filter is added the view is no longer that shelf, and highlighting it in
 * the sidebar would be a lie.
 */
export function activeShelf(pathname: string, params: URLSearchParams): LibraryShelf | null {
  if (pathname !== "/library") return null;
  if (params.get("saved")) return null;
  if (params.get("q")) return null;
  if (params.get("tab") && params.get("tab") !== "works") return null;
  const favorite = params.get("favorite") === "1";
  const watch = params.get("watch");
  const shelf = params.get("shelf");
  const revised = params.get("revised") === "1";
  if (shelf === "reading") return favorite || watch || revised ? null : "reading";
  if (revised) return favorite || watch ? null : "revised";
  if (favorite) return watch ? null : "favorite";
  if (!watch) return "all";
  return null;
}
