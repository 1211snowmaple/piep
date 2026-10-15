import { useEffect, useMemo, useState } from "react";
import { Badge, Button, Chip, Group, Modal, Stack, Text, TextInput } from "@mantine/core";
import { useDebouncedValue } from "@mantine/hooks";
import { useQuery } from "@tanstack/react-query";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { WorkCard } from "@/components/WorkCard";
import { formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { searchDownloadsV2 } from "@/services/dbApi";
import type { WorkCollection } from "@/types/collections";
import type { DownloadEntry, SearchV2Params } from "@/types/library";

/** 検索語を打たなくても棚が出る、という前提を崩さない程度の件数。 */
const PAGE_SIZE = 60;

type Scope = "recent" | "authors" | "tags" | "series" | "favorite";

/**
 * 束に作品を入れる。
 *
 * ここまでは検索欄と文字だけのチェックボックスで、**題名を思い出せることが
 * 前提**だった。棚には表紙もタグも作者もあるのに、入れるときだけ全部消えて
 * いた。並べるものを棚と同じカードにして、思い出す代わりに見て選べるようにする。
 *
 * 絞り込みは「この束の続きを探している」という状況に合わせる。だいたいの場合、
 * 入れたい作品は同じ作者か、同じタグか、同じシリーズにいる。
 */
export function CollectionAddWorksModal({
  opened,
  onClose,
  collection,
  busy,
  onAdd,
}: {
  opened: boolean;
  onClose: () => void;
  collection: WorkCollection;
  busy: boolean;
  onAdd: (works: DownloadEntry[]) => void;
}) {
  const [scope, setScope] = useState<Scope>("authors");
  const [query, setQuery] = useState("");
  const [debouncedQuery] = useDebouncedValue(query, 250);
  // Keep the records, not only their ids. Search/scope changes replace the
  // visible result page, but choices made on a previous page must still be
  // present when the user confirms the add.
  const [selected, setSelected] = useState<Map<number, DownloadEntry>>(() => new Map());

  const memberIds = useMemo(
    () => new Set(collection.members.map((member) => member.downloadId).filter((id): id is number => id !== null)),
    [collection.members],
  );

  // 束の側からわかることを、そのまま絞り込みの手がかりにする。
  const authors = useMemo(() => {
    const names = new Set<string>();
    for (const member of collection.members) names.add(member.work?.authorName ?? member.authorName);
    return [...names].filter(Boolean);
  }, [collection.members]);

  const sharedTags = useMemo(() => {
    const counts = new Map<string, number>();
    let withWork = 0;
    for (const member of collection.members) {
      if (!member.work) continue;
      withWork += 1;
      for (const tag of member.work.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    const threshold = Math.max(2, Math.ceil(withWork / 2));
    return [...counts.entries()]
      .filter(([, count]) => count >= threshold)
      .sort((left, right) => right[1] - left[1])
      .slice(0, 4)
      .map(([tag]) => tag);
  }, [collection.members]);

  const seriesKey = useMemo(() => {
    for (const member of collection.members) {
      if (member.work?.seriesId) return { source: member.work.source, key: member.work.seriesId };
    }
    return null;
  }, [collection.members]);

  useEffect(() => {
    if (!opened) return;
    setSelected(new Map());
    setQuery("");
    // 手がかりの強い順に既定を決める。何も無ければ最近保存した順。
    setScope(authors.length > 0 ? "authors" : sharedTags.length > 0 ? "tags" : "recent");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opened]);

  const params = useMemo<SearchV2Params>(() => {
    const base: SearchV2Params = {
      query: debouncedQuery.trim() || null,
      sortBy: debouncedQuery.trim() ? "relevance" : "downloaded_at",
      sortOrder: "desc",
      limit: PAGE_SIZE,
      projection: "libraryGallery",
    };
    if (scope === "authors" && authors.length > 0) return { ...base, authorsInclude: authors };
    if (scope === "tags" && sharedTags.length > 0) {
      return { ...base, tagsInclude: sharedTags, tagFilterMode: "or" };
    }
    if (scope === "series" && seriesKey) {
      return { ...base, seriesSource: seriesKey.source, seriesKey: seriesKey.key };
    }
    if (scope === "favorite") return { ...base, favorite: true };
    return base;
  }, [authors, debouncedQuery, scope, seriesKey, sharedTags]);

  const searchQuery = useQuery({
    queryKey: ["collection-add-works", collection.id, params],
    queryFn: () => searchDownloadsV2(params),
    enabled: opened,
  });

  const results = searchQuery.data?.items ?? [];
  const selectedWorks = [...selected.values()];
  const toggle = (work: DownloadEntry, next: boolean) =>
    setSelected((current) => {
      const updated = new Map(current);
      if (next) updated.set(work.id, work); else updated.delete(work.id);
      return updated;
    });

  return (
    <Modal opened={opened} onClose={onClose} title="コレクションに作品を追加" size="xl" className="collection-add">
      <Stack gap="sm">
        <TextInput
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
          leftSection={<Icons.search size={IconSize.action} />}
          placeholder="題名・作者・本文から探す（空のままでも棚が出ます）"
          autoFocus
        />
        <Chip.Group multiple={false} value={scope} onChange={(value) => setScope(value as Scope)}>
          <Group gap={6} wrap="wrap">
            {authors.length > 0 && (
              <Chip value="authors" size="xs" variant="light">
                この束の作者{authors.length > 1 ? ` ${authors.length}人` : `「${authors[0]}」`}
              </Chip>
            )}
            {sharedTags.length > 0 && (
              <Chip value="tags" size="xs" variant="light">共有タグ {sharedTags.slice(0, 2).join("・")}</Chip>
            )}
            {seriesKey && <Chip value="series" size="xs" variant="light">同じシリーズ</Chip>}
            <Chip value="recent" size="xs" variant="light">最近保存した順</Chip>
            <Chip value="favorite" size="xs" variant="light">お気に入り</Chip>
          </Group>
        </Chip.Group>

        {searchQuery.isLoading ? (
          <LoadingState label="作品を読み込んでいます" />
        ) : searchQuery.error ? (
          <ErrorState error={searchQuery.error} retry={() => searchQuery.refetch()} />
        ) : results.length === 0 ? (
          <Text size="sm" c="dimmed" ta="center" py="xl">この絞り込みに当てはまる作品はありません。</Text>
        ) : (
          <div className="collection-add__grid">
            {results.map((work) => {
              const already = memberIds.has(work.id);
              return (
                <div key={work.id} className="collection-add__slot" data-member={already || undefined}>
                  <WorkCard
                    work={work}
                    compact
                    selectionMode
                    selected={already || selected.has(work.id)}
                    onSelect={already ? undefined : (_id, next) => toggle(work, next)}
                  />
                  {already && <Badge size="xs" variant="filled" color="gray" className="collection-add__badge">追加済み</Badge>}
                </div>
              );
            })}
          </div>
        )}

        <Group justify="space-between">
          <Text size="xs" c="dimmed">
            {results.length >= PAGE_SIZE
              ? `${formatNumber(PAGE_SIZE)}件まで表示しています。絞り込むか、検索語で狭めてください。`
              : `${formatNumber(results.length)}件`}
          </Text>
          <Group gap="xs">
            <Button variant="default" onClick={onClose}>キャンセル</Button>
            <Button disabled={selectedWorks.length === 0 || busy} loading={busy} onClick={() => onAdd(selectedWorks)}>
              {selectedWorks.length > 0 ? `${formatNumber(selectedWorks.length)}作品を追加` : "作品を選んでください"}
            </Button>
          </Group>
        </Group>
      </Stack>
    </Modal>
  );
}
