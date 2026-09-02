import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { lazy, Suspense, type ReactNode } from "react";
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

  it("keeps the painted route visible while the next route bundle loads", async () => {
    window.location.hash = "#/";
    let reveal!: () => void;
    const LazyPage = lazy(() => new Promise<{ default: () => ReactNode }>((resolve) => {
      reveal = () => resolve({ default: () => <div>next-screen</div> });
    }));
    function RouteProbe() {
      const { pathname, navigate } = useAppRouter();
      return (
        <>
          <button type="button" onClick={() => navigate("/next")}>next</button>
          {pathname === "/" ? <div>current-screen</div> : <LazyPage />}
        </>
      );
    }
    render(
      <AppRouter>
        <Suspense fallback={<div>route-fallback</div>}><RouteProbe /></Suspense>
      </AppRouter>,
    );

    fireEvent.click(screen.getByRole("button", { name: "next" }));
    expect(screen.getByText("current-screen")).toBeInTheDocument();
    expect(screen.queryByText("route-fallback")).toBeNull();
    await act(async () => reveal());
    expect(await screen.findByText("next-screen")).toBeInTheDocument();
  });

  it("rolls a browser back navigation forward again when guarded work is kept", async () => {
    window.location.hash = "#/library";
    window.history.replaceState(null, "", window.location.href);
    const confirmNavigation = vi.fn().mockResolvedValue(false);
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter confirmNavigation={confirmNavigation}>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    act(() => result.current.navigate("/editor/101"));
    await waitFor(() => expect(result.current.pathname).toBe("/editor/101"));
    const unregister = registerUnsavedGuard(() => true);

    act(() => window.history.back());

    await waitFor(() => expect(confirmNavigation).toHaveBeenCalledOnce());
    await waitFor(() => expect(window.location.hash).toBe("#/editor/101"));
    expect(result.current.pathname).toBe("/editor/101");

    // Canceling must not overwrite the physical entry we tried to visit.
    // Once the guard is gone, Back reaches that same library entry normally.
    unregister();
    act(() => window.history.back());
    await waitFor(() => expect(window.location.hash).toBe("#/library"));
  });

  it("replays a confirmed browser back without reversing the physical stack", async () => {
    window.location.hash = "#/library";
    window.history.replaceState(null, "", window.location.href);
    const confirmNavigation = vi.fn().mockResolvedValue(true);
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter confirmNavigation={confirmNavigation}>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    act(() => result.current.navigate("/editor/101"));
    await waitFor(() => expect(result.current.pathname).toBe("/editor/101"));
    const unregister = registerUnsavedGuard(() => true);

    act(() => window.history.back());
    await waitFor(() => expect(confirmNavigation).toHaveBeenCalledOnce());
    await waitFor(() => expect(result.current.pathname).toBe("/library"));

    // The editor entry remains in front of the library entry after confirming.
    unregister();
    act(() => window.history.forward());
    await waitFor(() => expect(result.current.pathname).toBe("/editor/101"));
  });

  it("does not confirm a numeric navigation twice after it was authorized", async () => {
    window.location.hash = "#/library";
    window.history.replaceState(null, "", window.location.href);
    const confirmNavigation = vi.fn().mockResolvedValue(true);
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter confirmNavigation={confirmNavigation}>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    act(() => result.current.navigate("/editor/101"));
    await waitFor(() => expect(result.current.pathname).toBe("/editor/101"));
    const unregister = registerUnsavedGuard(() => true);

    const go = vi.spyOn(window.history, "go").mockImplementation((delta) => {
      if (delta !== -1) return;
      const backHref = window.location.href.replace("#/editor/101", "#/library");
      window.history.replaceState({ piepHistoryIndex: 0 }, "", backHref);
      window.dispatchEvent(new PopStateEvent("popstate", { state: { piepHistoryIndex: 0 } }));
    });
    act(() => result.current.navigate(-1));

    await waitFor(() => expect(result.current.pathname).toBe("/library"));
    expect(confirmNavigation).toHaveBeenCalledOnce();
    go.mockRestore();
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

describe("history position", () => {
  function Probe() {
    const { canGoBack, canGoForward, historyIndex, previousEntry } = useAppRouter();
    const navigate = useAppNavigate();
    return (
      <div>
        <span data-testid="index">{historyIndex}</span>
        <span data-testid="back">{String(canGoBack)}</span>
        <span data-testid="forward">{String(canGoForward)}</span>
        <span data-testid="previous">{previousEntry ?? "none"}</span>
        <button type="button" onClick={() => navigate("/works/7")}>work</button>
        <button type="button" onClick={() => navigate("/reader/7")}>reader</button>
      </div>
    );
  }

  it("knows whether the history controls have anywhere to go", async () => {
    window.location.hash = "#/library";
    window.history.replaceState(null, "", window.location.href);
    render(<AppRouter><Probe /></AppRouter>);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });

    // Nothing has been visited yet, so both controls would do nothing - and a
    // desktop window has no browser chrome to make that obvious.
    expect(screen.getByTestId("back")).toHaveTextContent("false");
    expect(screen.getByTestId("forward")).toHaveTextContent("false");

    fireEvent.click(screen.getByRole("button", { name: "work" }));
    await waitFor(() => expect(screen.getByTestId("index")).toHaveTextContent("1"));
    expect(screen.getByTestId("back")).toHaveTextContent("true");
    // Pushing discards whatever was ahead.
    expect(screen.getByTestId("forward")).toHaveTextContent("false");
  });

  it("remembers what the previous entry was showing", async () => {
    window.location.hash = "#/library?q=%E5%89%B5%E4%BD%9C";
    window.history.replaceState(null, "", window.location.href);
    render(<AppRouter><Probe /></AppRouter>);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });

    fireEvent.click(screen.getByRole("button", { name: "work" }));
    await waitFor(() => expect(screen.getByTestId("previous")).toHaveTextContent("/library?q=%E5%89%B5%E4%BD%9C"));

    // So that a "back to the work" control can go back to the copy that still
    // has the reader's tab and scroll position, rather than pushing a new one.
    fireEvent.click(screen.getByRole("button", { name: "reader" }));
    await waitFor(() => expect(screen.getByTestId("previous")).toHaveTextContent("/works/7"));
  });
});
