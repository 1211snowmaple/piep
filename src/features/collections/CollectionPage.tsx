import { useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  Modal,
  Stack,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { useDebouncedValue, useDisclosure } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { modals } from "@mantine/modals";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate, useRouteParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { PageHeader } from "@/components/PageHeader";
import { WorkCover } from "@/components/WorkCover";
import { formatNumber, errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { ProviderMark } from "@/lib/providers";
import {
  addWorkCollectionMembers,
  deleteWorkCollection,
  getWorkCollection,
  removeWorkCollectionMembers,
  reorderWorkCollectionMembers,
  upsertWorkCollection,
} from "@/services/collectionApi";
import { isTauriRuntime, searchDownloadsV2 } from "@/services/dbApi";
import { openSingleDialog } from "@/services/dialogApi";
import { exportCollectionEpub } from "@/services/epubApi";
import type { WorkCollection, WorkCollectionMember, WorkKey } from "@/types/collections";
import type { DownloadEntry } from "@/types/library";
import { CollectionFormModal } from "./CollectionFormModal";

function workKey(work: { source: string; sourceId: string }): WorkKey {
  return { source: work.source, sourceId: work.sourceId };
}

/** The detail screen for one collection. The list of collections lives in the
 *  library beside 作者・クリエイター and シリーズ, so a bare `/collections`
 *  sends the reader there rather than showing a second, competing list. */
export default function CollectionPage() {
  const { collectionId } = useRouteParams("/collections/:collectionId?");
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [formOpened, form] = useDisclosure(false);
  const collectionQuery = useQuery({
    queryKey: ["work-collection", collectionId],
    queryFn: () => getWorkCollection(collectionId!),
    enabled: runtime && Boolean(collectionId),
  });
  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["work-collections"] });
    queryClient.invalidateQueries({ queryKey: ["work-collection"] });
    queryClient.invalidateQueries({ queryKey: ["collections-for-work"] });
    queryClient.invalidateQueries({ queryKey: ["collections-for-person"] });
  };
  const saveMutation = useMutation({
    mutationFn: upsertWorkCollection,
    onSuccess: () => { invalidate(); form.close(); notifications.show({ color: "green", message: "コレクションを保存しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "コレクションを保存できません", message: errorMessage(error) }),
  });

  useEffect(() => {
    if (!collectionId) navigate("/library?tab=collections", { replace: true });
  }, [collectionId, navigate]);

  if (!collectionId) return null;
  if (!runtime) return <div className="page"><Alert>コレクションはデスクトップ版のローカルライブラリで利用できます。</Alert></div>;
  if (collectionQuery.isLoading) return <div className="page"><LoadingState label="コレクションを開いています" /></div>;
  if (collectionQuery.error || !collectionQuery.data) return <div className="page"><ErrorState error={collectionQuery.error ?? "コレクションが見つかりません"} retry={() => collectionQuery.refetch()} /></div>;
  return (
    <>
      <CollectionDetail
        collection={collectionQuery.data}
        onEdit={form.open}
        onChanged={(value) => { queryClient.setQueryData(["work-collection", collectionId], value); invalidate(); }}
      />
      <CollectionFormModal opened={formOpened} collection={collectionQuery.data} onClose={form.close} onSave={(input) => saveMutation.mutate(input)} />
    </>
  );
}


/** One work inside a collection: what it is, where it sits, and the three
  * things you can do with it from here. Ordering controls only appear on an
  * ordered collection, because moving a work up in a themed set means nothing. */
function MemberRow({ member, ordered, first, last, busy, onMove, onRead, onRemove }: {
  member: WorkCollectionMember;
  ordered: boolean;
  first: boolean;
  last: boolean;
  busy: boolean;
  onMove: (delta: number) => void;
  onRead: () => void;
  onRemove: () => void;
}) {
  return (
    <Card withBorder padding="sm">
      <Group wrap="nowrap" align="center">
        <WorkCover variant="compact" work={member} />
        <Box flex={1} miw={0}>
          <Group gap="xs">
            <Text fw={600} className="line-clamp-1">{member.title}</Text>
            {member.missing && <Badge color="orange" size="xs">未保存</Badge>}
            {member.addedBy === "suggestion" && <Badge color="violet" size="xs" variant="light">自動提案</Badge>}
          </Group>
          <Group gap="xs">
            <ProviderMark provider={member.source} />
            <Text size="xs" c="dimmed">{member.authorName} · {formatNumber(member.textLength)}字</Text>
          </Group>
        </Box>
        {ordered && (
          <Group gap={2} wrap="nowrap">
            <Tooltip label="一つ前へ"><ActionIcon variant="subtle" disabled={first || busy} onClick={() => onMove(-1)}><Icons.up size={IconSize.menu} /></ActionIcon></Tooltip>
            <Tooltip label="一つ後へ"><ActionIcon variant="subtle" disabled={last || busy} onClick={() => onMove(1)}><Icons.down size={IconSize.menu} /></ActionIcon></Tooltip>
          </Group>
        )}
        {member.downloadId && <Button variant="light" size="xs" onClick={onRead}>読む</Button>}
        <Tooltip label="コレクションから外す">
          <ActionIcon color="red" variant="subtle" disabled={busy} onClick={onRemove}><Icons.delete size={IconSize.menu} /></ActionIcon>
        </Tooltip>
      </Group>
    </Card>
  );
}

function CollectionDetail({ collection, onEdit, onChanged }: { collection: WorkCollection; onEdit: () => void; onChanged: (collection: WorkCollection) => void }) {
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [addOpened, addModal] = useDisclosure(false);
  const [query, setQuery] = useState("");
  const [debouncedQuery] = useDebouncedValue(query, 250);
  const [selectedIds, setSelectedIds] = useState(new Set<number>());
  const searchQuery = useQuery({
    queryKey: ["collection-add-search", debouncedQuery],
    queryFn: () => searchDownloadsV2({ query: debouncedQuery || null, sortBy: debouncedQuery ? "relevance" : "downloaded_at", sortOrder: "desc", limit: 40, projection: "bulk" }),
    enabled: addOpened,
  });
  const mutation = useMutation({
    mutationFn: async (action: { type: "remove"; key: WorkKey } | { type: "reorder"; keys: WorkKey[] } | { type: "add"; works: DownloadEntry[] }) => {
      if (action.type === "remove") return removeWorkCollectionMembers(collection.id, [action.key]);
      if (action.type === "reorder") return reorderWorkCollectionMembers(collection.id, action.keys);
      return addWorkCollectionMembers(collection.id, action.works.map((work) => ({ ...workKey(work), addedBy: "manual" })));
    },
    onSuccess: (value) => { onChanged(value); setSelectedIds(new Set()); addModal.close(); notifications.show({ color: "green", message: "コレクションを更新しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "コレクションを更新できません", message: errorMessage(error) }),
  });
  const missingCount = collection.memberCount - collection.availableCount;
  const epubMutation = useMutation({
    mutationFn: async (skipMissing: boolean) => {
      const outputDir = await openSingleDialog({ directory: true, title: "一冊にまとめたEPUBの出力先" });
      if (!outputDir) return null;
      return exportCollectionEpub(collection.id, "__auto__", outputDir, skipMissing);
    },
    onSuccess: (path) => { if (path) notifications.show({ color: "green", title: "一冊のEPUBを書き出しました", message: path }); },
    onError: (error) => notifications.show({ color: "red", title: "EPUBを書き出せません", message: errorMessage(error) }),
  });
  // 欠落は書き出しを塞ぐ理由にしない。設計提案 13-3 のとおり、除外して続けるか
  // 中止するかを利用者が選ぶ。確認は出力先ダイアログより前に出す。
  const startEpubExport = () => {
    if (missingCount <= 0) { epubMutation.mutate(false); return; }
    modals.openConfirmModal({
      title: "未保存の作品があります",
      children: <Text size="sm">「{collection.name}」の{formatNumber(missingCount)}作品はライブラリに保存されていないため、本文を収録できません。この{formatNumber(missingCount)}作品を除いて、残り{formatNumber(collection.availableCount)}作品で一冊にしますか？</Text>,
      labels: { confirm: "除外して書き出す", cancel: "中止する" },
      onConfirm: () => epubMutation.mutate(true),
    });
  };
  const move = (index: number, delta: number) => {
    const next = [...collection.members];
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    mutation.mutate({ type: "reorder", keys: next.map(workKey) });
  };
  const deleteCollection = () => modals.openConfirmModal({
    title: "コレクションを削除しますか？",
    children: <Text size="sm">作品そのものは削除されません。「{collection.name}」というまとまりと並びだけを削除します。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" }, confirmProps: { color: "red" },
    onConfirm: async () => { try { await deleteWorkCollection(collection.id); queryClient.invalidateQueries({ queryKey: ["work-collections"] }); navigate("/library?tab=collections"); } catch (error) { notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }); } },
  });
  const results = searchQuery.data?.items ?? [];
  const existing = useMemo(() => new Set(collection.members.filter((member) => member.downloadId).map((member) => member.downloadId!)), [collection.members]);
  const selectedWorks = results.filter((work) => selectedIds.has(work.id));
  return (
    <div className="page">
      <Stack gap="xl">
        <PageHeader eyebrow="Collection" title={collection.name} description={collection.description ?? `${collection.memberCount}作品をまとめた読書コレクション`} actions={<><Button variant="default" leftSection={<Icons.epub size={IconSize.action} />} loading={epubMutation.isPending} disabled={collection.availableCount === 0} onClick={startEpubExport}>一冊のEPUB</Button><Button variant="default" onClick={onEdit}>編集</Button><Button leftSection={<Icons.add size={IconSize.action} />} onClick={addModal.open}>作品を追加</Button></>} />
        <Group gap="xs"><Badge variant="light">{collection.collectionKind === "ordered" ? "順序付き" : "順序なし"}</Badge><Text size="sm" c="dimmed">{formatNumber(collection.memberCount)}作品 · {formatNumber(collection.totalTextLength)}字</Text>{missingCount > 0 && <Badge color="orange">未保存 {formatNumber(missingCount)}作品</Badge>}</Group>
        {collection.members.length === 0 ? <Alert color="gray">作品を追加すると、ここから順番に読み進めたり一冊のEPUBにできます。</Alert> : <Stack gap="sm">{collection.members.map((member, index) => (
          <MemberRow
            key={`${member.source}:${member.sourceId}`}
            member={member}
            ordered={collection.collectionKind === "ordered"}
            first={index === 0}
            last={index === collection.members.length - 1}
            busy={mutation.isPending}
            onMove={(delta) => move(index, delta)}
            onRead={() => navigate(`/reader/${member.downloadId}?collection=${encodeURIComponent(collection.id)}`)}
            onRemove={() => mutation.mutate({ type: "remove", key: workKey(member) })}
          />
        ))}</Stack>}
        <Divider /><Group justify="flex-end"><Button color="red" variant="subtle" leftSection={<Icons.delete size={IconSize.menu} />} onClick={deleteCollection}>コレクションを削除</Button></Group>
      </Stack>
      <Modal opened={addOpened} onClose={addModal.close} title="コレクションに作品を追加" size="lg">
        <Stack>
          <TextInput value={query} onChange={(event) => setQuery(event.currentTarget.value)} leftSection={<Icons.search size={IconSize.action} />} placeholder="タイトル・作者・本文から検索" autoFocus />
          {searchQuery.isLoading ? <LoadingState label="作品を検索しています" /> : searchQuery.error ? <ErrorState error={searchQuery.error} retry={() => searchQuery.refetch()} /> : <Stack gap={6} mah={420} style={{ overflowY: "auto" }}>{results.map((work) => <Checkbox key={work.id} disabled={existing.has(work.id)} checked={existing.has(work.id) || selectedIds.has(work.id)} onChange={(event) => { const isChecked = event.currentTarget.checked; setSelectedIds((current) => { const next = new Set(current); if (isChecked) next.add(work.id); else next.delete(work.id); return next; }); }} label={<Box><Text size="sm" fw={600}>{work.title}</Text><Text size="xs" c="dimmed">{work.authorName} · {work.source}</Text></Box>} />)}</Stack>}
          <Group justify="flex-end"><Button variant="default" onClick={addModal.close}>キャンセル</Button><Button disabled={selectedWorks.length === 0 || mutation.isPending} onClick={() => mutation.mutate({ type: "add", works: selectedWorks })}>{selectedWorks.length}作品を追加</Button></Group>
        </Stack>
      </Modal>
    </div>
  );
}
