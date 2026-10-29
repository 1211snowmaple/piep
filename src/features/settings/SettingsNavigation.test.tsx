import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { Icons } from "@/lib/icons";
import { SettingsNavigation } from "./SettingsNavigation";

it("finds a setting by its purpose and keeps the navigation keyboard operable", () => {
  const select = vi.fn();
  render(<MantineProvider><SettingsNavigation active="connections" onSelect={select} items={[
    { id: "connections", label: "サービス接続", description: "アカウント", group: "保存する", icon: Icons.credentials },
    { id: "library", label: "保存とバックアップ", description: "保存先・復元", group: "保存する", icon: Icons.database },
    { id: "appearance", label: "表示と操作", description: "テーマ", keywords: "ダーク", group: "使いやすくする", icon: Icons.appearance },
  ]} /></MantineProvider>);
  fireEvent.change(screen.getByRole("textbox", { name: "設定項目を検索" }), { target: { value: "復元" } });
  expect(screen.queryByRole("button", { name: /サービス接続/ })).toBeNull();
  const backup = screen.getByRole("button", { name: /保存とバックアップ/ });
  expect(backup.tagName).toBe("BUTTON");
  fireEvent.click(backup);
  expect(select).toHaveBeenCalledWith("library");
  expect(screen.getByRole("textbox", { name: "設定項目を検索" })).toHaveValue("");
  expect(screen.getByRole("button", { name: /表示と操作/ })).toBeVisible();
});
