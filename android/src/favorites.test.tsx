import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { PropsWithChildren } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { PcWork } from "./pcApi";
import { forgetFavoriteChanges, useFavorite, useFavoriteNotice } from "./favorites";
import { markFavoriteRead, nextFavoriteRevision } from "./favoriteFreshness";

const api = vi.hoisted(() => ({ setShelfFavorite: vi.fn(), shelf: { libraryId: "shelf" } as object | null }));
vi.mock("./pcApi", () => ({ pcErrorMessage: () => "前のサーバーのエラー" }));
vi.mock("./shelf", () => ({ connectedShelf: () => api.shelf, setShelfFavorite: api.setShelfFavorite }));
vi.mock("./appHistory", () => ({ useAppNavigation: () => ({ canBrowseServer: true }) }));

const work = {
  id: "0197a000-0000-7000-8000-000000000009", source: "pixiv", sourceId: "1", favorite: false,
  shelf: { snapshotId: "0197a000-0000-7000-8000-000000000008", edited: false, editId: null, favoriteGeneration: "0" },
} as PcWork;
const clients: QueryClient[] = [];
function wrapper({ children }: PropsWithChildren) {
  const client = new QueryClient();
  clients.push(client);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
beforeEach(() => { api.setShelfFavorite.mockReset(); api.shelf = { libraryId: "shelf" }; forgetFavoriteChanges(); });
afterEach(() => { cleanup(); clients.splice(0).forEach((client) => client.clear()); });

it("サーバーを変えたら、同じ作品にも前のサーバーのお気に入りを出さない", async () => {
  api.setShelfFavorite.mockResolvedValue({ favorite: true, generation: "1" });
  const view = renderHook(() => useFavorite(work), { wrapper });
  await act(async () => { view.result.current.onToggle?.(true); });
  expect(view.result.current.favorite).toBe(true);
  act(() => { forgetFavoriteChanges(); });
  expect(view.result.current.favorite).toBe(false);
});

it.each([false, true])("接続変更後に届いた古い応答を捨てる（失敗=%s）", async (reject) => {
  let finish!: (value: unknown) => void;
  api.setShelfFavorite.mockImplementationOnce(() => new Promise((resolve, fail) => { finish = reject ? fail : resolve; }));
  const view = renderHook(() => ({ ...useFavorite(work), notice: useFavoriteNotice() }), { wrapper });
  act(() => { view.result.current.onToggle?.(true); });
  expect(view.result.current.busy).toBe(true);
  act(() => { forgetFavoriteChanges(); });
  await act(async () => { finish(reject ? new Error("古い失敗") : { favorite: true, generation: "1" }); });
  expect(view.result.current).toMatchObject({ favorite: false, busy: false, notice: null });
});

it("書き込み後の新しいサーバー値を採用し、書き込み前に始めた応答では戻さない", async () => {
  const client = new QueryClient();
  clients.push(client);
  const provider = ({ children }: PropsWithChildren) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const readBeforeWrite = nextFavoriteRevision();
  api.setShelfFavorite.mockResolvedValue({ favorite: true, generation: "1" });
  client.setQueryData(["works", { favorite: true }], { pages: [] });
  client.setQueryData(["works-page", { favorite: true }, null, 1], { items: [] });
  client.setQueryData(["collection", "one"], { members: [] });
  client.setQueryData(["metadata", 1], { work: markFavoriteRead({ ...work }, readBeforeWrite) });
  const view = renderHook(({ current }: { current: PcWork }) => useFavorite(current), { wrapper: provider, initialProps: { current: work } });
  await act(async () => { view.result.current.onToggle?.(true); });
  expect(view.result.current.favorite).toBe(true);
  view.rerender({ current: markFavoriteRead({ ...work, favorite: false }, readBeforeWrite) });
  expect(view.result.current.favorite).toBe(true);
  client.setQueryData(["metadata", 1], { work: markFavoriteRead({ ...work, favorite: false }, nextFavoriteRevision()) });
  view.rerender({ current: client.getQueryData<{ work: PcWork }>(["metadata", 1])!.work });
  expect(view.result.current.favorite).toBe(false);
  for (const name of ["works", "works-page", "collection"]) {
    expect(client.getQueryCache().findAll({ queryKey: [name] }).every((query) => query.state.isInvalidated)).toBe(true);
  }
});

const shelfWork = {
  id: "0197a000-0000-7000-8000-000000000001", source: "pixiv", sourceId: "2", favorite: false,
  shelf: { snapshotId: "0197a000-0000-7000-8000-000000000002", edited: false, editId: null, favoriteGeneration: "4" },
} as PcWork;

it("知っている世代を添えて付け外しし、次は返った世代を使う", async () => {
  api.setShelfFavorite.mockResolvedValueOnce({ favorite: true, generation: "5", conflict: false });
  api.setShelfFavorite.mockResolvedValueOnce({ favorite: false, generation: "6", conflict: false });
  const view = renderHook(() => useFavorite(shelfWork), { wrapper });
  await act(async () => { view.result.current.onToggle?.(true); });
  expect(api.setShelfFavorite).toHaveBeenLastCalledWith(api.shelf, shelfWork.id, "4", true);
  expect(view.result.current.favorite).toBe(true);
  await act(async () => { view.result.current.onToggle?.(false); });
  expect(api.setShelfFavorite).toHaveBeenLastCalledWith(api.shelf, shelfWork.id, "5", false);
  expect(view.result.current.favorite).toBe(false);
});

it("先に変わっていたら今の値に合わせて知らせ、棚に届かない接続では押せない", async () => {
  api.setShelfFavorite.mockResolvedValueOnce({ favorite: false, generation: "7", conflict: true });
  const view = renderHook(() => ({ ...useFavorite(shelfWork), notice: useFavoriteNotice() }), { wrapper });
  await act(async () => { view.result.current.onToggle?.(true); });
  expect(view.result.current).toMatchObject({ favorite: false, busy: false });
  expect(view.result.current.notice).toContain("今の値に合わせました");
  api.setShelfFavorite.mockResolvedValueOnce({ favorite: true, generation: "8", conflict: false });
  await act(async () => { view.result.current.onToggle?.(true); });
  expect(api.setShelfFavorite).toHaveBeenLastCalledWith(api.shelf, shelfWork.id, "7", true);

  api.shelf = null;
  const readOnly = renderHook(() => useFavorite({ ...shelfWork, sourceId: "3" }), { wrapper });
  expect(readOnly.result.current.onToggle).toBeUndefined();
});
