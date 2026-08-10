import { brandGlyphElement, externalBrand } from "@/lib/providers";

type LocalAssetResolver = (path: string | null | undefined) => string | null;

// `style`/`svg`/`math` are re-parsed differently when the sanitised string is
// handed back to innerHTML, which is the classic mutation-XSS route, and a
// `style` attribute alone is enough to stretch stored content over the whole
// window. Saved documents are third-party HTML, so all of it is dropped.
const UNSAFE_ELEMENTS = "script, style, iframe, object, embed, form, link, base, meta, template, noscript, svg, math, portal, frame, frameset";
const SAFE_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);
const ALLOWED_URL_ATTRIBUTES = new Set(["href", "src"]);

export interface PrepareDocumentOptions {
  /**
   * Turn URLs written as plain text into links.
   *
   * Whether a URL in a saved work is a link at all is decided by how its author
   * wrote it: FANBOX only makes an embed of a URL that stands alone on its own
   * line, so one written mid-sentence arrives as text and stays text. That is
   * faithful to the source, which is what the work detail screen wants - but
   * while reading, an address you cannot follow is just noise.
   */
  linkifyBareUrls?: boolean;
}

/**
 * Bare URLs in prose.
 *
 * Japanese punctuation cannot appear unencoded in an address, so the character
 * class stops at it - otherwise a URL followed by a comma swallows the rest of
 * the clause.
 */
const BARE_URL = /https?:\/\/[^\s<>"'`、。，；：！？「」『』【】〈〉《》（）()[\]{}]+/g;

/** Text inside these carries no links: they are quoting, not addressing. */
const NO_LINKIFY_INSIDE = new Set(["A", "CODE", "PRE", "KBD", "SAMP", "TEXTAREA"]);

function linkifyTextNodes(document: Document) {
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const targets: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node as Text;
    if (!text.data || !/https?:\/\//i.test(text.data)) continue;
    let parent = text.parentElement;
    let skip = false;
    while (parent && parent !== document.body) {
      if (NO_LINKIFY_INSIDE.has(parent.tagName)) { skip = true; break; }
      parent = parent.parentElement;
    }
    if (!skip) targets.push(text);
  }

  for (const text of targets) {
    const fragment = document.createDocumentFragment();
    let lastIndex = 0;
    BARE_URL.lastIndex = 0;
    for (let match = BARE_URL.exec(text.data); match; match = BARE_URL.exec(text.data)) {
      // Trailing sentence punctuation belongs to the prose, not the address.
      const raw = match[0].replace(/[.,;:!?、。）)\]}>」』】]+$/, "");
      if (!raw) continue;
      let url: URL;
      try {
        url = new URL(raw);
      } catch {
        continue;
      }
      if (url.protocol !== "http:" && url.protocol !== "https:") continue;
      fragment.append(text.data.slice(lastIndex, match.index));
      const anchor = document.createElement("a");
      anchor.href = raw;
      anchor.textContent = raw;
      fragment.append(anchor);
      lastIndex = match.index + raw.length;
    }
    if (lastIndex === 0) continue;
    fragment.append(text.data.slice(lastIndex));
    text.replaceWith(fragment);
  }
}

export function prepareDocumentHtml(
  html: string,
  resolveLocalAsset: LocalAssetResolver,
  options: PrepareDocumentOptions = {},
): string {
  if (!html.trim() || typeof DOMParser === "undefined") return html;

  const document = new DOMParser().parseFromString(html, "text/html");
  document.querySelectorAll(UNSAFE_ELEMENTS).forEach((node) => node.remove());
  document.body.querySelectorAll<HTMLElement>("*").forEach((node) => {
    for (const attribute of [...node.attributes]) {
      const name = attribute.name.toLowerCase();
      if (name.startsWith("on") || name === "srcdoc" || name === "style" || name === "srcset" || name === "formaction" || name === "xlink:href") {
        node.removeAttribute(attribute.name);
        continue;
      }
      // Anything that resolves to a URL and is not an anchor href or an image
      // source is dropped rather than protocol-checked one attribute at a time.
      if (!ALLOWED_URL_ATTRIBUTES.has(name) && /^(?:data-)?(?:action|background|ping|poster|codebase)$/.test(name)) {
        node.removeAttribute(attribute.name);
      }
    }
    if (node.tagName === "IMG") {
      const source = node.getAttribute("src");
      if (source && !/^(?:https?:|data:image\/|asset:|https?:\/\/asset\.localhost)/i.test(source.trim())) node.removeAttribute("src");
    }
  });

  if (options.linkifyBareUrls) linkifyTextNodes(document);

  document.body.querySelectorAll<HTMLAnchorElement>("a[href]").forEach((anchor) => {
    let url: URL;
    try {
      url = new URL(anchor.href, window.location.href);
    } catch {
      anchor.removeAttribute("href");
      return;
    }
    if (!SAFE_PROTOCOLS.has(url.protocol)) {
      anchor.removeAttribute("href");
      return;
    }
    if (url.protocol === "http:" || url.protocol === "https:") {
      anchor.target = "_blank";
      anchor.rel = "noopener noreferrer";
      decorateDocumentLink(document, anchor, url);
    }
  });

  document.body.querySelectorAll<HTMLImageElement>("img").forEach((image) => {
    const localPath = image.dataset.localPath;
    if (localPath) {
      const resolved = resolveLocalAsset(localPath);
      if (resolved) image.src = resolved;
    }
    image.setAttribute("loading", "lazy");
    image.setAttribute("decoding", "async");
    if (!image.alt) image.alt = "本文画像";
  });

  return document.body.innerHTML;
}

/** The mark for a link card, built from the same definition the screens use. */
function linkCardBrand(document: Document, url: URL): HTMLElement {
  const brand = externalBrand(url.href);
  const host = document.createElement("span");
  host.className = "link-card-brand";
  host.style.setProperty("--link-card-brand", brand.color);
  host.title = brand.label;
  if (brand.brandGlyph) host.append(brandGlyphElement(document, brand.brandGlyph));
  else host.textContent = "↗";
  return host;
}

function decorateDocumentLink(document: Document, anchor: HTMLAnchorElement, url: URL) {
  const hostname = url.hostname.replace(/^www\./, "");
  const fanbox = hostname === "fanbox.cc" || hostname.endsWith(".fanbox.cc");
  const plainUrl = (anchor.textContent ?? "").trim();
  const isUrlOnly = /^https?:\/\/\S+$/i.test(plainUrl);

  if (!anchor.classList.contains("novel-link-card") && !isUrlOnly) {
    anchor.classList.add("novel-inline-link");
    return;
  }

  anchor.classList.add("novel-link-card");
  if (fanbox) anchor.classList.add("novel-link-card--fanbox");
  anchor.dataset.provider = externalBrand(url.href).provider ?? "web";

  const existingBrand = anchor.querySelector<HTMLElement>(".link-card-brand, .link-card-icon");
  if (existingBrand) existingBrand.replaceWith(linkCardBrand(document, url));

  if (!anchor.querySelector(".link-card-info")) {
    const title = plainUrl || url.href;
    anchor.replaceChildren();
    const brand = linkCardBrand(document, url);
    const info = document.createElement("span");
    info.className = "link-card-info";
    const titleNode = document.createElement("span");
    titleNode.className = "link-card-title";
    titleNode.textContent = title;
    const hostNode = document.createElement("span");
    hostNode.className = "link-card-host";
    hostNode.textContent = hostname;
    info.append(titleNode, hostNode);
    const arrow = document.createElement("span");
    arrow.className = "link-card-arrow";
    arrow.textContent = "↗";
    anchor.append(brand, info, arrow);
  } else if (!anchor.querySelector(".link-card-arrow")) {
    const arrow = document.createElement("span");
    arrow.className = "link-card-arrow";
    arrow.textContent = "↗";
    anchor.append(arrow);
  }
}

export function summaryText(value: string | null | undefined): string {
  if (!value) return "";
  const normalized = value
    .replace(/<\s*br\s*\/?\s*>/gi, "\n")
    .replace(/<\/(?:p|div|h[1-6]|li|blockquote)>/gi, "\n")
    .replace(/<[^>]+>/g, " ")
    .replace(/&nbsp;/gi, " ")
    .replace(/&amp;/gi, "&")
    .replace(/&lt;/gi, "<")
    .replace(/&gt;/gi, ">")
    .replace(/&quot;/gi, '"')
    .replace(/&#(?:39|x27);/gi, "'")
    .replace(/[ \t\f\v]+/g, " ")
    .replace(/\s*\n\s*/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  return normalized;
}

export function splitDocumentPages(html: string, source: string): string[] {
  if (!html.trim() || typeof DOMParser === "undefined") return html.trim() ? [html] : [];

  // FANBOX posts are continuous articles. Only pixiv's explicit [newpage]
  // markers represent source pages; viewport height and character count do not.
  if (source.toLowerCase() !== "pixiv") return [html];

  const document = new DOMParser().parseFromString(html, "text/html");
  const pages: string[] = [];
  let pageParts: string[] = [];

  const flush = () => {
    const page = pageParts.join("").trim();
    if (page) pages.push(page);
    pageParts = [];
  };

  for (const node of [...document.body.childNodes]) {
    if (node.nodeType === Node.COMMENT_NODE && node.textContent?.toLowerCase().includes("newpage")) {
      flush();
      continue;
    }

    const wrapper = document.createElement("div");
    wrapper.append(node.cloneNode(true));
    pageParts.push(wrapper.innerHTML);
  }
  flush();
  return pages.length ? pages : [html];
}
