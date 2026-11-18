import type {
  MobileCollection, MobileCollectionMember, MobileEntity, MobileEntityDetail, MobileEntityPage, MobileEntitySeriesPage,
  MobileMetadata, MobilePage, MobilePerson, MobileRevision, MobileSearchPage, MobileSeriesMembership, MobileShelfWork, MobileWork,
} from "@shared/mobileApi";
import { invoke } from "@tauri-apps/api/core";
import { markFavoriteRead, nextFavoriteRevision } from "./favoriteFreshness";
import { groupShelf, listShelfCollections, listShelfEntities, listShelfEntitySeries, nativeShelf, searchShelf, shelfCollectionDetail, shelfEntityDetail, shelfMetadata, shelfWork, shelfWorkCollections, SHELF_NOT_YET } from "./nativeReading";

export interface PcConnectionStatus {
  paired: boolean;
  reachable: boolean;
  url: string | null;
  deviceName: string | null;
  issue: "offline" | "certificate_changed" | "certificate_untrusted" | "secret_lost" | "protocol_mismatch" | "revoked" | "not_ready" | "remote_error" | null;
  capabilities: string[];
  nativeLibrary?: {
    scope: {
      libraryId: string;
      subjectId: string;
      syncEpoch: string;
      connectionGeneration: number;
    };
    worksList: boolean;
    /** 一覧・作品の画面・表紙を新しい棚から出せる。 */
    worksSearch?: boolean;
    /** 棚の作品のお気に入りを付け外しできる。 */
    favoriteWrite?: boolean;
    /** 作者・シリーズ・コレクションの画面を新しい棚から出せる。 */
    groups?: boolean;
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
};

export function pcConnectionIssue(status: PcConnectionStatus): string | null {
  if (!status.issue) return null;
  return pcConnectionIssueText[status.issue];
}

// サーバーから届く型は shared/mobileApi.ts（Rust の定義から生成）にある。
// ここでは、画面だけが持つ項目と、端末に残っている古い保存分には無い項目を足し引きする。

type Older<T, K extends keyof T> = Omit<T, K> & Partial<Pick<T, K>>;

export type PcWork = Omit<Older<MobileWork, "sourceCreatedAt" | "downloadedAt">, "id"> & {
  /** 旧 API の作品は数の ID、新しい棚の作品は棚の UUID（文字列）。 */
  id: number | string;
  /** 新しい棚の作品だけが持つ。`favoriteGeneration` はお気に入りの更新世代（十進の文字列）。 */
  shelf?: { snapshotId: string; edited: boolean; editId?: string | null; favoriteGeneration?: string };
  /** フロント側だけの取得順。お気に入りの書き込みより新しい応答を見分ける。 */
  favoriteReadSequence?: number;
};

/** 新しい棚の作品か（UUID で指す）。旧 API の番号を使う操作には渡さない。 */
export function isShelfWork(work: Pick<PcWork, "id">): boolean {
  return typeof work.id === "string";
}

/** 旧 API の番号。旧 API の挿絵など、番号でしか指せない操作の前に使う。 */
export function v1WorkId(work: Pick<PcWork, "id">): number {
  if (typeof work.id !== "number") throw new Error(SHELF_NOT_YET);
  return work.id;
}

/**
 * 端末に保存した版が、今サーバーにある版と同じか。新しい棚の作品は版と編集の UUID で、
 * 旧 API の作品は版の番号と内容の印で見る。
 */
export function sameSavedVersion(work: PcWork, saved: Pick<OfflineWork, "work" | "savedVersion" | "savedContentHash">): boolean {
  if (work.shelf || saved.work.shelf) {
    return Boolean(work.shelf && saved.work.shelf)
      && work.shelf!.snapshotId === saved.work.shelf!.snapshotId
      && (work.shelf!.editId ?? null) === (saved.work.shelf!.editId ?? null);
  }
  return work.currentVersion === saved.savedVersion && work.contentHash === saved.savedContentHash;
}

/** 新しい棚の作品ではまだ使えない操作。 */
const notYet = () => Promise.reject({ code: "shelf_not_yet", message: SHELF_NOT_YET });

export type PcEntity = MobileEntity;

export type PcEntityPage = MobileEntityPage;

export type PcEntitySeriesPage = MobileEntitySeriesPage;

export type PcEntityDetail = Omit<MobileEntityDetail, "collections"> & { collections: PcCollection[] };

export type PcCollection = Older<Omit<MobileCollection, "coverMode" | "position">, "coverRevision" | "hasSingleCover" | "hasFileCover" | "coverTiles"> & {
  coverMode?: "mosaic" | "spine" | "single" | "sigil" | "file";
  position?: number | null;
  /** 新しい棚のコレクションなら、その棚の UUID。表紙は棚から引く。 */
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

export type PcMetadata = Omit<MobileMetadata, "work" | "people" | "series"> & {
  work: PcWork;
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
  /** 新しい棚の作品なら、その棚の UUID。 */
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
  workId: number | string;
  page: number;
  snippet: string;
  pages: number;
}

export interface OfflineIssue {
  workId: number | string;
  issue: "offline_storage";
}

export const getPcStatus = () => invoke<PcConnectionStatus>("pc_connection_status");
export const pairPc = (payload: string, deviceName: string) => invoke<PcConnectionStatus>("pc_pair", { payload, deviceName });
export const disconnectPc = () => invoke<PcConnectionStatus>("pc_disconnect");
/** お気に入りを付け外しする。返るのはサーバーが残した値。 */
export const setPcFavorite = (source: string, sourceId: string, favorite: boolean) => invoke<{ source: string; sourceId: string; favorite: boolean }>("pc_set_favorite", { source, sourceId, favorite });
export const searchPcWorks = (criteria: PcWorkSearch, cursor: string | null = null, scope: PcWorkScope | null = null, offset: number | null = null, limit: number | null = null) => {
  const revision = nextFavoriteRevision();
  // 画面表示用の名前などを Rust コマンドへ渡さない。入力は三つの識別子だけ。
  const entity = scope ? { kind: scope.kind, source: scope.source, key: scope.key } : null;
  // 接続先が新しい棚の一覧を出せるなら、同じ条件でそこから引く。
  const library = nativeShelf();
  const request = library
    ? searchShelf(library, criteria, cursor, entity, offset, limit)
    : invoke<PcSearchPage>("pc_search_works", { criteria, cursor, offset, limit, scope: entity });
  return request.then((page) => {
    for (const work of page.items) markFavoriteRead(work, revision);
    return page;
  });
};
// 作者・シリーズ・コレクションの画面は、接続先が出せるなら新しい棚から引く（形は同じ）。
export const listPcEntities = (kind: "people" | "series", text = "", offset = 0, sort?: { sort: string; order: string }) => {
  const library = groupShelf();
  return library ? listShelfEntities(library, kind, text, offset, sort) : invoke<PcEntityPage>("pc_list_entities", { kind, text, offset, sort: sort?.sort ?? null, order: sort?.order ?? null });
};
export const getPcEntityDetail = (kind: "people" | "series", source: string, key: string) => {
  const library = groupShelf();
  return library ? shelfEntityDetail(library, kind, source, key) : invoke<PcEntityDetail>("pc_get_entity_detail", { kind, source, key });
};
export const listPcEntitySeries = (source: string, key: string, text: string, cursor: string | null, sort?: { sort: string; order: string }) => {
  const library = groupShelf();
  return library ? listShelfEntitySeries(library, source, key, text, cursor, sort) : invoke<PcEntitySeriesPage>("pc_list_entity_series", { source, key, text, cursor, sort: sort?.sort ?? null, order: sort?.order ?? null });
};
export const listPcCollections = () => {
  const library = groupShelf();
  return library ? listShelfCollections(library) : invoke<PcCollection[]>("pc_list_collections");
};
/** 新しい棚の作品は棚から引く。棚のコレクションを出せない接続では出さない。 */
export const getPcWorkCollections = (workId: number | string) => {
  if (typeof workId === "number") return invoke<PcCollection[]>("pc_get_work_collections", { workId });
  const library = groupShelf();
  return library ? shelfWorkCollections(library, workId) : Promise.resolve<PcCollection[]>([]);
};
export const getPcCollection = (collectionId: string) => {
  const revision = nextFavoriteRevision();
  const library = groupShelf();
  const request = library ? shelfCollectionDetail(library, collectionId) : invoke<PcCollectionDetail>("pc_get_collection", { collectionId });
  return request.then((collection) => {
    for (const member of collection.members) if (member.work) markFavoriteRead(member.work, revision);
    return collection;
  });
};
export const getPcMetadata = (workId: number | string) => {
  const revision = nextFavoriteRevision();
  const request = typeof workId === "string" ? shelfMetadata(workId) : invoke<PcMetadata>("pc_get_metadata", { workId });
  return request.then((metadata) => {
    markFavoriteRead(metadata.work, revision);
    return metadata;
  });
};
// 新しい棚の作品の本文・目次は読む画面が棚から組む（`nativeReading.ts`）。旧 API では引けない。
export const getPcPage = (workId: number | string, page: number, version: number | null = null) => typeof workId === "string" ? notYet() : invoke<PcPage>("pc_get_page", { workId, page, version });
export const getPcOutline = (workId: number | string, version: number | null = null) => typeof workId === "string" ? notYet() : invoke<PcOutlineEntry[]>("pc_get_outline", { workId, version });
export interface SearchSuggestion {
  kind: string;
  label: string;
  value: string;
  count: number | null;
  source?: string;
  sourceKey?: string;
}
export const searchPcSuggestions = (text: string) => invoke<{ items: SearchSuggestion[] }>("pc_search_suggest", { text }).then((result) => result.items);
export const lookupPcWork = (source: string, key: string) => invoke<PcWork | null>("pc_lookup_work", { source, key });
/** Rust が返す保存版の一覧の行。新しい棚の作品は棚の一覧と同じ形で残っている。 */
type StoredOfflineWork = Omit<OfflineWork, "work"> & { work: PcWork | MobileShelfWork };

/** 保存版の行を画面の形にする。新しい棚の作品は一覧の一件と同じ変換を通す。 */
function offlineWork(stored: StoredOfflineWork): OfflineWork {
  const work = stored.work;
  if (typeof work.id === "string" && "snapshotId" in work) return { ...stored, work: shelfWork(work) };
  return stored as OfflineWork;
}

export const listOfflineWorks = () => invoke<StoredOfflineWork[]>("offline_list_works").then((works) => works.map(offlineWork));
/** 端末に保存する。新しい棚の作品は、今つながっている棚から読む画面と同じ組み立てで保存する。 */
export const saveOfflineWork = (workId: number | string) => {
  if (typeof workId === "number") return invoke<StoredOfflineWork>("offline_save_work", { workId }).then(offlineWork);
  const library = nativeShelf();
  if (!library) return Promise.reject({ code: "library_changed", message: "接続先の本棚が変わりました。一覧を開き直してください" });
  return invoke<StoredOfflineWork>("offline_save_shelf_work", { library, work: workId }).then(offlineWork);
};
export const getOfflinePage = (workId: number | string, page: number) => invoke<OfflinePage>("offline_get_page", { workId, page });
export const getOfflineOutline = (workId: number | string, generation: string) => invoke<PcOutlineEntry[]>("offline_get_outline", { workId, generation });
export const getOfflineAsset = (workId: number | string, assetId: number, generation: string) => invoke<PcAsset>("offline_get_asset", { workId, assetId, generation });
export const deleteOfflineWork = (work: Pick<PcWork, "id" | "source" | "sourceId">) => invoke<void>("offline_delete_work", { workId: work.id, source: work.source, sourceId: work.sourceId });
/** 数値 ID はサーバーごとに振られる。同じ作品かは取得元の識別子まで見て決める。 */
export const sameWork = (left: Pick<PcWork, "id" | "source" | "sourceId">, right: Pick<PcWork, "id" | "source" | "sourceId">) => left.id === right.id && left.source === right.source && left.sourceId === right.sourceId;
export const searchOfflineText = (query: string) => invoke<OfflineTextHit[]>("offline_search_text", { query });
export const listOfflineIssues = () => invoke<OfflineIssue[]>("offline_list_issues");
export const deleteOfflineIssue = (workId: number | string) => invoke<void>("offline_delete_corrupt_work", { workId });

export function pcErrorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error && typeof error.message === "string") return error.message;
  return typeof error === "string" ? error : "サーバーとの通信に失敗しました";
}
