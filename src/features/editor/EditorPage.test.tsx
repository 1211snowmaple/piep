import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { getDemoEditor } from "@/mocks/demoData";
import type { WorkEditRevision } from "@/types/library";
import EditorPage, { isSafeDocumentLink } from "./EditorPage";

const dbApi = vi.hoisted(() => ({
  getEditorDocument: vi.fn(),
  saveWorkDraft: vi.fn(),
  activateWorkEdit: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getEditorDocument: dbApi.getEditorDocument,
  saveWorkDraft: dbApi.saveWorkDraft,
  activateWorkEdit: dbApi.activateWorkEdit,
}));

const revision: WorkEditRevision = { id: 8, downloadId: 101, baseVersion: 3, status: "draft", title: null, contentHash: null, createdAt: "2026-08-12T00:00:00Z", updatedAt: "2026-08-12T00:00:00Z" };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function renderEditor() {
  window.location.hash = "#/editor/101";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><EditorPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
  return client;
}

describe("EditorPage", () => {
  beforeEach(() => {
    dbApi.getEditorDocument.mockResolvedValue(getDemoEditor(101));
    dbApi.saveWorkDraft.mockResolvedValue(revision);
    dbApi.activateWorkEdit.mockResolvedValue({ ...revision, status: "active" });
  });

  it("accepts only http(s) document links", () => {
    expect(isSafeDocumentLink("https://example.com/story")).toBe(true);
    expect(isSafeDocumentLink("http://localhost/story")).toBe(true);
    expect(isSafeDocumentLink("javascript:alert(1)")).toBe(false);
    expect(isSafeDocumentLink("file:///C:/secret.txt")).toBe(false);
    expect(isSafeDocumentLink("not a url")).toBe(false);
  });

  it("keeps a stable hook order when the document finishes loading", async () => {
    renderEditor();
    const heading = await screen.findByLabelText("見出し 1");
    fireEvent.change(heading, { target: { value: "変更した見出し" } });
    expect(screen.getByText("未保存")).toBeInTheDocument();
  });

  it.each(["下書き保存", "反映"])("keeps newer edits dirty when %s finishes", async (action) => {
    const pending = deferred<WorkEditRevision>();
    dbApi.saveWorkDraft.mockReturnValueOnce(pending.promise);
    renderEditor();
    const heading = await screen.findByLabelText("見出し 1");
    fireEvent.change(heading, { target: { value: "保存対象の見出し" } });
    fireEvent.click(screen.getByRole("button", { name: action }));
    await waitFor(() => expect(dbApi.saveWorkDraft).toHaveBeenCalledOnce());

    fireEvent.change(heading, { target: { value: "保存開始後の見出し" } });
    await act(async () => { pending.resolve(revision); });
    if (action === "反映") await waitFor(() => expect(dbApi.activateWorkEdit).toHaveBeenCalledOnce());

    await waitFor(() => expect(screen.getByText("未保存")).toBeInTheDocument());
    expect(heading).toHaveValue("保存開始後の見出し");
  });

  it("clears an unchanged published snapshot and invalidates the reader's real query keys", async () => {
    const client = renderEditor();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const heading = await screen.findByLabelText("見出し 1");
    fireEvent.change(heading, { target: { value: "公開する見出し" } });
    fireEvent.click(screen.getByRole("button", { name: "反映" }));

    await waitFor(() => expect(dbApi.activateWorkEdit).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.queryByText("未保存")).toBeNull());
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["reader-metadata", 101] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["reader-content-page", 101] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["reader-content-search", 101] });
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: ["reader-document", 101] });
  });
});
