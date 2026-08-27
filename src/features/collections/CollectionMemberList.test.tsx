import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { CollectionMemberList } from "@/features/collections/CollectionMemberList";
import { demoWorks } from "@/mocks/demoData";
import type { WorkCollectionMember } from "@/types/collections";

function member(overrides: Partial<WorkCollectionMember> = {}): WorkCollectionMember {
  const work = demoWorks[0];
  return {
    collectionId: "collection-1",
    source: work.source,
    sourceId: work.sourceId,
    downloadId: work.id,
    title: work.title,
    authorName: work.authorName,
    coverPath: work.coverPath,
    textLength: work.textLength,
    position: 0,
    memberRole: "main",
    addedBy: "manual",
    pinned: false,
    note: null,
    missing: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    work,
    editions: [],
    ...overrides,
  };
}

function renderList(props: Partial<React.ComponentProps<typeof CollectionMemberList>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const merged: React.ComponentProps<typeof CollectionMemberList> = {
    members: [member(), member({ sourceId: "202", downloadId: 102, title: "つづき", position: 1, work: { ...demoWorks[1], id: 102 } })],
    ordered: true,
    view: "compact",
    busy: false,
    selectionMode: false,
    selected: new Set<number>(),
    onSelect: vi.fn(),
    onMove: vi.fn(),
    onDropAt: vi.fn(),
    onRemove: vi.fn(),
    ...props,
  };
  return {
    props: merged,
    ...render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <ModalsProvider>
            <AppRouter>
              <WorkspaceProvider>
                <div className="app-main"><CollectionMemberList {...merged} /></div>
              </WorkspaceProvider>
            </AppRouter>
          </ModalsProvider>
        </QueryClientProvider>
      </MantineProvider>,
    ),
  };
}

describe("CollectionMemberList", () => {
  it("draws members with the same card the shelf uses", () => {
    renderList();
    // 束の中でも棚と同じカードが出る。タグ行は縮小した投影では作れなかった。
    expect(document.querySelectorAll(".work-row").length).toBe(2);
    expect(screen.getAllByText(demoWorks[0].tags[0]).length).toBeGreaterThan(0);
  });

  it("keeps a keyboard path for reordering, not only drag", () => {
    const { props } = renderList();
    fireEvent.click(screen.getByRole("button", { name: `${demoWorks[0].title}を一つ後へ` }));
    expect(props.onMove).toHaveBeenCalledWith(0, 1);
    // 先頭は前へ動かせない。端で操作子が生きていると、押しても何も起きない。
    expect(screen.getByRole("button", { name: `${demoWorks[0].title}を一つ前へ` })).toBeDisabled();
  });

  // HTML5 の drag は使わない。取っ手は button で、Chromium は button の上で
  // 始まった仕草から祖先の drag を開始しない。掴んでも一度も動かなかったのは
  // これが理由で、いまはマウスも指も同じ pointer の道を通る。
  it.each(["mouse", "touch", "pen"])("carries a member with a %s pointer", (pointerType) => {
    const { props } = renderList({ view: "gallery" });
    const rows = Array.from(document.querySelectorAll<HTMLElement>(".collection-member"));
    const grip = document.querySelectorAll<HTMLElement>(".collection-member__grip")[0];
    // jsdom に elementFromPoint は無い。実際の広さも無いので、下に何があるかはここで答える。
    Object.defineProperty(document, "elementFromPoint", { value: () => rows[1], configurable: true });

    fireEvent.pointerDown(grip, { pointerId: 1, pointerType, button: 0 });
    expect(rows[0]).toHaveAttribute("data-dragging");
    fireEvent.pointerMove(grip, { pointerId: 1, pointerType, clientX: 10, clientY: 200 });
    expect(rows[1]).toHaveAttribute("data-drop-target");
    fireEvent.pointerUp(grip, { pointerId: 1, pointerType, clientX: 10, clientY: 200 });

    expect(props.onDropAt).toHaveBeenCalledWith(0, 1);
    expect(rows[0]).not.toHaveAttribute("data-dragging");
  });

  it("drops the grab when Escape is pressed, without moving anything", () => {
    const { props } = renderList({ view: "gallery" });
    const rows = Array.from(document.querySelectorAll<HTMLElement>(".collection-member"));
    const grip = document.querySelectorAll<HTMLElement>(".collection-member__grip")[0];

    fireEvent.pointerDown(grip, { pointerId: 1, pointerType: "mouse", button: 0 });
    expect(rows[0]).toHaveAttribute("data-dragging");
    fireEvent.keyDown(window, { key: "Escape" });

    expect(rows[0]).not.toHaveAttribute("data-dragging");
    expect(props.onDropAt).not.toHaveBeenCalled();
  });

  it("auto-scrolls the collection when a held pointer reaches the viewport edge", () => {
    const frames: FrameRequestCallback[] = [];
    const requestFrame = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.push(callback);
      return 1;
    });
    const cancelFrame = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);
    renderList({ view: "gallery" });
    const rows = Array.from(document.querySelectorAll<HTMLElement>(".collection-member"));
    const grip = document.querySelectorAll<HTMLElement>(".collection-member__grip")[0];
    const scroller = document.querySelector<HTMLElement>(".app-main")!;
    Object.defineProperties(scroller, {
      scrollTop: { value: 0, writable: true, configurable: true },
      scrollHeight: { value: 1_000, configurable: true },
      clientHeight: { value: 100, configurable: true },
    });
    vi.spyOn(scroller, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, top: 0, right: 300, bottom: 100, left: 0, width: 300, height: 100,
      toJSON: () => ({}),
    });
    Object.defineProperty(document, "elementFromPoint", { value: () => rows[1], configurable: true });

    fireEvent.pointerDown(grip, { pointerId: 2, pointerType: "touch", clientX: 20, clientY: 50 });
    fireEvent.pointerMove(grip, { pointerId: 2, pointerType: "touch", clientX: 20, clientY: 99 });
    expect(frames.length).toBeGreaterThan(0);
    frames[frames.length - 1](0);
    expect(scroller.scrollTop).toBeGreaterThan(0);
    fireEvent.pointerCancel(grip, { pointerId: 2, pointerType: "touch" });
    requestFrame.mockRestore();
    cancelFrame.mockRestore();
  });

  it("hides ordering controls when the collection has no reading order", () => {
    renderList({ ordered: false });
    expect(screen.queryByRole("button", { name: /一つ後へ/ })).toBeNull();
  });

  it("shows an unsaved member without pretending it is a work card", () => {
    renderList({
      members: [member({ downloadId: null, work: null, missing: true, title: "手元に無い話" })],
    });
    expect(screen.getByText("手元に無い話")).toBeInTheDocument();
    expect(screen.getByText("未保存")).toBeInTheDocument();
    expect(document.querySelectorAll(".work-row").length).toBe(0);
  });

  it("folds editions instead of listing them as separate members", () => {
    renderList({
      members: [member({ editions: [{ ...demoWorks[1], id: 900, title: "【FANBOXサンプル】同じ話" }] })],
    });
    // 畳んだ状態では、別版の題名は開くまで数に入らない。
    const toggle = screen.getByRole("button", { name: /別版 1件/ });
    expect(toggle).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(screen.getByText("【FANBOXサンプル】同じ話")).toBeInTheDocument();
  });

  it("progressively reveals a very large collection instead of mounting every card", () => {
    const members = Array.from({ length: 121 }, (_, index) => member({
      sourceId: `missing-${index + 1}`,
      downloadId: null,
      title: `member-${index + 1}`,
      position: index,
      work: null,
      missing: true,
    }));
    renderList({ members, ordered: false });

    expect(screen.queryByText("member-121")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "さらに表示（残り1件）" }));
    expect(screen.getByText("member-121")).toBeInTheDocument();
  });

  /**
   * ページ番号で見ているときも、位置は束の中での絶対位置でなければならない。
   *
   * 切った配列をそのまま `map` すると index はページ内の番号になる。そうなると
   * 2ページ目の先頭は「0番目」として扱われ、一つ前へ動かしたつもりで1ページ目の
   * 先頭が動く。しかも先頭は端として無効化されるので、ページをまたいで動かす
   * 手段そのものが消える。どちらも画面を見ただけでは気づきにくい。
   */
  describe("ページ番号で見ているとき", () => {
    const paged = (count: number) => Array.from({ length: count }, (_, index) => member({
      sourceId: `paged-${index + 1}`,
      downloadId: 1000 + index,
      title: `${index + 1}番目`,
      position: index,
      work: { ...demoWorks[0], id: 1000 + index, title: `${index + 1}番目` },
    }));

    it("そのページのぶんだけ描く", () => {
      renderList({ members: paged(8), page: { start: 4, end: 8 } });

      expect(screen.queryByText("4番目")).toBeNull();
      expect(screen.getByText("5番目")).toBeInTheDocument();
      expect(screen.getByText("8番目")).toBeInTheDocument();
    });

    it("並べ替えは、ページ内の番号ではなく束の中の位置で伝える", () => {
      const onMove = vi.fn();
      renderList({ members: paged(8), page: { start: 4, end: 8 }, onMove });

      fireEvent.click(screen.getByRole("button", { name: "5番目を一つ前へ" }));

      expect(onMove).toHaveBeenCalledWith(4, -1);
    });

    it("ページの先頭を、前のページへ動かせる", () => {
      renderList({ members: paged(8), page: { start: 4, end: 8 } });

      // ページ内の番号で見ていると端と判定され、この操作が無効化される。
      expect(screen.getByRole("button", { name: "5番目を一つ前へ" })).toBeEnabled();
    });

    it("読み上げの「何番目」も束の中での位置で名乗る", () => {
      renderList({ members: paged(8), page: { start: 4, end: 8 } });

      const rows = document.querySelectorAll<HTMLElement>(".collection-member");
      expect(rows[0]?.getAttribute("aria-posinset")).toBe("5");
      expect(rows[0]?.getAttribute("aria-setsize")).toBe("8");
    });

    it("ページで見ているあいだは「さらに表示」を出さない", () => {
      renderList({ members: paged(8), page: { start: 0, end: 4 } });

      expect(screen.queryByRole("button", { name: /さらに表示/ })).toBeNull();
    });
  });
});
