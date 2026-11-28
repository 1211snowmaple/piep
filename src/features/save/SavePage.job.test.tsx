import { MantineProvider } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { getOperationJobs } from "@/features/jobs/operationJobs";
import { hasUnsavedWork } from "@/lib/unsavedGuard";
import type { UpdateJobItemState, UpdateJobStatus } from "@/services/updateJobApi";
import SavePage from "./SavePage";
import { readSaveDraft, resetSaveDraftsForTest } from "./saveDraft";
import { summarizeSaveJob } from "./saveJobRows";

const browserApi = vi.hoisted(() => ({
  openEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  setEmbeddedBrowserBounds: vi.fn().mockResolvedValue(true),
  setEmbeddedBrowserVisible: vi.fn().mockResolvedValue(true),
  navigateEmbeddedBrowser: vi.fn().mockResolvedValue(undefined),
  getEmbeddedBrowserUrl: vi.fn().mockResolvedValue(null),
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
const downloadApi = vi.hoisted(() => ({ fetchPixivSeriesNovels: vi.fn(), fetchFanboxCreatorPosts: vi.fn() }));
const jobs = vi.hoisted(() => ({
  start: vi.fn(),
  get: vi.fn(),
  states: vi.fn(),
  cancel: vi.fn(),
  wait: vi.fn(),
  summaries: [] as import("@/services/updateJobApi").UpdateJobSummary[],
}));

vi.mock("@/services/browserApi", () => browserApi);
vi.mock("@/services/eventBus", () => ({ subscribeTauriEvent: vi.fn(() => () => undefined) }));
vi.mock("@/services/authApi", () => ({ getConnectionStatus: vi.fn().mockResolvedValue({ pixiv: { name: "利用者", webSession: true }, fanbox: { name: "利用者" } }) }));
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
  getUpdateJobCommand: jobs.get,
  listUpdateJobItemStatesCommand: jobs.states,
  cancelUpdateJobCommand: jobs.cancel,
}));
vi.mock("@/features/updates/updateJobs", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/updates/updateJobs")>()),
  waitForUpdateJob: jobs.wait,
  useUpdateJobSummaries: () => jobs.summaries,
}));

const SERIES = "https://www.pixiv.net/novel/series/1000";
const summary = (status: UpdateJobStatus = "running") => ({
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

async function collectCandidates(url = SERIES) {
  const address = await screen.findByLabelText("ブラウザのアドレス");
  fireEvent.change(address, { target: { value: url } });
  fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
  fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
  expect(await screen.findByText("第一話")).toBeInTheDocument();
}

describe("SavePage の Rust 保存ジョブ", () => {
  beforeEach(() => {
    resetSaveDraftsForTest();
    window.location.hash = "#/save/pixiv";
    downloadApi.fetchPixivSeriesNovels.mockReset().mockResolvedValue({ truncated: false, items: [
      { id: 1, title: "第一話", user: { name: "作者" } },
      { id: 2, title: "第二話", user: { name: "作者" } },
    ] });
    jobs.start.mockReset().mockResolvedValue(summary());
    jobs.get.mockReset().mockResolvedValue(summary("completed"));
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

  it("保存元のクエリとフラグメントを操作履歴に書かない", async () => {
    renderSavePage();
    await collectCandidates(`${SERIES}?tracking=example#private`);
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    await waitFor(() => expect(jobs.start).toHaveBeenCalledOnce());
    const detail = getOperationJobs()[0].detail;
    expect(detail).toContain("シリーズ 1000");
    expect(detail).not.toMatch(/tracking|private|https?:/);
    const stored = window.localStorage.getItem("piep.operation-history");
    expect(stored).toContain(detail);
    expect(stored).not.toMatch(/tracking|private/);
  });

  it("件数の上限に達しても候補を並べ、打ち切りを知らせる", async () => {
    const show = vi.spyOn(notifications, "show");
    try {
      downloadApi.fetchPixivSeriesNovels.mockResolvedValueOnce({
        items: [{ id: 1, title: "第一話" }, { id: 2, title: "第二話" }],
        truncated: true,
      });
      renderSavePage();
      await collectCandidates();
      expect(screen.getByRole("button", { name: "2件をライブラリに保存" })).toBeEnabled();
      expect(show).toHaveBeenCalledWith(expect.objectContaining({
        message: "件数の上限で候補の取得を打ち切りました。",
      }));
    } finally {
      show.mockRestore();
    }
  });

  it.each(["a&b", "a%3Fb", "a%2Fb", "a%25b", "a b", "a".repeat(65)])(
    "不正なクリエイター ID のページから候補を取得しない（%s）",
    async (id) => {
      window.location.hash = "#/save/fanbox";
      downloadApi.fetchFanboxCreatorPosts.mockClear();
      browserApi.readBrowserPageWorkCandidates.mockClear();
      renderSavePage();
      const address = await screen.findByLabelText("ブラウザのアドレス");
      fireEvent.change(address, { target: { value: `https://www.fanbox.cc/@${id}` } });
      fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
      expect(await screen.findByRole("button", { name: "候補を取得" })).toBeDisabled();
      expect(downloadApi.fetchFanboxCreatorPosts).not.toHaveBeenCalled();
      expect(browserApi.readBrowserPageWorkCandidates).not.toHaveBeenCalled();
    },
  );

  it("再試行のジョブのイベントで失敗行を処理中に戻す", async () => {
    const failed = { source: "pixiv", sourceId: "1", status: "failed", error: "取得できませんでした" };
    const saved = { source: "pixiv", sourceId: "2", status: "saved", error: null };
    jobs.states
      .mockResolvedValueOnce([failed, saved])
      .mockResolvedValueOnce([failed, saved])
      .mockRejectedValueOnce(new Error("開始時の照会に失敗しました"))
      .mockResolvedValue([{ ...failed, status: "saved", error: null }]);
    let reportPreviousItem!: (state: UpdateJobItemState) => void;
    jobs.wait.mockImplementationOnce(async (_jobId, _onSnapshot, onItemState) => {
      reportPreviousItem = onItemState;
      return { ...summary("failed"), savedCount: 1, errorCount: 1 };
    });
    jobs.start.mockResolvedValueOnce(summary()).mockResolvedValueOnce({ ...summary(), jobId: "save-2" });
    let finish!: () => void;
    jobs.wait.mockImplementationOnce((_jobId, _onSnapshot, onItemState) => new Promise((resolve) => {
      onItemState({ ...failed, status: "running", error: null });
      finish = () => resolve({ ...summary("completed"), jobId: "save-2", totals: 1, processed: 1, savedCount: 1 });
    }));
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    fireEvent.click(await screen.findByRole("button", { name: "失敗した1件をやり直す" }));
    await waitFor(() => expect(finish).toBeDefined());
    const running = readSaveDraft("pixiv").items[0];
    act(() => reportPreviousItem(failed));
    const afterOldEvent = readSaveDraft("pixiv").items[0];
    await act(async () => finish());
    expect(running).toMatchObject({ status: "downloading", error: undefined });
    expect(afterOldEvent).toBe(running);
  });

  it("中断後の待機行を表示し、操作の記録にも中断を残す", async () => {
    const running = { source: "pixiv", sourceId: "1", status: "running", error: null };
    const queued = { source: "pixiv", sourceId: "2", status: "queued", error: null };
    jobs.states.mockResolvedValueOnce([running, queued]).mockResolvedValue([{ ...running, status: "queued" }, queued]);
    const paused = { ...summary("paused"), processed: 0, activeLabel: "一時停止しました" };
    jobs.wait.mockImplementationOnce(async (_jobId, onSnapshot) => {
      onSnapshot(paused);
      return paused;
    });
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    await waitFor(() => expect(jobs.states).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
    expect(readSaveDraft("pixiv").items.map((item) => item.status)).toEqual(["pending", "pending"]);
    expect(getOperationJobs()[0]).toMatchObject({ status: "paused", current: 0, total: 2 });
  });

  it.each([0, 1])("項目失敗がなくてもジョブ全体の失敗を知らせる（保存 %i 件）", async (savedCount) => {
    const show = vi.spyOn(notifications, "show");
    jobs.states.mockResolvedValue([
      { source: "pixiv", sourceId: "1", status: savedCount ? "saved" : "queued", error: null },
      { source: "pixiv", sourceId: "2", status: "queued", error: null },
    ]);
    jobs.wait.mockResolvedValueOnce({ ...summary("failed"), savedCount, processed: savedCount, activeLabel: "処理が異常終了しました" });
    renderSavePage();
    await collectCandidates();
    show.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
    expect(show).toHaveBeenCalledWith(expect.objectContaining({
      title: savedCount ? "保存が一部失敗しました" : "保存できませんでした",
      message: `保存 ${savedCount} · 保存済み 0 · 失敗 0 · 未処理 ${2 - savedCount}`,
    }));
    expect(getOperationJobs()[0].status).toBe("failed");
    show.mockRestore();
  });

  it("最終照会が失敗したら画面を開き直しても照合まで再送せず、未処理の作品だけを送る", async () => {
    const show = vi.spyOn(notifications, "show");
    const queued = { source: "pixiv", sourceId: "1", status: "queued", error: null };
    const saved = { source: "pixiv", sourceId: "2", status: "saved", error: null };
    const failed = { ...summary("failed"), processed: 1, savedCount: 1 };
    jobs.get.mockResolvedValue(failed);
    jobs.start.mockResolvedValueOnce(summary()).mockResolvedValueOnce({ ...summary(), jobId: "save-2", totals: 1 });
    jobs.states
      .mockResolvedValueOnce([queued, { ...saved, status: "queued" }])
      .mockRejectedValueOnce(new Error("最終照会に失敗しました"))
      .mockRejectedValueOnce(new Error("再照会に失敗しました"))
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([queued, saved])
      .mockResolvedValue([{ ...queued, status: "saved" }]);
    jobs.wait.mockResolvedValueOnce(failed)
      .mockResolvedValueOnce({ ...summary("completed"), jobId: "save-2", totals: 1, processed: 1, savedCount: 1 });
    const first = renderSavePage();
    await collectCandidates();
    show.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
    expect(show).toHaveBeenCalledWith(expect.objectContaining({ title: "作品ごとの保存結果を確認できませんでした" }));

    first.unmount();
    renderSavePage();
    fireEvent.click(await screen.findByRole("button", { name: "前回の結果を確認して保存" }));
    await waitFor(() => expect(jobs.states).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.getByRole("button", { name: "前回の結果を確認して保存" })).toBeEnabled());
    expect(jobs.start).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "前回の結果を確認して保存" }));
    await waitFor(() => expect(jobs.states).toHaveBeenCalledTimes(4));
    await waitFor(() => expect(screen.getByRole("button", { name: "前回の結果を確認して保存" })).toBeEnabled());
    expect(jobs.start).toHaveBeenCalledTimes(1);

    fireEvent.click(await screen.findByRole("button", { name: "前回の結果を確認して保存" }));
    await waitFor(() => expect(jobs.start).toHaveBeenCalledTimes(2));
    expect(jobs.states).toHaveBeenNthCalledWith(5, "save-1");
    expect(jobs.start).toHaveBeenLastCalledWith([
      { source: "pixiv", sourceId: "1", title: "第一話" },
    ], expect.any(Boolean));
    await waitFor(() => expect(hasUnsavedWork("close")).toBe(false));
    show.mockRestore();
  });

  it("全件保存の最終照会が失敗しても、照合して保存済みの行を再送しない", async () => {
    const queued = ["1", "2"].map((sourceId) => ({ source: "pixiv", sourceId, status: "queued", error: null }));
    jobs.states.mockResolvedValueOnce(queued).mockRejectedValueOnce(new Error("照会に失敗しました"));
    renderSavePage();
    await collectCandidates();
    fireEvent.click(screen.getByRole("button", { name: "2件をライブラリに保存" }));

    fireEvent.click(await screen.findByRole("button", { name: "前回の結果を確認して保存" }));
    expect(await screen.findByRole("button", { name: "選択したものは保存済みです" })).toBeDisabled();
    expect(jobs.start).toHaveBeenCalledTimes(1);
  });

  it("閲覧制限を候補取得時のアカウントと一緒に Rust へ預ける", async () => {
    window.location.hash = "#/save/fanbox";
    downloadApi.fetchFanboxCreatorPosts.mockResolvedValue({
      accountKey: "取得時",
      data: { truncated: false, items: [{ id: "9", title: "限定投稿", isRestricted: true, feeRequired: 500, creatorId: "creator" }] },
    });
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: "https://creator.fanbox.cc/" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "候補を取得" }));
    expect(await screen.findByText("限定投稿")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "1件をライブラリに保存" }));
    await waitFor(() => expect(jobs.start).toHaveBeenCalledWith([
      { source: "fanbox", sourceId: "9", title: "限定投稿", fanboxAccess: { accountKey: "取得時", isRestricted: true, feeRequired: 500, creatorId: "creator" } },
    ], expect.any(Boolean)));
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

  it("重複する続きのページは後ろに足し、読み直すと棚から外れた作品を選べる", async () => {
    const first = "https://www.pixiv.net/tags/x/novels";
    const second = `${first}?p=2`;
    const work = (sourceId: string, saved = false) => ({ sourceId, title: `作品${sourceId}`, author: "", saved });
    browserApi.readBrowserPageWorkCandidates.mockReset()
      .mockResolvedValueOnce({ url: first, truncated: false, works: [work("1"), work("2")] })
      .mockResolvedValueOnce({ url: second, truncated: false, works: [work("3"), work("1", true), work("4")] })
      .mockResolvedValueOnce({ url: first, truncated: false, works: [work("2"), work("1")] });
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    const read = async (url: string) => {
      fireEvent.change(address, { target: { value: url } });
      fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
      await act(async () => fireEvent.click(screen.getByRole("button", { name: "このページから読み取る" })));
    };

    await read(first);
    await read(second);
    const checkboxes = () => screen.getAllByRole("checkbox", { name: /を保存対象にする$/ });
    expect(checkboxes().map((box) => box.getAttribute("aria-label"))).toEqual(["作品1を保存対象にする", "作品2を保存対象にする", "作品3を保存対象にする", "作品4を保存対象にする"]);
    expect(checkboxes()[0]).toBeDisabled();

    await read(first);
    const unsaved = screen.getByRole("checkbox", { name: "作品1を保存対象にする" });
    expect(checkboxes()[1]).toBe(unsaved);
    expect(unsaved).toBeEnabled();
    expect(unsaved).not.toBeChecked();
    fireEvent.click(unsaved);
    expect(unsaved).toBeChecked();
  });

  it("続きを開ける一覧でだけ、読み取りの設定を開ける", async () => {
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: "https://www.pixiv.net/" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    await screen.findByRole("button", { name: "このページから読み取る" });
    expect(screen.queryByRole("button", { name: "読み取りの設定" })).not.toBeInTheDocument();

    fireEvent.change(address, { target: { value: "https://www.pixiv.net/novel/bookmark_new.php" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "読み取りの設定" }));
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

  it("保存ジョブの集計は画面で使う項目を返す", () => {
    expect(summarizeSaveJob(summary("completed"))).toEqual({
      outcome: "completed",
      tally: "保存 2 · 保存済み 0 · 失敗 0",
      failed: 0,
      held: 0,
      partial: false,
    });
  });

  it("フォロー中の候補から監視に追加でき、上限での打ち切りを知らせる", async () => {
    window.location.hash = "#/save/fanbox";
    const page = "https://www.fanbox.cc/creators/following";
    upsertTarget.mockReset().mockResolvedValue(undefined);
    browserApi.readBrowserPageUserCandidates.mockReset().mockResolvedValue({
      url: page,
      truncated: true,
      users: [{ userId: "mizu-atelier", name: "mizu atelier", watched: false }],
    });
    const show = vi.spyOn(notifications, "show");
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: page } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    fireEvent.click(await screen.findByRole("button", { name: "クリエイターを読み取る" }));
    fireEvent.click(await screen.findByRole("button", { name: "1人を監視に追加" }));

    await waitFor(() => expect(upsertTarget).toHaveBeenCalledTimes(1));
    expect(show).toHaveBeenCalledWith(expect.objectContaining({ message: "件数の上限で打ち切ったページがあります。" }));
    show.mockRestore();
    expect(browserApi.readBrowserPageUserCandidates).toHaveBeenCalledWith("embedded", "fanbox");
    expect(upsertTarget).toHaveBeenCalledWith({ targetType: "author", source: "fanbox", sourceKey: "mizu-atelier", displayName: "mizu atelier", enabled: true, metadataJson: null });
  });

  it("別の取得元の作品のページでは候補を取得しない", async () => {
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: "https://www.fanbox.cc/@creator/posts/100" } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));

    expect(await screen.findByText("作品・シリーズ・作者・一覧のページを開いてください")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "候補を取得" })).toBeDisabled();
  });

  it("フォロー一覧の候補が 0 人でも、上限で打ち切ったことを知らせる", async () => {
    const page = "https://www.pixiv.net/users/7/following";
    browserApi.readBrowserPageUserCandidates.mockReset().mockResolvedValue({ url: page, truncated: true, users: [] });
    const show = vi.spyOn(notifications, "show");
    try {
      renderSavePage();
      const address = await screen.findByLabelText("ブラウザのアドレス");
      fireEvent.change(address, { target: { value: page } });
      fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
      fireEvent.click(await screen.findByRole("button", { name: "フォローユーザーを読み取る" }));
      await waitFor(() => expect(show).toHaveBeenCalledWith(expect.objectContaining({
        color: "yellow",
        message: "件数の上限で打ち切ったページがあります。",
      })));
    } finally {
      show.mockRestore();
    }
  });

  it("フォロー一覧の再読込で作者を並べ直し、選択を引き継いで監視状態を更新する", async () => {
    const page = "https://www.pixiv.net/users/7/following";
    const user = (userId: string, watched = false) => ({ userId, name: `作者${userId}`, watched });
    browserApi.readBrowserPageUserCandidates.mockReset()
      .mockResolvedValueOnce({ url: page, truncated: false, users: [user("1", true), user("2"), user("3")] })
      .mockResolvedValueOnce({ url: page, truncated: false, users: [user("4"), user("3"), user("2", true), user("1")] });
    renderSavePage();
    const address = await screen.findByLabelText("ブラウザのアドレス");
    fireEvent.change(address, { target: { value: page } });
    fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
    const read = await screen.findByRole("button", { name: "フォローユーザーを読み取る" });
    fireEvent.click(read);
    fireEvent.click(await screen.findByRole("checkbox", { name: "作者3を監視に追加する" }));
    fireEvent.click(read);
    await screen.findByRole("checkbox", { name: "作者4を監視に追加する" });

    const checkboxes = screen.getAllByRole("checkbox", { name: /を監視に追加する$/ });
    expect(checkboxes.map((box) => box.getAttribute("aria-label"))).toEqual(["作者4を監視に追加する", "作者3を監視に追加する", "作者2を監視に追加する", "作者1を監視に追加する"]);
    expect(checkboxes[1]).not.toBeChecked();
    expect(checkboxes[2]).toBeDisabled();
    expect(checkboxes[3]).toBeEnabled();
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

  it("取得元の切替後に古い読み取りが完了しても候補や新しい実行を変更しない", async () => {
    type Page = Awaited<ReturnType<typeof import("@/services/browserApi").readBrowserPageWorkCandidates>>;
    let finishOld!: (page: Page) => void;
    let finishNew!: (page: Page) => void;
    const oldRead = new Promise<Page>((resolve) => { finishOld = resolve; });
    const newRead = new Promise<Page>((resolve) => { finishNew = resolve; });
    browserApi.readBrowserPageWorkCandidates.mockReset().mockImplementation((_browser, source) => source === "pixiv" ? oldRead : newRead);
    renderSavePage();
    const open = async (url: string) => {
      const address = await screen.findByLabelText("ブラウザのアドレス");
      fireEvent.change(address, { target: { value: url } });
      fireEvent.click(screen.getByRole("button", { name: "URLを開く" }));
      fireEvent.click(await screen.findByRole("button", { name: "このページから読み取る" }));
    };
    const oldUrl = "https://www.pixiv.net/novel/bookmark_new.php";
    await open(oldUrl);
    await waitFor(() => expect(browserApi.readBrowserPageWorkCandidates).toHaveBeenCalledWith("embedded", "pixiv"));
    fireEvent.click(screen.getByRole("radio", { name: /FANBOX/ }));
    await waitFor(() => expect(window.location.hash).toContain("/save/fanbox"));
    const newUrl = "https://www.fanbox.cc/";
    await open(newUrl);
    await waitFor(() => expect(browserApi.readBrowserPageWorkCandidates).toHaveBeenCalledWith("embedded", "fanbox"));
    await act(async () => finishOld({ url: oldUrl, truncated: false, works: [{ sourceId: "old", title: "古い実行の作品", author: "", saved: false }] }));
    expect(readSaveDraft("pixiv").items).toEqual([]);
    expect(screen.queryByText("古い実行の作品")).toBeNull();
    expect(screen.getByRole("button", { name: "このページから読み取る" })).toBeDisabled();
    await act(async () => finishNew({ url: newUrl, truncated: false, works: [{ sourceId: "new", title: "新しい実行の作品", author: "", saved: false }] }));
    expect(await screen.findByText("新しい実行の作品")).toBeInTheDocument();
    expect(readSaveDraft("fanbox").items.map((item) => item.id)).toEqual(["new"]);
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
