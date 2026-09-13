import { useEffect, useRef, type RefObject } from "react";
import { setEmbeddedBrowserVisible } from "@/services/browserApi";

/** Fractions of the pane that get hit-tested, including the very edges. */
const SAMPLE_FRACTIONS = [0.02, 0.25, 0.5, 0.75, 0.98];

function overlaps(a: DOMRect, b: DOMRect) {
  return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
}

/**
 * Mantine puts floating surfaces under a shared portal node. Some components
 * add zero-sized wrappers, so measure every visible descendant rather than
 * assuming the first child is the painted surface. This catches small and
 * pointer-events:none overlays — notably tooltips — that point sampling can
 * never see reliably.
 */
function portalOverlaps(bounds: DOMRect) {
  const surfaces = document.querySelectorAll<HTMLElement>("[data-portal] *");
  for (const surface of surfaces) {
    const rect = surface.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0 || !overlaps(bounds, rect)) continue;
    // Computed style can force layout in a large menu. Only ask for it after
    // the cheap geometry check says the node could cover the native view.
    const style = window.getComputedStyle(surface);
    if (style.display !== "none" && style.visibility !== "hidden") return true;
  }
  return false;
}

/**
 * The embedded browser is a native child WebView. On every platform it
 * composites above the main WebView, so anything the DOM draws over that
 * rectangle - modals, dropdowns, toasts, the command palette - is painted
 * behind it and becomes both invisible and unclickable. No z-index can fix
 * that from CSS, so the child WebView is hidden while something covers it.
 *
 * Portaled surfaces are checked by rectangle intersection, including small
 * pointer-transparent tooltips. A point hit-test remains as a fallback for
 * inline overlays. Mantine often mounts a zero-height portal wrapper and puts
 * the visible rectangle on a descendant, so the geometry pass measures those
 * descendants instead of the wrapper itself.
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
      // Portaled surfaces are measured exactly. This includes non-interactive
      // visual overlays such as Tooltip, whose pointer-events:none otherwise
      // makes it invisible to elementFromPoint even while the native WebView
      // paints over it.
      if (portalOverlaps(bounds)) {
        apply(false);
        return;
      }
      for (const fx of SAMPLE_FRACTIONS) {
        for (const fy of SAMPLE_FRACTIONS) {
          const x = bounds.left + bounds.width * fx;
          const y = bounds.top + bounds.height * fy;
          const top = document.elementFromPoint(x, y);
          // Anything that is not the placeholder itself is drawn over the pane.
          // Non-portaled inline overlays are still caught here. Portaled
          // pointer-events:none surfaces were handled geometrically above.
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
