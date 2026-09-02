/** Known providers, while still allowing plugins to add a named provider. */
export type SourceKind = "pixiv" | "fanbox" | (string & {});

export interface DownloadEntry {
  id: number;
  source: SourceKind;
  sourceId: string;
  title: string;
  authorName: string;
  authorId: string;
  contentType: string;
  tags: string[];
  excerpt: string | null;
  coverPath: string | null;
  jsonPath: string;
  assetCount: number;
  fileSizeBytes: number;
  downloadedAt: string;
  sourceCreatedAt: string | null;
  sourceUpdatedAt: string | null;
  currentVersion: number;
  watchUpdates: boolean;
  textLength: number;
  favorite: boolean;
  personId?: string | null;
  personName?: string | null;
  /** Creator avatar shown beside the author name on cards. */
  personIconPath?: string | null;
  seriesId?: string | null;
  seriesTitle?: string | null;
  searchScore?: number | null;
  matchFields?: string[];
  scoreReasons?: ScoreReason[];
  matchHighlights?: SearchHighlight[];
  sortKey?: string | null;
}

export interface ScoreReason {
  field: string;
  matchType: string;
  term: string;
  contribution: number;
  detail?: string | null;
}

export interface SearchHighlight {
  field: string;
  text: string;
  segments: SearchHighlightSegment[];
  sourceChunkId?: string | null;
  matchType?: string | null;
}

export interface SearchHighlightSegment {
  text: string;
  matched: boolean;
}

export interface AssetEntry {
  id: number;
  downloadId: number;
  assetType: string;
  filename: string;
  localPath: string;
  originalUrl: string | null;
  mimeType: string | null;
  fileSizeBytes: number;
}

export interface DownloadVersion {
  id: number;
  downloadId: number;
  version: number;
  contentHash: string | null;
  textLength: number;
  jsonPath: string;
  originalJsonPath: string | null;
  assetCount: number;
  fileSizeBytes: number;
  createdAt: string;
  changeSummary: string | null;
}

export interface WorkEditRevision {
  id: number;
  downloadId: number;
  baseVersion: number;
  status: "draft" | "active" | "archived" | string;
  title: string | null;
  contentHash: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface WorkBlock {
  id: number;
  editRevisionId: number;
  order: number;
  blockType: "paragraph" | "heading" | "image" | "separator" | string;
  text: string | null;
  assetId: number | null;
  attrsJson: string | null;
}

export interface WorkBlockInput {
  blockType: "paragraph" | "heading" | "image" | "separator" | string;
  text?: string | null;
  assetId?: number | null;
  attrsJson?: string | null;
}

export interface ReaderDocument {
  download: DownloadEntry;
  assets: AssetEntry[];
  versions: DownloadVersion[];
  html: string;
  plainText: string;
  isEdited: boolean;
  activeEditRevision: WorkEditRevision | null;
}

export interface ReaderMetadata {
  download: DownloadEntry;
  versions: DownloadVersion[];
  assetCount: number;
  isEdited: boolean;
  activeEditRevision: WorkEditRevision | null;
}

export interface ReaderContentPage {
  page: number;
  pageCount: number;
  html: string;
  plainText: string;
  totalPlainTextChars: number;
}

export interface ReaderSearchHit {
  page: number;
  snippet: string;
  count: number;
}

export interface LibraryDiagnostics {
  measuredAt: string;
  totalDownloads: number;
  totalAssets: number;
  totalVersions: number;
  totalTextLength: number;
  databaseSizeBytes: number;
  walSizeBytes: number;
  storageSizeBytes: number;
  lexicalIndexSizeBytes: number;
  lexicalIndexFileCount: number;
  lexicalIndexSegmentCount: number;
  semanticIndexSizeBytes: number;
  sqlitePageCount: number;
  sqliteFreePages: number;
  sqliteCacheSizeBytes: number;
  liveDatabaseBytes: number;
  fragmentationPercent: number;
  orphanAssetRows: number;
  orphanAssetBytes: number;
  orphanAssetFiles: number;
  orphanAssetFileBytes: number;
  checkedFileReferences: number;
  missingJsonFiles: number;
  missingAssetFiles: number;
  missingProfileFiles: number;
  unsafeReferencedFiles: number;
  unreadableReferencedFiles: number;
  emptyReferencedFiles: number;
  mismatchedAssetFiles: number;
  transientFiles: number;
  transientFileBytes: number;
  fileIssueSamples: LibraryFileIssue[];
  /** Tauri host and all descendant WebView processes. */
  processMemoryBytes: number | null;
  processPrivateMemoryBytes: number | null;
  processCount: number;
  webviewProcessCount: number;
  gpuDedicatedMemoryBytes: number | null;
  gpuSharedMemoryBytes: number | null;
  listFirstPageMs: number;
  listP50Ms: number;
  listP95Ms: number;
  lexicalSearchMs: number | null;
  lexicalSearchP50Ms: number | null;
  lexicalSearchP95Ms: number | null;
  exactAuthorP50Ms: number | null;
  exactAuthorP95Ms: number | null;
  benchmarkQuery: string | null;
  searchIndex: SearchIndexStatus;
}

export interface LibraryFileIssue {
  issueType: "missing" | "unsafe" | "unreadable" | "empty" | "size_mismatch" | "transient" | string;
  category: "work_json" | "work_asset" | "profile" | "entity_json" | "transient" | string;
  path: string;
  label: string | null;
  expectedSizeBytes: number | null;
  actualSizeBytes: number | null;
}

export interface SearchIndexStatus {
  totalDownloads: number;
  indexedDownloads: number;
  pendingDownloads: number;
  /** 全文索引だけの話。意味索引の遅れはここに出ない。 */
  isComplete: boolean;
  phase: string;
  semanticIndexedChunks: number;
  /** 意味索引が最新の版で覆えている作品数。断片数と違い、棚の何割かが分かる。 */
  semanticIndexedDownloads: number;
  /** 意味索引がまだ追いついていない作品数。 */
  semanticPendingDownloads: number;
  /** Persistent policy shared by rebuilds and incremental updates. */
  semanticEnabled: boolean;
  semanticModelReady: boolean;
  embeddingProvider: string;
  gpuEnabled: boolean;
  throughputPerSec?: number | null;
}

export interface LibraryMaintenanceResult {
  compacted: boolean;
  beforeBytes: number;
  afterBytes: number;
  reclaimedBytes: number;
}

export interface SearchIndexOptimizationResult {
  optimized: boolean;
  beforeSegments: number;
  afterSegments: number;
  beforeBytes: number;
  afterBytes: number;
  reclaimedBytes: number;
  elapsedMs: number;
}

export interface EditorDocument {
  download: DownloadEntry;
  assets: AssetEntry[];
  activeRevision: WorkEditRevision | null;
  draftRevision: WorkEditRevision | null;
  baseVersion: number;
  blocks: WorkBlock[];
}

export interface DbStats {
  totalDownloads: number;
  pixivCount: number;
  fanboxCount: number;
  totalAssets: number;
  totalSizeBytes: number;
}

export interface FacetCount {
  name: string;
  count: number;
}

export interface EntityFacet {
  source: string;
  sourceKey: string;
  displayName: string;
  count: number;
  coverPath: string | null;
  description?: string | null;
  updatedAt?: string | null;
  latestDownloadedAt?: string | null;
  /** 配下の作品のうち、取得元でいちばん新しいものの時刻。 */
  latestSourceUpdatedAt?: string | null;
  sampleTitle?: string | null;
  iconPath?: string | null;
  bannerPath?: string | null;
  /** シリーズだけが持つ。null は「取得元にまだ聞いていない」。 */
  isConcluded?: boolean | null;
}

/**
 * 作者・シリーズの一覧を並べる鍵。
 *
 * 作品の並べ替えとは別の語彙を持つ。束ねに文字数も容量も無く、代わりに
 * 「中にある作品を見て決まる」鍵がある。
 */
export type EntitySortBy = "work_count" | "downloaded_at" | "source_updated_at" | "name";

/**
 * 一覧そのものにかける条件。
 *
 * 配下の作品にかける絞り込み（保存元・タグ・お気に入り）とは層が違う。
 * 追いかけているか、何作品以上あるか、完結しているか - どれも束ね自身の
 * 性質で、作品の側には無い。
 */
export interface EntityFacetScope {
  watch?: "watched" | "paused" | "unwatched" | null;
  minWorkCount?: number | null;
  /** シリーズだけに効く。null は指定なし。 */
  concluded?: boolean | null;
}

export interface FilterFacets {
  tags: FacetCount[];
  authors: FacetCount[];
  authorEntities: EntityFacet[];
  series: EntityFacet[];
  contentTypes: FacetCount[];
  assetTypes: FacetCount[];
}

export interface PersonEntry {
  id: number;
  source: string;
  sourceKey: string;
  displayName: string;
  iconPath: string | null;
  coverPath: string | null;
  description: string | null;
  linksJson: string | null;
  contentHash: string | null;
  currentVersion: number;
  lastCheckedAt: string | null;
  lastFetchedAt: string | null;
  createdAt: string;
  updatedAt: string;
  workCount: number | null;
}

export interface SeriesEntry {
  id: number;
  source: string;
  sourceKey: string;
  title: string;
  description: string | null;
  coverPath: string | null;
  contentHash: string | null;
  currentVersion: number;
  lastCheckedAt: string | null;
  lastFetchedAt: string | null;
  createdAt: string;
  updatedAt: string;
  workCount: number | null;
  /** 完結しているか。null は「取得元にまだ聞いていない」。 */
  isConcluded: boolean | null;
  /** 取得元で公開されている話数。手元の workCount と比べると取りこぼしが分かる。 */
  publishedContentCount: number | null;
}

export interface EntityVersion {
  id: number;
  entityType: string;
  source: string;
  sourceKey: string;
  version: number;
  contentHash: string | null;
  jsonPath: string;
  assetCount: number;
  fileSizeBytes: number;
  createdAt: string;
  changeSummary: string | null;
}

export interface UpdateTarget {
  id: number;
  targetType: "work" | "author" | "series" | string;
  source: string;
  sourceKey: string;
  displayName: string;
  enabled: boolean;
  lastCheckedAt: string | null;
  lastSeenSourceId: string | null;
  lastSeenSourceUpdatedAt: string | null;
  metadataJson: string | null;
  createdAt: string;
  updatedAt: string;
  /** When this target last turned up something. Null means it never has. */
  lastHitAt?: string | null;
  /** Failures in a row. Reset to zero by any successful check. */
  consecutiveErrors?: number;
}

export interface DashboardTrendPoint {
  bucket: string;
  count: number;
  pixivCount: number;
  fanboxCount: number;
  totalSizeBytes: number;
}

export interface SourceBreakdown {
  source: string;
  count: number;
  totalSizeBytes: number;
}

export interface DashboardSummary {
  stats: DbStats;
  favoriteCount: number;
  watchedCount: number;
  updateTargetCount: number;
  indexedCount: number;
  pendingIndexCount: number;
  topTags: FacetCount[];
  topAuthors: FacetCount[];
  recentDownloads: DownloadEntry[];
  sourceBreakdown: SourceBreakdown[];
  monthlyDownloads: DashboardTrendPoint[];
}

export type LibraryViewMode =
  | "gallery"
  | "compact"
  | "epubSelection"
  | "updateReview";

/**
 * Sort keys accepted by the desktop search command. The snake-case aliases
 * are kept because saved searches from earlier releases persist these values.
 */
export type LibrarySortBy =
  | "downloaded_at"
  | "source_created_at"
  | "source_updated_at"
  | "title"
  | "author_name"
  | "text_length"
  | "file_size_bytes"
  | "series_order"
  /** Score ranking. Only meaningful together with a text query, where it is the
   *  default; any other key makes the backend order the matches by that column. */
  | "relevance";

export type LibraryWatchFilter = "watched" | "unwatched";

/** Counts shown beside each shelf in the library sidebar. */
export interface LibraryShelfCounts {
  total: number;
  favorite: number;
  watched: number;
  /** Works with a recorded reading position that still exist in the library. */
  reading: number;
  /** 取り込んでいない改稿がある作品の数。 */
  revised: number;
}

export interface SavedSearchRecord {
  id: number;
  name: string;
  query: string | null;
  paramsJson: string;
  createdAt: string;
  updatedAt: string;
}

export interface SavedSearchInput {
  /** Omitted for a new entry; reusing a name replaces that entry. */
  id?: number | null;
  name: string;
  query?: string | null;
  paramsJson: string;
}
export type LibraryTagFilterMode = "and" | "or";
export type LibraryProjection = "libraryGallery" | "libraryCompact" | "bulk" | "entityFacet";
export type LibrarySearchMode = "smart" | "exact" | "semantic";

export interface SearchV2Params {
  text?: string | null;
  query?: string | null;
  source?: string | null;
  contentType?: string | null;
  sortBy?: LibrarySortBy | null;
  sortOrder?: "asc" | "desc" | null;
  limit?: number;
  cursor?: string | null;
  favorite?: boolean | null;
  tagsInclude?: string[] | null;
  tagsExclude?: string[] | null;
  tagFilterMode?: LibraryTagFilterMode | null;
  authorsInclude?: string[] | null;
  authorsExclude?: string[] | null;
  minCharCount?: number | null;
  maxCharCount?: number | null;
  assetFilter?: string | null;
  watchFilter?: LibraryWatchFilter | null;
  personSource?: string | null;
  personKey?: string | null;
  seriesSource?: string | null;
  seriesKey?: string | null;
  /** Restricts results to these works. An empty array means an empty shelf. */
  idsInclude?: number[] | null;
  /** Jumps to a numbered page. Ignored for relevance ordering, which is walked
   *  with a score cursor and has no nth page. */
  offset?: number | null;
  viewMode?: LibraryViewMode;
  projection?: LibraryProjection | null;
  searchMode?: LibrarySearchMode | null;
}

export interface SearchV2Result {
  items: DownloadEntry[];
  nextCursor: string | null;
  totalEstimate: number | null;
  searchMeta: {
    engine: string;
    query: string | null;
    totalEstimate: number | null;
    indexComplete: boolean;
    explanations?: string[];
    exactEntity?: {
      kind: "author" | "series" | string;
      label: string;
      source?: string | null;
      sourceKey?: string | null;
      strict: boolean;
    } | null;
    semanticIndexComplete?: boolean | null;
    semanticModelReady?: boolean | null;
  };
  facetsVersion: number;
}

export interface BulkMutationResult {
  matchedCount: number;
  changedCount: number;
}

export interface SearchSuggestParams {
  text?: string | null;
  limit?: number | null;
}

export interface SearchSuggestion {
  kind: "tag" | "author" | "series" | "title" | string;
  label: string;
  value: string;
  count?: number | null;
  exactMatch?: boolean;
  source?: string | null;
  sourceKey?: string | null;
}

export interface SearchSuggestResult {
  items: SearchSuggestion[];
}

export interface SearchRebuildProgress {
  jobId: string;
  /** "automatic" when the app caught its own index up at launch. */
  origin?: "manual" | "automatic" | string;
  status: "running" | "completed" | "canceled" | "failed" | string;
  totalDownloads: number;
  indexedDownloads: number;
  pendingDownloads: number;
  isComplete: boolean;
  phase?: string;
  /** Documents handled by this run. The bar tracks these, not the library-wide
   *  counts, which only move when a batch is committed. */
  processed?: number;
  processedTotal?: number;
  failed?: number;
  embeddingProvider?: string;
  gpuEnabled?: boolean;
  throughputPerSec?: number | null;
  etaSeconds?: number | null;
  error?: string;
}
