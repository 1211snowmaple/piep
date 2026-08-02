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
});
