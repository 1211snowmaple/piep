import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  const viewer = (current: typeof target | null) => (
    <MantineProvider>
      <QueryClientProvider client={client}>
        <PdfOriginalViewer target={current} onClose={vi.fn()} />
      </QueryClientProvider>
    </MantineProvider>
  );
  const view = render(viewer(target));
  return { ...view, show: (current: typeof target | null) => view.rerender(viewer(current)) };
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

  it("閉じた添付を開き直すと、最初のページから表示する", async () => {
    const view = renderViewer();
    await screen.findByText("1 / 3 ページ");
    await userEvent.click(screen.getByRole("button", { name: /次のページ/ }));
    await screen.findByText("2 / 3 ページ");
    view.show(null);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    view.show(target);
    expect(await screen.findByText("1 / 3 ページ")).toBeInTheDocument();
  });

  it.each([false, true])("別の添付には前の紙面とページ番号を引き継がない（同じパス: %s）", async (samePath) => {
    let finish!: (value: ReturnType<typeof page>) => void;
    dbApi.renderPdfAttachmentPage.mockImplementation((id: number, _path: string, index: number) => id === 7
      ? Promise.resolve(page(index))
      : new Promise<ReturnType<typeof page>>((resolve) => { finish = resolve; }));
    const view = renderViewer();
    await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" });
    await userEvent.click(screen.getByRole("button", { name: /次のページ/ }));
    await screen.findByText("2 / 3 ページ");
    const next = { downloadId: 8, localPath: samePath ? target.localPath : "C:/library/other.pdf", fileName: "別の添付.pdf" };
    view.show(next);
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    await waitFor(() => expect(dbApi.renderPdfAttachmentPage).toHaveBeenCalledWith(8, next.localPath, 0, expect.any(Number)));
    expect(dbApi.renderPdfAttachmentPage.mock.calls.filter(([id, , index]) => id === 8 && index !== 0)).toEqual([]);
    await act(async () => { finish({ ...page(0), pageCount: 1, image: "data:image/webp;base64,other" }); });
    expect(await screen.findByText("1 / 1 ページ")).toBeInTheDocument();
  });

  it("素早く複数のキーが届いても、総ページ数を越えない", async () => {
    renderViewer();
    await screen.findByRole("img", { name: "本文.pdf の 1 ページ目" });
    await userEvent.click(screen.getByRole("button", { name: /次のページ/ }));
    await screen.findByText("2 / 3 ページ");
    act(() => {
      fireEvent.keyDown(window, { key: "ArrowRight" });
      fireEvent.keyDown(window, { key: "ArrowRight" });
    });
    await waitFor(() => expect(screen.getByText("3 / 3 ページ")).toBeInTheDocument());
    expect(dbApi.renderPdfAttachmentPage.mock.calls.some(([, , index]) => index >= 3)).toBe(false);
  });

  it("says so when a page cannot be drawn, instead of showing an empty sheet", async () => {
    dbApi.renderPdfAttachmentPage.mockReset().mockRejectedValue("pdfium.dll が見つかりません");
    renderViewer();
    expect(await screen.findByText(/pdfium.dll が見つかりません/)).toBeInTheDocument();
  });
});
