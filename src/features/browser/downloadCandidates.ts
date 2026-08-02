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

export function normalizeUrlForSidebar(url: string): string {
  try {
    const parsed = new URL(url);
    parsed.hash = "";
    parsed.pathname = parsed.pathname.replace(/\/+$/, "") || "/";
    return parsed.toString();
  } catch {
    return url.trim();
  }
}

export function isSameSidebarUrl(a?: string | null, b?: string | null): boolean {
  if (!a || !b) return false;
  return normalizeUrlForSidebar(a) === normalizeUrlForSidebar(b);
}

export function getFanboxCreatorId(url: string): string | null {
  try {
    const subMatch = url.match(/https:\/\/([^.]+)\.fanbox\.cc/);
    if (subMatch && subMatch[1] !== "www" && subMatch[1] !== "api") return subMatch[1];
    const dirMatch = url.match(/fanbox\.cc\/@([^/?#\s]+)/);
    if (dirMatch) return dirMatch[1];
  } catch {}
  return null;
}

export function detectDownloadTarget(url: string): DownloadTargetKind {
  if (!url) return "unsupported";
  const isPixiv = url.includes("pixiv.net");
  const isFanbox = url.includes("fanbox.cc");

  if (isPixiv) {
    if (url.match(/novel\/series\/(\d+)/) || url.match(/novel\/series\/show\.php\?id=(\d+)/)) return "pixiv_series";
    const userIdMatch = url.match(/users\/(\d+)/);
    if (userIdMatch?.[1] && !url.includes("/novels/")) return "pixiv_user";
    if (url.match(/novels\/(\d+)/) || url.match(/novel\/show\.php\?id=(\d+)/)) return "pixiv_single";
  }

  if (isFanbox) {
    if (url.match(/posts\/(\d+)/)) return "fanbox_single";
    if (getFanboxCreatorId(url)) return "fanbox_creator";
  }

  return "unsupported";
}

export function stripSidebarItemStatus(items: SidebarItem[]): SidebarItem[] {
  return items.map(({ status: _status, ...item }) => ({ ...item }));
}
