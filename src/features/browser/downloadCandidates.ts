export interface SavedSourceTarget {
  source: "pixiv" | "fanbox";
  sourceId: string;
}

export interface PixivUser {
  id: string;
  name: string;
}

export interface PixivSeriesNavigation {
  seriesId: string;
  seriesTitle: string;
}

export interface PixivTag {
  name: string;
}

export interface PixivNovel {
  id: string;
  title: string;
  characterCount: number;
  createDate: string;
  user: PixivUser;
  seriesNavigation?: PixivSeriesNavigation;
  body?: string;
  cover_url?: string;
  tags?: (PixivTag | string)[];
  detail?: {
    id: string;
    title: string;
    user: PixivUser;
    cover_url?: string;
    seriesNavigation?: PixivSeriesNavigation;
    tags?: { tags: PixivTag[] } | PixivTag[] | string[];
  };
}

export interface FanboxUser {
  userId: string;
  name: string;
}

export interface FanboxPost {
  id: string;
  title: string;
  type: string;
  publishedDatetime: string;
  user: FanboxUser;
  body?: unknown;
  coverImageUrl?: string;
  tags?: string[];
  creatorId?: string;
  isRestricted?: boolean;
  feeRequired?: number;
}

export interface SidebarItem {
  id: string;
  title: string;
  subtitle?: string;
  selected: boolean;
  originalData: PixivNovel | FanboxPost;
  status?: "pending" | "downloading" | "success" | "skipped" | "failed" | "held";
  error?: string;
}

export type SidebarDownloadType =
  | "pixiv_single"
  | "pixiv_series"
  | "pixiv_user"
  | "fanbox_single"
  | "fanbox_creator";
export type SidebarMode = "empty" | "loading" | "analysis" | "downloadProgress" | "downloadDone";
export type DownloadTargetKind = SidebarDownloadType | "unsupported";

export interface SidebarAnalysisState {
  sourceUrl: string;
  title: string;
  items: SidebarItem[];
  downloadType: SidebarDownloadType;
  analyzedAt: number;
}

export function normalizeContentLinkUrl(url: string): string {
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

function sourceUrl(raw: string): URL | null {
  try {
    const parsed = new URL(normalizeContentLinkUrl(raw));
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed : null;
  } catch {
    return null;
  }
}

function isSourceHost(hostname: string, domain: string): boolean {
  return hostname === domain || hostname.endsWith(`.${domain}`);
}

function sourcePath(pathname: string): string {
  return pathname.replace(/^\/[a-z]{2}\//i, "/");
}

function isSourceId(value: string | null | undefined): value is string {
  return Boolean(value && /^\d+$/.test(value));
}

export function extractSavedSourceTarget(url: string): SavedSourceTarget | null {
  const target = describeDownloadTarget(url);
  if (target.kind === "pixiv_single") return { source: "pixiv", sourceId: target.id };
  if (target.kind === "fanbox_single") return { source: "fanbox", sourceId: target.id };
  return null;
}

export function getFanboxCreatorId(url: string): string | null {
  const parsed = sourceUrl(url);
  if (!parsed || !isSourceHost(parsed.hostname, "fanbox.cc")) return null;
  const labels = parsed.hostname.split(".");
  if (labels.length === 3 && labels[0] !== "www" && labels[0] !== "api") return labels[0];
  return parsed.pathname.match(/^\/@([^/]+)(?:\/|$)/)?.[1] ?? null;
}

/** そのページが指している相手。作品ID・シリーズID・作者ID・クリエイター名。 */
export interface DownloadTarget {
  kind: DownloadTargetKind;
  id: string;
}

/**
 * URLではなく、URLが指している相手を読む。
 *
 * 取得元はどれもSPAで、同じページを見ているあいだにもURLだけが動く
 * （/users/789 と /users/789/novels、末尾の ?p=2、言語の /en/）。文字列を
 * 突き合わせると、同じページに居るのに「移動した」ことになってしまう。
 */
export function describeDownloadTarget(url: string): DownloadTarget {
  const none: DownloadTarget = { kind: "unsupported", id: "" };
  const parsed = sourceUrl(url);
  if (!parsed) return none;
  const path = sourcePath(parsed.pathname);

  if (isSourceHost(parsed.hostname, "pixiv.net")) {
    const seriesId = path.match(/^\/novel\/series\/(\d+)\/?$/)?.[1]
      ?? (path === "/novel/series/show.php" ? parsed.searchParams.get("id") : null);
    if (isSourceId(seriesId)) return { kind: "pixiv_series", id: seriesId };
    const userId = path.match(/^\/users\/(\d+)(?:\/novels)?\/?$/)?.[1];
    if (userId) return { kind: "pixiv_user", id: userId };
    const novelId = path.match(/^\/novels\/(\d+)\/?$/)?.[1]
      ?? (path === "/novel/show.php" ? parsed.searchParams.get("id") : null);
    if (isSourceId(novelId)) return { kind: "pixiv_single", id: novelId };
  }

  if (isSourceHost(parsed.hostname, "fanbox.cc")) {
    const postId = path.match(/^\/(?:@[^/]+\/)?posts\/(\d+)\/?$/)?.[1];
    if (postId) return { kind: "fanbox_single", id: postId };
    const creatorId = getFanboxCreatorId(parsed.href);
    const creatorPage = /^\/(?:@[^/]+\/?)?$/.test(path)
      || /^\/(?:@[^/]+\/)?(?:posts|plans|about)\/?$/.test(path);
    if (creatorId && creatorPage) return { kind: "fanbox_creator", id: creatorId };
  }

  return none;
}

export function detectDownloadTarget(url: string): DownloadTargetKind {
  return describeDownloadTarget(url).kind;
}

/**
 * 取得した一覧が「どのページのものか」を表す合言葉。対応していないページは
 * 空文字。同じ合言葉のあいだは、取り直す必要がない。
 */
export function downloadTargetKey(url: string): string {
  const { kind, id } = describeDownloadTarget(url);
  return kind === "unsupported" ? "" : `${kind}:${id}`;
}
