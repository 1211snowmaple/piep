import { createContext, forwardRef, useCallback, useContext, useEffect, useMemo, useRef, useState, type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from "react";
import { hasUnsavedWork } from "@/lib/unsavedGuard";

/**
 * How the current location was arrived at.
 *
 * Scroll handling depends on it: a new destination starts at the top, going
 * back returns you to where you were, and rewriting the query string - a tab,
 * a filter, a sort - must not move the page at all.
 */
export type NavigationType = "push" | "replace" | "pop";

interface RouterValue {
  pathname: string;
  search: string;
  searchParams: URLSearchParams;
  navigationType: NavigationType;
  /**
   * Where this entry sits in the session's history.
   *
   * Scroll positions hang off it rather than off the address: two visits to the
   * same listing are two places the reader was standing, and a query string
   * rewritten in place is still the one place they are standing now.
   */
  historyIndex: number;
  /** Whether the header's history controls have anywhere to go. */
  canGoBack: boolean;
  canGoForward: boolean;
  /** The address one step back, when this session is what put it there. */
  previousEntry: string | null;
  navigate: (target: string | number, options?: { replace?: boolean }) => void;
}

const RouterContext = createContext<RouterValue | null>(null);

/**
 * Stamped onto every entry this session creates.
 *
 * `history` can say how many entries exist but not which one is showing, and a
 * desktop window has no browser chrome to fall back on - so without a counter
 * of our own the forward button is permanently lit whether or not it does
 * anything, and every entry for one address shares one scroll position.
 */
const HISTORY_INDEX_KEY = "piepHistoryIndex";

function readLocation() {
  const raw = window.location.hash.replace(/^#/, "") || "/";
  const url = new URL(raw, "https://piep.local");
  return { pathname: url.pathname || "/", search: url.search };
}

function readHistoryIndex(): number | null {
  const state = window.history.state as Record<string, unknown> | null;
  const value = state?.[HISTORY_INDEX_KEY];
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function stampHistoryIndex(index: number, href = window.location.href) {
  window.history.replaceState({ ...window.history.state, [HISTORY_INDEX_KEY]: index }, "", href);
}

interface AppRouterProps {
  children: ReactNode;
  confirmNavigation?: () => Promise<boolean>;
}

export function AppRouter({ children, confirmNavigation }: AppRouterProps) {
  const [location, setLocation] = useState(() => ({
    ...readLocation(),
    navigationType: "push" as NavigationType,
    index: readHistoryIndex() ?? 0,
  }));
  const confirmationPending = useRef(false);
  const confirmNavigationRef = useRef(confirmNavigation);
  confirmNavigationRef.current = confirmNavigation;
  const authorizedPopIndexRef = useRef<number | null>(null);
  const pendingPopRef = useRef<{
    fromIndex: number;
    toIndex: number;
    rollbackComplete: boolean;
    decision: boolean | null;
  } | null>(null);
  const indexRef = useRef(location.index);
  /** The furthest entry reached, which is where "forward" runs out. */
  const furthestRef = useRef(location.index);
  /** What each entry is showing, so a "back to X" control can tell whether back is X. */
  const entriesRef = useRef(new Map<number, string>([[location.index, `${location.pathname}${location.search}`]]));
  // One history step can raise both hashchange and popstate. Without this the
  // second delivery re-runs the navigation and restores the wrong position.
  const lastHandledHref = useRef<string | null>(null);
  useEffect(() => {
    if (!window.location.hash) {
      window.history.replaceState({ [HISTORY_INDEX_KEY]: indexRef.current }, "", `${window.location.pathname}${window.location.search}#/`);
    } else if (readHistoryIndex() === null) {
      stampHistoryIndex(indexRef.current);
    }
    lastHandledHref.current = window.location.href;
    // pushState raises neither event, so everything arriving here is the user's
    // own back or forward - or an address from outside the app, which lands
    // after the current entry and discards whatever was ahead of it.
    const applyPop = (index: number) => {
      const next = readLocation();
      indexRef.current = index;
      furthestRef.current = Math.max(furthestRef.current, index);
      entriesRef.current.set(index, `${next.pathname}${next.search}`);
      setLocation({ ...next, navigationType: "pop", index });
    };
    const finishPendingPop = () => {
      const pending = pendingPopRef.current;
      if (!pending?.rollbackComplete || pending.decision === null) return;
      pendingPopRef.current = null;
      confirmationPending.current = false;
      if (!pending.decision) return;
      // The rejected traversal has first been undone with a real history move.
      // Confirming now replays that same move, preserving both physical entries
      // and their state instead of rewriting either URL in place.
      authorizedPopIndexRef.current = pending.toIndex;
      window.history.go(pending.toIndex - pending.fromIndex);
    };
    const update = () => {
      const href = window.location.href;
      let index = readHistoryIndex();
      if (index === null) {
        index = indexRef.current + 1;
        stampHistoryIndex(index);
        furthestRef.current = index;
      }

      const pending = pendingPopRef.current;
      if (pending && !pending.rollbackComplete) {
        if (index === pending.fromIndex) {
          // The browser is physically back on the entry the app is still
          // rendering. `href` can equal lastHandledHref here, so this must run
          // before the duplicate hashchange/popstate check below.
          pending.rollbackComplete = true;
          lastHandledHref.current = href;
          finishPendingPop();
          return;
        }
        // One traversal can emit both hashchange and popstate. Ignore every
        // delivery from the rejected destination while the rollback is moving.
        if (index === pending.toIndex) return;
      }

      if (lastHandledHref.current === href) return;

      if (authorizedPopIndexRef.current === index) {
        authorizedPopIndexRef.current = null;
        lastHandledHref.current = href;
        applyPop(index);
        return;
      }

      const confirm = confirmNavigationRef.current;
      if (hasUnsavedWork("navigate") && confirm) {
        const fromIndex = indexRef.current;
        const fromHref = lastHandledHref.current;
        const rollback = fromIndex - index;
        if (fromHref && rollback !== 0 && !confirmationPending.current) {
          confirmationPending.current = true;
          const toIndex = index;
          // The browser has already moved. Put the current app route back
          // with a real traversal so neither the origin nor destination entry
          // is overwritten. React keeps rendering the origin until that move
          // and the user's decision have both completed.
          pendingPopRef.current = {
            fromIndex,
            toIndex,
            rollbackComplete: false,
            decision: null,
          };
          window.history.go(rollback);
          void confirm()
            .then((discard) => {
              const active = pendingPopRef.current;
              if (!active || active.fromIndex !== fromIndex || active.toIndex !== toIndex) return;
              active.decision = discard;
              finishPendingPop();
            })
            .catch(() => {
              const active = pendingPopRef.current;
              if (!active || active.fromIndex !== fromIndex || active.toIndex !== toIndex) return;
              active.decision = false;
              finishPendingPop();
            });
          return;
        }
      }

      lastHandledHref.current = href;
      applyPop(index);
    };
    window.addEventListener("hashchange", update);
    window.addEventListener("popstate", update);
    return () => { window.removeEventListener("hashchange", update); window.removeEventListener("popstate", update); };
  }, []);
  const commitNavigation = useCallback((target: string | number, options?: { replace?: boolean }) => {
    if (typeof target === "number") {
      authorizedPopIndexRef.current = indexRef.current + target;
      window.history.go(target);
      return;
    }
    const next = target.startsWith("/") ? target : `/${target}`;
    const href = `${window.location.pathname}${window.location.search}#${next}`;
    if (options?.replace) {
      window.history.replaceState({ [HISTORY_INDEX_KEY]: indexRef.current }, "", href);
      lastHandledHref.current = window.location.href;
      entriesRef.current.set(indexRef.current, next);
      setLocation({ ...readLocation(), navigationType: "replace", index: indexRef.current });
    } else if (`#${next}` !== window.location.hash) {
      // pushState rather than assigning the hash: an assigned entry carries no
      // state, so nothing distinguishes it from the one before it afterwards.
      const index = indexRef.current + 1;
      window.history.pushState({ [HISTORY_INDEX_KEY]: index }, "", href);
      indexRef.current = index;
      // Pushing discards whatever was ahead.
      furthestRef.current = index;
      for (const known of [...entriesRef.current.keys()]) if (known > index) entriesRef.current.delete(known);
      entriesRef.current.set(index, next);
      lastHandledHref.current = window.location.href;
      setLocation({ ...readLocation(), navigationType: "push", index });
    }
  }, []);
  const navigate = useCallback((target: string | number, options?: { replace?: boolean }) => {
    if (target === 0) return;
    if (typeof target === "string") {
      const next = target.startsWith("/") ? target : `/${target}`;
      if (`#${next}` === window.location.hash) return;
    }
    if (!hasUnsavedWork("navigate") || !confirmNavigation) {
      commitNavigation(target, options);
      return;
    }
    // Repeated clicks while the modal is opening used to stack confirmation
    // dialogs. Keep the first requested destination authoritative.
    if (confirmationPending.current) return;
    confirmationPending.current = true;
    void confirmNavigation()
      .then((discard) => { if (discard) commitNavigation(target, options); })
      .catch(() => undefined)
      .finally(() => { confirmationPending.current = false; });
  }, [commitNavigation, confirmNavigation]);
  const value = useMemo<RouterValue>(() => ({
    pathname: location.pathname,
    search: location.search,
    searchParams: new URLSearchParams(location.search),
    navigationType: location.navigationType,
    historyIndex: location.index,
    canGoBack: location.index > 0,
    canGoForward: location.index < furthestRef.current,
    previousEntry: entriesRef.current.get(location.index - 1) ?? null,
    navigate,
  }), [location, navigate]);
  return <RouterContext.Provider value={value}>{children}</RouterContext.Provider>;
}

export function useAppRouter() {
  const value = useContext(RouterContext);
  if (!value) throw new Error("useAppRouter must be used inside AppRouter");
  return value;
}

export function useAppNavigate() { return useAppRouter().navigate; }

/** The same screen, whatever query string it happened to be wearing. */
function sameScreen(a: string, b: string) {
  return a.split("?")[0] === b.split("?")[0];
}

/**
 * Returns to a screen, rather than opening a second copy of it.
 *
 * "作品詳細へ戻る" that pushes leaves the header's back button pointing at the
 * screen the reader has just closed, and the copy it opens has lost the tab,
 * the filter and the scroll position they left there. When the place a control
 * names is literally the previous entry, going back is what it means.
 */
export function useReturnTo() {
  const { canGoBack, navigate, previousEntry } = useAppRouter();
  return useCallback((to: string) => {
    if (canGoBack && previousEntry && sameScreen(previousEntry, to)) navigate(-1);
    else navigate(to);
  }, [canGoBack, navigate, previousEntry]);
}

export function useAppSearchParams(): [URLSearchParams, (params: URLSearchParams, options?: { replace?: boolean }) => void] {
  const router = useAppRouter();
  const setSearchParams = useCallback((params: URLSearchParams, options?: { replace?: boolean }) => {
    router.navigate(`${router.pathname}${params.size ? `?${params.toString()}` : ""}`, options);
  }, [router.navigate, router.pathname]);
  return [router.searchParams, setSearchParams];
}

export function matchPath(pattern: string, pathname: string): Record<string, string> | null {
  const patternParts = pattern.split("/").filter(Boolean);
  const pathParts = pathname.split("/").filter(Boolean);
  const params: Record<string, string> = {};
  let pathIndex = 0;
  for (const patternPart of patternParts) {
    const optional = patternPart.startsWith(":") && patternPart.endsWith("?");
    if (patternPart.startsWith(":")) {
      const name = patternPart.slice(1, optional ? -1 : undefined);
      const value = pathParts[pathIndex];
      if (value === undefined && !optional) return null;
      if (value !== undefined) {
        try {
          params[name] = decodeURIComponent(value);
        } catch {
          // A malformed external hash should render the not-found route, not
          // throw during the entire application render.
          return null;
        }
        pathIndex += 1;
      }
    } else {
      if (pathParts[pathIndex] !== patternPart) return null;
      pathIndex += 1;
    }
  }
  return pathIndex === pathParts.length ? params : null;
}

export function useRouteParams(pattern: string) {
  const { pathname } = useAppRouter();
  return matchPath(pattern, pathname) ?? {};
}

interface AppLinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> { to: string }

export const AppLink = forwardRef<HTMLAnchorElement, AppLinkProps>(({ to, onClick, ...others }, ref) => {
  const navigate = useAppNavigate();
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    navigate(to);
  };
  return <a ref={ref} href={`#${to}`} onClick={handleClick} {...others} />;
});

AppLink.displayName = "AppLink";
