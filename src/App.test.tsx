import type { ReactNode } from "react";
import { MantineProvider } from "@mantine/core";
import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";

const nativeWindow = vi.hoisted(() => ({ onCloseRequested: vi.fn() }));

vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => nativeWindow }));
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
vi.mock("@/app/AppFrame", () => ({ AppFrame: ({ children }: { children: ReactNode }) => children }));
vi.mock("@/app/WorkspaceContext", () => ({ WorkspaceProvider: ({ children }: { children: ReactNode }) => children }));
vi.mock("@/features/dashboard/DashboardPage", () => ({ default: () => null }));

function pendingRegistration() {
  let resolve!: (unlisten: () => void) => void;
  const promise = new Promise<() => void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("native close guard lifecycle", () => {
  beforeEach(() => {
    window.location.hash = "#/";
    nativeWindow.onCloseRequested.mockReset();
  });

  it("removes a listener that finishes registering after the app unmounts", async () => {
    const registration = pendingRegistration();
    const unlisten = vi.fn();
    nativeWindow.onCloseRequested.mockReturnValue(registration.promise);
    const view = render(<MantineProvider><App /></MantineProvider>);
    await waitFor(() => expect(nativeWindow.onCloseRequested).toHaveBeenCalledOnce());

    view.unmount();
    await act(async () => registration.resolve(unlisten));

    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("keeps only the current listener when the app remounts", async () => {
    const first = pendingRegistration();
    const second = pendingRegistration();
    const unlistenFirst = vi.fn();
    const unlistenSecond = vi.fn();
    nativeWindow.onCloseRequested.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const previous = render(<MantineProvider><App /></MantineProvider>);
    await waitFor(() => expect(nativeWindow.onCloseRequested).toHaveBeenCalledOnce());
    previous.unmount();
    const view = render(<MantineProvider><App /></MantineProvider>);
    await waitFor(() => expect(nativeWindow.onCloseRequested).toHaveBeenCalledTimes(2));

    // The active registration may finish before the abandoned one.
    await act(async () => second.resolve(unlistenSecond));
    await act(async () => first.resolve(unlistenFirst));
    expect(unlistenFirst).toHaveBeenCalledOnce();
    expect(unlistenSecond).not.toHaveBeenCalled();

    view.unmount();
    expect(unlistenFirst).toHaveBeenCalledOnce();
    expect(unlistenSecond).toHaveBeenCalledOnce();
  });
});
