import { describe, expect, it } from "vitest";
import type { WatchGroup } from "@/types/library";
import { watchGroupSelection } from "./watchIdentity";

describe("watch identity", () => {
  const group: WatchGroup = { id: "019ad000-0000-7000-8000-000000000001", name: "毎日", sortOrder: 0, createdAt: "", updatedAt: "", targetIds: [] };
  it("keeps UUIDs through select controls and never widens a missing choice", () => {
    expect(watchGroupSelection(String(group.id), [group])).toBe(group.id);
    expect(watchGroupSelection("missing-group", [])).toBe("missing-group");
    expect(watchGroupSelection("all", [group])).toBeNull();
  });
});
