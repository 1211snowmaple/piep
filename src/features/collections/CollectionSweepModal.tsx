import { useState } from "react";
import { Alert, Button, Group, Modal, Text } from "@mantine/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { listCollectionSuggestions, sweepCollectionCandidates } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { demoSuggestions } from "@/mocks/demoData";
import type { SavedSearchSuggestion } from "@/types/collections";
import { SuggestionInbox } from "./SuggestionInbox";
import "./discovery.css";

/** 閉じる操作は確認の中断。候補も、作品選択・読む順の下書きも消さない。 */
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

  return <Modal opened={opened} onClose={onClose} title="まとまりを探す" size="min(1180px, calc(100vw - 32px))" xOffset={12} yOffset={20} keepMounted closeButtonProps={{ "aria-label": "まとまりを探すを閉じる" }} classNames={{ content: "discovery-modal", body: "discovery-modal__body", header: "discovery-modal__header" }}>
    <Group className="discovery-toolbar" justify="space-between" align="center" gap="sm"><Text size="sm" c="dimmed">作品を見比べて、読む順を決めてからコレクションに。</Text><Button variant={(pending.data?.length ?? 0) > 0 ? "default" : "filled"} size="sm" leftSection={<Icons.search size={IconSize.menu} />} loading={sweep.isPending} disabled={reviewBusy} onClick={() => sweep.mutate()}>{(pending.data?.length ?? 0) > 0 ? "棚を探し直す" : "棚から探す"}</Button></Group>
    {sweep.error && <Alert color="red" title="棚を調べられません">{errorMessage(sweep.error)}</Alert>}
    <SuggestionInbox sweeping={sweep.isPending} savedSearchIdeas={savedSearchIdeas} note={note} onBusyChange={setReviewBusy} />
    <div className="discovery-modal__footnote"><Text size="xs" c="dimmed">閉じても候補は残ります。いつでも続きを確認できます。</Text><Button variant="subtle" color="gray" size="compact-sm" onClick={onClose}>閉じる</Button></div>
  </Modal>;
}
