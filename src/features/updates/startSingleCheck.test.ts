import { beforeEach, expect, it, vi } from "vitest";
import { startSingleCheck } from "./startSingleCheck";

const api = vi.hoisted(() => ({ start: vi.fn() }));
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
vi.mock("@/services/updateJobApi", () => ({ startUpdateJobCommand: api.start }));
vi.mock("@/features/updates/updateSchedule", () => ({ loadSchedule: async () => ({ watchSaved: false }) }));
beforeEach(() => api.start.mockReset().mockResolvedValue({}));

it("keeps the explicit work ID in the one-off check", async () => {
  const workId = "0195f1c2-0000-7000-8000-000000000001";
  await startSingleCheck({ kind: "work", workId, label: "題" });
  expect(api.start).toHaveBeenCalledWith({ scope: "work", mode: "check_only", workIds: [workId], adhocTargets: null, watchSaved: false });
});

it("keeps one-off author checks separate from the explicit work range", async () => {
  await startSingleCheck({ kind: "author", source: "pixiv", sourceKey: "77", label: "作者" });
  expect(api.start).toHaveBeenCalledWith({ scope: "author", mode: "check_only", workIds: null, adhocTargets: [{ targetType: "author", source: "pixiv", sourceKey: "77", displayName: "作者" }], watchSaved: false });
});
