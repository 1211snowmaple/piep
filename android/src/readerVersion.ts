import type { PcMetadata } from "./pcApi";

/** 読む画面で選べる版。棚は今の版も含めて返す。 */
export function readerVersions(metadata: PcMetadata): PcMetadata["versions"] {
  return metadata.versions;
}

export function readerMetadataForVersion(metadata: PcMetadata, selected: number | null): PcMetadata {
  if (selected === null) return metadata;
  const version = readerVersions(metadata).find((entry) => entry.version === selected);
  if (!version) throw new Error("指定された版がありません");
  return { ...metadata, work: { ...metadata.work, currentVersion: version.version, snapshotId: version.snapshotId, textLength: version.textLength ?? 0, edited: false, editId: null }, activeEditHash: null };
}
