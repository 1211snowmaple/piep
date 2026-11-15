import type { QueryClient } from "@tanstack/react-query";
import { getDemoEditor } from "@/mocks/demoData";
import { activateWorkEdit, discardWorkDraft, getAssetUrl, getEditorDocument, importWorkAsset, saveWorkDraft } from "@/services/dbApi";
import {
  libraryV2EditPointer,
  libraryV2Editor,
  libraryV2ImportEditImage,
  libraryV2SaveEditor,
  libraryV2Work,
  shelfFileUrl,
  type ShelfEditorMedia,
} from "@/services/libraryV2Api";
import { invalidateShelf } from "@/features/library/shelfV2";
import type { AssetEntry, EditorDocument, WorkBlock, WorkBlockInput, WorkEditRevision } from "@/types/library";

/**
 * 編集画面が読み書きする先。旧棚と新しい棚で、開く・下書きの保存・反映・下書きを捨てる・画像の
 * 取り込みの口だけを差し替える。段の編集・取り消し・プレビュー・校正は同じ画面のまま。
 */

/** 編集画面の画像。新しい棚の画像は `shelfFile`（ファイルの識別子）で引く。 */
export type EditorAsset = AssetEntry & { shelfFile?: string };

/** 編集画面の画像の URL。新しい棚の画像はファイルの識別子から、旧棚は置き場から。 */
export function editorAssetUrl(asset: EditorAsset): string | null {
  return asset.shelfFile ? shelfFileUrl(asset.shelfFile) : getAssetUrl(asset.localPath);
}

/** 編集画面が使う作品の情報。 */
export interface EditorWork {
  /** 読み手に見えている題（反映中の編集の題を含む）。 */
  title: string;
  source: string;
  currentVersion: number;
  seriesTitle?: string | null;
  authorName: string;
}

export type EditorRevisionHead = Pick<WorkEditRevision, "title" | "baseVersion">;

export interface EditorDoc {
  work: EditorWork;
  assets: EditorAsset[];
  activeRevision: EditorRevisionHead | null;
  draftRevision: EditorRevisionHead | null;
  baseVersion: number;
  blocks: WorkBlock[];
}

export interface EditorSource {
  /** 問い合わせの鍵。旧棚は番号、新しい棚は UUID。 */
  key: number | string;
  shelf: boolean;
  ready: boolean;
  /** 作品の画面への道。 */
  workPath: string;
  load: () => Promise<EditorDoc>;
  saveDraft: (doc: EditorDoc, title: string | null, blocks: WorkBlockInput[]) => Promise<void>;
  /** 保存して反映する。 */
  publish: (doc: EditorDoc, title: string | null, blocks: WorkBlockInput[]) => Promise<void>;
  /** 下書きを捨て、開き直した中身を返す。 */
  discard: () => Promise<EditorDoc>;
  importImage: (doc: EditorDoc, path: string) => Promise<EditorAsset>;
  /** 保存・反映の後に古くする問い合わせ。 */
  afterSave: (queryClient: QueryClient) => void;
  afterPublish: (queryClient: QueryClient) => void;
}

function fromLegacy(document: EditorDocument): EditorDoc {
  return {
    work: document.download,
    assets: document.assets,
    activeRevision: document.activeRevision,
    draftRevision: document.draftRevision,
    baseVersion: document.baseVersion,
    blocks: document.blocks,
  };
}

export function legacyEditorSource(id: number, runtime: boolean): EditorSource {
  const save = (doc: EditorDoc, title: string | null, blocks: WorkBlockInput[]) => saveWorkDraft(id, doc.baseVersion, title, blocks);
  return {
    key: id,
    shelf: false,
    ready: Number.isFinite(id),
    workPath: `/works/${id}`,
    load: async () => fromLegacy(runtime ? await getEditorDocument(id) : getDemoEditor(id)),
    saveDraft: async (doc, title, blocks) => {
      if (runtime) await save(doc, title, blocks);
    },
    publish: async (doc, title, blocks) => {
      if (!runtime) return;
      const revision = await save(doc, title, blocks);
      await activateWorkEdit(revision.id);
    },
    discard: async () => {
      if (!runtime) return fromLegacy(getDemoEditor(id));
      await discardWorkDraft(id);
      return fromLegacy(await getEditorDocument(id));
    },
    importImage: (_doc, path) => importWorkAsset(id, path),
    afterSave: (queryClient) => {
      queryClient.invalidateQueries({ queryKey: ["editor-document", id] });
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", id] });
    },
    // 反映は本文を差し替えるので、カードが出している字数と版の印も変わる。
    // それらは棚と作者ページの一覧から引いているため、そこも古くしておく。
    afterPublish: (queryClient) => {
      for (const key of [["reader-metadata", id], ["reader-content-page", id], ["reader-content-search", id], ["editor-document", id], ["library"], ["entity-works"]]) {
        queryClient.invalidateQueries({ queryKey: key });
      }
    },
  };
}

function shelfAsset(media: ShelfEditorMedia): EditorAsset {
  return {
    id: media.id,
    downloadId: 0,
    assetType: "illustration",
    filename: media.name,
    localPath: "",
    originalUrl: null,
    mimeType: media.mimeType,
    fileSizeBytes: 0,
    shelfFile: media.file,
  };
}

/** 作品ごとの、最後に見た編集の土台と世代。画面を作り直しても（同じ作品なら）引き継ぐ。 */
const shelfEditStates = new Map<string, { base: string; generation: number }>();

/**
 * 新しい棚の作品の編集（正本に切り替えた棚だけ書ける）。保存は今の世代を添えて送り、
 * 返ってきた世代を次の保存に使う（自動保存が開き直しを待たずに続けて書けるように）。
 */
export function shelfEditorSource(work: string): EditorSource {
  let state = shelfEditStates.get(work) ?? { base: "", generation: 0 };
  const remember = (next: typeof state) => {
    state = next;
    shelfEditStates.set(work, next);
  };
  const media = (doc: EditorDoc): ShelfEditorMedia[] => doc.assets
    .filter((asset) => asset.shelfFile)
    .map((asset) => ({ id: asset.id, file: asset.shelfFile!, name: asset.filename, mimeType: asset.mimeType }));
  const load = async (): Promise<EditorDoc> => {
    const [view, head] = await Promise.all([libraryV2Editor(work), libraryV2Work(work)]);
    remember({ base: view.base, generation: view.generation });
    const revision = (value: typeof view.active) => value && { title: value.title, baseVersion: value.baseVersion };
    return {
      work: {
        title: head.title,
        source: head.provider,
        currentVersion: head.version_number,
        seriesTitle: head.series_title,
        authorName: head.author_name,
      },
      assets: view.media.map(shelfAsset),
      activeRevision: revision(view.active),
      draftRevision: revision(view.draft),
      baseVersion: view.baseVersion,
      blocks: view.blocks,
    };
  };
  const save = async (doc: EditorDoc, title: string | null, blocks: WorkBlockInput[], publish: boolean) => {
    const result = await libraryV2SaveEditor(work, {
      base: state.base,
      expected: state.generation,
      title,
      blocks,
      media: media(doc),
      publish,
    });
    remember({ ...state, generation: result.generation });
  };
  return {
    key: work,
    shelf: true,
    ready: true,
    workPath: `/works/${work}`,
    load,
    saveDraft: (doc, title, blocks) => save(doc, title, blocks, false),
    publish: (doc, title, blocks) => save(doc, title, blocks, true),
    discard: async () => {
      await libraryV2EditPointer(work, state.generation, "discard");
      return load();
    },
    importImage: async (doc, path) => {
      const imported = await libraryV2ImportEditImage(path);
      const id = doc.assets.reduce((max, asset) => Math.max(max, asset.id), 0) + 1;
      return shelfAsset({ ...imported, id });
    },
    afterSave: (queryClient) => {
      queryClient.invalidateQueries({ queryKey: ["editor-document", work] });
      queryClient.invalidateQueries({ queryKey: ["library-v2-work", work] });
    },
    afterPublish: (queryClient) => { void invalidateShelf(queryClient); },
  };
}
