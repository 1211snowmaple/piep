import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import type { MobileCollection, MobileEntityDetail, MobileSearchSuggestions, MobileShelfCollectionDetail, MobileShelfPage, MobileShelfWork, MobileShelfWorkDetail } from "@shared/mobileApi";
import type { PcCollection, PcCollectionDetail, PcConnectionStatus, PcEntityDetail, PcEntityPage, PcEntitySeriesPage, PcMetadata, PcOutlineEntry, PcPage, PcSearchPage, PcWork, PcWorkSearch, SearchSuggestion } from "./pcApi";

type NativeScope = NonNullable<PcConnectionStatus["nativeLibrary"]>["scope"];

/** 棚から開いた一冊。ページと画像は端末の中で組んである。 */
export interface NativeOpened {
  key: string;
  entityId: string;
  view: { kind: "snapshot" | "edit"; id: string };
  edited: boolean;
  pageCount: number;
  sourcePageStarts: number[];
  totalPlainTextChars: number;
  outline: PcOutlineEntry[];
}

let scope: NativeScope | null = null;
const opened = new Map<string, Promise<NativeOpened | null>>();

/** 棚に届かない接続で、棚を使う操作をしたとき。 */
export const NO_SHELF = { code: "library_changed", message: "接続先の本棚が変わりました。一覧を開き直してください" };

/**
 * 接続先が配信している棚を使う。接続や棚が変わったら、前の棚で開いたものは使わない。
 */
export function adoptNativeLibrary(status: PcConnectionStatus | null): void {
  const next = status?.reachable && status.nativeLibrary?.readerBundle ? status.nativeLibrary.scope : null;
  const same = next && scope
    && next.libraryId === scope.libraryId
    && next.subjectId === scope.subjectId
    && next.syncEpoch === scope.syncEpoch
    && next.connectionGeneration === scope.connectionGeneration;
  if (same) return;
  scope = next;
  opened.clear();
}

/** 今つながっている棚。棚に届かない接続では null。 */
export function nativeShelf(): NativeScope | null {
  return scope && isTauri() ? scope : null;
}

/** お気に入りを付け外しした結果。`conflict` は別の端末やデスクトップで先に変わっていたとき。 */
export interface ShelfFavorite {
  favorite: boolean;
  generation: string;
  conflict: boolean;
}

/**
 * 棚の作品のお気に入りを付け外しする。押すたびに新しい要求 UUID を作る（同じ要求を送り直しても
 * 一度だけ効く）。`expected` は画面が知っている更新世代で、違えば今の値が返る。
 */
export function setShelfFavorite(library: NativeScope, work: string, expected: string, favorite: boolean): Promise<ShelfFavorite> {
  return invoke<ShelfFavorite>("pc_native_set_favorite", { library, work, request: crypto.randomUUID(), expected, favorite });
}

/** 一覧の問い合わせの鍵に足す。棚や接続が変われば、一覧も取り直す。 */
export function nativeShelfKey(): string | null {
  const shelf = nativeShelf();
  return shelf ? `${shelf.libraryId}/${shelf.subjectId}/${shelf.syncEpoch}/${shelf.connectionGeneration}` : null;
}

export async function shelfSuggestions(library: NativeScope, text: string): Promise<SearchSuggestion[]> {
  const found = await invoke<MobileSearchSuggestions>("pc_native_suggest", { library, text });
  return found.items;
}

/** 取得元と ID から棚の作品を引く。棚に無ければ失敗する。 */
export async function lookupShelfWork(library: NativeScope, source: string, key: string): Promise<PcWork> {
  return shelfWork(await invoke<MobileShelfWork>("pc_native_lookup_work", { library, provider: source, key }));
}

/** 棚のコレクション。表紙は棚から引くので、棚の UUID を添える。 */
function shelfCollection(collection: MobileCollection, library: NativeScope): PcCollection {
  return { ...collection, coverMode: collection.coverMode as PcCollection["coverMode"], shelfLibrary: library.libraryId };
}

export async function listShelfEntities(library: NativeScope, kind: "people" | "series", text: string, offset: number, sort?: { sort: string; order: string }): Promise<PcEntityPage> {
  return invoke<PcEntityPage>("pc_native_entities", { library, kind, text, offset, sort: sort?.sort ?? null, order: sort?.order ?? null });
}

export async function shelfEntityDetail(library: NativeScope, kind: "people" | "series", source: string, key: string): Promise<PcEntityDetail> {
  const detail = await invoke<MobileEntityDetail>("pc_native_entity_detail", { library, kind, source, key });
  return { ...detail, collections: detail.collections.map((collection) => shelfCollection(collection, library)) };
}

export async function listShelfEntitySeries(library: NativeScope, source: string, key: string, text: string, cursor: string | null, sort?: { sort: string; order: string }): Promise<PcEntitySeriesPage> {
  return invoke<PcEntitySeriesPage>("pc_native_entity_series", { library, source, key, text, cursor, sort: sort?.sort ?? null, order: sort?.order ?? null });
}

export async function listShelfCollections(library: NativeScope): Promise<PcCollection[]> {
  const items = await invoke<MobileCollection[]>("pc_native_collections", { library });
  return items.map((collection) => shelfCollection(collection, library));
}

export async function shelfWorkCollections(library: NativeScope, work: string): Promise<PcCollection[]> {
  const items = await invoke<MobileCollection[]>("pc_native_work_collections", { library, work });
  return items.map((collection) => shelfCollection(collection, library));
}

/** コレクションの画面。`collection` は棚の UUID か、棚へ移したときの元の ID。 */
export async function shelfCollectionDetail(library: NativeScope, collection: string): Promise<PcCollectionDetail> {
  const { members, ...summary } = await invoke<MobileShelfCollectionDetail>("pc_native_collection", { library, collection });
  return { ...shelfCollection(summary, library), members: members.map((member) => ({ ...member, work: member.work ? shelfWork(member.work) : null })) };
}

/** 棚の作者のアイコン・ヘッダー、シリーズの表紙。 */
export function shelfEntityCoverPath(kind: "people" | "series", source: string, key: string, banner: boolean): string | null {
  const shelf = nativeShelf();
  return shelf ? `shelf-entity/${shelf.libraryId}/${kind}/${source}/${encodeURIComponent(key)}${banner ? "/banner" : ""}` : null;
}

/** 棚の一件を、カードと画面が読む形にする。作品は棚の UUID のまま。 */
export function shelfWork(work: MobileShelfWork): PcWork {
  return {
    id: work.id,
    shelf: { snapshotId: work.snapshotId, edited: work.edited, editId: work.editId ?? null, favoriteGeneration: work.favoriteGeneration ?? "0" },
    source: work.source,
    sourceId: work.sourceId,
    title: work.title,
    authorName: work.authorName,
    authorId: work.authorId,
    contentType: work.contentType,
    tags: work.tags,
    excerpt: work.excerpt,
    textLength: work.textLength,
    assetCount: work.assetCount,
    hasCover: work.hasCover,
    currentVersion: work.currentVersion,
    // 内容の印（ハッシュ）は棚にない。作りものの値で埋めない。
    contentHash: null,
    favorite: work.favorite,
    seriesTitle: work.seriesTitle,
    seriesId: work.seriesId,
    sourceCreatedAt: work.sourceCreatedAt,
    downloadedAt: work.downloadedAt,
  };
}

/** 一覧の一ページを棚から引く。 */
export async function searchShelf(library: NativeScope, criteria: PcWorkSearch, cursor: string | null, entity: { kind: string; source: string; key: string } | null, offset: number | null, limit: number | null): Promise<PcSearchPage> {
  const page = await invoke<MobileShelfPage>("pc_native_search", { library, criteria, cursor, offset, limit, scope: entity });
  return { items: page.items.map(shelfWork), nextCursor: page.nextCursor, totalEstimate: page.totalEstimate, indexComplete: page.indexComplete };
}

/** 作品の画面を棚から引く。版の一覧には、以前の版を開くための版ごとの写しの ID を添える。 */
export async function shelfMetadata(workId: string): Promise<PcMetadata> {
  const library = nativeShelf();
  if (!library) throw NO_SHELF;
  const detail = await invoke<MobileShelfWorkDetail>("pc_native_work", { library, work: workId });
  return {
    work: shelfWork(detail.work),
    people: detail.people,
    series: detail.series,
    versions: detail.versions.map((version) => ({
      version: version.version,
      // 内容の印は棚にない。版は写しの ID で見分ける。
      contentHash: null,
      textLength: version.textLength ?? 0,
      assetCount: version.version === detail.work.currentVersion ? detail.work.assetCount : 0,
      createdAt: version.savedAt,
      snapshotId: version.snapshotId,
    })),
    isEdited: detail.work.edited,
    activeEditId: null,
    // 編集の中身の印は渡らない。編集の UUID で読書位置と保存版を分ける（編集し直せば別の本文）。
    activeEditHash: shelfEditHash(detail.work),
  };
}

/** 編集の印。端末に保存した版の印（Rust 側の `edit:{UUID}`）と同じ形。 */
export function shelfEditHash(work: Pick<MobileShelfWork, "edited" | "editId" | "snapshotId">): string | null {
  if (!work.edited) return null;
  return work.editId ? `edit:${work.editId}` : `edit:${work.snapshotId}`;
}

/** 棚の作品の表紙。版の UUID を鍵にして、版が変われば取り直す。 */
export function shelfCoverPath(work: PcWork, large: boolean): string | null {
  const shelf = nativeShelf();
  if (!shelf || !work.shelf) return null;
  return `shelf/${shelf.libraryId}/${work.id}/${work.shelf.snapshotId}${large ? "/large" : ""}`;
}

/**
 * 作品を棚で開く。棚に無い作品や、棚に届かない接続では null。
 * `fresh` は開き直すとき（端末が覚えておける冊数を超えて、組んだものが消えたとき）。
 */
export function openNative(work: Pick<PcWork, "source" | "sourceId">, fresh = false): Promise<NativeOpened | null> {
  const current = scope;
  if (!current || !isTauri()) return Promise.resolve(null);
  const id = `${work.source}:${work.sourceId}`;
  const found = opened.get(id);
  if (found && !fresh) return found;
  const next = invoke<NativeOpened>("pc_native_open", { scope: current, provider: work.source, key: work.sourceId })
    .catch(() => null);
  opened.set(id, next);
  // 開けなかったことは覚えない。次に開いたときに確かめ直す。
  void next.then((result) => { if (!result && opened.get(id) === next) opened.delete(id); });
  return next;
}

/** 作品の保存した版の一つ（版ごとの写し）を棚で開く。`fresh` は開き直すとき。 */
export function openNativeSnapshot(work: string, snapshot: string, fresh = false): Promise<NativeOpened | null> {
  const current = nativeShelf();
  if (!current) return Promise.resolve(null);
  const id = `snapshot:${work}:${snapshot}`;
  const found = opened.get(id);
  if (found && !fresh) return found;
  const next = invoke<NativeOpened>("pc_native_open_snapshot", { scope: current, work, snapshot })
    .catch(() => null);
  opened.set(id, next);
  void next.then((result) => { if (!result && opened.get(id) === next) opened.delete(id); });
  return next;
}

export const nativePage = (document: NativeOpened, page: number) => invoke<PcPage>("pc_native_page", { key: document.key, page });

/** 本文の挿絵。部分実体の受け取りとハッシュの確認は Rust 側で済ませる。 */
export function nativeImageUrl(document: NativeOpened, assetId: number): string | null {
  if (!isTauri()) return null;
  return convertFileSrc(`native/${document.key}/${assetId}`, "pcimg");
}
