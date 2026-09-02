import { useState } from "react";
import { Button, Group, Modal, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { sweepCollectionCandidates } from "@/services/collectionApi";
import type { SavedSearchSuggestion } from "@/types/collections";
import { SuggestionInbox } from "./SuggestionInbox";

/**
 * 棚から、まとまりになりそうなものを探す。
 *
 * 「名前を付け直す」と同じ形にした。棚の一覧の途中に結果を差し込むと、
 * 候補が出ているあいだずっとコレクションそのものが下へ押し出される。しかも
 * 候補は**片付けるもの**なので、いつまでも棚の上に居座る種類の情報ではない。
 *
 * 始めて、見て、採るか閉じるかして、終わる。その一続きが一枚の上で完結する
 * なら、それはオーバーレイである。
 */
export function CollectionSweepModal({ opened, onClose }: {
  opened: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [savedSearchIdeas, setSavedSearchIdeas] = useState<SavedSearchSuggestion[]>([]);
  const [note, setNote] = useState<string | null>(null);

  const sweep = useMutation({
    mutationFn: sweepCollectionCandidates,
    onSuccess: (result) => {
      queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] });
      queryClient.invalidateQueries({ queryKey: ["work-collections"] });
      setSavedSearchIdeas(result.savedSearchSuggestions);
      // 索引が読めなかったことを、結果の一部として持ち帰る。トーストだけに
      // すると、読む前に消える。
      setNote(result.note);
      notifications.show({
        color: result.bundles.length > 0 ? "green" : "gray",
        message: result.bundles.length > 0
          ? `${formatNumber(result.bundles.length)}件のまとまりが見つかりました`
          : "新しいまとまりは見つかりませんでした",
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "棚を走査できません", message: errorMessage(error) }),
  });

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="まとまりを探す"
      size="xl"
      // 走査の途中で閉じると、結果の行き場が無くなる。押した操作が終わる
      // までは、外側を押しても Esc でも閉じない。
      closeOnClickOutside={!sweep.isPending}
      closeOnEscape={!sweep.isPending}
      withCloseButton={!sweep.isPending}
    >
      <Stack gap="md">
        <SuggestionInbox
          sweeping={sweep.isPending}
          savedSearchIdeas={savedSearchIdeas}
          note={note}
        />
        <Group justify="space-between" wrap="nowrap">
          <Text size="xs" c="dimmed">
            採用するまで、棚のコレクションは変わりません。
          </Text>
          <Group gap="xs" wrap="nowrap">
            <Button variant="default" onClick={onClose} disabled={sweep.isPending}>閉じる</Button>
            <Button
              leftSection={<Icons.search size={IconSize.action} />}
              loading={sweep.isPending}
              onClick={() => sweep.mutate()}
            >
              {sweep.isSuccess ? "もう一度探す" : "棚から探す"}
            </Button>
          </Group>
        </Group>
      </Stack>
    </Modal>
  );
}
