import { defaultAdvancedValues, getAdvancedSettings, PREVIEW_ADVANCED_SPECS, type AdvancedSettings } from "@/services/advancedSettingsApi";

/**
 * 画面の側で使う詳細設定の値（読む速さ・処理の記録の件数）。
 *
 * 使う所はどれも描くときに一度読むだけなので、取り直しを待つ仕組みは持たせない。
 * 起動したときに一度読み、設定の画面で保存したら差し替える。読めなければ既定のまま。
 */
let applied: Record<string, number> = defaultAdvancedValues(PREVIEW_ADVANCED_SPECS);

export function applyAdvancedSettings(settings: AdvancedSettings) {
  applied = { ...defaultAdvancedValues(settings.specs), ...settings.values };
}

export async function loadAdvancedSettings(): Promise<void> {
  try {
    applyAdvancedSettings(await getAdvancedSettings());
  } catch {
    // 既定の値で動く。
  }
}

function value(key: string): number {
  return applied[key] ?? defaultAdvancedValues(PREVIEW_ADVANCED_SPECS)[key];
}

/** 黙読の速さの目安（字/分）。 */
export const readingCharsPerMinute = () => value("readingCharsPerMinute");
/** 処理の記録を残す件数の上限。起動したとき、設定を読む前に使う。 */
export const OPERATION_JOBS_CEILING = PREVIEW_ADVANCED_SPECS.find((spec) => spec.key === "operationJobsKept")?.max ?? 500;
/** 処理の記録（保存・EPUB など）を残す件数。 */
export const operationJobsKept = () => value("operationJobsKept");
