import { invoke } from "@tauri-apps/api/core";

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
export interface RevisionSnapshot { text: string; metadata?: RevisionMetadata }
export interface MissingImageLocation { reference: string; page: number; occurrence: number; excerpt: string }
export interface WorkAcquisitionDetails {
  kind: string;
  status: string;
  reason: string;
  recordedAt: string;
  version: number;
  missingImages: MissingImageLocation[];
  locationError?: string | null;
}
export const getRevisionSnapshot = (downloadId: number, version: number) => invoke<RevisionSnapshot>("get_revision_snapshot", { downloadId, version });
export const getWorkAcquisitionDetails = (downloadId: number) => invoke<WorkAcquisitionDetails | null>("get_work_acquisition_details", { downloadId });
