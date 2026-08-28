import { useCallback, useEffect, useRef, useState } from "react";
import { Alert, Badge, Box, Button, Card, Divider, Group, Progress, Stack, Switch, Text } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { APP_VERSION } from "@/lib/version";
import { errorMessage } from "@/lib/format";
import { ExpandableText } from "@/components/ExpandableText";
import {
  checkForAppUpdate,
  downloadAndInstallAppUpdate,
  restartForAppUpdate,
  type Update,
} from "@/services/appUpdateApi";
import {
  APP_UPDATE_CHECK_KEY,
  describeCheckFailure,
  downloadPercent,
  noReleasePublishedYet,
  launchCheckEnabled,
  storeLaunchCheck,
  summarizeNotes,
} from "@/features/settings/appUpdate";

type Phase = "idle" | "checking" | "current" | "available" | "downloading" | "ready" | "error";

/**
 * localStorage は無い・読めないことがある（プライベートウィンドウ、容量超過）。
 * 設定ひとつのために画面ごと落とすほどの値ではないので、既定に倒す。
 */
function readLaunchCheck(): string | null {
  try { return window.localStorage.getItem(APP_UPDATE_CHECK_KEY); } catch { return null; }
}

function writeLaunchCheck(value: string): void {
  try { window.localStorage.setItem(APP_UPDATE_CHECK_KEY, value); } catch { /* 覚えられないだけで、確認そのものは動く。 */ }
}

/**
 * アプリ自身の版を確認して入れ替える一枚。
 *
 * 入れ替えは押したときだけ走る。読書の途中で実行ファイルを差し替えられて
 * 再起動されるのは、便利さより先に不快さが来る。
 */
export function AppUpdateCard({ runtime }: { runtime: boolean }) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [update, setUpdate] = useState<Update | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // まだ配っていないだけの状態を、壊れている状態と同じ赤さで出さない。
  const [benign, setBenign] = useState(false);
  const [progress, setProgress] = useState<{ downloaded: number; total: number | null } | null>(null);
  const [launchCheck, setLaunchCheck] = useState(() => launchCheckEnabled(readLaunchCheck()));
  // 画面を離れたあとに状態を書き戻さない。確認は数秒かかることがある。
  // 付け直しで true に戻すのが要点 - StrictMode は一度剥がしてから付け直すので、
  // 立て直さないと二度目のマウントが「もう居ない」ままになり、確認結果が
  // どこにも届かず回り続ける。
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const runCheck = useCallback(async () => {
    setPhase("checking");
    setMessage(null);
    setBenign(false);
    try {
      const found = await checkForAppUpdate();
      if (!mounted.current) return;
      setUpdate(found);
      setPhase(found ? "available" : "current");
    } catch (error) {
      if (!mounted.current) return;
      const raw = errorMessage(error);
      setMessage(describeCheckFailure(raw));
      setBenign(noReleasePublishedYet(raw));
      setPhase("error");
    }
  }, []);

  // 開いた人は確認しに来ている。押させる手間をひとつ省く。
  useEffect(() => { if (runtime) void runCheck(); }, [runtime, runCheck]);

  const install = async () => {
    if (!update) return;
    setPhase("downloading");
    setProgress({ downloaded: 0, total: null });
    try {
      await downloadAndInstallAppUpdate(update, (next) => { if (mounted.current) setProgress(next); });
      if (!mounted.current) return;
      setPhase("ready");
    } catch (error) {
      if (!mounted.current) return;
      setMessage(`更新の適用に失敗しました（${errorMessage(error)}）`);
      setBenign(false);
      setPhase("error");
    }
  };

  const restart = async () => {
    try {
      await restartForAppUpdate();
    } catch (error) {
      if (!mounted.current) return;
      setMessage(`更新後の再起動に失敗しました（${errorMessage(error)}）`);
      setBenign(false);
      setPhase("error");
    }
  };

  const notes = summarizeNotes(update?.body);
  const percent = progress ? downloadPercent(progress.downloaded, progress.total) : null;

  return (
    <Card p="lg">
      <Stack gap="md">
        <Group justify="space-between" wrap="nowrap" align="flex-start">
          <Box miw={0}>
            <Group gap="xs">
              <Text fw={700}>アプリの更新</Text>
              {phase === "available" || phase === "ready"
                ? <Badge color="piep" variant="light">新しい版があります</Badge>
                : phase === "current" ? <Badge color="gray" variant="light">最新です</Badge> : null}
            </Group>
            <Text size="sm" c="dimmed" mt={4}>
              いま使っているのは v{APP_VERSION} です。作品の更新確認とは別に、piep 本体の版を確認します。
            </Text>
          </Box>
          <Button
            variant="default"
            size="xs"
            disabled={!runtime}
            loading={phase === "checking"}
            leftSection={<Icons.retry size={IconSize.menu} />}
            onClick={() => void runCheck()}
          >
            更新を確認
          </Button>
        </Group>

        {!runtime && <Text size="xs" c="dimmed">ブラウザプレビューには入れ替える実行ファイルがないため、確認できません。</Text>}

        {phase === "error" && message && (benign
          ? <Text size="sm" c="dimmed">{message}</Text>
          : <Alert color="red" title="確認できませんでした" icon={<Icons.warning size={IconSize.nav} />}>{message}</Alert>)}

        {(phase === "available" || phase === "downloading" || phase === "ready") && update && (
          <Card p="md" withBorder>
            <Stack gap="sm">
              <Group gap="xs">
                <Icons.appUpdate size={IconSize.action} />
                <Text fw={700}>v{update.version}</Text>
                {update.date && <Text size="xs" c="dimmed">{update.date}</Text>}
              </Group>
              {notes && <ExpandableText lines={4} label="更新の内容">{notes}</ExpandableText>}
              {phase === "downloading" && (
                <Box>
                  <Progress value={percent ?? 100} animated={percent === null} />
                  <Text size="xs" c="dimmed" mt={4}>
                    {percent === null ? "ダウンロードしています…" : `ダウンロードしています… ${percent}%`}
                  </Text>
                </Box>
              )}
              {phase === "ready"
                ? (
                  <Group>
                    <Text size="sm">入れ替えの準備ができました。再起動すると新しい版で開きます。</Text>
                    <Button size="xs" leftSection={<Icons.retry size={IconSize.menu} />} onClick={() => void restart()}>いま再起動する</Button>
                  </Group>
                )
                : (
                  <Group>
                    <Button
                      size="xs"
                      disabled={phase === "downloading"}
                      loading={phase === "downloading"}
                      leftSection={<Icons.appUpdate size={IconSize.menu} />}
                      onClick={() => void install()}
                    >
                      ダウンロードして更新
                    </Button>
                    <Text size="xs" c="dimmed">終わるまで piep はそのまま使えます。再起動は別に確認します。</Text>
                  </Group>
                )}
            </Stack>
          </Card>
        )}

        <Divider />
        <Switch
          checked={launchCheck}
          onChange={(event) => {
            const next = event.currentTarget.checked;
            setLaunchCheck(next);
            writeLaunchCheck(storeLaunchCheck(next));
          }}
          label="起動時に新しい版を確認する"
          description="見つかったときに知らせるだけで、入れ替えは押したときだけ行います。"
        />
      </Stack>
    </Card>
  );
}
