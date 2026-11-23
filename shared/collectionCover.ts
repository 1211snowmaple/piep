/** 表紙の作り方。既定はメンバーの表紙を並べる `mosaic`。 */
export type CollectionCoverMode = "mosaic" | "spine" | "single" | "sigil" | "file";

/** 使える材料から表紙を選ぶ。画像のない作品も複合表紙の枠に入れる。 */
export function selectCollectionCover<T>(requested: CollectionCoverMode, tiles: readonly T[], imageAvailable: boolean) {
  // 2×2 まで。枠を増やすと一枚ずつの絵が見分けづらくなる。
  const shown = tiles.slice(0, 4);
  let mode: CollectionCoverMode = "mosaic";
  if ((requested === "file" || requested === "single") && imageAvailable) mode = requested;
  else if (requested === "sigil" || shown.length === 0) mode = "sigil";
  else if (requested === "spine" && shown.length >= 2) mode = "spine";
  return { mode, tiles: shown };
}
