export type ReaderFont = "serif" | "sans";
export type ReaderTheme = "white" | "paper" | "night";

export interface ReaderType {
  fontSize: number;
  lineHeight: number;
  readerWidth: number;
  fontFamily: ReaderFont;
  readerTheme: ReaderTheme;
}

export const typeLimits = {
  fontSize: { min: 14, max: 36, step: 1 },
  lineHeight: { min: 1.4, max: 2.4, step: 0.02 },
  readerWidth: { min: 520, max: 900, step: 20 },
} as const;

/** タブレットは携帯より離して持つので、何も選んでいないときの本文を少し大きくする。 */
export function defaultFontSize(): number {
  return window.matchMedia("(min-width: 600px)").matches ? 21 : 18;
}

export function defaultReaderType(): ReaderType {
  return { fontSize: defaultFontSize(), lineHeight: 1.72, readerWidth: 680, fontFamily: "sans", readerTheme: "white" };
}

/** pages は転送ページごとに区切り、continuous は全ページを一続きにする。 */
export type ReaderLayout = "pages" | "continuous";
export type TapTurn = "sides" | "updown" | "off";

export interface ReaderBehavior {
  layout: ReaderLayout;
  tap: TapTurn;
  swipe: boolean;
  reverse: boolean;
  volumeKeys: boolean;
  motion: "slide" | "none";
  bars: "auto" | "pinned";
  pageButtons: "always" | "menu";
  autoScrollSpeed: number;
}

export const defaultReaderBehavior: ReaderBehavior = {
  layout: "pages",
  tap: "sides",
  swipe: true,
  reverse: false,
  volumeKeys: false,
  motion: "slide",
  bars: "auto",
  pageButtons: "always",
  autoScrollSpeed: 40,
};

export const autoScrollLimits = { min: 10, max: 160, step: 5 } as const;

const behaviorKey = "piep.android.reader.behavior.v1";

function choice<T extends string>(value: unknown, options: readonly T[], fallback: T): T {
  return options.find((option) => option === value) ?? fallback;
}

export function loadReaderBehavior(): ReaderBehavior {
  let raw: Partial<Record<keyof ReaderBehavior, unknown>> = {};
  try { raw = JSON.parse(window.localStorage.getItem(behaviorKey) ?? "{}") ?? {}; } catch { /* 読めなければ既定で開く */ }
  const speed = Number(raw.autoScrollSpeed);
  return {
    layout: choice(raw.layout, ["pages", "continuous"], defaultReaderBehavior.layout),
    tap: choice(raw.tap, ["sides", "updown", "off"], defaultReaderBehavior.tap),
    swipe: typeof raw.swipe === "boolean" ? raw.swipe : defaultReaderBehavior.swipe,
    reverse: typeof raw.reverse === "boolean" ? raw.reverse : defaultReaderBehavior.reverse,
    volumeKeys: typeof raw.volumeKeys === "boolean" ? raw.volumeKeys : defaultReaderBehavior.volumeKeys,
    motion: choice(raw.motion, ["slide", "none"], defaultReaderBehavior.motion),
    bars: choice(raw.bars, ["auto", "pinned"], defaultReaderBehavior.bars),
    pageButtons: choice(raw.pageButtons, ["always", "menu"], defaultReaderBehavior.pageButtons),
    autoScrollSpeed: Number.isFinite(speed) && speed >= autoScrollLimits.min && speed <= autoScrollLimits.max ? speed : defaultReaderBehavior.autoScrollSpeed,
  };
}

export function saveReaderBehavior(behavior: ReaderBehavior): void {
  try { window.localStorage.setItem(behaviorKey, JSON.stringify(behavior)); } catch { /* storage unavailable */ }
}
