import { expect, it } from "vitest";
import { fillDocumentImages } from "./readerDocument";

it("画像の番号 0 は読み込み、空や数でない番号は読めない印にする", async () => {
  const root = document.body.appendChild(document.createElement("div"));
  root.innerHTML = `<div class="online-document"><img data-asset-id="0" alt="a"><img data-asset-id="2" alt="b"><img data-asset-id="" alt="c"><img data-asset-id="x" alt="d"><img alt="e"></div>`;
  const asked: number[] = [];
  await fillDocumentImages({
    root,
    scroller: null,
    source: (assetId) => { asked.push(assetId); return `https://example.test/${assetId}`; },
    isCancelled: () => false,
    isSnapshotChange: () => false,
    onSnapshotChanged: () => {},
  });
  expect(asked).toEqual([0, 2]);
  expect([...root.querySelectorAll("img")].map((image) => image.getAttribute("src"))).toEqual(["https://example.test/0", "https://example.test/2"]);
  expect(root.querySelectorAll(".reader-missing-image")).toHaveLength(3);
  root.remove();
});
