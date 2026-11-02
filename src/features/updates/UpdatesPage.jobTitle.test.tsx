import { MantineProvider } from "@mantine/core";
import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { JobActivityTitle } from "./UpdatesPage";

it("keeps the whole title readable and the height steady while a job runs", () => {
  const title = "とても長い題名がそのまま読める";
  const page = render(<MantineProvider><JobActivityTitle label={title} reserveLines /></MantineProvider>);
  const heading = screen.getByRole("heading", { name: title });
  expect(heading).toHaveAttribute("title", title);
  expect(heading).toHaveClass("line-clamp-2", "update-job-title--reserved");

  page.rerender(<MantineProvider><JobActivityTitle label="完了しました" reserveLines={false} /></MantineProvider>);
  expect(screen.getByRole("heading", { name: "完了しました" })).not.toHaveClass("update-job-title--reserved");
});
