import type { ReactNode } from "react";

/**
 * 本文の行内記法を、読書画面と同じ姿でプレビューに描く。
 *
 * 編集画面が扱うのは記法のままの文字列なので、プレビューには
 * `[[rb:漢字>かんじ]]` や `[jump:3]` がそのまま並んでいた。読書画面では
 * ルビとリンクになるものが、**仕上がりを確かめるための画面にだけ**
 * 素の記法で出ていた。ここが揃っていないと、プレビューは仕上がりを
 * 示していない。
 *
 * 開き方（規則）は Rust の `expand_pixiv_inline_notation` と同じ。あちらは
 * エスケープ済みの HTML を扱うので `&gt;`、こちらは編集中の生の文字列なので
 * `>` を見る。
 */
const NOTATION = /\[\[rb:([^\]>]+?)\s*>\s*([^\]]+?)\]\]|\[\[jumpuri:([^\]>]+?)\s*>\s*(https?:\/\/[^\]\s]+?)\]\]|\[jump:(\d+)\]/g;

export function renderNotation(text: string): ReactNode {
  if (!text) return text;
  NOTATION.lastIndex = 0;
  const nodes: ReactNode[] = [];
  let cursor = 0;
  let key = 0;
  for (let match = NOTATION.exec(text); match; match = NOTATION.exec(text)) {
    if (match.index > cursor) nodes.push(text.slice(cursor, match.index));
    const [, kanji, kana, linkLabel, url, page] = match;
    if (kanji !== undefined && kana !== undefined) {
      nodes.push(<ruby key={key += 1}>{kanji}<rt>{kana}</rt></ruby>);
    } else if (linkLabel !== undefined && url !== undefined) {
      // プレビューでは飛ばない。押せる形にすると、確かめるつもりの一押しで
      // 画面が変わってしまう。宛先は見えるようにしておく。
      nodes.push(<span key={key += 1} className="editor-preview-notation-link" title={url}>{linkLabel}</span>);
    } else if (page !== undefined) {
      nodes.push(<span key={key += 1} className="editor-preview-notation-jump">{page}ページへ</span>);
    }
    cursor = match.index + match[0].length;
  }
  if (!nodes.length) return text;
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}

/** 行ごとに分けて、行の終わりに `<br />` を置く。 */
export function renderNotationLines(text: string | null | undefined): ReactNode {
  const lines = (text ?? "").split("\n");
  return lines.map((line, index) => (
    <span key={index}>{renderNotation(line)}{index < lines.length - 1 && <br />}</span>
  ));
}
