import { normalizeContentLinkUrl } from "@shared/contentLinkTarget";

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
  | "fanbox_creator"
  | "pixiv_page"
  | "fanbox_page";
export type SidebarMode = "empty" | "loading" | "analysis" | "downloadProgress" | "downloadDone";
export type DownloadTargetKind = SidebarDownloadType | "unsupported";

export interface SidebarAnalysisState {
  sourceUrl: string;
  title: string;
  items: SidebarItem[];
  downloadType: SidebarDownloadType;
  analyzedAt: number;
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

/**
 * URL だけでは中身が分からない、取得元の一覧ページの取得元。
 *
 * 検索結果、ランキング、ブックマーク、フォロー新着など。作品・シリーズ・作者の
 * ページは API で全件を取れるので、ここには入れない。
 */
export function pageListSource(url: string): "pixiv" | "fanbox" | null {
  if (describeDownloadTarget(url).kind !== "unsupported") return null;
  const parsed = sourceUrl(url);
  if (parsed?.protocol !== "https:") return null;
  if (isSourceHost(parsed.hostname, "pixiv.net")) return "pixiv";
  if (isSourceHost(parsed.hostname, "fanbox.cc")) return "fanbox";
  return null;
}

/** ページ番号（`?p=`）で続きを開ける pixiv の一覧。分からないページは送らない。 */
const PAGED_PIXIV_LISTS = [
  /^\/tags\/[^/]+\/novels\/?$/,
  /^\/users\/\d+\/bookmarks\/novels(?:\/[^/]+)?\/?$/,
  /^\/novel\/bookmark\.php$/,
  /^\/(?:novel\/)?bookmark_new(?:_r18)?\.php$/,
  /^\/novel\/ranking\.php$/,
  /^\/users\/\d+\/following\/?$/,
];

/** pixiv のフォロー一覧か。ここからは作品でなく作者を読む。 */
export function isFollowingListUrl(url: string): boolean {
  const parsed = sourceUrl(url);
  return pageListSource(url) === "pixiv" && /^\/users\/\d+\/following\/?$/.test(sourcePath(parsed?.pathname ?? ""));
}

/** 一覧の何ページ目か。URL に無ければ 1 ページ目である。 */
export function listPageNumber(url: string): number {
  const value = sourceUrl(url)?.searchParams.get("p") ?? "";
  const page = /^\d{1,6}$/.test(value) ? Number(value) : 1;
  return page >= 1 ? page : 1;
}

/** 一覧の次のページの URL。続きの開き方が分かっている一覧でなければ null。 */
export function nextPagedListUrl(url: string): string | null {
  if (pageListSource(url) !== "pixiv") return null;
  const parsed = new URL(normalizeContentLinkUrl(url));
  const path = sourcePath(parsed.pathname);
  if (!PAGED_PIXIV_LISTS.some((pattern) => pattern.test(path))) return null;
  parsed.searchParams.set("p", String(listPageNumber(url) + 1));
  return parsed.href;
}

/** 一覧から読み取った作品ひとつ。 */
export interface PageWorkCandidate {
  sourceId: string;
  title: string;
  author: string;
  saved: boolean;
}

/** 読み取った作品を候補の行にする。棚にあるものは選ばず、保存済みと出す。 */
export function pageWorkItems(source: "pixiv" | "fanbox", works: PageWorkCandidate[]): SidebarItem[] {
  return works.map((work) => {
    const title = work.title || `作品 ${work.sourceId}`;
    return {
      id: work.sourceId,
      title,
      subtitle: work.author || undefined,
      selected: !work.saved,
      ...(work.saved ? { status: "skipped" as const } : {}),
      originalData: source === "pixiv"
        ? { id: work.sourceId, title, characterCount: 0, createDate: "", user: { id: "", name: work.author } }
        : { id: work.sourceId, title, type: "", publishedDatetime: "", user: { userId: "", name: work.author } },
    };
  });
}

/** 同じ一覧の続きを足す。行の状態と選択は、すでにある行のものを残す。 */
export function mergePageWorkItems(current: SidebarItem[], incoming: SidebarItem[]): SidebarItem[] {
  const known = new Set(current.map((item) => item.id));
  return [...current, ...incoming.filter((item) => !known.has(item.id))];
}

export function detectDownloadTarget(url: string): DownloadTargetKind {
  return describeDownloadTarget(url).kind;
}

/**
 * 取得した一覧が「どのページのものか」を表す合言葉。対応していないページは
 * 空文字。同じ合言葉のあいだは、取り直す必要がない。一覧のページ番号は合言葉に
 * 入れない。続きのページは同じ一覧で、読むと候補に足していく。
 */
export function downloadTargetKey(url: string): string {
  const { kind, id } = describeDownloadTarget(url);
  if (kind !== "unsupported") return `${kind}:${id}`;
  if (!pageListSource(url)) return "";
  const parsed = new URL(normalizeContentLinkUrl(url));
  parsed.searchParams.delete("p");
  return `page:${parsed.hostname}${sourcePath(parsed.pathname)}${parsed.search}`;
}

export { normalizeContentLinkUrl };
