import { useEffect, useRef } from "react";
import {
  Alert,
  Badge,
  Button,
  Group,
  Paper,
  SimpleGrid,
  Stack,
  Text,
} from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ListPager, PagingModeToggle, useBoundedNumberedPage, usePageSize, usePagingMode } from "@/components/ListPager";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { generateCollectionSuggestion, listCollectionSuggestions, upsertWorkCollection } from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { demoSuggestions } from "@/mocks/demoData";
import type { WorkCollectionSummary } from "@/types/collections";
import { invalidateCollectionViews, workCollectionsQueryOptions } from "./collectionQueries";
import { CollectionCard } from "./CollectionCard";
import { CollectionFormModal } from "./CollectionFormModal";
import { CollectionSweepModal } from "./CollectionSweepModal";

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
  const collectionsQuery = useQuery(workCollectionsQueryOptions());
  const invalidate = () => {
    invalidateCollectionViews(queryClient);
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
  // 束の一覧は一度に全件を持っている。棚のようにサーバーへ問い直す必要は無い
  // ので、ページ番号は手元の配列を切るだけで足りる。
  const [pagingMode] = usePagingMode("library-collections");
  const [pageSize] = usePageSize();
  const numberedPages = pagingMode === "pages";
  const {
    page: requestedPage,
    maxPage: maxDirectPage,
    limitNotice: pageLimitNotice,
    clearLimitNotice,
  } = useBoundedNumberedPage(numberedPages, searchParams, setSearchParams, pageSize);
  const lastPage = Math.max(1, Math.ceil(collections.length / pageSize));
  // 束を消したり絞り込んだりすると最後のページが消える。URL が残っていても、
  // 表示は必ず存在するページに収める。
  const currentPage = Math.min(requestedPage, lastPage);
  const visibleCollections = numberedPages
    ? collections.slice((currentPage - 1) * pageSize, currentPage * pageSize)
    : collections;
  const goToPage = (next: number) => {
    clearLimitNotice();
    const params = new URLSearchParams(searchParams);
    if (next > 1) params.set("page", String(next)); else params.delete("page");
    setSearchParams(params);
  };
  // 走査はオーバーレイの中で完結させる。棚の一覧に結果を差し込んでいたころは、
  // 候補が出ているあいだコレクションそのものが下へ押し出されていた。
  // 見出しに残すのは入口の一本だけで、まだ片付けていない候補の数を添える。
  const [sweepOpened, sweepModal] = useDisclosure(false);
  const pendingSuggestions = useQuery({
    queryKey: ["collection-suggestions", "pending"],
    queryFn: () => (runtime ? listCollectionSuggestions("pending") : Promise.resolve(demoSuggestions)),
  });
  const pendingCount = pendingSuggestions.data?.length ?? 0;
  return (
    <Stack gap="md" className="collections-panel">
      {/* 件数・探す・作るを一行に畳む。説明文と件数と操作で三行使っていた頃は、
          棚そのものが画面の下へ押し出されていた。何ができるかは操作の名前が
          言っているので、案内の一文は要らない。 */}
      <Group justify="space-between" align="center" my="md" gap="xs" wrap="nowrap" className="collections-panel__header">
        <Group gap="xs" wrap="nowrap">
          <Text size="sm" c="dimmed">{formatNumber(collections.length)}件</Text>
          <PagingModeToggle scope="library-collections" />
        </Group>
        <Group gap="xs" wrap="nowrap">
          <Button
            variant="subtle"
            leftSection={<Icons.search size={IconSize.action} />}
            rightSection={pendingCount > 0 ? <Badge size="sm" variant="filled" circle>{formatNumber(pendingCount)}</Badge> : undefined}
            onClick={sweepModal.open}
          >
            まとまりを探す
          </Button>
          <Button leftSection={<Icons.add size={IconSize.action} />} disabled={!runtime} onClick={form.open}>新しいコレクション</Button>
        </Group>
      </Group>
      {!runtime && <Alert color="gray">プレビューではコレクションの変更は保存されません。</Alert>}
      {suggestionMutation.isPending && <Alert icon={<Icons.collectionSuggest size={IconSize.action} />}>本文・キャプションのリンクを双方向へたどり、その後にシリーズ、作者、タイトル、意味的な近さを補助根拠として調べています。</Alert>}
      {collectionsQuery.isLoading ? <LoadingState label="コレクションを読み込んでいます" /> : collectionsQuery.error ? <ErrorState error={collectionsQuery.error} retry={() => collectionsQuery.refetch()} /> : collections.length === 0 ? (normalizedQuery
        ? <Paper withBorder p="xl"><Stack align="center"><Icons.search size={IconSize.hero} /><Text fw={700}>一致するコレクションがありません</Text><Text size="sm" c="dimmed" ta="center">「{query.trim()}」に当てはまるものは見つかりませんでした。</Text></Stack></Paper>
        : <Paper withBorder p="xl"><Stack align="center"><Icons.collection size={IconSize.hero} /><Text fw={700}>まだコレクションはありません</Text><Text size="sm" c="dimmed" ta="center">前後編、非公式の連載、pixivとFANBOXに分かれた作品などをまとめられます。</Text><Button onClick={form.open} disabled={!runtime}>最初のコレクションを作成</Button></Stack></Paper>) : <SimpleGrid cols={{ base: 1, sm: 2, xl: 3 }}>{visibleCollections.map((collection) => <CollectionCard key={collection.id} collection={collection} />)}</SimpleGrid>}
      {/* 番号で見るときだけ出す。「自動」のときは全件がそのまま並んでいる。 */}
      {numberedPages && collections.length > pageSize && (
        <ListPager
          scope="library-collections"
          hasNext={false}
          loading={false}
          loaded={visibleCollections.length}
          total={collections.length}
          onLoad={() => undefined}
          unit="件"
          pages={{ current: currentPage, size: pageSize, onGoTo: goToPage, maxDirectPage, limitNotice: pageLimitNotice }}
        />
      )}
      <CollectionFormModal opened={formOpened} onClose={form.close} onSave={(input) => saveMutation.mutate(input)} />
      {/* 候補は一箇所にまとめる。1作から広げたものと棚の走査で出たものを
          別の場所に置くと、同じ束が二度出てくるように見える。 */}
      <CollectionSweepModal opened={sweepOpened} onClose={sweepModal.close} />
    </Stack>
  );
}
