import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderNotation, renderNotationLines } from "./notation";

function html(node: React.ReactNode): string {
  return render(<div>{node}</div>).container.innerHTML;
}

describe("プレビューに描く行内記法", () => {
  it("ルビは、読書画面と同じ ruby として描く", () => {
    expect(html(renderNotation("[[rb:紫陽花>あじさい]]が咲く"))).toContain("<ruby>紫陽花<rt>あじさい</rt></ruby>");
  });

  it("外部リンクは宛先を保ったまま、押せない形で描く", () => {
    const output = html(renderNotation("続きは[[jumpuri:こちら>https://example.com/next]]から"));
    expect(output).toContain("こちら");
    expect(output).toContain('title="https://example.com/next"');
    // プレビューの一押しで画面が変わってしまわないよう、リンクにはしない。
    expect(output).not.toContain("<a ");
  });

  it("ページ内移動は、行き先の番号を見せる", () => {
    expect(html(renderNotation("[jump:3]"))).toContain("3ページへ");
  });

  it("記法の無い行はそのまま", () => {
    expect(renderNotation("ただの本文")).toBe("ただの本文");
  });

  it("行の終わりは改行として描く", () => {
    expect(html(renderNotationLines("一行目\n二行目"))).toContain("<br>");
  });

  it("閉じていない記法は、書いたとおりの文字として残す", () => {
    expect(html(renderNotation("[[rb:漢字"))).toContain("[[rb:漢字");
  });
});
