import { memo } from "react";
import { Badge, Card, Checkbox, Group, Loader, Stack, Text, ThemeIcon } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import type { SidebarItem } from "@/features/browser/downloadCandidates";

/**
 * 候補の一行。**メモ化しておく。**
 *
 * 保存は1件ごとに `items` を組み直すので、素のままだと**そのたびに全行が
 * 描き直される**。818件の保存なら 818行 x 2回 x 818件で百万回を超え、
 * その確保が積み上がって画面のプロセスがメモリ不足で落ちた（実際に落ちた）。
 * 変わるのは1行だけなので、他の行は前の結果を使い回す。
 *
 * そのためには押したときの関数も毎回作り直さないことが要る。作り直すと
 * props が毎回変わり、メモ化は素通しになる。
 */
export const CandidateRow = memo(function CandidateRow({
  item,
  disabled,
  onToggle,
}: {
  item: SidebarItem;
  disabled: boolean;
  onToggle: (id: string) => void;
}) {
  const status = item.status;
  return (
    <Card
      p="sm"
      className="candidate-row"
      data-selected={(item.selected && status !== "held") || undefined}
    >
      <Group wrap="nowrap" align="flex-start">
        <Checkbox
          checked={item.selected && status !== "held"}
          disabled={disabled || status === "success" || status === "skipped" || status === "held"}
          onChange={() => onToggle(item.id)}
          aria-label={`${item.title}を保存対象にする`}
          mt={3}
        />
        <Stack gap={3} flex={1} miw={0}>
          <Text size="sm" fw={650} className="line-clamp-2">
            {item.title}
          </Text>
          {item.subtitle && (
            <Text size="xs" c="dimmed" className="line-clamp-1">
              {item.subtitle}
            </Text>
          )}
          {item.error && (
            <Text size="xs" c="red" className="line-clamp-3">
              {item.error}
            </Text>
          )}
          <Text size="10px" c="dimmed">
            ID {item.id}
          </Text>
        </Stack>
        <StatusIcon status={status} />
      </Group>
    </Card>
  );
});

function StatusIcon({ status }: { status: SidebarItem["status"] }) {
  if (status === "held") return <Badge color="yellow" size="xs">保留</Badge>;
  if (status === "downloading") return <Loader size="xs" />;
  if (status === "success")
    return (
      <ThemeIcon color="green" size="sm" radius="xl">
        <Icons.confirm size={IconSize.inline} />
      </ThemeIcon>
    );
  if (status === "skipped")
    return (
      <Badge color="gray" size="xs">
        保存済み
      </Badge>
    );
  if (status === "failed")
    return (
      <ThemeIcon color="red" size="sm" radius="xl">
        <Icons.cancel size={IconSize.inline} />
      </ThemeIcon>
    );
  return null;
}

export function EmptyCandidates({ supportedPages }: { supportedPages: string }) {
  return (
    <Stack align="center" ta="center" py={48} px="md">
      <ThemeIcon size={48} radius="xl" variant="light" color="gray">
        <Icons.link size={IconSize.feature} />
      </ThemeIcon>
      <Text size="sm" fw={700}>
        ページを開いて候補を取得
      </Text>
      <Text size="xs" c="dimmed">
        {supportedPages}に対応しています。
      </Text>
    </Stack>
  );
}
