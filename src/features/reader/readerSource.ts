import { getAssetUrl, getReaderContentPage, getReaderMetadata, getReaderOutline, isTauriRuntime, searchReaderContent } from "@/services/dbApi";
import { libraryV2Content, libraryV2Outline, libraryV2SearchContent, libraryV2Work, shelfBodyUrl, staleEditBaseOf, type ShelfWork } from "@/services/libraryV2Api";
export { SHELF_WORK_ID } from "@/services/libraryV2Api";
import { getDemoReader } from "@/mocks/demoData";
import { readingKeyOf } from "@/features/library/readingShelf";
import type { ReaderContentPage, ReaderMetadata, ReaderOutlineEntry, ReaderSearchHit } from "@/types/library";

/**
 * 読む画面が読む先。旧棚と新しい棚（試験中・読むだけ）で、本文・目次・本文内検索・作品の頭を
 * 取ってくる口だけを差し替える。読書位置・しおり・組み方・鍵盤の操作は同じ画面のまま。
 */

/** 読む画面の頭に出す作品の情報。 */
export interface ReaderHead {
  /** 作品の画面の ID（旧棚は番号、新しい棚は UUID）。 */
  id: number | string;
  /** 読書位置・しおりを引く鍵（`readingKeyOf`）。旧棚から移した作品は、新しい棚でも旧棚の番号。 */
  readingKey: number | string;
  title: string;
  authorName: string;
  source: string;
  sourceId: string;
  contentType: string;
  /** 取得元の作者の ID。元ページの URL に使う。 */
  authorKey: string;
  sourceCreatedAt: string | null;
  textLength: number;
  currentVersion: number;
  /** 選べる版の番号（新しい順）。 */
  versions: number[];
  /** 今読む本文が編集版か。 */
  isEdited: boolean;
  /** 編集版の基準の版が今の版より古いとき、その版。 */
  staleEditBase: number | null;
  /** 反映していない下書きがあるか。 */
  hasDraft: boolean;
}

/** 作品の頭。旧棚なら旧棚の読む画面の情報も持つ（前後の作品・保存フォルダー）。 */
export interface ReaderHeadData {
  head: ReaderHead;
  legacy: ReaderMetadata | null;
}

/**
 * 作品の頭の問い合わせ。取ってきたものは作品の画面と同じ鍵・同じ形で持ち（旧棚は
 * `reader-metadata`、新しい棚は `library-v2-work`）、読む画面は `select` で読み替える。
 */
export interface ReaderHeadQuery {
  queryKey: readonly unknown[];
  queryFn: () => Promise<unknown>;
  select: (data: unknown) => ReaderHeadData;
}

export interface ReaderSource {
  /** 問い合わせの鍵。旧棚は番号、新しい棚は UUID。読書位置・しおりは頭の `readingKey` で引く。 */
  key: number | string;
  /** 新しい棚の作品。書き換える操作（編集）と、旧棚の保存フォルダー・前後の作品は使えない。 */
  shelf: boolean;
  head: ReaderHeadQuery;
  /** 本文の一ページ。`version` が null なら今読む本文、`page` は 0 から。 */
  page(version: number | null, page: number): Promise<ReaderContentPage>;
  outline(version: number | null): Promise<ReaderOutlineEntry[]>;
  search(term: string, version: number | null): Promise<ReaderSearchHit[]>;
  /** 本文の `data-local-path` を URL にする。 */
  assetUrl(localPath: string | null | undefined): string | null;
}

function legacyHead(doc: ReaderMetadata): ReaderHead {
  const work = doc.download;
  const edit = doc.activeEditRevision;
  return {
    id: work.id,
    readingKey: work.id,
    title: work.title,
    authorName: work.authorName,
    source: work.source,
    sourceId: work.sourceId,
    contentType: work.contentType,
    authorKey: work.personId || work.authorId,
    sourceCreatedAt: work.sourceCreatedAt,
    textLength: work.textLength,
    currentVersion: work.currentVersion,
    versions: doc.versions.map((item) => item.version),
    isEdited: doc.isEdited,
    staleEditBase: edit && edit.baseVersion < work.currentVersion ? edit.baseVersion : null,
    hasDraft: Boolean(doc.draftEditRevision),
  };
}

/** 旧棚の作品。ブラウザのプレビューでは見本の作品を読む。 */
export function legacyReaderSource(id: number): ReaderSource {
  const runtime = isTauriRuntime();
  const demo = () => getDemoReader(id);
  return {
    key: id,
    shelf: false,
    head: {
      queryKey: ["reader-metadata", id],
      queryFn: (): Promise<ReaderMetadata> => runtime
        ? getReaderMetadata(id)
        : Promise.resolve((() => { const sample = demo(); return { download: sample.download, versions: sample.versions, assetCount: sample.assets.length, isEdited: sample.isEdited, activeEditRevision: sample.activeEditRevision, draftEditRevision: null }; })()),
      select: (data) => ({ head: legacyHead(data as ReaderMetadata), legacy: data as ReaderMetadata }),
    },
    page: (version, page) => runtime
      ? getReaderContentPage(id, version, page)
      : Promise.resolve((() => { const sample = demo(); return { page: 0, pageCount: 1, html: sample.html, plainText: sample.plainText, totalPlainTextChars: sample.plainText.length, sourcePageStarts: [0] }; })()),
    outline: (version) => runtime ? getReaderOutline(id, version) : Promise.resolve([]),
    search: (term, version) => runtime ? searchReaderContent(id, term, version) : Promise.resolve([]),
    assetUrl: getAssetUrl,
  };
}

function shelfHead(work: ShelfWork): ReaderHead {
  return {
    id: work.id,
    readingKey: readingKeyOf(work),
    title: work.title,
    authorName: work.author_name,
    source: work.provider,
    sourceId: work.external_key,
    contentType: work.content_type,
    authorKey: work.authorKey ?? "",
    sourceCreatedAt: work.published_at === null ? null : new Date(work.published_at).toISOString(),
    textLength: work.text_length,
    currentVersion: work.version_number,
    versions: [...new Set(work.versions.map((item) => item.version))].sort((left, right) => right - left),
    isEdited: work.edit_id !== null,
    staleEditBase: staleEditBaseOf(work),
    // 下書きは移していない。
    hasDraft: false,
  };
}

/**
 * 新しい棚の作品（試験中・読むだけ）。版の番号は、その番号の版の保存状態（snapshot）に
 * 読み替えて渡す。番号の無い版は今読む本文（有効な編集があればそれ）。
 */
export function shelfReaderSource(id: string): ReaderSource {
  // 版の番号を読み替えるのにも同じ作品の頭を使う。新しい棚は読むだけなので、開いている
  // あいだに変わらない。
  let work: Promise<ShelfWork> | null = null;
  const load = () => {
    work ??= libraryV2Work(id).catch((error: unknown) => { work = null; throw error; });
    return work;
  };
  const snapshotOf = async (version: number | null): Promise<string | null> => {
    if (version === null) return null;
    const found = (await load()).versions.find((item) => item.version === version);
    if (!found) throw new Error(`v${version} は保存されていません`);
    return found.snapshot;
  };
  return {
    key: id,
    shelf: true,
    head: {
      queryKey: ["library-v2-work", id],
      queryFn: load,
      select: (data) => ({ head: shelfHead(data as ShelfWork), legacy: null }),
    },
    page: async (version, page) => {
      const content = await libraryV2Content(id, await snapshotOf(version), page);
      return { ...content, plainText: "" };
    },
    outline: async (version) => libraryV2Outline(id, await snapshotOf(version)),
    search: async (term, version) => libraryV2SearchContent(id, await snapshotOf(version), term),
    assetUrl: shelfBodyUrl,
  };
}
