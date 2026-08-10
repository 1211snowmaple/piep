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
  navigate: (target: string | number, options?: { replace?: boolean }) => void;
}

const RouterContext = createContext<RouterValue | null>(null);

function readLocation() {
  const raw = window.location.hash.replace(/^#/, "") || "/";
  const url = new URL(raw, "https://piep.local");
  return { pathname: url.pathname || "/", search: url.search };
}

interface AppRouterProps {
  children: ReactNode;
  confirmNavigation?: () => Promise<boolean>;
}

export function AppRouter({ children, confirmNavigation }: AppRouterProps) {
  const [location, setLocation] = useState(() => ({ ...readLocation(), navigationType: "push" as NavigationType }));
  const confirmationPending = useRef(false);
  // A hashchange fires for our own pushes too, so the kind of the navigation is
  // recorded as it is made; anything left over is the user's back or forward.
  const pendingType = useRef<NavigationType | null>(null);
  // One hash assignment can raise both hashchange and popstate. Without this
  // the second delivery finds no pending type and reclassifies the reader's own
  // click as a back navigation, which then restores the wrong scroll position.
  const lastHandledHref = useRef<string | null>(null);
  useEffect(() => {
    const update = () => {
      if (lastHandledHref.current === window.location.href) return;
      lastHandledHref.current = window.location.href;
      const type = pendingType.current ?? "pop";
      pendingType.current = null;
      setLocation({ ...readLocation(), navigationType: type });
    };
    window.addEventListener("hashchange", update);
    window.addEventListener("popstate", update);
    if (!window.location.hash) window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#/`);
    return () => { window.removeEventListener("hashchange", update); window.removeEventListener("popstate", update); };
  }, []);
  const commitNavigation = useCallback((target: string | number, options?: { replace?: boolean }) => {
    if (typeof target === "number") { window.history.go(target); return; }
    const next = target.startsWith("/") ? target : `/${target}`;
    if (options?.replace) {
      window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#${next}`);
      lastHandledHref.current = window.location.href;
      setLocation({ ...readLocation(), navigationType: "replace" });
    } else if (`#${next}` !== window.location.hash) {
      pendingType.current = "push";
      window.location.hash = next;
    }
  }, []);
  const navigate = useCallback((target: string | number, options?: { replace?: boolean }) => {
    if (target === 0) return;
    if (typeof target === "string") {
      const next = target.startsWith("/") ? target : `/${target}`;
      if (`#${next}` === window.location.hash) return;
    }
    if (!hasUnsavedWork() || !confirmNavigation) {
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
  const value = useMemo<RouterValue>(() => ({ ...location, searchParams: new URLSearchParams(location.search), navigate }), [location, navigate]);
  return <RouterContext.Provider value={value}>{children}</RouterContext.Provider>;
}

export function useAppRouter() {
  const value = useContext(RouterContext);
  if (!value) throw new Error("useAppRouter must be used inside AppRouter");
  return value;
}

export function useAppNavigate() { return useAppRouter().navigate; }

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
