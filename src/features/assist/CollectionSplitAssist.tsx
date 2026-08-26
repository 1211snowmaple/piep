import { useEffect, useState } from "react";
import { Alert, Badge, Button, Group, Loader, Modal, Paper, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation } from "@tanstack/react-query";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { useAssist } from "@/features/assist/useAssist";
import { proposeSplits, type BundleSplit } from "@/services/assistApi";
import type { WorkCollection } from "@/types/collections";

/**
 * この束を分けたほうがよいか、見てもらう。
 *
 * **分けるのは利用者。** ここは案を出すだけで、押しても束は変わらない。
 * 「分ける必要は無い」が返ってくるのも正しい答えで、そのときはそう出す。
 *
 * 束を開くたびに常に見えている必要は無い機能なので、覆いの中に置く。
 * 画面に常駐させていた頃は、案が出ると本題である束の中身が画面の外へ
 * 押し出され、しかも一度出した案をしまう道が無かった。
 */
export function CollectionSplitAssist({
  opened,
  onClose,
  collection,
  onSplit,
}: {
  opened: boolean;
  onClose: () => void;
  collection: WorkCollection;
  /** 選んだ塊で新しい束を作る。位置は一覧の並び順（0 始まり）。 */
  onSplit: (name: string, positions: number[]) => void;
}) {
  const { engine } = useAssist();
  const [splits, setSplits] = useState<BundleSplit[] | null>(null);

  // 開き直したら白紙から。前に開いたときの案が、いまの中身の案に見えてしまう。
  useEffect(() => {
    if (!opened) setSplits(null);
  }, [opened]);

  const run = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return proposeSplits(engine, collection.id);
    },
    onSuccess: setSplits,
    onError: (error) => notifications.show({ color: "red", title: "案を出せません", message: errorMessage(error) }),
  });

  return (
    <Modal opened={opened} onClose={onClose} title="分けたほうがよいか見てもらう" size="lg">
      <Stack>
        <Text size="sm" c="dimmed">
          題名とタグから、二つ以上に分けたほうが読みやすいかを見ます。
          分けるのは利用者で、ここは案を出すだけです。
        </Text>

        {engine ? (
          <Button
            variant="light"
            color="grape"
            leftSection={<Icons.optimize size={IconSize.action} />}
            loading={run.isPending}
            onClick={() => run.mutate()}
          >
            {splits ? "もう一度見てもらう" : "見てもらう"}
          </Button>
        ) : (
          <Alert color="gray" icon={<Icons.info size={IconSize.action} />}>
            <Text size="sm">
              設定の「AIの手伝い」でつなぎ先を選ぶと、ここで案を出せます。
            </Text>
          </Alert>
        )}

        {run.isPending && (
          <Group gap={6}>
            <Loader size="xs" />
            <Text size="xs" c="dimmed">考えてもらっています…</Text>
          </Group>
        )}

        {splits && splits.length === 0 && (
          <Text size="sm" c="dimmed">このままで良さそうです。</Text>
        )}

        {splits && splits.length > 0 && (
          <Stack gap={6}>
            {splits.map((split) => (
              <Paper key={split.name} withBorder p="sm">
                <Group justify="space-between" align="flex-start" wrap="nowrap">
                  <div>
                    <Group gap={6}>
                      <Text size="sm" fw={650}>{split.name}</Text>
                      <Badge size="xs" variant="light">{formatNumber(split.positions.length)}作品</Badge>
                    </Group>
                    <Text size="xs" c="dimmed">{split.reason}</Text>
                    {/* 題名は何が入るかの見当を付けるためだけ。全部読ませる場所ではない。 */}
                    <Text size="xs" c="dimmed" className="line-clamp-1">
                      {split.positions
                        .map((position) => collection.members[position]?.title)
                        .filter(Boolean)
                        .join(" / ")}
                    </Text>
                  </div>
                  <Button
                    size="compact-xs"
                    variant="light"
                    onClick={() => { onSplit(split.name, split.positions); onClose(); }}
                  >
                    この束を作る
                  </Button>
                </Group>
              </Paper>
            ))}
            <Text size="xs" c="dimmed">
              新しい束を作っても、<b>元の束はそのまま残ります。</b>要らなくなったら手で外してください。
            </Text>
          </Stack>
        )}
      </Stack>
    </Modal>
  );
}
