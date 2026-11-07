import { describe, expect, it } from "vitest";
import type { AdminStatus, Connection } from "./api";
import { adminPlace, bytes, connectionWarnings, draftConnection, findIssues, isNewer, pageFromName, relativeTime, sameConnection, stamp, windowLabel } from "./format";

const status = (overrides: Partial<AdminStatus> = {}): AdminStatus => ({
  version: "0.36.0",
  apiVersion: 2,
  works: 10,
  devices: [],
  sync: null,
  publicUrl: null,
  update: null,
  serverName: "Maple",
  startedAt: "2026-11-05T00:00:00Z",
  ...overrides,
});

describe("format", () => {
  it("says when something happened", () => {
    const now = new Date(2026, 10, 5, 21, 0).getTime();
    expect(relativeTime(null, now)).toBe("なし");
    expect(relativeTime(now - 20_000, now)).toBe("たった今");
    expect(relativeTime(now - 5 * 60_000, now)).toBe("5 分前");
    expect(relativeTime(new Date(2026, 10, 5, 3, 12).getTime(), now)).toBe("今日 3:12");
    expect(relativeTime(new Date(2026, 10, 4, 23, 5).getTime(), now)).toBe("昨日 23:05");
    expect(relativeTime(new Date(2026, 8, 1, 3, 0).getTime(), now)).toBe("9月1日 3:00");
    expect(relativeTime(new Date(2025, 8, 1, 3, 0).getTime(), now)).toBe("2025年9月1日 3:00");
  });

  it("compares versions by number", () => {
    expect(isNewer("0.36.0", "0.35.9")).toBe(true);
    expect(isNewer("0.100.0", "0.99.0")).toBe(true);
    expect(isNewer("0.36.0", "0.36.0")).toBe(false);
    expect(isNewer("0.35.0", "0.36.0")).toBe(false);
    expect(isNewer(null, "0.36.0")).toBe(false);
  });

  it("writes sizes and windows for people", () => {
    expect(bytes(512)).toBe("512 B");
    expect(bytes(1536)).toBe("1.5 KB");
    expect(bytes(null)).toBe("不明");
    expect(windowLabel("03:00-06:00")).toBe("3:00〜6:00");
    expect(windowLabel("22:30-05:00")).toBe("22:30〜5:00");
  });

  it("lists problems with the heaviest first", () => {
    expect(findIssues(status(), undefined)).toEqual([]);
    const issues = findIssues(
      status({
        sync: { generation: 1, committedAt: null, sessionOpen: false, indexPending: 0, indexError: "x" },
        update: { auto: true, window: "03:00-06:00", phase: { state: "idle" }, latest: null, lastCheckedAt: null, lastError: "失敗", lastAppliedAt: null, skipped: ["0.36.1"], previous: null },
      }),
      { contentBytes: 0, databaseBytes: 0, indexBytes: 0, trashBytes: 0, freeBytes: 1024 },
    );
    expect(issues.map((issue) => issue.page)).toEqual(["library", "updates", "updates", "library"]);
    expect(issues[2]?.message).toBe("0.36.1 は起動に失敗したため飛ばしています");
  });

  it("keeps old page names working", () => {
    expect(pageFromName("security")).toBe("settings");
    expect(pageFromName("account")).toBe("account");
    expect(pageFromName("nothing")).toBeNull();
  });

  it("lines up record times to the second", () => {
    const now = new Date(2026, 10, 7, 12, 0);
    expect(stamp(new Date(2026, 10, 7, 9, 5, 3), now)).toBe("11/07 09:05:03");
    expect(stamp(new Date(2025, 0, 2, 23, 59, 59), now)).toBe("2025/01/02 23:59:59");
  });
});

const connection = (overrides: Partial<Connection> = {}): Connection => ({
  listen: "127.0.0.1:38777",
  adminListen: null,
  publicUrl: null,
  localAdmin: null,
  tls: "none",
  ...overrides,
});

describe("connection", () => {
  it("says where the admin page is", () => {
    expect(adminPlace(connection())).toEqual({ url: "http://127.0.0.1:38777/admin/", thisMachineOnly: true });
    expect(adminPlace(connection({ adminListen: "0.0.0.0:38778", tls: "self-signed" }))).toEqual({ url: "https://サーバーの PC のアドレス:38778/admin/", thisMachineOnly: false });
    expect(adminPlace(connection({ publicUrl: "https://example.com/", localAdmin: "127.0.0.1:38778" }))).toEqual({ url: "http://127.0.0.1:38778/admin/", thisMachineOnly: true });
    expect(adminPlace(connection({ publicUrl: "https://example.com/" }))).toBeNull();
  });

  it("warns only when the login page is reachable from outside", () => {
    expect(connectionWarnings(connection())).toEqual([]);
    expect(connectionWarnings(connection({ listen: "0.0.0.0:38777", tls: "self-signed" }))).toHaveLength(1);
    // 公開の URL があれば、管理画面はこの機械の中にしか出ない。
    expect(connectionWarnings(connection({ listen: "0.0.0.0:38777", tls: "self-signed", publicUrl: "https://example.com/", localAdmin: "127.0.0.1:38778" }))).toEqual([]);
    expect(connectionWarnings(connection({ adminListen: "192.168.1.2:38778", tls: "self-signed" }))[0]).toContain("192.168.1.2:38778");
  });

  it("drafts from the saved settings and compares them", () => {
    const saved = connection({ adminListen: "127.0.0.1:40000", publicUrl: "https://example.com/" });
    expect(draftConnection(saved)).toMatchObject({ split: true, adminListen: "127.0.0.1:40000", publicUrl: "https://example.com/", cert: "", key: "" });
    expect(draftConnection(connection())).toMatchObject({ split: false, adminListen: "127.0.0.1:38778", publicUrl: "" });
    expect(sameConnection(saved, { ...saved, localAdmin: "127.0.0.1:38778" })).toBe(true);
    expect(sameConnection(saved, { ...saved, tls: "self-signed" })).toBe(false);
  });
});
