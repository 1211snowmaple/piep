import { MantineProvider } from "@mantine/core";
import { act, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { adminApi, ApiFailure } from "./api";
import { ReauthProvider, useReauth } from "./shared";

afterEach(() => vi.restoreAllMocks());

it.each([
  ["確認", true],
  ["キャンセル", false],
] as const)("再認証の「%s」で待っていた両方の操作に結果を返す", async (outcome, passed) => {
  const required = new ApiFailure(403, "reauth_required", "再認証が必要です");
  const first = vi.fn().mockRejectedValueOnce(required).mockResolvedValue("一つ目");
  const second = vi.fn().mockRejectedValueOnce(required).mockResolvedValue("二つ目");
  const firstResult = vi.fn();
  const secondResult = vi.fn();
  const reauth = vi.spyOn(adminApi, "reauth").mockResolvedValue(undefined);
  const user = userEvent.setup();
  const { result } = renderHook(useReauth, {
    wrapper: ({ children }: { children: ReactNode }) => (
      <MantineProvider><ReauthProvider>{children}</ReauthProvider></MantineProvider>
    ),
  });

  await act(async () => { void result.current.confirmed(first).then(firstResult); });
  const password = await screen.findByLabelText("パスワード");
  await user.type(password, "試験用パスワード");
  await act(async () => { void result.current.confirmed(second).then(secondResult); });
  await user.click(screen.getByRole("button", { name: outcome }));

  await waitFor(() => {
    expect(firstResult).toHaveBeenCalledExactlyOnceWith(passed ? "一つ目" : undefined);
    expect(secondResult).toHaveBeenCalledExactlyOnceWith(passed ? "二つ目" : undefined);
  });
  expect(reauth).toHaveBeenCalledTimes(passed ? 1 : 0);
  expect(first).toHaveBeenCalledTimes(passed ? 2 : 1);
  expect(second).toHaveBeenCalledTimes(passed ? 2 : 1);
});
