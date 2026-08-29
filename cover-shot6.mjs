import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";
const OUT = "C:/Users/hiron/AppData/Local/Temp/claude/C--Users-hiron-piep/8254dbb9-d761-4f3f-88fe-76d875d700ab/scratchpad/";
const jpg = readFileSync("C:/Users/hiron/AppData/Roaming/com.hiron.piep/downloads/pixiv/28977373/v1/data_assets/cover/ci28977373_3b0270495282fc044c17521e0d9fa2bb_master1200.jpg");
const src = `data:image/jpeg;base64,${jpg.toString("base64")}`;
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

const measure = async (url, selector, viewValue, name) => {
  if (viewValue) await page.addInitScript((v) => localStorage.setItem("piep.library-view", JSON.stringify(v)), viewValue);
  await page.goto(url, { waitUntil: "networkidle" });
  await page.waitForSelector(selector);
  const info = await page.evaluate(([sel, s]) => {
    const frame = document.querySelectorAll(sel)[0];
    frame.innerHTML = `<img class="m_9e117634 mantine-Image-root work-cover__image" style="--image-object-fit: contain" src="${s}">`;
    const img = frame.querySelector("img");
    return new Promise((r) => {
      const done = () => {
        frame.style.setProperty("--work-cover-ratio", String(img.naturalWidth / img.naturalHeight));
        requestAnimationFrame(() => {
          const f = frame.getBoundingClientRect(), b = img.getBoundingClientRect();
          const scale = Math.min(b.width / img.naturalWidth, b.height / img.naturalHeight);
          r({ frame: `${f.width.toFixed(0)}x${f.height.toFixed(0)}`, box: `${b.width.toFixed(0)}x${b.height.toFixed(0)}`,
              painted: `${(img.naturalWidth*scale).toFixed(0)}x${(img.naturalHeight*scale).toFixed(0)}`,
              fitsInFrame: b.width <= f.width + 0.5 && b.height <= f.height + 0.5 });
        });
      };
      img.complete ? done() : (img.onload = done);
    });
  }, [selector, src]);
  console.log(name, JSON.stringify(info));
  await page.locator(selector).first().screenshot({ path: OUT + `after-${name}.png` });
};

await measure("http://localhost:1420/#/works/101", ".work-hero__cover", null, "detail");
await measure("http://localhost:1420/#/library", ".work-row__cover .work-cover", "compact", "list");
await measure("http://localhost:1420/#/library", ".work-card .work-cover", "gallery", "card");
await browser.close();
