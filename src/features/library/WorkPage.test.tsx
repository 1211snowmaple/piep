import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { getDemoReader } from "@/mocks/demoData";
import WorkPage from "./WorkPage";

const runtime = vi.hoisted(() => ({ tauri: true }));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => runtime.tauri,
}));
vi.mock("@/features/library/ShelfWorkPage", () => ({
  ShelfWorkPage: ({ workId }: { workId: string }) => <p>棚の作品 {workId}</p>,
}));

function renderWork(hash: string) {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><WorkPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
}

describe("WorkPage", () => {
  beforeEach(() => { runtime.tauri = true; });

  it("opens a work of the shelf by its id", async () => {
    renderWork("#/works/0195F1C2-0000-7000-8000-000000000001");
    expect(await screen.findByText("棚の作品 0195f1c2-0000-7000-8000-000000000001")).toBeInTheDocument();
  });

  // 作品を番号で指すのは、旧棚のころのリンクとプレビューの見本だけ。アプリでは旧棚を開かない。
  it("does not open a work by an old number in the app", async () => {
    renderWork("#/works/101");
    expect(await screen.findByText("作品が見つかりません")).toBeInTheDocument();
  });

  it("shows the sample work by its number in the browser preview", async () => {
    runtime.tauri = false;
    renderWork("#/works/101");
    const title = getDemoReader("101").download.title;
    const heading = await screen.findByRole("heading", { name: title });
    expect(heading).toHaveClass("work-hero__title");
    expect(heading).not.toHaveClass("line-clamp-2");
  });
});
