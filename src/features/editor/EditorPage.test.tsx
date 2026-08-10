import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import EditorPage, { isSafeDocumentLink } from "./EditorPage";

describe("EditorPage", () => {
  it("accepts only http(s) document links", () => {
    expect(isSafeDocumentLink("https://example.com/story")).toBe(true);
    expect(isSafeDocumentLink("http://localhost/story")).toBe(true);
    expect(isSafeDocumentLink("javascript:alert(1)")).toBe(false);
    expect(isSafeDocumentLink("file:///C:/secret.txt")).toBe(false);
    expect(isSafeDocumentLink("not a url")).toBe(false);
  });

  it("keeps a stable hook order when the document finishes loading", async () => {
    window.location.hash = "#/editor/101";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><EditorPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    const heading = await screen.findByLabelText("見出し 1");
    fireEvent.change(heading, { target: { value: "変更した見出し" } });
    expect(screen.getByText("未保存")).toBeInTheDocument();
  });
});
