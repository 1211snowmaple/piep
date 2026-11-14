import { useLocalStorage } from "@mantine/hooks";
import { useQuery } from "@tanstack/react-query";
import {
  libraryV2Decorations,
  libraryV2Search,
  libraryV2Status,
  shelfFileUrl,
  type LibraryV2Status,
  type ShelfCard,
  type ShelfCursor,
  type ShelfDecoration,
  type ShelfFilter,
  type ShelfListQuery,
  type ShelfMatch,
  type ShelfSort,
  type ShelfTextSearch,
} from "@/services/libraryV2Api";
import type { LibrarySortBy, ListedWork, SearchV2Params, SearchV2Result, ShelfEntry } from "@/types/library";

/**
 * ライブラリの一覧を新しい棚から出す（試験中・読むだけ）。
 *
 * 旧棚が正本のあいだ、新しい棚は移行で作った写しで、アプリは読むだけにする。一覧の
 * 条件（検索語・絞り込み・並び順）は新しい棚の問い合わせに直して渡し、結果はカードが
 * 読む形（`ShelfEntry`）にする。作品は新しい棚の ID のままで、旧棚の番号に読み替えない。
 */

export const SHELF_V2_PREFERENCE_KEY = "piep.library-shelf-v2";

export function isShelfEntry(work: ListedWork): work is ShelfEntry {
  return typeof work.id === "string";
}

/** 一覧を新しい棚から出すか。端末ごとの好み。 */
export function useShelfV2Preference() {
  return useLocalStorage<boolean>({ key: SHELF_V2_PREFERENCE_KEY, defaultValue: false, getInitialValueInEffect: false });
}

const SETTLED: ReadonlySet<LibraryV2Status["state"]> = new Set(["absent", "ready", "failed"]);

/** 新しい棚の状態。開いている途中と索引の同期中は、落ち着くまで読み直す。 */
export function useLibraryV2Status(enabled: boolean) {
  return useQuery({
    queryKey: ["library-v2-status"],
    queryFn: libraryV2Status,
    enabled,
    refetchInterval: (query) => {
      const state = query.state.data?.state;
      return state && SETTLED.has(state) ? false : 2_000;
    },
  });
}

/** 一覧を出せる状態か。語の検索は索引の同期が済むまで使えない（`ready` だけ）。 */
export function shelfListable(status: LibraryV2Status | undefined): boolean {
  return status?.state === "ready" || status?.state === "indexing";
}

/** 新しい棚では使えない条件。画面はこれを出して、黙って外さない。 */
export class ShelfUnsupportedError extends Error {
  constructor(readonly conditions: string[]) {
    super(`新しい棚ではまだ使えない条件です: ${conditions.join("、")}`);
    this.name = "ShelfUnsupportedError";
  }
}

const SORTS: Partial<Record<LibrarySortBy, ShelfSort>> = {
  downloaded_at: "saved",
  source_created_at: "published",
  source_updated_at: "updated",
  title: "title",
  author_name: "author",
  text_length: "length",
  file_size_bytes: "size",
  asset_count: "media",
  current_version: "version",
  relevance: "relevance",
};

const DATE_FIELDS = {
  downloaded_at: "saved",
  source_created_at: "published",
  source_updated_at: "updated",
} as const;

const MEDIA = {
  has_assets: "any",
  no_assets: "none",
  has_images: "images",
  has_files: "files",
  has_images_and_files: "images_and_files",
} as const;

/** 旧棚の日付の絞り込みは UTC の日で両端を含む。新しい棚はミリ秒で、終わりを含まない。 */
function utcDay(value: string | null | undefined, offsetDays = 0): number | null {
  if (!value || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const [year, month, day] = value.split("-").map(Number);
  return Date.UTC(year, month - 1, day + offsetDays);
}

function range(min: number | null | undefined, max: number | null | undefined) {
  return { min: min ?? null, max: max ?? null };
}

function nonEmpty(values: string[] | null | undefined): string[] {
  return (values ?? []).filter((value) => value.trim() !== "");
}

/** 旧棚の続きの位置（文字列）と新しい棚の続きの位置を行き来する。 */
export function encodeShelfCursor(cursor: ShelfCursor | null): string | null {
  return cursor ? JSON.stringify(cursor) : null;
}

function decodeShelfCursor(cursor: string | null | undefined): ShelfCursor | null {
  if (!cursor) return null;
  try {
    const value = JSON.parse(cursor) as ShelfCursor;
    return value && typeof value.work === "string" ? value : null;
  } catch {
    return null;
  }
}

/**
 * 一覧の条件を新しい棚の問い合わせにする。使えない条件があれば `ShelfUnsupportedError`。
 *
 * シリーズの指名（`seriesKey`）は検索語の書き方（`series:取得元:キー`）にして渡す。新しい棚は
 * シリーズを自分の ID で指すので、取得元のキーは書き方の側で引く。
 */
export function shelfRequest(params: SearchV2Params): { search: ShelfTextSearch; query: ShelfListQuery } {
  const unsupported: string[] = [];
  if (params.idsInclude) unsupported.push("読みかけ");
  if (params.offset !== null && params.offset !== undefined) unsupported.push("ページ番号");
  if (params.personKey) unsupported.push("作者の指名");
  const sortBy = params.sortBy ?? "downloaded_at";
  const sort = SORTS[sortBy];
  if (!sort) unsupported.push("シリーズ順");
  if (unsupported.length) throw new ShelfUnsupportedError(unsupported);

  const terms = [params.text?.trim() ?? ""];
  if (params.seriesKey) {
    terms.push(params.seriesSource ? `series:${params.seriesSource}:${params.seriesKey}` : `series:${params.seriesKey}`);
  }
  const text = terms.filter(Boolean).join(" ");

  const filter: ShelfFilter = {
    provider: params.source ?? null,
    content_type: params.contentType ?? null,
    favorite: params.favorite ?? null,
    watching: params.watchFilter ? params.watchFilter === "watched" : null,
    edited: params.editFilter ? params.editFilter === "edited" : null,
    has_cover: params.coverFilter ? params.coverFilter === "has_cover" : null,
    in_series: params.seriesFilter ? params.seriesFilter === "in_series" : null,
    media: params.assetFilter ? MEDIA[params.assetFilter] : null,
    revision: params.revisionFilter ?? null,
    acquisition: params.acquisitionFilter ?? null,
    text_length: range(params.minCharCount, params.maxCharCount),
    media_count: range(params.minAssetCount, params.maxAssetCount),
    logical_size: range(params.minFileSizeBytes, params.maxFileSizeBytes),
    date: params.dateFrom || params.dateTo
      ? {
          field: DATE_FIELDS[params.dateField ?? "downloaded_at"],
          from: utcDay(params.dateFrom),
          until: utcDay(params.dateTo, 1),
        }
      : null,
    tags: nonEmpty(params.tagsInclude),
    any_tag: params.tagFilterMode === "or",
    excluded_tags: nonEmpty(params.tagsExclude),
    authors: nonEmpty(params.authorsInclude),
    excluded_authors: nonEmpty(params.authorsExclude),
  };
  return {
    search: {
      text,
      versions: params.versionScope === "all" ? "all" : "current",
      mode: params.searchMode === "semantic" ? "meaning" : "words",
    },
    query: {
      sort: sort!,
      descending: (params.sortOrder ?? "desc") === "desc",
      limit: params.limit ?? 60,
      after: decodeShelfCursor(params.cursor),
      filter,
      count: !params.cursor,
    },
  };
}

function isoOrNull(ms: number | null): string | null {
  return ms === null ? null : new Date(ms).toISOString();
}

/** 新しい棚の一件を、カードが読む形にする。 */
export function shelfEntry(card: ShelfCard, match?: ShelfMatch, decoration?: ShelfDecoration): ShelfEntry {
  return {
    id: card.id,
    shelf: "v2",
    coverUrl: shelfFileUrl(card.coverFile),
    source: card.provider,
    sourceId: card.external_key,
    title: card.title,
    authorName: card.author_name,
    authorId: "",
    contentType: card.content_type,
    tags: card.tags,
    excerpt: card.excerpt,
    coverPath: null,
    jsonPath: "",
    assetCount: card.media_count,
    fileSizeBytes: card.logical_size,
    downloadedAt: new Date(card.saved_at).toISOString(),
    sourceCreatedAt: isoOrNull(card.published_at),
    sourceUpdatedAt: isoOrNull(card.updated_at),
    currentVersion: card.version_number,
    watchUpdates: card.watching,
    textLength: card.text_length,
    favorite: card.favorite,
    seriesTitle: card.series_title,
    seriesId: null,
    searchScore: match?.score ?? null,
    matchFields: decoration?.match_fields ?? [],
    scoreReasons: decoration?.score_reasons ?? [],
    matchHighlights: decoration?.highlights ?? [],
    matchedVersion: match?.matched_version ?? null,
    historicalMatchCount: match?.historical_matches ?? 0,
  };
}

export type ShelfSearchResult = Omit<SearchV2Result, "items"> & { items: ShelfEntry[] };

/**
 * 一覧の一ページを新しい棚から引く。一致した箇所と理由は、語の検索で一致があったときだけ
 * 続けて引く（旧棚の一覧と同じく、カードに一致の理由を出す）。
 */
export async function searchShelf(params: SearchV2Params): Promise<ShelfSearchResult> {
  const { search, query } = shelfRequest(params);
  const view = await libraryV2Search(search, query);
  const decorations = search.text && view.matches.length
    ? await libraryV2Decorations(search.text, view.matches)
    : [];
  const matchByWork = new Map(view.matches.map((match) => [match.work, match]));
  const decorationByWork = new Map(decorations.map((decoration) => [decoration.work, decoration]));
  const items = view.cards.map((card) => shelfEntry(card, matchByWork.get(card.id), decorationByWork.get(card.id)));
  const total = view.page.total;
  // 文言は旧棚の一覧（`search_explanations`）と同じにする。
  const intent = view.intent;
  const explanations = intent
    ? [`${intent.kind === "series" ? "シリーズ" : "作者"}「${intent.label}」への完全一致として解釈し、関係する作品だけに限定しました`]
    : [];
  return {
    items,
    nextCursor: encodeShelfCursor(view.page.next),
    totalEstimate: total,
    searchMeta: {
      engine: search.mode === "meaning" ? "library-v2-meaning" : "library-v2",
      query: search.text || null,
      totalEstimate: total,
      indexComplete: true,
      explanations,
      exactEntity: intent ? { kind: intent.kind, label: intent.label, strict: true } : null,
    },
    facetsVersion: 0,
  };
}
