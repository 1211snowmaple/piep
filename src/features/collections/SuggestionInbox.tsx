import { useMemo, useState } from "react";
import { Alert, Badge, Box, Button, Card, Chip, Group, Menu, Modal, Stack, Text, Tooltip } from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate } from "@/app/router";
import { WorkCover } from "@/components/WorkCover";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import {
  acceptCollectionSuggestion,
  dismissCollectionSuggestion,
  dismissSweptSuggestions,
  listCollectionSuggestions,
  rejectCollectionSuggestion,
  suggestionNameOverride,
} from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { nameCollectionSuggestion } from "@/services/assistApi";
import { useAssist } from "@/features/assist/useAssist";
import type {
  CollectionSuggestion,
  CollectionSuggestionMember,
  SavedSearchSuggestion,
  WorkKey,
} from "@/types/collections";

const EMPTY: CollectionSuggestion[] = [];
/** 一度に出す件数。棚の走査は300件を超えることがあり、全部出しても読まれない。 */
const PAGE = 12;

type Track = "all" | "sequence" | "theme";

/** 取得元と作品IDを、どちらにも現れない区切りでつないだ鍵。 */
function memberKey(member: { source: string; sourceId: string }): string {
  return `${member.source}${member.sourceId}`;
}

/**
 * 見つかったまとまり。
 *
 * 空の棚に「まだコレクションはありません」と出すのをやめる。無いのではなく、
 * **まだ探していない**だけだったからである。棚を一度なめれば、前後編も
 * 連鎖ものも、作者をまたいだ題材の束も、すでにそこにある。
 *
 * ただし出す量は絞る。300件を並べて1件ずつ閉じさせるくらいなら、
 * 見つけないほうがましである。
 */
export function SuggestionInbox({ sweeping, savedSearchIdeas, onSweep }: {
  /** 走査は棚の見出しから始める。ここは結果を出すところに徹する。 */
  sweeping: boolean;
  savedSearchIdeas: SavedSearchSuggestion[];
  onSweep: () => void;
}) {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [track, setTrack] = useState<Track>("all");
  const [shownCount, setShownCount] = useState(PAGE);

  const suggestionsQuery = useQuery({
    queryKey: ["collection-suggestions", "pending"],
    queryFn: () => (runtime ? listCollectionSuggestions("pending") : Promise.resolve(EMPTY)),
  });
  const suggestions = suggestionsQuery.data ?? EMPTY;
  const counts = useMemo(() => ({
    all: suggestions.length,
    sequence: suggestions.filter((value) => value.track === "sequence").length,
    theme: suggestions.filter((value) => value.track === "theme").length,
  }), [suggestions]);
  const filtered = track === "all" ? suggestions : suggestions.filter((value) => value.track === track);
  const shown = filtered.slice(0, shownCount);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] });
    queryClient.invalidateQueries({ queryKey: ["work-collections"] });
  };

  const dismissAll = useMutation({
    mutationFn: (scope: Track) => dismissSweptSuggestions(scope === "all" ? undefined : scope),
    onSuccess: (removed) => {
      invalidate();
      notifications.show({ message: `${formatNumber(removed)}件の候補を閉じました` });
    },
    onError: (error) => notifications.show({ color: "red", title: "候補を閉じられません", message: errorMessage(error) }),
  });

  const confirmDismissAll = (scope: Track) => modals.openConfirmModal({
    title: "候補をまとめて閉じますか？",
    children: (
      <Text size="sm">
        {scope === "all" ? "走査で見つかった候補すべて" : scope === "sequence" ? "続き物の候補すべて" : "テーマの候補すべて"}を
        画面から消します。<b>「二度と出さない」とは記録しません</b>ので、もう一度走査すれば同じものが出てきます。
        作ったコレクションはそのまま残ります。
      </Text>
    ),
    labels: { confirm: "閉じる", cancel: "キャンセル" },
    onConfirm: () => dismissAll.mutate(scope),
  });

  if (!runtime) return null;

  // 候補も副産物も無いときは何も出さない。走査を始める操作は棚の見出しに
  // 一本だけ置いてある。ここに二本目を置くと、行が一つ増えるだけだった。
  if (suggestionsQuery.isSuccess && counts.all === 0 && savedSearchIdeas.length === 0) return null;


  return (
    <Stack gap="md">
      <Group justify="space-between" align="flex-start" wrap="nowrap">
        <Box miw={0}>
          <Group gap="xs">
            <Icons.collectionSuggest size={IconSize.action} />
            <Text fw={700}>見つかったまとまり</Text>
            {counts.all > 0 && <Badge>{formatNumber(counts.all)}</Badge>}
          </Group>
          <Text size="sm" c="dimmed">
            棚を一度なめて、続き物と、題材の近い束を洗い出します。採用するまで何も変わりません。
          </Text>
        </Box>
        <Group gap="xs" wrap="nowrap">
          {counts.all > 0 && (
            <Menu position="bottom-end">
              <Menu.Target>
                <Button variant="default" rightSection={<Icons.more size={IconSize.menu} />} loading={dismissAll.isPending}>
                  まとめて
                </Button>
              </Menu.Target>
              <Menu.Dropdown>
                <Menu.Label>候補を閉じる</Menu.Label>
                <Menu.Item leftSection={<Icons.hide size={IconSize.menu} />} onClick={() => confirmDismissAll("all")}>
                  すべて閉じる（{formatNumber(counts.all)}件）
                </Menu.Item>
                <Menu.Item disabled={counts.sequence === 0} leftSection={<Icons.hide size={IconSize.menu} />} onClick={() => confirmDismissAll("sequence")}>
                  続き物だけ閉じる（{formatNumber(counts.sequence)}件）
                </Menu.Item>
                <Menu.Item disabled={counts.theme === 0} leftSection={<Icons.hide size={IconSize.menu} />} onClick={() => confirmDismissAll("theme")}>
                  テーマだけ閉じる（{formatNumber(counts.theme)}件）
                </Menu.Item>
              </Menu.Dropdown>
            </Menu>
          )}
          <Button
            variant="light"
            leftSection={<Icons.search size={IconSize.action} />}
            loading={sweeping}
            onClick={onSweep}
          >
            棚から探す
          </Button>
        </Group>
      </Group>

      {sweeping && (
        <Alert icon={<Icons.collectionSuggest size={IconSize.action} />}>
          本文のリンク、題名の連番、公式シリーズの連番をたどってから、タグと本文の近さで題材の束を探しています。
        </Alert>
      )}

      {counts.all > 0 && (
        <Chip.Group multiple={false} value={track} onChange={(value) => { setTrack(value as Track); setShownCount(PAGE); }}>
          <Group gap={6}>
            <Chip value="all" size="xs" variant="light">すべて {formatNumber(counts.all)}</Chip>
            <Chip value="sequence" size="xs" variant="light">続き物 {formatNumber(counts.sequence)}</Chip>
            <Chip value="theme" size="xs" variant="light">テーマ {formatNumber(counts.theme)}</Chip>
          </Group>
        </Chip.Group>
      )}

      {savedSearchIdeas.length > 0 && (
        <Alert color="gray" icon={<Icons.savedSearch size={IconSize.action} />} title="束にしなかったもの">
          <Stack gap={6}>
            <Text size="sm">
              次のタグは付いている作品が多すぎて、読む単位にはなりません。
              まとまりではなく<b>絞り込み</b>として持つほうが扱いやすいので、保存した検索にしておけます。
            </Text>
            <Group gap={6} wrap="wrap">
              {savedSearchIdeas.map((idea) => (
                <Tooltip key={idea.tag} label={idea.reason} multiline maw={340}>
                  <Button
                    size="compact-xs"
                    variant="default"
                    leftSection={<Icons.filter size={IconSize.inline} />}
                    onClick={() => navigate(`/library?tag=${encodeURIComponent(idea.tag)}`)}
                  >
                    {idea.tag} {formatNumber(idea.workCount)}
                  </Button>
                </Tooltip>
              ))}
            </Group>
            <Text size="xs" c="dimmed">
              押すとその絞り込みで棚を開きます。気に入ったら、そこで「検索を保存」してください。
            </Text>
          </Stack>
        </Alert>
      )}

      <div className="suggestion-grid">
        {shown.map((suggestion) => (
          <SuggestionBundleCard key={suggestion.id} suggestion={suggestion} onChanged={invalidate} />
        ))}
      </div>

      {filtered.length > shown.length && (
        <Group justify="center">
          <Button variant="subtle" onClick={() => setShownCount((current) => current + PAGE)}>
            もっと見る（残り {formatNumber(filtered.length - shown.length)}件）
          </Button>
        </Group>
      )}
    </Stack>
  );
}

/**
 * 束ひとつぶんの候補。
 *
 * 出す情報は三つだけ — 何が入るか、なぜ束なのか、名前をどうするか。
 * かつて出していた「確度 74%」は、この三つのどれでもなかった。
 */
function SuggestionBundleCard({ suggestion, onChanged }: { suggestion: CollectionSuggestion; onChanged: () => void }) {
  const navigate = useAppNavigate();
  const [name, setName] = useState(suggestion.proposedName);
  const [excluded, setExcluded] = useState<ReadonlySet<string>>(new Set());
  const [membersOpened, membersModal] = useDisclosure(false);
  const keys = suggestion.members
    .filter((member) => !excluded.has(memberKey(member)))
    .map((member) => ({ source: member.source, sourceId: member.sourceId }) as WorkKey);

  const accept = useMutation({
    mutationFn: () => acceptCollectionSuggestion({
      suggestionId: suggestion.id,
      memberKeys: keys,
      name: suggestionNameOverride(suggestion.proposedName, name),
    }),
    onSuccess: (created) => { onChanged(); navigate(`/collections/${created.id}`); },
    onError: (error) => notifications.show({ color: "red", title: "採用できません", message: errorMessage(error) }),
  });
  const dismiss = useMutation({
    mutationFn: () => dismissCollectionSuggestion(suggestion.id),
    onSuccess: () => { onChanged(); notifications.show({ message: "この候補を閉じました" }); },
    onError: (error) => notifications.show({ color: "red", title: "閉じられません", message: errorMessage(error) }),
  });
  const reject = useMutation({
    mutationFn: () => rejectCollectionSuggestion(suggestion.id, keys),
    onSuccess: () => { onChanged(); notifications.show({ message: "この組合せは今後の候補から外します" }); },
    onError: (error) => notifications.show({ color: "red", title: "外せません", message: errorMessage(error) }),
  });

  // 命名エンジンは既定で切ってある。切ってあるときはボタンも出さない。
  const { engine } = useAssist();
  const askModel = useMutation({
    mutationFn: () => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return nameCollectionSuggestion(suggestion.id, engine);
    },
    onSuccess: (updated) => {
      const proposed = updated.nameOptions.find((option) => option.source === "llm");
      if (proposed) setName(proposed.name);
      onChanged();
    },
    onError: (error) => notifications.show({ color: "red", title: "モデルが名前を返しません", message: errorMessage(error) }),
  });
  const busy = accept.isPending || dismiss.isPending || reject.isPending || askModel.isPending;

  const confirmReject = () => modals.openConfirmModal({
    title: "この組合せを今後の候補から外しますか？",
    children: <Text size="sm">いま残っている{formatNumber(keys.length)}作品を、同じ規則では再提案しません。外した作品は対象になりません。規則が変われば改めて評価されます。</Text>,
    labels: { confirm: "候補から外す", cancel: "キャンセル" },
    confirmProps: { color: "red" },
    onConfirm: () => reject.mutate(),
  });

  const toggle = (key: string) => setExcluded((current) => {
    const next = new Set(current);
    if (next.has(key)) next.delete(key); else next.add(key);
    return next;
  });

  return (
    <Card withBorder padding="md" className="suggestion-card">
      <Stack gap="sm">
        <Group justify="space-between" align="flex-start" wrap="nowrap">
          <Box miw={0}>
            <Text fw={720} className="line-clamp-2">{name}</Text>
            <Text size="sm" c="dimmed">{suggestion.evidenceSummary}</Text>
          </Box>
          <Badge variant="light" color={suggestion.track === "theme" ? "grape" : "piep"}>
            {suggestion.track === "theme" ? "テーマ" : "続き物"}
          </Badge>
        </Group>

        {/* 表紙を並べる。題名の行だけでは、知っている束かどうかが分からない。
            押すとその作品だけ束から外れる。 */}
        <div className="suggestion-card__covers">
          {suggestion.members.slice(0, 5).map((member) => (
            <SuggestionCoverButton
              key={memberKey(member)}
              member={member}
              dropped={excluded.has(memberKey(member))}
              onToggle={() => toggle(memberKey(member))}
            />
          ))}
          {suggestion.members.length > 5 && (
            <Tooltip label="すべての作品を表紙で見る">
              <button type="button" className="suggestion-card__more" onClick={membersModal.open}>
                ＋{formatNumber(suggestion.members.length - 5)}
              </button>
            </Tooltip>
          )}
        </div>

        <Group gap={4} wrap="wrap">
          {suggestion.nameOptions.map((option) => (
            <Tooltip key={`${option.source}-${option.name}`} label={option.label}>
              <Chip
                size="xs"
                variant="light"
                color={option.source === "llm" ? "grape" : undefined}
                checked={name === option.name}
                onChange={() => setName(option.name)}
              >
                {option.name}
              </Chip>
            </Tooltip>
          ))}
          {engine && !suggestion.nameOptions.some((option) => option.source === "llm") && (
            <Button
              variant="subtle"
              size="compact-xs"
              color="grape"
              leftSection={<Icons.optimize size={IconSize.inline} />}
              loading={askModel.isPending}
              disabled={busy}
              onClick={() => askModel.mutate()}
            >
              モデルに考えてもらう
            </Button>
          )}
        </Group>

        <Group justify="space-between" wrap="nowrap">
          <Group gap={4}>
            <Button variant="subtle" color="gray" size="compact-xs" disabled={busy} onClick={() => dismiss.mutate()}>あとで</Button>
            <Button variant="subtle" color="red" size="compact-xs" disabled={busy || keys.length === 0} onClick={confirmReject}>二度と出さない</Button>
          </Group>
          <Button
            size="compact-sm"
            leftSection={<Icons.confirm size={IconSize.menu} />}
            loading={accept.isPending}
            disabled={busy || keys.length === 0}
            onClick={() => accept.mutate()}
          >
            {formatNumber(keys.length)}作品で作る
          </Button>
        </Group>
      </Stack>

      {/* 中身を表紙で確かめる。カードに全部並べると1枚が小さくなりすぎるので、
          広げたいときだけ広げる。 */}
      <Modal opened={membersOpened} onClose={membersModal.close} title={name} size="lg">
        <Stack gap="sm">
          <Text size="sm" c="dimmed">{suggestion.evidenceSummary}</Text>
          <Text size="xs" c="dimmed">表紙を押すと、その作品だけ束から外れます。</Text>
          <div className="suggestion-members">
            {suggestion.members.map((member) => (
              <div key={memberKey(member)} className="suggestion-members__item">
                <SuggestionCoverButton
                  member={member}
                  dropped={excluded.has(memberKey(member))}
                  onToggle={() => toggle(memberKey(member))}
                  large
                />
                <Text size="xs" className="line-clamp-2">{member.title}</Text>
                <Text size="xs" c="dimmed" className="line-clamp-1">{member.authorName}</Text>
              </div>
            ))}
          </div>
          <Group justify="flex-end">
            <Button variant="default" onClick={membersModal.close}>閉じる</Button>
          </Group>
        </Stack>
      </Modal>
    </Card>
  );
}

function SuggestionCoverButton({ member, dropped, onToggle, large = false }: {
  member: CollectionSuggestionMember;
  dropped: boolean;
  onToggle: () => void;
  large?: boolean;
}) {
  return (
    <Tooltip label={`${member.title}${dropped ? "（外しています）" : ""}`} multiline maw={320}>
      <button
        type="button"
        className="suggestion-card__cover"
        data-dropped={dropped || undefined}
        data-large={large || undefined}
        aria-label={dropped ? `${member.title}を戻す` : `${member.title}を外す`}
        aria-pressed={!dropped}
        onClick={onToggle}
      >
        <WorkCover work={member} variant="compact" />
      </button>
    </Tooltip>
  );
}
