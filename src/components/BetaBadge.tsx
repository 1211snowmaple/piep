import { Badge, type BadgeProps } from "@mantine/core";

/**
 * β版の機能に付ける印。どの画面でもこれを使い、色と字形を揃える。
 * Mantine の Badge は既定で大文字にするので、`tt` を外すと β が B に見える。
 */
export function BetaBadge({ size = "xs" }: { size?: BadgeProps["size"] }) {
  return <Badge size={size} variant="light" color="yellow" tt="none">β</Badge>;
}
