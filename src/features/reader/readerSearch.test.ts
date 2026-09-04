import { describe, expect, it } from "vitest";
import { findMatchRanges, foldForSearch, highlightMatches } from "./readerSearch";

describe("本文内検索の畳み方", () => {
  it("棚の検索と同じ表記ゆれを吸収する", () => {
    expect(foldForSearch("カタカナ")).toBe(foldForSearch("かたかな"));
    expect(foldForSearch("ＡＢＣ")).toBe(foldForSearch("abc"));
    expect(foldForSearch("Piep")).toBe(foldForSearch("ｐｉｅｐ"));
    expect(foldForSearch("１２３")).toBe(foldForSearch("123"));
    expect(foldForSearch("　")).toBe(foldForSearch(" "));
  });

  it("畳んでも文字数が変わらない", () => {
    for (const text of ["カタカナ", "ＡＢＣ", "混ざったＴＥＸＴです", "ヴァイオリン"]) {
      expect([...foldForSearch(text)].length).toBe([...text].length);
    }
  });
});

describe("一致箇所の割り出し", () => {
  it("元の本文での位置を返す", () => {
    expect(findMatchRanges("あかさたなあかさたな", "あか")).toEqual([
      { start: 0, end: 2 },
      { start: 5, end: 7 },
    ]);
  });

  it("カタカナで書かれた語をひらがなで探せる", () => {
    expect(findMatchRanges("彼はカタカナで書いた", "かたかな")).toEqual([{ start: 2, end: 6 }]);
  });

  it("重なる一致は数えない", () => {
    // 入れ子の印になると、一覧の件数と本文の印がずれる。
    expect(findMatchRanges("あああ", "ああ")).toEqual([{ start: 0, end: 2 }]);
  });

  it("空の検索語では何も返さない", () => {
    expect(findMatchRanges("本文", "")).toEqual([]);
    expect(findMatchRanges("本文", "   ")).toEqual([]);
  });
});

describe("本文への印付け", () => {
  it("一致箇所を数えて印を入れる", () => {
    const { html, count } = highlightMatches("<p>雨の日と雨の夜</p>", "雨");
    expect(count).toBe(2);
    expect(html).toContain('data-reader-hit="0"');
    expect(html).toContain('data-reader-hit="1"');
  });

  it("タグの名前には印を入れない", () => {
    const { count } = highlightMatches('<a href="https://example.com/p">本文</a>', "p");
    expect(count).toBe(0);
  });

  it("一致が無ければ本文をそのまま返す", () => {
    const source = "<p>本文</p>";
    const { html, count } = highlightMatches(source, "みつからない");
    expect(count).toBe(0);
    expect(html).toBe(source);
  });

  it("印の中身は元の表記のまま", () => {
    const { html } = highlightMatches("<p>カタカナ</p>", "かたかな");
    expect(html).toContain(">カタカナ<");
  });
});
