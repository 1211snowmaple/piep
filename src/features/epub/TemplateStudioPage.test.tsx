import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { demoFileContent, demoFileKinds, demoFiles, demoPreview, demoTemplates } from "./templateStudioDemo";
import TemplateStudioPage from "./TemplateStudioPage";

const epubApi = vi.hoisted(() => ({
  listEpubTemplates: vi.fn(),
  getTemplateFiles: vi.fn(),
  listTemplateFileKinds: vi.fn(),
  previewEpubTemplate: vi.fn(),
  readTemplateFile: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  searchDownloadsV2: () => Promise.resolve({ items: [], nextCursor: null, totalEstimate: 0, searchMeta: {}, facetsVersion: 1 }),
}));

vi.mock("@/services/epubApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/epubApi")>()),
  listEpubTemplates: epubApi.listEpubTemplates,
  getTemplateFiles: epubApi.getTemplateFiles,
  listTemplateFileKinds: epubApi.listTemplateFileKinds,
  previewEpubTemplate: epubApi.previewEpubTemplate,
  readTemplateFile: epubApi.readTemplateFile,
}));

describe("TemplateStudioPage unsaved guard", () => {
  it("blocks leaving after a structure edit until the user confirms", async () => {
    const template = { ...demoTemplates[0], name: "custom", isBuiltin: false };
    epubApi.listEpubTemplates.mockResolvedValue([template]);
    epubApi.getTemplateFiles.mockResolvedValue(demoFiles);
    epubApi.listTemplateFileKinds.mockResolvedValue(demoFileKinds);
    epubApi.previewEpubTemplate.mockResolvedValue(demoPreview);
    epubApi.readTemplateFile.mockImplementation((_name: string, filename: string) => Promise.resolve(demoFileContent(filename)));
    const confirmNavigation = vi.fn().mockResolvedValue(false);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    window.location.hash = "#/epub/templates";

    render(
      <MantineProvider>
        <ModalsProvider>
          <QueryClientProvider client={client}>
            <AppRouter confirmNavigation={confirmNavigation}><TemplateStudioPage /></AppRouter>
          </QueryClientProvider>
        </ModalsProvider>
      </MantineProvider>,
    );

    const label = await screen.findByLabelText("表示名");
    fireEvent.change(label, { target: { value: "未保存の表示名" } });
    fireEvent.click(screen.getByRole("button", { name: "書き出しへ" }));

    await waitFor(() => expect(confirmNavigation).toHaveBeenCalledOnce());
    expect(window.location.hash).toBe("#/epub/templates");
  });

  it("asks before discarding code when another template file is selected", async () => {
    const template = { ...demoTemplates[0], name: "custom", isBuiltin: false };
    epubApi.listEpubTemplates.mockResolvedValue([template]);
    epubApi.getTemplateFiles.mockResolvedValue(demoFiles);
    epubApi.listTemplateFileKinds.mockResolvedValue(demoFileKinds);
    epubApi.previewEpubTemplate.mockResolvedValue(demoPreview);
    epubApi.readTemplateFile.mockImplementation((_name: string, filename: string) => Promise.resolve(demoFileContent(filename)));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    window.location.hash = "#/epub/templates";
    render(
      <MantineProvider>
        <ModalsProvider>
          <QueryClientProvider client={client}>
            <AppRouter><TemplateStudioPage /></AppRouter>
          </QueryClientProvider>
        </ModalsProvider>
      </MantineProvider>,
    );

    const codeTab = await screen.findByRole("tab", { name: "コード" });
    fireEvent.click(codeTab);
    await waitFor(() => expect(codeTab).toHaveAttribute("aria-selected", "true"));
    const editor = await screen.findByLabelText("style.css.j2の内容");
    await waitFor(() => expect(editor).toHaveValue(demoFileContent("style.css.j2")));
    fireEvent.change(editor, { target: { value: "/* unsaved */" } });
    expect(editor).toHaveValue("/* unsaved */");
    fireEvent.click(screen.getByRole("button", { name: "_base_style.css.j2" }));

    expect(await screen.findByText("保存していないコードを破棄しますか？")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "編集を続ける" }));
    expect(screen.getByDisplayValue("/* unsaved */")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "_base_style.css.j2" }));
    fireEvent.click(await screen.findByRole("button", { name: "破棄して移動" }));
    expect(await screen.findByLabelText("_base_style.css.j2の内容")).toBeInTheDocument();
  });
});
