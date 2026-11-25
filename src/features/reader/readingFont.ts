import { useLocalStorage } from "@mantine/hooks";
import { useEffect } from "react";
import { READER_FONTS, READER_TYPE_DEFAULTS, readerTypeFont, type ReaderFont } from "@shared/readerType";

/** 読む画面の表示設定を覚えておく場所。 */
export const READER_SETTINGS_KEY = "piep.reader-settings";

/** 本文を出す場所が使う CSS の変数。読む画面で選んだ書体が入る。 */
export const READING_FONT_VARIABLE = "--piep-reading-font";

/** 覚えていた表示設定から、本文の書体の指定を出す。 */
export function readingFontFamily(stored: unknown): string {
  const font = stored && typeof stored === "object" ? (stored as { font?: unknown }).font : undefined;
  return READER_FONTS[readerTypeFont(font, Object.keys(READER_FONTS) as ReaderFont[], READER_TYPE_DEFAULTS.font)].family;
}

/**
 * 読む画面で選んだ書体を、本文を出すほかの場所（作品の画面の本文・以前の版・改稿の差分・
 * 編集の下見）にも使う。以前はそこだけ明朝に決め打ちで、読む画面の既定のゴシックと
 * 場所によって本文の書体が違っていた。
 */
export function useReadingFontVariable(): void {
  const [stored] = useLocalStorage<unknown>({ key: READER_SETTINGS_KEY, defaultValue: {} });
  const family = readingFontFamily(stored);
  useEffect(() => {
    document.documentElement.style.setProperty(READING_FONT_VARIABLE, family);
  }, [family]);
}
