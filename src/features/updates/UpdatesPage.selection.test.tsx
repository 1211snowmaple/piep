import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { it, expect } from "vitest";
import { AppRouter } from "@/app/router";
import UpdatesPage, { CandidatesPanel } from "./UpdatesPage";

function renderUpdatesPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><UpdatesPage /></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

it("更新画面を離れて戻っても、外した候補のチェックは戻らない", async () => {
  window.location.hash = "#/updates";
  const page = renderUpdatesPage();
  const candidate = await screen.findByRole("checkbox", { name: "星を編む人 第十三話を選択" });
  expect(candidate).toBeChecked();
  fireEvent.click(candidate);
  expect(candidate).not.toBeChecked();

  page.unmount();
  window.location.hash = "#/library";
  window.location.hash = "#/updates";
  renderUpdatesPage();

  expect(await screen.findByRole("checkbox", { name: "星を編む人 第十三話を選択" })).not.toBeChecked();
});

it("種類で隠れた選択を明示し、全解除は表示外にも適用する", async () => {
  window.location.hash = "#/updates";
  renderUpdatesPage();
  await screen.findByRole("checkbox", { name: "制作ノート #25を選択" });

  fireEvent.click(screen.getByRole("radio", { name: "新作 1" }));
  expect(screen.getByText(/全候補から\d+件を選択中.*表示外で\d+件を選択中/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "選択をすべて解除" }));
  expect(screen.getByRole("checkbox", { name: "制作ノート #25を選択" })).not.toBeChecked();

  fireEvent.click(screen.getByRole("radio", { name: "すべて 3" }));
  expect(screen.getByRole("checkbox", { name: "4月のまとめ（加筆）を選択" })).not.toBeChecked();
  expect(screen.getByRole("button", { name: /選択候補.*まとめて保存/ })).toBeDisabled();
});

it("一時停止中は保存待ちの候補を選べ、未読込の候補があっても選択分を保存できる", () => {
  const candidate = {
    id: 10, key: "pixiv:10", source: "pixiv" as const, sourceId: "10",
    title: "保存待ちの作品", subtitle: "", targetLabel: "作者", targetType: "author" as const,
    selected: true, status: "queued" as const, kind: "new" as const,
  };
  let saved = false;
  render(
    <MantineProvider>
      <AppRouter><CandidatesPanel
        candidates={[candidate]}
        selectedIds={new Set([candidate.id])}
        selectableIds={[candidate.id]}
        running={false}
        saving={false}
        allowQueuedSelection
        hasMore
        jobActive
        onToggle={() => {}}
        onSelectMany={() => {}}
        onClearSelection={() => {}}
        onSave={() => { saved = true; }}
        onLoadMore={() => {}}
        onDismiss={() => {}}
      /></AppRouter>
    </MantineProvider>,
  );

  expect(screen.getByRole("checkbox", { name: "保存待ちの作品を選択" })).toBeEnabled();
  const save = screen.getByRole("button", { name: "選択候補1件をまとめて保存" });
  expect(save).toBeEnabled();
  fireEvent.click(save);
  expect(saved).toBe(true);
});
