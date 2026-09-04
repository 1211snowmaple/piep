/**
 * 本文内検索の、画面側の受け持ち。
 *
 * どのページに何件あるかは端末側（Rust）が答える。**いま開いているページの
 * どこにあるか**はここで数える。同じ畳み方を両側で使うので、一覧の件数と
 * 本文の印はずれない。
 *
 * 以前はページ番号を返すだけで、押してもページの先頭へ動くだけだった。
 * `[newpage]` の無い作品ではページが 1 つしかないので、**押しても何も
 * 起こらなかった**。
 */

/**
 * 1 文字を 1 文字へ畳む正規化。Rust 側の `fold_char` と同じ規則。
 *
 * 長さを変えないので、畳んだ側で見つけた位置がそのまま元の本文の位置になる。
 * 半角カナの濁点のように長さが変わるものは畳まない ―― 位置がずれるほうが、
 * 少し見つからないことより困る。
 */
export function foldChar(ch: string): string {
  if (ch === "　") return " ";
  const compat = ch.normalize("NFKC");
  const single = [...compat].length === 1 ? compat : ch;
  const lowered = single.toLowerCase();
  const one = [...lowered].length === 1 ? lowered : single;
  const code = one.codePointAt(0);
  if (code === undefined) return one;
  // カタカナ ァ(U+30A1) 〜 ヶ(U+30F6) はひらがなへ寄せる。
  if (code >= 0x30a1 && code <= 0x30f6) return String.fromCodePoint(code - 0x60);
  return one;
}

export function foldForSearch(value: string): string {
  let folded = "";
  for (const ch of value) folded += foldChar(ch);
  return folded;
}

/** 畳んだ文字と、それぞれが元の文字列の何文字目(UTF-16)から来たか。 */
interface FoldedText {
  chars: string[];
  /** `offsets[i]` は `chars[i]` の元の位置。末尾に文字列長を 1 つ足してある。 */
  offsets: number[];
}

function fold(text: string): FoldedText {
  const chars: string[] = [];
  const offsets: number[] = [];
  let index = 0;
  for (const ch of text) {
    chars.push(foldChar(ch));
    offsets.push(index);
    index += ch.length;
  }
  offsets.push(text.length);
  return { chars, offsets };
}

export interface MatchRange {
  start: number;
  end: number;
}

/** 一致した範囲を、元の文字列の位置(UTF-16)で返す。 */
export function findMatchRanges(text: string, term: string): MatchRange[] {
  const needle = [...foldForSearch(term)];
  if (!needle.length) return [];
  const { chars, offsets } = fold(text);
  if (chars.length < needle.length) return [];
  const ranges: MatchRange[] = [];
  for (let start = 0; start <= chars.length - needle.length; start += 1) {
    let matched = true;
    for (let offset = 0; offset < needle.length; offset += 1) {
      if (chars[start + offset] !== needle[offset]) {
        matched = false;
        break;
      }
    }
    if (!matched) continue;
    ranges.push({ start: offsets[start], end: offsets[start + needle.length] });
    // 重ならない一致だけを数える。「ああ」を「あああ」から2件見つけると、
    // 印が入れ子になって数と見た目が合わなくなる。
    start += needle.length - 1;
  }
  return ranges;
}

export const HIT_ATTRIBUTE = "data-reader-hit";

/** 本文に一致箇所の印を入れる。戻り値の `count` はそのページの件数。 */
export function highlightMatches(html: string, term: string): { html: string; count: number } {
  if (!term.trim() || !html || typeof DOMParser === "undefined") return { html, count: 0 };
  const document = new DOMParser().parseFromString(html, "text/html");
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const targets: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node as Text;
    if (text.data.trim()) targets.push(text);
  }

  let count = 0;
  for (const text of targets) {
    const ranges = findMatchRanges(text.data, term);
    if (!ranges.length) continue;
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    for (const range of ranges) {
      fragment.append(text.data.slice(cursor, range.start));
      const mark = document.createElement("mark");
      mark.className = "reader-hit";
      mark.setAttribute(HIT_ATTRIBUTE, String(count));
      mark.textContent = text.data.slice(range.start, range.end);
      fragment.append(mark);
      cursor = range.end;
      count += 1;
    }
    fragment.append(text.data.slice(cursor));
    text.replaceWith(fragment);
  }
  return { html: count ? document.body.innerHTML : html, count };
}
