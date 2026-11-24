import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * 詳細設定。`src-tauri/src/commands/settings.rs` と対になる。
 *
 * 項目・既定・範囲は Rust の `advanced_settings.rs` が一つだけ持ち、画面はそれを受け取って
 * 入力欄を作る。範囲の外は Rust が断る。
 */

export type AdvancedSettingUnit = "milliseconds" | "seconds" | "minutes" | "days" | "count" | "charsPerMinute";
export type AdvancedSettingGroup = "source" | "history" | "server" | "assist" | "reading";

export interface AdvancedSettingSpec {
  key: string;
  label: string;
  group: AdvancedSettingGroup;
  unit: AdvancedSettingUnit;
  default: number;
  min: number;
  max: number;
}

export interface AdvancedSettings {
  specs: AdvancedSettingSpec[];
  values: Record<string, number>;
}

/**
 * ブラウザで開いたプレビュー用の写し。Rust には届かないので、ここに同じ表を持つ。
 * `advancedSettingsApi.test.ts` が Rust の表と突き合わせる。
 */
export const PREVIEW_ADVANCED_SPECS: AdvancedSettingSpec[] = [
  { key: "requestGapMs", label: "取得元へ問い合わせる間隔", group: "source", unit: "milliseconds", default: 800, min: 400, max: 60_000 },
  { key: "updateCheckDelayMs", label: "更新の確認で、作品や作者の間にあける時間", group: "source", unit: "milliseconds", default: 2_000, min: 1_000, max: 60_000 },
  { key: "newWorksDelayMs", label: "新作だけの確認で、作者の間にあける時間", group: "source", unit: "milliseconds", default: 1_500, min: 750, max: 60_000 },
  { key: "rateLimitRetries", label: "取得制限に当たった作品をやり直す回数", group: "source", unit: "count", default: 3, min: 0, max: 5 },
  { key: "consecutiveErrorLimit", label: "失敗が続いたら更新の確認を止める回数", group: "source", unit: "count", default: 3, min: 1, max: 10 },
  { key: "deepCheckIntervalDays", label: "本文まで突き合わせる間隔", group: "source", unit: "days", default: 14, min: 7, max: 90 },
  { key: "sourceTimeoutSecs", label: "取得元の応答を待つ時間", group: "source", unit: "seconds", default: 30, min: 15, max: 120 },
  { key: "assetDownloadConcurrency", label: "画像を同時に取る数", group: "source", unit: "count", default: 2, min: 1, max: 2 },
  { key: "keepUpdateJobs", label: "更新の確認の履歴を残す件数", group: "history", unit: "count", default: 50, min: 10, max: 500 },
  { key: "keepUpdateJobDays", label: "更新の確認の履歴を残す日数", group: "history", unit: "days", default: 30, min: 7, max: 365 },
  { key: "operationJobsKept", label: "処理の記録を残す件数", group: "history", unit: "count", default: 100, min: 20, max: 500 },
  { key: "autoSendQuietMinutes", label: "棚の変更が落ち着いてから送るまで", group: "server", unit: "minutes", default: 5, min: 1, max: 60 },
  { key: "autoSendRetryMinutes", label: "送れなかったときにやり直すまで", group: "server", unit: "minutes", default: 15, min: 5, max: 120 },
  { key: "assistTimeoutSecs", label: "AI の応答を待つ時間", group: "assist", unit: "seconds", default: 180, min: 60, max: 1_800 },
  { key: "readingCharsPerMinute", label: "読む速さ", group: "reading", unit: "charsPerMinute", default: 550, min: 200, max: 2_000 },
];

export function defaultAdvancedValues(specs: AdvancedSettingSpec[]): Record<string, number> {
  return Object.fromEntries(specs.map((spec) => [spec.key, spec.default]));
}

let previewValues = defaultAdvancedValues(PREVIEW_ADVANCED_SPECS);

export async function getAdvancedSettings(): Promise<AdvancedSettings> {
  if (!isTauriRuntime()) return { specs: PREVIEW_ADVANCED_SPECS, values: { ...previewValues } };
  return invoke<AdvancedSettings>("settings_advanced_get");
}

/** 丸ごと置き換える。範囲の外があれば何も変えずに断られる。 */
export async function saveAdvancedSettings(values: Record<string, number>): Promise<AdvancedSettings> {
  if (!isTauriRuntime()) {
    // プレビューには保存先が無い。この画面を開いているあいだだけ覚える。
    previewValues = { ...defaultAdvancedValues(PREVIEW_ADVANCED_SPECS), ...values };
    return { specs: PREVIEW_ADVANCED_SPECS, values: { ...previewValues } };
  }
  return invoke<AdvancedSettings>("settings_advanced_save", { values });
}
