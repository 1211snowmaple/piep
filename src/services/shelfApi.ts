import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ReaderOutlineEntry, ReaderSearchHit, ScoreReason, SearchHighlight, SearchSuggestResult, WorkBlock, WorkBlockInput } from "@/types/library";
import type { PdfPageImage } from "@/services/dbApi";
import type { RevisionMetadata } from "@/services/workInspectionApi";

/**
 * 棚（読み取り専用の写し）の命令。`src-tauri/src/commands/shelf.rs` と対になる。
 *
 * 作品は棚の ID（UUID の文字列）で指し、画像はファイルの識別子（中身の SHA-256 の
 * 16 進）で指す。
 */

/** 棚の作品の ID（UUID）。プレビューの見本は番号なので、ID の形で見分ける。 */
export const SHELF_WORK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** 作者のシリーズの一覧の一ページの件数。 */
export const ENTITY_SERIES_PAGE_SIZE = 60;

export type ShelfState =
  | { state: "starting" }
  | { state: "opening"; path: string }
  | { state: "indexing"; path: string }
  | { state: "ready"; path: string }
  | { state: "failed"; path: string; message: string };

export type ShelfSort =
  | "saved" | "published" | "updated" | "title" | "author"
  | "length" | "size" | "media" | "version" | "series_order" | "relevance";

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
  /** 読みかけ（棚の読書位置があり、読み終えていない）。 */
  reading?: boolean | null;
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
  /** 作者の棚の ID。役を問わず、その作者が載っている作品。 */
  person?: string | null;
  /** シリーズの棚の ID。 */
  series?: string | null;
}

export interface ShelfListQuery {
  sort: ShelfSort;
  descending: boolean;
  limit: number;
  after: ShelfCursor | null;
  /** ページ番号で開くときに読み飛ばす件数（`after` とは一緒に使わない）。 */
  offset?: number;
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
  /** 取得元の作者の ID。作者の画面へ移るのに使う。 */
  author_key?: string | null;
  coverFile: string | null;
  /** 作者のアイコンのファイルの識別子（16 進）。 */
  authorIconFile?: string | null;
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

export function shelfStatus(): Promise<ShelfState> {
  return invoke<ShelfState>("shelf_status");
}

/** 作品ごとの利用者の状態。 */
export interface ShelfUserState {
  favorite: boolean;
  watching: boolean;
  generation: number;
}

/** お気に入り・更新の監視を変える。書かない項目は今のまま。 */
export function shelfSetUserState(work: string, change: { favorite?: boolean; watching?: boolean }): Promise<ShelfUserState> {
  return invoke<ShelfUserState>("shelf_set_user_state", {
    request: crypto.randomUUID(),
    work,
    favorite: change.favorite ?? null,
    watching: change.watching ?? null,
  });
}

/** 棚の読書位置。ページは 1 から、`ratio` はそのページの中の割合（0〜1）。 */
export interface ShelfReading {
  page: number;
  ratio: number;
  finished: boolean;
  /** このアプリが書いた位置か。そうなら端末に残した細かい位置を使う。 */
  mine: boolean;
  /** ミリ秒。 */
  updatedAt: number;
}

/** 読書位置。`snapshot` を省くと今読む本文。ほかの本文の位置なら null。 */
export function shelfReadingPosition(work: string, snapshot: string | null): Promise<ShelfReading | null> {
  return invoke<ShelfReading | null>("shelf_reading_position", { work, snapshot });
}

export function shelfSetReadingPosition(work: string, snapshot: string | null, page: number, ratio: number, finished: boolean): Promise<void> {
  return invoke<void>("shelf_set_reading_position", { request: crypto.randomUUID(), work, snapshot, page, ratio, finished });
}

/** 読みかけから外す（読書位置を消す）。外した数を返す。 */
export function shelfClearReadingPositions(works: string[]): Promise<number> {
  return invoke<number>("shelf_clear_reading_positions", { works });
}

/** 作品を棚から外す。 */
export function shelfRemoveWork(work: string): Promise<void> {
  return invoke<void>("shelf_remove_work", { request: crypto.randomUUID(), work });
}

/** 取得元の作者・シリーズの指名。 */
export interface ShelfEntityKey {
  provider: string | null;
  key: string;
}

export function shelfSearch(search: ShelfTextSearch, query: ShelfListQuery, person: ShelfEntityKey | null = null): Promise<ShelfSearchView> {
  return invoke<ShelfSearchView>("shelf_search", { search, query, person });
}

/** 検索欄の候補（`search_suggest` の形）。題の候補の `sourceKey` は棚の作品の ID。 */
export function shelfSearchSuggest(text: string, limit = 8): Promise<SearchSuggestResult> {
  const trimmed = text.trim();
  if (!trimmed) return Promise.resolve({ items: [] });
  return invoke<SearchSuggestResult>("shelf_search_suggest", { text: trimmed, limit });
}

export function shelfDecorations(text: string, matches: ShelfMatch[]): Promise<ShelfDecoration[]> {
  return invoke<ShelfDecoration[]>("shelf_decorations", { text, matches });
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
  /** 有効な編集の基準の版の番号。編集が無ければ null。 */
  editBaseVersion: number | null;
};

/** 編集版が今の版より古い版の上に載っているとき、その基準の版。 */
export function staleEditBaseOf(work: Pick<ShelfWork, "edit_id" | "editBaseVersion" | "version_number">): number | null {
  return work.edit_id !== null && work.editBaseVersion !== null && work.editBaseVersion < work.version_number
    ? work.editBaseVersion
    : null;
}

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

export function shelfWork(work: string): Promise<ShelfWork> {
  return invoke<ShelfWork>("shelf_work", { work });
}

/** `snapshot` を省くと今読む本文（有効な編集があればそれ）。`page` は 0 から。 */
export function shelfContent(work: string, snapshot: string | null, page: number): Promise<ShelfContentPage> {
  return invoke<ShelfContentPage>("shelf_content", { work, snapshot, page });
}

/** 本文全体の見出し（読む画面の目次）。 */
export function shelfOutline(work: string, snapshot: string | null): Promise<ReaderOutlineEntry[]> {
  return invoke<ReaderOutlineEntry[]>("shelf_outline", { work, snapshot });
}

/** 本文内検索（読む画面）。 */
export function shelfSearchContent(work: string, snapshot: string | null, query: string, limit = 50): Promise<ReaderSearchHit[]> {
  return invoke<ReaderSearchHit[]>("shelf_search_content", { work, snapshot, query, limit });
}

/** 作品が入っている一つの並び（コレクション・シリーズ・作者）での、位置と両隣。 */
export interface ShelfNeighbors {
  kind: "collection" | "series" | "person";
  /** コレクション・シリーズ・作者の棚の ID。 */
  id: string;
  /** コレクションの名前・シリーズの題・作者の名前。 */
  title: string;
  /** 読む順のある並びか（シリーズと、読む順ありのコレクション）。 */
  ordered: boolean;
  /** 1 から。 */
  position: number;
  total: number;
  /** 棚にない所属作品の数（コレクションだけ）。 */
  missing: number;
  previous: ShelfCard | null;
  next: ShelfCard | null;
}

/** 読む画面の前後の作品。前も後もない並びは含まない。 */
export function shelfNeighbors(work: string): Promise<ShelfNeighbors[]> {
  return invoke<ShelfNeighbors[]>("shelf_neighbors", { work });
}

export function shelfMedia(work: string, snapshot: string | null): Promise<ShelfMedia[]> {
  return invoke<ShelfMedia[]>("shelf_media", { work, snapshot });
}

/**
 * 作品の添付の PDF の1ページを絵にする（「原本を見る」）。`file` はファイルの識別子、
 * `page` は 0 から。
 */
export function shelfRenderPdfPage(work: string, file: string, page: number, width: number): Promise<PdfPageImage> {
  return invoke<PdfPageImage>("shelf_render_pdf_page", { work, file, page, width });
}

/** 画像・添付の一覧で、紙面を開ける添付（PDF）か。 */
export function isShelfPdf(item: ShelfMedia): item is ShelfMedia & { file: string } {
  return item.kind === "file" && item.file !== null && /\.pdf$/i.test(item.name ?? "");
}

export interface ShelfRevisionSnapshot {
  /** null は未取得。空文字は保存した空の本文。 */
  text: string | null;
  metadata: RevisionMetadata;
}

export function shelfRevisionSnapshot(work: string, snapshot: string): Promise<ShelfRevisionSnapshot> {
  return invoke<ShelfRevisionSnapshot>("shelf_revision_snapshot", { work, snapshot });
}

export function shelfOriginal(work: string, snapshot: string | null): Promise<string | null> {
  return invoke<string | null>("shelf_original", { work, snapshot });
}

/** 棚のファイルの識別子を、画面が読める URL にする。 */
export function shelfFileUrl(fingerprint: string | null | undefined): string | null {
  return fingerprint ? convertFileSrc(fingerprint, "piep-library") : null;
}

/**
 * 本文の `data-local-path`（`識別子` か `識別子/印`）を URL にする。棚の識別子でないもの
 * （保存フォルダーのパスなど）は読まない。
 */
export function shelfBodyUrl(localPath: string | null | undefined): string | null {
  const fingerprint = localPath?.split("/", 1)[0] ?? "";
  return /^[0-9a-f]{64}$/.test(fingerprint) ? shelfFileUrl(fingerprint) : null;
}

/** 作者・シリーズの画面の頭（`EntityHead`）。画像はファイルの識別子。 */
export interface ShelfEntityHead {
  id: string;
  kind: "person" | "series";
  provider: string;
  external_key: string;
  /** 作者の名前、またはシリーズの題。 */
  name: string;
  description: string | null;
  links: string[];
  icon: string | null;
  banner: string | null;
  cover: string | null;
  is_concluded: boolean | null;
  published_content_count: number | null;
  work_count: number;
  /** 今の版。作品にだけ名前の載った作者・シリーズは版を持たない。 */
  snapshot: string | null;
  version_number: number | null;
  /** ミリ秒。 */
  last_checked_at: number | null;
  last_fetched_at: number | null;
  /** 古い順。 */
  versions: ShelfVersion[];
}

export interface ShelfTagCount {
  name: string;
  count: number;
}

/** 作者のシリーズの一件（`SeriesFacet`）。日時はミリ秒。 */
export interface ShelfSeriesFacet {
  id: string;
  provider: string;
  external_key: string;
  name: string;
  work_count: number;
  cover: string | null;
  description: string | null;
  is_concluded: boolean | null;
  sample_title: string | null;
  latest_saved_at: number;
  latest_published_at: number;
  latest_updated_at: number;
  total_text_length: number;
  total_size: number;
  total_media: number;
  total_revisions: number;
}

export interface ShelfSeriesFacetPage {
  items: ShelfSeriesFacet[];
  /** 続きの位置（`offset` に渡す）。 */
  next: number | null;
  total: number;
}

export type ShelfSeriesSort =
  | "work_count" | "downloaded_at" | "source_created_at" | "source_updated_at" | "name"
  | "text_length" | "file_size_bytes" | "asset_count" | "current_version";

/** 取得元の ID から作者・シリーズの頭を引く。棚に無ければ null。 */
export function shelfEntity(kind: "person" | "series", provider: string, key: string): Promise<ShelfEntityHead | null> {
  return invoke<ShelfEntityHead | null>("shelf_entity", { kind, provider, key });
}

/** 取得元と作品の番号から、棚に載っている作品を引く（本文のリンクの宛先）。無ければ null。 */
export function shelfFindWork(provider: string, key: string): Promise<{ id: string; title: string } | null> {
  return invoke<{ id: string; title: string } | null>("shelf_find_work", { provider, key });
}

/** 作者・シリーズの作品に付いたタグ。多い順。 */
export function shelfEntityTags(kind: "person" | "series", entity: string, limit = 200): Promise<ShelfTagCount[]> {
  return invoke<ShelfTagCount[]>("shelf_entity_tags", { kind, entity, limit });
}

/** 作者の作品が属するシリーズの一ページ。`order` を省くと鍵ごとの既定の向き。 */
export function shelfPersonSeries(
  person: string,
  options: { query?: string | null; sort?: ShelfSeriesSort | null; order?: "asc" | "desc" | null; offset?: number | null; limit: number },
): Promise<ShelfSeriesFacetPage> {
  return invoke<ShelfSeriesFacetPage>("shelf_person_series", {
    person,
    query: options.query ?? null,
    sort: options.sort ?? null,
    order: options.order ?? null,
    offset: options.offset ?? null,
    limit: options.limit,
  });
}

/** ライブラリの作者・シリーズの一覧の一件（`EntityRow`）。日時はミリ秒、画像はファイルの識別子。 */
export interface ShelfEntityRow {
  id: string;
  provider: string;
  external_key: string;
  name: string;
  work_count: number;
  description: string | null;
  is_concluded: boolean | null;
  icon: string | null;
  banner: string | null;
  cover: string | null;
  sample_title: string | null;
  updated_at: number | null;
  latest_saved_at: number;
  latest_published_at: number;
  latest_updated_at: number;
  total_text_length: number;
  total_size: number;
  total_media: number;
  total_revisions: number;
  /** 更新監視の登録（移した時点の写し）。 */
  watch: "watching" | "paused" | null;
}

export interface ShelfEntityListPage {
  items: ShelfEntityRow[];
  /** 条件に当てはまる数（ページに依らない）。 */
  total: number;
}

/** 作者・シリーズ自身の性質で絞る条件（`EntityScope`）。 */
export interface ShelfEntityListScope {
  watch?: "watched" | "paused" | "unwatched" | null;
  min_work_count?: number | null;
  concluded?: boolean | null;
}

/**
 * ライブラリの「作者・クリエイター」「シリーズ」タブの一ページ。`filter` は作品の
 * 一覧と同じ絞り込みで、当てはまる作品を持つものだけを、その作品の数で出す。
 */
export function shelfEntities(
  kind: "person" | "series",
  options: {
    query?: string | null;
    filter: ShelfFilter;
    series?: ShelfEntityKey | null;
    scope?: ShelfEntityListScope | null;
    sort?: ShelfSeriesSort | null;
    order?: "asc" | "desc" | null;
    offset?: number | null;
    limit: number;
  },
): Promise<ShelfEntityListPage> {
  return invoke<ShelfEntityListPage>("shelf_entities", {
    kind,
    query: options.query ?? null,
    filter: options.filter,
    series: options.series ?? null,
    scope: options.scope ?? null,
    sort: options.sort ?? null,
    order: options.order ?? null,
    offset: options.offset ?? null,
    limit: options.limit,
  });
}

/** コレクションの表紙のモザイクの一マス（`CollectionTile`）。 */
export interface ShelfCollectionTile {
  provider: string;
  external_key: string;
  title: string;
  author_name: string;
  cover: string | null;
}

/** 作者・シリーズに関わるコレクションの一件（`CollectionSummary`）。日時はミリ秒。 */
export interface ShelfCollectionSummary {
  id: string;
  name: string;
  description: string | null;
  ordered: boolean;
  cover: string | null;
  cover_image: string | null;
  cover_mode: string;
  name_source: string;
  track: string;
  tiles: ShelfCollectionTile[];
  revision: number;
  member_count: number;
  available_count: number;
  total_text_length: number;
  created_at: number;
  updated_at: number;
}

/** 作者・シリーズの作品を一つでも含むコレクション。最後に変えた順。 */
export function shelfEntityCollections(kind: "person" | "series", entity: string): Promise<ShelfCollectionSummary[]> {
  return invoke<ShelfCollectionSummary[]>("shelf_entity_collections", { kind, entity });
}

/** 作品の入ったコレクション（作品の画面の「コレクションに追加」）。 */
export function shelfWorkCollections(work: string): Promise<ShelfCollectionSummary[]> {
  return invoke<ShelfCollectionSummary[]>("shelf_work_collections", { work });
}

/** コレクションの一回（`WorkCollectionMember`）。 */
export interface ShelfCollectionMember {
  provider: string;
  external_key: string;
  /** 棚にある作品。棚に無い回（未保存）は null。 */
  work: ShelfCard | null;
  title: string;
  author_name: string;
  /** 表紙のファイルの識別子。 */
  cover: string | null;
  text_length: number;
  position: number;
  role: string;
  added_by: string;
  pinned: boolean;
  note: string | null;
  missing: boolean;
  created_at: number;
  updated_at: number;
  /** 同じ作品の別版。本文の長い順。 */
  editions: ShelfCard[];
}

export interface ShelfCollection extends ShelfCollectionSummary {
  members: ShelfCollectionMember[];
}

/** ライブラリの「コレクション」タブの一覧（更新の新しい順）。 */
export function shelfCollections(): Promise<ShelfCollectionSummary[]> {
  return invoke<ShelfCollectionSummary[]>("shelf_collections");
}

/** コレクションの画面。`id` は棚のコレクションの ID。 */
export function shelfCollection(id: string): Promise<ShelfCollection> {
  return invoke<ShelfCollection>("shelf_collection", { id });
}

/** 編集画面の画像。`id` は画面の中だけの番号、`file` はファイルの識別子。 */
export interface ShelfEditorMedia {
  id: number;
  file: string;
  name: string;
  mimeType: string | null;
}

/** 編集の版の一つ（下書きか反映中）。 */
export interface ShelfEditorRevision {
  id: string;
  baseVersion: number;
  /** 付け直した題。付け直していなければ null。 */
  title: string | null;
  createdAt: number;
}

/** 編集画面を開くときの中身（下書き、反映中の版、今の版の本文の順）。 */
export interface ShelfEditorView {
  base: string;
  baseVersion: number;
  generation: number;
  active: ShelfEditorRevision | null;
  draft: ShelfEditorRevision | null;
  sourceTitle: string;
  blocks: WorkBlock[];
  media: ShelfEditorMedia[];
}

export interface ShelfEditState {
  active: string | null;
  draft: string | null;
  generation: number;
}

export function shelfEditor(work: string): Promise<ShelfEditorView> {
  return invoke<ShelfEditorView>("shelf_editor", { work });
}

/** 段を下書きに保存する（`publish` なら続けて反映する）。 */
export function shelfSaveEditor(work: string, save: {
  base: string;
  expected: number;
  title: string | null;
  blocks: WorkBlockInput[];
  media: ShelfEditorMedia[];
  publish: boolean;
}): Promise<ShelfEditState> {
  return invoke<ShelfEditState>("shelf_save_editor", { request: crypto.randomUUID(), work, save });
}

/** 作品の編集のようす（下書き・反映中の版・以前の編集版を新しい順に）。 */
export interface ShelfEditHistory {
  generation: number;
  active: ShelfEditorRevision | null;
  draft: ShelfEditorRevision | null;
  archived: ShelfEditorRevision[];
}

export function shelfEditHistory(work: string): Promise<ShelfEditHistory> {
  return invoke<ShelfEditHistory>("shelf_edit_history", { work });
}

/**
 * 編集の指し先の変更。下書きを捨てる・反映をやめる・以前の編集版をもう一度反映する・
 * 以前の編集版を一覧から外す（版そのものは棚に残る）・全部外す。
 */
export type ShelfEditChange =
  | { kind: "discard" | "unpublish" | "forget_all" }
  | { kind: "restore" | "forget"; edit: string };

/** 編集の指し先を変える。 */
export function shelfEditPointer(work: string, expected: number, change: ShelfEditChange): Promise<ShelfEditState> {
  return invoke<ShelfEditState>("shelf_edit_pointer", { request: crypto.randomUUID(), work, expected, change });
}

/** 編集に使う画像を棚へ取り込む。`id` は 0 で返る（画面が番号を付ける）。 */
export function shelfImportEditImage(path: string): Promise<ShelfEditorMedia> {
  return invoke<ShelfEditorMedia>("shelf_import_edit_image", { path });
}

/** 画面が作品を指す鍵（取得元と取得元の ID）。棚に無い回も指せる。 */
export interface ShelfWorkKey {
  source: string;
  sourceId: string;
}

/** コレクションの一回の編集（`CollectionChangeInput`）。None の項目は今のまま。 */
export type ShelfCollectionChange =
  | { kind: "create"; title: string; description: string | null; ordered: boolean; works: ShelfWorkKey[] }
  | { kind: "describe"; title: string; description: string | null; ordered: boolean; name_source: string | null }
  | { kind: "look"; cover_work: ShelfWorkKey | null; cover_mode: string | null; cover_image: string | null }
  | { kind: "add"; works: ShelfWorkKey[] }
  | { kind: "remove"; works: ShelfWorkKey[] }
  | { kind: "reorder"; works: ShelfWorkKey[] }
  | { kind: "sort"; by: "published" | "episode" }
  | { kind: "delete" };

/**
 * コレクションを一回編集する。作るときは `id` に新しい ID を渡す。
 * 編集した後の中身を返す（消したときは null）。送り直しは同じ `request` で一度きりになる。
 */
export function shelfEditCollection(id: string, change: ShelfCollectionChange): Promise<ShelfCollection | null> {
  return invoke<ShelfCollection | null>("shelf_edit_collection", { request: crypto.randomUUID(), id, change });
}

/** 作者・シリーズの今の版を作った取得元の応答（画面の「保存データ」）。 */
export function shelfEntityOriginal(entity: string, snapshot: string): Promise<string | null> {
  return invoke<string | null>("shelf_entity_original", { entity, snapshot });
}
