import { expect, it } from "vitest";
import { selectCollectionCover, type CollectionCoverMode } from "./collectionCover";

it.each<CollectionCoverMode>(["file", "single"])("指定された %s の画像を優先し、使えなければ枠か紋を出す", (mode) => {
  expect(selectCollectionCover(mode, ["先頭"], true).mode).toBe(mode);
  expect(selectCollectionCover(mode, ["先頭"], false).mode).toBe("mosaic");
  expect(selectCollectionCover(mode, [], false).mode).toBe("sigil");
});

it("背表紙は二枠から使い、複合表紙には先頭四枠までを使う", () => {
  const tiles = ["一", "二", "三", "四", "五"];
  expect(selectCollectionCover("spine", tiles, false)).toEqual({ mode: "spine", tiles: tiles.slice(0, 4) });
  expect(selectCollectionCover("spine", ["一"], false).mode).toBe("mosaic");
  expect(selectCollectionCover("mosaic", tiles, false)).toEqual({ mode: "mosaic", tiles: tiles.slice(0, 4) });
  expect(selectCollectionCover("mosaic", [], false).mode).toBe("sigil");
  expect(selectCollectionCover("sigil", tiles, false).mode).toBe("sigil");
});
