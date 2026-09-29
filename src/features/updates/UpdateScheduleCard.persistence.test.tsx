import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

const storage = vi.hoisted(() => ({
  get: vi.fn(),
  set: vi.fn(),
  save: vi.fn(),
}));

vi.mock("@/store", () => ({ store: storage }));
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));

import { UpdateScheduleCard } from "./UpdateScheduleCard";
import { saveSchedule, updateScheduleDefaults } from "./updateSchedule";

beforeEach(() => {
  vi.clearAllMocks();
  storage.get.mockResolvedValue(updateScheduleDefaults);
  storage.set.mockResolvedValue(undefined);
  storage.save.mockRejectedValue(new Error("disk unavailable"));
});

it("restores the in-memory setting when writing the settings file fails", async () => {
  await expect(saveSchedule({ ...updateScheduleDefaults, onStartup: true })).rejects.toThrow("disk unavailable");
  expect(storage.set).toHaveBeenNthCalledWith(1, "update_schedule", { ...updateScheduleDefaults, onStartup: true });
  expect(storage.set).toHaveBeenNthCalledWith(2, "update_schedule", updateScheduleDefaults);
});

it("keeps the previous switch value and summary when saving fails", async () => {
  const onChanged = vi.fn();
  render(<MantineProvider><UpdateScheduleCard onChanged={onChanged} /></MantineProvider>);
  const startup = screen.getByRole("switch", { name: /起動時に確認する/ });
  await waitFor(() => expect(startup).toBeEnabled());
  fireEvent.click(startup);
  await waitFor(() => expect(storage.save).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(startup).toBeEnabled());
  expect(startup).not.toBeChecked();
  expect(screen.getByText("自動では実行しません")).toBeInTheDocument();
  expect(onChanged).not.toHaveBeenCalled();
});
