// インストーラーに出す絵を、ロゴから起こす。
//
// Tauri のひな型のままだと、NSIS も WiX も**汎用の絵**を出す。アイコンは
// 作ってあるのに、入れるときの画面にはどこにも出てこない。ここで作るのは
// 次の4枚で、寸法は入れ物（NSIS / WiX）が決めている。
//
//   nsis-header.bmp    150x57    2ページ目以降の右上に出る帯
//   nsis-sidebar.bmp   164x314   最初と最後のページの左側
//   wix-banner.bmp     493x58    2ページ目以降の上の帯
//   wix-dialog.bmp     493x312   最初と最後のページの背景
//
// SVG をそのまま貼って Chromium で撮る。文字を画像に焼くのではなく、
// **アウトライン化済みのロゴをそのまま拡大縮小する**ので、どの寸法でも
// 線の太さの比が崩れない。撮ったあと BMP へ変換する（どちらの入れ物も
// BMP しか受け付けない）。
//
// 語標（`piep_lockup.svg`）だけを使う。しるしはその中に入っているので、
// 別に並べると同じ絵が二つ出る。
//
//   node tools/installer-art/render.mjs

import { chromium } from "@playwright/test";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const repo = resolve(import.meta.dirname, "..", "..");
const outDir = resolve(repo, "src-tauri", "installer");
const lockup = readFileSync(resolve(repo, "piep_lockup.svg"), "utf8");

/** アプリの色。`docs/policy/05-frontend.md` の決めごとと同じもの。 */
const INK = "#0E151D";
const PAPER = "#FFFFFF";
const BLUE = "#0096FA";

/** SVG の宣言部を落として、そのまま埋め込める形にする。 */
function inline(svg, width) {
  return svg
    .replace(/<\?xml[^>]*\?>/, "")
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/\swidth="[^"]*"/, ` width="${width}"`)
    .replace(/\sheight="[^"]*"/, "")
    .trim();
}

const FONT = `"Yu Gothic UI", "Meiryo", system-ui, sans-serif`;

const pages = [
  {
    name: "nsis-sidebar",
    width: 164,
    height: 314,
    // 縦長の板。地は濃い墨で、しるしと語標を上寄せに置く。下に一行だけ。
    body: `
      <div style="width:164px;height:314px;background:${INK};position:relative;overflow:hidden">
        <div style="position:absolute;left:-44px;bottom:-74px;width:224px;height:224px;
                    border-radius:50%;background:${BLUE};opacity:.12"></div>
        <div style="position:absolute;inset:0;display:flex;align-items:center;
                    justify-content:center;padding:0 20px 44px">
          ${inline(lockup, 124)}
        </div>
        <div style="position:absolute;left:0;right:0;bottom:20px;text-align:center;
                    font-family:${FONT};font-size:9px;line-height:1.7;color:#C2CCD6;
                    letter-spacing:.04em">
          自分のPCに、<br>丸ごと残して読む
        </div>
      </div>`,
  },
  {
    name: "nsis-header",
    width: 150,
    height: 57,
    // 右上の小さな帯。地は紙のままにして、語標だけを置く。
    body: `
      <div style="width:150px;height:57px;background:${PAPER};
                  display:flex;align-items:center;justify-content:flex-end;padding-right:12px">
        ${inline(lockup, 96)}
      </div>`,
  },
  {
    name: "wix-banner",
    width: 493,
    height: 58,
    body: `
      <div style="width:493px;height:58px;background:${PAPER};position:relative;
                  display:flex;align-items:center;padding-left:16px">
        ${inline(lockup, 104)}
        <div style="position:absolute;right:0;top:0;bottom:0;width:6px;background:${BLUE}"></div>
      </div>`,
  },
  {
    name: "wix-dialog",
    width: 493,
    height: 312,
    // 最初と最後の頁の地。**WiX はこの絵の上に文字を重ねる。** 標準の
    // WixUI では題と説明が x=135 から始まるので、濃い帯はそこへ届かせない。
    // 届かせると、白い文字ではなく黒い文字が濃い地に乗って読めなくなる。
    body: `
      <div style="width:493px;height:312px;background:${PAPER};position:relative;overflow:hidden">
        <div style="position:absolute;left:0;top:0;bottom:0;width:126px;background:${INK};
                    overflow:hidden;display:flex;align-items:center;justify-content:center;padding:0 11px">
          <div style="position:absolute;left:-74px;bottom:-92px;width:220px;height:220px;
                      border-radius:50%;background:${BLUE};opacity:.12"></div>
          <div style="position:relative">${inline(lockup, 100)}</div>
        </div>
      </div>`, 
  },
];

const browser = await chromium.launch();
const page = await browser.newPage({ deviceScaleFactor: 1 });
mkdirSync(outDir, { recursive: true });
const written = [];
for (const spec of pages) {
  await page.setViewportSize({ width: spec.width, height: spec.height });
  await page.setContent(
    `<!doctype html><meta charset="utf-8">
     <style>html,body{margin:0;padding:0}</style>
     ${spec.body}`,
    { waitUntil: "load" },
  );
  const png = resolve(outDir, `${spec.name}.png`);
  mkdirSync(dirname(png), { recursive: true });
  await page.screenshot({ path: png, clip: { x: 0, y: 0, width: spec.width, height: spec.height } });
  written.push({ png, name: spec.name, width: spec.width, height: spec.height });
}
await browser.close();

writeFileSync(resolve(outDir, "rendered.json"), `${JSON.stringify(written, null, 2)}\n`);
for (const item of written) {
  console.log(`${item.name}  ${item.width}x${item.height}  ->  ${item.png}`);
}
console.log("PNG を書き出した。BMP への変換は tools/installer-art/to-bmp.ps1 が行う。");
