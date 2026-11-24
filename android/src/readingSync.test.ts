import { beforeEach, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke, isTauri: () => true }));

const library = "0190a000-0000-7000-8000-000000000001";

beforeEach(() => {
  vi.resetModules();
  window.localStorage.clear();
  api.invoke.mockReset();
});

/** 同期は棚と受け渡すので、棚に届いている接続にしてから読み込む。 */
async function adoptEpoch(epoch = "0190a000-0000-7000-8000-000000000005") {
  const shelf = await import("./shelf");
  shelf.adoptShelf({
    url: "https://server", reachable: true,
    shelf: {
      scope: { libraryId: library, subjectId: "0190a000-0000-7000-8000-000000000004", syncEpoch: epoch, connectionGeneration: 1 },
      readerBundle: true,
    },
  } as never);
}

async function loadSync() {
  await adoptEpoch();
  return import("./readingSync");
}

it("前の同期世代の実行中に新しい世代を確認したら、その世代でも同期する", async () => {
  let finish!: () => void;
  api.invoke.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
  api.invoke.mockResolvedValue(undefined);
  const { refreshReading } = await loadSync();
  const first = refreshReading();
  const epoch = "0190a000-0000-7000-8000-000000000007";
  await adoptEpoch(epoch);
  const next = refreshReading();
  finish();
  await Promise.all([first, next]);
  expect(api.invoke).toHaveBeenCalledTimes(2);
  expect(api.invoke).toHaveBeenLastCalledWith("pc_reading_sync", { scope: expect.objectContaining({ syncEpoch: epoch }) });
});

it("同期世代が変わったら、直前に同期が終わっていても取り直す", async () => {
  api.invoke.mockResolvedValue(undefined);
  const { refreshReading } = await loadSync();
  await refreshReading();
  await refreshReading();
  expect(api.invoke).toHaveBeenCalledTimes(1);
  await adoptEpoch("0190a000-0000-7000-8000-000000000007");
  await refreshReading();
  expect(api.invoke).toHaveBeenCalledTimes(2);
});

it("同期中に届いた送信要求をまとめ、同期は常に一つだけ動かす", async () => {
  const finish: (() => void)[] = [];
  api.invoke.mockImplementation(() => new Promise<void>((resolve) => { finish.push(resolve); }));
  const { refreshReading, sendReading } = await loadSync();
  const first = refreshReading(0);
  const requests = [sendReading(), sendReading(), sendReading()];
  expect(api.invoke).toHaveBeenCalledTimes(1);
  finish[0]();
  await vi.waitFor(() => expect(api.invoke).toHaveBeenCalledTimes(2));
  // 次の同期も、途中の送信要求がいくつあっても一回にまとめる。
  const next = [sendReading(), sendReading()];
  expect(api.invoke).toHaveBeenCalledTimes(2);
  finish[1]();
  await vi.waitFor(() => expect(api.invoke).toHaveBeenCalledTimes(3));
  finish[2]();
  await Promise.all([first, ...requests, ...next]);
  expect(api.invoke).toHaveBeenCalledTimes(3);
});

it("同期を切ったら、待っている追加送信を実行しない", async () => {
  let finish!: () => void;
  api.invoke.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
  const { refreshReading, sendReading, setReadingSyncEnabled } = await loadSync();
  const first = refreshReading(0);
  const queued = sendReading();
  setReadingSyncEnabled(false);
  finish();
  await Promise.all([first, queued]);
  expect(api.invoke).toHaveBeenCalledTimes(1);
});
