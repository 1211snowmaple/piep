// エミュレーターの画面の大きさと密度を順に変え、そのつど今の画面の崩れを数える。
//
//   node tools/layout-audit.mjs emulator-5556 [大きさ:密度 ...] [--out 保存先]
//
// 大きさを省くと、スマホ幅からタブレット横向きまでの決まった組み合わせを試す。
// 見つけるもの:
//   OUT-X       画面の横からはみ出した要素（横に送る列の中は除く）
//   CLIP        はみ出しを隠す親に、ボタンや印が切られている
//   TEXT-CUT    ボタンの字が収まらず切れている（… で省く作りのものは除く）
//   INTO-SAFE   ボタンや字が、端末の下の操作領域（ジェスチャーの線・3 ボタン）に入り込んでいる
//   OVER-NAV    浮かぶボタンが下メニューに重なっている
//   COVERED     字が別の要素の下に隠れている（高さが足りずにはみ出した中身など）
// 何か見つかれば終了コード 1。--out を付けると各大きさの画面写真も残す。
// 開発用の WebView（デバッグ版 APK）でだけ動く。製品の機能には含めない。
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const device = args.shift();
if (!device) throw new Error("端末名（例: emulator-5556）を渡してください");
const outAt = args.indexOf("--out");
const out = outAt >= 0 ? args.splice(outAt, 2)[1] : null;
const specs = args.length ? args : ["1080x2400:480", "1080x2400:420", "1080x2400:360", "1600x2560:400", "1600x2560:320", "2560x1600:440", "2560x1600:400", "2560x1600:320"];
const port = 9400 + Math.floor(Math.random() * 500);

const adb = (...rest) => execFileSync("adb", ["-s", device, ...rest]).toString();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function evaluate(expression) {
  const socket = [...new Set(adb("shell", "cat", "/proc/net/unix").match(/webview_devtools_remote_\d+/g) ?? [])].pop();
  if (!socket) throw new Error("piep の WebView が見つかりません（デバッグ版を起動していますか）");
  adb("forward", `tcp:${port}`, `localabstract:${socket}`);
  const tabs = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
  const tab = tabs.find((item) => item.type === "page" && item.url.startsWith("http://tauri.localhost")) ?? tabs.find((item) => item.type === "page");
  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  const result = await new Promise((resolve) => {
    ws.onmessage = (event) => { const message = JSON.parse(event.data); if (message.id === 1) resolve(message); };
    ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
  });
  ws.close();
  return result.result?.result?.value;
}

/** WebView の中で走らせる点検。文字列にして送る。 */
function audit() {
  const W = innerWidth, H = innerHeight;
  const found = [];
  const name = (el) => {
    const cls = typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\s+/).slice(0, 2).join(".") : "";
    const label = el.getAttribute("aria-label") || (el.textContent || "").trim().slice(0, 18);
    return `${el.tagName.toLowerCase()}${cls}${label ? ` "${label}"` : ""}`;
  };
  const shown = (el) => {
    const style = getComputedStyle(el);
    if (style.visibility === "hidden" || style.display === "none" || Number(style.opacity) === 0) return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const scrollsX = (el) => { const s = getComputedStyle(el); return (s.overflowX === "auto" || s.overflowX === "scroll") && el.scrollWidth > el.clientWidth + 1; };
  const scrollsY = (el) => { const s = getComputedStyle(el); return (s.overflowY === "auto" || s.overflowY === "scroll") && el.scrollHeight > el.clientHeight + 1; };
  const within = (el, test) => { for (let p = el.parentElement; p; p = p.parentElement) if (test(p)) return true; return false; };
  const all = [...document.querySelectorAll("body *")].filter((el) => shown(el) && !el.closest("[inert],[aria-hidden=true]"));
  const controls = all.filter((el) => el.matches("button,a,input,select,[role=button],[role=slider],svg"));
  for (const el of all) {
    const r = el.getBoundingClientRect();
    if ((r.right > W + 1 || r.left < -1) && !within(el, scrollsX) && getComputedStyle(el).position !== "fixed") found.push(`OUT-X ${name(el)} l=${r.left | 0} r=${r.right | 0} W=${W}`);
  }
  for (const el of controls) {
    const r = el.getBoundingClientRect();
    for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      const s = getComputedStyle(p);
      if (s.overflowX === "visible" && s.overflowY === "visible") continue;
      const pr = p.getBoundingClientRect();
      const sideways = r.right > pr.right + 1 || r.left < pr.left - 1;
      if (scrollsX(p) || scrollsY(p)) { if (sideways && s.overflowX !== "auto" && s.overflowX !== "scroll") found.push(`CLIP ${name(el)} in ${name(p)}`); break; }
      if (sideways || r.bottom > pr.bottom + 1 || r.top < pr.top - 1) { found.push(`CLIP ${name(el)} in ${name(p)}`); break; }
    }
  }
  for (const el of all.filter((e) => e.matches("button,a"))) {
    const s = getComputedStyle(el);
    const ellipsis = s.textOverflow === "ellipsis" || el.querySelector("*") && [...el.querySelectorAll("*")].some((child) => getComputedStyle(child).textOverflow === "ellipsis");
    if (!ellipsis && el.scrollWidth > el.clientWidth + 1 && s.overflowX !== "visible") found.push(`TEXT-CUT ${name(el)} ${el.scrollWidth}>${el.clientWidth}`);
  }
  const inset = parseFloat(document.documentElement.style.getPropertyValue("--piep-android-bottom-inset")) || 0;
  if (inset > 0) {
    for (const el of all.filter((e) => e.matches("button,a,input,[role=slider],span,small,strong,svg,p,h1,h2,h3"))) {
      const r = el.getBoundingClientRect();
      if (r.bottom > H - inset + 1 && r.top < H && !within(el, scrollsY)) found.push(`INTO-SAFE ${name(el)} bottom=${r.bottom | 0} limit=${(H - inset) | 0}`);
    }
  }
  // 字が別の要素の下に隠れていないか。真ん中の点を一番上で描いているのが、その字の要素かどうかで見る。
  // スクロールして見出しやメニューの下へ流れた行と、意図して重ねる浮かぶ面は数えない。
  const floating = ".scroll-top,.app-toast,.reader-rail,.reader-toolbar,.reader-panel,.reader-toasts,.mobile-header,.bottom-nav,.mobile-sidebar-scrim,.sidebar.mobile-open,[role=dialog],.reader-sheet-layer,.filter-layer";
  const leaves = all.filter((el) => [...el.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim()));
  for (const el of leaves) {
    const r = el.getBoundingClientRect();
    const x = r.left + r.width / 2, y = r.top + r.height / 2;
    if (x < 0 || y < 0 || x > W || y > H) continue;
    let clipped = false;
    for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      const s = getComputedStyle(p);
      if (s.overflowX === "visible" && s.overflowY === "visible") continue;
      const pr = p.getBoundingClientRect();
      if (x < pr.left || x > pr.right || y < pr.top || y > pr.bottom) { clipped = true; break; }
    }
    if (clipped) continue;
    const hit = document.elementFromPoint(x, y);
    if (!hit || el.contains(hit) || hit.contains(el)) continue;
    if (hit.closest(floating) && !el.closest(floating)) continue;
    found.push(`COVERED ${name(el)} under ${name(hit)}`);
  }
  const nav = document.querySelector(".bottom-nav");
  if (nav && shown(nav) && !nav.closest("[inert]")) {
    const n = nav.getBoundingClientRect();
    const floating = all.filter((e) => getComputedStyle(e).position === "fixed" || e.matches(".scroll-top"));
    for (const f of floating) {
      const r = f.getBoundingClientRect();
      const fullScreen = r.width >= W - 1 && r.height >= H - 1;
      if (!fullScreen && !f.contains(nav) && r.bottom > n.top + 1 && r.top < n.bottom) found.push(`OVER-NAV ${name(f)} bottom=${r.bottom | 0} navTop=${n.top | 0}`);
    }
  }
  return { width: W, height: H, found: [...new Set(found)] };
}

let problems = 0;
if (out) mkdirSync(out, { recursive: true });
try {
  for (const spec of specs) {
    const [size, density] = spec.split(":");
    adb("shell", "wm", "size", size);
    if (density) adb("shell", "wm", "density", density);
    await sleep(4000);
    const result = await evaluate(`(${audit.toString()})()`);
    console.log(`== ${spec}  ${result.width}x${result.height}`);
    for (const line of result.found) console.log(`  ${line}`);
    problems += result.found.length;
    if (out) writeFileSync(join(out, `${device}-${size}-${density ?? "default"}.png`), execFileSync("adb", ["-s", device, "exec-out", "screencap", "-p"]));
  }
} finally {
  adb("shell", "wm", "size", "reset");
  adb("shell", "wm", "density", "reset");
  try { adb("forward", "--remove", `tcp:${port}`); } catch { /* 転送が無ければそのまま */ }
}
process.exitCode = problems ? 1 : 0;
