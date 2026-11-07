import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { PropsWithChildren } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { PcWork } from "./pcApi";
import { forgetFavoriteChanges, useFavorite, useFavoriteNotice } from "./favorites";

const api = vi.hoisted(() => ({ setPcFavorite: vi.fn() }));
vi.mock("./pcApi", () => ({ setPcFavorite: api.setPcFavorite, pcErrorMessage: () => "前のサーバーのエラー" }));
vi.mock("./appHistory", () => ({ useAppNavigation: () => ({ canWriteFavorite: true }) }));

const work = { source: "pixiv", sourceId: "1", favorite: false } as PcWork;
const clients: QueryClient[] = [];
function wrapper({ children }: PropsWithChildren) {
  const client = new QueryClient();
  clients.push(client);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
beforeEach(() => { api.setPcFavorite.mockReset(); forgetFavoriteChanges(); });
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); });

it("サーバーを変えたら、同じ作品にも前のサーバーのお気に入りを出さない", async () => {
  api.setPcFavorite.mockResolvedValue({ favorite: true });
  const view = renderHook(() => useFavorite(work), { wrapper });
  await act(async () => { view.result.current.onToggle?.(true); });
  expect(view.result.current.favorite).toBe(true);
  act(() => { forgetFavoriteChanges(); });
  expect(view.result.current.favorite).toBe(false);
});

it.each([false, true])("接続変更後に届いた古い応答を捨てる（失敗=%s）", async (reject) => {
  let finish!: (value: unknown) => void;
  api.setPcFavorite.mockImplementationOnce(() => new Promise((resolve, fail) => { finish = reject ? fail : resolve; }));
  const view = renderHook(() => ({ ...useFavorite(work), notice: useFavoriteNotice() }), { wrapper });
  act(() => { view.result.current.onToggle?.(true); });
  expect(view.result.current.busy).toBe(true);
  act(() => { forgetFavoriteChanges(); });
  await act(async () => { finish(reject ? new Error("古い失敗") : { favorite: true }); });
  expect(view.result.current).toMatchObject({ favorite: false, busy: false, notice: null });
});
