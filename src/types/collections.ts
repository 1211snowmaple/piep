import type { SourceKind } from "@/types/library";

export interface WorkKey {
  source: string;
  sourceId: string;
}

export type CollectionKind = "ordered" | "unordered";

export interface WorkCollectionSummary {
  id: string;
  name: string;
  description: string | null;
  collectionKind: CollectionKind;
  coverDownloadId: number | null;
  coverPath: string | null;
  revision: number;
  memberCount: number;
  availableCount: number;
  totalTextLength: number;
  createdAt: string;
  updatedAt: string;
}

export interface WorkCollectionMember {
  collectionId: string;
  source: SourceKind;
  sourceId: string;
  downloadId: number | null;
  title: string;
  authorName: string;
  coverPath: string | null;
  textLength: number;
  position: number;
  memberRole: "main" | "supplement" | "appendix" | string;
  addedBy: "manual" | "suggestion" | "import" | string;
  pinned: boolean;
  note: string | null;
  missing: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface WorkCollection extends WorkCollectionSummary {
  members: WorkCollectionMember[];
}

export interface WorkCollectionInput {
  id?: string | null;
  name: string;
  description?: string | null;
  collectionKind: CollectionKind;
  coverDownloadId?: number | null;
}

export interface WorkCollectionMemberInput extends WorkKey {
  titleSnapshot?: string | null;
  authorSnapshot?: string | null;
  position?: number | null;
  memberRole?: "main" | "supplement" | "appendix" | null;
  addedBy?: "manual" | "suggestion" | "import" | null;
  pinned?: boolean | null;
  note?: string | null;
}

export interface WorkLink {
  id: number;
  fromSource: string;
  fromSourceId: string;
  fromDownloadId: number | null;
  toSource: string;
  toSourceId: string;
  toDownloadId: number | null;
  relationType: string;
  evidenceType: string;
  anchorText: string | null;
  contextText: string | null;
  confidence: number;
  status: "observed" | "accepted" | "rejected";
  discoveredAt: string;
  updatedAt: string;
}

export interface CollectionSuggestionEvidence {
  kind: "seed" | "official_series" | "content_link" | "same_author" | "title_similarity" | "semantic_similarity" | string;
  label: string;
  contribution: number;
}

export interface CollectionSuggestionMember extends WorkKey {
  downloadId: number | null;
  title: string;
  authorName: string;
  coverPath: string | null;
  textLength: number;
  proposedPosition: number;
  score: number;
  selected: boolean;
  evidence: CollectionSuggestionEvidence[];
}

export interface CollectionSuggestion {
  id: string;
  proposedName: string;
  collectionKind: CollectionKind;
  score: number;
  ruleVersion: string;
  state: "pending" | "accepted" | "rejected";
  members: CollectionSuggestionMember[];
  createdAt: string;
  updatedAt: string;
}
