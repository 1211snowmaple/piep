import { expect, it } from "vitest";
import type { PcMetadata } from "./pcApi";
import { readerMetadataForVersion, readerVersions } from "./readerVersion";

it("版を選ぶとその版の本文情報と原文を使い、最新版の編集情報を引き継がない", () => {
  const metadata = {
    work: { currentVersion: 2, snapshotId: "s2", textLength: 200, edited: true, editId: "e" },
    versions: [{ version: 1, snapshotId: "s1", textLength: 100, savedAt: "" }, { version: 2, snapshotId: "s2", textLength: 200, savedAt: "" }],
    activeEditHash: "edit",
  } as unknown as PcMetadata;
  expect(readerMetadataForVersion(metadata, null)).toBe(metadata);
  expect(readerMetadataForVersion(metadata, 1)).toMatchObject({ work: { currentVersion: 1, snapshotId: "s1", textLength: 100, edited: false, editId: null }, activeEditHash: null });
  expect(readerVersions(metadata).map((entry) => entry.version)).toEqual([1, 2]);
  expect(readerMetadataForVersion(metadata, 2)).toMatchObject({ work: { snapshotId: "s2" }, activeEditHash: null });
  expect(() => readerMetadataForVersion(metadata, 9)).toThrow("指定された版がありません");
  expect(metadata.activeEditHash).toBe("edit");
});
