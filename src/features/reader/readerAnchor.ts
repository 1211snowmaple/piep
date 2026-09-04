/**
 * 読んでいた場所を、文字の大きさが変わっても指し続けるための目印。
 *
 * 位置をスクロール量(px)だけで憶えていたころは、文字サイズ・行間・本文幅を
 * 変えた瞬間に、しおりも読みかけの位置もまとめてずれた。読書設定を自由に
 * 変えられることが売りなのに、**変えると場所を失う**という作りだった。
 *
 * 本文は行の終わりごとに要素（`<br>` など）を持っているので、それを数えて
 * 「何行目にいたか」で憶える。行は組み方を変えても同じ行のままなので、
 * 文字を大きくしても同じところへ戻れる。
 */

/**
 * 行の区切りとして数える要素。どれも本文の流れの中で 1 行を占める。
 *
 * pixiv の本文は `<br>` が行の終わりを作り、FANBOX や編集した本文は段落や
 * 見出しが並ぶ。両方を拾っておけば、どの取得元でも目印が足りなくならない。
 */
const MARK_SELECTOR = "br, p, h2, h3, hr, img, figure, blockquote, li";

function marks(article: HTMLElement): HTMLElement[] {
  return [...article.querySelectorAll<HTMLElement>(MARK_SELECTOR)];
}

/**
 * いま読んでいるところの手前にある、いちばん近い目印の番号。
 *
 * 「手前」は組み方で変わる。横書きなら窓の上端より上、縦書きなら窓の右端より
 * 右 ―― 縦書きでは行が上から下へ並び、桁が右から左へ進むので、上端で測ると
 * どの段落もほぼ同じ高さになり、**最初の目印で必ず打ち切られて null になる**。
 * 目印が無ければ px へ落ちるが、縦書きの px（scrollTop）は常に 0 なので、
 * 縦書きで読んだ場所は一つも憶えられていなかった。
 */
export function currentAnchor(viewport: HTMLElement, article: HTMLElement, vertical = false): number | null {
  const found = marks(article);
  if (!found.length) return null;
  const box = viewport.getBoundingClientRect();
  let anchor: number | null = null;
  for (let index = 0; index < found.length; index += 1) {
    const rect = found[index].getBoundingClientRect();
    // まだ読んでいない側へ出たら、その手前が現在地。
    const beyond = vertical ? box.right - rect.right : rect.top - box.top;
    if (beyond > 1) break;
    anchor = index;
  }
  return anchor;
}

/**
 * 論理方向で要素まで寄せる。
 *
 * 縦書きでは block が横、inline が縦になるので、`scrollTop` を直に書く
 * やり方ではどちらか一方でしか合わない。実装の無い環境（試験用の DOM など）
 * では黙って何もしない ―― 位置合わせのために読書が止まる道理はない。
 */
export function scrollElementIntoView(element: Element, options: ScrollIntoViewOptions): boolean {
  if (typeof element.scrollIntoView !== "function") return false;
  element.scrollIntoView(options);
  return true;
}

/** 目印の番号まで送る。その番号が無ければ false を返し、呼び手が px へ落とす。 */
export function scrollToAnchor(_viewport: HTMLElement, article: HTMLElement, anchor: number): boolean {
  const found = marks(article);
  const mark = found[anchor];
  if (!mark) return false;
  return scrollElementIntoView(mark, { block: "start", inline: "start", behavior: "auto" });
}

/** 読み進んだ量。向きの取り決めに依らず、始まりからの距離で測る。 */
export function flowOffset(viewport: HTMLElement, vertical: boolean): number {
  if (!vertical) return viewport.scrollTop;
  // 縦書きは右から左へ進む。この engine では scrollLeft の最大値が本の
  // 始まりで、読み進むほど 0 に近づく（実測で確かめている）。
  return Math.max(0, viewport.scrollWidth - viewport.clientWidth - viewport.scrollLeft);
}

export function flowLength(viewport: HTMLElement, vertical: boolean): number {
  return vertical
    ? viewport.scrollWidth - viewport.clientWidth
    : viewport.scrollHeight - viewport.clientHeight;
}

/** 本文の先頭へ戻す。 */
export function scrollToStart(viewport: HTMLElement, vertical: boolean, smooth = true): void {
  viewport.scrollTo({
    top: 0,
    left: vertical ? viewport.scrollWidth : 0,
    behavior: smooth ? "smooth" : "auto",
  });
}

/** 読み始めからの距離を指定して寄せる。向きの取り決めはここだけが知っている。 */
export function scrollToFlowOffset(viewport: HTMLElement, vertical: boolean, offset: number): void {
  const total = flowLength(viewport, vertical);
  const bounded = Math.max(0, Math.min(offset, Math.max(0, total)));
  if (vertical) viewport.scrollLeft = total - bounded;
  else viewport.scrollTop = bounded;
}

/**
 * 画面 1 枚ぶん読み進める（戻る）。
 *
 * 読む道具として当たり前の操作が、この画面には無かった。矢印も Space も
 * 本文を1行も動かさず、PageDown は**転送の都合で割られた 128KB のページ**を
 * まるごと飛ばしていた。押した人は、読んでいない十数画面を跳び越していた。
 */
export function scrollByScreen(viewport: HTMLElement, vertical: boolean, direction: 1 | -1): void {
  // 読んでいた行を見失わないよう、少しだけ重ねて送る。
  const overlap = 48;
  const step = Math.max(80, (vertical ? viewport.clientWidth : viewport.clientHeight) - overlap);
  viewport.scrollBy({
    top: vertical ? 0 : step * direction,
    // 縦書きは右から左へ進む。進む向きは scrollLeft が減る向き。
    left: vertical ? -step * direction : 0,
    behavior: "auto",
  });
}

/** これ以上は同じページに読むところが無い、という端。 */
export function atFlowEdge(viewport: HTMLElement, vertical: boolean, direction: 1 | -1): boolean {
  const offset = flowOffset(viewport, vertical);
  const total = flowLength(viewport, vertical);
  // 2px は端末ごとの端数。ここを 0 にすると最後の一押しが効かない。
  return direction === 1 ? offset >= total - 2 : offset <= 2;
}
