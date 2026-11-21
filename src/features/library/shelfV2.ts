import { useEffect } from "react";
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { adoptDeviceReadingKeys, readingKeysAdopted } from "@/features/library/readingShelf";
import { rekeyBookmarks } from "@/features/reader/bookmarks";
import {
  libraryV2AdoptDevicePositions,
  libraryV2Decorations,
  libraryV2Entities,
  libraryV2Collection,
  libraryV2EditCollection,
  type ShelfCollectionChange,
  libraryV2Collections,
  libraryV2EntityCollections,
  libraryV2WorkCollections,
  type ShelfCollection,
  type ShelfCollectionSummary,
  type ShelfEntityHead,
  type ShelfEntityKey,
  type ShelfEntityRow,
  type ShelfSeriesFacet,
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
import type { CollectionCoverMode, CollectionNameSource, WorkCollection, WorkCollectionSummary } from "@/types/collections";
import type { EntityFacet, EntityFacetScope, EntitySortBy, EntityVersion, LibrarySortBy, ListedWork, PersonEntry, SearchV2Params, SearchV2Result, SeriesEntry, ShelfEntry, SourceKind } from "@/types/library";

/**
 * ライブラリの一覧を棚から出す。
 *
 * 一覧の条件（検索語・絞り込み・並び順）は棚の問い合わせに直して渡し、結果はカードが
 * 読む形（`ShelfEntry`）にする。作品は棚の ID のまま。
 */

export function isShelfEntry(work: ListedWork): work is ShelfEntry {
  return typeof work.id === "string";
}

const SETTLED: ReadonlySet<LibraryV2Status["state"]> = new Set(["ready", "failed"]);

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

/**
 * 新しい棚へ書いたあと、画面が持っている問い合わせを読み直させる。印・件数・絞り込みの結果が
 * どこに出ているかを一つずつ追わず、棚の状態のほかは全部古いことにする（今出ている画面だけが
 * 読み直す）。
 */
export function invalidateShelf(queryClient: QueryClient, keep?: readonly unknown[]): Promise<void> {
  return queryClient.invalidateQueries({
    predicate: (query) => query.queryKey[0] !== "library-v2-status"
      && !(keep && keep.length === query.queryKey.length && keep.every((part, index) => part === query.queryKey[index])),
  });
}

/** 読みかけの棚を旧棚の番号で渡すか（端末の記録を棚の ID へ書き直す前だけ）。 */
function legacyReadingWorks(params: SearchV2Params): number[] | null {
  return readingKeysAdopted() ? null : params.idsInclude ?? null;
}

/**
 * 棚を正本に切り替えたら、端末に旧棚の番号で残した読書位置としおりを一度だけ棚の ID へ
 * 書き直し、読みかけを棚へ引き取る。アプリの頭で一度だけ使う。
 */
export function useAdoptReadingKeys(runtime: boolean) {
  const queryClient = useQueryClient();
  const pending = runtime && !readingKeysAdopted();
  const status = useLibraryV2Status(pending);
  const ready = pending && shelfListable(status.data);
  useEffect(() => {
    if (!ready) return;
    adoptDeviceReadingKeys(libraryV2AdoptDevicePositions, rekeyBookmarks)
      .then(() => invalidateShelf(queryClient))
      .catch((error: unknown) => console.error("端末の読書位置を棚へ引き取れません", error));
  }, [ready, queryClient]);
}

/** 棚ではまだ使えない条件。画面はこれを出して、黙って外さない。 */
export class ShelfUnsupportedError extends Error {
  constructor(readonly conditions: string[]) {
    super(`棚ではまだ使えない条件です: ${conditions.join("、")}`);
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
  series_order: "series_order",
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

/** 作者・シリーズの画面の一覧。棚の ID で、その作者・シリーズの作品に絞る。 */
export interface ShelfEntityScope {
  person?: string | null;
  series?: string | null;
}

/** 作品の絞り込み（検索語・並び順・ページを除いたもの）を、新しい棚の絞り込みにする。 */
export function shelfFilter(params: SearchV2Params, scope: ShelfEntityScope = {}): ShelfFilter {
  return {
    provider: params.source ?? null,
    content_type: params.contentType ?? null,
    favorite: params.favorite ?? null,
    watching: params.watchFilter ? params.watchFilter === "watched" : null,
    // 読みかけの棚。端末の記録を棚の ID へ書き直したあとは、棚の読書位置で絞る。
    reading: params.idsInclude && readingKeysAdopted() ? true : null,
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
    person: scope.person ?? null,
    series: scope.series ?? null,
  };
}

/**
 * 一覧の条件を新しい棚の問い合わせにする。使えない条件があれば `ShelfUnsupportedError`。
 *
 * シリーズの指名（`seriesKey`）は検索語の書き方（`series:取得元:キー`）にして渡す。新しい棚は
 * シリーズを自分の ID で指すので、取得元のキーは書き方の側で引く。作者・シリーズの画面は
 * 頭を引いた棚の ID を `scope` で渡す。
 */
export function shelfRequest(params: SearchV2Params, scope: ShelfEntityScope = {}): { search: ShelfTextSearch; query: ShelfListQuery; legacyWorks: number[] | null; person: ShelfEntityKey | null } {
  const unsupported: string[] = [];
  if (params.personKey && !params.personSource) unsupported.push("取得元のない作者の指名");
  const sortBy = params.sortBy ?? "downloaded_at";
  const sort = SORTS[sortBy];
  if (!sort) unsupported.push("この並び順");
  if (unsupported.length) throw new ShelfUnsupportedError(unsupported);

  const terms = [params.text?.trim() ?? ""];
  if (params.seriesKey) {
    terms.push(params.seriesSource ? `series:${params.seriesSource}:${params.seriesKey}` : `series:${params.seriesKey}`);
  }
  const text = terms.filter(Boolean).join(" ");

  const filter = shelfFilter(params, scope);
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
      after: params.offset ? null : decodeShelfCursor(params.cursor),
      // ページ番号の一覧は、旧棚と同じく何件目からかで開く。
      offset: params.offset ?? 0,
      filter,
      count: !params.cursor || Boolean(params.offset),
    },
    // 読みかけの棚。読書位置を旧棚の番号で憶えているあいだは、番号のまま渡して棚の側で読み替える。
    legacyWorks: legacyReadingWorks(params),
    // 作者の指名（前後の作品・作者からの一覧）。棚の側で取得元の ID から引く。
    person: params.personKey ? { provider: params.personSource ?? null, key: params.personKey } : null,
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
    legacyId: card.legacyId,
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
/** 棚の一覧が一度に返す件数の上限。 */
const SHELF_PAGE_LIMIT = 500;

/** 新しい棚の作品を ID で引く（並びは渡した順。外した作品や無い ID は返らない）。 */
export async function shelfEntriesByIds(ids: string[]): Promise<ShelfEntry[]> {
  const unique = [...new Set(ids)];
  const byId = new Map<string, ShelfEntry>();
  // 棚の一覧は一度に 500 件まで。
  for (let start = 0; start < unique.length; start += SHELF_PAGE_LIMIT) {
    const chunk = unique.slice(start, start + SHELF_PAGE_LIMIT);
    const view = await libraryV2Search(
      { text: "", versions: "current", mode: "words" },
      { sort: "saved", descending: true, limit: chunk.length, after: null, filter: { works: chunk }, count: false },
    );
    for (const card of view.cards) byId.set(card.id, shelfEntry(card));
  }
  return ids.flatMap((id) => byId.get(id) ?? []);
}

export async function searchShelf(params: SearchV2Params, scope: ShelfEntityScope = {}): Promise<ShelfSearchResult> {
  const { search, query, legacyWorks, person } = shelfRequest(params, scope);
  const view = await libraryV2Search(search, query, legacyWorks, person);
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
    // ページ番号で開いたページに続きの位置はない（次のページも番号で開く）。
    nextCursor: params.offset ? null : encodeShelfCursor(view.page.next),
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

/**
 * 作者・シリーズの頭を、画面が読む旧棚の形にする。画像はファイルの識別子のまま入れ、画面が
 * `shelfFileUrl` で URL にする（旧棚のパスとは読み方が違う）。
 */
function entityCommon(head: ShelfEntityHead) {
  const first = head.versions[0];
  const last = head.versions[head.versions.length - 1];
  return {
    id: 0,
    source: head.provider,
    sourceKey: head.external_key,
    description: head.description,
    contentHash: null,
    currentVersion: head.version_number ?? 0,
    lastCheckedAt: isoOrNull(head.last_checked_at),
    lastFetchedAt: isoOrNull(head.last_fetched_at),
    createdAt: isoOrNull(first?.saved_at ?? null) ?? "",
    updatedAt: isoOrNull(last?.saved_at ?? null) ?? "",
    workCount: head.work_count,
  };
}

export function shelfPersonEntry(head: ShelfEntityHead): PersonEntry {
  return {
    ...entityCommon(head),
    displayName: head.name,
    iconPath: head.icon,
    coverPath: head.banner,
    linksJson: JSON.stringify(head.links),
  };
}

export function shelfSeriesEntry(head: ShelfEntityHead): SeriesEntry {
  return {
    ...entityCommon(head),
    title: head.name,
    coverPath: head.cover,
    isConcluded: head.is_concluded,
    publishedContentCount: head.published_content_count,
  };
}

/** 保存した版を、画面の履歴の形にする（新しい順）。新しい棚は版ごとの大きさを持たない。 */
export function shelfEntityVersions(head: ShelfEntityHead): EntityVersion[] {
  return [...head.versions].reverse().map((version) => ({
    id: version.version,
    entityType: head.kind,
    source: head.provider,
    sourceKey: head.external_key,
    version: version.version,
    contentHash: null,
    jsonPath: "",
    assetCount: 0,
    fileSizeBytes: 0,
    createdAt: new Date(version.saved_at).toISOString(),
    changeSummary: null,
  }));
}

/** 作者のシリーズの一件を、カードが読む形にする。表紙はファイルの識別子（`shelf: "v2"`）。 */
export function shelfSeriesFacet(item: ShelfSeriesFacet): EntityFacet {
  return {
    shelf: "v2",
    source: item.provider,
    sourceKey: item.external_key,
    displayName: item.name,
    count: item.work_count,
    coverPath: item.cover,
    description: item.description,
    updatedAt: null,
    latestDownloadedAt: isoOrNull(item.latest_saved_at),
    latestSourceUpdatedAt: isoOrNull(item.latest_updated_at),
    sampleTitle: item.sample_title,
    iconPath: null,
    bannerPath: null,
    isConcluded: item.is_concluded,
  };
}

/**
 * ライブラリの「作者・クリエイター」「シリーズ」タブの一ページを新しい棚から引く。
 *
 * 作品の絞り込みは作品の一覧と同じ関数で直す（二つのタブで同じ条件が別の意味にならない）。
 * シリーズの指名は取得元がなくても使える（検索語の `series:キー` と同じ）。行は旧棚の `EntityFacet` の形にし、監視の
 * 登録（移した時点の写し）を `watch` に添える。
 */
export async function searchShelfEntities(
  kind: "person" | "series",
  searchText: string | null,
  pageSize: number,
  offset: number,
  filters: SearchV2Params | null,
  sortBy: EntitySortBy,
  sortOrder: "asc" | "desc" | null,
  scope: EntityFacetScope | null,
): Promise<{ items: ShelfEntityFacet[]; total: number }> {
  const params = filters ?? {};
  const page = await libraryV2Entities(kind, {
    query: searchText,
    filter: shelfFilter(params),
    legacyWorks: legacyReadingWorks(params),
    series: params.seriesKey ? { provider: params.seriesSource ?? null, key: params.seriesKey } : null,
    scope: scope
      ? { watch: scope.watch ?? null, min_work_count: scope.minWorkCount ?? null, concluded: scope.concluded ?? null }
      : null,
    sort: sortBy,
    order: sortOrder,
    offset,
    limit: pageSize,
  });
  return { items: page.items.map(shelfEntityFacet), total: page.total };
}

/** 新しい棚の作者・シリーズの一覧の一件。`watch` は移した時点の更新監視の登録。 */
export type ShelfEntityFacet = EntityFacet & { shelf: "v2"; watch: "watching" | "paused" | null };

/** 一覧の一件を、カードが読む形にする。画像はファイルの識別子（`shelf: "v2"`）。 */
export function shelfEntityFacet(row: ShelfEntityRow): ShelfEntityFacet {
  return {
    shelf: "v2",
    source: row.provider,
    sourceKey: row.external_key,
    displayName: row.name,
    count: row.work_count,
    // 旧棚と同じく、作者はアイコン（なければヘッダー）、シリーズは表紙。
    coverPath: row.icon ?? row.banner ?? row.cover,
    description: row.description,
    updatedAt: isoOrNull(row.updated_at),
    latestDownloadedAt: isoOrNull(row.latest_saved_at),
    latestSourceUpdatedAt: isoOrNull(row.latest_updated_at),
    sampleTitle: row.sample_title,
    iconPath: row.icon,
    bannerPath: row.banner ?? row.cover,
    isConcluded: row.is_concluded,
    watch: row.watch,
  };
}

/** 作者・シリーズに関わるコレクションを、カードが読む形にする（新しい順）。 */
export async function shelfEntityCollections(kind: "person" | "series", entity: string): Promise<WorkCollectionSummary[]> {
  return (await libraryV2EntityCollections(kind, entity)).map(shelfCollectionSummary);
}

/** 作品の入ったコレクション。 */
export async function shelfWorkCollections(work: string): Promise<WorkCollectionSummary[]> {
  return (await libraryV2WorkCollections(work)).map(shelfCollectionSummary);
}

/**
 * 新しい棚のコレクションの要約を、旧棚の形にする。ID は旧棚の ID（コレクションの画面はまだ
 * 旧棚で開く）。表紙はファイルの識別子で、`shelf: "v2"` で読み分ける。
 */
export function shelfCollectionSummary(item: ShelfCollectionSummary): WorkCollectionSummary {
  return {
    shelf: "v2",
    id: item.legacy_id ?? item.id,
    name: item.name,
    description: item.description,
    collectionKind: item.ordered ? "ordered" : "unordered",
    coverDownloadId: null,
    coverPath: item.cover,
    coverMode: item.cover_mode as CollectionCoverMode,
    coverImagePath: item.cover_image,
    coverTiles: item.tiles.map((tile) => ({
      source: tile.provider as SourceKind,
      sourceId: tile.external_key,
      title: tile.title,
      authorName: tile.author_name,
      coverPath: tile.cover,
    })),
    nameSource: item.name_source as CollectionNameSource,
    revision: item.revision,
    memberCount: item.member_count,
    availableCount: item.available_count,
    totalTextLength: item.total_text_length,
    createdAt: new Date(item.created_at).toISOString(),
    updatedAt: new Date(item.updated_at).toISOString(),
  };
}

/** ライブラリの「コレクション」タブの一覧を新しい棚から引く。 */
export async function shelfCollections(): Promise<WorkCollectionSummary[]> {
  return (await libraryV2Collections()).map(shelfCollectionSummary);
}

/**
 * コレクションの画面を新しい棚から引く。回の作品は一覧と同じ `ShelfEntry`（作品の画面・読む画面は
 * 新しい棚の ID で開く）。`downloadId` は旧棚の番号（選んで外す操作は新しい棚には無い）。
 */
export async function shelfCollection(id: string): Promise<WorkCollection> {
  return shelfCollectionView(await libraryV2Collection(id));
}

/**
 * 新しい棚のコレクションを一回編集する（正本に切り替えた棚だけ）。編集した後の中身を返す
 * （消したときは null）。
 */
export async function editShelfCollection(id: string, change: ShelfCollectionChange): Promise<WorkCollection | null> {
  const item = await libraryV2EditCollection(id, change);
  return item ? shelfCollectionView(item) : null;
}

function shelfCollectionView(item: ShelfCollection): WorkCollection {
  const summary = shelfCollectionSummary(item);
  return {
    ...summary,
    members: item.members.map((member) => ({
      collectionId: summary.id,
      source: member.provider as SourceKind,
      sourceId: member.external_key,
      downloadId: member.work?.legacyId ?? null,
      title: member.title,
      authorName: member.author_name,
      coverPath: member.cover,
      textLength: member.text_length,
      position: member.position,
      memberRole: member.role,
      addedBy: member.added_by,
      pinned: member.pinned,
      note: member.note,
      missing: member.missing,
      createdAt: new Date(member.created_at).toISOString(),
      updatedAt: new Date(member.updated_at).toISOString(),
      work: member.work ? shelfEntry(member.work) : null,
      editions: member.editions.map((card) => shelfEntry(card)),
    })),
  };
}
