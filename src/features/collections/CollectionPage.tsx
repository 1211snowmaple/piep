import { useEffect, useMemo, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Button,
  Group,
  Menu,
  Paper,
  SegmentedControl,
  Stack,
  Text,
  Tooltip,
} from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { modals } from "@mantine/modals";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invalidateCollectionViews } from "./collectionQueries";
import { useAppNavigate, useAppSearchParams, useRouteParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { ListPager, PagingModeToggle, useBoundedNumberedPage, usePageSize, usePagingMode } from "@/components/ListPager";
import { CollectionCover } from "@/components/CollectionCover";
import { NamedWorkList } from "@/components/NamedWorkList";
import { formatNumber, errorMessage } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { parseViewMode, useViewMode } from "@/lib/viewMode";
import { CollectionSplitAssist } from "@/features/assist/CollectionSplitAssist";
import { AssistLauncher } from "@/features/assist/AssistLauncher";
import { useAssist } from "@/features/assist/useAssist";
import { usePageAssist } from "@/app/PageAssistContext";
import {
  addDownloadsToCollection,
  addWorkCollectionMembers,
  createCollectionFromDownloads,
  deleteWorkCollection,
  getWorkCollection,
  removeWorkCollectionMembers,
  reorderWorkCollectionMembers,
  sortWorkCollectionMembers,
  upsertWorkCollection,
} from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { getDemoCollection } from "@/mocks/demoData";
import { openSingleDialog } from "@/services/dialogApi";
import { exportCollectionEpub } from "@/services/epubApi";
import { readExportSettings, toCompressOptions } from "@/features/epub/exportSettings";
import type { WorkCollection, WorkCollectionMember, WorkKey } from "@/types/collections";
import type { DownloadEntry } from "@/types/library";
import { CollectionAddWorksModal } from "./CollectionAddWorksModal";
import { CollectionAdditionsModal } from "./CollectionAdditionsModal";
import { CollectionCoverModal } from "./CollectionCoverModal";
import { CollectionRenameModal } from "./CollectionRenameModal";
import { CollectionMemberList } from "./CollectionMemberList";

function workKey(work: { source: string; sourceId: string }): WorkKey {
  return { source: work.source, sourceId: work.sourceId };
}

export function optimisticCollectionOrder(
  collection: WorkCollection,
  members: WorkCollectionMember[],
): WorkCollection {
  return {
    ...collection,
    members: members.map((member, position) => ({ ...member, position })),
    coverTiles: members.slice(0, 4).map((member) => ({
      source: member.source,
      sourceId: member.sourceId,
      title: member.title,
      authorName: member.authorName,
      coverPath: member.coverPath,
    })),
  };
}

/** Every screen that mentions a collection reads one of these four keys.
 *
 *  Kept in one place because the two components below both need it: while the
 *  list was invalidated by hand in each caller, the delete path refreshed only
 *  ["work-collections"], so the 所属 menus on the work and author pages went on
 *  offering a collection that no longer existed. */


/** The detail screen for one collection. The list of collections lives in the
 *  library beside 作者・クリエイター and シリーズ, so a bare `/collections`
 *  sends the reader there rather than showing a second, competing list. */
export default function CollectionPage() {
  const { collectionId } = useRouteParams("/collections/:collectionId?");
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [formOpened, form] = useDisclosure(false);
  const [renameAssist, setRenameAssist] = useState(false);
  const collectionQuery = useQuery({
    queryKey: ["work-collection", collectionId],
    queryFn: () => runtime ? getWorkCollection(collectionId!) : Promise.resolve(getDemoCollection(collectionId!)),
    enabled: Boolean(collectionId),
  });
  const invalidate = () => invalidateCollectionViews(queryClient);
  const saveMutation = useMutation({
    mutationFn: upsertWorkCollection,
    onSuccess: () => { invalidate(); form.close(); notifications.show({ color: "green", message: "コレクションを保存しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "コレクションを保存できません", message: errorMessage(error) }),
  });

  useEffect(() => {
    if (!collectionId) navigate("/library?tab=collections", { replace: true });
  }, [collectionId, navigate]);

  if (!collectionId) return null;
  if (collectionQuery.isLoading) return <div className="page"><LoadingState label="コレクションを開いています" /></div>;
  if (collectionQuery.error || !collectionQuery.data) return <div className="page"><ErrorState error={collectionQuery.error ?? "コレクションが見つかりません"} retry={() => collectionQuery.refetch()} /></div>;
  return (
    <>
      <CollectionDetail
        collection={collectionQuery.data}
        readOnly={!runtime}
        onEdit={(assist = false) => { setRenameAssist(assist); form.open(); }}
        onChanged={(value) => {
          queryClient.setQueryData(["work-collection", collectionId], value);
          // 返却値が詳細の確定データなので、同じ詳細を直後に再取得しない。
          // 一覧と所属だけを同期する。
          queryClient.invalidateQueries({ queryKey: ["work-collections"] });
          queryClient.invalidateQueries({ queryKey: ["collections-for-work"] });
          queryClient.invalidateQueries({ queryKey: ["collections-for-person"] });
        }}
      />
      <CollectionRenameModal
        opened={formOpened}
        collection={collectionQuery.data}
        busy={saveMutation.isPending}
        modelAssist={renameAssist}
        onClose={() => { setRenameAssist(false); form.close(); }}
        onSave={(input) => saveMutation.mutate(input)}
      />
    </>
  );
}

type MemberAction =
  | { type: "remove"; keys: WorkKey[] }
  | { type: "reorder"; keys: WorkKey[] }
  | { type: "add"; works: DownloadEntry[] }
  | { type: "sort"; mode: "published" | "episode" };

function CollectionDetail({ collection, readOnly, onEdit, onChanged }: { collection: WorkCollection; readOnly: boolean; onEdit: (assist?: boolean) => void; onChanged: (collection: WorkCollection) => void }) {
  const { engine: splitEngine } = useAssist("collection_split");
  const { engine: namingEngine } = useAssist("collection_naming");
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [addOpened, addModal] = useDisclosure(false);
  const [coverOpened, coverModal] = useDisclosure(false);
  const [splitOpened, splitModal] = useDisclosure(false);
  // 束は作った時点で閉じない。新作が届けば、いま入っていない作品のなかに
  // この束へ入るべきものが出てくる。
  const [additionsOpened, additionsModal] = useDisclosure(false);
  const [view, setView] = useViewMode();
  const [selectionMode, setSelectionMode] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const ordered = collection.collectionKind === "ordered";

  // ページ番号は棚と同じ仕組みを使う。束の中身はすでに全件が手元にあるので、
  // 棚のようにサーバーへ問い直すのではなく、持っている配列を切るだけでよい。
  // 読み込み方はこの一覧ぶんだけを覚える。束は番号で飛びたいが棚は流し読み
  // したい、という分かれ方をするので、全部を一つの好みで縛らない。
  const [urlParams, setUrlParams] = useAppSearchParams();
  const [pagingMode] = usePagingMode("collection-members");
  const [pageSize] = usePageSize();
  const numberedPages = pagingMode === "pages";
  const {
    page: requestedPage,
    maxPage: maxDirectPage,
    limitNotice: pageLimitNotice,
    clearLimitNotice,
  } = useBoundedNumberedPage(numberedPages, urlParams, setUrlParams, pageSize);
  const memberCount = collection.members.length;
  const lastPage = Math.max(1, Math.ceil(memberCount / pageSize));
  // 束から作品を外すと最後のページが消えることがある。URL が残っていても、
  // 表示は必ず存在するページに収める。
  const currentPage = Math.min(requestedPage, lastPage);
  const memberWindow = numberedPages
    ? { start: (currentPage - 1) * pageSize, end: currentPage * pageSize }
    : undefined;
  const goToPage = (next: number) => {
    clearLimitNotice();
    const params = new URLSearchParams(urlParams);
    if (next > 1) params.set("page", String(next));
    else params.delete("page");
    setUrlParams(params);
  };

  const mutation = useMutation({
    // 同じ束への操作は直列に流す。並べ替えは押すたびに飛ぶので、走らせたままだと
    // 応答の到着順で決まってしまう。onSuccess はサーバが返した並びをそのまま
    // 確定させるため、遅れて届いた古い応答が画面と保存済みの並びを上書きしうる。
    scope: { id: `work-collection:${collection.id}` },
    mutationFn: async (action: MemberAction) => {
      if (action.type === "remove") return removeWorkCollectionMembers(collection.id, action.keys);
      if (action.type === "reorder") return reorderWorkCollectionMembers(collection.id, action.keys);
      if (action.type === "sort") return sortWorkCollectionMembers(collection.id, action.mode);
      return addWorkCollectionMembers(collection.id, action.works.map((work) => ({ ...workKey(work), addedBy: "manual" })));
    },
    onSuccess: (value, action) => {
      onChanged(value);
      setSelected([]);
      if (action.type === "add") addModal.close();
      if (action.type === "remove") setSelectionMode(false);
      notifications.show({ color: "green", message: "コレクションを更新しました" });
    },
    onError: (error) => notifications.show({ color: "red", title: "コレクションを更新できません", message: errorMessage(error) }),
  });
  // 分けた先は**新しい束**として作る。元の束は触らない — 分けるかどうかは
  // 案を見てから決めることで、押した瞬間に元が壊れてはいけない。
  const splitMutation = useMutation({
    mutationFn: ({ name, ids }: { name: string; ids: number[] }) =>
      createCollectionFromDownloads(name, collection.collectionKind, ids),
    onSuccess: (created) => {
      queryClient.invalidateQueries({ queryKey: ["work-collections"] });
      notifications.show({
        color: "green",
        message: `「${created.name}」を作りました。元のコレクションはそのままです`,
        onClick: () => navigate(`/collections/${created.id}`),
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "分けられません", message: errorMessage(error) }),
  });

  // 探して選んだ作品を束へ入れる。棚の id をそのまま渡す経路が既にあるので、
  // 作品鍵へ組み直さない。
  const addFromSuggestions = useMutation({
    scope: { id: `work-collection:${collection.id}` },
    mutationFn: (downloadIds: number[]) => addDownloadsToCollection(collection.id, downloadIds),
    onSuccess: (value, downloadIds) => {
      onChanged(value);
      additionsModal.close();
      notifications.show({
        color: "green",
        message: `${formatNumber(downloadIds.length)}作品をコレクションに入れました`,
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "コレクションに入れられません", message: errorMessage(error) }),
  });

  /**
   * 並び方を切り替える。
   *
   * 作るときの一度きりしか選べなかった。束は作ったあとで性格が変わる —
   * テーマのつもりで集めたら読む順が見えてきた、その逆もある。名前を
   * 付け直せるのと同じ理由で、これも直せるべきである。
   *
   * 並びそのものは失わない。メンバーは順序なしでも position を持ったままで、
   * 順序付きへ戻せば元の並びがそのまま効く。
   */
  const kindMutation = useMutation({
    mutationFn: (collectionKind: "ordered" | "unordered") => upsertWorkCollection({
      id: collection.id,
      name: collection.name,
      description: collection.description,
      collectionKind,
      coverDownloadId: collection.coverDownloadId,
    }),
    onSuccess: (value) => {
      onChanged(value);
      notifications.show({
        color: "green",
        message: value.collectionKind === "ordered"
          ? "順序付きにしました。並べ替えと整列が使えます"
          : "順序なしにしました。並びは覚えたままです",
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "並び方を変えられません", message: errorMessage(error) }),
  });

  const coverMutation = useMutation({
    mutationFn: upsertWorkCollection,
    onSuccess: (value) => {
      onChanged(value);
      coverModal.close();
      notifications.show({ color: "green", message: "表紙を変えました" });
    },
    onError: (error) => notifications.show({ color: "red", title: "表紙を変えられません", message: errorMessage(error) }),
  });

  // 本文が取れていない作品も、一冊にはできない側に数える。行はあるが中身が
  // 無いので、そのまま入れると空の章になる。
  const emptyCount = collection.members.filter((member) => member.work && member.work.textLength === 0).length;
  const missingCount = collection.memberCount - collection.availableCount + emptyCount;
  const exportableCount = collection.availableCount - emptyCount;
  const epubMutation = useMutation({
    mutationFn: async (skipMissing: boolean) => {
      const outputDir = await openSingleDialog({ directory: true, title: "一冊にまとめたEPUBの出力先" });
      if (!outputDir) return null;
      // 画像最適化と組み方は、EPUB画面で決めたものをそのまま使う。ここだけ
      // 渡す口が塞がっていたので、**いちばん大きくなる本だけが無圧縮**だった。
      const settings = readExportSettings();
      return exportCollectionEpub(collection.id, "__auto__", outputDir, skipMissing, toCompressOptions(settings.compression), settings.writingMode);
    },
    onSuccess: (path) => { if (path) notifications.show({ color: "green", title: "一冊のEPUBを書き出しました", message: path }); },
    onError: (error) => notifications.show({ color: "red", title: "EPUBを書き出せません", message: errorMessage(error) }),
  });
  // 欠落は書き出しを塞ぐ理由にしない。設計提案 13-3 のとおり、除外して続けるか
  // 中止するかを利用者が選ぶ。確認は出力先ダイアログより前に出す。
  // 収録できない作品は、数ではなく題名で見せる。「3作品を除きます」だけでは、
  // 落ちるのが読みたかった一冊なのかどうかが分からない。
  const missingMembers = collection.members.filter(
    (member) => !member.work || member.work.textLength === 0,
  );
  const startEpubExport = () => {
    if (missingCount <= 0) { epubMutation.mutate(false); return; }
    modals.openConfirmModal({
      title: "未保存の作品があります",
      children: (
        <Stack gap="xs">
          <Text size="sm">
            「{collection.name}」の次の{formatNumber(missingCount)}作品は、ライブラリに無いか本文が取れていないため収録できません。
            これを除いて、残り{formatNumber(exportableCount)}作品で一冊にしますか？
          </Text>
          <NamedWorkList works={missingMembers} />
        </Stack>
      ),
      labels: { confirm: "除外して書き出す", cancel: "中止する" },
      onConfirm: () => epubMutation.mutate(true),
    });
  };

  const reorderTo = async (members: WorkCollectionMember[]): Promise<boolean> => {
    const previous = collection;
    // mosaic/spine は先頭4件から作る。本文の並びだけ先に動かして表紙を
    // 古いままにすると、保存中だけ二つの順序が同時に見えてしまう。
    const optimistic = optimisticCollectionOrder(collection, members);
    // 掴んだ瞬間に画面を動かし、保存に失敗したときだけ元へ戻す。
    onChanged(optimistic);
    try {
      await mutation.mutateAsync({ type: "reorder", keys: members.map(workKey) });
      return true;
    } catch {
      onChanged(previous);
      return false;
    }
  };

  const move = async (index: number, delta: number) => {
    const next = [...collection.members];
    const target = index + delta;
    if (target < 0 || target >= next.length) return false;
    [next[index], next[target]] = [next[target], next[index]];
    return reorderTo(next);
  };
  /** 掴んで運ぶ並べ替えは、抜いて差し込む。入れ替えではない。 */
  const dropAt = async (from: number, to: number) => {
    const next = [...collection.members];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    return reorderTo(next);
  };

  /**
   * 一括の整列。
   *
   * 束が大きくなるほど、1つずつ動かすのは現実的でなくなる。前後編が20組ある
   * 束を手で並べ直す人はいない。
   *
   * 判断は保存側でする。話数の語彙（周目・丸数字・part・先頭の `03：`）を
   * 画面側にも置くと、片方だけ直したときに黙ってずれる。
   */
  const sortBy = (mode: "published" | "episode") => mutation.mutate({ type: "sort", mode });

  const deleteCollection = () => modals.openConfirmModal({
    title: "コレクションを削除しますか？",
    children: <Text size="sm">作品そのものは削除されません。「{collection.name}」というまとまりと並びだけを削除します。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" }, confirmProps: { color: "red" },
    // Deleting touches the same caches every other edit on this screen does.
    // Invalidating only the list left the work and author pages still counting
    // this collection in their 所属 menus. The deleted collection's own entry is
    // removed rather than invalidated: refetching it would only ask for a row
    // that is gone.
    onConfirm: async () => { try { await deleteWorkCollection(collection.id); queryClient.removeQueries({ queryKey: ["work-collection", collection.id] }); invalidateCollectionViews(queryClient); navigate("/library?tab=collections"); } catch (error) { notifications.show({ color: "red", title: "削除できません", message: errorMessage(error) }); } },
  });

  // 選んで外すのは、この画面で唯一**確認を出さない**まとめ操作だった。
  // 1件ずつ外すときも、束ごと消すときも窓が出るのに、選んだぶんをまとめて
  // 外すときだけ、押した瞬間に消えていた。並びも一緒に失われる。
  const removeSelected = () => {
    const chosen = collection.members
      .filter((member) => member.downloadId !== null && selectedSet.has(member.downloadId));
    if (chosen.length === 0) return;
    modals.openConfirmModal({
      title: "選んだ作品をこのコレクションから外しますか？",
      children: (
        <Stack gap="xs">
          <Text size="sm">
            次の{formatNumber(chosen.length)}作品を「{collection.name}」から外します。
            作品そのものは棚に残ります。
          </Text>
          <NamedWorkList works={chosen} />
        </Stack>
      ),
      labels: { confirm: "外す", cancel: "キャンセル" },
      confirmProps: { color: "red" },
      onConfirm: () => mutation.mutate({ type: "remove", keys: chosen.map(workKey) }),
    });
  };

  const assistItems = [
    {
      id: "collection_naming",
      label: "名前を考えてもらう",
      description: "題名とタグを見て名前と説明を提案します",
      enabled: !readOnly && Boolean(namingEngine),
      unavailableReason: readOnly ? "このコレクションは編集できません" : "命名機能を設定すると使えます",
      onSelect: () => onEdit(true),
    },
    {
      id: "collection_split",
      label: "分け方を考えてもらう",
      description: "内容のまとまりから分割案を作ります",
      enabled: !readOnly && collection.availableCount >= 4 && Boolean(splitEngine),
      unavailableReason: collection.availableCount < 4 ? "4作品以上で利用できます" : "分割提案機能を設定すると使えます",
      onSelect: splitModal.open,
    },
  ];
  const headerHosted = usePageAssist(
    `collection-assist-${collection.id}`,
    "このコレクションで使えるAIの手伝い",
    assistItems,
  );

  return (
    <div className="page">
      <Stack gap="lg">
        {readOnly && (
          <Alert color="gray">
            プレビューは閲覧専用です。コレクションの変更と書き出しはデスクトップアプリで利用できます。
          </Alert>
        )}
        <Paper withBorder p="md" className="collection-hero">
          <Group wrap="nowrap" align="center" gap="md">
            <div className="collection-hero__cover">
              <CollectionCover collection={collection} variant="detail" />
              <Button variant="subtle" size="compact-xs" color="gray" disabled={readOnly} onClick={coverModal.open}>表紙を変える</Button>
            </div>
            <Stack gap={6} flex={1} miw={0}>
              <Text className="page-header__eyebrow">コレクション</Text>
              <Text component="h1" fw={800} fz={24} lh={1.3}>{collection.name}</Text>
              {collection.description && <Text size="sm" c="dimmed">{collection.description}</Text>}
              <Group gap="xs" wrap="wrap">
                <Badge variant="light" color={ordered ? "piep" : "gray"}>{ordered ? "順序付き" : "順序なし"}</Badge>
                <Text size="sm" c="dimmed">{formatNumber(collection.memberCount)}作品 · {formatNumber(collection.totalTextLength)}字</Text>
                {missingCount > 0 && <Badge color="orange" variant="light">収録できない {formatNumber(missingCount)}作品</Badge>}
              </Group>
              <Group gap="xs" mt={4} wrap="wrap">
                <Button leftSection={<Icons.add size={IconSize.action} />} disabled={readOnly} onClick={addModal.open}>作品を追加</Button>
                <Button variant="default" leftSection={<Icons.epub size={IconSize.action} />} loading={epubMutation.isPending} disabled={readOnly || exportableCount <= 0} onClick={startEpubExport}>一冊のEPUB</Button>
                <Button variant="default" disabled={readOnly} onClick={() => onEdit(false)}>名前を付け直す</Button>
                {/* 「作品を追加」の隣に置く。どちらも束へ作品を入れる操作で、
                    違うのは**誰が探すか**だけである。 */}
                <Button
                  variant="default"
                  leftSection={<Icons.collectionSuggest size={IconSize.action} />}
                  disabled={readOnly || collection.availableCount === 0}
                  onClick={additionsModal.open}
                >
                  合う作品を探す
                </Button>
                {!headerHosted && <AssistLauncher items={assistItems} />}
                <Menu position="bottom-end">
                  <Menu.Target>
                    <ActionIcon variant="default" size="lg" aria-label="このコレクションのその他の操作"><Icons.more size={IconSize.nav} /></ActionIcon>
                  </Menu.Target>
                  <Menu.Dropdown>
                    <Menu.Label>並べ替え</Menu.Label>
                    {/* 並び方の切り替えは、それが効くものの隣に置く。
                        //
                        // 作るときの一度きりしか選べなかった。保存側は最初から
                        // 更新に対応していて、編集用の画面まで作ってあったのに、
                        // 編集の入口が名前だけのモーダルへ移ったときに置き去りに
                        // なっていた。
                        //
                        // ここに置くと、下の二つが灰色になっている理由も同時に
                        // 分かる。順序なしの束に「整列」は無い。 */}
                    <Menu.Item
                      leftSection={<Icons.collection size={IconSize.menu} />}
                      disabled={readOnly || kindMutation.isPending}
                      onClick={() => kindMutation.mutate(ordered ? "unordered" : "ordered")}
                    >
                      {ordered ? "順序なしにする" : "順序付きにする"}
                    </Menu.Item>
                    <Menu.Item leftSection={<Icons.sort size={IconSize.menu} />} disabled={readOnly || !ordered || collection.members.length < 2} onClick={() => sortBy("published")}>投稿日順に整列</Menu.Item>
                    <Menu.Item leftSection={<Icons.sort size={IconSize.menu} />} disabled={readOnly || !ordered || collection.members.length < 2} onClick={() => sortBy("episode")}>題名の連番順に整列</Menu.Item>
                    <Menu.Divider />
                    <Menu.Item leftSection={<Icons.select size={IconSize.menu} />} disabled={readOnly || collection.availableCount === 0} onClick={() => { setSelectionMode(true); setSelected([]); }}>複数選択して外す</Menu.Item>
                    <Menu.Divider />
                    {/* 削除は最下部に大きく置いていた。押す機会より、押して
                        しまう機会のほうが多い場所だった。 */}
                    <Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} disabled={readOnly} onClick={deleteCollection}>コレクションを削除</Menu.Item>
                  </Menu.Dropdown>
                </Menu>
              </Group>
            </Stack>
          </Group>
        </Paper>

        {collection.members.length === 0 ? (
          <Paper withBorder p="xl">
            <Stack align="center" gap="sm">
              <Icons.collection size={IconSize.hero} />
              <Text fw={700}>まだ作品が入っていません</Text>
              <Text size="sm" c="dimmed" ta="center">作品を追加すると、ここから順番に読み進めたり一冊のEPUBにできます。</Text>
              <Button disabled={readOnly} onClick={addModal.open}>作品を追加</Button>
            </Stack>
          </Paper>
        ) : (
          <>
            <Group justify="space-between" wrap="wrap" className="collection-member-toolbar">
              <Group gap="xs" wrap="wrap">
                {/* 読み込み方は左端に置く。ここにあった「掴んで運ぶか、矢印で
                    入れ替えられます」は、取っ手も矢印も見えているものの説明で
                    しかなかった。並べ替えられない束でだけ、そう言えない理由を
                    残す（閲覧専用では取っ手も矢印も出てこない）。 */}
                <PagingModeToggle scope="collection-members" />
                {selectionMode ? (
                  <>
                    <Text size="sm" fw={650}>{formatNumber(selected.length)}件を選択中</Text>
                    <Button size="compact-sm" variant="subtle" color="red" disabled={selected.length === 0 || mutation.isPending} onClick={removeSelected}>選んだ作品を外す</Button>
                    <Button size="compact-sm" variant="subtle" color="gray" onClick={() => { setSelectionMode(false); setSelected([]); }}>やめる</Button>
                  </>
                ) : (!ordered || readOnly) && (
                  <Text size="sm" c="dimmed">{!ordered ? "順序なしのコレクションです。" : "順序付きのコレクションです。"}</Text>
                )}
              </Group>
              <Group gap="xs" wrap="nowrap">
                <SegmentedControl
                  className="view-mode-switch"
                  aria-label="コレクションの表示形式"
                  value={view}
                  onChange={(value) => setView(parseViewMode(value))}
                  data={[
                    { value: "gallery", label: <Tooltip label="表紙で見る"><Icons.viewGrid size={IconSize.menu} aria-label="表紙表示" /></Tooltip> },
                    { value: "compact", label: <Tooltip label="一覧で見る"><Icons.viewList size={IconSize.menu} aria-label="一覧表示" /></Tooltip> },
                  ]}
                />
              </Group>
            </Group>

            <CollectionMemberList
              members={collection.members}
              ordered={ordered}
              view={view}
              busy={readOnly || mutation.isPending}
              selectionMode={selectionMode}
              selected={selectedSet}
              onSelect={(id, next) => setSelected((current) => (next ? [...new Set([...current, id])] : current.filter((value) => value !== id)))}
              onMove={move}
              onDropAt={dropAt}
              onRemove={(member) => mutation.mutate({ type: "remove", keys: [workKey(member)] })}
              page={memberWindow}
            />

            {/* 番号で見るときだけ出す。「自動」のときは一覧が自分で続きを出す。 */}
            {numberedPages && memberCount > pageSize && (
              <ListPager
                scope="collection-members"
                hasNext={false}
                loading={false}
                loaded={memberWindow ? Math.min(memberWindow.end, memberCount) - memberWindow.start : memberCount}
                total={memberCount}
                onLoad={() => undefined}
                unit="作品"
                pages={{
                  current: currentPage,
                  size: pageSize,
                  onGoTo: goToPage,
                  maxDirectPage,
                  limitNotice: pageLimitNotice,
                }}
              />
            )}
          </>
        )}
      </Stack>

      {!readOnly && <CollectionAddWorksModal
        opened={addOpened}
        onClose={addModal.close}
        collection={collection}
        busy={mutation.isPending}
        onAdd={(works) => mutation.mutate({ type: "add", works })}
      />}
      {!readOnly && <CollectionAdditionsModal
        opened={additionsOpened}
        onClose={additionsModal.close}
        collection={collection}
        busy={mutation.isPending}
        onAdd={(downloadIds) => addFromSuggestions.mutate(downloadIds)}
      />}
      {!readOnly && <CollectionCoverModal
        opened={coverOpened}
        onClose={coverModal.close}
        collection={collection}
        busy={coverMutation.isPending}
        onSave={(input) => coverMutation.mutate(input)}
      />}
      {!readOnly && <CollectionSplitAssist
        opened={splitOpened}
        onClose={splitModal.close}
        collection={collection}
        onSplit={async (name, positions) => {
          const ids = positions
            .map((position) => collection.members[position]?.downloadId)
            .filter((id): id is number => typeof id === "number");
          if (ids.length === 0) return false;
          try {
            await splitMutation.mutateAsync({ name, ids });
            return true;
          } catch {
            // 失敗は splitMutation の onError が知らせる。案は窓に残す。
            return false;
          }
        }}
      />}
    </div>
  );
}
