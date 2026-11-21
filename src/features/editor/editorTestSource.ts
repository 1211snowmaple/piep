import type { EditorDoc, EditorSource } from "@/features/editor/editorSource";
import type { EditorDocument, WorkBlockInput, WorkEditRevision } from "@/types/library";

/**
 * 試験用の編集画面の読み書き先。棚の命令の代わりに、渡した関数で開く・保存・反映・下書きを
 * 捨てるを行う。段の編集・取り消し・自動保存などの画面の振る舞いを、棚の形を作らずに確かめる。
 */
export interface EditorTestApi {
  getEditorDocument: (id: number) => Promise<EditorDocument>;
  saveWorkDraft: (id: number, baseVersion: number, title: string | null, blocks: WorkBlockInput[]) => Promise<WorkEditRevision | null>;
  activateWorkEdit: (revision: number) => Promise<unknown>;
  discardWorkDraft?: (id: number) => Promise<unknown>;
}

function toDoc(document: EditorDocument): EditorDoc {
  return {
    work: document.download,
    assets: document.assets,
    activeRevision: document.activeRevision,
    draftRevision: document.draftRevision,
    baseVersion: document.baseVersion,
    blocks: document.blocks,
  };
}

export function editorTestSource(id: number, api: EditorTestApi): EditorSource {
  const save = (doc: EditorDoc, title: string | null, blocks: WorkBlockInput[]) => api.saveWorkDraft(id, doc.baseVersion, title, blocks);
  return {
    key: id,
    shelf: true,
    ready: Number.isFinite(id),
    workPath: `/works/${id}`,
    load: async () => toDoc(await api.getEditorDocument(id)),
    saveDraft: async (doc, title, blocks) => { await save(doc, title, blocks); },
    publish: async (doc, title, blocks) => {
      // 旧棚と同じく、保存が版を返さなければ反映できずに止まる。
      const revision = await save(doc, title, blocks) as WorkEditRevision;
      await api.activateWorkEdit(revision.id);
    },
    discard: async () => {
      await api.discardWorkDraft?.(id);
      return toDoc(await api.getEditorDocument(id));
    },
    importImage: () => Promise.reject(new Error("試験では画像を取り込めません")),
    afterSave: () => undefined,
    afterPublish: () => undefined,
  };
}
