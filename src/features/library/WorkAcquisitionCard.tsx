import { Accordion, Alert, Anchor, Badge, Button, Group, Stack, Text } from "@mantine/core";
import { useMutation, useQuery } from "@tanstack/react-query";
import { notifications } from "@mantine/notifications";
import { ErrorState } from "@/components/AsyncState";
import { acquisitionStatusMeta } from "@/lib/acquisition";
import { errorMessage, formatDate } from "@/lib/format";
import { getWorkAcquisitionDetails, type MissingImageLocation } from "@/services/workInspectionApi";
import { recheckDeferredUpdateCandidatesCommand } from "@/services/updateJobApi";

export function WorkAcquisitionCard({ id, source, sourceId, runtime, onLocate }: {
  id: number; source: string; sourceId: string; runtime: boolean;
  onLocate: (location: MissingImageLocation, version: number) => void;
}) {
  const query = useQuery({ queryKey: ["work-acquisition", id], queryFn: () => getWorkAcquisitionDetails(id), enabled: runtime });
  const recheck = useMutation({
    mutationFn: () => recheckDeferredUpdateCandidatesCommand([`${source}:${sourceId}`], false),
    onSuccess: () => notifications.show({ color: "blue", message: "再確認を開始しました。完了すると取得状態が更新されます。" }),
    onError: (error) => notifications.show({ color: "red", title: "再確認を開始できません", message: errorMessage(error) }),
  });
  if (query.error) return <ErrorState error={query.error} retry={() => query.refetch()} />;
  const details = query.data;
  if (!details) return null;
  const status = acquisitionStatusMeta(details.status === "dismissed" ? "dismissed" : details.kind);
  return <Alert color={status.color} title={<Group gap="xs"><Text fw={700} size="sm">取得状態</Text><Badge size="xs" color={status.color}>{status.label}</Badge></Group>}>
    <Stack gap="xs">
      <Group justify="space-between" align="flex-start" wrap="nowrap">
        <Text size="sm">{details.kind === "incomplete" ? "本文は保存済みです。取得できなかった画像があります。" : details.reason}</Text>
        <Button size="xs" variant="subtle" loading={recheck.isPending} onClick={() => recheck.mutate()}>再確認</Button>
      </Group>
      <Accordion variant="default" chevronSize={14}>
        <Accordion.Item value="reason"><Accordion.Control>理由と本文の位置{details.missingImages.length ? `（${details.missingImages.length}か所）` : ""}</Accordion.Control><Accordion.Panel>
          <Stack gap="sm">
            <Text size="sm" style={{ overflowWrap: "anywhere" }}>{details.reason}</Text>
            <Text size="xs" c="dimmed">状態の記録: {formatDate(details.recordedAt, true)}</Text>
            {details.locationError && <Text size="xs" c="red">本文の位置を読み取れません: {details.locationError}</Text>}
            {details.missingImages.map((image, index) => <Stack gap={2} key={`${image.page}:${image.reference}:${image.occurrence}`}>
              <Anchor component="button" type="button" ta="left" size="sm" onClick={() => onLocate(image, details.version)}>本文 {image.page}ページ · {image.reference}</Anchor>
              <Text size="xs" c="dimmed" lineClamp={2}>{image.excerpt || `画像 ${index + 1}`}</Text>
            </Stack>)}
          </Stack>
        </Accordion.Panel></Accordion.Item>
      </Accordion>
    </Stack>
  </Alert>;
}
