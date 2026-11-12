import { invoke } from "@tauri-apps/api/core";
import { markFavoriteRead, nextFavoriteRevision } from "./favoriteFreshness";

export interface PcConnectionStatus {
  paired: boolean;
  reachable: boolean;
  url: string | null;
  deviceName: string | null;
  issue: "offline" | "certificate_changed" | "certificate_untrusted" | "secret_lost" | "protocol_mismatch" | "revoked" | "not_ready" | "remote_error" | null;
  capabilities: string[];
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

export interface PcWork {
  id: number;
  source: string;
  sourceId: string;
  title: string;
  authorName: string;
  authorId: string;
  contentType: string;
  tags: string[];
  excerpt: string | null;
  textLength: number;
  assetCount: number;
  hasCover: boolean;
  currentVersion: number;
  contentHash: string | null;
  favorite: boolean;
  /** フロント側だけの取得順。お気に入りの書き込みより新しい応答を見分ける。 */
  favoriteReadSequence?: number;
  seriesTitle: string | null;
  seriesId: string | null;
  sourceCreatedAt?: string | null;
  downloadedAt?: string;
}

export interface PcEntity {
  source: string;
  sourceKey: string;
  displayName: string;
  count: number;
  description: string | null;
  sampleTitle: string | null;
  hasCover: boolean;
  isConcluded: boolean | null;
}

export interface PcEntityPage {
  items: PcEntity[];
  nextOffset: number | null;
  total: number;
}

export interface PcEntitySeriesPage {
  items: PcEntity[];
  nextCursor: string | null;
  total: number;
}

export interface PcEntityDetail {
  source: string;
  sourceKey: string;
  displayName: string;
  description: string | null;
  workCount: number;
  hasCover: boolean;
  hasBanner: boolean;
  isConcluded: boolean | null;
  publishedCount: number | null;
  links: string[];
  tags: { name: string; count: number }[];
  seriesCount: number | null;
  collections: PcCollection[];
}

export interface PcCollection {
  id: string;
  name: string;
  description: string | null;
  collectionKind: string;
  memberCount: number;
  availableCount: number;
  coverWorkId: number | null;
  coverMode?: "mosaic" | "spine" | "single" | "sigil" | "file";
  coverRevision?: number;
  hasSingleCover?: boolean;
  hasFileCover?: boolean;
  coverTiles?: { source: string; sourceId: string; title: string; authorName: string; hasCover: boolean }[];
  position?: number | null;
}

export interface PcCollectionDetail extends PcCollection {
  members: { title: string; authorName: string; position: number; work: PcWork | null }[];
}

export interface PcWorkScope {
  kind: "person" | "series";
  source: string;
  key: string;
}

export interface PcSearchPage {
  items: PcWork[];
  nextCursor: string | null;
  totalEstimate: number | null;
  indexComplete: boolean;
}

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

export interface PcMetadata {
  work: PcWork;
  people?: PcPerson[];
  series?: PcSeriesMembership[];
  versions: { version: number; contentHash: string | null; textLength: number; assetCount: number; createdAt: string }[];
  isEdited: boolean;
  activeEditId: number | null;
  activeEditHash: string | null;
}

export interface PcPerson {
  source: string;
  key: string;
  role: string;
  displayName: string;
}

export interface PcSeriesMembership {
  source: string;
  key: string;
  title: string;
  contentOrder: number | null;
}

export interface PcPage {
  page: number;
  pageCount: number;
  html: string;
  totalPlainTextChars: number;
  sourcePageStarts: number[];
  /** この本文の版。古いサーバーは返さない。 */
  revision?: PcRevision;
}

export interface PcRevision {
  currentVersion: number;
  contentHash: string | null;
  activeEditHash: string | null;
}

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
  workId: number;
  page: number;
  snippet: string;
  pages: number;
}

export interface OfflineIssue {
  workId: number;
  issue: "offline_storage";
}

export const getPcStatus = () => invoke<PcConnectionStatus>("pc_connection_status");
export const pairPc = (payload: string, deviceName: string) => invoke<PcConnectionStatus>("pc_pair", { payload, deviceName });
export const disconnectPc = () => invoke<PcConnectionStatus>("pc_disconnect");
/** お気に入りを付け外しする。返るのはサーバーが残した値。 */
export const setPcFavorite = (source: string, sourceId: string, favorite: boolean) => invoke<{ source: string; sourceId: string; favorite: boolean }>("pc_set_favorite", { source, sourceId, favorite });
export const searchPcWorks = (criteria: PcWorkSearch, cursor: string | null = null, scope: PcWorkScope | null = null, offset: number | null = null, limit: number | null = null) => {
  const revision = nextFavoriteRevision();
  return invoke<PcSearchPage>("pc_search_works", {
  criteria,
  cursor,
  offset,
  limit,
  // 画面表示用の名前などを Rust コマンドへ渡さない。入力は三つの識別子だけ。
  scope: scope ? { kind: scope.kind, source: scope.source, key: scope.key } : null,
  }).then((page) => {
    for (const work of page.items) markFavoriteRead(work, revision);
    return page;
  });
};
export const listPcEntities = (kind: "people" | "series", text = "", offset = 0, sort?: { sort: string; order: string }) => invoke<PcEntityPage>("pc_list_entities", { kind, text, offset, sort: sort?.sort ?? null, order: sort?.order ?? null });
export const getPcEntityDetail = (kind: "people" | "series", source: string, key: string) => invoke<PcEntityDetail>("pc_get_entity_detail", { kind, source, key });
export const listPcEntitySeries = (source: string, key: string, text: string, cursor: string | null, sort?: { sort: string; order: string }) => invoke<PcEntitySeriesPage>("pc_list_entity_series", { source, key, text, cursor, sort: sort?.sort ?? null, order: sort?.order ?? null });
export const listPcCollections = () => invoke<PcCollection[]>("pc_list_collections");
export const getPcWorkCollections = (workId: number) => invoke<PcCollection[]>("pc_get_work_collections", { workId });
export const getPcCollection = (collectionId: string) => {
  const revision = nextFavoriteRevision();
  return invoke<PcCollectionDetail>("pc_get_collection", { collectionId }).then((collection) => {
    for (const member of collection.members) if (member.work) markFavoriteRead(member.work, revision);
    return collection;
  });
};
export const getPcMetadata = (workId: number) => {
  const revision = nextFavoriteRevision();
  return invoke<PcMetadata>("pc_get_metadata", { workId }).then((metadata) => {
    markFavoriteRead(metadata.work, revision);
    return metadata;
  });
};
export const getPcPage = (workId: number, page: number, version: number | null = null) => invoke<PcPage>("pc_get_page", { workId, page, version });
export const getPcOutline = (workId: number, version: number | null = null) => invoke<PcOutlineEntry[]>("pc_get_outline", { workId, version });
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
export const listOfflineWorks = () => invoke<OfflineWork[]>("offline_list_works");
export const saveOfflineWork = (workId: number) => invoke<OfflineWork>("offline_save_work", { workId });
export const getOfflinePage = (workId: number, page: number) => invoke<OfflinePage>("offline_get_page", { workId, page });
export const getOfflineOutline = (workId: number, generation: string) => invoke<PcOutlineEntry[]>("offline_get_outline", { workId, generation });
export const getOfflineAsset = (workId: number, assetId: number, generation: string) => invoke<PcAsset>("offline_get_asset", { workId, assetId, generation });
export const deleteOfflineWork = (work: Pick<PcWork, "id" | "source" | "sourceId">) => invoke<void>("offline_delete_work", { workId: work.id, source: work.source, sourceId: work.sourceId });
/** 数値 ID はサーバーごとに振られる。同じ作品かは取得元の識別子まで見て決める。 */
export const sameWork = (left: Pick<PcWork, "id" | "source" | "sourceId">, right: Pick<PcWork, "id" | "source" | "sourceId">) => left.id === right.id && left.source === right.source && left.sourceId === right.sourceId;
export const searchOfflineText = (query: string) => invoke<OfflineTextHit[]>("offline_search_text", { query });
export const listOfflineIssues = () => invoke<OfflineIssue[]>("offline_list_issues");
export const deleteOfflineIssue = (workId: number) => invoke<void>("offline_delete_corrupt_work", { workId });

export function pcErrorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error && typeof error.message === "string") return error.message;
  return typeof error === "string" ? error : "サーバーとの通信に失敗しました";
}
