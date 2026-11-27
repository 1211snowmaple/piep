// @vitest-environment-options {"url":"https://www.fanbox.cc/creators/following"}
import type {} from "vitest/jsdom";
import { beforeEach, describe, expect, it } from "vitest";
import script from "../../../src-tauri/src/auth/page_users.js?raw";

interface PageUsers {
  url: string;
  rows: { href: string; name: string }[];
  truncated: boolean;
}

const read = new Function("document", "expectedUrl", "source = 'fanbox'", `return (${script})(expectedUrl, source);`) as
  (doc: Document, expectedUrl: string, source?: "pixiv" | "fanbox") => PageUsers;
const followingUrl = "https://www.fanbox.cc/creators/following";
const supportingUrl = "https://www.fanbox.cc/creators/supporting";
const list = (cards: string) => `<main><section><h2>フォロー中</h2><ul>${cards}</ul></section></main>`;
const pixivUrl = "https://www.pixiv.net/users/7/following";
const userCard = (href: string, name: string) => `<li><h3><a href="${href}">${name}</a></h3></li>`;

beforeEach(() => {
  jsdom.reconfigure({ url: followingUrl });
  document.head.querySelector("base")?.remove();
});

describe("表示中のフォロー一覧の読み取り", () => {
  beforeEach(() => jsdom.reconfigure({ url: pixivUrl }));

  it("特定できた一覧のカード内だけを読み、相対リンクは文書の URL で解決する", () => {
    document.head.insertAdjacentHTML("beforeend", '<base href="https://example.invalid/">');
    document.body.innerHTML = `<aside><a href="https://www.pixiv.net/users/91">案内の作者</a></aside>
      ${list(userCard("/users/8", "一覧の作者") + '<li><p><a href="https://www.pixiv.net/users/92">紹介文の作者</a></p></li>')}
      <main><section><h2>おすすめ</h2><ul>${userCard("https://www.pixiv.net/users/93", "おすすめの作者")}</ul></section></main>`;
    expect(read(document, pixivUrl, "pixiv").rows).toEqual([{ href: "https://www.pixiv.net/users/8", name: "一覧の作者" }]);
  });

  it("対象一覧を特定できなければ候補を返さない", () => {
    document.body.innerHTML = `<main><section><h2>おすすめ</h2><ul>${userCard("/users/8", "おすすめの作者")}</ul></section></main>`;
    expect(read(document, pixivUrl, "pixiv")).toEqual({ url: pixivUrl, rows: [], truncated: false });
  });

  it("作者 ID の重複、持ち主、作者トップ以外、名前のないリンクで枠を消費しない", () => {
    const otherLinks = Array.from({ length: 2000 }, (_, i) => [
      userCard(`/en/users/8/?rest=${i}`, "同じ作者"),
      userCard(`/users/${10000 + i}/following`, "案内"),
      userCard(`/users/${20000 + i}`, " \n "),
    ].join("")).join("");
    document.body.innerHTML = list(userCard("/users/8", "同じ作者") + userCard("/users/7", "持ち主") + otherLinks + userCard("/users/9", "最後の作者"));
    expect(read(document, pixivUrl, "pixiv")).toEqual({
      url: pixivUrl,
      rows: [
        { href: "https://www.pixiv.net/users/8", name: "同じ作者" },
        { href: "https://www.pixiv.net/users/9", name: "最後の作者" },
      ],
      truncated: false,
    });
  });

  it("作者名を文字の途中で切らず、長い名前が並んでも応答を 2 MiB 以内に収める", () => {
    const name = "𠮷".repeat(1100);
    document.body.innerHTML = list(Array.from({ length: 600 }, (_, i) => userCard(`/users/${1000 + i}`, name)).join(""));
    const page = read(document, pixivUrl, "pixiv");
    expect(new TextEncoder().encode(JSON.stringify(page)).byteLength).toBeLessThanOrEqual(2 * 1024 * 1024);
    expect(page.rows).toHaveLength(600);
    expect(page.rows[0].name).toBe("𠮷".repeat(200));
    expect(page.truncated).toBe(false);
  });
});

describe("表示中のクリエイター一覧の読み取り", () => {
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

  it.each(["hidden", 'aria-hidden="true"', 'style="display: none"', ""])(
    "同じ見出しの別区画より選択タブの表示中の区画を優先する（%s）",
    (visibility) => {
      document.body.innerHTML = `<main>
        <a href="/creators/following" aria-current="page" aria-controls="active">フォロー中</a>
        <section ${visibility}><h2>フォロー中</h2><ul>${userCard("/@other", "別区画の作者")}</ul></section>
        <section id="active"><h2>フォロー中</h2><ul>${userCard("/@active", "表示中の作者")}</ul></section>
      </main>`;
      expect(read(document, followingUrl).rows).toEqual([{ href: "https://www.fanbox.cc/@active", name: "表示中の作者" }]);
    },
  );

  it("末尾のスラッシュが異なるタブも同じ一覧に結び付ける", () => {
    document.body.innerHTML = `<main>
      <a href="/creators/following/" aria-current="page" aria-controls="active">フォロー中</a>
      <div id="active"><ul>${userCard("/@active", "表示中の作者")}</ul></div>
    </main>`;
    expect(read(document, followingUrl).rows).toEqual([{ href: "https://www.fanbox.cc/@active", name: "表示中の作者" }]);
  });

  it("見出しに含まれる名前欄を一つと数え、名前リンクだけでも拾う", () => {
    document.body.innerHTML = list('<li><h3><a href="/@artist"><span itemprop="name">作者名</span></a></h3></li>');
    expect(read(document, followingUrl).rows).toEqual([{ href: "https://www.fanbox.cc/@artist", name: "作者名" }]);
  });

  it("同じ作者への名前とアイコンのリンクは合わせてカード 1 枚分と数える", () => {
    const cards = Array.from({ length: 2000 }, (_, i) => `<li>
      <a href="/@artist-${i}"><img alt=""></a><h3><a href="https://artist-${i}.fanbox.cc/">作者 ${i}</a></h3>
    </li>`).join("");
    document.body.innerHTML = list(cards);
    const exact = read(document, followingUrl);
    expect(exact.rows).toHaveLength(2000);
    expect(exact.truncated).toBe(false);
    document.querySelector("ul")?.insertAdjacentHTML("beforeend", userCard("/@over-limit", "上限を超える作者"));
    const over = read(document, followingUrl);
    expect(over.rows).toEqual(exact.rows);
    expect(over.truncated).toBe(true);
  });

  it("カードの途中でリンク走査の上限に達したら、そのカードを候補に入れない", () => {
    const first = userCard("/@first", "先に読んだ作者");
    const large = `<li>${'<a href="/creators/find">案内</a>'.repeat(9999)}<h3><a href="/@last">最後の作者</a></h3></li>`;
    document.body.innerHTML = list(first + large);
    expect(read(document, followingUrl)).toMatchObject({
      rows: [{ href: "https://www.fanbox.cc/@first", name: "先に読んだ作者" }],
      truncated: true,
    });
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
