import { useState } from "react";
import {
  ActionIcon,
  Avatar,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Collapse,
  Divider,
  Grid,
  Group,
  Menu,
  Paper,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { useForm, type UseFormReturnType } from "@mantine/form";
import { modals } from "@mantine/modals";
import { Icons, IconSize } from "@/lib/icons";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { errorMessage, formatDate, formatNumber } from "@/lib/format";
import { ProviderMark } from "@/lib/providers";
import { getAssetUrl } from "@/services/dbApi";
import type { UpdateTarget, WatchGroup } from "@/types/library";

export type TargetValues = { targetType: string; source: string; sourceKey: string; displayName: string };
/** `groupId` があれば、追加した対象をそのグループにも入れる。 */
export type TargetAction = { action: "add" | "toggle" | "delete"; target?: UpdateTarget; enabled?: boolean; values?: TargetValues; groupId?: number };
export type GroupAction =
  | { action: "create"; name: string; addTargetIds?: number[] }
  | { action: "rename"; group: WatchGroup; name: string }
  | { action: "delete"; group: WatchGroup }
  | { action: "members"; group: WatchGroup; targetIds: number[]; add: boolean };

/**
 * 左の束。既定の四つは監視対象そのものの性質で決まり、その下に利用者が
 * 作ったグループが並ぶ。どれを選んでも右の一覧の作法は同じ。
 */
export type WatchView = "all" | "author" | "series" | "paused" | `group:${number}`;

const PAGE_SIZE = 60;
/** 行に並べるグループ名の数。それより多ければ「+N」に畳む。 */
const ROW_GROUP_BADGES = 3;

export function TargetsPanel({ targets, groups, loading, error, retry, form, mutation, groupMutation, onSetEnabled, onCheck, checkDisabled }: {
  targets: UpdateTarget[];
  groups: WatchGroup[];
  loading: boolean;
  error: unknown;
  retry: () => void;
  form: UseFormReturnType<TargetValues, TargetValues, any>;
  mutation: { mutate: (input: TargetAction) => void; isPending: boolean };
  groupMutation: { mutateAsync: (input: GroupAction) => Promise<unknown>; isPending: boolean };
  /** まとめて止める・再開する。 */
  onSetEnabled: (targets: UpdateTarget[], enabled: boolean) => void;
  /** 一覧の対象だけを確認する。`label` は知らせに出す呼び名。 */
  onCheck: (targetIds: number[], label: string) => void;
  checkDisabled: boolean;
}) {
  const [view, setView] = useState<WatchView>("all");
  const [showPaused, setShowPaused] = useState(false);
  const [showAdd, setShowAdd] = useState(false);
  const [searchText, setSearchText] = useState("");
  const [activeVisibleCount, setActiveVisibleCount] = useState(PAGE_SIZE);
  const [pausedVisibleCount, setPausedVisibleCount] = useState(PAGE_SIZE);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  if (loading) return <LoadingState />;
  if (error) return <ErrorState error={error} retry={retry} />;

  const viewGroup = view.startsWith("group:") ? groups.find((group) => `group:${group.id}` === view) ?? null : null;
  // 消したグループを選んだままにしない。
  const currentView: WatchView = view.startsWith("group:") && !viewGroup ? "all" : view;
  const changeView = (next: WatchView) => {
    setView(next);
    setSelected(new Set());
    setActiveVisibleCount(PAGE_SIZE);
    setPausedVisibleCount(PAGE_SIZE);
    setShowPaused(next === "paused");
  };

  const inView = (target: UpdateTarget) => {
    if (currentView === "author" || currentView === "series") return target.targetType === currentView;
    if (currentView === "paused") return !target.enabled;
    if (viewGroup) return viewGroup.targetIds.includes(target.id);
    return true;
  };
  const scoped = targets.filter(inView);
  const search = searchText.trim().normalize("NFKC").toLocaleLowerCase();
  const matching = scoped.filter((target) => !search || `${target.displayName} ${target.sourceKey} ${target.source} ${(target.watchGroups ?? []).join(" ")}`.normalize("NFKC").toLocaleLowerCase().includes(search));
  const active = matching.filter((target) => target.enabled);
  const paused = matching.filter((target) => !target.enabled);
  const checkableIds = scoped.filter((target) => target.enabled).map((target) => target.id);
  const selectedTargets = targets.filter((target) => selected.has(target.id));
  const visibleIds = [...active.slice(0, activeVisibleCount), ...(showPaused || currentView === "paused" ? paused.slice(0, pausedVisibleCount) : [])].map((target) => target.id);
  const allVisibleSelected = visibleIds.length > 0 && visibleIds.every((id) => selected.has(id));

  const viewTitle = viewGroup?.name ?? ({ all: "すべての監視対象", author: "作者", series: "シリーズ", paused: "停止中" } as Record<string, string>)[currentView];
  const toggleSelected = (id: number, checked: boolean) => setSelected((current) => {
    const next = new Set(current);
    if (checked) next.add(id);
    else next.delete(id);
    return next;
  });

  const confirmDelete = (target: UpdateTarget) => modals.openConfirmModal({
    title: "監視対象を削除しますか？",
    children: <Text size="sm">「{target.displayName}」を更新監視から削除します。保存済みの作品は削除されません。</Text>,
    labels: { confirm: "削除する", cancel: "キャンセル" },
    confirmProps: { color: "red" },
    onConfirm: () => mutation.mutate({ action: "delete", target }),
  });
  const askGroupName = (title: string, current: string, submit: (name: string) => Promise<unknown>) => {
    const id = modals.open({
      title,
      children: <GroupNameForm current={current} onSubmit={async (name) => { await submit(name); modals.close(id); }} />,
    });
  };
  const createGroup = (addTargetIds?: number[]) => askGroupName(
    addTargetIds?.length ? `新しいグループに${formatNumber(addTargetIds.length)}件を入れる` : "監視グループを作る",
    "",
    async (name) => {
      const created = await groupMutation.mutateAsync({ action: "create", name, addTargetIds });
      if (addTargetIds?.length) setSelected(new Set());
      else if (typeof created === "number") changeView(`group:${created}`);
    },
  );
  const confirmDeleteGroup = (group: WatchGroup) => modals.openConfirmModal({
    title: "監視グループを消しますか？",
    children: <Text size="sm">「{group.name}」を消します。中の監視対象{formatNumber(group.targetIds.length)}件は、監視対象のまま残ります。自動確認の範囲にしていた場合は、すべての監視対象に戻ります。</Text>,
    labels: { confirm: "消す", cancel: "やめる" },
    confirmProps: { color: "red" },
    onConfirm: () => void groupMutation.mutateAsync({ action: "delete", group }).then(() => changeView("all")).catch(() => undefined),
  });
  const setMembers = (group: WatchGroup, add: boolean) => void groupMutation
    .mutateAsync({ action: "members", group, targetIds: [...selected], add })
    .then(() => setSelected(new Set()))
    .catch(() => undefined);

  const navItems: { view: WatchView; label: string; count: number }[] = [
    { view: "all", label: "すべて", count: targets.length },
    { view: "author", label: "作者", count: targets.filter((target) => target.targetType === "author").length },
    { view: "series", label: "シリーズ", count: targets.filter((target) => target.targetType === "series").length },
    { view: "paused", label: "停止中", count: targets.filter((target) => !target.enabled).length },
  ];

  const row = (target: UpdateTarget) => {
    // 開いているグループの名前は、どの行にも付くので言わない。
    const groupNames = (target.watchGroups ?? []).filter((name) => name !== viewGroup?.name);
    return (
      <Group px="md" py="sm" gap="sm" wrap="nowrap" key={target.id} className="watch-target" data-selected={selected.has(target.id) || undefined}>
        <Checkbox checked={selected.has(target.id)} onChange={(event) => toggleSelected(target.id, event.currentTarget.checked)} aria-label={`${target.displayName}を選択`} />
        <TargetFace target={target} />
        <Stack gap={3} flex={1} miw={0}>
          <Group gap={6} wrap="nowrap">
            <ProviderMark provider={target.source} compact />
            <Text size="sm" fw={650} className="identity-name line-clamp-1">{target.displayName}</Text>
            <TargetHealthBadge target={target} />
          </Group>
          <Text size="xs" c="dimmed" className="line-clamp-1">
            {target.targetType === "series" ? "シリーズ" : "作者"} · {target.sourceKey} · 最終確認 {formatDate(target.lastCheckedAt, true)} · 最後のヒット {target.lastHitAt ? formatDate(target.lastHitAt, true) : "なし"}
          </Text>
          {groupNames.length > 0 && (
            <Group gap={4} wrap="nowrap" className="watch-target__groups">
              {groupNames.slice(0, ROW_GROUP_BADGES).map((name) => <Badge key={name} size="xs" variant="outline" color="gray" style={{ flex: "none", textTransform: "none" }}>{name}</Badge>)}
              {groupNames.length > ROW_GROUP_BADGES && <Tooltip label={groupNames.slice(ROW_GROUP_BADGES).join("、")}><Badge size="xs" variant="outline" color="gray" style={{ flex: "none" }}>+{groupNames.length - ROW_GROUP_BADGES}</Badge></Tooltip>}
            </Group>
          )}
        </Stack>
        <Tooltip label={target.enabled ? "確認を止める（一覧では下に畳まれます）" : "確認を再開する"}>
          <Switch
            checked={target.enabled}
            disabled={mutation.isPending}
            onChange={(event) => mutation.mutate({ action: "toggle", target, enabled: event.currentTarget.checked })}
            aria-label={`${target.displayName}の監視`}
          />
        </Tooltip>
        <Tooltip label="削除">
          <ActionIcon variant="subtle" color="red" disabled={mutation.isPending} aria-label={`${target.displayName}を削除`} onClick={() => confirmDelete(target)}>
            <Icons.delete size={IconSize.menu} />
          </ActionIcon>
        </Tooltip>
      </Group>
    );
  };

  const moreButton = (remaining: number, onClick: () => void) => remaining > 0 && <><Divider /><Button variant="subtle" fullWidth onClick={onClick}>さらに表示（残り{formatNumber(remaining)}件）</Button></>;

  return (
    <Grid gap="lg" align="flex-start">
      <Grid.Col span={{ base: 12, md: 3 }}>
        <Card p="xs" className="watch-nav">
          <nav aria-label="監視グループ">
            <Stack gap={2}>
              {navItems.map((item) => <WatchNavItem key={item.view} label={item.label} count={item.count} active={currentView === item.view} onClick={() => changeView(item.view)} />)}
              <Group justify="space-between" wrap="nowrap" px="xs" pt="sm" pb={4}>
                <Text size="xs" fw={700} c="dimmed">監視グループ</Text>
                <Tooltip label="グループを作る">
                  <ActionIcon size="sm" variant="subtle" color="gray" aria-label="監視グループを作る" onClick={() => createGroup()}>
                    <Icons.add size={IconSize.menu} />
                  </ActionIcon>
                </Tooltip>
              </Group>
              {groups.length
                ? groups.map((group) => <WatchNavItem key={group.id} label={group.name} count={group.targetIds.length} active={currentView === `group:${group.id}`} onClick={() => changeView(`group:${group.id}`)} />)
                : <Text size="xs" c="dimmed" px="xs" pb="xs">よく見る作者などをグループにまとめておくと、そのグループだけを確認できます。</Text>}
            </Stack>
          </nav>
        </Card>
      </Grid.Col>

      <Grid.Col span={{ base: 12, md: 9 }}>
        <Stack gap="md">
          <Card p="md">
            <Stack gap="sm">
              <Group justify="space-between" align="flex-start" wrap="wrap" gap="sm">
                <Box miw={0}>
                  <Text fw={700} className="line-clamp-1">{viewTitle}</Text>
                  <Text size="xs" c="dimmed">
                    {search ? `${formatNumber(matching.length)} / ${formatNumber(scoped.length)}件が一致` : `監視中 ${formatNumber(scoped.filter((target) => target.enabled).length)}件 · 停止中 ${formatNumber(scoped.filter((target) => !target.enabled).length)}件`}
                  </Text>
                </Box>
                <Group gap="xs" wrap="nowrap">
                  {currentView !== "paused" && (
                    <Button
                      size="sm"
                      variant="light"
                      leftSection={<Icons.updates size={IconSize.menu} />}
                      disabled={checkDisabled || !checkableIds.length}
                      onClick={() => onCheck(checkableIds, viewGroup ? `「${viewGroup.name}」` : viewTitle)}
                    >
                      {formatNumber(checkableIds.length)}件を確認
                    </Button>
                  )}
                  <Button size="sm" variant="default" leftSection={<Icons.add size={IconSize.menu} />} aria-expanded={showAdd} onClick={() => setShowAdd((open) => !open)}>IDで追加</Button>
                  {viewGroup && (
                    <Menu position="bottom-end" withinPortal>
                      <Menu.Target>
                        <ActionIcon size={36} variant="default" aria-label={`「${viewGroup.name}」の操作`}><Icons.more size={IconSize.action} /></ActionIcon>
                      </Menu.Target>
                      <Menu.Dropdown>
                        <Menu.Item leftSection={<Icons.edit size={IconSize.menu} />} onClick={() => askGroupName("グループの名前を変える", viewGroup.name, (name) => groupMutation.mutateAsync({ action: "rename", group: viewGroup, name }))}>名前を変える</Menu.Item>
                        <Menu.Item color="red" leftSection={<Icons.delete size={IconSize.menu} />} onClick={() => confirmDeleteGroup(viewGroup)}>グループを消す</Menu.Item>
                      </Menu.Dropdown>
                    </Menu>
                  )}
                </Group>
              </Group>

              <Collapse expanded={showAdd}>
                <Paper withBorder p="sm" radius="md">
                  <form onSubmit={form.onSubmit((values) => mutation.mutate({ action: "add", values, groupId: viewGroup?.id }))}>
                    <Stack gap="xs">
                      <Text size="xs" c="dimmed">ここに入るのは、自分で追加した作者とシリーズだけです。保存や更新確認で勝手に増えることはありません。{viewGroup && `追加した対象は「${viewGroup.name}」にも入ります。`}</Text>
                      <Grid gap="xs" align="flex-end">
                        <Grid.Col span={{ base: 6, sm: 2 }}><Select size="sm" label="種類" data={[{ value: "author", label: "作者" }, { value: "series", label: "シリーズ" }]} allowDeselect={false} {...form.getInputProps("targetType")} /></Grid.Col>
                        <Grid.Col span={{ base: 6, sm: 2 }}><Select size="sm" label="ソース" data={[{ value: "pixiv", label: "pixiv" }, { value: "fanbox", label: "FANBOX" }]} allowDeselect={false} {...form.getInputProps("source")} /></Grid.Col>
                        <Grid.Col span={{ base: 12, sm: 3 }}><TextInput size="sm" label="ID" {...form.getInputProps("sourceKey")} /></Grid.Col>
                        <Grid.Col span={{ base: 12, sm: 3 }}><TextInput size="sm" label="表示名" {...form.getInputProps("displayName")} /></Grid.Col>
                        <Grid.Col span={{ base: 12, sm: 2 }}><Button size="sm" type="submit" fullWidth loading={mutation.isPending}>追加</Button></Grid.Col>
                      </Grid>
                    </Stack>
                  </form>
                </Paper>
              </Collapse>

              <TextInput
                aria-label="監視対象を探す"
                placeholder="表示名・ID・ソース・グループ名で探す"
                leftSection={<Icons.search size={IconSize.menu} />}
                value={searchText}
                onChange={(event) => {
                  setSearchText(event.currentTarget.value);
                  setActiveVisibleCount(PAGE_SIZE);
                  setPausedVisibleCount(PAGE_SIZE);
                  if (event.currentTarget.value.trim()) setShowPaused(true);
                }}
              />
            </Stack>
          </Card>

          {/* 選んだときだけ出る。いつも並べておくと、押せない操作が一覧の上を塞ぐ。 */}
          {selected.size > 0 && (
            <Paper withBorder p="xs" radius="md" className="watch-bulk" role="toolbar" aria-label="選んだ監視対象の操作">
              <Group gap="xs" wrap="wrap">
                <Text size="sm" fw={650} px={6}>{formatNumber(selected.size)}件を選択中</Text>
                <Menu position="bottom-start" withinPortal>
                  <Menu.Target>
                    <Button size="xs" variant="light" rightSection={<Icons.expand size={IconSize.inline} />} loading={groupMutation.isPending}>グループに入れる</Button>
                  </Menu.Target>
                  <Menu.Dropdown>
                    {groups.filter((group) => group.id !== viewGroup?.id).map((group) => <Menu.Item key={group.id} onClick={() => setMembers(group, true)}>{group.name}</Menu.Item>)}
                    {groups.some((group) => group.id !== viewGroup?.id) && <Menu.Divider />}
                    <Menu.Item leftSection={<Icons.add size={IconSize.menu} />} onClick={() => createGroup([...selected])}>新しいグループを作って入れる…</Menu.Item>
                  </Menu.Dropdown>
                </Menu>
                {viewGroup && <Button size="xs" variant="default" onClick={() => setMembers(viewGroup, false)}>このグループから外す</Button>}
                {selectedTargets.some((target) => target.enabled) && <Button size="xs" variant="default" leftSection={<Icons.pause size={IconSize.inline} />} onClick={() => { onSetEnabled(selectedTargets.filter((target) => target.enabled), false); setSelected(new Set()); }}>止める</Button>}
                {selectedTargets.some((target) => !target.enabled) && <Button size="xs" variant="default" leftSection={<Icons.resume size={IconSize.inline} />} onClick={() => { onSetEnabled(selectedTargets.filter((target) => !target.enabled), true); setSelected(new Set()); }}>再開する</Button>}
                <Box style={{ flex: 1 }} />
                <Button size="xs" variant="subtle" color="gray" onClick={() => setSelected(new Set())}>選択を解除</Button>
              </Group>
            </Paper>
          )}

          {!scoped.length ? (
            viewGroup
              ? <EmptyState icon={Icons.people} title="このグループは空です" description="「すべて」などで監視対象を選び、「グループに入れる」から加えてください。" />
              : <EmptyState icon={Icons.people} title={currentView === "paused" ? "止めている対象はありません" : "監視対象はまだありません"} description={currentView === "paused" ? "確認を止めた対象は、ここに集まります。" : "作者やシリーズの画面で「新着を監視」を入れるか、「IDで追加」から加えてください。"} />
          ) : (
            <>
              {currentView !== "paused" && (
                <Card p={0}>
                  <Group px="md" py={8} gap="sm" wrap="nowrap" className="watch-list__head">
                    <Checkbox
                      size="xs"
                      checked={allVisibleSelected}
                      indeterminate={!allVisibleSelected && visibleIds.some((id) => selected.has(id))}
                      disabled={!visibleIds.length}
                      onChange={() => setSelected((current) => {
                        const next = new Set(current);
                        visibleIds.forEach((id) => (allVisibleSelected ? next.delete(id) : next.add(id)));
                        return next;
                      })}
                      aria-label="表示している対象をすべて選択"
                    />
                    <Text size="xs" c="dimmed">監視中</Text>
                  </Group>
                  <Divider />
                  {active.length
                    ? active.slice(0, activeVisibleCount).map((target, index) => <Box key={target.id}>{index > 0 && <Divider />}{row(target)}</Box>)
                    : <Text p="lg" c="dimmed" size="sm">{search ? "一致する監視中の対象はありません。" : "確認中の対象はありません。"}</Text>}
                  {moreButton(active.length - activeVisibleCount, () => setActiveVisibleCount((count) => count + PAGE_SIZE))}
                </Card>
              )}

              {/* 止めた対象はここへ畳む。トグルひとつで一覧から下がり、消したいときだけ
                  削除すればいい。切り替えた瞬間に消えるより戻しやすい。 */}
              {paused.length > 0 && (
                <Card p={0}>
                  {currentView === "paused" ? (
                    <Group px="md" py={8} gap="sm" wrap="nowrap" className="watch-list__head">
                      <Checkbox
                        size="xs"
                        checked={allVisibleSelected}
                        indeterminate={!allVisibleSelected && visibleIds.some((id) => selected.has(id))}
                        onChange={() => setSelected((current) => {
                          const next = new Set(current);
                          visibleIds.forEach((id) => (allVisibleSelected ? next.delete(id) : next.add(id)));
                          return next;
                        })}
                        aria-label="表示している対象をすべて選択"
                      />
                      <Text size="xs" c="dimmed">確認しない対象です。再開も削除もここからできます</Text>
                    </Group>
                  ) : (
                    <UnstyledButton className="update-paused__head" onClick={() => setShowPaused((open) => !open)} aria-expanded={showPaused}>
                      <Group p="md" justify="space-between" wrap="nowrap">
                        <Group gap="xs" wrap="nowrap">
                          <Icons.next size={IconSize.menu} style={{ transform: showPaused ? "rotate(90deg)" : undefined }} aria-hidden />
                          <Text size="sm" fw={650}>停止中</Text>
                          <Badge size="sm" variant="light" color="gray">{paused.length}</Badge>
                        </Group>
                        <Text size="xs" c="dimmed">確認しない対象です。再開も削除もここからできます</Text>
                      </Group>
                    </UnstyledButton>
                  )}
                  {(showPaused || currentView === "paused") && <>
                    {paused.slice(0, pausedVisibleCount).map((target) => <Box key={target.id}><Divider />{row(target)}</Box>)}
                    {moreButton(paused.length - pausedVisibleCount, () => setPausedVisibleCount((count) => count + PAGE_SIZE))}
                  </>}
                </Card>
              )}
              {currentView === "paused" && !paused.length && <Text c="dimmed" size="sm">一致する停止中の対象はありません。</Text>}
            </>
          )}
        </Stack>
      </Grid.Col>
    </Grid>
  );
}

function WatchNavItem({ label, count, active, onClick }: { label: string; count: number; active: boolean; onClick: () => void }) {
  return (
    <UnstyledButton className="watch-nav__item" data-active={active || undefined} aria-current={active ? "true" : undefined} onClick={onClick}>
      <Text size="sm" fw={active ? 700 : 500} className="line-clamp-1" style={{ flex: 1, minWidth: 0 }}>{label}</Text>
      <Text size="xs" c="dimmed" style={{ flex: "none" }}>{formatNumber(count)}</Text>
    </UnstyledButton>
  );
}

/**
 * 一覧の顔。作者はプロフィール画像を丸く、シリーズは表紙を角丸で。
 *
 * 作者の丸に表紙を入れない（作者一覧のカードと同じ決まり）。画像が無ければ
 * 記号にする - 監視対象は保存していない作者でもよいので、「置いていない」と
 * 言い切れる材料はここには無い。
 */
function TargetFace({ target }: { target: UpdateTarget }) {
  const src = getAssetUrl(target.iconPath);
  const series = target.targetType === "series";
  return (
    <Avatar src={src} size={40} radius={series ? "sm" : "xl"} color={series ? "gray" : "piep"} className="watch-target__face" imageProps={{ loading: "lazy", decoding: "async" }} aria-hidden>
      {series ? <Icons.series size={IconSize.action} /> : <Icons.person size={IconSize.action} />}
    </Avatar>
  );
}

function GroupNameForm({ current, onSubmit }: { current: string; onSubmit: (name: string) => Promise<void> }) {
  const [pending, setPending] = useState(false);
  const form = useForm({
    initialValues: { name: current },
    validate: { name: (value) => (!value.trim() ? "グループ名を入力してください" : value.trim().length > 40 ? "グループ名は40文字までにしてください" : null) },
  });
  return (
    <form onSubmit={form.onSubmit(async ({ name }) => {
      setPending(true);
      try {
        await onSubmit(name.trim());
      } catch (error) {
        // 同じ名前があるときなどは、閉じずにその場で直せるようにする。
        form.setFieldError("name", errorMessage(error));
      } finally {
        setPending(false);
      }
    })}>
      <Stack>
        <TextInput label="グループ名" placeholder="毎日見る作者" data-autofocus {...form.getInputProps("name")} />
        <Button type="submit" loading={pending}>{current ? "変更する" : "作る"}</Button>
      </Stack>
    </form>
  );
}

/** 対象が何日ヒットしなければ「休眠」と見なすか。 */
const DORMANT_DAYS = 180;
/** 何回続けて失敗したら知らせるか。 */
const FAILING_STREAK = 3;

/**
 * 対象の様子を一目で。
 *
 * 監視対象は増える一方で、放っておくと「もう更新の来ない作者」と「ずっと
 * 失敗している対象」が同じ顔で並ぶ。どちらも確認のたびに時間を使うので、
 * 畳むか外すかを決められるようにする。
 */
export function TargetHealthBadge({ target }: { target: UpdateTarget }) {
  const errors = target.consecutiveErrors ?? 0;
  if (errors >= FAILING_STREAK) {
    return <Tooltip label={`${errors}回続けて確認に失敗しています`}><Badge size="xs" color="red" variant="light" style={{ flex: "none" }}>確認できていません</Badge></Tooltip>;
  }
  if (!target.enabled) return <Badge size="xs" color="gray" variant="light" style={{ flex: "none" }}>停止中</Badge>;
  const reference = target.lastHitAt ?? target.createdAt;
  const days = (Date.now() - new Date(reference).getTime()) / 86_400_000;
  if (Number.isFinite(days) && days >= DORMANT_DAYS) {
    return <Tooltip label={`${Math.round(days)}日、新しいものが見つかっていません`}><Badge size="xs" color="gray" variant="light" style={{ flex: "none" }}>休眠</Badge></Tooltip>;
  }
  return null;
}
