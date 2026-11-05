import DOMPurify from "dompurify";

function safeHtml(source: string): string {
  return DOMPurify.sanitize(source, {
    ALLOWED_TAGS: ["p", "br", "h1", "h2", "h3", "ruby", "rt", "rp", "span", "strong", "em", "b", "i", "u", "s", "blockquote", "figure", "figcaption", "img", "hr", "a", "div", "ul", "ol", "li", "code", "pre"],
    ALLOWED_ATTR: ["class", "alt", "data-asset-id", "data-page", "href", "width", "height"],
    FORBID_TAGS: ["script", "style", "iframe", "object", "embed", "form", "svg", "math", "template"],
  });
}

export function safePageHtml(source: string): string {
  const holder = document.createElement("div");
  holder.innerHTML = safeHtml(source);
  for (const anchor of holder.querySelectorAll<HTMLAnchorElement>("a")) {
    const page = anchor.dataset.page;
    if (anchor.classList.contains("jump-link") && page && /^\d+$/.test(page)) {
      anchor.setAttribute("href", "#");
      continue;
    }
    const href = anchor.getAttribute("href");
    if (!href || !/^https?:\/\//i.test(href)) anchor.removeAttribute("href");
    anchor.removeAttribute("data-page");
  }
  return holder.innerHTML;
}

function missingImage(image: HTMLImageElement): void {
  const message = document.createElement("span");
  message.className = "reader-missing-image";
  message.setAttribute("role", "alert");
  message.textContent = `画像を読み込めません（${image.alt || "本文画像"}）`;
  image.replaceWith(message);
}

// サーバーが変換結果を使い回す幅と同じ段。
const illustrationWidths = [720, 1080, 1600, 2160];
const unsizedWorkers = 4;
// 一画面と少し先まで近づいたら取り始める。指で弾いても白い枠を見ないで済む距離。
const nearViewport = "150% 0px";

function illustrationWidth(root: HTMLElement): number {
  // 余白を除いた本文の幅。縦の画面でも 1 段上の大きさを取りに行かない。
  const column = root.querySelector<HTMLElement>(".online-document")?.clientWidth || root.clientWidth;
  const wanted = column * window.devicePixelRatio;
  return illustrationWidths.find((width) => width >= wanted) ?? illustrationWidths[illustrationWidths.length - 1];
}

/** 届く前から枠の高さを決める。本文が先に出ても、読む位置がずれない。 */
function reserveSpace(image: HTMLImageElement): boolean {
  const width = Number(image.getAttribute("width"));
  const height = Number(image.getAttribute("height"));
  if (!(width > 0 && height > 0)) {
    image.removeAttribute("width");
    image.removeAttribute("height");
    return false;
  }
  image.style.width = `${width}px`;
  image.style.aspectRatio = `${width} / ${height}`;
  return true;
}

/**
 * 本文の挿絵を埋める。本文の表示は待たせない。
 *
 * 縦横の分かる絵は枠だけ先に取り、画面に近づいてから取りに行く。ページの全部を
 * 一度に取ると、見ていない絵の通信が本文や次のページの要求まで詰まらせる。
 * 縦横を返さない古いサーバーでは位置が決まらないので、今までどおり順に取りに行く。
 */
export async function fillDocumentImages({ root, scroller, source, isCancelled, isSnapshotChange, onSnapshotChanged, onChange }: {
  root: HTMLElement;
  scroller: HTMLElement | null;
  source: (assetId: number, width: number) => string | Promise<string>;
  isCancelled: () => boolean;
  isSnapshotChange: (error: unknown) => boolean;
  onSnapshotChanged: () => void;
  onChange?: () => void;
}): Promise<void> {
  const images = [...root.querySelectorAll<HTMLImageElement>(".online-document img")];
  if (!images.length) return;
  const width = illustrationWidth(root);
  const load = async (image: HTMLImageElement) => {
    let src: string;
    try {
      src = await source(Number(image.dataset.assetId), width);
    } catch (error) {
      if (isCancelled()) return;
      if (isSnapshotChange(error)) { onSnapshotChanged(); return; }
      missingImage(image);
      onChange?.();
      return;
    }
    if (isCancelled() || !image.isConnected) return;
    image.addEventListener("load", () => { image.classList.remove("reader-image-waiting"); onChange?.(); }, { once: true });
    image.addEventListener("error", () => { missingImage(image); onChange?.(); }, { once: true });
    image.decoding = "async";
    image.src = src;
    image.removeAttribute("data-asset-id");
    onChange?.();
  };
  const unsized: HTMLImageElement[] = [];
  const observer = scroller ? new IntersectionObserver((entries) => {
    if (isCancelled()) { observer?.disconnect(); return; }
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      observer?.unobserve(entry.target);
      void load(entry.target as HTMLImageElement);
    }
  }, { root: scroller, rootMargin: nearViewport }) : null;
  for (const image of images) {
    const id = Number(image.dataset.assetId);
    if (!Number.isSafeInteger(id) || id <= 0) { missingImage(image); continue; }
    image.classList.add("reader-image-waiting");
    if (reserveSpace(image) && observer) observer.observe(image);
    else unsized.push(image);
  }
  let next = 0;
  const work = async () => {
    while (!isCancelled() && next < unsized.length) await load(unsized[next++]);
  };
  await Promise.all(Array.from({ length: Math.min(unsizedWorkers, unsized.length) }, work));
}
