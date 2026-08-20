import { useEffect, useRef, useState } from "react";
import {
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Divider,
  Group,
  Paper,
  SimpleGrid,
  Stack,
  Text,
  Title,
} from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { modals } from "@mantine/modals";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import {
  acceptCollectionSuggestion,
  generateCollectionSuggestion,
  listCollectionSuggestions,
  dismissCollectionSuggestion,
  listWorkCollections,
  rejectCollectionSuggestion,
  upsertWorkCollection,
} from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import type { CollectionSuggestion, WorkCollectionSummary, WorkKey } from "@/types/collections";
import { CollectionCard } from "./CollectionCard";
import { CollectionFormModal } from "./CollectionFormModal";

const EMPTY_COLLECTIONS: WorkCollectionSummary[] = [];

function memberKey(member: { source: string; sourceId: string }): string {
  return `${member.source}${member.sourceId}`;
}

function SuggestionCard({ suggestion, busy, onAccept, onReject, onDismiss }: {
  suggestion: CollectionSuggestion;
  busy: boolean;
  onAccept: (suggestion: CollectionSuggestion, keys: WorkKey[]) => void;
  onReject: (suggestion: CollectionSuggestion, keys: WorkKey[]) => void;
  onDismiss: (suggestion: CollectionSuggestion) => void;
}) {
  const [selected, setSelected] = useState(() => new Set(suggestion.members.map(memberKey)));
  const selectedKeys = suggestion.members
    .filter((member) => selected.has(memberKey(member)))
    .map((member) => ({ source: member.source, sourceId: member.sourceId }));
  return (
    <Card withBorder padding="lg">
      <Stack gap="md">
        <Group justify="space-between" align="flex-start">
          <Box><Text fw={700}>{suggestion.proposedName}</Text><Text size="xs" c="dimmed">{suggestion.collectionKind === "ordered" ? "順序付き" : "順序なし"} · 候補 {suggestion.members.length}作品</Text></Box>
          <Badge variant="light" color="violet">確度 {Math.round(suggestion.score * 100)}%</Badge>
        </Group>
        <Stack gap={6}>
          {suggestion.members.map((member) => {
            const key = memberKey(member);
            return (
              <Paper key={key} withBorder p="sm">
                <Group wrap="nowrap" align="flex-start">
                  <Checkbox checked={selected.has(key)} onChange={(event) => { const isChecked = event.currentTarget.checked; setSelected((current) => { const next = new Set(current); if (isChecked) next.add(key); else next.delete(key); return next; }); }} aria-label={`${member.title}を含める`} mt={3} />
                  <Box flex={1} miw={0}>
                    <Group gap="xs"><Text size="sm" fw={600} className="line-clamp-1">{member.title}</Text><Badge size="xs" variant="outline">{Math.round(member.score * 100)}%</Badge></Group>
                    <Text size="xs" c="dimmed">{member.authorName}</Text>
                    <Group gap={5} mt={5}>{member.evidence.map((evidence) => <Badge key={`${key}-${evidence.kind}`} size="xs" variant="light" color={evidence.kind === "content_link" ? "blue" : evidence.kind === "official_series" ? "green" : "gray"}>{evidence.label}</Badge>)}</Group>
                  </Box>
                </Group>
              </Paper>
            );
          })}
        </Stack>
        {/* Three different outcomes, kept apart on purpose: 削除 forgets the
         *  draft, 今後提案しない records that these works do not belong
         *  together, and 作成 turns the ticked works into a collection. */}
        <Group justify="space-between">
          <Button variant="subtle" color="gray" disabled={busy} onClick={() => onDismiss(suggestion)}>このひな型を削除</Button>
          <Group gap="xs">
            <Button variant="subtle" color="red" disabled={busy || selectedKeys.length === 0} onClick={() => onReject(suggestion, selectedKeys)}>選択した作品を今後提案しない</Button>
            <Button leftSection={<Icons.confirm size={IconSize.menu} />} disabled={busy || selectedKeys.length === 0} onClick={() => onAccept(suggestion, selectedKeys)}>選択した作品で作成</Button>
          </Group>
        </Group>
      </Stack>
    </Card>
  );
}

/** The collection list as it appears inside the library, beside 作品 /
 *  作者・クリエイター / シリーズ. The detail screen stays on its own route. */
/**
 * 一覧の並べ替え。コレクションは手で作った束ねなので、鍵も作品や作者とは違う。
 * 「いつ作ったか」「名前」「何作品入っているか」の三つで足りる。
 */
export type CollectionSortBy = "created_at" | "name" | "member_count";

/** 名前と説明にかかる、ただの絞り込み。件数を数える側と同じ関数を使う。 */
export function filterCollections(items: WorkCollectionSummary[], query: string): WorkCollectionSummary[] {
  const normalized = query.trim().toLocaleLowerCase("ja-JP");
  if (!normalized) return items;
  return items.filter((collection) =>
    `${collection.name} ${collection.description ?? ""}`.toLocaleLowerCase("ja-JP").includes(normalized));
}

function sortCollections(items: WorkCollectionSummary[], sortBy: CollectionSortBy): WorkCollectionSummary[] {
  const sorted = [...items];
  if (sortBy === "name") return sorted.sort((a, b) => a.name.localeCompare(b.name, "ja"));
  if (sortBy === "member_count") return sorted.sort((a, b) => b.memberCount - a.memberCount || a.name.localeCompare(b.name, "ja"));
  return sorted.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

export function CollectionsPanel({ query = "", sortBy = "created_at" }: { query?: string; sortBy?: CollectionSortBy } = {}) {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useAppSearchParams();
  const [formOpened, form] = useDisclosure(false);
  const generatedSeed = useRef<number | null>(null);
  const collectionsQuery = useQuery({
    queryKey: ["work-collections"],
    queryFn: () => runtime ? listWorkCollections() : Promise.resolve(EMPTY_COLLECTIONS),
  });
  const suggestionsQuery = useQuery({
    queryKey: ["collection-suggestions", "pending"],
    queryFn: () => runtime ? listCollectionSuggestions("pending") : Promise.resolve([]),
  });
  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["work-collections"] });
    queryClient.invalidateQueries({ queryKey: ["work-collection"] });
    queryClient.invalidateQueries({ queryKey: ["collections-for-work"] });
    queryClient.invalidateQueries({ queryKey: ["collections-for-person"] });
    queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] });
  };
  const saveMutation = useMutation({
    mutationFn: upsertWorkCollection,
    onSuccess: (saved) => { invalidate(); form.close(); navigate(`/collections/${saved.id}`); notifications.show({ color: "green", message: "コレクションを保存しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "コレクションを保存できません", message: errorMessage(error) }),
  });
  const suggestionMutation = useMutation({
    mutationFn: (seedId: number) => generateCollectionSuggestion([seedId]),
    onSuccess: () => { queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] }); notifications.show({ color: "green", message: "関連作品のひな型を作成しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "ひな型を作成できません", message: errorMessage(error) }),
  });
  const acceptMutation = useMutation({
    mutationFn: ({ suggestion, keys }: { suggestion: CollectionSuggestion; keys: WorkKey[] }) => acceptCollectionSuggestion({ suggestionId: suggestion.id, memberKeys: keys }),
    onSuccess: (saved) => { invalidate(); navigate(`/collections/${saved.id}`); notifications.show({ color: "green", message: "提案からコレクションを作成しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "提案を採用できません", message: errorMessage(error) }),
  });
  const rejectMutation = useMutation({
    mutationFn: ({ suggestionId, keys }: { suggestionId: string; keys: WorkKey[] }) => rejectCollectionSuggestion(suggestionId, keys),
    onSuccess: () => { invalidate(); notifications.show({ message: "この組合せを今後の提案から除外します" }); },
    onError: (error) => notifications.show({ color: "red", title: "提案を却下できません", message: errorMessage(error) }),
  });
  const dismissMutation = useMutation({
    mutationFn: dismissCollectionSuggestion,
    onSuccess: () => { invalidate(); notifications.show({ message: "ひな型を削除しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "ひな型を削除できません", message: errorMessage(error) }),
  });
  const confirmRejectSuggestion = (suggestion: CollectionSuggestion, keys: WorkKey[]) => modals.openConfirmModal({
    title: "この組合せを今後の候補から外しますか？",
    children: <Text size="sm">選択中の{keys.length}作品を、現在の候補生成規則では「{suggestion.proposedName}」として再提案しません。チェックを外した作品は対象になりません。規則が改善された場合は改めて評価できます。</Text>,
    labels: { confirm: "候補から外す", cancel: "キャンセル" },
    confirmProps: { color: "red" },
    onConfirm: () => rejectMutation.mutate({ suggestionId: suggestion.id, keys }),
  });

  // 作品詳細の「関連作品からひな型を作る」からは ?suggest=<id> で来る。
  useEffect(() => {
    const seed = Number(searchParams.get("suggest"));
    if (!runtime || !Number.isSafeInteger(seed) || seed <= 0 || generatedSeed.current === seed) return;
    generatedSeed.current = seed;
    suggestionMutation.mutate(seed);
    const next = new URLSearchParams(searchParams);
    next.delete("suggest");
    setSearchParams(next, { replace: true });
  }, [runtime, searchParams, setSearchParams, suggestionMutation]);

  // 上のツールバーはこのタブでも出したままにする（タブを移るたびに画面が
   // 飛ばないため）。出ている以上、検索も並べ替えもここに効かせる。
  const normalizedQuery = query.trim();
  const collections = sortCollections(filterCollections(collectionsQuery.data ?? [], query), sortBy);
  const suggestions = suggestionsQuery.data ?? [];
  return (
    <Stack gap="xl">
      <Group justify="space-between" align="flex-start">
        <Text size="sm" c="dimmed" maw={640}>作者・公式シリーズ・取得元を越えて、読む単位を自分でまとめます。ひとつの作品を複数のコレクションに入れられます。</Text>
        <Button leftSection={<Icons.add size={IconSize.action} />} disabled={!runtime} onClick={form.open}>新しいコレクション</Button>
      </Group>
      {!runtime && <Alert color="gray">プレビューではコレクションの変更は保存されません。</Alert>}
      {suggestionMutation.isPending && <Alert icon={<Icons.collectionSuggest size={IconSize.action} />}>本文・キャプションのリンクを双方向へたどり、その後にシリーズ、作者、タイトル、意味的な近さを補助根拠として調べています。</Alert>}
      {suggestions.length > 0 && <Stack gap="md"><Group gap="xs"><Icons.collectionSuggest size={IconSize.action} /><Title order={2} size="h3">確認待ちのひな型</Title><Badge>{suggestions.length}</Badge></Group>{suggestions.map((suggestion) => <SuggestionCard key={suggestion.id} suggestion={suggestion} busy={acceptMutation.isPending || rejectMutation.isPending || dismissMutation.isPending} onAccept={(value, keys) => acceptMutation.mutate({ suggestion: value, keys })} onReject={confirmRejectSuggestion} onDismiss={(value) => dismissMutation.mutate(value.id)} />)}<Divider /></Stack>}
      {collectionsQuery.isLoading ? <LoadingState label="コレクションを読み込んでいます" /> : collectionsQuery.error ? <ErrorState error={collectionsQuery.error} retry={() => collectionsQuery.refetch()} /> : collections.length === 0 ? (normalizedQuery
        ? <Paper withBorder p="xl"><Stack align="center"><Icons.search size={IconSize.hero} /><Text fw={700}>一致するコレクションがありません</Text><Text size="sm" c="dimmed" ta="center">「{query.trim()}」に当てはまるものは見つかりませんでした。</Text></Stack></Paper>
        : <Paper withBorder p="xl"><Stack align="center"><Icons.collection size={IconSize.hero} /><Text fw={700}>まだコレクションはありません</Text><Text size="sm" c="dimmed" ta="center">前後編、非公式の連載、pixivとFANBOXに分かれた作品などをまとめられます。</Text><Button onClick={form.open} disabled={!runtime}>最初のコレクションを作成</Button></Stack></Paper>) : <SimpleGrid cols={{ base: 1, sm: 2, xl: 3 }}>{collections.map((collection) => <CollectionCard key={collection.id} collection={collection} />)}</SimpleGrid>}
      <CollectionFormModal opened={formOpened} onClose={form.close} onSave={(input) => saveMutation.mutate(input)} />
    </Stack>
  );
}
