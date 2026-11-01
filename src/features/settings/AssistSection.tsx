import { useEffect, useState } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Code,
  Collapse,
  Group,
  List,
  Loader,
  NumberInput,
  Select,
  Stack,
  Switch,
  Text,
  TextInput,
  Textarea,
  ThemeIcon,
  UnstyledButton,
} from "@mantine/core";
import { useDisclosure } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage, formatBytes, formatDuration, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import {
  DEFAULT_ASSIST_SETTINGS,
  ASSIST_FEATURES,
  assistTarget,
  discoverAssistEngines,
  getAssistRuntimeProfile,
  isLocalAssistUrl,
  loadAssistSettings,
  normalizeAssistBaseUrl,
  saveAssistSettings,
  toEngine,
  tryAssistEngine,
  validateAssistSettings,
  type AssistSettings,
  type AssistFeatureId,
  type DiscoveredEngine,
  type TrialResult,
} from "@/services/assistApi";
import { getSearchIndexStatus, isTauriRuntime } from "@/services/dbApi";
import { setSemanticSearchEnabled, startSearchRebuildIndex } from "@/services/searchApi";

/**
 * 機能ごとに、実際に何を送るか。設定画面の「送る項目」はこの表から出す。
 *
 * 以前は機能の名前から大まかに決めており、あらすじ・前回のあらすじ・校正
 * にも、効かない「題名」「タグ」のスイッチが出ていた。
 */
const FEATURE_SENDS: Record<AssistFeatureId, { summary: string; title: boolean; author: boolean; tags: boolean; excerpt: boolean }> = {
  search_interpretation: { summary: "入力した検索文と、棚のタグの候補", title: false, author: false, tags: false, excerpt: false },
  work_synopsis: { summary: "本文（許可したときだけ）と、下で選んだ項目", title: true, author: true, tags: true, excerpt: false },
  work_tagging: { summary: "下で選んだ項目と、棚のタグの候補", title: true, author: true, tags: true, excerpt: true },
  author_style: { summary: "手元の作品の題名とタグ（通信はしません）", title: true, author: false, tags: true, excerpt: false },
  reader_recap: { summary: "前の話の本文（許可したときだけ）と、その題名", title: true, author: false, tags: false, excerpt: false },
  collection_split: { summary: "下で選んだ項目", title: true, author: true, tags: true, excerpt: false },
  collection_naming: { summary: "下で選んだ項目", title: true, author: true, tags: true, excerpt: false },
  editor_proofread: { summary: "編集中の本文だけ（許可したときだけ。約3,000字ずつ）", title: false, author: false, tags: false, excerpt: false },
};

/**
 * 手元の言語モデルに手伝ってもらうための設定。
 *
 * piep はモデルを同梱しない。**すでに動いているものを、こちらから探しに行く。**
 * 「OpenAI 互換のエンドポイント」を打たせない — 番号を覚えているのは piep の
 * 仕事であって、利用者の仕事ではない。
 *
 * 順番は「探す → 試す → 使う」。つながることと使えることは別なので、
 * 保存する前に**実際に何が返るのか**を見せる。
 */
export function AssistSection() {
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<AssistSettings>(DEFAULT_ASSIST_SETTINGS);
  const [semanticEnabled, setSemanticEnabled] = useState(false);
  const [found, setFound] = useState<DiscoveredEngine[] | null>(null);
  const [trial, setTrial] = useState<TrialResult | null>(null);
  const [manualOpened, manual] = useDisclosure(false);
  const [helpOpened, help] = useDisclosure(false);

  const settingsQuery = useQuery({ queryKey: ["naming-settings"], queryFn: loadAssistSettings });
  const semanticStatusQuery = useQuery({
    queryKey: ["search-index-status"],
    queryFn: getSearchIndexStatus,
    enabled: runtime,
  });
  useEffect(() => {
    if (settingsQuery.data) setDraft(settingsQuery.data);
  }, [settingsQuery.data]);
  useEffect(() => {
    if (semanticStatusQuery.data) {
      setSemanticEnabled(semanticStatusQuery.data.semanticEnabled);
    }
  }, [semanticStatusQuery.data]);

  const profileQuery = useQuery({
    queryKey: ["assist-runtime-profile", normalizeAssistBaseUrl(draft.baseUrl), draft.model.trim()],
    queryFn: () => getAssistRuntimeProfile(toEngine(draft)),
    enabled: runtime && isLocalAssistUrl(draft.baseUrl) && Boolean(assistTarget(draft)),
    retry: false,
    staleTime: 15_000,
  });

  const save = useMutation({
    mutationFn: async (next: AssistSettings) => {
      await saveAssistSettings(next);
      if (runtime) {
        // 意味検索の入り切りが**変わったときだけ**触る。
        //
        // 以前は保存のたびに再構築を頼んでいた。追加指示の一文を直して保存した
        // だけで索引の作り直しが始まり、棚の書き込み権を取り、アクティビティに
        // 身に覚えのない行が並んでいた。
        const wasEnabled = semanticStatusQuery.data?.semanticEnabled;
        if (semanticEnabled !== wasEnabled) {
          try {
            if (semanticEnabled) await startSearchRebuildIndex({ includeSemantic: true });
            else await setSemanticSearchEnabled(false);
          } catch (error) {
            return { semanticError: errorMessage(error) };
          }
        }
      }
      return { semanticError: null };
    },
    onSuccess: (result, next) => {
      queryClient.invalidateQueries({ queryKey: ["naming-settings"] });
      queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
      if (result.semanticError) {
        notifications.show({
          color: "orange",
          title: "AI設定は保存しました",
          message: `意味検索の切り替えは完了していません: ${result.semanticError}`,
        });
        return;
      }
      notifications.show({
        color: "green",
        message: next.enabled ? "モデルを使う設定にしました" : "モデルを使わない設定にしました",
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "保存できません", message: errorMessage(error) }),
  });

  const discover = useMutation({
    mutationFn: discoverAssistEngines,
    onSuccess: (engines) => {
      setFound(engines);
      setTrial(null);
      if (engines.length === 1 && engines[0].models.length > 0) {
        // 1つしか無いなら選ばせる意味が無い。そのまま入れる。
        setDraft((current) => ({
          ...current,
          baseUrl: engines[0].baseUrl,
          model: engines[0].models[0],
          remoteConsentUrl: null,
          verifiedTarget: null,
        }));
      }
    },
    onError: (error) => notifications.show({ color: "red", title: "探せません", message: errorMessage(error) }),
  });

  const runTrial = useMutation({
    mutationFn: async () => {
      const target = assistTarget(draft);
      if (!target) throw new Error("つなぎ先とモデルを指定してください");
      // This button verifies the shared model. `toEngine` normally applies a
      // feature override (collection naming by default), which could make a
      // different model pass and incorrectly mark the shared target verified.
      const engine = toEngine(draft);
      return {
        target,
        result: await tryAssistEngine({
          ...engine,
          model: draft.model.trim(),
          featureProfile: undefined,
        }),
      };
    },
    onSuccess: ({ target, result }) => {
      setTrial(result);
      setDraft((current) => assistTarget(current) === target ? { ...current, verifiedTarget: target } : current);
    },
    onError: (error) => {
      setTrial(null);
      notifications.show({ color: "red", title: "試せません", message: errorMessage(error) });
    },
  });

  const verifyFeatureModel = useMutation({
    mutationFn: async (featureId: AssistFeatureId) => {
      const engine = toEngine(draft, featureId);
      const target = assistTarget({ baseUrl: engine.baseUrl, model: engine.model });
      if (!target) throw new Error("つなぎ先とモデルを指定してください");
      // The connection trial checks structured output. The actual feature
      // profile is intentionally omitted because the trial material is the
      // small built-in naming sample, not library data.
      const result = await tryAssistEngine({ ...engine, featureProfile: undefined });
      return { featureId, target, result };
    },
    onSuccess: ({ featureId, target, result }) => {
      setDraft((current) => ({
        ...current,
        featureProfiles: {
          ...current.featureProfiles,
          [featureId]: { ...current.featureProfiles[featureId], verifiedTarget: target },
        },
      }));
      notifications.show({
        color: "green",
        message: `命名の試し書きが通りました（${formatDuration(Math.max(1, result.elapsedMs / 1000))}）。${ASSIST_FEATURES.find((feature) => feature.id === featureId)?.label ?? featureId}での答えは、まだ確かめていません。`,
      });
    },
    onError: (error) => notifications.show({ color: "red", title: "このモデルを試せません", message: errorMessage(error) }),
  });

  const normalizedUrl = normalizeAssistBaseUrl(draft.baseUrl);
  const local = isLocalAssistUrl(draft.baseUrl);
  const externalSecure = local || Boolean(normalizedUrl?.startsWith("https://"));
  const remoteAllowed = local || (normalizedUrl !== null && normalizeAssistBaseUrl(draft.remoteConsentUrl ?? "") === normalizedUrl);
  const ready = Boolean(normalizedUrl && draft.model.trim() && externalSecure && remoteAllowed);
  const verified = draft.verifiedTarget === assistTarget(draft);
  const saveProblem = validateAssistSettings(draft);
  const selected = found?.find((engine) => engine.baseUrl === draft.baseUrl);
  const models = selected?.models ?? (draft.model ? [draft.model] : []);
  const updateFeature = (featureId: AssistFeatureId, change: Partial<AssistSettings["featureProfiles"][AssistFeatureId]>) => {
    setDraft((current) => ({
      ...current,
      featureProfiles: {
        ...current.featureProfiles,
        [featureId]: { ...current.featureProfiles[featureId], ...change },
      },
    }));
  };

  // 保存していない変更があるか。機能ごとの設定は保存ボタンから離れた下の方に
  // あるので、直したまま画面を離れても気づけるよう、保存の帯で知らせる。
  const dirty = Boolean(settingsQuery.data)
    && (JSON.stringify(settingsQuery.data) !== JSON.stringify(draft)
      || (semanticStatusQuery.data ? semanticEnabled !== semanticStatusQuery.data.semanticEnabled : false));
  const enableBlocked = !draft.enabled && (!ready || !verified);

  return (
    <Stack gap="lg">
      <div>
        <Group gap="xs">
          <Text fw={700} size="lg">AIの手伝い</Text>
          <Badge size="sm" variant="light" color="yellow" tt="none">β</Badge>
        </Group>
        <Text size="sm" c="dimmed">
          <b>保存・検索・読書は、AIを設定しなくても使えます。</b>
          言葉での検索、タグの補完、あらすじ、前回のあらすじ、コレクションの命名と分割、本文の校正に、
          手元で動かしているモデルを使いたいときだけ設定してください。
          <b>AIの案やあらすじは、ボタンを押したときにだけ作ります。</b>
        </Text>
      </div>

      <Alert color="yellow" title="AIの手伝いと意味検索はβ版です">
        <Text size="sm">
          結果はモデルや作品によって変わります。あらすじには要点や結末の抜けが、タグの案や校正の候補には誤りがあり得ます。
          採る前に、原文や根拠を確かめてください。
        </Text>
      </Alert>

      {/* ---- 1. 選ぶ ---------------------------------------------------- */}
      <Card withBorder p="lg">
        <Stack gap="md">
          <Group justify="space-between" align="flex-start" wrap="nowrap">
            <div>
              <Text fw={650}>1. この端末で動いているモデルを探す</Text>
              <Text size="sm" c="dimmed">
                LM Studio・Ollama・llama.cpp・Strata などが動いていれば見つかります。
              </Text>
            </div>
            <Button
              flex="none"
              leftSection={<Icons.search size={IconSize.action} />}
              loading={discover.isPending}
              disabled={!runtime}
              onClick={() => discover.mutate()}
            >
              探す
            </Button>
          </Group>

          {found !== null && found.length === 0 && (
            <Alert color="yellow" icon={<Icons.notice size={IconSize.action} />} title="見つかりませんでした">
              <Stack gap="xs">
                <Text size="sm">
                  推論サーバーが動いていないようです。piep はモデルを同梱していないので、
                  <b>別のアプリで動かしているモデルを使います。</b>
                </Text>
                <UnstyledButton onClick={help.toggle}>
                  <Group gap={4}>
                    <Icons.expand size={IconSize.inline} style={{ transform: helpOpened ? "rotate(180deg)" : undefined }} />
                    <Text size="sm" fw={650}>用意のしかた</Text>
                  </Group>
                </UnstyledButton>
                <Collapse expanded={helpOpened}>
                  <List size="sm" spacing={6} pt={6}>
                    <List.Item>
                      <b>LM Studio</b> — モデルを選んで「Local Server」を開始します。
                      手順がいちばん少ないので、初めてならこれがおすすめです。
                    </List.Item>
                    <List.Item>
                      <b>Ollama</b> — 入れると常駐します。<Code>ollama pull &lt;モデル名&gt;</Code> で
                      モデルを取ってくれば見つかります。
                    </List.Item>
                    <List.Item>
                      起動したら、この画面で<b>もう一度「探す」</b>を押してください。
                    </List.Item>
                  </List>
                </Collapse>
              </Stack>
            </Alert>
          )}

          {found && found.length > 0 && (
            <Stack gap={6}>
              <Text size="sm" fw={650}>{formatNumber(found.length)}件見つかりました</Text>
              {found.map((engine) => (
                <UnstyledButton
                  key={engine.baseUrl}
                  className="collection-pick"
                  data-selected={draft.baseUrl === engine.baseUrl || undefined}
                  aria-pressed={draft.baseUrl === engine.baseUrl}
                  onClick={() => {
                    setDraft({
                      ...draft,
                      baseUrl: engine.baseUrl,
                      model: engine.models[0] ?? "",
                      remoteConsentUrl: null,
                      verifiedTarget: null,
                    });
                    setTrial(null);
                  }}
                >
                  <ThemeIcon variant="light" color={draft.baseUrl === engine.baseUrl ? "piep" : "gray"} size="sm">
                    <Icons.desktopApp size={IconSize.inline} />
                  </ThemeIcon>
                  <span className="collection-pick__body">
                    {/* サーバーが名乗ったときだけ言い切る。名乗らなければ、
                        番号から推した名前だとそのまま書く。 */}
                    <Text size="sm" fw={650}>
                      {engine.label}
                      {!engine.identified && <Text span size="xs" c="dimmed" fw={400}>（ポート番号からの推測）</Text>}
                    </Text>
                    <Text size="xs" c="dimmed" className="line-clamp-1">
                      {engine.models.slice(0, 2).join("、")}
                      {engine.models.length > 2 && ` ほか${formatNumber(engine.models.length - 2)}件`}
                      {" · "}{engine.baseUrl}
                    </Text>
                  </span>
                  {draft.baseUrl === engine.baseUrl && <Icons.confirm size={IconSize.menu} />}
                </UnstyledButton>
              ))}
            </Stack>
          )}

          {manualOpened ? (
            <TextInput
              label="使うモデルのID"
              description="一覧（/models）を公開していないサーバーでは、APIで使うモデルIDを直接入れます。"
              placeholder="model-id"
              value={draft.model}
              onChange={(event) => {
                setDraft({ ...draft, model: event.currentTarget.value, verifiedTarget: null });
                setTrial(null);
              }}
              disabled={!runtime}
            />
          ) : (
            <Select
              label="使うモデル"
              placeholder={models.length > 0 ? "モデルを選ぶ" : "先に「探す」を押してください"}
              data={models}
              value={draft.model || null}
              searchable
              allowDeselect={false}
              onChange={(value) => {
                setDraft({ ...draft, model: value ?? "", verifiedTarget: null });
                setTrial(null);
              }}
              disabled={!runtime || models.length === 0}
            />
          )}

          <UnstyledButton onClick={manual.toggle}>
            <Group gap={4}>
              <Icons.expand size={IconSize.inline} style={{ transform: manualOpened ? "rotate(180deg)" : undefined }} />
              <Text size="xs" c="dimmed">つなぎ先とモデルを自分で入れる</Text>
            </Group>
          </UnstyledButton>
          <Collapse expanded={manualOpened}>
            <TextInput
              label="つなぎ先"
              description="OpenAI 互換の API の base URL（…/v1）。「探す」で見つからない場所で動かしているときに入れます。上の一覧で選ぶと、ここも書き換わります。"
              value={draft.baseUrl}
              onChange={(event) => {
                setDraft({
                  ...draft,
                  baseUrl: event.currentTarget.value,
                  remoteConsentUrl: null,
                  verifiedTarget: null,
                });
                setTrial(null);
              }}
              placeholder="http://127.0.0.1:1234/v1"
              disabled={!runtime}
            />
          </Collapse>
        </Stack>
      </Card>

      {!local && (
        <Alert color="orange" icon={<Icons.warning size={IconSize.action} />} title="この端末の外を指しています">
          <Stack gap="xs">
            <Text size="sm">
              題名・作者名・タグ・公式シリーズ名がこのURLへ送られます。タグの補完では作品の概要も、言葉での検索では入力した検索文も送られます。
              {draft.allowBody && <b> あらすじ・前回のあらすじ・本文の校正では、本文も送られます。</b>}
            </Text>
            {!externalSecure && (
              <Text size="sm" c="red"><b>外部の宛先には HTTPS が必要です。</b></Text>
            )}
            <Switch
              checked={remoteAllowed}
              onChange={(event) => setDraft({
                ...draft,
                remoteConsentUrl: event.currentTarget.checked ? normalizedUrl : null,
              })}
              label="表示中のこの宛先へ送ることを許可する"
              disabled={!runtime || !normalizedUrl || !externalSecure}
            />
          </Stack>
        </Alert>
      )}

      {/* ---- 2. 試す ---------------------------------------------------- */}
      <Card withBorder p="lg">
        <Stack gap="md">
          <Group justify="space-between" align="flex-start" wrap="nowrap">
            <div>
              <Text fw={650}>2. 使えるかどうか試す</Text>
              <Text size="sm" c="dimmed">
                つながることと、使えることは別です。棚から同じタグの作品を数作送り、
                それをまとめるコレクションの名前を、候補の中から選んでもらいます。
                答えが返ってくるか、どれくらいかかるかを確かめられます。
              </Text>
            </div>
            <Button
              flex="none"
              variant="default"
              leftSection={<Icons.optimize size={IconSize.action} />}
              loading={runTrial.isPending}
              disabled={!runtime || !ready}
              onClick={() => runTrial.mutate()}
            >
              試し書き
            </Button>
          </Group>

          {profileQuery.data && (
            <Alert color={profileQuery.data.summaryChunkChars > 0 ? "blue" : "orange"} icon={<Icons.optimize size={IconSize.action} />} title="読み取れた性能から、一度に送る量を決めました">
              <Group gap={6} wrap="wrap">
                <Badge variant="light" color="gray">論理CPU {formatNumber(profileQuery.data.logicalCpuCores)}</Badge>
                {profileQuery.data.availableMemoryBytes !== null && (
                  <Badge variant="light" color="gray">空きメモリ {formatBytes(profileQuery.data.availableMemoryBytes)}</Badge>
                )}
                {profileQuery.data.contextLength !== null && (
                  <Badge variant="light" color="gray">文脈長 {formatNumber(profileQuery.data.contextLength)} トークン</Badge>
                )}
                {profileQuery.data.evalBatchSize !== null && (
                  <Badge variant="light" color="gray">batch {formatNumber(profileQuery.data.evalBatchSize)}</Badge>
                )}
                {profileQuery.data.flashAttention && <Badge variant="light" color="green">Flash Attention</Badge>}
                {profileQuery.data.kvCacheOnGpu && <Badge variant="light" color="green">KV キャッシュは GPU</Badge>}
              </Group>
              {profileQuery.data.summaryChunkChars > 0 ? (
                <Text size="sm" mt={6}>
                  長い本文は約{formatNumber(profileQuery.data.summaryChunkChars)}字ずつ送り、
                  このサーバーへの依頼はアプリ全体で同時に{formatNumber(profileQuery.data.concurrentRequests)}件までにします。
                  サーバーが公開している読み込み設定と空きメモリを上限にするだけで、読み込み設定そのものは変えません。
                </Text>
              ) : (
                <Text size="sm" mt={6}>
                  いまの文脈長では、本文の要約を送って答えを受け取る余地がありません。文脈長を8K以上にしてモデルを読み込み直してください。
                </Text>
              )}
            </Alert>
          )}

          {runTrial.isPending && (
            <Group gap="xs">
              <Loader size="xs" />
              <Text size="sm" c="dimmed">
                棚から数作を送って、名前を選んでもらっています…
                （初回はモデルの読み込みで十数秒かかることがあります）
              </Text>
            </Group>
          )}

          {trial && (
            <Alert
              color="green"
              icon={<Icons.confirm size={IconSize.action} />}
              title={`答えが返ってきました（${formatDuration(Math.max(1, trial.elapsedMs / 1000))}）`}
            >
              {/* 名前だけ見せても良し悪しは決められない。何を送り、何から選ばせた
                  のかを並べて、選ばれた名前が送った作品に合うかを見てもらう。 */}
              <Stack gap="sm">
                <div>
                  <Text size="sm" fw={600}>送った作品（{formatNumber(trial.sentTitles.length)}作）</Text>
                  {trial.sentTitles.some((title) => title.trim()) ? (
                    <Text size="sm" c="dimmed">{trial.sentTitles.filter((title) => title.trim()).join("／")}</Text>
                  ) : (
                    <Text size="sm" c="dimmed">題名は送らない設定なので、タグだけを送りました。</Text>
                  )}
                </div>
                <div>
                  <Text size="sm" fw={600}>選んでもらった名前の候補</Text>
                  <Stack gap={2} mt={2}>
                    {trial.candidates.map((candidate) => {
                      const chosen = candidate.name === trial.name;
                      return (
                        <Group key={candidate.name} gap={6} wrap="nowrap">
                          {chosen ? <Icons.confirm size={IconSize.inline} /> : <span style={{ width: IconSize.inline }} />}
                          <Text size="sm" fw={chosen ? 700 : 400}>{candidate.name}</Text>
                          <Text size="xs" c="dimmed">{candidate.subtitle}</Text>
                        </Group>
                      );
                    })}
                  </Stack>
                </div>
                <Text size="sm">
                  モデルが選んだのは<b>「{trial.name}」</b>です。候補は作品の題名・シリーズ・タグから作るので、
                  モデルが名前を書き換えることはありません。答えが返ってくれば、このモデルは使えます。
                  選んだ名前が送った作品に合わないことが続くなら、別のモデルも試してください。
                </Text>
              </Stack>
            </Alert>
          )}
        </Stack>
      </Card>

      {/* ---- 3. 使う ---------------------------------------------------- */}
      <Card withBorder p="lg">
        <Stack gap="md">
          <Switch
            checked={draft.enabled}
            onChange={(event) => setDraft({ ...draft, enabled: event.currentTarget.checked })}
            label="3. モデルに手伝ってもらう"
            description={enableBlocked
              ? "先に 1 でモデルを選び、2 の「試し書き」を通してください。"
              : "切っていても piep はすべて使えます。入れると、各画面に手伝いのボタンが出ます。"}
            disabled={!runtime || enableBlocked}
          />

          <Switch
            checked={draft.allowBody}
            onChange={(event) => setDraft({ ...draft, allowBody: event.currentTarget.checked })}
            label="本文を送ることも許可する"
            description="本文を使うのは、あらすじ・前回のあらすじ・本文の校正だけです。ほかの機能は題名やタグなどで足ります。"
            disabled={!runtime || !draft.enabled}
          />
        </Stack>
      </Card>

      {/* ---- 細かい設定 ------------------------------------------------- */}
      {/* 以前は 1 と 2 のあいだにあり、「探す → 試す → 使う」の流れを途中で
          断ち切っていた。どれも決めなくても使えるので、流れの後ろに置く。 */}
      <Card withBorder p="lg">
        <Stack gap="md">
          <div>
            <Text fw={650}>機能ごとの設定（任意）</Text>
            <Text size="sm" c="dimmed">
              使わない機能は切っておけます。機能ごとに、モデル、追加の指示、送る項目と量を変えられます。
              追加の指示は piep の決まった指示の後ろに足されるだけで、答えの形や安全のための条件は変わりません。
            </Text>
          </div>
          {ASSIST_FEATURES.map((feature) => {
            const profile = draft.featureProfiles[feature.id];
            const sends = FEATURE_SENDS[feature.id];
            const limitsWorks = ["author_style", "collection_split", "collection_naming"].includes(feature.id);
            const limitsTags = sends.tags;
            const nativeAuthorStats = feature.id === "author_style";
            return (
              <Card key={feature.id} withBorder padding="sm" bg="var(--mantine-color-default-hover)">
                <Stack gap="sm">
                  <Group justify="space-between" align="flex-start" wrap="nowrap">
                    <div>
                      <Text size="sm" fw={650}>{feature.label}</Text>
                      <Text size="xs" c="dimmed">{feature.description}</Text>
                      <Text size="xs" c="dimmed">{nativeAuthorStats ? "集計に使うもの" : "送るもの"}：{sends.summary}</Text>
                    </div>
                    <Switch
                      aria-label={`${feature.label}を使う`}
                      checked={profile.enabled}
                      onChange={(event) => updateFeature(feature.id, { enabled: event.currentTarget.checked })}
                    />
                  </Group>
                  {profile.enabled && (
                    <>
                      {!nativeAuthorStats && <>
                        <TextInput
                          label="この機能だけで使うモデル"
                          description="空欄なら、1 で選んだモデルを使います"
                          placeholder={draft.model || "1 で選んだモデル"}
                          value={profile.model}
                          onChange={(event) => updateFeature(feature.id, { model: event.currentTarget.value, verifiedTarget: null })}
                        />
                        {profile.model.trim() && (
                          <Group justify="space-between" gap="sm">
                            <Text size="xs" c={profile.verifiedTarget === assistTarget({ baseUrl: draft.baseUrl, model: profile.model }) ? "green" : "dimmed"}>
                              {profile.verifiedTarget === assistTarget({ baseUrl: draft.baseUrl, model: profile.model })
                                ? "命名の試し書きは通りました。この機能での答えはまだ確かめていません"
                                : "この機能だけのモデルは、一度試し書きを通すと使えます"}
                            </Text>
                            <Button
                              size="compact-xs"
                              variant="light"
                              loading={verifyFeatureModel.isPending && verifyFeatureModel.variables === feature.id}
                              disabled={!runtime || !normalizeAssistBaseUrl(draft.baseUrl)}
                              onClick={() => verifyFeatureModel.mutate(feature.id)}
                            >
                              このモデルを試す
                            </Button>
                          </Group>
                        )}
                        <Textarea
                          label="追加の指示"
                          description="文体や、重く見てほしい点だけを書きます。秘密の情報は書かないでください。"
                          value={profile.additionalInstructions}
                          onChange={(event) => updateFeature(feature.id, { additionalInstructions: event.currentTarget.value })}
                          autosize
                          minRows={2}
                          maxRows={5}
                          maxLength={2_000}
                        />
                      </>}
                      {feature.id === "work_tagging" && <Textarea
                        label="タグの判定条件"
                        description="「タグ名：条件」を一行に一つ。似たタグの違いや、付けない場合を決めておけます。空欄ならタグ名から判断します。"
                        placeholder="園芸：植物を育てたり手入れしたりする描写がある"
                        value={profile.tagCriteria}
                        onChange={(event) => updateFeature(feature.id, { tagCriteria: event.currentTarget.value })}
                        autosize minRows={2} maxRows={8} maxLength={12_000}
                      />}
                      {(limitsWorks || limitsTags) && <Group grow align="flex-end">
                        {limitsWorks && <NumberInput
                          label={nativeAuthorStats ? "集計する作品数の上限" : "送る作品数の上限"}
                          min={1}
                          max={1_000}
                          value={profile.inputPolicy.maxItems ?? 200}
                          onChange={(value) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, maxItems: Number(value) || 1 } })}
                        />}
                        {limitsTags && <NumberInput
                          label={nativeAuthorStats ? "1作品あたりに集計するタグ数の上限" : "1作品あたりに送るタグ数の上限"}
                          min={1}
                          max={100}
                          value={profile.inputPolicy.maxTagsPerItem ?? 30}
                          onChange={(value) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, maxTagsPerItem: Number(value) || 1 } })}
                        />}
                      </Group>}
                      {(sends.title || sends.author || sends.tags || sends.excerpt) && (
                        <Stack gap={6}>
                          <Text size="xs" fw={600}>{nativeAuthorStats ? "集計に使う項目" : "送る項目"}</Text>
                          <Group gap="lg">
                            {sends.title && <Switch size="sm" label="題名" checked={profile.inputPolicy.includeTitle ?? true} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeTitle: event.currentTarget.checked } })} />}
                            {sends.author && <Switch size="sm" label="作者名" checked={profile.inputPolicy.includeAuthor ?? true} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeAuthor: event.currentTarget.checked } })} />}
                            {sends.tags && <Switch size="sm" label="タグ" checked={profile.inputPolicy.includeTags ?? true} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeTags: event.currentTarget.checked } })} />}
                            {sends.excerpt && <Switch size="sm" label="概要" checked={profile.inputPolicy.includeExcerpt ?? false} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeExcerpt: event.currentTarget.checked } })} />}
                          </Group>
                        </Stack>
                      )}
                    </>
                  )}
                </Stack>
              </Card>
            );
          })}
        </Stack>
      </Card>

      <Card withBorder p="lg">
        <Switch
          label="作品単位の意味検索を使う"
          description="言葉で探すとき、作品ごとの意味の近さでも候補を探します。ふだんの検索は、これまでどおり文字の一致で探します。"
          checked={semanticEnabled}
          onChange={(event) => setSemanticEnabled(event.currentTarget.checked)}
        />
      </Card>

      <Alert icon={<Icons.secure size={IconSize.action} />} title="送るもの・送らないもの">
        <Stack gap={6}>
          <Text size="sm">
            <b>題名・作者名・タグ・公式シリーズ名</b> — タグの補完、コレクションの命名と分割に使います。
            タグの補完では作品の概要も送り、言葉での検索では入力した検索文とタグの候補を送ります。どれも本文は使いません。
            作者の題材メモはモデルも通信も使わず、手元の作品のタグを集計するだけです。
          </Text>
          <Text size="sm">
            <b>本文</b> — あらすじ、前回のあらすじ、本文の校正だけが使い、上で許可したときに限って送ります。
            あらすじは、モデルが公開している文脈長に収まる大きさ（分からなければ約2,800字）に区切って本文全体を送り、
            区切りごとの覚え書きを順にまとめます。校正は、選んだブロックか編集中の本文を約3,000字ずつ送ります。画像は送りません。
          </Text>
          <Text size="sm">
            返ってきたものはすべて<b>案</b>で、採るかどうかはあなたが決めます。
            モデルの案から付けたタグは<b>取得元のタグと分けて保存</b>するので、あとから外せます。
          </Text>
        </Stack>
      </Alert>

      {/* 保存は最後の帯にまとめる。機能ごとの設定を直したあとも、ここまで
          戻らずに押せるよう、画面の下に貼り付けておく。 */}
      <Group className="assist-save-bar" justify="space-between" wrap="nowrap">
        <Group gap={6} miw={0}>
          <Badge variant="light" color={local ? "green" : "orange"}>{local ? "この端末の中" : "外部の宛先"}</Badge>
          {draft.model && <Badge variant="light" color="gray" tt="none" maw={260}>{draft.model}</Badge>}
          {/* 押せない保存ボタンの横に「変更があります」だけを出すと、なぜ押せないのかが
              分からない。押せないときは、その理由を代わりに出す。 */}
          {dirty && (
            <Text size="xs" c="orange" fw={600}>
              {saveProblem ?? "保存していない変更があります"}
            </Text>
          )}
        </Group>
        <Button
          loading={save.isPending}
          disabled={!runtime || Boolean(saveProblem)}
          title={saveProblem ?? undefined}
          onClick={() => save.mutate(draft)}
        >
          保存
        </Button>
      </Group>
    </Stack>
  );
}
