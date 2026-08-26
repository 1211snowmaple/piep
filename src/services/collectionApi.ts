import { invoke } from "@tauri-apps/api/core";
import type {
  CollectionKind,
  CollectionNameCandidate,
  CollectionSuggestion,
  CollectionSweepResult,
  WorkCollection,
  WorkCollectionInput,
  WorkCollectionMemberInput,
  WorkCollectionSummary,
  WorkKey,
  WorkLink,
} from "@/types/collections";

export function listWorkCollections(): Promise<WorkCollectionSummary[]> {
  return invoke<WorkCollectionSummary[]>("db_list_work_collections");
}

export function getWorkCollection(collectionId: string): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_get_work_collection", { collectionId });
}

export function upsertWorkCollection(input: WorkCollectionInput): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_upsert_work_collection", { input });
}

export function deleteWorkCollection(collectionId: string): Promise<void> {
  return invoke<void>("db_delete_work_collection", { collectionId });
}

export function addWorkCollectionMembers(collectionId: string, members: WorkCollectionMemberInput[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_add_work_collection_members", { collectionId, members });
}

/** 棚で選んだ作品を、そのまま束へ入れる。並びは投稿日順になる。 */
export function addDownloadsToCollection(collectionId: string, downloadIds: number[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_add_downloads_to_collection", { collectionId, downloadIds });
}

/** 選んだ作品から、新しい束をひとつ作る。作成と追加で1往復。 */
export function createCollectionFromDownloads(
  name: string,
  collectionKind: CollectionKind,
  downloadIds: number[],
): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_create_collection_from_downloads", { name, collectionKind, downloadIds });
}

export function removeWorkCollectionMembers(collectionId: string, members: WorkKey[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_remove_work_collection_members", { collectionId, members });
}

/** 投稿日順か、題名の連番順に一度で整える。話数の語彙は保存側にしか無い。 */
export function sortWorkCollectionMembers(collectionId: string, mode: "published" | "episode"): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_sort_work_collection_members", { collectionId, mode });
}

export function reorderWorkCollectionMembers(collectionId: string, members: WorkKey[]): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_reorder_work_collection_members", { collectionId, members });
}

export function listCollectionsForWork(source: string, sourceId: string): Promise<WorkCollectionSummary[]> {
  return invoke<WorkCollectionSummary[]>("db_list_collections_for_work", { source, sourceId });
}

export function refreshWorkLinks(downloadId: number): Promise<WorkLink[]> {
  return invoke<WorkLink[]>("db_refresh_work_links", { downloadId });
}

export function listWorkLinksForWork(source: string, sourceId: string): Promise<WorkLink[]> {
  return invoke<WorkLink[]>("db_list_work_links_for_work", { source, sourceId });
}

export function generateCollectionSuggestion(seedDownloadIds: number[], limit = 60): Promise<CollectionSuggestion> {
  return invoke<CollectionSuggestion>("db_generate_collection_suggestion", {
    request: { seedDownloadIds, limit },
  });
}

/**
 * すでにあるコレクションに、名前の案を出し直す。
 *
 * 束は作ったあとで中身が変わる。作品を足せば共有タグが変わり、外せば題名の
 * 共通部分も変わる。作ったときの名前に縛られる理由は無い。保存はしない。
 */
export function proposeCollectionNames(collectionId: string): Promise<CollectionNameCandidate[]> {
  return invoke<CollectionNameCandidate[]>("db_propose_collection_names", { collectionId });
}

/**
 * 棚全体を走査して、束の候補を作り直す。
 *
 * 意味索引を全部読むので、画面を開いた瞬間に走らせるものではない。
 * 利用者が「探す」と言ったときだけ動かす。
 */
export function sweepCollectionCandidates(): Promise<CollectionSweepResult> {
  return invoke<CollectionSweepResult>("db_sweep_collection_candidates");
}

/**
 * 走査で出た候補を、まとめて閉じる。
 *
 * 300件を1件ずつ閉じる人はいない。消えるのは下書きだけで、「二度と出さない」
 * とは記録しない — 規則が変われば、また出てくる。
 */
export function dismissSweptSuggestions(track?: "sequence" | "theme"): Promise<number> {
  return invoke<number>("db_dismiss_swept_suggestions", { track: track ?? null });
}

export function listCollectionSuggestions(stateFilter: "pending" | "accepted" | "rejected" | "all" = "pending"): Promise<CollectionSuggestion[]> {
  return invoke<CollectionSuggestion[]>("db_list_collection_suggestions", { stateFilter });
}

export function acceptCollectionSuggestion(input: {
  suggestionId: string;
  name?: string | null;
  collectionKind?: CollectionKind | null;
  memberKeys?: WorkKey[] | null;
}): Promise<WorkCollection> {
  return invoke<WorkCollection>("db_accept_collection_suggestion", { input });
}

/** 既定の案をそのまま採るときは名前を送らない。名前を常に送ると backend は
 * 利用者が編集したと解釈し、自動案まで `manual` に変えてしまう。 */
export function suggestionNameOverride(proposedName: string, selectedName: string): string | undefined {
  return selectedName === proposedName ? undefined : selectedName;
}

/** `memberKeys` limits the negative feedback to the works the reader left
 *  ticked, so unticking the ones that do belong keeps them eligible later. */
export function rejectCollectionSuggestion(suggestionId: string, memberKeys?: WorkKey[]): Promise<boolean> {
  return invoke<boolean>("db_reject_collection_suggestion", { suggestionId, memberKeys: memberKeys ?? null });
}

/** Removes the draft from the inbox without recording negative feedback, so
 *  the same works can be suggested again later. */
export function dismissCollectionSuggestion(suggestionId: string): Promise<boolean> {
  return invoke<boolean>("db_dismiss_collection_suggestion", { suggestionId });
}

export function listCollectionsForPerson(source: string, personKey: string): Promise<WorkCollectionSummary[]> {
  return invoke<WorkCollectionSummary[]>("db_list_collections_for_person", { source, personKey });
}
