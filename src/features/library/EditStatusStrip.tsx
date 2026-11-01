import { Alert, Badge, Button, Group, Paper, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { formatDate } from "@/lib/format";
import type { WorkEditRevision } from "@/types/library";

/**
 * いま見えている本文が、取り込んだままか、編集版か、それとも書きかけが
 * 別にあるのかを、本文のすぐ上で名乗る。
 *
 * 編集画面の「反映」を押すと本文が切り替わるが、本文タブは何も言わずに
 * 新しい本文を出していた。自動保存が作った下書きは反映するまで使われないのに、
 * それもどこにも出ていなかった。直したのか、直っていないのか、が見えなかった。
 */
export function EditStatusStrip({
  activeRevision,
  draftRevision,
  onEdit,
  onCompare,
}: {
  activeRevision: WorkEditRevision | null;
  draftRevision?: WorkEditRevision | null;
  onEdit: () => void;
  onCompare?: () => void;
}) {
  if (!activeRevision && !draftRevision) return null;
  return (
    <>
      {activeRevision && (
        <Paper className="edit-status-strip" withBorder px="sm" py={6}>
          <Group justify="space-between" wrap="wrap" gap="xs">
            <Group gap="xs" wrap="nowrap" miw={0}>
              <Badge color="blue" variant="light">編集版</Badge>
              <Text size="xs" c="dimmed">
                v{activeRevision.baseVersion} をもとに編集した本文です（{formatDate(activeRevision.updatedAt, true)}に反映）。取り込んだ本文はそのまま残っています。
              </Text>
            </Group>
            {onCompare && <Button size="compact-xs" variant="subtle" leftSection={<Icons.versionHistory size={IconSize.inline} />} onClick={onCompare}>元の本文と比べる</Button>}
          </Group>
        </Paper>
      )}
      {draftRevision && (
        <Alert className="edit-status-strip" color="yellow" variant="light" p="xs" icon={<Icons.warning size={IconSize.menu} />}>
          <Group justify="space-between" wrap="wrap" gap="xs">
            <Text size="xs">
              {formatDate(draftRevision.updatedAt, true)}に保存した<strong>反映していない下書き</strong>があります。読書画面とEPUBにはまだ使われていません。
            </Text>
            <Button size="compact-xs" variant="light" color="yellow" leftSection={<Icons.edit size={IconSize.inline} />} onClick={onEdit}>編集画面で反映する</Button>
          </Group>
        </Alert>
      )}
    </>
  );
}
