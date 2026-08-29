import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";
const OUT = "C:/Users/hiron/AppData/Local/Temp/claude/C--Users-hiron-piep/8254dbb9-d761-4f3f-88fe-76d875d700ab/scratchpad/";
const jpg = readFileSync("C:/Users/hiron/AppData/Roaming/com.hiron.piep/downloads/pixiv/28977373/v1/data_assets/cover/ci28977373_3b0270495282fc044c17521e0d9fa2bb_master1200.jpg");
const src = `data:image/jpeg;base64,${jpg.toString("base64")}`;
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
await page.addInitScript(() => localStorage.setItem("piep.library-view", JSON.stringify("compact")));
await page.goto("http://localhost:1420/#/library", { waitUntil: "networkidle" });
await page.waitForSelector(".work-row__cover .work-cover", { timeout: 20000 });
const info = await page.evaluate((s) => {
  const frame = document.querySelector(".work-row__cover .work-cover");
  frame.innerHTML = `<img class="m_9e117634 mantine-Image-root work-cover__image" style="--image-object-fit: contain" src="${s}">`;
  const img = frame.querySelector("img");
  return new Promise((r) => {
    const done = () => {
      const cs = getComputedStyle(img);
      const f = frame.getBoundingClientRect(), b = img.getBoundingClientRect();
      r({ objectFit: cs.objectFit, frame: `${Math.round(f.width)}x${Math.round(f.height)}`,
          frameRatio: (f.width / f.height).toFixed(4), box: `${Math.round(b.width)}x${Math.round(b.height)}`,
          natural: `${img.naturalWidth}x${img.naturalHeight}`, imgRatio: (img.naturalWidth / img.naturalHeight).toFixed(4) });
    };
    img.complete ? done() : (img.onload = done);
  });
}, src);
console.log(JSON.stringify(info, null, 1));
await page.locator(".work-row__cover .work-cover").screenshot({ path: OUT + "row-cover.png" });
await browser.close();
