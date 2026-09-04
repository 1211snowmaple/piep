import { useRef } from "react";
import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { forgetContentLinkResolutions, useLibraryLinkMarks } from "@/lib/contentLinks";

const dbApi = vi.hoisted(() => ({
  getDownloadBySource: vi.fn(),
  getPerson: vi.fn(),
  getSeries: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getDownloadBySource: dbApi.getDownloadBySource,
  getPerson: dbApi.getPerson,
  getSeries: dbApi.getSeries,
}));

function Body({ html }: { html: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useLibraryLinkMarks(ref, html);
  return <div ref={ref} dangerouslySetInnerHTML={{ __html: html }} />;
}

describe("本文のリンクに、手元にあることを示す印", () => {
  beforeEach(() => {
    forgetContentLinkResolutions();
    dbApi.getDownloadBySource.mockReset();
    dbApi.getPerson.mockReset();
    dbApi.getSeries.mockReset();
    dbApi.getPerson.mockResolvedValue(null);
    dbApi.getSeries.mockResolvedValue(null);
  });

  /**
   * FANBOX の投稿は、続きの回や関連する記事をカードで指す。その宛先が棚に
   * あるなら外の住所ではないのに、押してみるまで見分けが付かなかった。
   */
  it("保存済みの投稿を指すカードには、そう書いた札が付く", async () => {
    dbApi.getDownloadBySource.mockResolvedValue({ id: 42 });
    const html = `<a href="https://sio.fanbox.cc/posts/8421" class="novel-link-card"><span class="link-card-info"><span class="link-card-title">前回</span></span></a>`;
    const view = render(<Body html={html} />);

    await waitFor(() => expect(view.container.querySelector("a")?.dataset.inLibrary).toBe("1"));
    expect(view.container.querySelector(".link-card-saved")?.textContent).toBe("ライブラリにあります");
    expect(dbApi.getDownloadBySource).toHaveBeenCalledWith("fanbox", "8421");
  });

  it("保存していない宛先には、何も足さない", async () => {
    dbApi.getDownloadBySource.mockResolvedValue(null);
    const view = render(<Body html={`<a href="https://sio.fanbox.cc/posts/999" class="novel-link-card"><span class="link-card-info"></span></a>`} />);

    await waitFor(() => expect(dbApi.getDownloadBySource).toHaveBeenCalled());
    expect(view.container.querySelector("a")?.dataset.inLibrary).toBeUndefined();
    expect(view.container.querySelector(".link-card-saved")).toBeNull();
  });

  it("同じ宛先が何本並んでも、問い合わせは一度きり", async () => {
    dbApi.getDownloadBySource.mockResolvedValue({ id: 7 });
    const link = `<a href="https://www.pixiv.net/novel/show.php?id=111">続き</a>`;
    const view = render(<Body html={`${link}${link}${link}`} />);

    await waitFor(() => expect(view.container.querySelectorAll("a[data-in-library]").length).toBe(3));
    expect(dbApi.getDownloadBySource).toHaveBeenCalledTimes(1);
    // 地の文のリンクは札ではなく、控えめな印で示す。
    expect(view.container.querySelector("a")?.classList.contains("novel-inline-link--saved")).toBe(true);
  });

  it("外のリンクは問い合わせもしない", async () => {
    render(<Body html={`<a href="https://example.com/">外の記事</a>`} />);
    await Promise.resolve();
    expect(dbApi.getDownloadBySource).not.toHaveBeenCalled();
  });
});
