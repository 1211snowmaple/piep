import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { discoverySuggestion } from "./discoveryTestData";

const api = vi.hoisted(() => ({ listCollectionSuggestions: vi.fn(), acceptCollectionSuggestion: vi.fn(), rejectCollectionSuggestion: vi.fn(), dismissCollectionSuggestion: vi.fn(), getReaderContentPage: vi.fn(), navigate: vi.fn(), suggestionNameOverride: (proposed: string, selected: string) => proposed === selected ? undefined : selected }));
vi.mock("@/services/collectionApi", () => api);
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true, getAssetUrl: (path: string) => path, getReaderContentPage: api.getReaderContentPage }));
vi.mock("@/services/assistApi", () => ({ nameCollectionSuggestion: vi.fn() }));
vi.mock("@/features/assist/useAssist", () => ({ useAssist: () => ({ engine: null }) }));
vi.mock("@/app/router", () => ({ useAppNavigate: () => api.navigate }));
vi.mock("@/components/WorkCover", () => ({ WorkCover: () => <span /> }));
import { SuggestionInbox } from "./SuggestionInbox";

function renderInbox() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<MantineProvider><ModalsProvider><QueryClientProvider client={client}><SuggestionInbox sweeping={false} savedSearchIdeas={[]} /></QueryClientProvider></ModalsProvider></MantineProvider>);
}

describe("SuggestionInbox", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listCollectionSuggestions.mockResolvedValue([discoverySuggestion()]);
    api.acceptCollectionSuggestion.mockResolvedValue({ id: "col-1" });
    api.rejectCollectionSuggestion.mockResolvedValue(true);
    api.getReaderContentPage.mockResolvedValue({ page: 0, pageCount: 2, plainText: "保存した本文をここで確認できます。", html: "", totalPlainTextChars: 200, sourcePageStarts: [0] });
  });

  it("選んだ作品を編集した順序と名前で保存し、確認画面を維持する", async () => {
    renderInbox();
    await screen.findByRole("button", { name: "3作品で作る" });
    await userEvent.click(screen.getByRole("button", { name: "作品3を一つ前へ" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "作品2を含める" }));
    await userEvent.clear(screen.getByRole("textbox", { name: "コレクション名" }));
    await userEvent.type(screen.getByRole("textbox", { name: "コレクション名" }), "確認した連作");
    await userEvent.click(screen.getByRole("button", { name: "2作品で作る" }));
    await waitFor(() => expect(api.acceptCollectionSuggestion).toHaveBeenCalledWith({ suggestionId: "sug-1", name: "確認した連作", memberKeys: [{ source: "pixiv", sourceId: "1" }, { source: "fanbox", sourceId: "3" }] }));
    expect(api.navigate).not.toHaveBeenCalled();
    expect(await screen.findByText("「確認した連作」を作りました。")).toBeInTheDocument();
  });

  it("題名・取得元・根拠を表示し、題名を押しても選択を変えない", async () => {
    renderInbox();
    await userEvent.click(await screen.findByText("作品1", { exact: true }));
    expect(screen.getByRole("checkbox", { name: "作品1を含める" })).toBeChecked();
    const detail = screen.getByRole("region", { name: "選んだ候補の詳細" });
    expect(within(detail).getByText("第1話")).toBeInTheDocument();
    expect(within(detail).getAllByText("FANBOX").length).toBeGreaterThan(0);
  });

  it("本文をページ送りして戻っても読む順と選択を保持する", async () => {
    renderInbox();
    await userEvent.click(await screen.findByRole("button", { name: "作品2を一つ前へ" }));
    await userEvent.click(screen.getAllByRole("button", { name: "本文を確認" })[0]);
    expect(await screen.findByText("保存した本文をここで確認できます。")).toBeInTheDocument();
    expect(api.getReaderContentPage).toHaveBeenCalledWith(2, null, 0, true);
    await userEvent.click(screen.getByRole("button", { name: "本文の次のページ" }));
    await waitFor(() => expect(api.getReaderContentPage).toHaveBeenCalledWith(2, null, 1, true));
    await userEvent.click(screen.getByRole("button", { name: "候補の確認に戻る" }));
    const rows = screen.getAllByRole("checkbox");
    expect(rows[0]).toHaveAccessibleName("作品2を含める");
    expect(rows[0]).toBeChecked();
  });

  it("保留・候補切替のあとも名前と選択を維持し、削除しない", async () => {
    api.listCollectionSuggestions.mockResolvedValue([discoverySuggestion(), discoverySuggestion("sug-2", "星の連作")]);
    renderInbox();
    await userEvent.click(await screen.findByRole("checkbox", { name: "作品2を含める" }));
    await userEvent.click(screen.getByRole("button", { name: "保留して次へ" }));
    expect(screen.getByRole("heading", { name: "星の連作" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "保留 1" }));
    expect(screen.getByRole("heading", { name: "雨の連作" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "作品2を含める" })).not.toBeChecked();
    expect(api.dismissCollectionSuggestion).not.toHaveBeenCalled();
    expect(api.rejectCollectionSuggestion).not.toHaveBeenCalled();
  });

  it("続き物を先に表示しテーマを別に選べる", async () => {
    api.listCollectionSuggestions.mockResolvedValue([{ ...discoverySuggestion("theme", "海の物語"), track: "theme", collectionKind: "unordered" }, discoverySuggestion()]);
    renderInbox();
    expect(await screen.findByRole("heading", { name: "雨の連作" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: "テーマ 1" }));
    expect(screen.getByRole("heading", { name: "海の物語" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "作品2を一つ前へ" })).not.toBeInTheDocument();
  });

  it("名前の別案を選べる", async () => {
    renderInbox();
    await userEvent.click(await screen.findByRole("button", { name: "他の名前の案 1" }));
    await userEvent.click(await screen.findByRole("button", { name: /作者の連作/ }));
    expect(screen.getByRole("textbox", { name: "コレクション名" })).toHaveValue("作者の連作");
  });

  it("2作品未満や空の名前では採用できない", async () => {
    renderInbox();
    await userEvent.click(await screen.findByRole("checkbox", { name: "作品1を含める" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "作品2を含める" }));
    expect(screen.getByRole("button", { name: "1作品で作る" })).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox", { name: "作品1を含める" }));
    await userEvent.clear(screen.getByRole("textbox", { name: "コレクション名" }));
    expect(screen.getByRole("button", { name: "2作品で作る" })).toBeDisabled();
  });

  it("読込失敗を空の候補として扱わず再試行できる", async () => {
    api.listCollectionSuggestions.mockRejectedValueOnce(new Error("read failed"));
    renderInbox();
    await userEvent.click(await screen.findByRole("button", { name: "もう一度読み込む" }));
    expect(await screen.findByRole("heading", { name: "雨の連作" })).toBeInTheDocument();
  });

  it("却下は選択中の組合せを確認してから送る", async () => {
    renderInbox();
    await userEvent.click(await screen.findByRole("checkbox", { name: "作品2を含める" }));
    await userEvent.click(screen.getByRole("button", { name: "この組合せを候補から外す" }));
    expect(api.rejectCollectionSuggestion).not.toHaveBeenCalled();
    await userEvent.click(await screen.findByRole("button", { name: "候補から外す" }));
    await waitFor(() => expect(api.rejectCollectionSuggestion).toHaveBeenCalledWith("sug-1", [{ source: "pixiv", sourceId: "1" }, { source: "fanbox", sourceId: "3" }]));
  });
});
