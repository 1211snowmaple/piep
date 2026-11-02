import { invoke } from "@tauri-apps/api/core";

export interface PcConnectionStatus {
  paired: boolean;
  reachable: boolean;
  url: string | null;
  deviceName: string | null;
  issue: "offline" | "certificate_changed" | "protocol_mismatch" | "revoked" | "remote_error" | null;
}

export const pcConnectionIssueText: Record<NonNullable<PcConnectionStatus["issue"]>, string> = {
  offline: "PC が見つかりません。piep の共有、同じ Wi-Fi、PC の起動状態を確認してください。",
  certificate_changed: "PC の証明書が登録時と違います。接続先を確認して登録し直してください。",
  protocol_mismatch: "PC 版との接続形式が合いません。両方のアプリを更新してください。",
  revoked: "PC の共有が再起動されたか、端末の登録が解除されました。PC 側で登録文字列を出し直して再登録してください。",
  remote_error: "PC の共有は応答していますが、作品を読み込めません。PC 側の状態を確認してください。",
};

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
  sort: "standard" | "dateDesc" | "dateAsc" | "publishedDesc" | "publishedAsc" | "titleAsc" | "titleDesc" | "authorAsc" | "authorDesc" | "lengthDesc" | "lengthAsc" | "assetsDesc" | "assetsAsc" | "versionDesc" | "versionAsc";
}

export const defaultPcSearch: PcSearchCriteria = { text: "", source: "", contentType: "", author: "", authorExclude: "", favorite: false, tag: "", tagExclude: "", tagMode: "and", minChars: "", maxChars: "", minAssets: "", maxAssets: "", revision: "", edit: "", cover: "", dateField: "downloaded_at", dateFrom: "", dateTo: "", sort: "standard" };

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

export interface OfflineIssue {
  workId: number;
  issue: "offline_storage";
}

export const getPcStatus = () => invoke<PcConnectionStatus>("pc_connection_status");
export const pairPc = (payload: string, deviceName: string) => invoke<PcConnectionStatus>("pc_pair", { payload, deviceName });
export const disconnectPc = () => invoke<PcConnectionStatus>("pc_disconnect");
export const searchPcWorks = (criteria: PcSearchCriteria, cursor: string | null = null, scope: PcWorkScope | null = null, offset: number | null = null) => invoke<PcSearchPage>("pc_search_works", {
  criteria,
  cursor,
  offset,
  // 画面表示用の名前などを Rust コマンドへ渡さない。入力は三つの識別子だけ。
  scope: scope ? { kind: scope.kind, source: scope.source, key: scope.key } : null,
});
export const listPcEntities = (kind: "people" | "series", text = "", offset = 0) => invoke<PcEntityPage>("pc_list_entities", { kind, text, offset });
export const listPcCollections = () => invoke<PcCollection[]>("pc_list_collections");
export const getPcWorkCollections = (workId: number) => invoke<PcCollection[]>("pc_get_work_collections", { workId });
export const getPcCollection = (collectionId: string) => invoke<PcCollectionDetail>("pc_get_collection", { collectionId });
export const getPcMetadata = (workId: number) => invoke<PcMetadata>("pc_get_metadata", { workId });
export const getPcPage = (workId: number, page: number, version: number | null = null) => invoke<PcPage>("pc_get_page", { workId, page, version });
export const getPcOutline = (workId: number) => invoke<PcOutlineEntry[]>("pc_get_outline", { workId });
export const getPcAsset = (workId: number, assetId: number) => invoke<PcAsset>("pc_get_asset", { workId, assetId });
export const getPcCover = (workId: number) => invoke<PcAsset>("pc_get_cover", { workId });
export const getPcCollectionCover = (collectionId: string, slot: "file" | `tile-${0 | 1 | 2 | 3}`) => invoke<PcAsset>("pc_get_collection_cover", { collectionId, slot });
export const getPcEntityCover = (kind: "people" | "series", source: string, key: string) => invoke<PcAsset>("pc_get_entity_cover", { kind, source, key });
export const listOfflineWorks = () => invoke<OfflineWork[]>("offline_list_works");
export const saveOfflineWork = (workId: number) => invoke<OfflineWork>("offline_save_work", { workId });
export const getOfflinePage = (workId: number, page: number) => invoke<OfflinePage>("offline_get_page", { workId, page });
export const getOfflineOutline = (workId: number, generation: string) => invoke<PcOutlineEntry[]>("offline_get_outline", { workId, generation });
export const getOfflineAsset = (workId: number, assetId: number, generation: string) => invoke<PcAsset>("offline_get_asset", { workId, assetId, generation });
export const getOfflineCover = (workId: number) => invoke<PcAsset>("offline_get_cover", { workId });
export const getOfflineCollectionCover = (workId: number, collectionId: string) => invoke<PcAsset>("offline_get_collection_cover", { workId, collectionId });
export const getOfflineEntityCover = (workId: number, kind: "people" | "series") => invoke<PcAsset>("offline_get_entity_cover", { workId, kind });
export const deleteOfflineWork = (workId: number) => invoke<void>("offline_delete_work", { workId });
export const listOfflineIssues = () => invoke<OfflineIssue[]>("offline_list_issues");
export const deleteOfflineIssue = (workId: number) => invoke<void>("offline_delete_corrupt_work", { workId });

export function pcErrorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error && typeof error.message === "string") return error.message;
  return typeof error === "string" ? error : "PC との通信に失敗しました";
}
