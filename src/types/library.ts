export type SourceKind = "pixiv" | "fanbox" | string;

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
  sampleTitle?: string | null;
}

export interface DashboardTrendPoint {
  bucket: string;
  count: number;
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

export interface SearchV2Params {
  text?: string | null;
  query?: string | null;
  source?: string | null;
  contentType?: string | null;
  sortBy?: string | null;
  sortOrder?: "asc" | "desc" | null;
  limit?: number;
  cursor?: string | null;
  favorite?: boolean | null;
  tagsInclude?: string[] | null;
  tagsExclude?: string[] | null;
  tagFilterMode?: "and" | "or" | string | null;
  authorsInclude?: string[] | null;
  authorsExclude?: string[] | null;
  minCharCount?: number | null;
  maxCharCount?: number | null;
  assetFilter?: string | null;
  watchFilter?: string | null;
  personSource?: string | null;
  personKey?: string | null;
  seriesSource?: string | null;
  seriesKey?: string | null;
  viewMode?: LibraryViewMode;
  projection?: "libraryGallery" | "libraryCompact" | "bulk" | "entityFacet" | null;
  searchMode?: "smart" | "exact" | "semantic" | null;
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
}

export interface SearchSuggestResult {
  items: SearchSuggestion[];
}

export interface SearchRebuildProgress {
  jobId: string;
  status: "running" | "completed" | "canceled" | "failed" | string;
  totalDownloads: number;
  indexedDownloads: number;
  pendingDownloads: number;
  isComplete: boolean;
  phase?: string;
  indexedChunks?: number;
  embeddingProvider?: string;
  gpuEnabled?: boolean;
  throughputPerSec?: number | null;
  error?: string;
}
