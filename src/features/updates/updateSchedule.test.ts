import { describe, expect, it } from "vitest";
import { describeSchedule, isRunDue, normalizeSchedule, updateScheduleDefaults } from "./updateSchedule";

const HOUR = 3_600_000;

describe("update schedule", () => {
  it("keeps stored settings inside a range a person would choose", () => {
    expect(normalizeSchedule({ intervalHours: 0.2 })).toMatchObject({ intervalHours: 1 });
    expect(normalizeSchedule({ intervalHours: 100_000 })).toMatchObject({ intervalHours: 336 });
    expect(normalizeSchedule({ intervalHours: -5 })).toMatchObject({ intervalHours: 0 });
    expect(normalizeSchedule({ intervalHours: "6" })).toMatchObject({ intervalHours: 6 });
    expect(normalizeSchedule(undefined)).toEqual(updateScheduleDefaults);
    expect(normalizeSchedule({ mode: "nonsense" })).toMatchObject({ mode: "check_only" });
  });

  it("does nothing when the schedule is off", () => {
    const off = normalizeSchedule({});
    expect(isRunDue(off, null, Date.now(), true)).toBe(false);
    expect(isRunDue(off, null, Date.now(), false)).toBe(false);
  });

  // 閉じている間に何回分の間隔が過ぎていても、起動時の実行は1回きり。
  it("runs once at startup instead of replaying every interval that passed", () => {
    const settings = normalizeSchedule({ onStartup: true, intervalHours: 6 });
    const now = Date.now();
    expect(isRunDue(settings, now - 30 * HOUR, now, true)).toBe(true);
    // 走ったばかりなら、次の起動では走らせない。
    expect(isRunDue(settings, now - HOUR, now, true)).toBe(false);
  });

  it("opens the app without a check when only the interval is set", () => {
    const settings = normalizeSchedule({ intervalHours: 6 });
    const now = Date.now();
    expect(isRunDue(settings, null, now, true)).toBe(false);
    expect(isRunDue(settings, null, now, false)).toBe(true);
    expect(isRunDue(settings, now - 2 * HOUR, now, false)).toBe(false);
    expect(isRunDue(settings, now - 7 * HOUR, now, false)).toBe(true);
  });

  it("runs on every open when startup is on and no interval is set", () => {
    const settings = normalizeSchedule({ onStartup: true });
    const now = Date.now();
    expect(isRunDue(settings, now - 60_000, now, true)).toBe(true);
    expect(isRunDue(settings, now - 60_000, now, false)).toBe(false);
  });

  it("describes itself the way the settings screen reads it", () => {
    expect(describeSchedule(normalizeSchedule({}))).toBe("自動では実行しません");
    expect(describeSchedule(normalizeSchedule({ onStartup: true, intervalHours: 12, mode: "auto_save" })))
      .toBe("起動時 と 12時間ごとに確認して保存");
  });
});
