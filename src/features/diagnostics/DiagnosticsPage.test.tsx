import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import DiagnosticsPage, { previewDiagnostics } from "./DiagnosticsPage";
import type { LibraryDiagnostics } from "@/types/library";

function renderPage(previewData?: LibraryDiagnostics) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><DiagnosticsPage previewData={previewData} /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
}

describe("DiagnosticsPage", () => {
  it("waits to be asked before measuring", async () => {
    // Measuring walks the whole storage folder and runs three benchmarks
    // against the real library. Opening a screen is not a request for that.
    renderPage();
    expect(await screen.findByRole("heading", { name: "ライブラリ診断" })).toBeInTheDocument();
    expect(screen.getByText("まだ計測していません")).toBeInTheDocument();
    expect(screen.queryByText("実データ性能")).toBeNull();
  });

  it("explains measured performance and storage health once it has run", async () => {
    renderPage();
    fireEvent.click(await screen.findByRole("button", { name: "計測する" }));
    expect(await screen.findByText("実データ性能")).toBeInTheDocument();
    expect(screen.getByText("索引と保存領域")).toBeInTheDocument();
    expect(screen.getByText("DB参照と保存ファイルは一致しています")).toBeInTheDocument();
    expect(screen.getByText(/9,633件のDB参照を照合/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "DBを最適化" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "検索索引を最適化" })).toBeDisabled();
  });

  it("turns file warnings into inspectable recovery choices", async () => {
    renderPage({
      ...previewDiagnostics,
      missingJsonFiles: 1,
      unsafeReferencedFiles: 1,
      fileIssueSamples: [
        {
          issueType: "missing",
          category: "work_json",
          path: "C:\\library\\downloads\\pixiv\\42\\v1\\original.json",
          label: null,
          expectedSizeBytes: null,
          actualSizeBytes: null,
        },
        {
          issueType: "unsafe",
          category: "profile",
          path: "C:\\outside\\icon.png",
          label: null,
          expectedSizeBytes: null,
          actualSizeBytes: 128,
        },
      ],
    });
    fireEvent.click(await screen.findByRole("button", { name: "計測する" }));
    expect(await screen.findByText("次に行うこと")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "対象ファイルを確認" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "残ったフォルダーを再取り込み" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "バックアップ復元へ" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "元サービスから再保存" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "対象ファイルを確認" }));
    expect(await screen.findByRole("dialog", { name: "確認が必要なファイル（最大2件）" })).toBeInTheDocument();
    expect(screen.getByText("C:\\library\\downloads\\pixiv\\42\\v1\\original.json")).toBeInTheDocument();
    expect(screen.getByText("C:\\outside\\icon.png")).toBeInTheDocument();
    expect(screen.getByText("表示だけでは変更しません")).toBeInTheDocument();
  });
});
