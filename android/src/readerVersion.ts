import type { PcMetadata } from "./pcApi";

/** 旧サーバーで履歴に現行版の行がなくても、現行の原文は選べる。 */
export function readerVersions(metadata: PcMetadata): PcMetadata["versions"] {
  if (metadata.versions.some((entry) => entry.version === metadata.work.currentVersion)) return metadata.versions;
  const work = metadata.work;
  return [...metadata.versions, { version: work.currentVersion, contentHash: work.contentHash, textLength: work.textLength, assetCount: work.assetCount, createdAt: "" }];
}

export function readerMetadataForVersion(metadata: PcMetadata, selected: number | null): PcMetadata {
  if (selected === null) return metadata;
  const version = readerVersions(metadata).find((entry) => entry.version === selected);
  if (!version) throw new Error("指定された版がありません");
  return { ...metadata, work: { ...metadata.work, currentVersion: version.version, contentHash: version.contentHash, textLength: version.textLength, assetCount: version.assetCount }, isEdited: false, activeEditId: null, activeEditHash: null };
}
