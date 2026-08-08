type LocalAssetResolver = (path: string | null | undefined) => string | null;

// `style`/`svg`/`math` are re-parsed differently when the sanitised string is
// handed back to innerHTML, which is the classic mutation-XSS route, and a
// `style` attribute alone is enough to stretch stored content over the whole
// window. Saved documents are third-party HTML, so all of it is dropped.
const UNSAFE_ELEMENTS = "script, style, iframe, object, embed, form, link, base, meta, template, noscript, svg, math, portal, frame, frameset";
const SAFE_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);
const ALLOWED_URL_ATTRIBUTES = new Set(["href", "src"]);

export function prepareDocumentHtml(html: string, resolveLocalAsset: LocalAssetResolver): string {
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
  anchor.dataset.provider = fanbox ? "fanbox" : "web";

  const existingBrand = anchor.querySelector<HTMLElement>(".link-card-brand, .link-card-icon");
  if (existingBrand) {
    existingBrand.className = "link-card-brand";
    existingBrand.textContent = fanbox ? "F" : "↗";
  }

  if (!anchor.querySelector(".link-card-info")) {
    const title = plainUrl || url.href;
    anchor.replaceChildren();
    const brand = document.createElement("span");
    brand.className = "link-card-brand";
    brand.textContent = fanbox ? "F" : "↗";
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
