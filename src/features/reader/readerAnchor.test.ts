import { describe, expect, it, vi } from "vitest";
import { atFlowEdge, currentAnchor, flowOffset, scrollByScreen, scrollToFlowOffset } from "./readerAnchor";

/** 縦書き・横書きの箱を、寸法だけ本物らしく作る。jsdom は組版をしない。 */
function viewportWith(box: { top: number; right: number }, size: { width: number; height: number }) {
  const element = document.createElement("div");
  element.getBoundingClientRect = () => ({ ...box, left: box.right - size.width, bottom: box.top + size.height, width: size.width, height: size.height, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  Object.defineProperties(element, {
    clientHeight: { configurable: true, value: size.height },
    clientWidth: { configurable: true, value: size.width },
  });
  return element;
}

function markWith(rect: { top: number; right: number }) {
  const mark = document.createElement("p");
  mark.getBoundingClientRect = () => ({ top: rect.top, right: rect.right, left: rect.right - 10, bottom: rect.top + 10, width: 10, height: 10, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  return mark;
}

describe("読んでいた場所の目印", () => {
  it("横書きでは、窓の上端より上にある最後の目印を指す", () => {
    const viewport = viewportWith({ top: 0, right: 800 }, { width: 800, height: 600 });
    const article = document.createElement("article");
    for (const top of [-200, -100, 40, 300]) article.append(markWith({ top, right: 400 }));
    expect(currentAnchor(viewport, article, false)).toBe(1);
  });

  /**
   * 縦書きでは段落の上端がどれもほぼ同じ高さになる。上端で測っていたころは
   * 最初の目印で必ず打ち切られ、**縦書きで読んだ場所は一つも憶えられなかった**。
   */
  it("縦書きでは、窓の右端より右にある最後の目印を指す", () => {
    const viewport = viewportWith({ top: 0, right: 800 }, { width: 800, height: 600 });
    const article = document.createElement("article");
    // どの段落も上端は同じ。読み終えた側（右）から並べる。
    for (const right of [1200, 900, 700, 300]) article.append(markWith({ top: 20, right }));
    expect(currentAnchor(viewport, article, true)).toBe(1);
  });

  it("目印が一つも無ければ null（呼び手が px へ落とす）", () => {
    const viewport = viewportWith({ top: 0, right: 800 }, { width: 800, height: 600 });
    expect(currentAnchor(viewport, document.createElement("article"), true)).toBeNull();
  });
});

describe("読み進む向きの送り", () => {
  function scrollable(vertical: boolean) {
    const element = viewportWith({ top: 0, right: 800 }, { width: 800, height: 600 });
    Object.defineProperties(element, {
      scrollHeight: { configurable: true, value: vertical ? 600 : 3_000 },
      scrollWidth: { configurable: true, value: vertical ? 4_000 : 800 },
    });
    element.scrollBy = vi.fn();
    return element;
  }

  it("横書きは下へ、縦書きは左へ、画面ほぼ1枚ぶん進む", () => {
    const horizontal = scrollable(false);
    scrollByScreen(horizontal, false, 1);
    expect(horizontal.scrollBy).toHaveBeenCalledWith(expect.objectContaining({ top: 552, left: 0 }));

    const vertical = scrollable(true);
    scrollByScreen(vertical, true, 1);
    // 縦書きは右から左へ進む＝ scrollLeft が減る向き。
    expect(vertical.scrollBy).toHaveBeenCalledWith(expect.objectContaining({ top: 0, left: -752 }));
  });

  it("戻るときは向きが逆になる", () => {
    const vertical = scrollable(true);
    scrollByScreen(vertical, true, -1);
    expect(vertical.scrollBy).toHaveBeenCalledWith(expect.objectContaining({ left: 752 }));
  });

  it("端に着いたことを、向きに依らず答える", () => {
    const vertical = scrollable(true);
    vertical.scrollLeft = 4_000 - 800; // 先頭（右端）
    expect(atFlowEdge(vertical, true, -1)).toBe(true);
    expect(atFlowEdge(vertical, true, 1)).toBe(false);
    vertical.scrollLeft = 0; // 読み終わり（左端）
    expect(atFlowEdge(vertical, true, 1)).toBe(true);

    const horizontal = scrollable(false);
    horizontal.scrollTop = 0;
    expect(atFlowEdge(horizontal, false, -1)).toBe(true);
    horizontal.scrollTop = 3_000 - 600;
    expect(atFlowEdge(horizontal, false, 1)).toBe(true);
  });

  it("読み始めからの距離で寄せられる", () => {
    const vertical = scrollable(true);
    scrollToFlowOffset(vertical, true, 1_000);
    expect(flowOffset(vertical, true)).toBe(1_000);

    const horizontal = scrollable(false);
    scrollToFlowOffset(horizontal, false, 900);
    expect(horizontal.scrollTop).toBe(900);
  });
});
