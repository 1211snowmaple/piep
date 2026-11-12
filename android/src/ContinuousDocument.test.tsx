import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ContinuousDocument } from "./ContinuousDocument";

vi.mock("./readerDocument", () => ({ fillDocumentImages: async () => {} }));

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (callback: () => void) => window.setTimeout(callback, 0));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => window.clearTimeout(id));
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it.each([false, true])("保存世代が変わった次の本文を混ぜない（世代変更=%s）", async (changed) => {
  const viewport = document.createElement("div");
  Object.defineProperty(viewport, "clientHeight", { value: 100 });
  Object.defineProperty(viewport, "clientWidth", { value: 500 });
  const onSnapshotChanged = vi.fn();
  const onInfo = vi.fn();
  const view = render(<ContinuousDocument
    viewportRef={{ current: viewport }}
    load={async (page) => ({
      page: { page, pageCount: 2, html: "", totalPlainTextChars: 2, sourcePageStarts: [0, 1] },
      html: `<p>Page ${page} from ${changed && page === 1 ? "snapshot-B" : "snapshot-A"}</p>`,
      generation: changed && page === 1 ? "snapshot-B" : "snapshot-A",
    })}
    imageSource={() => ""} isSnapshotChange={() => false} onSnapshotChanged={onSnapshotChanged}
    start={() => ({ page: 0, top: 0, nonce: 0 })} target={null} typeKey="fixture" header={null}
    onInfo={onInfo} onPosition={() => {}} onCommit={() => {}} onLink={() => {}}
    onStartRejected={() => false} flushRef={{ current: null }}
  />);
  if (changed) {
    await waitFor(() => expect(onSnapshotChanged).toHaveBeenCalledTimes(1));
    expect(view.container.textContent).not.toContain("snapshot-B");
    expect(onInfo).toHaveBeenCalledTimes(1);
  } else {
    await waitFor(() => expect(view.container.textContent).toContain("Page 1 from snapshot-A"));
    expect(onSnapshotChanged).not.toHaveBeenCalled();
    expect(view.container.textContent).toContain("Page 0 from snapshot-A");
  }
});
