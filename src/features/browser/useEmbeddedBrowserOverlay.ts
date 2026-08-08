import { useEffect, useRef, type RefObject } from "react";
import { setEmbeddedBrowserVisible } from "@/services/browserApi";

/** Fractions of the pane that get hit-tested, including the very edges. */
const SAMPLE_FRACTIONS = [0.02, 0.25, 0.5, 0.75, 0.98];

/**
 * The embedded browser is a native child WebView. On every platform it
 * composites above the main WebView, so anything the DOM draws over that
 * rectangle - modals, dropdowns, toasts, the command palette - is painted
 * behind it and becomes both invisible and unclickable. No z-index can fix
 * that from CSS, so the child WebView is hidden while something covers it.
 *
 * Coverage is decided by hit-testing points inside the pane rather than by
 * matching overlay class names: Mantine mounts modals and drawers under a
 * zero-height wrapper and positions the visible surface on a descendant, so
 * measuring the matched root reported "no overlap" for every one of them.
 * `elementFromPoint` asks the browser what is actually on top, which needs no
 * knowledge of how any particular overlay is built.
 */
export function useEmbeddedBrowserOverlay(viewportRef: RefObject<HTMLElement | null>, enabled: boolean) {
  const visibleRef = useRef(true);

  useEffect(() => {
    if (!enabled) return;
    let frame = 0;
    let disposed = false;

    const apply = (visible: boolean) => {
      if (disposed || visibleRef.current === visible) return;
      visibleRef.current = visible;
      setEmbeddedBrowserVisible(visible).catch(() => undefined);
    };

    const evaluate = () => {
      const viewport = viewportRef.current;
      if (!viewport) return;
      const bounds = viewport.getBoundingClientRect();
      if (bounds.width < 1 || bounds.height < 1) return;
      for (const fx of SAMPLE_FRACTIONS) {
        for (const fy of SAMPLE_FRACTIONS) {
          const x = bounds.left + bounds.width * fx;
          const y = bounds.top + bounds.height * fy;
          const top = document.elementFromPoint(x, y);
          // Anything that is not the placeholder itself is drawn over the pane.
          // Elements with pointer-events:none are skipped by elementFromPoint,
          // which is what we want: they do not visually occlude either.
          if (top && top !== viewport && !viewport.contains(top)) {
            apply(false);
            return;
          }
        }
      }
      apply(true);
    };

    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(evaluate);
    };

    const observer = new MutationObserver(schedule);
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["style", "class", "hidden", "aria-hidden"],
    });
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    // Overlays animate open and closed, so a low-rate poll catches the frames
    // where the mutation has already been applied but layout has not settled.
    const timer = window.setInterval(schedule, 250);
    schedule();

    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      window.clearInterval(timer);
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      visibleRef.current = true;
    };
  }, [enabled, viewportRef]);
}
