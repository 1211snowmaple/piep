import { Box, Card, Group, NumberInput, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { shelfRepairAttemptLimit, shelfSetRepairAttemptLimit } from "@/services/shelfApi";

const QUERY_KEY = ["shelf", "repair-attempt-limit"] as const;

/**
 * 欠けたまま保存した作品（見られなかった挿絵・伏せられた作品情報・取れなかった添付）を、
 * 同じ状態で何回まで取りに行くか。棚の台帳に置く設定で、触ったその場で書く。
 *
 * 入力の幅は決めておき、読み込み中も同じ場所に同じ大きさで出す（中身を動かさない）。
 */
export function RepairAttemptLimitCard({ runtime }: { runtime: boolean }) {
  const client = useQueryClient();
  const limit = useQuery({ queryKey: QUERY_KEY, queryFn: shelfRepairAttemptLimit, enabled: runtime });
  const save = useMutation({
    mutationFn: (value: number) => shelfSetRepairAttemptLimit(value),
    onSuccess: (next) => client.setQueryData(QUERY_KEY, next),
    onError: (error) => notifications.show({ color: "red", title: "設定を保存できません", message: errorMessage(error) }),
  });
  const current = limit.data;
  return (
    <Card p="lg">
      <Group justify="space-between" align="flex-start" wrap="nowrap">
        <Box miw={0}>
          <Text fw={700} size="sm">欠けた作品を取り直す回数</Text>
          <Text size="xs" c="dimmed" mt={4}>
            挿絵や添付が取れなかった作品、作品情報が伏せられていた作品を、同じ状態のまま何回まで取りに行くかです。最初の保存も1回に数えます。作品が更新されると数え直します。
          </Text>
        </Box>
        <NumberInput
          aria-label="欠けた作品を取り直す回数"
          w={96}
          style={{ flexShrink: 0 }}
          suffix=" 回"
          hideControls
          min={current?.min ?? 1}
          max={current?.max ?? 5}
          allowDecimal={false}
          allowNegative={false}
          clampBehavior="strict"
          value={current?.value ?? ""}
          disabled={!runtime || !current || save.isPending}
          onChange={(value) => {
            if (typeof value === "number" && current && value !== current.value && value >= current.min && value <= current.max) save.mutate(value);
          }}
        />
      </Group>
    </Card>
  );
}
