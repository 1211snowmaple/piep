import { createContext, forwardRef, useCallback, useContext, useEffect, useMemo, useState, type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from "react";

interface RouterValue {
  pathname: string;
  search: string;
  searchParams: URLSearchParams;
  navigate: (target: string | number, options?: { replace?: boolean }) => void;
}

const RouterContext = createContext<RouterValue | null>(null);

function readLocation() {
  const raw = window.location.hash.replace(/^#/, "") || "/";
  const url = new URL(raw, "https://piep.local");
  return { pathname: url.pathname || "/", search: url.search };
}

export function AppRouter({ children }: { children: ReactNode }) {
  const [location, setLocation] = useState(readLocation);
  useEffect(() => {
    const update = () => setLocation(readLocation());
    window.addEventListener("hashchange", update);
    window.addEventListener("popstate", update);
    if (!window.location.hash) window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#/`);
    return () => { window.removeEventListener("hashchange", update); window.removeEventListener("popstate", update); };
  }, []);
  const navigate = useCallback((target: string | number, options?: { replace?: boolean }) => {
    if (typeof target === "number") { window.history.go(target); return; }
    const next = target.startsWith("/") ? target : `/${target}`;
    if (options?.replace) {
      window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}#${next}`);
      setLocation(readLocation());
    } else if (`#${next}` !== window.location.hash) {
      window.location.hash = next;
    }
  }, []);
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
      if (value !== undefined) { params[name] = decodeURIComponent(value); pathIndex += 1; }
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
