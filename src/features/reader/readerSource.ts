import { getAssetUrl } from "@/services/dbApi";
import { shelfContent, shelfOutline, shelfReadingPosition, shelfSearchContent, shelfSetReadingPosition, shelfClearReadingPositions, shelfWork, shelfBodyUrl, staleEditBaseOf, type ShelfReading, type ShelfWork } from "@/services/shelfApi";
export { SHELF_WORK_ID } from "@/services/shelfApi";
import { getDemoReader } from "@/mocks/demoData";
import type { ReaderContentPage, ReaderMetadata, ReaderOutlineEntry, ReaderSearchHit } from "@/types/library";

/**
 * 読む画面が読む先。棚とプレビューの見本で、本文・目次・本文内検索・作品の頭を取ってくる口だけを
 * 差し替える。読書位置・しおり・組み方・鍵盤の操作は同じ画面のまま。
 */

/** 読む画面の頭に出す作品の情報。 */
export interface ReaderHead {
  /** 作品の画面の ID（棚は UUID、プレビューの見本は短い文字列）。読書位置・しおりもこれで引く。 */
  id: string;
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

/** 作品の頭。見本なら見本の読む画面の情報も持つ（前後の作品）。 */
export interface ReaderHeadData {
  head: ReaderHead;
}

/**
 * 作品の頭の問い合わせ。取ってきたものは作品の画面と同じ鍵・同じ形で持ち（棚は
 * `shelf-work`、見本は `reader-metadata`）、読む画面は `select` で読み替える。
 */
export interface ReaderHeadQuery {
  queryKey: readonly unknown[];
  queryFn: () => Promise<unknown>;
  select: (data: unknown) => ReaderHeadData;
}

export interface ReaderSource {
  /** 問い合わせの鍵。棚は UUID、見本は短い文字列。読書位置・しおりは頭の `id` で引く。 */
  key: string;
  /** 棚の作品（見本でない）。 */
  shelf: boolean;
  head: ReaderHeadQuery;
  /** 本文の一ページ。`version` が null なら今読む本文、`page` は 0 から。 */
  page(version: number | null, page: number): Promise<ReaderContentPage>;
  outline(version: number | null): Promise<ReaderOutlineEntry[]>;
  search(term: string, version: number | null): Promise<ReaderSearchHit[]>;
  /** 本文の `data-local-path` を URL にする。 */
  assetUrl(localPath: string | null | undefined): string | null;
  /**
   * 棚の読書位置（棚の作品だけ）。ページは 1 から、
   * `ratio` はそのページの中の割合。端末の細かい位置（何行目か）は端末の記録に残す。
   */
  shelfReading?: {
    read(version: number | null): Promise<ShelfReading | null>;
    write(version: number | null, page: number, ratio: number, finished: boolean): Promise<void>;
    /** 読書位置を消す（先頭で閉じたとき。読みかけにしない）。 */
    clear(): Promise<void>;
  };
}

function demoHead(doc: ReaderMetadata): ReaderHead {
  const work = doc.download;
  const edit = doc.activeEditRevision;
  return {
    id: work.id,
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

/** ブラウザのプレビューの見本の作品。 */
export function previewReaderSource(id: string): ReaderSource {
  const demo = () => getDemoReader(id);
  return {
    key: id,
    shelf: false,
    head: {
      queryKey: ["reader-metadata", id],
      queryFn: (): Promise<ReaderMetadata> => Promise.resolve((() => { const sample = demo(); return { download: sample.download, versions: sample.versions, assetCount: sample.assets.length, isEdited: sample.isEdited, activeEditRevision: sample.activeEditRevision, draftEditRevision: null }; })()),
      select: (data) => ({ head: demoHead(data as ReaderMetadata) }),
    },
    page: () => Promise.resolve((() => { const sample = demo(); return { page: 0, pageCount: 1, html: sample.html, plainText: sample.plainText, totalPlainTextChars: sample.plainText.length, sourcePageStarts: [0] }; })()),
    outline: () => Promise.resolve([]),
    search: () => Promise.resolve([]),
    assetUrl: getAssetUrl,
  };
}

function shelfHead(work: ShelfWork): ReaderHead {
  return {
    id: work.id,
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
 * 棚の作品。版の番号は、その番号の版の保存状態（snapshot）に読み替えて渡す。番号の無い版は
 * 今読む本文（有効な編集があればそれ）。
 */
export function shelfReaderSource(id: string): ReaderSource {
  // 版の番号を読み替えるのにも同じ作品の頭を使う。
  let work: Promise<ShelfWork> | null = null;
  const load = () => {
    work ??= shelfWork(id).catch((error: unknown) => { work = null; throw error; });
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
      queryKey: ["shelf-work", id],
      queryFn: load,
      select: (data) => ({ head: shelfHead(data as ShelfWork) }),
    },
    page: async (version, page) => {
      const content = await shelfContent(id, await snapshotOf(version), page);
      return { ...content, plainText: "" };
    },
    outline: async (version) => shelfOutline(id, await snapshotOf(version)),
    search: async (term, version) => shelfSearchContent(id, await snapshotOf(version), term),
    assetUrl: shelfBodyUrl,
    shelfReading: {
      read: async (version) => shelfReadingPosition(id, await snapshotOf(version)),
      write: async (version, page, ratio, finished) => shelfSetReadingPosition(id, await snapshotOf(version), page, ratio, finished),
      clear: async () => { await shelfClearReadingPositions([id]); },
    },
  };
}
