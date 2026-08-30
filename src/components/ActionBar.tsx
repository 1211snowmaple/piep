import type { ReactNode } from "react";
import { Button, Group, Tooltip } from "@mantine/core";
import { IconSize, type LucideIcon } from "@/lib/icons";

export interface ActionBarItem {
  key: string;
  label: string;
  icon: LucideIcon;
  onClick: () => void;
  /** 主操作はひとつだけ。既定は枠線だけの補助操作。 */
  primary?: boolean;
  loading?: boolean;
  disabled?: boolean;
}

/**
 * 詳細見出しに付随する操作の列。
 *
 * 折り返さない。同じ性格の操作が2行に割れると、1つだけ取り残されたものが
 * 別の何かに見える。幅が足りないときはボタンの文字だけを畳み、押せる場所は
 * 減らさない - 名前はツールチップと読み上げ（aria-label）に残る。
 * 畳む境目は画面幅ではなく、この列が実際に置かれている場所の幅で決める。
 * サイドバーが開いているかどうかで、同じ画面幅でも余りが 130px 変わるため。
 */
export function ActionBar({ items, label, children }: { items: ActionBarItem[]; label: string; children?: ReactNode }) {
  return (
    <Group gap="xs" wrap="nowrap" className="action-bar" role="group" aria-label={label}>
      {items.map((item) => (
        <Tooltip key={item.key} label={item.label}>
          <Button
            className="action-bar__item"
            variant={item.primary ? "filled" : "default"}
            loading={item.loading}
            disabled={item.disabled}
            aria-label={item.label}
            leftSection={<item.icon size={IconSize.menu} />}
            onClick={item.onClick}
          >
            <span className="action-bar__label">{item.label}</span>
          </Button>
        </Tooltip>
      ))}
      {children}
    </Group>
  );
}
