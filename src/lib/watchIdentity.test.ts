import { describe, expect, it } from "vitest";
import type { WatchGroup } from "@/types/library";
import { watchGroupMatches, watchGroupSelection } from "./watchIdentity";

describe("watch identity after switching the shelf", () => {
  const group: WatchGroup = { id: "019ad000-0000-7000-8000-000000000001", legacyId: 7, name: "毎日", sortOrder: 0, createdAt: "", updatedAt: "", targetIds: [] };
  it("resolves an old device setting only to its recorded group", () => {
    expect(watchGroupMatches(group, 7)).toBe(true);
    expect(watchGroupMatches(group, 8)).toBe(false);
    expect(watchGroupMatches(group, group.id)).toBe(true);
    expect(watchGroupMatches(group, null)).toBe(false);
  });
  it("keeps UUIDs through select controls and never widens a missing choice", () => {
    expect(watchGroupSelection(String(group.id), [group])).toBe(group.id);
    expect(watchGroupSelection("9", [{ ...group, id: 9 }])).toBe(9);
    expect(watchGroupSelection("missing-group", [])).toBe("missing-group");
    expect(watchGroupSelection("all", [group])).toBeNull();
  });
});
