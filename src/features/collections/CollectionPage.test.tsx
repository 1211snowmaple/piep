import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import CollectionPage from "./CollectionPage";

function renderPage(path: string) {
  window.localStorage.removeItem("piep.library-view");
  window.location.hash = path;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <ModalsProvider>
        <QueryClientProvider client={client}>
          <AppRouter><WorkspaceProvider><CollectionPage /></WorkspaceProvider></AppRouter>
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );
}

describe("CollectionPage browser preview", () => {
  it("renders demo details read-only instead of exposing failing desktop mutations", async () => {
    renderPage("#/collections/demo-series");

    expect(await screen.findByText("星を編む人 第十一話・第十二話")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "作品を追加" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "一冊のEPUB" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "名前を付け直す" })).toBeDisabled();
    expect(screen.queryByText("コレクションに作品を追加")).toBeNull();
    expect(document.querySelector(".collection-members--gallery")).toBeInTheDocument();
  });

  it("shows not-found for an unknown demo collection id", async () => {
    renderPage("#/collections/does-not-exist");
    expect(await screen.findByText("コレクションが見つかりません")).toBeInTheDocument();
  });
});
