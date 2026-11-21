import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { WorkCard } from "@/components/WorkCard";
import { demoWorks } from "@/mocks/demoData";
import { withClippedText } from "@/test/clipped";

const updateJobApi = vi.hoisted(() => ({ listPendingRevisionsCommand: vi.fn() }));
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  listPendingRevisionsCommand: updateJobApi.listPendingRevisionsCommand,
}));
const shelfApi = vi.hoisted(() => ({
  libraryV2Status: vi.fn(),
  libraryV2SetUserState: vi.fn(),
  libraryV2RemoveWork: vi.fn(),
}));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...shelfApi,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));

function renderCard(props: Partial<React.ComponentProps<typeof WorkCard>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <ModalsProvider>
          <AppRouter>
            <WorkspaceProvider>
              <WorkCard work={demoWorks[0]} {...props} />
            </WorkspaceProvider>
          </AppRouter>
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("WorkCard", () => {
  beforeEach(() => {
    updateJobApi.listPendingRevisionsCommand.mockResolvedValue([]);
    shelfApi.libraryV2Status.mockReset();
    shelfApi.libraryV2SetUserState.mockReset();
    shelfApi.libraryV2RemoveWork.mockReset();
  });

  it("is keyboard accessible and opens the work route", () => {
    window.location.hash = "#/library";
    renderCard();
    fireEvent.keyDown(screen.getByRole("link", { name: /雨上がりの図書室でを開く/ }), { key: "Enter" });
    expect(window.location.hash).toBe("#/works/101");
  });

  it("shows the same no-image picture as the author pages when an author has no icon", () => {
    const { unmount } = renderCard({ work: { ...demoWorks[1], personIconPath: null } });
    expect(screen.getByRole("link", { name: "mizu atelierの作品を見る" }).querySelector("img.no-image-mark")).not.toBeNull();
    unmount();
    // プロフィール画像の無い取得元と、作者の分からない作品は人型のまま。
    renderCard({ work: { ...demoWorks[1], source: "local", personIconPath: null } });
    expect(document.querySelector("img.no-image-mark")).toBeNull();
    cleanup();
    renderCard({ work: { ...demoWorks[1], personId: null, authorId: "", personIconPath: null } });
    expect(document.querySelector("img.no-image-mark")).toBeNull();
  });

  it("opens the work from non-control card content", () => {
    window.location.hash = "#/library";
    renderCard();

    fireEvent.click(screen.getByText("雨音が止んだ午後、閉館前の図書室で二人はもう一度出会った。"));

    expect(window.location.hash).toBe("#/works/101");
  });

  it("keeps the compact row surface clickable without stealing creator clicks", () => {
    window.location.hash = "#/library";
    const row = renderCard({ compact: true });
    fireEvent.click(screen.getByLabelText("雨上がりの図書室で（表紙なし）"));
    expect(window.location.hash).toBe("#/works/101");
    row.unmount();

    window.location.hash = "#/library";
    renderCard({ compact: true });
    fireEvent.click(screen.getByRole("link", { name: "青葉しおりの作品を見る" }));
    expect(window.location.hash).toBe("#/people/pixiv/8001234");
  });

  // 選ぶ面はカードそのもの。印は中央に大きく置くが、名前と押し込みの状態は
  // カードが持つ - でないとキーボードで選べる場所が無くなる。
  it("makes the whole card the only interactive action in selection mode", () => {
    const onSelect = vi.fn();
    window.location.hash = "#/library";
    const view = renderCard({ selectionMode: true, selected: false, onSelect });
    const card = screen.getByRole("button", { name: /雨上がりの図書室でを選択/ });
    expect(card).toHaveClass("work-card");
    expect(card).toHaveAttribute("aria-pressed", "false");
    expect(view.container.querySelector(".work-card__body")).toHaveAttribute("inert");
    expect(view.container.querySelector(".work-card__footer")).toHaveAttribute("inert");
    expect(view.container.querySelector(".card-select")).not.toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getAllByRole("button")).toEqual([card]);

    fireEvent.click(screen.getByText("青葉しおり"));
    expect(onSelect).toHaveBeenCalledWith(demoWorks[0].id, true);
    expect(window.location.hash).toBe("#/library");
  });

  it("shows a clear selected state and lets the card undo it", () => {
    const onSelect = vi.fn();
    const view = renderCard({ selectionMode: true, selected: true, onSelect });

    expect(view.container.querySelector(".work-card")).toHaveAttribute("data-selected", "true");
    expect(view.container.querySelector(".card-select")).toHaveAttribute("data-checked", "true");
    const card = screen.getByRole("button", { name: /雨上がりの図書室でを選択解除/ });
    expect(card).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(card);
    expect(onSelect).toHaveBeenCalledWith(demoWorks[0].id, false);
  });

  it("keeps only the compact selection toggle in the tab order", () => {
    const view = renderCard({ compact: true, selectionMode: true, onSelect: vi.fn() });
    const toggle = screen.getByRole("button", { name: /雨上がりの図書室でを選択/ });
    expect(toggle.closest("[inert]")).toBeNull();
    expect(view.container.querySelector(".work-row__main")).toHaveAttribute("inert");
    expect(view.container.querySelector(".work-row__tags")).toHaveAttribute("inert");
    expect(view.container.querySelector(".work-row__facts")).toHaveAttribute("inert");
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getAllByRole("button")).toEqual([toggle]);
  });

  it("filters the library by a tag without opening the work", () => {
    renderCard({ work: { ...demoWorks[0], tags: ["創作", "青春", "短編", "恋愛", "雨", "図書室"] } });

    // Every tag is rendered; how many stay visible is decided by measurement,
    // which needs a real layout and so is exercised in the browser instead.
    expect(screen.getByText("図書室")).toBeInTheDocument();
    window.location.hash = "#/library";
    fireEvent.click(screen.getByText("雨"));
    expect(window.location.hash).toBe(`#/library?tag=${encodeURIComponent("雨")}`);
  });

  it.each(["Fate/Grand Order", '表示に "引用" があるタグ'])("keeps the complete tag %s as a filter rather than splitting it into search words", (tag) => {
    renderCard({ work: { ...demoWorks[0], tags: [tag] } });
    fireEvent.click(screen.getByText(tag));
    const params = new URLSearchParams(window.location.hash.split("?")[1]);
    expect(params.getAll("tag")).toEqual([tag]);
    expect(params.has("q")).toBe(false);
  });

  it("keeps frequent actions directly available on the card", () => {
    renderCard();

    expect(screen.getByRole("button", { name: /雨上がりの図書室で：読む/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "雨上がりの図書室で：お気に入りを解除" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "雨上がりの図書室で：EPUBキューに追加" })).toBeInTheDocument();
    // Watching is a toggle on the card, not a link to the update centre.
    expect(screen.getByRole("button", { name: "雨上がりの図書室で：更新監視をオフにする" })).toHaveAttribute("aria-pressed", "true");
  });

  it("offers the full text on hover only when the card had to clip it", async () => {
    const caption = demoWorks[0].excerpt as string;
    const series = demoWorks[0].seriesTitle as string;

    // 全部見えている文字を、その真上にもう一度出しても読む物は増えない。
    // 増えるのは、隠れるもの - シリーズ行や、一つ上のカード - だけである。
    const roomy = renderCard();
    fireEvent.mouseEnter(screen.getByText(caption));
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(screen.queryByRole("tooltip")).toBeNull();
    roomy.unmount();

    const card = withClippedText(() => renderCard());
    const captionElement = screen.getByText(caption);
    fireEvent.mouseEnter(captionElement);
    expect(await screen.findByRole("tooltip")).toHaveTextContent(caption);
    fireEvent.mouseLeave(captionElement);
    card.unmount();

    const row = withClippedText(() => renderCard({ compact: true }));
    fireEvent.mouseEnter(screen.getByRole("button", { name: `シリーズ「${series}」を開く` }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(series);
    row.unmount();

    // 作者名にも同じ手立てを付けた。切られた名前ほど、誰なのかが要る。
    const author = demoWorks[0].personName || demoWorks[0].authorName;
    withClippedText(() => renderCard({ compact: true }));
    fireEvent.mouseEnter(screen.getByRole("link", { name: `${author}の作品を見る` }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(author);
  });

  it("only offers the version chip once a work has more than one revision", () => {
    const single = renderCard({ work: { ...demoWorks[0], currentVersion: 1 } });
    expect(screen.queryByRole("button", { name: /バージョン履歴/ })).toBeNull();
    single.unmount();

    renderCard({ work: { ...demoWorks[0], currentVersion: 3 } });
    window.location.hash = "#/library";
    fireEvent.click(screen.getByRole("button", { name: /バージョン履歴/ }));
    expect(window.location.hash).toBe("#/works/101?tab=history");
  });

  it("labels a history-only search hit and opens the matching saved revision", () => {
    window.location.hash = "#/library?q=%E6%97%A7%E7%89%88&versions=all";
    renderCard({
      work: {
        ...demoWorks[0],
        currentVersion: 3,
        matchedVersion: 1,
        historicalMatchCount: 2,
        matchFields: ["body"],
        scoreReasons: [{ field: "body", matchType: "exact", term: "旧版", contribution: 10 }],
      },
    });

    const historicalHit = screen.getByRole("button", { name: /過去版 v1 を読む/ });
    expect(historicalHit).toHaveTextContent("過去版 v1");
    fireEvent.click(historicalHit);
    expect(window.location.hash).toBe("#/reader/101?version=1");
  });

  it("labels a semantic candidate as similar without claiming a text match", () => {
    renderCard({
      work: {
        ...demoWorks[0],
        matchFields: ["content"],
        scoreReasons: [{ field: "content", matchType: "semantic", term: "旅", contribution: 10 }],
      },
    });

    expect(screen.getByLabelText("類似理由: 本文の傾向")).toHaveTextContent("類似");
    expect(screen.queryByText("一致")).toBeNull();
  });

  it("labels and deep-links an unsaved rewrite separately from saved history", async () => {
    updateJobApi.listPendingRevisionsCommand.mockResolvedValue([{ work: "101", foundAt: "2026-09-01T00:00:00Z" }]);
    renderCard({ work: { ...demoWorks[0], currentVersion: 2 } });

    expect(await screen.findByRole("button", { name: /未保存の改稿を比較/ })).toHaveTextContent("未保存の改稿");
    expect(screen.getByRole("button", { name: /バージョン履歴/ })).toHaveTextContent("履歴 2版");
    fireEvent.click(screen.getByRole("button", { name: /未保存の改稿を比較/ }));
    expect(window.location.hash).toBe("#/works/101?tab=history&compare=pending");
  });

  it("marks an unsaved rewrite of a shelf work by its UUID", async () => {
    const shelfId = "0195f1c2-0000-7000-8000-000000000001";
    updateJobApi.listPendingRevisionsCommand.mockResolvedValue([{ work: shelfId, foundAt: "2026-09-01T00:00:00Z" }]);
    const { id: _legacy, ...rest } = demoWorks[0];
    renderCard({ work: { ...rest, id: shelfId, shelf: "v2", coverUrl: null, coverPath: null, legacyId: null } });

    fireEvent.click(await screen.findByRole("button", { name: /未保存の改稿を比較/ }));
    expect(window.location.hash).toBe(`#/works/${shelfId}?tab=history&compare=pending`);
  });

  it("uses the shared no-cover treatment and keeps the provider visible", () => {
    renderCard({ work: { ...demoWorks[0], source: "fanbox", coverPath: null } });

    expect(screen.getByLabelText("雨上がりの図書室で（表紙なし）")).toBeInTheDocument();
    expect(screen.getByLabelText("FANBOX")).toBeInTheDocument();
    expect(screen.queryByText("FANBOX")).toBeNull();
  });

  it("shows a work of the shelf by its own id and cover", async () => {
    updateJobApi.listPendingRevisionsCommand.mockResolvedValue([{ work: "101", foundAt: "2026-09-01T00:00:00Z" }]);
    window.location.hash = "#/library";
    const shelfId = "0195f1c2-0000-7000-8000-000000000001";
    const { id: _legacy, ...rest } = demoWorks[0];
    renderCard({ work: { ...rest, id: shelfId, shelf: "v2", coverUrl: "http://piep-library.localhost/abc", coverPath: null, legacyId: null } });

    expect(screen.getByAltText("雨上がりの図書室での表紙")).toHaveAttribute("src", "http://piep-library.localhost/abc");
    // 改稿の印は棚の作品 ID で照らすので、旧棚の番号の改稿は出さない。EPUB のキューには棚の作品の ID で積む。
    const queue = screen.getByRole("button", { name: /雨上がりの図書室で：EPUBキューに追加/ });
    expect(queue).toBeEnabled();
    fireEvent.click(queue);
    expect(screen.getByRole("button", { name: /雨上がりの図書室で：EPUBキューから外す/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /雨上がりの図書室で：読む$/ })).toBeEnabled();
    await vi.waitFor(() => expect(updateJobApi.listPendingRevisionsCommand).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: /未保存の改稿を比較/ })).toBeNull();

    fireEvent.keyDown(screen.getByRole("link", { name: /雨上がりの図書室でを開く/ }), { key: "Enter" });
    expect(window.location.hash).toBe(`#/works/${shelfId}`);
  });

  it("writes the flags of a work on the shelf", async () => {
    window.location.hash = "#/library";
    shelfApi.libraryV2Status.mockResolvedValue({ state: "ready", path: "x" });
    shelfApi.libraryV2SetUserState.mockResolvedValue({ favorite: true, watching: false, generation: 2 });
    const shelfId = "0195f1c2-0000-7000-8000-000000000001";
    const { id: _legacy, ...rest } = demoWorks[0];
    renderCard({ work: { ...rest, id: shelfId, shelf: "v2", coverUrl: null, coverPath: null, legacyId: null, favorite: false, watchUpdates: false } });

    fireEvent.click(await screen.findByRole("button", { name: /雨上がりの図書室で：お気に入りに追加/ }));
    expect(shelfApi.libraryV2SetUserState).toHaveBeenCalledWith(shelfId, { favorite: true });
    fireEvent.click(screen.getByRole("button", { name: /雨上がりの図書室で：更新監視をオンにする/ }));
    expect(shelfApi.libraryV2SetUserState).toHaveBeenCalledWith(shelfId, { watching: true });
  });

});
