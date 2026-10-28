import { Badge, Popover, Text, UnstyledButton } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { errorMessage } from "@/lib/format";

/** Compact status in every update list; the full diagnostic stays available. */
export function UpdateStatusBadge({ label, color, detail, title }: {
  label: string; color: string; detail?: string | null; title: string;
}) {
  const badge = <Badge color={color} variant="light" rightSection={detail ? <Icons.info size={IconSize.inline} /> : undefined} style={{ flex: "none", cursor: detail ? "pointer" : undefined }}>{label}</Badge>;
  if (!detail) return badge;
  return <Popover width={360} position="bottom-end" withArrow shadow="md" trapFocus returnFocus>
    <Popover.Target><UnstyledButton aria-label={`${title}の状態の詳細`} style={{ flex: "none" }}>{badge}</UnstyledButton></Popover.Target>
    <Popover.Dropdown maw="calc(100vw - 32px)" style={{ maxHeight: "min(50vh, 320px)", overflowY: "auto" }}><Text size="sm" style={{ overflowWrap: "anywhere" }}>{errorMessage(detail)}</Text></Popover.Dropdown>
  </Popover>;
}
