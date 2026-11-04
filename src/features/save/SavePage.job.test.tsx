import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { hasUnsavedWork } from "@/lib/unsavedGuard";
import SavePage from "./SavePage";
import { resetSaveDraftsForTest } from "./saveDraft";

const browserApi = vi.hoisted(() => ({
  openEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  setEmbeddedBrowserBounds: vi.fn().mockResolvedValue(true),
  setEmbeddedBrowserVisible: vi.fn().mockResolvedValue(true),
  navigateEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  getEmbeddedBrowserUrl: vi.fn().mockResolvedValue(null),
  closeEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  destroyEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  goBackEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  goForwardEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  reloadEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  openStandaloneBrowser: vi.fn().mockResolvedValue(false),
  closeStandaloneBrowser: vi.fn().mockResolvedValue(true),
  getStandaloneBrowserUrl: vi.fn().mockResolvedValue(null),
  readBrowserPageWorkCandidates: vi.fn(),
  readBrowserPageUserCandidates: vi.fn(),
}));

const upsertTarget = vi.hoisted(() => vi.fn());
const downloadApi = vi.hoisted(() => ({ fetchPixivSeriesNovels: vi.fn() }));
const jobs = vi.hoisted(() => ({
  start: vi.fn(),
  states: vi.fn(),
  cancel: vi.fn(),
  wait: vi.fn(),
  summaries: [] as import("@/services/updateJobApi").UpdateJobSummary[],
}));

vi.mock("@/services/browserApi", () => browserApi);
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: vi.fn(() => () => undefined) }));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue("token"), set: vi.fn(), save: vi.fn() } }));
vi.mock("@/services/downloadApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/downloadApi")>()),
  ...downloadApi,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  upsertUpdateTarget: upsertTarget,
}));
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  startSaveJobCommand: jobs.start,
  listUpdateJobItemStatesCommand: jobs.states,
  cancelUpdateJobCommand: jobs.cancel,
}));
vi.mock("@/features/updates/updateJobs", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/updates/updateJobs")>()),
  waitForUpdateJob: jobs.wait,
  useUpdateJobSummaries: () => jobs.summaries,
}));

const SERIES = "https://www.pixiv.net/novel/series/1000";
const summary = (status: "running" | "completed" | "canceled" = "running") => ({
  jobId: "save-1",
  status,
  scope: "save" as const,
  mode: "save" as const,
  totals: 2,
  processed: status === "running" ? 0 : 2,
  candidateCount: 0,
  savedCount: status === "completed" ? 2 : 0,
  errorCount: 0,
  activeLabel: null,
  startedAt: "2026-08-28T00:00:00Z",
  updatedAt: "2026-08-28T00:00:00Z",
  finishedAt: status === "running" ? null : "2026-08-28T00:00:01Z",
  logs: [],
  candidates: [],
  nextCandidateCursor: null,
  previousLogCursor: null,
});

function renderSavePage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><SavePage /></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

async function collectCandidates() {
  const address = await screen.findByLabelText("ブラウザのアドレス");
  fireEvent.change(address, { target: { value: SERIES } });
  fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
  fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
  expect(await screen.findByText("第一話")).toBeInTheDocument();
}

describe("SavePage の Rust 保存ジョブ", () => {
  beforeEach(() => {
    resetSaveDraftsForTest();
    window.location.hash = "#/save/pixiv";
    downloadApi.fetchPixivSeriesNovels.mockReset().mockResolvedValue([
      { id: 1, title: "第一話", user: { name: "作者" } },
      { id: 2, title: "第二話", user: { name: "作者" } },
    ]);
    jobs.start.mockReset().mockResolvedValue(summary());
    jobs.states.mockReset().mockResolvedValue([
      { source: "pixiv", sourceId: "1", status: "saved", error: null },
      { source: "pixiv", sourceId: "2", status: "saved", error: null },
    ]);
    jobs.cancel.mockReset().mockResolvedValue(summary("canceled"));
    jobs.summaries = [];
    jobs.wait.mockReset().mockImplementation(async (_jobId, onSnapshot) => {
      const final = summary("completed");
      onSnapshot?.(final);
      return final;
    });
  });

  it("選んだ作品を本文ではなく取得元IDで Rust へ一度だけ預ける", async () => {
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));

    await waitFor(() => expect(jobs.start).toHaveBeenCalledTimes(1));
    expect(jobs.start).toHaveBeenCalledWith([
      { source: "pixiv", sourceId: "1", title: "第一話" },
      { source: "pixiv", sourceId: "2", title: "第二話" },
    ], expect.any(Boolean));
    await waitFor(() => expect(screen.getByRole("button", { name: "選択したものは保存済みです" })).toBeDisabled());
  });

  it("保存中に画面を離れて戻っても同じ候補を再送しない", async () => {
    let finish!: (value: ReturnType<typeof summary>) => void;
    jobs.wait.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const first = renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    await waitFor(() => expect(jobs.start).toHaveBeenCalledTimes(1));

    first.unmount();
    renderSavePage();
    const button = await screen.findByRole("button", { name: "保存・更新の処理中です" });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(jobs.start).toHaveBeenCalledTimes(1);

    finish(summary("completed"));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
  });

  it("再起動後もRustで保存中なら候補を再送しない", async () => {
    jobs.summaries = [summary()];
    renderSavePage();
    await collectCandidates();
    expect(screen.getByRole("button", { name: "保存・更新の処理中です" })).toBeDisabled();
    expect(jobs.start).not.toHaveBeenCalled();
  });

  it("取得元名をパスに含む別サイトでは候補取得を有効にしない", async () => {
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: "https://evil.example/www.pixiv.net/novel/series/1000" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));

    expect(await screen.findByText("作品・シリーズ・作者・一覧のページを開いてください")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "候補を取得" })).toBeDisabled();
    expect(downloadApi.fetchPixivSeriesNovels).not.toHaveBeenCalled();
  });

  it("一覧のページは表示中の文書から読み取り、棚にある作品を預けない", async () => {
    const page = "https://www.pixiv.net/novel/bookmark_new.php";
    browserApi.readBrowserPageWorkCandidates.mockReset().mockResolvedValue({
      url: page,
      truncated: false,
      works: [
        { sourceId: "1", title: "第一話", author: "作者", saved: false },
        { sourceId: "2", title: "第二話", author: "作者", saved: true },
      ],
    });
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: page } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));

    fireEvent.click(await screen.findByRole("button", { name: "このページから読み取る" }));

    expect(await screen.findByText("第一話")).toBeInTheDocument();
    expect(browserApi.readBrowserPageWorkCandidates).toHaveBeenCalledWith("embedded", "pixiv");
    expect(downloadApi.fetchPixivSeriesNovels).not.toHaveBeenCalled();
    expect(screen.getByText("保存済み")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "1件をライブラリに保存" }));

    await waitFor(() => expect(jobs.start).toHaveBeenCalledTimes(1));
    expect(jobs.start).toHaveBeenCalledWith([{ source: "pixiv", sourceId: "1", title: "第一話" }], false);
  });

  it("手で続きのページへ進んで読むと候補に足し、選び直した行を残す", async () => {
    const first = "https://www.pixiv.net/tags/x/novels";
    const second = `${first}?p=2`;
    browserApi.readBrowserPageWorkCandidates.mockReset()
      .mockResolvedValueOnce({ url: first, truncated: false, works: [{ sourceId: "1", title: "第一話", author: "", saved: false }] })
      .mockResolvedValueOnce({ url: second, truncated: false, works: [{ sourceId: "2", title: "第二話", author: "", saved: false }] });
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: first } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "このページから読み取る" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: "第一話を保存対象にする" }));

    fireEvent.change(address, { target: { value: second } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "このページから読み取る" }));

    expect(await screen.findByText("第二話")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "第一話を保存対象にする" })).not.toBeChecked();
    expect(screen.getByText("1 / 2件を選択")).toBeInTheDocument();
    expect(browserApi.navigateEmbeddedBrowser).not.toHaveBeenCalledWith(`${first}?p=3`);
  });

  it("続きを開ける一覧でだけ、読み方を指定できる", async () => {
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: "https://www.pixiv.net/" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    await screen.findByRole("button", { name: "このページから読み取る" });
    expect(screen.queryByRole("button", { name: "読み方" })).not.toBeInTheDocument();

    fireEvent.change(address, { target: { value: "https://www.pixiv.net/novel/bookmark_new.php" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "読み方" }));
    fireEvent.change(await screen.findByLabelText("ページ数"), { target: { value: "5" } });

    expect(await screen.findByRole("button", { name: "5ページ読み取る" })).toBeInTheDocument();
    expect(screen.getByText(/5ページ · 5秒ごと/)).toBeInTheDocument();
    expect(screen.getByText(/その分の通信が増えます/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("ページ数"), { target: { value: "1" } });
  });

  it("フォロー一覧から作者を読み、選んだ人だけ監視に足す", async () => {
    const page = "https://www.pixiv.net/users/7/following";
    upsertTarget.mockReset().mockResolvedValue(undefined);
    browserApi.readBrowserPageUserCandidates.mockReset().mockResolvedValue({
      url: page,
      truncated: false,
      users: [
        { userId: "1", name: "作者一", watched: false },
        { userId: "2", name: "作者二", watched: true },
        { userId: "3", name: "作者三", watched: false },
      ],
    });
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: page } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "フォローユーザーを読み取る" }));

    expect(await screen.findByRole("checkbox", { name: "作者二を監視に追加する" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: "作者三を監視に追加する" }));
    fireEvent.click(screen.getByRole("button", { name: "1人を監視に追加" }));

    await waitFor(() => expect(upsertTarget).toHaveBeenCalledTimes(1));
    expect(upsertTarget).toHaveBeenCalledWith({ targetType: "author", source: "pixiv", sourceKey: "1", displayName: "作者一", enabled: true, metadataJson: null });
    expect(browserApi.readBrowserPageWorkCandidates).not.toHaveBeenCalled();
    expect(await screen.findByRole("checkbox", { name: "作者一を監視に追加する" })).toBeDisabled();
  });

  it("読み取れる作品が無いページでは候補を置き換えない", async () => {
    const page = "https://www.pixiv.net/novel/bookmark_new.php";
    browserApi.readBrowserPageWorkCandidates.mockReset().mockResolvedValue({ url: page, truncated: false, works: [] });
    renderSavePage();
    await collectCandidates();
    const address = screen.getByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: page } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));

    fireEvent.click(await screen.findByRole("button", { name: "このページから読み取る" }));

    await waitFor(() => expect(browserApi.readBrowserPageWorkCandidates).toHaveBeenCalled());
    expect(screen.getByText("第一話")).toBeInTheDocument();
  });

  it("保留された作品を選択数や再保存対象に含めない", async () => {
    jobs.states.mockResolvedValue([
      { source: "pixiv", sourceId: "1", status: "held", error: null },
      { source: "pixiv", sourceId: "2", status: "saved", error: null },
    ]);
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));

    const held = await screen.findByRole("checkbox", { name: "第一話を保存対象にする" });
    await waitFor(() => expect(held).toBeDisabled());
    expect(held).not.toBeChecked();
    expect(screen.getByText("1 / 2件を選択")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "すべて" }));
    expect(held).not.toBeChecked();
    expect(screen.getByText("1 / 2件を選択")).toBeInTheDocument();
  });

  it("実行中は終了を守り、中止を同じ Rust ジョブへ渡す", async () => {
    let finish!: (value: ReturnType<typeof summary>) => void;
    jobs.wait.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));

    await waitFor(() => expect(jobs.start).toHaveBeenCalled());
    expect(hasUnsavedWork("close")).toBe(true);
    fireEvent.click(await screen.findByRole("button", { name: "中止" }));
    expect(await screen.findByRole("button", { name: "中止しています" })).toBeDisabled();
    await waitFor(() => expect(jobs.cancel).toHaveBeenCalledWith("save-1"));

    finish(summary("canceled"));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
  });

  it("候補欄をたたんでも進捗・中止・ライブラリを残す", async () => {
    let finish!: (value: ReturnType<typeof summary>) => void;
    jobs.wait.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    await waitFor(() => expect(jobs.start).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "保存候補をたたむ" }));

    expect(screen.getByRole("button", { name: "保存候補を開く" })).toBeInTheDocument();
    expect(screen.getByRole("status", { name: "保存候補 2件中2件を選択" })).toBeInTheDocument();
    expect(screen.getByRole("status", { name: /保存の準備をしています 0\/2/ })).toBeInTheDocument();
    expect(screen.getByText("0/2")).toBeInTheDocument();
    expect(screen.queryByText("保存中")).toBeNull();
    expect(screen.queryByText("中止")).toBeNull();
    expect(screen.getByRole("button", { name: "ライブラリを開く" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "保存を中止" }));
    expect(await screen.findByRole("button", { name: "中止しています" })).toBeDisabled();
    await waitFor(() => expect(jobs.cancel).toHaveBeenCalledWith("save-1"));

    finish(summary("canceled"));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
  });
});
