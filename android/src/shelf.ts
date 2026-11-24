import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import type { MobileCollection, MobileEntityDetail, MobileSearchSuggestions, MobileCollectionDetail, MobileWorkPage, MobileWork, MobileWorkDetail } from "@shared/mobileApi";
import type { PcCollection, PcCollectionDetail, PcConnectionStatus, PcEntityDetail, PcEntityPage, PcEntitySeriesPage, PcMetadata, PcOutlineEntry, PcPage, PcSearchPage, PcWork, PcWorkSearch, ReadingScope, SearchSuggestion } from "./pcApi";

export type ShelfScope = NonNullable<PcConnectionStatus["shelf"]>["scope"];

/** 棚から開いた一冊。ページと画像は端末の中で組んである。 */
export interface OpenedShelfWork {
  key: string;
  entityId: string;
  view: { kind: "snapshot" | "edit"; id: string };
  edited: boolean;
  pageCount: number;
  sourcePageStarts: number[];
  totalPlainTextChars: number;
  outline: PcOutlineEntry[];
}

let scope: ShelfScope | null = null;
const opened = new Map<string, Promise<OpenedShelfWork>>();

/** 棚に届かない接続で、棚を使う操作をしたとき。 */
export const NO_SHELF = { code: "library_changed", message: "接続先の本棚が変わりました。一覧を開き直してください" };
const READER_VIEW_CHANGED = { code: "reader_view_changed", message: "本文が変わりました。もう一度読み込んでください" };

function shelfKey(shelf: ShelfScope): string {
  return `${shelf.libraryId}/${shelf.subjectId}/${shelf.syncEpoch}/${shelf.connectionGeneration}`;
}

/**
 * 接続先が配信している棚を使う。接続や棚が変わったら、前の棚で開いたものは使わない。
 */
export function adoptShelf(status: PcConnectionStatus | null): void {
  const next = status?.reachable && status.shelf?.readerBundle ? status.shelf.scope : null;
  if (next && scope && shelfKey(next) === shelfKey(scope)) return;
  scope = next;
  opened.clear();
}

/** 今つながっている棚。棚に届かない接続では null。 */
export function connectedShelf(): ShelfScope | null {
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
export function setShelfFavorite(library: ShelfScope, work: string, expected: string, favorite: boolean): Promise<ShelfFavorite> {
  return invoke<ShelfFavorite>("pc_set_favorite", { library, work, request: crypto.randomUUID(), expected, favorite });
}

export function isCurrentShelf(library: ShelfScope): boolean {
  const current = connectedShelf();
  return current !== null && shelfKey(library) === shelfKey(current);
}

/** 一覧の問い合わせの鍵に足す。棚や接続が変われば、一覧も取り直す。 */
export function connectedShelfKey(): string | null {
  const shelf = connectedShelf();
  return shelf ? shelfKey(shelf) : null;
}

export async function shelfSuggestions(library: ShelfScope, text: string): Promise<SearchSuggestion[]> {
  const found = await invoke<MobileSearchSuggestions>("pc_suggest", { library, text });
  return found.items;
}

/** 取得元と ID から棚の作品を引く。棚に無ければ失敗する。 */
export async function lookupShelfWork(library: ShelfScope, source: string, key: string): Promise<PcWork> {
  return shelfWork(await invoke<MobileWork>("pc_lookup_work", { library, provider: source, key }), library);
}

/** 棚のコレクション。表紙は棚から引くので、棚の UUID を添える。 */
function shelfCollection(collection: MobileCollection, library: ShelfScope): PcCollection {
  return { ...collection, coverMode: collection.coverMode as PcCollection["coverMode"], shelfLibrary: library.libraryId };
}

export async function listShelfEntities(library: ShelfScope, kind: "people" | "series", text: string, offset: number, sort?: { sort: string; order: string }): Promise<PcEntityPage> {
  return invoke<PcEntityPage>("pc_entities", { library, kind, text, offset, sort: sort?.sort ?? null, order: sort?.order ?? null });
}

export async function shelfEntityDetail(library: ShelfScope, kind: "people" | "series", source: string, key: string): Promise<PcEntityDetail> {
  const detail = await invoke<MobileEntityDetail>("pc_entity_detail", { library, kind, source, key });
  return { ...detail, collections: detail.collections.map((collection) => shelfCollection(collection, library)) };
}

export async function listShelfEntitySeries(library: ShelfScope, source: string, key: string, text: string, cursor: string | null, sort?: { sort: string; order: string }): Promise<PcEntitySeriesPage> {
  return invoke<PcEntitySeriesPage>("pc_entity_series", { library, source, key, text, cursor, sort: sort?.sort ?? null, order: sort?.order ?? null });
}

export async function listShelfCollections(library: ShelfScope): Promise<PcCollection[]> {
  const items = await invoke<MobileCollection[]>("pc_collections", { library });
  return items.map((collection) => shelfCollection(collection, library));
}

export async function shelfWorkCollections(library: ShelfScope, work: string): Promise<PcCollection[]> {
  const items = await invoke<MobileCollection[]>("pc_work_collections", { library, work });
  return items.map((collection) => shelfCollection(collection, library));
}

/** コレクションの画面。`collection` は棚の UUID。 */
export async function shelfCollectionDetail(library: ShelfScope, collection: string): Promise<PcCollectionDetail> {
  const { members, ...summary } = await invoke<MobileCollectionDetail>("pc_collection", { library, collection });
  return { ...shelfCollection(summary, library), members: members.map((member) => ({ ...member, work: member.work ? shelfWork(member.work, library) : null })) };
}

/** 棚の作者のアイコン・ヘッダー、シリーズの表紙。 */
export function shelfEntityCoverPath(kind: "people" | "series", source: string, key: string, banner: boolean): string | null {
  const shelf = connectedShelf();
  return shelf ? `shelf-entity/${shelf.libraryId}/${kind}/${source}/${encodeURIComponent(key)}${banner ? "/banner" : ""}` : null;
}

/** 作品を取得したときの棚と利用者を、読書記録のために添える。 */
export function shelfWork(work: MobileWork, { libraryId, subjectId }: ReadingScope): PcWork {
  return { ...work, readingScope: { libraryId, subjectId } };
}

/** 一覧の一ページを棚から引く。 */
export async function searchShelf(library: ShelfScope, criteria: PcWorkSearch, cursor: string | null, entity: { kind: string; source: string; key: string } | null, offset: number | null, limit: number | null): Promise<PcSearchPage> {
  const page = await invoke<MobileWorkPage>("pc_search", { library, criteria, cursor, offset, limit, scope: entity });
  return { items: page.items.map((work) => shelfWork(work, library)), nextCursor: page.nextCursor, totalEstimate: page.totalEstimate, indexComplete: page.indexComplete };
}

/** 作品の画面を棚から引く。版の一覧には、以前の版を開くための版ごとの写しの ID を添える。 */
export async function shelfMetadata(workId: string): Promise<PcMetadata> {
  const library = connectedShelf();
  if (!library) throw NO_SHELF;
  const detail = await invoke<MobileWorkDetail>("pc_work", { library, work: workId });
  return {
    work: shelfWork(detail.work, library),
    people: detail.people,
    series: detail.series,
    versions: detail.versions,
  };
}

/** 棚の作品の表紙。版の UUID を鍵にして、版が変われば取り直す。 */
export function shelfCoverPath(work: PcWork, large: boolean): string | null {
  const shelf = connectedShelf();
  if (!shelf) return null;
  return `shelf/${shelf.libraryId}/${work.id}/${work.snapshotId}${large ? "/large" : ""}`;
}

/**
 * 作品情報で確定した保存版か編集を、その情報を引いた棚で開く。
 * `fresh` は開き直すとき（端末が覚えておける冊数を超えて、組んだものが消えたとき）。
 */
export function openShelfWork(library: ShelfScope, work: Pick<PcWork, "id" | "snapshotId" | "editId">, fresh = false): Promise<OpenedShelfWork> {
  if (!isCurrentShelf(library)) return Promise.reject(NO_SHELF);
  const view: OpenedShelfWork["view"] = work.editId
    ? { kind: "edit", id: work.editId }
    : { kind: "snapshot", id: work.snapshotId };
  const id = `${shelfKey(library)}/${work.id}/${view.kind}/${view.id}`;
  const found = opened.get(id);
  if (found && !fresh) return found;
  const next = invoke<OpenedShelfWork>("pc_open", { scope: library, work: work.id, view }).then((document) => {
    if (!isCurrentShelf(library)) throw NO_SHELF;
    if (document.entityId !== work.id || document.view.kind !== view.kind || document.view.id !== view.id) throw READER_VIEW_CHANGED;
    return document;
  });
  opened.set(id, next);
  // 失敗した要求は覚えない。同じ鍵を開き直していたら、新しい要求を残す。
  void next.catch(() => { if (opened.get(id) === next) opened.delete(id); });
  return next;
}

export const shelfPage = (document: OpenedShelfWork, page: number) => invoke<PcPage>("pc_page", { key: document.key, page });

/** 本文の挿絵。部分実体の受け取りとハッシュの確認は Rust 側で済ませる。 */
export function shelfImageUrl(document: OpenedShelfWork, assetId: number): string | null {
  if (!isTauri()) return null;
  return convertFileSrc(`shelf-page/${document.key}/${assetId}`, "pcimg");
}
