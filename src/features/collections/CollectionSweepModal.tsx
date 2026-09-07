import { useState } from "react";
import { Alert, Button, Group, Modal, Text } from "@mantine/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { dismissSweptSuggestions, listCollectionSuggestions, sweepCollectionCandidates } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { demoSuggestions } from "@/mocks/demoData";
import type { SavedSearchSuggestion } from "@/types/collections";
import { SuggestionInbox } from "./SuggestionInbox";
import "./discovery.css";

/** 閉じたら、確認しなかった候補は捨てる。
 *
 * **残しておくと、次に開いたときも同じ顔ぶれが並ぶ。** 走査は同じ棚から同じ規則で
 * 探すので、貯めても増えるのは「見送ったもの」だけだった。捨てるのは走査で
 * 作ったものに限り、否定は記録しない（`dismiss_swept_suggestions` は消すだけで、
 * 「同じ組合せを二度と出さない」とは覚えない）。だから次の走査でまた出てくる。 */
export function CollectionSweepModal({ opened, onClose }: { opened: boolean; onClose: () => void }) {
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [savedSearchIdeas, setSavedSearchIdeas] = useState<SavedSearchSuggestion[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [reviewBusy, setReviewBusy] = useState(false);
  const pending = useQuery({ queryKey: ["collection-suggestions", "pending"], queryFn: () => runtime ? listCollectionSuggestions("pending") : Promise.resolve(demoSuggestions) });
  const sweep = useMutation({
    mutationFn: () => runtime ? sweepCollectionCandidates() : Promise.resolve({ bundles: demoSuggestions, savedSearchSuggestions: [], semanticUsed: true, note: null }),
    onSuccess: (result) => { queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] }); setSavedSearchIdeas(result.savedSearchSuggestions); setNote(result.note); },
  });

  const close = () => {
    onClose();
    if (!runtime) return;
    // 閉じる操作を待たせない。消え終わってから一覧を組み直す。
    void dismissSweptSuggestions()
      .then(() => queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] }))
      .catch(() => undefined);
  };

  return <Modal opened={opened} onClose={close} title="まとまりを探す" size="min(1040px, calc(100vw - 64px))" xOffset={12} yOffset={16} keepMounted closeButtonProps={{ "aria-label": "まとまりを探すを閉じる" }} classNames={{ content: "discovery-modal", body: "discovery-modal__body", header: "discovery-modal__header" }}>
    <Group className="discovery-toolbar" justify="space-between" align="center" gap="sm"><Text size="sm" c="dimmed">作品を見比べて、読む順を決めてからコレクションに。</Text><Button variant={(pending.data?.length ?? 0) > 0 ? "default" : "filled"} size="sm" leftSection={<Icons.search size={IconSize.menu} />} loading={sweep.isPending} disabled={reviewBusy} onClick={() => sweep.mutate()}>{(pending.data?.length ?? 0) > 0 ? "棚を探し直す" : "棚から探す"}</Button></Group>
    {sweep.error && <Alert color="red" title="棚を調べられません">{errorMessage(sweep.error)}</Alert>}
    <SuggestionInbox sweeping={sweep.isPending} savedSearchIdeas={savedSearchIdeas} note={note} onBusyChange={setReviewBusy} />
    <div className="discovery-modal__footnote"><Text size="xs" c="dimmed">閉じると、確認しなかった候補は捨てます。次に開いたときはまた探せます。</Text><Button variant="subtle" color="gray" size="compact-sm" onClick={close}>閉じる</Button></div>
  </Modal>;
}
