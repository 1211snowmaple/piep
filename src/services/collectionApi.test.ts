import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  acceptCollectionSuggestion,
  addWorkCollectionMembers,
  generateCollectionSuggestion,
  listCollectionsForPerson,
  rejectCollectionSuggestion,
  reorderWorkCollectionMembers,
  suggestionNameOverride,
} from "@/services/collectionApi";
import { exportCollectionEpub } from "@/services/epubApi";

describe("collectionApi", () => {
  beforeEach(() => invoke.mockReset());

  it("keeps stable provider work keys in collection mutations", async () => {
    invoke.mockResolvedValue({ id: "collection-1", members: [] });
    const members = [{ source: "pixiv", sourceId: "123", addedBy: "manual" as const }];
    await addWorkCollectionMembers("collection-1", members);
    expect(invoke).toHaveBeenLastCalledWith("db_add_work_collection_members", {
      collectionId: "collection-1",
      members,
    });

    const order = [
      { source: "fanbox", sourceId: "9" },
      { source: "pixiv", sourceId: "123" },
    ];
    await reorderWorkCollectionMembers("collection-1", order);
    expect(invoke).toHaveBeenLastCalledWith("db_reorder_work_collection_members", {
      collectionId: "collection-1",
      members: order,
    });
  });

  it("sends suggestion seeds and the user's final member selection separately", async () => {
    invoke.mockResolvedValue({ id: "suggestion-1", members: [] });
    await generateCollectionSuggestion([4, 8], 40);
    expect(invoke).toHaveBeenLastCalledWith("db_generate_collection_suggestion", {
      request: { seedDownloadIds: [4, 8], limit: 40 },
    });

    const memberKeys = [{ source: "pixiv", sourceId: "4" }];
    await acceptCollectionSuggestion({ suggestionId: "suggestion-1", memberKeys });
    expect(invoke).toHaveBeenLastCalledWith("db_accept_collection_suggestion", {
      input: { suggestionId: "suggestion-1", memberKeys },
    });
  });

  it("does not turn an untouched automatic suggestion name into a manual name", () => {
    expect(suggestionNameOverride("題名からの案", "題名からの案")).toBeUndefined();
    expect(suggestionNameOverride("題名からの案", "利用者が直した名前")).toBe("利用者が直した名前");
  });

  /** 却下は「提案まるごと」ではなく、利用者が残した作品だけを対象にする。
   *  チェックを外した作品まで恒久ブロックしないための境界である。 */
  it("scopes a rejection to the works the reader left selected", async () => {
    invoke.mockResolvedValue(true);
    const memberKeys = [{ source: "pixiv", sourceId: "7" }];
    await rejectCollectionSuggestion("suggestion-1", memberKeys);
    expect(invoke).toHaveBeenLastCalledWith("db_reject_collection_suggestion", {
      suggestionId: "suggestion-1",
      memberKeys,
    });

    await rejectCollectionSuggestion("suggestion-2");
    expect(invoke).toHaveBeenLastCalledWith("db_reject_collection_suggestion", {
      suggestionId: "suggestion-2",
      memberKeys: null,
    });
  });

  it("asks the author's collections by the stable person key", async () => {
    invoke.mockResolvedValue([]);
    await listCollectionsForPerson("fanbox", "creator-9");
    expect(invoke).toHaveBeenLastCalledWith("db_list_collections_for_person", {
      source: "fanbox",
      personKey: "creator-9",
    });
  });

  /** 欠落作品があっても、利用者が承知すれば書き出せる。既定は中止のまま。 */
  it("carries the skip-missing decision into the collection EPUB export", async () => {
    invoke.mockResolvedValue("C:/out/collection.epub");
    await exportCollectionEpub("collection-1", "__auto__", "C:/out");
    expect(invoke).toHaveBeenLastCalledWith("export_collection_epub", {
      collectionId: "collection-1", templateName: "__auto__", outputDir: "C:/out", compressOptions: null, skipMissing: false,
    });

    await exportCollectionEpub("collection-1", "__auto__", "C:/out", true);
    expect(invoke).toHaveBeenLastCalledWith("export_collection_epub", {
      collectionId: "collection-1", templateName: "__auto__", outputDir: "C:/out", compressOptions: null, skipMissing: true,
    });
  });
});
