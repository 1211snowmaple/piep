import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { AppRouter, matchPath, useAppNavigate, useAppRouter } from "@/app/router";
import { registerUnsavedGuard } from "@/lib/unsavedGuard";

describe("matchPath", () => {
  it("extracts and decodes required parameters", () => {
    expect(matchPath("/people/:source/:sourceKey", "/people/pixiv/%E9%9D%92%E8%91%89")).toEqual({ source: "pixiv", sourceKey: "青葉" });
  });

  it("supports an optional trailing parameter", () => {
    expect(matchPath("/save/:source?", "/save")).toEqual({});
    expect(matchPath("/save/:source?", "/save/fanbox")).toEqual({ source: "fanbox" });
  });

  it("rejects partial and extra paths", () => {
    expect(matchPath("/works/:workId", "/works")).toBeNull();
    expect(matchPath("/works/:workId", "/works/1/edit")).toBeNull();
  });

  it("rejects malformed encoded parameters without throwing", () => {
    expect(matchPath("/people/:source/:sourceKey", "/people/pixiv/%E0%A4%A")).toBeNull();
  });
});

describe("AppRouter", () => {
  it("navigates with a hash and keeps query parameters", () => {
    window.location.hash = "#/";
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    act(() => result.current.navigate("/library?q=%E5%89%B5%E4%BD%9C"));
    expect(window.location.hash).toBe("#/library?q=%E5%89%B5%E4%BD%9C");
  });

  it("does not leave guarded work until the user confirms", async () => {
    window.location.hash = "#/editor/101";
    const confirmNavigation = vi.fn().mockResolvedValue(false);
    const unregister = registerUnsavedGuard(() => true);
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter confirmNavigation={confirmNavigation}>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    await act(async () => { result.current.navigate("/library"); });
    expect(confirmNavigation).toHaveBeenCalledOnce();
    expect(window.location.hash).toBe("#/editor/101");
    unregister();
  });

  it("continues guarded navigation after confirmation", async () => {
    window.location.hash = "#/editor/101";
    const unregister = registerUnsavedGuard(() => true);
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter confirmNavigation={() => Promise.resolve(true)}>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    await act(async () => { result.current.navigate("/library"); });
    expect(window.location.hash).toBe("#/library");
    unregister();
  });
});

describe("navigation type", () => {
  function Probe() {
    const { navigationType, pathname } = useAppRouter();
    const navigate = useAppNavigate();
    return (
      <div>
        <span data-testid="type">{navigationType}</span>
        <span data-testid="path">{pathname}</span>
        <button type="button" onClick={() => navigate("/library")}>push</button>
        <button type="button" onClick={() => navigate("/library?tab=people", { replace: true })}>replace</button>
      </div>
    );
  }

  it("tells a new destination from a rewritten query string and from going back", async () => {
    window.location.hash = "#/";
    render(<AppRouter><Probe /></AppRouter>);
    // jsdom delivers a hashchange for the assignment above after mount; let it
    // land before measuring, or it lands in the middle of the first click.
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });

    // Scroll handling depends on this: only a push starts at the top, only a
    // pop restores, and a replace must not move the page at all.
    fireEvent.click(screen.getByRole("button", { name: "push" }));
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent("/library"));
    expect(screen.getByTestId("type")).toHaveTextContent("push");

    fireEvent.click(screen.getByRole("button", { name: "replace" }));
    await waitFor(() => expect(screen.getByTestId("type")).toHaveTextContent("replace"));

    // A hash change the app did not make is the user's back or forward button.
    window.location.hash = "#/";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await waitFor(() => expect(screen.getByTestId("type")).toHaveTextContent("pop"));
  });
});
