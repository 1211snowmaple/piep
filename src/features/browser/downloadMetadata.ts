export interface NormalizedSaveMetadata {
  title: string;
  authorName: string;
  authorId: string;
  contentType: string;
  tags: string[];
  excerpt: string | null;
  sourceCreatedAt: string | null;
}

function firstString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value;
  }
  return null;
}

function normalizeTags(value: unknown): string[] {
  const candidate = value && typeof value === "object" && !Array.isArray(value)
    ? (value as { tags?: unknown }).tags
    : value;
  if (!Array.isArray(candidate)) return [];
  return [...new Set(candidate.map((tag) => {
    if (typeof tag === "string") return tag.trim();
    if (tag && typeof tag === "object") return firstString((tag as { name?: unknown }).name) ?? "";
    return "";
  }).filter(Boolean))];
}

/**
 * Older backend builds and FANBOX response variants can leave one or more
 * `{ body: post }` envelopes around a post. Only unwrap when the outer object
 * is not itself a post, so the actual article `body` remains untouched.
 */
export function normalizeFanboxPostPayload<T = any>(value: unknown): T {
  let current: any = value;
  for (let depth = 0; depth < 8; depth += 1) {
    if (!current || typeof current !== "object" || Array.isArray(current) || current.id != null || current.title != null) break;
    const nested = current.post ?? current.data ?? current.result ?? current.body;
    if (!nested || typeof nested !== "object" || Array.isArray(nested)) break;
    current = nested;
  }
  return current as T;
}

/**
 * `fallbackAuthorId` は、取り直しのときに要る。
 *
 * 新規保存なら作者が分からなければ "0" でよいが、すでに手元にある作品を
 * 取り直すときに "0" へ落とすと、**作者との紐付けが切れて作者ページから
 * 消える**。呼ぶ側が知っている作者IDを渡せるようにしておく。
 */
export function normalizePixivSaveMetadata(data: any, fallbackTitle = "pixiv小説", fallbackAuthor = "unknown", fallbackAuthorId = "0"): NormalizedSaveMetadata {
  const detail = data?.detail ?? data ?? {};
  return {
    title: firstString(detail.title, data?.title, fallbackTitle) ?? fallbackTitle,
    authorName: firstString(detail.user?.name, data?.user?.name, fallbackAuthor) ?? fallbackAuthor,
    authorId: String(detail.user?.id ?? data?.user?.id ?? fallbackAuthorId),
    contentType: "novel",
    tags: normalizeTags(detail.tags ?? data?.tags),
    excerpt: firstString(detail.caption, data?.caption),
    sourceCreatedAt: firstString(detail.create_date, detail.createDate, data?.create_date, data?.createDate),
  };
}

export function normalizeFanboxSaveMetadata(data: any, fallbackTitle = "FANBOX投稿", fallbackAuthor = "unknown", fallbackAuthorId = "0"): NormalizedSaveMetadata {
  data = normalizeFanboxPostPayload(data);
  return {
    title: firstString(data?.title, fallbackTitle) ?? fallbackTitle,
    authorName: firstString(data?.user?.name, fallbackAuthor) ?? fallbackAuthor,
    authorId: String(data?.creatorId ?? data?.creator_id ?? data?.user?.userId ?? data?.user?.user_id ?? fallbackAuthorId),
    contentType: firstString(data?.type, data?.postType, data?.post_type, "article") ?? "article",
    tags: normalizeTags(data?.tags),
    excerpt: firstString(data?.excerpt),
    sourceCreatedAt: firstString(data?.publishedDatetime, data?.published_datetime),
  };
}
