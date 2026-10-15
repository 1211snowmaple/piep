import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MantineProvider } from "@mantine/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { discoverySuggestion } from "./discoveryTestData";

const api = vi.hoisted(() => ({ sweepCollectionCandidates: vi.fn(), listCollectionSuggestions: vi.fn(), dismissSweptSuggestions: vi.fn(), acceptCollectionSuggestion: vi.fn(), rejectCollectionSuggestion: vi.fn(), suggestionNameOverride: (proposed: string, selected: string) => proposed === selected ? undefined : selected }));
vi.mock("@/services/collectionApi", () => api);
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true, getAssetUrl: (path: string) => path, getReaderContentPage: vi.fn() }));
vi.mock("@/services/assistApi", () => ({ nameCollectionSuggestion: vi.fn() }));
vi.mock("@/features/assist/useAssist", () => ({ useAssist: () => ({ engine: null }) }));
vi.mock("@/app/router", () => ({ useAppNavigate: () => vi.fn() }));
vi.mock("@/components/WorkCover", () => ({ WorkCover: () => <span /> }));
import { CollectionSweepModal } from "./CollectionSweepModal";

function Harness() {
  const [opened, setOpened] = useState(true);
  return <><button onClick={() => setOpened(true)}>確認を再開</button><CollectionSweepModal opened={opened} onClose={() => setOpened(false)} /></>;
}
function renderModal() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><QueryClientProvider client={client}><Harness /></QueryClientProvider></MantineProvider>);
}

describe("CollectionSweepModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listCollectionSuggestions.mockResolvedValue([]);
    api.sweepCollectionCandidates.mockResolvedValue({ bundles: [], savedSearchSuggestions: [], semanticUsed: true, note: null });
    api.dismissSweptSuggestions.mockResolvedValue(0);
  });
  it("明示的に探すまでは走査しない", async () => {
    renderModal();
    expect(await screen.findByRole("button", { name: "棚から探す" })).toBeInTheDocument();
    expect(api.sweepCollectionCandidates).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "棚から探す" }));
    await waitFor(() => expect(api.sweepCollectionCandidates).toHaveBeenCalledOnce());
  });
  /** 残しておくと、次に開いても同じ顔ぶれが並ぶ。走査は同じ棚を同じ規則で
   *  見るので、貯まるのは「見送ったもの」だけだった。捨てて、開くたびに
   *  探し直せるようにする。**否定は記録しないので、次の走査でまた出てくる。** */
  it("閉じたら、確認しなかった候補は捨てる", async () => {
    api.listCollectionSuggestions.mockResolvedValue([discoverySuggestion()]);
    renderModal();
    await screen.findByRole("checkbox", { name: "作品2を含める" });
    await userEvent.click(screen.getByRole("button", { name: "まとまりを探すを閉じる" }));
    await waitFor(() => expect(api.dismissSweptSuggestions).toHaveBeenCalledOnce());
  });
  it("走査中でも閉じられ、再開後に結果を確認できる", async () => {
    let complete!: (value: unknown) => void;
    api.sweepCollectionCandidates.mockImplementation(() => new Promise((resolve) => { complete = resolve; }));
    renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "棚から探す" }));
    await userEvent.click(screen.getByRole("button", { name: "まとまりを探すを閉じる" }));
    api.listCollectionSuggestions.mockResolvedValue([discoverySuggestion()]);
    complete({ bundles: [discoverySuggestion()], savedSearchSuggestions: [], semanticUsed: true, note: null });
    await userEvent.click(screen.getByRole("button", { name: "確認を再開" }));
    expect(await screen.findByRole("heading", { name: "雨の連作" })).toBeInTheDocument();
  });
  it("探索できなかった範囲とエラーを画面に残す", async () => {
    api.sweepCollectionCandidates.mockResolvedValueOnce({ bundles: [], savedSearchSuggestions: [], semanticUsed: false, note: "テーマの索引を読み込めませんでした。" });
    renderModal();
    await userEvent.click(await screen.findByRole("button", { name: "棚から探す" }));
    expect(await screen.findByText("テーマの索引を読み込めませんでした。")).toBeInTheDocument();
    api.sweepCollectionCandidates.mockRejectedValueOnce(new Error("sweep failed"));
    await userEvent.click(screen.getByRole("button", { name: "棚から探す" }));
    expect(await screen.findByText("棚を調べられません")).toBeInTheDocument();
  });
});
