import { describe, expect, it } from "vitest";

const { isSavableCandidateStatus, isSettledCandidateStatus, parseWorkId } = await import(
  "@/features/updates/UpdatesPage"
);

describe("update candidate selection", () => {
  it("only allows new and failed candidates to be selected for saving", () => {
    expect(isSavableCandidateStatus("candidate")).toBe(true);
    expect(isSavableCandidateStatus("failed")).toBe(true);
    for (const status of ["queued", "running", "saved", "skipped", "done"]) {
      expect(isSavableCandidateStatus(status)).toBe(false);
    }
  });

  /**
   * 済んだものは畳む。保存し終えた44件が翌日も同じ顔で並んでいると、どれが
   * 残りなのか読めない。処理の途中（待機中・保存中）は、まだ済んでいない。
   */
  it("counts only finished candidates as settled", () => {
    for (const status of ["saved", "skipped", "done"]) {
      expect(isSettledCandidateStatus(status)).toBe(true);
    }
    for (const status of ["candidate", "failed", "queued", "running"]) {
      expect(isSettledCandidateStatus(status)).toBe(false);
    }
  });
});

describe("work-scoped update URLs", () => {
  it("only accepts shelf work IDs", () => {
    expect(parseWorkId("0190A1B2-C3D4-7E5F-8A9B-0C1D2E3F4A5B")).toBe("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b");
    expect(parseWorkId("42")).toBeNull();
    expect(parseWorkId("abc")).toBeNull();
    expect(parseWorkId(null)).toBeNull();
  });
});

