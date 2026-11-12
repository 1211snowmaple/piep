import { expect, it } from "vitest";
import type { PcMetadata } from "./pcApi";
import { readerMetadataForVersion, readerVersions } from "./readerVersion";

it("版を選ぶとその版の本文情報と原文を使い、最新版の編集情報を引き継がない", () => {
  const metadata = { work: { currentVersion: 2, contentHash: "v2", textLength: 200, assetCount: 3 }, versions: [{ version: 1, contentHash: "v1", textLength: 100, assetCount: 1, createdAt: "" }], isEdited: true, activeEditId: 7, activeEditHash: "edit" } as PcMetadata;
  expect(readerMetadataForVersion(metadata, null)).toBe(metadata);
  expect(readerMetadataForVersion(metadata, 1)).toMatchObject({ work: { currentVersion: 1, contentHash: "v1", textLength: 100, assetCount: 1 }, isEdited: false, activeEditId: null, activeEditHash: null });
  expect(readerVersions(metadata).map((entry) => entry.version)).toEqual([1, 2]);
  expect(readerMetadataForVersion(metadata, 2)).toMatchObject({ work: { contentHash: "v2" }, activeEditHash: null });
  expect(() => readerMetadataForVersion(metadata, 9)).toThrow("指定された版がありません");
  expect(metadata.activeEditHash).toBe("edit");
});
