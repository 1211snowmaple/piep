import { invoke } from "@tauri-apps/api/core";
import type {
  CollectionAdditionResult,
  CollectionKind,
  CollectionNameCandidate,
  CollectionSuggestion,
  CollectionSweepResult,
  WorkKey,
} from "@/types/collections";

export function generateCollectionSuggestion(seedDownloadIds: number[], limit = 60): Promise<CollectionSuggestion> {
  return invoke<CollectionSuggestion>("db_generate_collection_suggestion", {
    request: { seedDownloadIds, limit },
  });
}

/** 新しい棚の作品（UUID）を種に候補を作る。正本に切り替えた棚だけ。 */
export function generateShelfCollectionSuggestion(seedWorks: string[], limit = 60): Promise<CollectionSuggestion> {
  return invoke<CollectionSuggestion>("db_generate_collection_suggestion", {
    request: { seedWorks, limit },
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
 * すでにあるコレクションへ、あとから入れるとよさそうな作品を探す。
 *
 * 束は作った時点で閉じない。新作は毎日届くし、旧作をあとから保存することも
 * ある。押しても保存はしない — 返るのは案だけで、入れるかどうかは利用者が
 * 決める。
 */
export function suggestCollectionAdditions(collectionId: string): Promise<CollectionAdditionResult> {
  return invoke<CollectionAdditionResult>("db_suggest_collection_additions", { collectionId });
}

/**
 * 走査で出た候補を、まとめて片付ける。
 *
 * 消えるのは下書きだけで、「二度と出さない」とは記録しない — 規則が変われば、
 * また出てくる。系統を選んで閉じる引数は持たない。系統を選んで閉じたい人は
 * いなかったので、その道ごとやめた。
 */
export function dismissSweptSuggestions(): Promise<number> {
  return invoke<number>("db_dismiss_swept_suggestions");
}

export function listCollectionSuggestions(stateFilter: "pending" | "accepted" | "rejected" | "all" = "pending"): Promise<CollectionSuggestion[]> {
  return invoke<CollectionSuggestion[]>("db_list_collection_suggestions", { stateFilter });
}

export function acceptCollectionSuggestion(input: {
  suggestionId: string;
  name?: string | null;
  collectionKind?: CollectionKind | null;
  memberKeys?: WorkKey[] | null;
}): Promise<AcceptedCollection> {
  return invoke<AcceptedCollection>("db_accept_collection_suggestion", { input });
}

/** 候補を採って作ったコレクション。新しい棚では ID が UUID。 */
export interface AcceptedCollection {
  id: string;
  name: string;
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
