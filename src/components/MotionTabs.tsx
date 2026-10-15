import { useLayoutEffect, useRef, useState } from "react";
import { Tabs, type TabsProps } from "@mantine/core";
import { transitionContent } from "@/lib/contentTransition";

interface MotionTabsProps extends TabsProps {
  /** Library results live below the tabs; ordinary tabs own their panels. */
  contentSelector?: string;
}

function MotionTabsRoot({ value, onChange, children, className, contentSelector, ...props }: MotionTabsProps) {
  const root = useRef<HTMLDivElement>(null);
  const [indicator, setIndicator] = useState<{ x: number; y: number; width: number } | null>(null);

  useLayoutEffect(() => {
    const node = root.current;
    if (!node || props.orientation === "vertical" || (props.variant && props.variant !== "default")) return;
    const list = node.querySelector<HTMLElement>("[role=tablist]");
    const selected = list?.querySelector<HTMLElement>("[role=tab][aria-selected=true]");
    if (!list || !selected) { setIndicator(null); return; }
    const measure = () => {
      const bounds = node.getBoundingClientRect();
      const tab = selected.getBoundingClientRect();
      if (tab.width <= 0) return;
      const next = { x: tab.left - bounds.left, y: tab.bottom - bounds.top - 2, width: tab.width };
      setIndicator((current) => current?.x === next.x && current.y === next.y && current.width === next.width ? current : next);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(list);
    observer.observe(selected);
    return () => observer.disconnect();
  }, [value, props.orientation, props.variant]);

  const change = (next: string | null) => {
    if (next === value) return;
    transitionContent(() => onChange?.(next), {
      content: () => contentSelector
        ? document.querySelector<HTMLElement>(contentSelector)
        : [...(root.current?.querySelectorAll<HTMLElement>("[role=tabpanel]") ?? [])]
          .find((panel) => !panel.hidden && panel.style.display !== "none") ?? null,
    });
  };

  return <Tabs {...props} ref={root} value={value} onChange={change}
    className={["motion-tabs", className].filter(Boolean).join(" ")}
    data-indicator-ready={Boolean(indicator) || undefined}>
    {children}
    {indicator && <span aria-hidden className="motion-tabs__indicator" style={{ width: indicator.width, transform: `translate(${indicator.x}px, ${indicator.y}px)` }} />}
  </Tabs>;
}

/** Retain Mantine's tab roles, panel lifecycle, and arrow-key navigation. */
export const MotionTabs = Object.assign(MotionTabsRoot, { List: Tabs.List, Tab: Tabs.Tab, Panel: Tabs.Panel });
