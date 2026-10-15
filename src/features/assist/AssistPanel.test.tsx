import { MantineProvider } from "@mantine/core";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AssistNoteBody } from "@/features/assist/AssistPanel";

describe("source extract notes", () => {
  it.each(["work_synopsis/v9", "reader_recap/v9", "work_synopsis/v10", "reader_recap/v10", "work_synopsis/v11", "reader_recap/v11"])("keeps source gaps visible for %s", (promptVersion) => {
    const text = "第一企画。\n\n［原文の途中を省略］\n\n第三企画の目的。";
    render(<MantineProvider><AssistNoteBody text={text} promptVersion={promptVersion} /></MantineProvider>);
    const content = screen.getByText((_, element) => element?.textContent === text);
    expect(content).toHaveStyle({ whiteSpace: "pre-wrap" });
    expect(screen.getByText("原文から選んだ抜粋です。省略箇所の前後がつながるとは限らず、要点や結末を網羅していません。")).toBeInTheDocument();
  });

  it.each(["work_synopsis/v11", "reader_recap/v11"])("explains the separate source tail for %s", (promptVersion) => {
    const tail = "最後の行動。 \n\t";
    render(<MantineProvider><AssistNoteBody text="選択された原文。" sourceTail={tail} promptVersion={promptVersion} /></MantineProvider>);
    expect(screen.getByText("選択された原文。")).toBeVisible();
    const details = screen.getByText("本文末尾の原文を確認").closest("details");
    expect(details).not.toHaveAttribute("open");
    const source = screen.getByText((_, element) => element?.textContent === tail);
    expect(source).toHaveStyle({ whiteSpace: "pre-wrap" });
    expect(source).not.toBeVisible();
    details?.setAttribute("open", "");
    expect(source).toBeVisible();
    expect(screen.getByText("本文末尾の原文は確認用です。重要箇所や結末とは限りません。")).toBeInTheDocument();
  });

  it.each(["work_synopsis/v10", "work_synopsis/v11"])("does not parse source labels in %s", (promptVersion) => {
    const marker = "\n\n［本文の末尾・確認用の原文］\n\n";
    const text = `元本文${marker}本文内の欄名`;
    const { rerender } = render(<MantineProvider><AssistNoteBody text={text} promptVersion={promptVersion} /></MantineProvider>);
    expect(screen.queryByText("本文末尾の原文を確認")).not.toBeInTheDocument();
    expect(screen.getByText((_, element) => element?.textContent === text)).toBeVisible();
    rerender(<MantineProvider><AssistNoteBody text={text} sourceTail={`本物の確認欄${marker}原文にある同じ欄名`} promptVersion={promptVersion} /></MantineProvider>);
    expect(screen.getByText((_, element) => element?.textContent === text)).toBeVisible();
    expect(screen.getByText((_, element) => element?.textContent === `本物の確認欄${marker}原文にある同じ欄名`)).not.toBeVisible();
  });

  it.each([undefined, null, ""])("hides absent source context %s", (sourceTail) => {
    render(<MantineProvider><AssistNoteBody text="原文" sourceTail={sourceTail} promptVersion="work_synopsis/v11" /></MantineProvider>);
    expect(screen.queryByText("本文末尾の原文を確認")).not.toBeInTheDocument();
    expect(screen.getByText("原文")).toBeVisible();
  });

  it("does not label older free text or tag statistics as source excerpts", () => {
    const { rerender } = render(<MantineProvider><AssistNoteBody text="以前のメモ" promptVersion="work_synopsis/v1" /></MantineProvider>);
    expect(screen.queryByText(/原文から選んだ抜粋です/)).not.toBeInTheDocument();
    rerender(<MantineProvider><AssistNoteBody text="タグの集計" modelId="piep:tag-statistics" promptVersion="author_style/v6" /></MantineProvider>);
    expect(screen.queryByText(/原文から選んだ抜粋です/)).not.toBeInTheDocument();
    expect(screen.getByText("対象作品のタグ集計です · 集計条件 author_style/v6")).toBeInTheDocument();
  });
});
