import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 添付 PDF の原本表示。
 *
 * 守るのは、**押した通りのページが描かれること**と、送っている間に紙面が
 * 消えないことである。原本は取り込んだ本文を疑うために開くので、いま何ページ
 * 目を見ているのかが分からなくなると用をなさない。
 */
const dbApi = vi.hoisted(() => ({ renderPdfAttachmentPage: vi.fn() }));
vi.mock("@/services/dbApi", () => dbApi);

import { PdfOriginalViewer } from "./PdfOriginalViewer";

const target = { downloadId: 7, localPath: "C:/library/work/本文.pdf", fileName: "本文.pdf" };

function page(index: number) {
  return {
    image: `data:image/webp;base64,page-${index}`,
    width: 1200,
    height: 1553,
    pageCount: 3,
  };
}

function renderViewer() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <PdfOriginalViewer target={target} onClose={vi.fn()} />
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("PdfOriginalViewer", () => {
  beforeEach(() => {
    dbApi.renderPdfAttachmentPage.mockReset().mockImplementation(
      (_id: number, _path: string, index: number) => Promise.resolve(page(index)),
    );
  });

  it("draws the first page and says where in the file it is", async () => {
    renderViewer();
    const image = await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" });
    expect(image).toHaveAttribute("src", "data:image/webp;base64,page-0");
    expect(await screen.findByText("1 / 3 ページ")).toBeInTheDocument();
    // 最初のページでは戻れない。
    expect(screen.getByRole("button", { name: /前のページ/ })).toBeDisabled();
  });

  it("asks for the next page and keeps a page on screen while it draws", async () => {
    renderViewer();
    await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" });
    await userEvent.click(screen.getByRole("button", { name: /次のページ/ }));

    await waitFor(() =>
      expect(dbApi.renderPdfAttachmentPage).toHaveBeenCalledWith(7, target.localPath, 1, expect.any(Number)),
    );
    // 描き替わるまでのあいだ、紙面が消えて空白にならないこと。
    expect(screen.getByRole("img")).toBeInTheDocument();
    await screen.findByRole("img", { name: "本文.pdf の 2 ページ目" });
    expect(await screen.findByText("2 / 3 ページ")).toBeInTheDocument();
  });

  it("stops at the last page", async () => {
    renderViewer();
    await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" });
    await userEvent.click(screen.getByRole("button", { name: /次のページ/ }));
    await screen.findByRole("img", { name: "本文.pdf の 2 ページ目" });
    await userEvent.click(screen.getByRole("button", { name: /次のページ/ }));
    await screen.findByRole("img", { name: "本文.pdf の 3 ページ目" });

    expect(screen.getByRole("button", { name: /次のページ/ })).toBeDisabled();
  });

  it("says so when a page cannot be drawn, instead of showing an empty sheet", async () => {
    dbApi.renderPdfAttachmentPage.mockReset().mockRejectedValue("pdfium.dll が見つかりません");
    renderViewer();
    expect(await screen.findByText(/pdfium.dll が見つかりません/)).toBeInTheDocument();
  });
});
