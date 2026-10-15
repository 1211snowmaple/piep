import { describe, expect, it } from "vitest";
import { heldReason, summarizeUpdateJobItems, updateJobSubject } from "./updateJobOutcome";
import type { UpdateJobItemState, UpdateJobSummary } from "@/services/updateJobApi";

const item = (itemType: string, status: string, error: string | null = null): UpdateJobItemState => ({
  source: "fanbox", sourceId: "1", title: "作品", itemType, status, error,
});

describe("update job outcome", () => {
  it("separates missing source posts from permission holds and completed checks", () => {
    const items = [
      ...Array.from({ length: 7 }, () => item("work", "held", "公開元で投稿が見つからないため確認を保留しました")),
      ...Array.from({ length: 12 }, () => item("work", "skipped")),
      item("target", "done"),
      item("candidate", "held", "[閲覧制限] 月額1000円以上"),
    ];
    const summary = summarizeUpdateJobItems(items);
    expect(summary.unchanged).toBe(12);
    expect(summary.checkedTargets).toBe(1);
    expect(summary.held).toHaveLength(8);
    expect(summary.missing).toHaveLength(7);
    expect(summary.access).toHaveLength(1);
    expect(summary.other).toHaveLength(0);
    expect(heldReason(item("work", "held", "候補から外しています"))).toBe("other");
  });

  it("names a one-off author check in history", () => {
    const job = { scope: "author", mode: "check_only", subjectLabel: "氷砂糖", subjectSource: "fanbox" } as UpdateJobSummary;
    expect(updateJobSubject(job)).toBe("氷砂糖 · FANBOX · 作者 · 確認のみ");
    expect(updateJobSubject({ ...job, subjectLabel: null })).toBe("作者 · 確認のみ");
  });
});
