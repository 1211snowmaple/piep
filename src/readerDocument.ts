import DOMPurify from "dompurify";
import type { PcAsset } from "./pcApi";

function safeHtml(source: string): string {
  return DOMPurify.sanitize(source, {
    ALLOWED_TAGS: ["p", "br", "h1", "h2", "h3", "ruby", "rt", "rp", "span", "strong", "em", "b", "i", "u", "s", "blockquote", "figure", "figcaption", "img", "hr", "a", "div", "ul", "ol", "li", "code", "pre"],
    ALLOWED_ATTR: ["class", "alt", "data-asset-id", "data-page", "href"],
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

/**
 * 本文の挿絵を埋める。四つの要求で接続を分け合い、届いたものから順に出す。
 * 版が変わっていたら snapshot_changed を返すので、呼び出し側が読み直す。
 */
export async function fillDocumentImages({ root, loadAsset, isCancelled, isSnapshotChange, onChange }: {
  root: HTMLElement;
  loadAsset: (assetId: number) => Promise<PcAsset>;
  isCancelled: () => boolean;
  isSnapshotChange: (error: unknown) => boolean;
  onChange?: () => void;
}): Promise<"done" | "snapshot_changed"> {
  const images = [...root.querySelectorAll<HTMLImageElement>(".online-document img")];
  if (!images.length) return "done";
  const uniqueIds = [...new Set(images.map((image) => Number(image.dataset.assetId)).filter((id) => Number.isSafeInteger(id) && id > 0))];
  for (const image of images) {
    const id = Number(image.dataset.assetId);
    if (!Number.isSafeInteger(id) || id <= 0) missingImage(image);
  }
  let nextAsset = 0;
  let snapshotChanged = false;
  const fetchAsset = async () => {
    while (!isCancelled() && !snapshotChanged && nextAsset < uniqueIds.length) {
      const assetId = uniqueIds[nextAsset++];
      let source: string | null = null;
      try {
        const asset = await loadAsset(assetId);
        source = `data:${asset.mimeType};base64,${asset.base64}`;
      } catch (error) {
        if (isSnapshotChange(error)) { snapshotChanged = true; return; }
        // Other image failures leave a visible placeholder.
      }
      if (isCancelled() || snapshotChanged) return;
      for (const image of root.querySelectorAll<HTMLImageElement>(`.online-document img[data-asset-id="${assetId}"]`)) {
        if (source) {
          image.src = source;
          image.removeAttribute("data-asset-id");
        } else missingImage(image);
      }
      onChange?.();
    }
  };
  await Promise.all(Array.from({ length: Math.min(4, uniqueIds.length) }, fetchAsset));
  return snapshotChanged ? "snapshot_changed" : "done";
}
