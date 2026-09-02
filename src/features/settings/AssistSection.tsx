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
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
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
 * コレクションの名前を、手元の言語モデルにも考えてもらうための設定。
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
      if (runtime) {
        // 意味検索の入り切りが**変わったときだけ**触る。
        //
        // 以前は保存のたびに再構築を頼んでいた。追加指示の一文を直して保存した
        // だけで索引の作り直しが始まり、棚の書き込み権を取り、アクティビティに
        // 身に覚えのない行が並んでいた。
        const wasEnabled = semanticStatusQuery.data?.semanticEnabled;
        if (semanticEnabled !== wasEnabled) {
          if (semanticEnabled) await startSearchRebuildIndex({ includeSemantic: true });
          else await setSemanticSearchEnabled(false);
        }
      }
      await saveAssistSettings(next);
    },
    onSuccess: (_result, next) => {
      queryClient.invalidateQueries({ queryKey: ["naming-settings"] });
      queryClient.invalidateQueries({ queryKey: ["search-index-status"] });
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
        message: `${ASSIST_FEATURES.find((feature) => feature.id === featureId)?.label ?? featureId}のモデルを確認しました（${formatNumber(result.elapsedMs)}ミリ秒）`,
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

  return (
    <Stack gap="lg">
      <div>
        <Text fw={700} size="lg">AIの手伝い</Text>
        <Text size="sm" c="dimmed">
          コレクションの名前、タグの補完、あらすじ、言葉での検索。
          <b>どれも piep 単体で完結するように作ってあり、ここは設定しなくても動きます。</b>
          手元で動かしている言語モデルに手伝わせたいときだけ使ってください。
          <b>裏で勝手に走ることはありません</b> — 押したときだけ動きます。
        </Text>
      </div>

      {/* ---- 1. 探す ---------------------------------------------------- */}
      <Card withBorder p="lg">
        <Stack gap="md">
          <Group justify="space-between" align="flex-start" wrap="nowrap">
            <div>
              <Text fw={650}>1. この端末で動いているモデルを探す</Text>
              <Text size="sm" c="dimmed">
                LM Studio・Ollama・llama.cpp などが動いていれば、そのまま見つかります。
              </Text>
            </div>
            <Button
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
                  推論サーバーが動いていないようです。piep はモデルを持っていないので、
                  <b>別のアプリで動かしたものを借りて使います。</b>
                </Text>
                <UnstyledButton onClick={help.toggle}>
                  <Group gap={4}>
                    <Icons.expand size={IconSize.inline} style={{ transform: helpOpened ? "rotate(180deg)" : undefined }} />
                    <Text size="sm" fw={650}>どうやって用意するか</Text>
                  </Group>
                </UnstyledButton>
                <Collapse expanded={helpOpened}>
                  <List size="sm" spacing={6} pt={6}>
                    <List.Item>
                      <b>LM Studio</b> — モデルを選んで「Local Server」を開始します。
                      いちばん手数が少ないので、初めてならこれをおすすめします。
                    </List.Item>
                    <List.Item>
                      <b>Ollama</b> — 入れると常駐します。<Code>ollama pull &lt;モデル名&gt;</Code> で
                      モデルを取ってくれば、そのまま見つかります。
                    </List.Item>
                    <List.Item>
                      どちらも起動したら、この画面で<b>もう一度「探す」</b>を押してください。
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
                    <Text size="sm" fw={650}>{engine.label} のあたり</Text>
                    <Text size="xs" c="dimmed">
                      モデル {formatNumber(engine.models.length)}件 · {engine.baseUrl}
                    </Text>
                  </span>
                  {draft.baseUrl === engine.baseUrl && <Icons.confirm size={IconSize.menu} />}
                </UnstyledButton>
              ))}
            </Stack>
          )}

          <UnstyledButton onClick={manual.toggle}>
            <Group gap={4}>
              <Icons.expand size={IconSize.inline} style={{ transform: manualOpened ? "rotate(180deg)" : undefined }} />
              <Text size="xs" c="dimmed">つなぎ先を自分で入れる</Text>
            </Group>
          </UnstyledButton>
          <Collapse expanded={manualOpened}>
            <TextInput
              label="つなぎ先"
              description="OpenAI 互換の base URL。既定の場所以外で動かしている場合だけ使います。"
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

      <Card withBorder p="lg">
        <Stack gap="md">
          <div>
            <Text fw={650}>機能ごとの設定</Text>
            <Text size="sm" c="dimmed">
              使う機能だけを有効にし、モデル・追加指示・送る情報の上限を個別に決めます。
              追加指示は piep の出力契約の後ろに足され、形式や安全条件は変更できません。
            </Text>
          </div>
          {ASSIST_FEATURES.map((feature) => {
            const profile = draft.featureProfiles[feature.id];
            const limitsWorks = ["author_style", "collection_split", "collection_naming"].includes(feature.id);
            const limitsTags = ["work_tagging", "author_style", "collection_split", "collection_naming"].includes(feature.id);
            const sendsTitle = feature.id !== "search_interpretation";
            const sendsAuthor = ["work_tagging", "collection_split", "collection_naming"].includes(feature.id);
            const sendsTags = feature.id !== "search_interpretation";
            const sendsExcerpt = feature.id === "work_tagging";
            return (
              <Card key={feature.id} withBorder padding="sm" bg="var(--mantine-color-default-hover)">
                <Stack gap="sm">
                  <Group justify="space-between" align="flex-start" wrap="nowrap">
                    <div>
                      <Text size="sm" fw={650}>{feature.label}</Text>
                      <Text size="xs" c="dimmed">{feature.description}</Text>
                    </div>
                    <Switch
                      aria-label={`${feature.label}を使う`}
                      checked={profile.enabled}
                      onChange={(event) => updateFeature(feature.id, { enabled: event.currentTarget.checked })}
                    />
                  </Group>
                  {profile.enabled && (
                    <>
                      <TextInput
                        label="この機能だけで使うモデル"
                        description="空なら上で選んだ共通モデルを使います"
                        placeholder={draft.model || "共通モデル"}
                        value={profile.model}
                        onChange={(event) => updateFeature(feature.id, { model: event.currentTarget.value, verifiedTarget: null })}
                      />
                      {profile.model.trim() && (
                        <Group justify="space-between" gap="sm">
                          <Text size="xs" c={profile.verifiedTarget === assistTarget({ baseUrl: draft.baseUrl, model: profile.model }) ? "green" : "dimmed"}>
                            {profile.verifiedTarget === assistTarget({ baseUrl: draft.baseUrl, model: profile.model })
                              ? "この接続先とモデルは確認済みです"
                              : "機能別モデルは一度試してから使えます"}
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
                        description="文体や重視する観点だけを書きます。秘密情報は入力しないでください。"
                        value={profile.additionalInstructions}
                        onChange={(event) => updateFeature(feature.id, { additionalInstructions: event.currentTarget.value })}
                        autosize
                        minRows={2}
                        maxRows={5}
                        maxLength={2_000}
                      />
                      {(limitsWorks || limitsTags) && <Group grow align="flex-end">
                        {limitsWorks && <NumberInput
                          label="送る作品数の上限"
                          min={1}
                          max={1_000}
                          value={profile.inputPolicy.maxItems ?? 200}
                          onChange={(value) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, maxItems: Number(value) || 1 } })}
                        />}
                        {limitsTags && <NumberInput
                          label="1作品あたりのタグ上限"
                          min={1}
                          max={100}
                          value={profile.inputPolicy.maxTagsPerItem ?? 30}
                          onChange={(value) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, maxTagsPerItem: Number(value) || 1 } })}
                        />}
                      </Group>}
                      {(sendsTitle || sendsAuthor || sendsTags || sendsExcerpt) && (
                        <Group gap="lg">
                          {sendsTitle && <Switch size="sm" label="題名" checked={profile.inputPolicy.includeTitle ?? true} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeTitle: event.currentTarget.checked } })} />}
                          {sendsAuthor && <Switch size="sm" label="作者名" checked={profile.inputPolicy.includeAuthor ?? true} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeAuthor: event.currentTarget.checked } })} />}
                          {sendsTags && <Switch size="sm" label="タグ" checked={profile.inputPolicy.includeTags ?? true} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeTags: event.currentTarget.checked } })} />}
                          {sendsExcerpt && <Switch size="sm" label="概要" checked={profile.inputPolicy.includeExcerpt ?? false} onChange={(event) => updateFeature(feature.id, { inputPolicy: { ...profile.inputPolicy, includeExcerpt: event.currentTarget.checked } })} />}
                        </Group>
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
        <Stack gap="sm">
          <Switch
            label="作品単位の意味検索を使う"
            description="言葉で探すとき、作品ごとに一つのベクトルで候補を探します。通常の検索は従来どおり字面検索です。"
            checked={semanticEnabled}
            onChange={(event) => setSemanticEnabled(event.currentTarget.checked)}
          />
        </Stack>
      </Card>

      {!local && (
        <Alert color="orange" icon={<Icons.warning size={IconSize.action} />} title="この端末の外を指しています">
          <Stack gap="xs">
            <Text size="sm">
              題名・作者名・タグ・公式シリーズ名が、このURLへ送られます。
              {draft.allowBody && <b> あらすじを作るときは、分割した本文全体も送られます。</b>}
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
          <div>
            <Text fw={650}>2. 使えるかどうか試す</Text>
            <Text size="sm" c="dimmed">
              つながることと、使えることは別です。あなたの棚の作品を実際に渡して、
              どんな名前が返ってくるかを見てから決めてください。
            </Text>
          </div>

          <Group align="flex-end" gap="sm">
            {manualOpened ? (
              <TextInput
                flex={1}
                label="モデル名"
                description="つなぎ先が /models を公開していない場合も、APIで使うモデルIDを直接入力できます。"
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
                flex={1}
                label="モデル"
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
            <Button
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
            <Alert color={profileQuery.data.summaryChunkChars > 0 ? "blue" : "orange"} icon={<Icons.optimize size={IconSize.action} />} title="検出できた能力から安全枠を計算">
              <Group gap={6} wrap="wrap">
                <Badge variant="light" color="gray">論理CPU {formatNumber(profileQuery.data.logicalCpuCores)}</Badge>
                {profileQuery.data.availableMemoryBytes !== null && (
                  <Badge variant="light" color="gray">空きメモリ {formatBytes(profileQuery.data.availableMemoryBytes)}</Badge>
                )}
                {profileQuery.data.contextLength !== null && (
                  <Badge variant="light" color="gray">文脈 {formatNumber(profileQuery.data.contextLength)} token</Badge>
                )}
                {profileQuery.data.evalBatchSize !== null && (
                  <Badge variant="light" color="gray">batch {formatNumber(profileQuery.data.evalBatchSize)}</Badge>
                )}
                {profileQuery.data.flashAttention && <Badge variant="light" color="green">Flash Attention</Badge>}
                {profileQuery.data.kvCacheOnGpu && <Badge variant="light" color="green">KV cache GPU</Badge>}
              </Group>
              {profileQuery.data.summaryChunkChars > 0 ? (
                <Text size="sm" mt={6}>
                  長文は最大約{formatNumber(profileQuery.data.summaryChunkChars)}文字ずつ、
                  この推論先へはアプリ全体で最大{formatNumber(profileQuery.data.concurrentRequests)}件まで同時に処理します。
                  サーバーが公開したロード設定と空きメモリを上限として使い、ロード設定そのものは変更しません。
                </Text>
              ) : (
                <Text size="sm" mt={6}>
                  現在の文脈長では本文要約の入力と出力を安全に収められません。8K以上の文脈長でモデルを読み直してください。
                </Text>
              )}
            </Alert>
          )}

          {runTrial.isPending && (
            <Group gap="xs">
              <Loader size="xs" />
              <Text size="sm" c="dimmed">
                棚から数作を渡して、名前を考えてもらっています…
                （初回はモデルの読み込みで十数秒かかることがあります）
              </Text>
            </Group>
          )}

          {trial && (
            <Alert color="green" icon={<Icons.confirm size={IconSize.action} />} title="返ってきました">
              <Stack gap={4}>
                <Text fw={700}>{trial.name}</Text>
                <Text size="sm" c="dimmed">{trial.subtitle}</Text>
                <Text size="xs" c="dimmed">{formatNumber(trial.elapsedMs)}ミリ秒</Text>
                <Text size="sm" mt={6}>
                  この名前が中身と噛み合っていれば、そのモデルで大丈夫です。
                  <b>言い換えられた・ぼやけた名前が返る場合は、モデルを替えてください。</b>
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
            description="切っていても piep は完結します。手伝いのボタンが画面に出るようになるだけです。"
            disabled={!runtime || (!draft.enabled && (!ready || !verified))}
          />

          <Switch
            checked={draft.allowBody}
            onChange={(event) => setDraft({ ...draft, allowBody: event.currentTarget.checked })}
            label="本文を送ることも許可する"
            description="あらすじと「前回のあらすじ」だけが本文を使います。名前・タグ・検索の言い換えは題名とタグだけで足ります。"
            disabled={!runtime || !draft.enabled}
          />

          <Group justify="space-between">
            <Group gap={6}>
              <Badge variant="light" color={local ? "green" : "orange"}>{local ? "この端末の中" : "外部の宛先"}</Badge>
              {draft.model && <Badge variant="light" color="gray">{draft.model}</Badge>}
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
      </Card>

      <Alert icon={<Icons.secure size={IconSize.action} />} title="送るもの・送らないもの">
        <Stack gap={6}>
          <Text size="sm">
            <b>題名・作者名・タグ・公式シリーズ名</b> — 名前、タグの補完、作風のまとめ、
            束の分割、言葉での検索。これらは本文を使いません。
          </Text>
          <Text size="sm">
            <b>本文</b> — あらすじと「前回のあらすじ」だけ。上で明示的に許可したときに限り、
            本文全体を、モデルが公開した文脈長に収まる大きさ（不明時は約2,800字）で隙間なく送り、
            部分ごとの記録を段階的に統合します。画像は送りません。
          </Text>
          <Text size="sm">
            返ってきたものはすべて<b>案</b>です。採るかどうかは利用者が決めます。
            モデルが付けたタグは <b>取得元のタグと区別して保存</b>され、あとから外せます。
          </Text>
        </Stack>
      </Alert>
    </Stack>
  );
}
