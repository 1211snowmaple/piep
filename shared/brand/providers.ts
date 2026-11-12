// 取得元（pixiv・FANBOX）の名前・色・印。デスクトップと携帯で同じものを見せる。

/**
 * A service's mark, described as data rather than as markup.
 *
 * The same mark has to be drawn by React on the author and save screens and by
 * plain DOM inside saved documents, where a link card is built by hand. Two
 * hand-written copies is how pixiv ended up as its own glyph in one place and a
 * generic arrow in the other.
 */
export type BrandGlyph =
  | { kind: "path"; d: string }
  | { kind: "word"; text: string; wide?: boolean };

export const BRAND_GLYPHS: Record<string, BrandGlyph> = {
  pixiv: { kind: "path", d: "M5.2 14.7V3.3h4.3c2.7 0 4.5 1.7 4.5 4.2s-1.8 4.2-4.5 4.2H7.8v3H5.2Zm2.6-5.3h1.5c1.3 0 2.1-.7 2.1-1.9s-.8-1.9-2.1-1.9H7.8v3.8Z" },
  fanbox: { kind: "word", text: "F", wide: true },
};

/** 取得元の名前と色。知らない取得元は `null`。 */
export const PROVIDER_BRANDS: Record<string, { label: string; color: string; softColor: string }> = {
  pixiv: { label: "pixiv", color: "#0096FA", softColor: "#E8F5FF" },
  fanbox: { label: "FANBOX", color: "#F2C624", softColor: "#FFF8D9" },
};

/** 画面に出す取得元の名前。知らない取得元は記録のまま、空なら「その他」。 */
export function providerLabel(id: string): string {
  return PROVIDER_BRANDS[id.toLowerCase()]?.label ?? (id || "その他");
}
