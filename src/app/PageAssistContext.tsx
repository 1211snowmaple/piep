import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { AssistLauncherItem } from "@/features/assist/AssistLauncher";

export interface PageAssistRegistration {
  key: string;
  label: string;
  items: AssistLauncherItem[];
}

interface PageAssistContextValue {
  register: (registration: PageAssistRegistration) => void;
  unregister: (key: string) => void;
}

const PageAssistContext = createContext<PageAssistContextValue | null>(null);

/** Owns the one page-scoped AI entry rendered by AppFrame's header. */
export function PageAssistProvider({
  children,
  onChange,
}: {
  children: ReactNode;
  onChange: (registrations: PageAssistRegistration[]) => void;
}) {
  const [registrations, setRegistrations] = useState<Map<string, PageAssistRegistration>>(() => new Map());

  const register = useCallback((registration: PageAssistRegistration) => {
    setRegistrations((current) => {
      const next = new Map(current);
      next.set(registration.key, registration);
      return next;
    });
  }, []);
  const unregister = useCallback((key: string) => {
    setRegistrations((current) => {
      if (!current.has(key)) return current;
      const next = new Map(current);
      next.delete(key);
      return next;
    });
  }, []);

  const ordered = useMemo(() => [...registrations.values()], [registrations]);
  useLayoutEffect(() => onChange(ordered), [onChange, ordered]);
  const value = useMemo(() => ({ register, unregister }), [register, unregister]);
  return <PageAssistContext.Provider value={value}>{children}</PageAssistContext.Provider>;
}

/**
 * Registers page-wide actions without making every screen know AppFrame.
 * Callbacks stay live through a ref; the header only re-renders when visible
 * menu state changes, not whenever a page creates a new closure.
 */
export function usePageAssist(
  key: string,
  label: string,
  items: AssistLauncherItem[],
  active = true,
) {
  const context = useContext(PageAssistContext);
  const latestItems = useRef(items);
  latestItems.current = items;

  const visibleState = JSON.stringify(items.map((item) => ({
    id: item.id,
    label: item.label,
    description: item.description,
    enabled: item.enabled,
    unavailableReason: item.unavailableReason,
    badge: item.badge,
  })));
  const stableItems = useMemo<AssistLauncherItem[]>(() => {
    const visibleItems = JSON.parse(visibleState) as Array<Omit<AssistLauncherItem, "onSelect">>;
    return visibleItems.map((item, index) => ({
      ...item,
      onSelect: () => latestItems.current[index]?.onSelect(),
    }));
  }, [visibleState]);

  useLayoutEffect(() => {
    if (!context || !active) return;
    context.register({ key, label, items: stableItems });
    return () => context.unregister(key);
  }, [active, context, key, label, stableItems]);

  return context !== null;
}
