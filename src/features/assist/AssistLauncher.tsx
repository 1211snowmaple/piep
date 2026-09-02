import { ActionIcon, Badge, Menu, Text, Tooltip } from "@mantine/core";
import { useAppNavigate } from "@/app/router";
import { Icons, IconSize } from "@/lib/icons";

export interface AssistLauncherItem {
  id: string;
  label: string;
  description?: string;
  enabled: boolean;
  unavailableReason?: string;
  onSelect: () => void;
  badge?: string;
}

/**
 * その画面で使える「手伝い」を一つの印に束ねる入口。
 *
 * モデルが未設定でも印は消さない。灰色のまま理由と設定への道を示すことで、
 * 機能の存在と現在使えない理由を同じ場所で確認できる。
 */
export function AssistLauncher({
  items,
  label = "この画面で使える手伝い",
  size = 36,
  placement = "local",
}: {
  items: AssistLauncherItem[];
  label?: string;
  size?: number | "sm" | "md" | "lg";
  placement?: "local" | "header";
}) {
  const navigate = useAppNavigate();
  const available = items.some((item) => item.enabled);

  return (
    <Menu position="bottom-end" width={320} withinPortal>
      <Menu.Target>
        <Tooltip label={label}>
          <ActionIcon
            variant={placement === "header" ? "subtle" : "default"}
            color={available ? "grape" : "gray"}
            size={size}
            aria-label={label}
            data-assist-ready={available || undefined}
            data-placement={placement}
          >
            <Icons.optimize
              size={IconSize.action}
              fill={placement === "header" && !available ? "currentColor" : "none"}
            />
          </ActionIcon>
        </Tooltip>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>{label}</Menu.Label>
        {items.length === 0 && (
          <Menu.Item disabled>
            <Text size="sm" fw={650}>この画面で使える機能はありません</Text>
            <Text size="xs" c="dimmed">対応する画面では、ここに使える手伝いが並びます</Text>
          </Menu.Item>
        )}
        {items.map((item) => (
          <Menu.Item
            key={item.id}
            leftSection={<Icons.optimize size={IconSize.menu} />}
            rightSection={item.badge ? <Badge size="xs" variant="light" color="gray">{item.badge}</Badge> : undefined}
            disabled={!item.enabled}
            onClick={item.onSelect}
          >
            <Text size="sm" fw={650}>{item.label}</Text>
            <Text size="xs" c="dimmed">
              {item.enabled ? item.description : item.unavailableReason ?? "設定でこの機能を有効にしてください"}
            </Text>
          </Menu.Item>
        ))}
        <Menu.Divider />
        <Menu.Item leftSection={<Icons.settings size={IconSize.menu} />} onClick={() => navigate("/settings?section=assist")}>
          AIと意味検索の設定
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  );
}
