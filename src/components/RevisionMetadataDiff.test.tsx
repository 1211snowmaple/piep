import { MantineProvider } from "@mantine/core";
import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { RevisionMetadataDiff } from "./RevisionMetadataDiff";
import type { RevisionMetadata } from "@/services/workInspectionApi";

it("marks only edited caption spans, leaving shared text and both panels neutral", () => {
  const before: RevisionMetadata = { title: "題名", caption: "同じ説明です。\n削除する文です。\n共通のリンク https://example.com/next", tags: [], publishedAt: null, updatedAt: null, assets: [] };
  const after = { ...before, caption: "同じ説明です。\n共通のリンク https://example.com/next" };
  const { container } = render(<MantineProvider><RevisionMetadataDiff before={before} after={after} /></MantineProvider>);
  expect(container.querySelectorAll(".revision-metadata-value[data-kind]")).toHaveLength(0);
  expect(container.querySelectorAll("ins")).toHaveLength(0);
  expect(container.querySelector("del")?.textContent).toContain("削除する文");
  expect(screen.getAllByText(/同じ説明です/).every((element) => element.tagName === "SPAN")).toBe(true);
});
