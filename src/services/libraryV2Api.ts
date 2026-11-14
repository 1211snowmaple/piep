import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ReaderOutlineEntry, ReaderSearchHit, ScoreReason, SearchHighlight } from "@/types/library";

/**
 * 新しい棚（読み取り専用の写し）の命令。`src-tauri/src/commands/library_v2.rs` と対になる。
 *
 * 作品は新しい棚の ID（UUID の文字列）で指し、画像はファイルの識別子（中身の SHA-256 の
 * 16 進）で指す。旧棚の番号やパスへの読み替えはしない。
 */

/** 新しい棚の作品の ID（UUID）。旧棚の作品は番号なので、画面の ID の形で見分ける。 */
export const SHELF_WORK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export type LibraryV2Status =
  | { state: "starting" }
  | { state: "absent"; path: string }
  | { state: "opening"; path: string }
  | { state: "indexing"; path: string }
  | { state: "ready"; path: string; unresolved: string[] }
  | { state: "failed"; path: string; message: string };

export type ShelfSort =
  | "saved" | "published" | "updated" | "title" | "author"
  | "length" | "size" | "media" | "version" | "relevance";

export type ShelfSortValue = number | string;

export interface ShelfCursor {
  value: ShelfSortValue;
  work: string;
}

export interface ShelfCountRange {
  min?: number | null;
  max?: number | null;
}

export interface ShelfDateRange {
  field: "saved" | "published" | "updated";
  /** ミリ秒。含む。 */
  from: number | null;
  /** ミリ秒。含まない。 */
  until: number | null;
}

/** `WorkFilter`。書かない項目は既定（絞らない）。 */
export interface ShelfFilter {
  provider?: string | null;
  content_type?: string | null;
  favorite?: boolean | null;
  watching?: boolean | null;
  edited?: boolean | null;
  has_cover?: boolean | null;
  in_series?: boolean | null;
  media?: "any" | "none" | "images" | "files" | "images_and_files" | null;
  revision?: "revised" | "pending_revision" | "saved_revisions" | "first_version" | null;
  acquisition?: "complete" | "attention" | "incomplete" | "unavailable" | "restricted" | "error" | "dismissed" | null;
  text_length?: ShelfCountRange;
  media_count?: ShelfCountRange;
  logical_size?: ShelfCountRange;
  date?: ShelfDateRange | null;
  tags?: string[];
  any_tag?: boolean;
  excluded_tags?: string[];
  authors?: string[];
  excluded_authors?: string[];
  works?: string[] | null;
}

export interface ShelfListQuery {
  sort: ShelfSort;
  descending: boolean;
  limit: number;
  after: ShelfCursor | null;
  filter: ShelfFilter;
  count: boolean;
}

export interface ShelfTextSearch {
  text: string;
  versions: "current" | "all";
  mode: "words" | "meaning";
}

/** 一覧の一件（`WorkCard` と表紙の識別子）。 */
export interface ShelfCard {
  id: string;
  snapshot_id: string;
  edit_id: string | null;
  provider: string;
  external_key: string;
  content_type: string;
  title: string;
  author_name: string;
  /** ミリ秒。 */
  saved_at: number;
  published_at: number | null;
  updated_at: number | null;
  text_length: number;
  logical_size: number;
  media_count: number;
  version_number: number;
  favorite: boolean;
  watching: boolean;
  revised: boolean;
  image_count: number;
  file_count: number;
  excerpt: string | null;
  tags: string[];
  cover: string | null;
  series_title: string | null;
  series_position: number | null;
  coverFile: string | null;
}

export interface ShelfSemanticEvidence {
  field: string;
  text: string;
  score: number;
}

export interface ShelfMatch {
  work: string;
  score: number;
  matched_version: number | null;
  historical_matches: number;
  semantic: ShelfSemanticEvidence | null;
}

export type ShelfIntent =
  | { kind: "author"; label: string }
  | { kind: "series"; label: string; series: string | null };

export interface ShelfSearchView {
  page: { items: unknown[]; next: ShelfCursor | null; total: number | null };
  intent: ShelfIntent | null;
  matches: ShelfMatch[];
  cards: ShelfCard[];
}

export interface ShelfDecoration {
  work: string;
  match_fields: string[];
  score_reasons: ScoreReason[];
  highlights: SearchHighlight[];
  field_score: number;
}

export function libraryV2Status(): Promise<LibraryV2Status> {
  return invoke<LibraryV2Status>("library_v2_status");
}

export function libraryV2Search(search: ShelfTextSearch, query: ShelfListQuery): Promise<ShelfSearchView> {
  return invoke<ShelfSearchView>("library_v2_search", { search, query });
}

export function libraryV2Decorations(text: string, matches: ShelfMatch[]): Promise<ShelfDecoration[]> {
  return invoke<ShelfDecoration[]>("library_v2_decorations", { text, matches });
}

/** 保存した版の一つ。`version` は取得元の改稿の順（1 から）。 */
export interface ShelfVersion {
  snapshot: string;
  version: number;
  /** ミリ秒。 */
  saved_at: number;
  text_length: number | null;
}

/** 作品の画面の頭と版の一覧。 */
export type ShelfWork = ShelfCard & {
  /** 取得元の説明（HTML のこともある）。 */
  caption: string | null;
  authorKey: string | null;
  seriesKey: string | null;
  /** 古い順。 */
  versions: ShelfVersion[];
};

/** 本文の一ページ。画像は `data-local-path` にファイルの識別子（添付は `識別子/印`）を持つ。 */
export interface ShelfContentPage {
  page: number;
  pageCount: number;
  html: string;
  /** 本文全体の平文の字数。 */
  totalPlainTextChars: number;
  /** pixiv の原稿のページが何ページ目（0 から）から始まるか。 */
  sourcePageStarts: number[];
}

/** 作品の画像・添付の一件。 */
export interface ShelfMedia {
  kind: "media" | "file";
  role: string;
  name: string | null;
  mime_type: string | null;
  length: number;
  /** ファイルの識別子。未保存なら null。 */
  file: string | null;
}

export function libraryV2Work(work: string): Promise<ShelfWork> {
  return invoke<ShelfWork>("library_v2_work", { work });
}

/** `snapshot` を省くと今読む本文（有効な編集があればそれ）。`page` は 0 から。 */
export function libraryV2Content(work: string, snapshot: string | null, page: number): Promise<ShelfContentPage> {
  return invoke<ShelfContentPage>("library_v2_content", { work, snapshot, page });
}

/** 本文全体の見出し（読む画面の目次）。 */
export function libraryV2Outline(work: string, snapshot: string | null): Promise<ReaderOutlineEntry[]> {
  return invoke<ReaderOutlineEntry[]>("library_v2_outline", { work, snapshot });
}

/** 本文内検索（読む画面）。 */
export function libraryV2SearchContent(work: string, snapshot: string | null, query: string, limit = 50): Promise<ReaderSearchHit[]> {
  return invoke<ReaderSearchHit[]>("library_v2_search_content", { work, snapshot, query, limit });
}

export function libraryV2Media(work: string, snapshot: string | null): Promise<ShelfMedia[]> {
  return invoke<ShelfMedia[]>("library_v2_media", { work, snapshot });
}

export function libraryV2VersionText(work: string, snapshot: string): Promise<string> {
  return invoke<string>("library_v2_version_text", { work, snapshot });
}

export function libraryV2Original(work: string, snapshot: string | null): Promise<string | null> {
  return invoke<string | null>("library_v2_original", { work, snapshot });
}

/** 棚のファイルの識別子を、画面が読める URL にする。 */
export function shelfFileUrl(fingerprint: string | null | undefined): string | null {
  return fingerprint ? convertFileSrc(fingerprint, "piep-library") : null;
}

/**
 * 本文の `data-local-path`（`識別子` か `識別子/印`）を URL にする。棚の識別子でないもの
 * （旧棚のパスなど）は読まない。
 */
export function shelfBodyUrl(localPath: string | null | undefined): string | null {
  const fingerprint = localPath?.split("/", 1)[0] ?? "";
  return /^[0-9a-f]{64}$/.test(fingerprint) ? shelfFileUrl(fingerprint) : null;
}
