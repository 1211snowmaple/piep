import { act, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setEmbeddedBrowserVisible } from "@/services/browserApi";
import { useEmbeddedBrowserOverlay } from "./useEmbeddedBrowserOverlay";

vi.mock("@/services/browserApi", () => ({
  setEmbeddedBrowserVisible: vi.fn().mockResolvedValue(true),
}));

const rect = (left: number, top: number, width: number, height: number) => ({
  x: left,
  y: top,
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height,
  toJSON: () => ({}),
}) as DOMRect;

function Harness() {
  const viewport = useRef<HTMLDivElement>(null);
  useEmbeddedBrowserOverlay(viewport, true);
  return <div ref={viewport} data-testid="browser-viewport" />;
}

describe("useEmbeddedBrowserOverlay", () => {
  let frames: FrameRequestCallback[];
  const originalElementFromPoint = document.elementFromPoint;

  beforeEach(() => {
    frames = [];
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.push(callback);
      return frames.length;
    });
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);
    vi.mocked(setEmbeddedBrowserVisible).mockClear();
  });

  afterEach(() => {
    document.querySelectorAll("[data-overlay-test]").forEach((node) => node.remove());
    if (originalElementFromPoint) {
      Object.defineProperty(document, "elementFromPoint", {
        configurable: true,
        value: originalElementFromPoint,
      });
    } else {
      delete (document as Partial<Document>).elementFromPoint;
    }
    vi.restoreAllMocks();
  });

  const flushFrame = () => {
    const pending = frames.splice(0);
    pending.forEach((callback) => callback(performance.now()));
  };

  it("hides the native view for a small non-interactive portaled tooltip", () => {
    render(<Harness />);
    const viewport = screen.getByTestId("browser-viewport");
    vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue(rect(100, 100, 800, 600));
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: () => viewport,
    });

    const portal = document.createElement("div");
    portal.dataset.portal = "true";
    portal.dataset.overlayTest = "true";
    const tooltip = document.createElement("div");
    tooltip.setAttribute("role", "tooltip");
    tooltip.style.pointerEvents = "none";
    vi.spyOn(tooltip, "getBoundingClientRect").mockReturnValue(rect(734, 542, 104, 32));
    portal.append(tooltip);
    document.body.append(portal);

    act(() => {
      window.dispatchEvent(new Event("resize"));
      flushFrame();
    });

    expect(setEmbeddedBrowserVisible).toHaveBeenCalledWith(false);

    tooltip.remove();
    act(() => {
      window.dispatchEvent(new Event("resize"));
      flushFrame();
    });
    expect(setEmbeddedBrowserVisible).toHaveBeenLastCalledWith(true);
  });
});
