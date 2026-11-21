import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ShelfEditorView } from "@/services/shelfApi";
import { editorAssetUrl, shelfEditorSource } from "./editorSource";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  shelfEditor: vi.fn(),
  shelfWork: vi.fn(),
  shelfSaveEditor: vi.fn(),
  shelfEditPointer: vi.fn(),
  shelfImportEditImage: vi.fn(),
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  ...api,
}));

const WORK = "0195f1c2-0000-7000-8000-000000000001";
const FILE = "ab".repeat(32);

const view: ShelfEditorView = {
  base: "0195f1c2-0000-7000-8000-0000000000c1",
  baseVersion: 2,
  generation: 3,
  active: null,
  draft: { id: "d", baseVersion: 2, title: "直した題", createdAt: 1 },
  sourceTitle: "元の題",
  blocks: [{ id: 0, editRevisionId: 0, order: 0, blockType: "image", text: null, assetId: 1, attrsJson: null }],
  media: [{ id: 1, file: FILE, name: "絵.png", mimeType: "image/png" }],
};

describe("the editor on the new shelf", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.shelfEditor.mockResolvedValue(view);
    api.shelfWork.mockResolvedValue({ title: "見えている題", provider: "pixiv", version_number: 3, series_title: null, author_name: "作者" });
    api.shelfSaveEditor.mockResolvedValue({ active: null, draft: "d2", generation: 4 });
    api.shelfEditPointer.mockResolvedValue({ active: null, draft: null, generation: 5 });
    api.shelfImportEditImage.mockResolvedValue({ id: 0, file: "cd".repeat(32), name: "新.png", mimeType: "image/png" });
  });

  it("opens the shelf, saves with the generation it last saw, and imports images by file", async () => {
    const source = shelfEditorSource(WORK);
    const doc = await source.load();
    expect(doc.work).toMatchObject({ title: "見えている題", source: "pixiv", currentVersion: 3 });
    expect(doc.draftRevision).toEqual({ title: "直した題", baseVersion: 2 });
    expect(editorAssetUrl(doc.assets[0])).toBe(`http://piep-library.localhost/${FILE}`);

    const blocks = [{ blockType: "image", assetId: 1 }];
    await source.saveDraft(doc, null, blocks);
    expect(api.shelfSaveEditor).toHaveBeenLastCalledWith(WORK, {
      base: view.base, expected: 3, title: null, blocks,
      media: [{ id: 1, file: FILE, name: "絵.png", mimeType: "image/png" }],
      publish: false,
    });
    // 次の保存は返ってきた世代で送る（開き直しを待たない）。
    await source.publish(doc, "題", blocks);
    expect(api.shelfSaveEditor).toHaveBeenLastCalledWith(WORK, expect.objectContaining({ expected: 4, title: "題", publish: true }));

    const image = await source.importImage(doc, "C:/絵/新.png");
    expect(image).toMatchObject({ id: 2, shelfFile: "cd".repeat(32), filename: "新.png" });

    // 画面を作り直しても（同じ作品なら）最後に見た世代で続ける。
    await shelfEditorSource(WORK).discard();
    expect(api.shelfEditPointer).toHaveBeenCalledWith(WORK, 4, { kind: "discard" });
    expect(api.shelfEditor).toHaveBeenCalledTimes(2);
  });
});
