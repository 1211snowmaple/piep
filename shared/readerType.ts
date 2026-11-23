/**
 * 読む画面の文字の組み方。デスクトップと携帯で範囲・刻み・既定・書体を同じにする。
 * 上限は大きいほう、刻みは細かいほうに揃えた（読む距離や目の具合に幅広く応えるため）。
 */

export interface ReaderTypeLimit {
  min: number;
  max: number;
  step: number;
}

export const READER_TYPE_LIMITS = {
  fontSize: { min: 14, max: 36, step: 1 },
  lineHeight: { min: 1.4, max: 2.4, step: 0.02 },
  width: { min: 520, max: 900, step: 20 },
} as const satisfies Record<string, ReaderTypeLimit>;

export type ReaderFont = "serif" | "sans" | "ud-serif" | "ud-sans";

/**
 * 書体の選択肢。明朝とゴシックの二つだけでは、長く読むと目が疲れる人や、
 * 細い横線が見えにくい人に応えられなかった。BIZ UD は Windows に最初から
 * 入っている読みやすさ重視の書体で、等幅（UDP でない方）を選ぶのは、
 * 小説の地の文、とくに縦書きで字の並びが揃って見えるため。入っていない
 * 環境では、いつもの明朝・ゴシックに落ちる。
 */
export const READER_FONTS: Record<ReaderFont, { label: string; family: string }> = {
  serif: { label: "明朝", family: '"Noto Serif JP", "Yu Mincho", "Hiragino Mincho ProN", serif' },
  sans: { label: "ゴシック", family: '"Noto Sans JP", "Yu Gothic UI", "Hiragino Kaku Gothic ProN", sans-serif' },
  "ud-serif": { label: "UD明朝", family: '"BIZ UDMincho", "Noto Serif JP", "Yu Mincho", serif' },
  "ud-sans": { label: "UDゴシック", family: '"BIZ UDGothic", "Noto Sans JP", "Yu Gothic UI", sans-serif' },
};

export const READER_TYPE_DEFAULTS = {
  fontSize: 18,
  lineHeight: 1.72,
  width: 680,
  font: "sans" as ReaderFont,
};

/** 覚えていた値を範囲に収め、刻みに合わせる。数でなければ `fallback`。 */
export function readerTypeNumber(value: unknown, limit: ReaderTypeLimit, fallback: number): number {
  const number = typeof value === "string" && value.trim() !== "" ? Number(value) : value;
  if (typeof number !== "number" || !Number.isFinite(number)) return fallback;
  const clamped = Math.min(limit.max, Math.max(limit.min, number));
  const steps = Math.round((clamped - limit.min) / limit.step);
  // 0.02 刻みの足し算で出る端数（1.7200000000000002 など）を残さない。
  return Number((limit.min + steps * limit.step).toFixed(4));
}

/** 書体の名前。`fonts` に無いものは `fallback`。 */
export function readerTypeFont<T extends ReaderFont>(value: unknown, fonts: readonly T[], fallback: T): T {
  return fonts.find((font) => font === value) ?? fallback;
}
