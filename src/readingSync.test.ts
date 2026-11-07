import { beforeEach, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke, isTauri: () => true }));

beforeEach(() => {
  vi.resetModules();
  window.localStorage.clear();
  api.invoke.mockReset();
});

it("同期中に届いた送信要求をまとめ、同期は常に一つだけ動かす", async () => {
  const finish: (() => void)[] = [];
  api.invoke.mockImplementation(() => new Promise<void>((resolve) => { finish.push(resolve); }));
  const { refreshReading, sendReading } = await import("./readingSync");
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
  const { refreshReading, sendReading, setReadingSyncEnabled } = await import("./readingSync");
  const first = refreshReading(0);
  const queued = sendReading();
  setReadingSyncEnabled(false);
  finish();
  await Promise.all([first, queued]);
  expect(api.invoke).toHaveBeenCalledTimes(1);
});
