import { useEffect, useState } from "react";
import { Alert, Badge, Button, Group, Modal, Stack, Text } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useMutation } from "@tanstack/react-query";
import { WorkCover } from "@/components/WorkCover";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { suggestCollectionAdditions } from "@/services/collectionApi";
import type { CollectionAdditionCandidate, WorkCollection } from "@/types/collections";

/**
 * すでにある束へ、あとから入れるとよさそうな作品を探す。
 *
 * コレクションは作った時点で閉じない。新作は毎日届くし、旧作をあとから
 * 保存することもある。**作ったときの顔ぶれに縛られる理由は無い** — 名前を
 * 付け直せるのと同じ話である。
 *
 * これまでは「作品を追加」から棚を自分でたどるしかなかった。3,900作の棚で
 * 「この束に足りないのはどれか」を人が思い出すのは、できない相談である。
 *
 * 出す件数は絞る。確かだと言える数作だけを出し、足りなければもう一度押す。
 * 選び方に乱れが入っているので、二度目には別の顔が出てくる。
 */
export function CollectionAdditionsModal({ opened, onClose, collection, busy, onAdd }: {
  opened: boolean;
  onClose: () => void;
  collection: WorkCollection;
  busy: boolean;
  onAdd: (downloadIds: number[]) => void;
}) {
  const [dropped, setDropped] = useState<ReadonlySet<number>>(new Set());

  const search = useMutation({
    mutationFn: () => suggestCollectionAdditions(collection.id),
    onSuccess: () => setDropped(new Set()),
    onError: (error) =>
      notifications.show({ color: "red", title: "候補を探せません", message: errorMessage(error) }),
  });

  // 開いたら探しに行く。「探す」と書かれたものを押して開いた窓で、もう一度
  // 「探す」を押させる理由が無い。閉じたら結果を捨てる — 次に開いたときは
  // 棚の中身が変わっているかもしれない。
  const { mutate: runSearch, reset: resetSearch } = search;
  useEffect(() => {
    if (opened) runSearch();
    else resetSearch();
  }, [opened, runSearch, resetSearch]);

  const result = search.data;
  const candidates = result?.candidates ?? [];
  const chosen = candidates
    .filter((candidate) => !dropped.has(candidate.downloadId))
    .map((candidate) => candidate.downloadId);

  const toggle = (downloadId: number) => setDropped((current) => {
    const next = new Set(current);
    if (next.has(downloadId)) next.delete(downloadId); else next.add(downloadId);
    return next;
  });

  return (
    <Modal
      opened={opened}
      onClose={onClose}
      title="この束に合う作品を探す"
      size="xl"
      closeOnClickOutside={!busy}
      closeOnEscape={!busy}
    >
      <Stack gap="md">
        <Text size="sm" c="dimmed">
          「{collection.name}」に入っている作品を手がかりに、まだ入っていない作品から
          合いそうなものを探します。押しても勝手には入りません。
        </Text>

        {search.isPending && (
          <Alert icon={<Icons.collectionSuggest size={IconSize.action} />}>
            本文のリンク、公式シリーズ、作者、題名、タグ、そして本文の近さを見ています。
          </Alert>
        )}

        {search.error && (
          <Alert color="red" title="候補を探せません">{errorMessage(search.error)}</Alert>
        )}

        {/* 探せなかったことと、無かったことを混ぜない。 */}
        {result?.note && (
          <Alert color="yellow" icon={<Icons.warning size={IconSize.action} />} title="一部しか探せていません">
            {result.note}
          </Alert>
        )}

        {candidates.length > 0 && (
          <>
            <Group justify="space-between" wrap="nowrap">
              <Text size="sm" fw={650}>{formatNumber(candidates.length)}作品の案</Text>
              {result && result.eligibleCount > candidates.length && (
                <Text size="xs" c="dimmed">
                  条件を満たしたのは{formatNumber(result.eligibleCount)}作品。確かなものから出しています
                </Text>
              )}
            </Group>
            <Stack gap="xs">
              {candidates.map((candidate) => (
                <AdditionRow
                  key={candidate.downloadId}
                  candidate={candidate}
                  dropped={dropped.has(candidate.downloadId)}
                  onToggle={() => toggle(candidate.downloadId)}
                />
              ))}
            </Stack>
          </>
        )}

        {search.isSuccess && candidates.length === 0 && !result?.note && (
          <Stack align="center" gap="xs" py="lg">
            <Icons.collectionSuggest size={IconSize.hero} />
            <Text fw={700}>いま足せそうな作品はありません</Text>
            <Text size="sm" c="dimmed" ta="center" maw={440}>
              確かだと言えるものだけを出しています。新しい作品を保存したあとに、もう一度探してみてください。
            </Text>
          </Stack>
        )}

        {/* 操作は下に貼り付ける。候補は縦に長いので、下まで送らないと押せない
            操作は「使いにくい」ではなく無いに等しい。 */}
        <Group className="overlay-actions" justify="space-between" wrap="nowrap">
          <Text size="xs" c="dimmed">行を押すと、その作品だけ外れます。</Text>
          <Group gap="xs" wrap="nowrap">
            <Button variant="default" onClick={onClose} disabled={busy}>閉じる</Button>
            <Button
              variant="light"
              leftSection={<Icons.search size={IconSize.action} />}
              loading={search.isPending}
              disabled={busy}
              onClick={() => search.mutate()}
            >
              もう一度探す
            </Button>
            <Button
              leftSection={<Icons.add size={IconSize.action} />}
              loading={busy}
              disabled={chosen.length === 0 || search.isPending}
              onClick={() => onAdd(chosen)}
            >
              {formatNumber(chosen.length)}作品を追加
            </Button>
          </Group>
        </Group>
      </Stack>
    </Modal>
  );
}

/**
 * 候補ひとつぶん。
 *
 * 出すのは三つ — 何の作品か、なぜこれなのか、入れるか外すか。走査の候補カードが
 * 確度%をやめたのと同じ理由で、ここでも数字は出さない。「0.74」は読む人の
 * 判断材料にならないが、「同じ公式シリーズです」はなる。
 */
function AdditionRow({ candidate, dropped, onToggle }: {
  candidate: CollectionAdditionCandidate;
  dropped: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="addition-row" data-dropped={dropped || undefined}>
      {/* 行ぜんぶを入切の切り替えにする。表紙だけを押させていたが、押せると
          分かる手がかりが無く、表紙の無い作品では押す的すら見えなかった。 */}
      <button
        type="button"
        className="suggestion-member addition-row__pick"
        data-dropped={dropped || undefined}
        aria-label={dropped ? `${candidate.title}を候補に戻す` : `${candidate.title}を候補から外す`}
        aria-pressed={!dropped}
        onClick={onToggle}
      >
        <span className="suggestion-member__mark" aria-hidden="true">
          {dropped ? <Icons.cancel size={IconSize.inline} /> : <Icons.confirm size={IconSize.inline} />}
        </span>
        <span className="suggestion-member__cover">
          <WorkCover work={candidate} variant="compact" />
        </span>
        <span className="suggestion-member__body">
          {/* 題名は折らずに最後まで出す。1行で切っていたが、この束へ足す候補は
              同じ連載の続きであることが多い。**同じ書き出しで始まる作品**を
              1行で切ると、どれも同じ文字列になって見分けられない。 */}
          <Text size="sm" fw={650}>{candidate.title}</Text>
          <Text size="xs" c="dimmed">
            {candidate.authorName}
            {candidate.textLength > 0 ? ` · ${formatNumber(candidate.textLength)}字` : ""}
          </Text>
          <Text size="xs" c="dimmed">{candidate.reason}</Text>
        </span>
      </button>
      {/* 縮ませない。`wrap="nowrap"` の行では、Mantine の Badge は
          `overflow: hidden` を持つので最小幅が 0 になり、隣の題名が伸びると
          「続..」のように潰れる。 */}
      <Group gap={4} wrap="wrap" justify="flex-end" style={{ flexShrink: 0 }}>
        {candidate.evidence.map((evidence) => (
          <Badge key={evidence.kind} size="xs" variant="light" color={evidence.kind === "semantic_similarity" ? "grape" : "piep"}>
            {evidence.label}
          </Badge>
        ))}
      </Group>
    </div>
  );
}
