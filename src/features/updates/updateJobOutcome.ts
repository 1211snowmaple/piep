import type { UpdateJobItemState, UpdateJobSummary } from "@/services/updateJobApi";

export type HeldReason = "missing" | "access" | "other";

export function heldReason(item: UpdateJobItemState): HeldReason {
  const reason = item.error ?? "";
  if (reason.includes("公開元で投稿が見つからない")) return "missing";
  if (reason.includes("閲覧制限") || reason.includes("閲覧条件") || reason.includes("必要な支援額")) return "access";
  return "other";
}

export function summarizeUpdateJobItems(items: UpdateJobItemState[]) {
  const held = items.filter((item) => item.status === "held");
  return {
    unchanged: items.filter((item) => item.itemType === "work" && item.status === "skipped").length,
    skippedCandidates: items.filter((item) => item.itemType === "candidate" && item.status === "skipped").length,
    checkedTargets: items.filter((item) => item.itemType === "target" && item.status === "done").length,
    held,
    missing: held.filter((item) => heldReason(item) === "missing"),
    access: held.filter((item) => heldReason(item) === "access"),
    other: held.filter((item) => heldReason(item) === "other"),
  };
}

export function updateJobSubject(job: UpdateJobSummary): string {
  const scope = ({ all: "すべての監視対象", work: "作品", author: "作者", series: "シリーズ", save: "候補" } as Record<string, string>)[job.scope] ?? job.scope;
  const mode = ({ check_only: "確認のみ", auto_save: "確認と自動保存", save: "保存" } as Record<string, string>)[job.mode] ?? job.mode;
  if (!job.subjectLabel) return `${scope} · ${mode}`;
  const provider = job.subjectSource === "fanbox" ? "FANBOX" : job.subjectSource === "pixiv" ? "pixiv" : job.subjectSource;
  return `${job.subjectLabel} · ${provider ? `${provider} · ` : ""}${scope} · ${mode}`;
}
