import { describe, expect, it } from "vitest";
import {
  describeCheckFailure,
  downloadPercent,
  launchCheckEnabled,
  noReleasePublishedYet,
  storeLaunchCheck,
  summarizeNotes,
} from "@/features/settings/appUpdate";

describe("app update preferences", () => {
  // 覚えていない・壊れている値で更新の存在を隠さない。
  it("checks on launch until it is explicitly turned off", () => {
    expect(launchCheckEnabled(null)).toBe(true);
    expect(launchCheckEnabled("1")).toBe(true);
    expect(launchCheckEnabled("なにか")).toBe(true);
    expect(launchCheckEnabled("0")).toBe(false);
    expect(launchCheckEnabled(storeLaunchCheck(false))).toBe(false);
    expect(launchCheckEnabled(storeLaunchCheck(true))).toBe(true);
  });
});

describe("app update progress", () => {
  // 長さを告げない配信もある。そのときに 0% と嘘をつかない。
  it("reports a percentage only when the total is known", () => {
    expect(downloadPercent(0, null)).toBeNull();
    expect(downloadPercent(500, 0)).toBeNull();
    expect(downloadPercent(512, 1024)).toBe(50);
    // 実際より多く届くことがあっても、100を超えて見せない。
    expect(downloadPercent(2048, 1024)).toBe(100);
  });
});

describe("app update notes", () => {
  it("keeps the notes short and drops empty ones", () => {
    expect(summarizeNotes(null)).toBeNull();
    expect(summarizeNotes("   ")).toBeNull();
    expect(summarizeNotes("  変更点  ")).toBe("変更点");
    expect(summarizeNotes("あ".repeat(20), 10)).toBe(`${"あ".repeat(10)}…`);
  });
});

describe("app update failures", () => {
  // 署名鍵を入れ忘れているだけのときに、原因の分からない失敗に見せない。
  it("names the missing signing key when that is what went wrong", () => {
    expect(describeCheckFailure("failed to parse pubkey")).toContain("署名鍵");
    expect(describeCheckFailure("Updater is disabled")).toContain("署名鍵");
    expect(describeCheckFailure("error sending request")).not.toContain("署名鍵");
    expect(describeCheckFailure("error sending request")).toContain("error sending request");
  });

  // 下書きのままのリリースは、壊れているのではなくまだ配っていないだけ。
  it("reads a missing manifest as nothing published yet", () => {
    expect(noReleasePublishedYet("Network Error: Status Code: 404")).toBe(true);
    expect(noReleasePublishedYet("Could not fetch a valid release JSON")).toBe(true);
    expect(noReleasePublishedYet("error sending request")).toBe(false);
    expect(describeCheckFailure("Network Error: Status Code: 404")).toContain("まだありません");
    expect(describeCheckFailure("Network Error: Status Code: 404")).not.toContain("失敗");
  });
});
