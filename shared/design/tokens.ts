// piep の見た目の土台のうち、JavaScript から要る値。
// CSS から読む同じ値は tokens.css にある。二つが食い違わないことは tokens.test.ts が見る。

/** ブランドの青。0 が最も淡い。 */
export const piep = [
  "#edf8ff",
  "#d8efff",
  "#b8e2ff",
  "#8dd1ff",
  "#59b8ff",
  "#279cff",
  "#0d86f4",
  "#0872d3",
  "#0b61ad",
  "#0c518d",
] as const;

/** ブランドの緑。0 が最も淡い。 */
export const leaf = [
  "#f4fae8",
  "#e7f3cf",
  "#d0e9a3",
  "#b5dc72",
  "#9dcf49",
  "#86b918",
  "#729f12",
  "#5e8310",
  "#506b13",
  "#455b15",
] as const;

/** 画面の書体。 */
export const FONT_FAMILY =
  '"Segoe UI", "Yu Gothic UI", Meiryo, "Hiragino Sans", system-ui, -apple-system, sans-serif';
