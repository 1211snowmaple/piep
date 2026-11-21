import type { AssetEntry } from "@/types/library";

export interface RevisionAsset {
  key: string;
  kind: string;
  label: string;
  source: string;
  localPath: string | null;
  available: boolean | null;
  contentHash: string | null;
}
export interface RevisionMetadata {
  /** 本文の指定と資産の対応が未準備なら false。 */
  assetsKnown?: boolean;
  title: string | null;
  caption: string | null;
  author?: string | null;
  series?: string | null;
  bodyStructure?: string | null;
  tags: string[] | null;
  publishedAt: string | null;
  updatedAt: string | null;
  assets: RevisionAsset[];
}
export interface RevisionSnapshot {
  text: string;
  metadata?: RevisionMetadata;
  /** 編集画面で足した絵。反映中の編集を読んだときだけ入る。 */
  addedImages?: AssetEntry[];
}
export interface MissingImageLocation { reference: string; page: number; occurrence: number; excerpt: string }
export interface WorkAcquisitionDetails {
  kind: string;
  status: string;
  reason: string;
  recordedAt: string;
  version: number;
  missingImages: MissingImageLocation[];
  /** Pixiv detail fields the source masked when the work was saved. */
  missingMetadata?: Array<"title" | "caption" | "tags">;
  locationError?: string | null;
}
