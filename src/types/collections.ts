import type { DownloadEntry, SourceKind } from "@/types/library";

export interface WorkKey {
  source: string;
  sourceId: string;
}

export type CollectionKind = "ordered" | "unordered";

/** 表紙の作り方。既定はメンバーの表紙を並べる `mosaic`。 */
export type CollectionCoverMode = "mosaic" | "spine" | "single" | "sigil" | "file";

/** 束の出自。読む順のある続き物か、味が同じテーマか、手で作ったか。 */
export type CollectionTrack = "manual" | "sequence" | "theme";

/** 名前がどこから来たか。`manual` は自動命名で上書きしない。 */
export type CollectionNameSource = "manual" | "title" | "series" | "tags" | "author" | "llm";

/** モザイク表紙の1マス。表紙が無いメンバーも紋を描く材料を持つ。 */
export interface CollectionCoverTile {
  source: SourceKind;
  sourceId: string;
  title: string;
  authorName: string;
  coverPath: string | null;
}

export interface WorkCollectionSummary {
  id: string;
  name: string;
  description: string | null;
  collectionKind: CollectionKind;
  coverDownloadId: number | null;
  coverPath: string | null;
  coverMode: CollectionCoverMode;
  coverImagePath: string | null;
  /** 並び順の先頭から最大4件。表紙が無いメンバーも席を残す。 */
  coverTiles: CollectionCoverTile[];
  nameSource: CollectionNameSource;
  track: CollectionTrack;
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
  /** 保存済みメンバーの作品そのもの。棚と同じ `WorkCard` に渡せる。 */
  work: DownloadEntry | null;
  /** 同じ作品の別版。続きではないので、行の中に畳む。 */
  editions: DownloadEntry[];
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
  coverMode?: CollectionCoverMode | null;
  coverImagePath?: string | null;
  nameSource?: CollectionNameSource | null;
  track?: CollectionTrack | null;
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

/** 束の名前の案。一つに決めず、どこから来た案かを添えて並べる。 */
export interface CollectionNameCandidate {
  source: CollectionNameSource;
  name: string;
  label: string;
}

export interface CollectionSuggestion {
  id: string;
  proposedName: string;
  nameOptions: CollectionNameCandidate[];
  collectionKind: CollectionKind;
  track: CollectionTrack;
  origin: "seed" | "sweep" | string;
  /** 「なぜこれが束なのか」の一行。確度%の代わりに画面へ出す。 */
  evidenceSummary: string;
  score: number;
  ruleVersion: string;
  state: "pending" | "accepted" | "rejected";
  members: CollectionSuggestionMember[];
  createdAt: string;
  updatedAt: string;
}

/**
 * 棚の走査の結果。
 *
 * 束にならなかったものも返る。「催眠」759作はまとまりではなく絞り込みの結果
 * なので束にはしないが、確かに一つの見方ではあるので保存した検索として勧める。
 */
export interface CollectionSweepResult {
  bundles: CollectionSuggestion[];
  savedSearchSuggestions: SavedSearchSuggestion[];
}

/** 束にするには大きすぎるタグ。保存した検索としてなら意味がある。 */
export interface SavedSearchSuggestion {
  tag: string;
  workCount: number;
  /** なぜ束にしなかったのかの一行。 */
  reason: string;
}
