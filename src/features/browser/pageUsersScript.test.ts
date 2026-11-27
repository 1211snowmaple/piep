// @vitest-environment-options {"url":"https://www.fanbox.cc/creators/following"}
import { beforeEach, describe, expect, it } from "vitest";
import script from "../../../src-tauri/src/auth/page_users.js?raw";

const read = new Function("document", "expectedUrl", `return (${script})(expectedUrl, "fanbox");`);
const followingUrl = "https://www.fanbox.cc/creators/following";
const supportingUrl = "https://www.fanbox.cc/creators/supporting";
const list = (cards: string) => `<main><section><h2>フォロー中</h2><ul>${cards}</ul></section></main>`;

describe("表示中のクリエイター一覧の読み取り", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", followingUrl);
  });

  it("一覧のカードの名前だけを採用し、アイコンだけのカードも残す", () => {
    document.body.innerHTML = `<main>
      <section><h2>フォロー中</h2><ul>
        <li><p><a href="/@unrelated">紹介文の中のリンク</a></p></li>
        <li><a href="/@Icon"><img alt="" src="icon.png"></a><p><a href="/@icon">紹介文</a></p></li>
        <li><a href="/@Artist"><img alt="" src="artist.png"></a></li>
        <li><a href="https://artist.fanbox.cc/en/"><h3>作者名</h3><p>名前ではない紹介文</p></a></li>
      </ul></section>
      <section><h2>おすすめ</h2><ul><li><h3><a href="/@suggested">おすすめの作者</a></h3></li></ul></section>
    </main>`;
    expect(read(document, followingUrl)).toEqual({
      url: followingUrl,
      rows: [
        { href: "https://www.fanbox.cc/@icon", name: "" },
        { href: "https://www.fanbox.cc/@artist", name: "作者名" },
      ],
      truncated: false,
    });
  });

  it("支援中のタブの区画だけを読み、対象一覧が無ければ候補を返さない", () => {
    window.history.replaceState(null, "", supportingUrl);
    document.body.innerHTML = `<main>
      <a href="/creators/supporting" role="tab" aria-selected="true" aria-controls="active">支援中</a>
      <div id="active"><div role="list"><div role="listitem"><h3><a href="/@supported">支援中の作者</a></h3></div></div></div>
      <section><h2>フォロー中</h2><ul><li><h3><a href="/@followed">フォロー中の作者</a></h3></li></ul></section>
    </main>`;
    expect(read(document, supportingUrl).rows).toEqual([{ href: "https://www.fanbox.cc/@supported", name: "支援中の作者" }]);
    document.getElementById("active")?.remove();
    expect(read(document, supportingUrl).rows).toEqual([]);
  });

  it("別の一覧へ移った文書は読み取らない", () => {
    document.body.innerHTML = list('<li><h3><a href="/@artist">作者</a></h3></li>');
    expect(read(document, supportingUrl)).toEqual({ url: followingUrl, rows: [], truncated: false });
  });

  it("候補外のリンクを上限に数えず、候補リンクが上限を超えたときだけ知らせる", () => {
    const invalid = `<li><h3><a href="https://artist.fanbox.cc/xy">別のページ</a></h3></li>`;
    const valid = `<li><h3><a href="https://artist.fanbox.cc/zh-tw/">作者</a></h3></li>`;
    document.body.innerHTML = list(invalid.repeat(2001) + valid.repeat(2000));
    expect(read(document, followingUrl)).toMatchObject({
      rows: [{ href: "https://www.fanbox.cc/@artist", name: "作者" }],
      truncated: false,
    });
    document.querySelector("ul")?.insertAdjacentHTML("beforeend", valid);
    expect(read(document, followingUrl).truncated).toBe(true);
  });
});
