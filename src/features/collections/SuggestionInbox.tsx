import { useMemo, useState } from "react";
import { Alert, Badge, Box, Button, Card, Chip, Collapse, Group, Stack, Text, Tooltip, UnstyledButton } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate } from "@/app/router";
import { NamedWorkList } from "@/components/NamedWorkList";
import { WorkCover } from "@/components/WorkCover";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import {
  acceptCollectionSuggestion,
  dismissCollectionSuggestion,
  listCollectionSuggestions,
  rejectCollectionSuggestion,
  suggestionNameOverride,
} from "@/services/collectionApi";
import { isTauriRuntime } from "@/services/dbApi";
import { demoSuggestions } from "@/mocks/demoData";
import { nameCollectionSuggestion } from "@/services/assistApi";
import { useAssist } from "@/features/assist/useAssist";
import { AssistLauncher } from "@/features/assist/AssistLauncher";
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
 * 見つけないほうがましである。走査そのものが系統ごとに8件しか作らなく
 * なったので、ここへ届く時点ですでに少ない。
 *
 * 置き場所はオーバーレイである。棚の一覧の途中に居座ると、候補が出ている間
 * ずっとコレクションそのものが下へ押し出される。「名前を付け直す」と同じで、
 * これは**始めて、終わらせて、閉じる**たぐいの操作なので、棚の上ではなく
 * 棚の手前でやる。
 */
export function SuggestionInbox({ sweeping, savedSearchIdeas, note }: {
  /** 走査はオーバーレイの操作列から始める。ここは結果を出すところに徹する。 */
  sweeping: boolean;
  savedSearchIdeas: SavedSearchSuggestion[];
  /** 意味索引が読めなかったなど、探しきれなかった事情。 */
  note?: string | null;
}) {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [track, setTrack] = useState<Track>("all");
  // 副産物は畳んでおく。毎回同じ内容なので、開いたままにする理由がない。
  const [savedSearchOpen, setSavedSearchOpen] = useState(false);
  const [shownCount, setShownCount] = useState(PAGE);

  const suggestionsQuery = useQuery({
    // プレビューでは見本を出す。空のままだと、カードの崩れがデスクトップ版で
    // しか見つからない。実際、2作の束で構成作品が読めないことに気づいたのは
    // 実機の画面を見てからだった。
    queryKey: ["collection-suggestions", "pending"],
    queryFn: () => (runtime ? listCollectionSuggestions("pending") : Promise.resolve(demoSuggestions)),
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


  return (
    <Stack gap="md">
      {sweeping && (
        <Alert icon={<Icons.collectionSuggest size={IconSize.action} />}>
          本文のリンク、題名の連番、公式シリーズの連番をたどってから、タグと本文の近さで題材の束を探しています。
        </Alert>
      )}

      {/* 探しきれなかった事情は、結果の隣に出す。意味索引が読めないことと、
          題材の束が本当に無いことを、同じ「見つかりませんでした」で
          済ませない。 */}
      {!sweeping && note && (
        <Alert color="yellow" icon={<Icons.warning size={IconSize.action} />} title="一部しか探せていません">
          {note}
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

      {/* 束にしなかったタグは、結果の**下**へ置いて畳んでおく。
          //
          // これは走査の副産物であり、しかも棚の性質なので**毎回同じものが
          // 出る**。「洗脳 1,720」は今回見つけたことではない。毎回同じ内容が
          // 本題の上に240px居座っていたのは、順序の間違いだった。 */}
      {savedSearchIdeas.length > 0 && (
        <div>
          <Button
            variant="subtle"
            color="gray"
            size="compact-sm"
            leftSection={<Icons.savedSearch size={IconSize.menu} />}
            rightSection={<Icons.expand size={IconSize.menu} style={{ transform: savedSearchOpen ? "rotate(180deg)" : undefined }} />}
            onClick={() => setSavedSearchOpen((open) => !open)}
          >
            束にしなかったタグ {formatNumber(savedSearchIdeas.length)}件
          </Button>
          <Collapse expanded={savedSearchOpen}>
            <Stack gap={6} mt="xs">
              <Text size="sm" c="dimmed">
                付いている作品が多すぎて、読む単位になりません。まとまりではなく<b>絞り込み</b>として
                持つほうが扱いやすいので、押すとその絞り込みで棚を開きます。
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
            </Stack>
          </Collapse>
        </div>
      )}

      {/* オーバーレイの中なので、空でも黙って消えるわけにはいかない。
          押した人は結果を見に来ている。 */}
      {!sweeping && suggestionsQuery.isSuccess && counts.all === 0 && (
        <Stack align="center" gap="xs" py="lg">
          <Icons.collectionSuggest size={IconSize.hero} />
          <Text fw={700}>いま出せるまとまりはありません</Text>
          <Text size="sm" c="dimmed" ta="center" maw={420}>
            確かだと言えるものだけを出しています。作品が増えたあとや、
            もう一度探したときに、別の束が見つかることがあります。
          </Text>
        </Stack>
      )}
      {suggestionsQuery.error && (
        <Alert color="red" title="候補を読み込めません">
          {errorMessage(suggestionsQuery.error)}
        </Alert>
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
  const [namesOpen, setNamesOpen] = useState(false);
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

  // 選ばれていない案の数。0 なら開く先が無いので、畳む操作そのものを出さない。
  const others = suggestion.nameOptions.filter((option) => option.name !== name);

  const { engine } = useAssist("collection_naming");
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

  // これは棚に**記録を書く**操作である。同じ組合せは今後の候補に出てこない。
  // 件数だけを見せて押させてよい操作ではないので、対象の題名をそのまま並べる。
  const kept = suggestion.members.filter((member) => !excluded.has(memberKey(member)));
  const confirmReject = () => modals.openConfirmModal({
    title: "この組合せを今後の候補から外しますか？",
    children: (
      <Stack gap="xs">
        <Text size="sm">
          次の{formatNumber(kept.length)}作品を、同じ規則では再提案しません。
          外した作品は対象になりません。規則が変われば改めて評価されます。
        </Text>
        <NamedWorkList works={kept} />
      </Stack>
    ),
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
            {/* 採る名前そのものを2行で切っていた。押す前に読めない名前を
                付けさせない。 */}
            <Text fw={720}>{name}</Text>
            <Text size="sm" c="dimmed">{suggestion.evidenceSummary}</Text>
          </Box>
          {/* 潰させない。名前の隣で縮んで「続..」になっていた。3文字の紋が
              読めないなら、置いている意味がない。 */}
          <Badge
            variant="light"
            style={{ flexShrink: 0 }}
            color={suggestion.track === "theme" ? "grape" : "piep"}
          >
            {suggestion.track === "theme" ? "テーマ" : "続き物"}
          </Badge>
        </Group>

        {/* 何が入るのかを、題名で読めるようにする。
            //
            // 表紙を5枚並べるだけだった。表紙の無い作品は真っ黒な四角で、
            // 題名はツールチップの中にしか無く、しかも一覧を開く「＋N」は
            // **6作以上のときしか出なかった**。2作の束では、何を束ねようと
            // しているのかを読む方法が一つも無かったことになる。
            //
            // 「2作品で作る」も「二度と出さない」も、中身を見ずに押させて
            // よい操作ではない。前者は棚に束を作り、後者はその組合せを
            // 今後の候補から永久に外す。 */}
        <SuggestionMemberList
          members={suggestion.members}
          excluded={excluded}
          onToggle={toggle}
        />

        {/* 名前の案は畳んでおく。
            //
            // 全文を縦に並べていたので、カードの**42%**が名前の案だった。
            // 構成作品（178px）より大きい。しかも名前は作ったあとで
            // 「名前を付け直す」からいつでも直せるので、ここで必ず決める
            // 必要はない。既定でよければ触らずに済むのが正しい。 */}
        <Stack gap={4}>
          {others.length > 0 && (
            <Button
              variant="subtle"
              color="gray"
              size="compact-xs"
              px={4}
              styles={{ root: { alignSelf: "flex-start" }, label: { fontWeight: 500 } }}
              rightSection={<Icons.expand size={IconSize.inline} style={{ transform: namesOpen ? "rotate(180deg)" : undefined }} />}
              onClick={() => setNamesOpen((open) => !open)}
            >
              他の名前の案 {formatNumber(others.length)}
            </Button>
          )}
          <Collapse expanded={namesOpen}>
            {/* 出すのは**他の**案だけ。いま選ばれている名前はカードの見出しに
                大きく出ているので、ここへもう一度並べると数が合わなくなる
                （「他の案 2」と書いて3行出ていた）。押せば入れ替わり、
                入れ替わったぶんがこの一覧に戻ってくる。 */}
            <Stack gap={4}>
              {others.map((option) => (
                <UnstyledButton
                  key={`${option.source}-${option.name}`}
                  className="collection-pick"
                  onClick={() => setName(option.name)}
                >
                  <span className="collection-pick__body">
                    <Text size="sm" fw={600}>{option.name}</Text>
                    <Text size="xs" c="dimmed">
                      {option.label}
                      {option.source === "llm" && option.modelId ? ` · ${option.modelId}` : ""}
                      {option.source === "llm" && option.createdAt
                        ? ` · ${new Date(option.createdAt).toLocaleDateString("ja-JP")}`
                        : ""}
                    </Text>
                  </span>
                </UnstyledButton>
              ))}
            </Stack>
          </Collapse>
          <AssistLauncher
            size="sm"
            label="この候補で使える手伝い"
            items={[{
              id: "collection_naming",
              label: suggestion.nameOptions.some((option) => option.source === "llm") ? "名前案を作り直す" : "名前案を追加する",
              description: "候補作品の題名とタグから名前を考えます",
              enabled: Boolean(engine) && !busy,
              unavailableReason: engine ? "ほかの操作が終わるまで待ってください" : "設定でコレクション命名を有効にしてください",
              onSelect: () => askModel.mutate(),
              badge: suggestion.nameOptions.some((option) => option.source === "llm") ? "生成済み" : undefined,
            }]}
          />
        </Stack>

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

    </Card>
  );
}

/** カードの中に畳まずに出しておく作品の数。 */
const VISIBLE_MEMBERS = 4;

/**
 * 束に入る作品を、題名で読めるように並べる。
 *
 * 表紙だけでは足りない。この棚の作品は表紙を持たないものが多く、持っていても
 * 縮めれば何の話か分からない。**判断の材料は題名と作者である。**
 *
 * 一行ずつが入切の切り替えになっている。以前は表紙を押すと外れたが、押せると
 * 分かる手がかりが何も無かった。チェックの印を出して、押せることと、いま
 * 入っているかどうかを同じ場所で見せる。
 */
function SuggestionMemberList({ members, excluded, onToggle }: {
  members: CollectionSuggestionMember[];
  excluded: ReadonlySet<string>;
  onToggle: (key: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  // 畳むのは、長い束でカードが縦に伸びきらないようにするため。開くのは
  // このカードの中で、別の窓を重ねない。
  const shown = expanded ? members : members.slice(0, VISIBLE_MEMBERS);
  const hidden = members.length - shown.length;

  return (
    <div className="suggestion-members">
      {shown.map((member) => {
        const key = memberKey(member);
        const dropped = excluded.has(key);
        return (
          <button
            key={key}
            type="button"
            className="suggestion-member"
            data-dropped={dropped || undefined}
            aria-pressed={!dropped}
            aria-label={dropped ? `${member.title}を束に戻す` : `${member.title}を束から外す`}
            onClick={() => onToggle(key)}
          >
            <span className="suggestion-member__mark" aria-hidden="true">
              {dropped ? <Icons.cancel size={IconSize.inline} /> : <Icons.confirm size={IconSize.inline} />}
            </span>
            <span className="suggestion-member__cover">
              <WorkCover work={member} variant="compact" />
            </span>
            <span className="suggestion-member__body">
              {/* 題名は最後まで出す。折らない。
                  //
                  // 2行で切っていたが、この棚の題名は長く、しかも**同じ書き出しで
                  // 始まる作品が束になる**。切った結果が二作とも
                  // 「鉄壁の聖騎士さまが催眠ねっちょりポリネシアンセックスで…」に
                  // なり、見分けるための一文字が一つ残らず省略の向こうへ行った。
                  // 見分けられない一覧は、無いのと同じである。 */}
              <Text size="sm" fw={600}>{member.title}</Text>
              <Text size="xs" c="dimmed">
                {member.authorName}
                {member.textLength > 0 ? ` · ${formatNumber(member.textLength)}字` : ""}
              </Text>
            </span>
          </button>
        );
      })}
      {hidden > 0 && (
        <Button variant="subtle" size="compact-xs" onClick={() => setExpanded(true)}>
          残り{formatNumber(hidden)}作品を見る
        </Button>
      )}
      {expanded && members.length > VISIBLE_MEMBERS && (
        <Button variant="subtle" size="compact-xs" color="gray" onClick={() => setExpanded(false)}>
          畳む
        </Button>
      )}
    </div>
  );
}
