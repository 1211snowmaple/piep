import { MantineProvider, Popover } from "@mantine/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it } from "vitest";
import { UpdateStatusBadge } from "./UpdateStatusBadge";

it("keeps long diagnostics out of the row and opens them from the status", async () => {
  render(<MantineProvider theme={{ components: { Popover: Popover.extend({ defaultProps: { hideDetached: false, transitionProps: { duration: 0 } } }) } }}><UpdateStatusBadge title="作品" label="見つからない" color="yellow" detail="保存済みの本文は保持されています" /></MantineProvider>);
  expect(screen.getByText("見つからない")).toBeVisible();
  expect(screen.queryByText("保存済みの本文は保持されています")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "作品の状態の詳細" }));
  await waitFor(() => expect(screen.getByText("保存済みの本文は保持されています")).toBeVisible());
});
