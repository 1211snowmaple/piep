import { MantineProvider } from "@mantine/core";
import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AssistProgressEvent } from "@/services/assistApi";
import { theme } from "@/theme";
import { AssistProgressLine, assistStageText } from "./AssistProgress";

const listeners: ((event: { payload: AssistProgressEvent }) => void)[] = [];
vi.mock("@/services/eventBus", () => ({
  subscribeTauriEvent: (_name: string, handler: (event: { payload: AssistProgressEvent }) => void) => {
    listeners.push(handler);
    return () => undefined;
  },
}));

const event = (change: Partial<AssistProgressEvent>): AssistProgressEvent => ({
  feature: "work_synopsis",
  downloadId: 7,
  stage: "drafting",
  section: null,
  sections: null,
  retry: false,
  ...change,
});

describe("AssistProgressLine", () => {
  it("names the stage for this work only", () => {
    render(<MantineProvider theme={theme}><AssistProgressLine feature="work_synopsis" downloadId={7} active /></MantineProvider>);
    expect(screen.getByText("準備しています")).toBeInTheDocument();

    act(() => listeners.forEach((listener) => listener({ payload: event({ downloadId: 8, stage: "checking" }) })));
    expect(screen.getByText("準備しています")).toBeInTheDocument();

    act(() => listeners.forEach((listener) => listener({ payload: event({ stage: "checking" }) })));
    expect(screen.getByText("下書きの文を、一つずつ原文と照らしています")).toBeInTheDocument();
  });

  it("shows the section and a retry", () => {
    expect(assistStageText(event({ stage: "reading", section: 2, sections: 5, retry: true })))
      .toBe("長い本文を区切って読んでいます（2/5）。答えの形が崩れたので、頼み直しています");
  });
});
