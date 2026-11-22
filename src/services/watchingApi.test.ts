import { beforeEach, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke, convertFileSrc: (path: string, protocol = "asset") => `${protocol}://${path}` }));

import { createWatchGroup, deleteUpdateTarget, getAssetUrl, renameWatchGroup, setUpdateTargetEnabled, setWatchGroupMembers } from "./dbApi";

const target = "019ad000-0000-7000-8000-000000000001";
const group = "019ad000-0000-7000-8000-000000000002";
beforeEach(() => invoke.mockReset());

it("sends the target UUID, generation and a request ID with each declared change", async () => {
  invoke.mockResolvedValueOnce({ id: target, watchGeneration: 12 }).mockResolvedValueOnce(undefined);
  await setUpdateTargetEnabled("author", "pixiv", "77", false);
  expect(invoke).toHaveBeenLastCalledWith("db_set_update_target_enabled", expect.objectContaining({ targetId: target, expectedGeneration: 12, requestId: expect.any(String), enabled: false }));
  const first = invoke.mock.calls[1][1].requestId;
  invoke.mockResolvedValueOnce({ id: target, watchGeneration: 13 }).mockResolvedValueOnce(undefined);
  await deleteUpdateTarget("author", "pixiv", "77");
  expect(invoke).toHaveBeenLastCalledWith("db_delete_update_target", expect.objectContaining({ targetId: target, expectedGeneration: 13 }));
  expect(invoke.mock.calls[3][1].requestId).not.toBe(first);
});

it("keeps group and member UUIDs", async () => {
  const groups = [{ id: group, watchGeneration: 3 }];
  invoke.mockResolvedValueOnce(groups).mockResolvedValueOnce(1);
  await setWatchGroupMembers(group, [target], true);
  expect(invoke).toHaveBeenLastCalledWith("db_set_watch_group_members", expect.objectContaining({ groupId: group, targetIds: [target], expectedGeneration: 3 }));
  invoke.mockResolvedValueOnce(groups).mockResolvedValueOnce(undefined);
  await renameWatchGroup(group, "毎週");
  expect(invoke).toHaveBeenLastCalledWith("db_rename_watch_group", expect.objectContaining({ groupId: group, name: "毎週" }));
  invoke.mockResolvedValueOnce(groups).mockResolvedValueOnce(group);
  await expect(createWatchGroup("新しいグループ")).resolves.toBe(group);
});

it("uses the authenticated shelf protocol for saved faces", () => {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  expect(getAssetUrl(`shelf:${"a".repeat(64)}`)).toBe(`piep-library://${"a".repeat(64)}`);
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});
