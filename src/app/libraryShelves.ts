/** The shelves of the library, in the order the sidebar lists them. */
export type LibraryShelf = "all" | "favorite" | "reading" | "watched";

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
  { value: "watched", label: "更新監視", search: "watch=watched" },
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
  if (shelf === "reading") return favorite || watch ? null : "reading";
  if (favorite) return watch ? null : "favorite";
  if (watch === "watched") return "watched";
  if (!watch) return "all";
  return null;
}
