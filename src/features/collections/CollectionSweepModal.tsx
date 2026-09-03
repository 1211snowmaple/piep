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
 * ## 押すものは一つ
 *
 * 走査は毎回、確度を重みにした籤で選び直す。だから「更新」は同じものを取り
 * 直す操作ではなく、**別の顔ぶれを引き直す**操作になる。これがこの窓で唯一の
 * 動詞なので、右下に一つだけ置く。
 *
 * ## 閉じることが、片付けることである
 *
 * 「すべて閉じる」という別のボタンは置かない。候補は下書きで、閉じるとは
 * **見終わったということ**である。それを二つの操作に分けると、窓を閉じたのに
 * 数字だけが棚の入口に残る。
 *
 * だから ✕ を押すと、出ていた候補はそのまま片付く。消えるのは下書きだけで、
 * 「二度と出さない」とは記録しない。もう一度開いて「更新」を押せば、また
 * 探しに行く（そして籤なので、別の顔ぶれが出る）。
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

  const dismissAll = useMutation({
    mutationFn: () => dismissSweptSuggestions(),
    onSuccess: (removed) => {
      invalidate();
      setSavedSearchIdeas([]);
      setNote(null);
      if (removed > 0) {
        notifications.show({ message: `${formatNumber(removed)}件の候補を片付けました。「更新」でまた探せます` });
      }
    },
    onError: (error) => notifications.show({ color: "red", title: "候補を片付けられません", message: errorMessage(error) }),
  });

  /**
   * 閉じると片付く。
   *
   * 先に窓を閉じてから頼む。片付けの往復を待たせると、押してから消えるまでの
   * あいだ窓が固まって見える。この部品は閉じても外れない（`opened` が false に
   * なるだけ）ので、答えは戻ってくる。
   */
  const closeAndClear = () => {
    onClose();
    if (pendingCount > 0) dismissAll.mutate();
  };

  return (
    <Modal
      opened={opened}
      onClose={closeAndClear}
      title="まとまりを探す"
      size="xl"
      className="sweep-modal"
      // 走査の途中で閉じると、結果の行き場が無くなる。押した操作が終わる
      // までは、外側を押しても Esc でも閉じない。
      closeOnClickOutside={!sweep.isPending}
      closeOnEscape={!sweep.isPending}
      withCloseButton={!sweep.isPending}
      // ✕ は閉じるだけの印ではなくなった。何が起きるかを名前で言う。
      // 読み上げで聞いている人には、この一行しか手がかりが無い。
      closeButtonProps={{ "aria-label": pendingCount > 0 ? "閉じて候補を片付ける" : "閉じる" }}
    >
      <Stack gap="md">
        <SuggestionInbox
          sweeping={sweep.isPending}
          savedSearchIdeas={savedSearchIdeas}
          note={note}
        />
        {/* 操作は下に貼り付ける。候補は縦に長いので、下まで送らないと押せない
            操作は「使いにくい」ではなく無いに等しい。 */}
        <Group className="overlay-actions" justify="flex-end" wrap="nowrap">
          <Button
            leftSection={<Icons.retry size={IconSize.action} />}
            loading={sweep.isPending}
            onClick={() => sweep.mutate()}
          >
            {pendingCount === 0 ? "棚から探す" : "更新"}
          </Button>
        </Group>
      </Stack>
    </Modal>
  );
}
