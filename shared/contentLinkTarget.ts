// 本文中のリンクが、取得元のどの作品・作者・シリーズを指すかを読む。
// 手元にあるかを調べて開く部分は、デスクトップと携帯でそれぞれ持つ。

/**
 * アプリ向けの `pixiv://` の宛先を、ブラウザで開ける `https://` の宛先に直す。
 * それ以外は前後の空白を除いてそのまま返す。
 */
export function normalizeContentLinkUrl(raw: string): string {
  const url = raw.trim();
  const deepLink = url.match(/^pixiv:\/\/(illusts?|novels?|users?)\/(\d+)\/?(\?[^#]*)?(#.*)?$/i);
  if (!deepLink) return url;
  const [, kind, id, query = "", fragment = ""] = deepLink;
  if (/^novels?$/i.test(kind)) {
    const extra = new URLSearchParams(query.slice(1)).toString();
    return `https://www.pixiv.net/novel/show.php?id=${id}${extra ? `&${extra}` : ""}${fragment}`;
  }
  const path = /^users?$/i.test(kind) ? `users/${id}` : `artworks/${id}`;
  return `https://www.pixiv.net/${path}${query}${fragment}`;
}

export type ContentLinkTarget =
  | { kind: "work"; source: string; sourceId: string }
  | { kind: "person"; source: string; sourceKey: string }
  | { kind: "series"; source: string; sourceKey: string };

/** The creator subdomain, when a FANBOX address carries one. */
function fanboxCreator(url: URL): string | null {
  const host = url.hostname.replace(/^www\./, "");
  if (host.endsWith(".fanbox.cc")) return host.slice(0, -".fanbox.cc".length) || null;
  const inPath = url.pathname.match(/^\/@([^/]+)/);
  return inPath?.[1] ?? null;
}

/**
 * What a link inside saved text names, if it names anything the app holds.
 *
 * pixiv writes its own captions with `pixiv://` deep links for the app, so the
 * addresses that most want resolving are the ones a browser cannot open either.
 */
export function contentLinkTarget(raw: string): ContentLinkTarget | null {
  let url: URL;
  try {
    url = new URL(normalizeContentLinkUrl(raw.trim()));
  } catch {
    return null;
  }
  // 取得元の内部リンクとして扱うのは、ブラウザで同じサービスへ安全に渡せる
  // 形だけ。`http:` や資格情報付きの見た目だけ FANBOX の URL を、手元の作品へ
  // 解決して本物らしく見せない。
  if (url.protocol !== "https:" || url.port || url.username || url.password) return null;
  const host = url.hostname.replace(/^www\./, "");

  if (host === "pixiv.net") {
    const series = url.pathname.match(/\/novel\/series\/(\d+)/);
    if (series) return { kind: "series", source: "pixiv", sourceKey: series[1] };
    const user = url.pathname.match(/^\/(?:[a-z]{2}\/)?users\/(\d+)/);
    if (user) return { kind: "person", source: "pixiv", sourceKey: user[1] };
    const novelId = url.pathname.match(/^\/(?:[a-z]{2}\/)?novel\/show\.php$/) ? url.searchParams.get("id") : null;
    const novelPath = url.pathname.match(/^\/(?:[a-z]{2}\/)?novels?\/(\d+)/);
    const id = novelId ?? novelPath?.[1];
    if (id && /^\d+$/.test(id)) return { kind: "work", source: "pixiv", sourceId: id };
    return null;
  }

  if (host === "fanbox.cc" || host.endsWith(".fanbox.cc")) {
    const post = url.pathname.match(/\/posts\/(\d+)/);
    if (post) return { kind: "work", source: "fanbox", sourceId: post[1] };
    const creator = fanboxCreator(url);
    if (creator) return { kind: "person", source: "fanbox", sourceKey: creator };
    return null;
  }

  return null;
}
