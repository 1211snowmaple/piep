import type {
  MobileCollection, MobileCollectionMember, MobileEntity, MobileEntityDetail, MobileEntityPage, MobileEntitySeriesPage,
  MobilePage, MobilePerson, MobileSeriesMembership, MobileVersion, MobileWork, MobileWorkPage,
} from "@shared/mobileApi";
import { invoke } from "@tauri-apps/api/core";
import { markFavoriteRead, nextFavoriteRevision } from "./favoriteFreshness";
import { listShelfCollections, listShelfEntities, listShelfEntitySeries, lookupShelfWork, connectedShelf, NO_SHELF, searchShelf, shelfCollectionDetail, shelfEntityDetail, shelfMetadata, shelfSuggestions, shelfWork, shelfWorkCollections } from "./shelf";

export interface PcConnectionStatus {
  paired: boolean;
  reachable: boolean;
  url: string | null;
  deviceName: string | null;
  issue: "offline" | "certificate_changed" | "certificate_untrusted" | "secret_lost" | "protocol_mismatch" | "revoked" | "not_ready" | "remote_error" | "no_library" | null;
  capabilities: string[];
  /** 接続先が配信している棚。届かないとき・棚がまだ無いときは無い。 */
  shelf?: {
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
  no_library: "この端末ではライブラリを開けません。デスクトップ版の「設定 → サーバー」で、未送信なら「今すぐ送る」を押し、送信済みなら「ライブラリを読める携帯」でこの端末をオンにしてください。",
};

export function pcConnectionIssue(status: PcConnectionStatus): string | null {
  if (!status.issue) return null;
  return pcConnectionIssueText[status.issue];
}

// サーバーから届く型は shared/mobileApi.ts（Rust の定義から生成）にある。
// ここでは、画面だけが持つ項目を足す。

/** 端末の読書記録を分ける棚と読み手。 */
export type ReadingScope = Pick<NonNullable<PcConnectionStatus["shelf"]>["scope"], "libraryId" | "subjectId">;

export function sameReadingScope(left: ReadingScope | null, right: ReadingScope | null): boolean {
  return left?.libraryId === right?.libraryId && left?.subjectId === right?.subjectId;
}

export type PcWork = MobileWork & {
  readingScope: ReadingScope;
  /** フロント側だけの取得順。お気に入りの書き込みより新しい応答を見分ける。 */
  favoriteReadSequence?: number;
};

/** 端末に保存した版が、今サーバーにある版と同じか。版と編集の UUID で見る。 */
export function sameSavedVersion(work: Pick<PcWork, "snapshotId" | "editId">, saved: Pick<OfflineWork, "work">): boolean {
  return work.snapshotId === saved.work.snapshotId && (work.editId ?? null) === (saved.work.editId ?? null);
}

/** 棚に届かない接続で、棚を使う操作をしたとき。 */
const noShelf = () => Promise.reject(NO_SHELF);

export type PcEntity = MobileEntity;

export type PcEntityPage = MobileEntityPage;

export type PcEntitySeriesPage = MobileEntitySeriesPage;

export type PcEntityDetail = Omit<MobileEntityDetail, "collections"> & { collections: PcCollection[] };

export type PcCollection = Omit<MobileCollection, "coverMode"> & {
  coverMode: "mosaic" | "spine" | "single" | "sigil" | "file";
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

export type PcSearchPage = Omit<MobileWorkPage, "items"> & { items: PcWork[] };

export interface PcSearchCriteria {
  text: string;
  source: "" | "pixiv" | "fanbox";
  contentType: "" | "novel" | "article" | "text" | "image" | "file";
  /** 作者とタグは一件ずつ。札で選んだタグは区切り文字を含んでいてもそのまま一件にする。 */
  author: string[];
  authorExclude: string[];
  favorite: boolean;
  tag: string[];
  tagExclude: string[];
  tagMode: "and" | "or";
  minChars: string;
  maxChars: string;
  minAssets: string;
  maxAssets: string;
  revision: "" | "revised" | "first_version";
  edit: "" | "edited" | "unedited";
  cover: "" | "has_cover" | "no_cover";
  dateField: "saved" | "published" | "updated";
  dateFrom: string;
  dateTo: string;
  // remembered は画面だけの値で「検索ボタン横のいつもの順に従う」。送る前に実際の順へ直す。
  sort: PcSort | "remembered";
}

// standard はサーバーの標準の順。検索語があれば関連度、シリーズの中ならシリーズの順になる。
/** 携帯で選べる作品の順。値はデスクトップと共通の `鍵:向き`（`@shared/sortOptions`）。 */
export type FixedSort =
  | "saved:desc" | "saved:asc" | "published:desc" | "published:asc"
  | "title:asc" | "title:desc" | "author:asc" | "author:desc"
  | "length:desc" | "length:asc" | "media:desc" | "media:asc" | "version:desc" | "version:asc";
export type PcSort = "standard" | FixedSort;

/** 作者やシリーズの中だけで使える順を足したもの。 */
export type PcWorkSort = PcSort | "series_order:asc" | "series_order:desc";
export type PcWorkSearch = Omit<PcSearchCriteria, "sort"> & { sort: PcWorkSort };

export const defaultPcSearch: PcSearchCriteria = { text: "", source: "", contentType: "", author: [], authorExclude: [], favorite: false, tag: [], tagExclude: [], tagMode: "and", minChars: "", maxChars: "", minAssets: "", maxAssets: "", revision: "", edit: "", cover: "", dateField: "saved", dateFrom: "", dateTo: "", sort: "remembered" };

/** 入力欄に書いた作者・タグを一件ずつに分ける。分けるのは自由入力だけ。 */
export function splitSearchTerms(text: string): string[] {
  return text.split(/[\n\r,、，]+/).map((part) => part.trim()).filter(Boolean);
}

/** 一件ずつの作者・タグを、入力欄や札に出す一続きの文字にする。 */
export function joinSearchTerms(terms: string[]): string {
  return terms.join("、");
}

/** 作品の保存した版。写しの ID で以前の版を棚から開く。 */
export type PcVersion = MobileVersion;

/** 作品の画面と読む画面が使う、作品・作者・シリーズ・版。 */
export interface PcMetadata {
  work: PcWork;
  people: PcPerson[];
  series: PcSeriesMembership[];
  /** 古い順。 */
  versions: PcVersion[];
  /** 編集の印（`edit:{UUID}`）。編集がなければ null。 */
}

export type PcPerson = MobilePerson;

export type PcSeriesMembership = MobileSeriesMembership;

export type PcPage = MobilePage;

export interface OfflinePage extends PcPage {
  generation: string;
  readingScope: ReadingScope;
  snapshotId: string;
  editId: string | null;
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
  people?: PcPerson[];
  series?: PcSeriesMembership[];
  savedAtMs: number;
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
  const library = connectedShelf();
  if (!library) return noShelf();
  return searchShelf(library, criteria, cursor, entity, offset, limit).then((page) => {
    for (const work of page.items) markFavoriteRead(work, revision);
    return page;
  });
};
export const listPcEntities = (kind: "people" | "series", text = "", offset = 0, sort?: { sort: string; order: string }) => {
  const library = connectedShelf();
  return library ? listShelfEntities(library, kind, text, offset, sort) : noShelf();
};
export const getPcEntityDetail = (kind: "people" | "series", source: string, key: string) => {
  const library = connectedShelf();
  return library ? shelfEntityDetail(library, kind, source, key) : noShelf();
};
export const listPcEntitySeries = (source: string, key: string, text: string, cursor: string | null, sort?: { sort: string; order: string }) => {
  const library = connectedShelf();
  return library ? listShelfEntitySeries(library, source, key, text, cursor, sort) : noShelf();
};
export const listPcCollections = () => {
  const library = connectedShelf();
  return library ? listShelfCollections(library) : noShelf();
};
export const getPcWorkCollections = (workId: string) => {
  const library = connectedShelf();
  return library ? shelfWorkCollections(library, workId) : Promise.resolve<PcCollection[]>([]);
};
export const getPcCollection = (collectionId: string) => {
  const revision = nextFavoriteRevision();
  const library = connectedShelf();
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
  const shelf = connectedShelf();
  return shelf ? shelfSuggestions(shelf, text) : Promise.resolve([]);
};
/** 取得元と ID から棚の作品を引く（本文の中のリンク・題の候補）。棚に無いときは失敗になる。 */
export const lookupPcWork = (source: string, key: string): Promise<PcWork | null> => {
  const shelf = connectedShelf();
  return shelf ? lookupShelfWork(shelf, source, key) : Promise.resolve(null);
};
/** Rust が返す保存版の一覧の行。作品は棚の一覧と同じ形で残っている。 */
type StoredOfflineWork = Omit<OfflineWork, "work"> & { work: MobileWork; readingScope: ReadingScope };

/** 保存版の行を画面の形にする。作品は一覧の一件と同じ変換を通す。 */
const offlineWork = ({ work, readingScope, ...saved }: StoredOfflineWork): OfflineWork => ({ ...saved, work: shelfWork(work, readingScope) });

export const listOfflineWorks = () => invoke<StoredOfflineWork[]>("offline_list_works").then((works) => works.map(offlineWork));
/** 端末に保存する。今つながっている棚から、読む画面と同じ組み立てで保存する。 */
export const saveOfflineWork = (workId: string) => {
  const library = connectedShelf();
  if (!library) return noShelf();
  return invoke<StoredOfflineWork>("offline_save_shelf_work", { library, work: workId }).then(offlineWork);
};
export async function getOfflinePage(work: PcWork, page: number): Promise<OfflinePage> {
  const saved = await invoke<OfflinePage>("offline_get_page", { workId: work.id, page });
  if (!sameReadingScope(work.readingScope, saved.readingScope) || work.snapshotId !== saved.snapshotId || work.editId !== saved.editId) {
    throw new Error("端末の保存が更新されました。作品を開き直してください");
  }
  return saved;
}
export const getOfflineOutline = (workId: string, generation: string) => invoke<PcOutlineEntry[]>("offline_get_outline", { workId, generation });
export const getOfflineAsset = (workId: string, assetId: number, generation: string) => invoke<PcAsset>("offline_get_asset", { workId, assetId, generation });
export const deleteOfflineWork = (work: Pick<PcWork, "id" | "source" | "sourceId">) => invoke<void>("offline_delete_work", { workId: work.id, source: work.source, sourceId: work.sourceId });
/** 保存した作品が、この棚と利用者の同じ作品か。 */
export const sameWork = (left: PcWork, right: PcWork) => left.id === right.id && sameReadingScope(left.readingScope, right.readingScope);
export const searchOfflineText = (query: string) => invoke<OfflineTextHit[]>("offline_search_text", { query });
export const listOfflineIssues = () => invoke<OfflineIssue[]>("offline_list_issues");
export const deleteOfflineIssue = (workId: string) => invoke<void>("offline_delete_corrupt_work", { workId });

export function pcErrorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error && typeof error.message === "string") return error.message;
  return typeof error === "string" ? error : "サーバーとの通信に失敗しました";
}
