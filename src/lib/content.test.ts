import { describe, expect, it } from "vitest";
import { prepareDocumentHtml, splitDocumentPages, summaryText } from "./content";

describe("document content helpers", () => {
  it("hydrates local images and removes executable markup", () => {
    const html = prepareDocumentHtml(
      '<p onclick="evil()">本文</p><img data-local-path="C:/book/image.jpg"><script>evil()</script>',
      (path) => path ? `asset://${path}` : null,
    );
    expect(html).toContain('src="asset://C:/book/image.jpg"');
    expect(html).toContain('loading="lazy"');
    expect(html).not.toContain("onclick");
    expect(html).not.toContain("script");
  });

  it("strips markup that could cover the app or survive re-parsing", () => {
    const html = prepareDocumentHtml(
      [
        '<p style="position:fixed;inset:0;z-index:9999">覆い</p>',
        "<svg><style><img src=x onerror=evil()></style></svg>",
        '<img src="javascript:evil()">',
        '<img src="https://i.pximg.net/a.jpg" srcset="javascript:evil()">',
        '<a href="javascript:evil()">わな</a>',
        '<base href="https://evil.example/">',
      ].join(""),
      () => null,
    );
    expect(html).not.toContain("position:fixed");
    expect(html).not.toContain("style=");
    expect(html).not.toContain("<svg");
    expect(html).not.toContain("javascript:");
    expect(html).not.toContain("srcset");
    expect(html).not.toContain("<base");
    // Legitimate remote images are untouched.
    expect(html).toContain("https://i.pximg.net/a.jpg");
  });

  it("turns stored html excerpts into readable compact text", () => {
    expect(summaryText("一行目<br />二行目 <b>強調</b> &amp; 続き")).toBe("一行目\n二行目 強調 & 続き");
  });

  it("uses pixiv newpage markers without inventing viewport pages", () => {
    expect(splitDocumentPages("前半<!-- newpage -->後半", "pixiv")).toEqual(["前半", "後半"]);
    expect(splitDocumentPages(`${"あ".repeat(8000)}<br>${"い".repeat(8000)}<br>`, "pixiv")).toHaveLength(1);
    expect(splitDocumentPages("前半<!-- newpage -->後半", "fanbox")).toHaveLength(1);
  });

  it("turns standalone document URLs into safe cards", () => {
    const html = prepareDocumentHtml('<a href="https://sugar4022.fanbox.cc/posts/1">https://sugar4022.fanbox.cc/posts/1</a>', () => null);
    expect(html).toContain("novel-link-card--fanbox");
    expect(html).toContain("link-card-brand");
    expect(html).not.toContain("🔗");
  });

  it("keeps pixiv's own app links followable instead of stripping them", () => {
    // pixiv writes its captions with these. No browser accepts the scheme, so
    // the address used to be removed and the anchor kept - underlined blue text
    // that did nothing at all when clicked.
    const html = prepareDocumentHtml('<a href="pixiv://novels/20563258">novel/20563258</a>', () => null, { inlineLinks: true });
    expect(html).toContain('href="https://www.pixiv.net/novel/show.php?id=20563258"');
    expect(html).toContain("novel/20563258");
  });

  it("stops looking like a link when there is nowhere left to go", () => {
    const html = prepareDocumentHtml('<a href="javascript:alert(1)">押しても何も起きない</a>', () => null);
    expect(html).not.toContain("<a");
    expect(html).toContain("押しても何も起きない");
  });

  it("sets a caption as text rather than as a stack of cards", () => {
    // A caption is mostly links - the series, the earlier part, the author's
    // other accounts - and the cards were taller than the caption itself.
    const caption = '<a href="https://www.pixiv.net/novel/series/10848521">https://www.pixiv.net/novel/series/10848521</a>';
    expect(prepareDocumentHtml(caption, () => null, { inlineLinks: true })).not.toContain("novel-link-card");
    expect(prepareDocumentHtml(caption, () => null, { inlineLinks: true })).toContain("novel-inline-link");
    // The body of a work still gets the card, which is what an embed looked like.
    expect(prepareDocumentHtml(caption, () => null)).toContain("novel-link-card");
  });
});

describe("bare URLs while reading", () => {
  const read = (html: string) => prepareDocumentHtml(html, () => null, { linkifyBareUrls: true });

  it("leaves a written-out address as text unless reading", () => {
    // Whether a URL is a link is the author's choice, and the detail screen
    // shows the work as they wrote it.
    const html = prepareDocumentHtml("<p>続きは https://example.com/next です</p>", () => null);
    expect(html).not.toContain("<a");
  });

  it("makes a followable link out of one written mid-sentence", () => {
    const html = read("<p>続きは https://example.com/next です</p>");
    expect(html).toContain('href="https://example.com/next"');
    expect(html).toContain("続きは");
    expect(html).toContain("です");
  });

  it("keeps Japanese sentence punctuation out of the address", () => {
    expect(read("<p>詳細は https://example.com/a。</p>")).toContain('href="https://example.com/a"');
    expect(read("<p>（https://example.com/b）</p>")).toContain('href="https://example.com/b"');
    expect(read("<p>ここ https://example.com/c、そして</p>")).toContain('href="https://example.com/c"');
  });

  it("becomes a card when the address stands alone, as an embed would", () => {
    const html = read("<p>https://example.com/only</p>");
    expect(html).toContain("novel-link-card");
  });

  it("does not touch an address that is already a link", () => {
    const html = read('<p><a href="https://example.com/x">見る</a></p>');
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(html).toContain("見る");
  });

  it("leaves quoted code alone", () => {
    const html = read("<pre>curl https://example.com/api</pre>");
    expect(html).not.toContain("<a");
  });

  it("never creates a link out of a dangerous scheme", () => {
    const html = read("<p>javascript:alert(1) data:text/html,x file:///etc/passwd</p>");
    expect(html).not.toContain("<a");
  });

  it("handles several addresses in one paragraph", () => {
    const html = read("<p>A https://example.com/1 と B https://example.com/2 です</p>");
    expect(html.match(/<a /g)).toHaveLength(2);
  });
});

describe("link card marks", () => {
  const read = (html: string) => prepareDocumentHtml(html, () => null, { linkifyBareUrls: true });

  it("uses each service's own mark, not a generic arrow", () => {
    // pixiv links inside a work used to get the fallback arrow while the same
    // link on an author page carried the pixiv mark.
    const pixiv = read("<p>https://www.pixiv.net/novel/show.php?id=1</p>");
    expect(pixiv).toContain("brand-word-glyph--pixiv");
    expect(pixiv).toContain('data-provider="pixiv"');

    const fanbox = read("<p>https://creator.fanbox.cc/posts/1</p>");
    expect(fanbox).toContain("brand-word-glyph");
    expect(fanbox).toContain('data-provider="fanbox"');
  });

  it("colours the mark from the one service definition", () => {
    const pixiv = read("<p>https://www.pixiv.net/novel/show.php?id=1</p>");
    expect(pixiv.toLowerCase()).toContain("#0096fa");
    const fanbox = read("<p>https://creator.fanbox.cc/posts/1</p>");
    expect(fanbox.toLowerCase()).toContain("#f2c624");
  });

  it("falls back to an arrow only for a service it does not know", () => {
    const other = read("<p>https://example.com/thing</p>");
    expect(other).toContain('data-provider="web"');
    expect(other).not.toContain("brand-word-glyph--pixiv");
  });
});
