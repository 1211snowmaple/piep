import { useState } from "react";
import { Button, Group, Modal, Stack } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { dismissSweptSuggestions, listCollectionSuggestions, sweepCollectionCandidates } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { demoSuggestions } from "@/mocks/demoData";
import type { SavedSearchSuggestion } from "@/types/collections";
import { SuggestionInbox } from "./SuggestionInbox";

/**
 * 棚から、まとまりになりそうなものを探す。
 *
 * 「名前を付け直す」と同じ形にした。棚の一覧の途中に結果を差し込むと、
 * 候補が出ているあいだずっとコレクションそのものが下へ押し出される。しかも
 * 候補は**片付けるもの**なので、いつまでも棚の上に居座る種類の情報ではない。
 *
 * 操作は二つだけ置く。**更新**と**すべて閉じる**である。
 *
 * 走査は毎回、確度を重みにした籤で選び直す。だから「更新」は同じものを取り
 * 直す操作ではなく、**別の顔ぶれを引き直す**操作になる。これがこの窓の主な
 * 動詞なので、いちばん押しやすいところへ置く。
 *
 * 「続き物だけ閉じる」「テーマだけ閉じる」はやめた。系統を選んで閉じたい人は
 * いない。畳んだメニューの中に一つだけ使う項目を隠していたことになる。
 */
export function CollectionSweepModal({ opened, onClose }: {
  opened: boolean;
  onClose: () => void;
}) {
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [savedSearchIdeas, setSavedSearchIdeas] = useState<SavedSearchSuggestion[]>([]);
  const [note, setNote] = useState<string | null>(null);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] });
    queryClient.invalidateQueries({ queryKey: ["work-collections"] });
  };

  const pending = useQuery({
    queryKey: ["collection-suggestions", "pending"],
    queryFn: () => (runtime ? listCollectionSuggestions("pending") : Promise.resolve(demoSuggestions)),
  });
  const pendingCount = pending.data?.length ?? 0;

  const sweep = useMutation({
    mutationFn: sweepCollectionCandidates,
    onSuccess: (result) => {
      invalidate();
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

  // 確認の窓は出さない。
  //
  // 消えるのは下書きだけで、「二度と出さない」とは記録しない。しかも取り戻す
  // 操作（更新）が同じ画面の隣にある。**一手で戻せるものに確認を挟むと、
  // 確認のほうが高くつく。**
  const dismissAll = useMutation({
    mutationFn: () => dismissSweptSuggestions(),
    onSuccess: (removed) => {
      invalidate();
      setSavedSearchIdeas([]);
      notifications.show({ message: `${formatNumber(removed)}件の候補を閉じました。「更新」でまた探せます` });
    },
    onError: (error) => notifications.show({ color: "red", title: "候補を閉じられません", message: errorMessage(error) }),
  });

  const busy = sweep.isPending || dismissAll.isPending;

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="まとまりを探す"
      size="xl"
      className="sweep-modal"
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
        {/* 操作は下に貼り付けておく。候補は縦に長いので、下まで送らないと
            押せない操作は「使いにくい」ではなく「無い」に等しい。 */}
        <Group className="overlay-actions" justify="space-between" wrap="nowrap">
          <Button
            variant="subtle"
            color="gray"
            leftSection={<Icons.hide size={IconSize.action} />}
            disabled={pendingCount === 0 || busy}
            loading={dismissAll.isPending}
            onClick={() => dismissAll.mutate()}
          >
            すべて閉じる
          </Button>
          <Group gap="xs" wrap="nowrap">
            <Button variant="default" onClick={onClose} disabled={sweep.isPending}>閉じる</Button>
            <Button
              leftSection={<Icons.retry size={IconSize.action} />}
              loading={sweep.isPending}
              disabled={dismissAll.isPending}
              onClick={() => sweep.mutate()}
            >
              {pendingCount === 0 ? "棚から探す" : "更新"}
            </Button>
          </Group>
        </Group>
      </Stack>
    </Modal>
  );
}
