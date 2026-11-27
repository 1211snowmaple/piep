/**
 * 保存ジョブの様子を、候補の行と報せに写す。
 *
 * 保存は Rust のジョブで走り、画面はそれを覗いているだけである。ここにあるのは
 * 覗いた値の読み方で、画面の状態には触らない。
 */
import type { SidebarItem } from "@/features/browser/downloadCandidates";
import type { UpdateJobItemState, UpdateJobSnapshot } from "@/services/updateJobApi";

type RowStatus = Exclude<SidebarItem["status"], undefined>;

const ROW_STATUS: Record<string, RowStatus> = {
  queued: "pending",
  running: "downloading",
  saved: "success",
  done: "success",
  skipped: "skipped",
  failed: "failed",
  held: "held",
};

const STATUS_RANK: Record<RowStatus, number> = {
  pending: 0,
  downloading: 1,
  success: 2,
  skipped: 2,
  failed: 2,
  held: 2,
};

/** 保存しに行く行か。済んだもの・保留は外す。 */
export function isPendingSave(item: SidebarItem): boolean {
  return item.selected && item.status !== "success" && item.status !== "skipped" && item.status !== "held";
}

/**
 * ジョブの項目ひとつの状態を行に当てる。変わらなければ同じ行を返す。
 *
 * 状態の取得は複数回が重なりうる。古い応答があとから戻っても、済んだ行を
 * 「処理中」へ巻き戻さない。棚の照会で付いた「保存済み」よりはジョブを優先する。
 */
export function applyJobItemState(row: SidebarItem, state: UpdateJobItemState): SidebarItem {
  const status = ROW_STATUS[state.status];
  if (!status) return row;
  if (!row.shelfSkipped && row.status) {
    if (STATUS_RANK[row.status] > STATUS_RANK[status]) return row;
    if (row.status === status && (row.error ?? null) === state.error) return row;
  }
  return {
    ...row,
    status,
    shelfSkipped: undefined,
    selected: status === "held" ? false : row.selected,
    error: state.error ?? undefined,
  };
}

/** 取得元の合う項目だけを、ID で引けるようにする。 */
export function jobItemStatesById(states: UpdateJobItemState[], source: string): Map<string, UpdateJobItemState> {
  return new Map(
    states
      .filter((state) => state.source === source && state.sourceId)
      .map((state) => [state.sourceId as string, state]),
  );
}

export type SaveJobOutcome = "canceled" | "paused" | "failed" | "completed";

/** 終わったジョブの数え上げと、どう終わったか。 */
export function summarizeSaveJob(final: UpdateJobSnapshot): {
  outcome: SaveJobOutcome;
  tally: string;
  failed: number;
  held: number;
} {
  const saved = final.savedCount;
  const failed = final.errorCount;
  const held = final.heldCount ?? 0;
  const skipped = Math.max(0, final.processed - saved - failed - held);
  const tally = `保存 ${saved} · 保存済み ${skipped} · 失敗 ${failed}${held ? ` · 保留 ${held}（更新 → 保留・非表示から再確認できます）` : ""}`;
  const outcome: SaveJobOutcome =
    final.status === "canceled" || final.status === "canceling"
      ? "canceled"
      : final.status === "paused" || final.status === "auth_required"
        ? "paused"
        : saved === 0 && failed > 0
          ? "failed"
          : "completed";
  return { outcome, tally, failed, held };
}
