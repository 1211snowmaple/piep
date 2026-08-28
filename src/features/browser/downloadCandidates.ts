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
}

export interface SidebarItem {
  id: string;
  title: string;
  subtitle?: string;
  selected: boolean;
  originalData: PixivNovel | FanboxPost;
  status?: "pending" | "downloading" | "success" | "skipped" | "failed";
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
  return url
    .replace(/pixiv:\/\/illusts?\/(\d+)/g, "https://www.pixiv.net/artworks/$1")
    .replace(/pixiv:\/\/novels?\/(\d+)/g, "https://www.pixiv.net/novel/show.php?id=$1")
    .replace(/pixiv:\/\/users?\/(\d+)/g, "https://www.pixiv.net/users/$1");
}

export function extractSavedSourceTarget(url: string): SavedSourceTarget | null {
  const normalized = normalizeContentLinkUrl(url);
  const pixivNovelMatch = normalized.match(/pixiv\.net\/(?:[a-z]{2}\/)?novels\/(\d+)/)
    || normalized.match(/pixiv\.net\/(?:[a-z]{2}\/)?novel\/show\.php\?id=(\d+)/);
  if (pixivNovelMatch?.[1]) {
    return { source: "pixiv", sourceId: pixivNovelMatch[1] };
  }

  const fanboxPostMatch = normalized.match(/fanbox\.cc\/(?:@[^/]+\/)?posts\/(\d+)/);
  if (fanboxPostMatch?.[1]) {
    return { source: "fanbox", sourceId: fanboxPostMatch[1] };
  }

  return null;
}

export function getFanboxCreatorId(url: string): string | null {
  const subMatch = url.match(/https:\/\/([^.]+)\.fanbox\.cc/);
  if (subMatch && subMatch[1] !== "www" && subMatch[1] !== "api") return subMatch[1];
  const dirMatch = url.match(/fanbox\.cc\/@([^/?#\s]+)/);
  if (dirMatch) return dirMatch[1];
  return null;
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
  if (!url) return none;
  const normalized = normalizeContentLinkUrl(url);

  if (normalized.includes("pixiv.net")) {
    const seriesId = normalized.match(/novel\/series\/(?:show\.php\?id=)?(\d+)/)?.[1];
    if (seriesId) return { kind: "pixiv_series", id: seriesId };
    const userId = normalized.match(/users\/(\d+)/)?.[1];
    if (userId && !normalized.includes("/novels/")) return { kind: "pixiv_user", id: userId };
    const novelId = normalized.match(/novels\/(\d+)/)?.[1] || normalized.match(/novel\/show\.php\?id=(\d+)/)?.[1];
    if (novelId) return { kind: "pixiv_single", id: novelId };
  }

  if (normalized.includes("fanbox.cc")) {
    const postId = normalized.match(/posts\/(\d+)/)?.[1];
    if (postId) return { kind: "fanbox_single", id: postId };
    const creatorId = getFanboxCreatorId(normalized);
    if (creatorId) return { kind: "fanbox_creator", id: creatorId };
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

