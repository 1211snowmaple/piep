import type {
  MobileCollection, MobileCollectionMember, MobileEntity, MobileEntityDetail, MobileEntityPage, MobileEntitySeriesPage,
  MobileMetadata, MobilePage, MobilePerson, MobileRevision, MobileSearchPage, MobileSeriesMembership, MobileShelfWork, MobileVersion, MobileWork,
} from "@shared/mobileApi";
import { invoke } from "@tauri-apps/api/core";
import { markFavoriteRead, nextFavoriteRevision } from "./favoriteFreshness";
import { listShelfCollections, listShelfEntities, listShelfEntitySeries, lookupShelfWork, nativeShelf, NO_SHELF, searchShelf, shelfCollectionDetail, shelfEntityDetail, shelfMetadata, shelfSuggestions, shelfWork, shelfWorkCollections } from "./nativeReading";

export interface PcConnectionStatus {
  paired: boolean;
  reachable: boolean;
  url: string | null;
  deviceName: string | null;
  issue: "offline" | "certificate_changed" | "certificate_untrusted" | "secret_lost" | "protocol_mismatch" | "revoked" | "not_ready" | "remote_error" | "no_library" | null;
  capabilities: string[];
  /** 接続先が配信している棚。届かないとき・棚がまだ無いときは無い。 */
  nativeLibrary?: {
    scope: {
      libraryId: string;
      subjectId: string;
      syncEpoch: string;
      connectionGeneration: number;
    };
    readerBundle: boolean;
  };
}

export const pcConnectionIssueText: Record<NonNullable<PcConnectionStatus["issue"]>, string> = {
  offline: "サーバーに接続できません。通信状態と、サーバーが動いているかを確認してください。",
  certificate_changed: "サーバーの証明書が登録時と違います。接続先を確認して登録し直してください。",
  certificate_untrusted: "サーバーの証明書を確認できません。URL が正しいか確かめてください。",
  secret_lost: "この端末に保存した登録を読めません。登録し直してください。",
  protocol_mismatch: "サーバーとの接続形式が合いません。アプリとサーバーを、どちらも新しい版にしてください。",
  revoked: "端末の登録が解除されました。デスクトップ版の「携帯を追加」で登録文字列を出し直して再登録してください。",
  not_ready: "サーバーは本棚の準備中です。しばらくしてからもう一度試してください。",
  remote_error: "サーバーは応答していますが、作品を読み込めません。サーバーの状態を確認してください。",
  no_library: "サーバーにはまだライブラリがありません。デスクトップ版からライブラリを送ってください。",
};

export function pcConnectionIssue(status: PcConnectionStatus): string | null {
  if (!status.issue) return null;
  return pcConnectionIssueText[status.issue];
}

// サーバーから届く型は shared/mobileApi.ts（Rust の定義から生成）にある。
// ここでは、画面だけが持つ項目と、端末に残っている古い保存分には無い項目を足し引きする。

type Older<T, K extends keyof T> = Omit<T, K> & Partial<Pick<T, K>>;

export type PcWork = Omit<Older<MobileWork, "sourceCreatedAt" | "downloadedAt">, "id"> & {
  /** 棚の作品の UUID。 */
  id: string;
  /** 棚の版と編集。`favoriteGeneration` はお気に入りの更新世代（十進の文字列）。 */
  shelf?: { snapshotId: string; edited: boolean; editId?: string | null; favoriteGeneration?: string };
  /** フロント側だけの取得順。お気に入りの書き込みより新しい応答を見分ける。 */
  favoriteReadSequence?: number;
};

/** 端末に保存した版が、今サーバーにある版と同じか。版と編集の UUID で見る。 */
export function sameSavedVersion(work: PcWork, saved: Pick<OfflineWork, "work">): boolean {
  return Boolean(work.shelf && saved.work.shelf)
    && work.shelf!.snapshotId === saved.work.shelf!.snapshotId
    && (work.shelf!.editId ?? null) === (saved.work.shelf!.editId ?? null);
}

/** 棚に届かない接続で、棚を使う操作をしたとき。 */
const noShelf = () => Promise.reject(NO_SHELF);

export type PcEntity = MobileEntity;

export type PcEntityPage = MobileEntityPage;

export type PcEntitySeriesPage = MobileEntitySeriesPage;

export type PcEntityDetail = Omit<MobileEntityDetail, "collections"> & { collections: PcCollection[] };

export type PcCollection = Older<Omit<MobileCollection, "coverMode" | "position">, "coverRevision" | "hasSingleCover" | "hasFileCover" | "coverTiles"> & {
  coverMode?: "mosaic" | "spine" | "single" | "sigil" | "file";
  position?: number | null;
  /** コレクションの棚の UUID。表紙は棚から引く。 */
  shelfLibrary?: string;
};

export type PcCollectionDetail = PcCollection & {
  members: (Omit<MobileCollectionMember, "work"> & { work: PcWork | null })[];
};

export interface PcWorkScope {
  kind: "person" | "series";
  source: string;
  key: string;
}

export type PcSearchPage = Omit<MobileSearchPage, "items"> & { items: PcWork[] };

export interface PcSearchCriteria {
  text: string;
  source: "" | "pixiv" | "fanbox";
  contentType: "" | "novel" | "article" | "text" | "image" | "file";
  author: string;
  authorExclude: string;
  favorite: boolean;
  tag: string;
  tagExclude: string;
  tagMode: "and" | "or";
  minChars: string;
  maxChars: string;
  minAssets: string;
  maxAssets: string;
  revision: "" | "revised" | "first_version";
  edit: "" | "edited" | "unedited";
  cover: "" | "has_cover" | "no_cover";
  dateField: "downloaded_at" | "source_created_at" | "source_updated_at";
  dateFrom: string;
  dateTo: string;
  // remembered は画面だけの値で「検索ボタン横のいつもの順に従う」。送る前に実際の順へ直す。
  sort: PcSort | "remembered";
}

// standard はサーバーの標準の順。検索語があれば関連度、シリーズの中ならシリーズの順になる。
export type PcSort = "standard" | "dateDesc" | "dateAsc" | "publishedDesc" | "publishedAsc" | "titleAsc" | "titleDesc" | "authorAsc" | "authorDesc" | "lengthDesc" | "lengthAsc" | "assetsDesc" | "assetsAsc" | "versionDesc" | "versionAsc";

/** 作者やシリーズの中だけで使える順を足したもの。 */
export type PcWorkSort = PcSort | "seriesAsc" | "seriesDesc";
export type PcWorkSearch = Omit<PcSearchCriteria, "sort"> & { sort: PcWorkSort };

export const defaultPcSearch: PcSearchCriteria = { text: "", source: "", contentType: "", author: "", authorExclude: "", favorite: false, tag: "", tagExclude: "", tagMode: "and", minChars: "", maxChars: "", minAssets: "", maxAssets: "", revision: "", edit: "", cover: "", dateField: "downloaded_at", dateFrom: "", dateTo: "", sort: "remembered" };

/** 作品の保存した版。その版の写しの ID を添える（以前の版を棚から開く）。 */
export type PcVersion = MobileVersion & { snapshotId?: string };

export type PcMetadata = Omit<MobileMetadata, "work" | "people" | "series" | "versions"> & {
  work: PcWork;
  versions: PcVersion[];
  people?: PcPerson[];
  series?: PcSeriesMembership[];
};

export type PcPerson = MobilePerson;

export type PcSeriesMembership = MobileSeriesMembership;

export type PcPage = MobilePage;

export type PcRevision = MobileRevision;

export interface OfflinePage extends PcPage {
  generation: string;
}

export interface PcOutlineEntry {
  page: number;
  index: number;
  title: string;
}

export interface PcAsset {
  mimeType: string;
  base64: string;
}

export interface OfflineWork {
  work: PcWork;
  /** 保存した棚の UUID。 */
  shelfLibrary?: string;
  people?: PcPerson[];
  series?: PcSeriesMembership[];
  savedAtMs: number;
  savedVersion: number;
  savedContentHash: string | null;
  savedEditHash: string | null;
  pageCount: number;
  collections?: PcCollection[];
  collectionPositions?: Record<string, number>;
  collectionCoverIds?: string[];
  hasAuthorCover?: boolean;
  hasSeriesCover?: boolean;
  authorCoverEntity?: { source: string; key: string } | null;
  seriesCoverEntity?: { source: string; key: string } | null;
}

/** 保存した作品の本文で見つかった箇所。page は 0 から数える。pages は語を含むページの数。 */
export interface OfflineTextHit {
  workId: string;
  page: number;
  snippet: string;
  pages: number;
}

export interface OfflineIssue {
  workId: string;
  issue: "offline_storage";
}

export const getPcStatus = () => invoke<PcConnectionStatus>("pc_connection_status");
export const pairPc = (payload: string, deviceName: string) => invoke<PcConnectionStatus>("pc_pair", { payload, deviceName });
export const disconnectPc = () => invoke<PcConnectionStatus>("pc_disconnect");
export const searchPcWorks = (criteria: PcWorkSearch, cursor: string | null = null, scope: PcWorkScope | null = null, offset: number | null = null, limit: number | null = null) => {
  const revision = nextFavoriteRevision();
  // 画面表示用の名前などを Rust コマンドへ渡さない。入力は三つの識別子だけ。
  const entity = scope ? { kind: scope.kind, source: scope.source, key: scope.key } : null;
  const library = nativeShelf();
  if (!library) return noShelf();
  return searchShelf(library, criteria, cursor, entity, offset, limit).then((page) => {
    for (const work of page.items) markFavoriteRead(work, revision);
    return page;
  });
};
export const listPcEntities = (kind: "people" | "series", text = "", offset = 0, sort?: { sort: string; order: string }) => {
  const library = nativeShelf();
  return library ? listShelfEntities(library, kind, text, offset, sort) : noShelf();
};
export const getPcEntityDetail = (kind: "people" | "series", source: string, key: string) => {
  const library = nativeShelf();
  return library ? shelfEntityDetail(library, kind, source, key) : noShelf();
};
export const listPcEntitySeries = (source: string, key: string, text: string, cursor: string | null, sort?: { sort: string; order: string }) => {
  const library = nativeShelf();
  return library ? listShelfEntitySeries(library, source, key, text, cursor, sort) : noShelf();
};
export const listPcCollections = () => {
  const library = nativeShelf();
  return library ? listShelfCollections(library) : noShelf();
};
export const getPcWorkCollections = (workId: string) => {
  const library = nativeShelf();
  return library ? shelfWorkCollections(library, workId) : Promise.resolve<PcCollection[]>([]);
};
export const getPcCollection = (collectionId: string) => {
  const revision = nextFavoriteRevision();
  const library = nativeShelf();
  if (!library) return noShelf();
  return shelfCollectionDetail(library, collectionId).then((collection) => {
    for (const member of collection.members) if (member.work) markFavoriteRead(member.work, revision);
    return collection;
  });
};
export const getPcMetadata = (workId: string) => {
  const revision = nextFavoriteRevision();
  return shelfMetadata(workId).then((metadata) => {
    markFavoriteRead(metadata.work, revision);
    return metadata;
  });
};
export interface SearchSuggestion {
  kind: string;
  label: string;
  value: string;
  count: number | null;
  exactMatch?: boolean;
  source?: string | null;
  sourceKey?: string | null;
}
/** 検索欄の候補（題の候補の `sourceKey` は棚の作品の ID）。 */
export const searchPcSuggestions = (text: string): Promise<SearchSuggestion[]> => {
  const shelf = nativeShelf();
  return shelf ? shelfSuggestions(shelf, text) : Promise.resolve([]);
};
/** 取得元と ID から棚の作品を引く（本文の中のリンク・題の候補）。棚に無いときは失敗になる。 */
export const lookupPcWork = (source: string, key: string): Promise<PcWork | null> => {
  const shelf = nativeShelf();
  return shelf ? lookupShelfWork(shelf, source, key) : Promise.resolve(null);
};
/** Rust が返す保存版の一覧の行。作品は棚の一覧と同じ形で残っている。 */
type StoredOfflineWork = Omit<OfflineWork, "work"> & { work: MobileShelfWork };

/** 保存版の行を画面の形にする。作品は一覧の一件と同じ変換を通す。 */
const offlineWork = (stored: StoredOfflineWork): OfflineWork => ({ ...stored, work: shelfWork(stored.work) });

export const listOfflineWorks = () => invoke<StoredOfflineWork[]>("offline_list_works").then((works) => works.map(offlineWork));
/** 端末に保存する。今つながっている棚から、読む画面と同じ組み立てで保存する。 */
export const saveOfflineWork = (workId: string) => {
  const library = nativeShelf();
  if (!library) return noShelf();
  return invoke<StoredOfflineWork>("offline_save_shelf_work", { library, work: workId }).then(offlineWork);
};
export const getOfflinePage = (workId: string, page: number) => invoke<OfflinePage>("offline_get_page", { workId, page });
export const getOfflineOutline = (workId: string, generation: string) => invoke<PcOutlineEntry[]>("offline_get_outline", { workId, generation });
export const getOfflineAsset = (workId: string, assetId: number, generation: string) => invoke<PcAsset>("offline_get_asset", { workId, assetId, generation });
export const deleteOfflineWork = (work: Pick<PcWork, "id" | "source" | "sourceId">) => invoke<void>("offline_delete_work", { workId: work.id, source: work.source, sourceId: work.sourceId });
/** 同じ作品かは取得元の識別子まで見て決める。 */
export const sameWork = (left: Pick<PcWork, "id" | "source" | "sourceId">, right: Pick<PcWork, "id" | "source" | "sourceId">) => left.id === right.id && left.source === right.source && left.sourceId === right.sourceId;
export const searchOfflineText = (query: string) => invoke<OfflineTextHit[]>("offline_search_text", { query });
export const listOfflineIssues = () => invoke<OfflineIssue[]>("offline_list_issues");
export const deleteOfflineIssue = (workId: string) => invoke<void>("offline_delete_corrupt_work", { workId });

export function pcErrorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error && typeof error.message === "string") return error.message;
  return typeof error === "string" ? error : "サーバーとの通信に失敗しました";
}
