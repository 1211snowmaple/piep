import { useEffect, useMemo, useRef, useState } from "react";
import {
  Accordion,
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  Grid,
  Group,
  NumberInput,
  Progress,
  ScrollArea,
  SegmentedControl,
  Select,
  Slider,
  Stack,
  Switch,
  Text,
  TextInput,
  Title,
  Tooltip,
} from "@mantine/core";
import { useForm, isNotEmpty, type UseFormReturnType } from "@mantine/form";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery } from "@tanstack/react-query";
import { WorkCard } from "@/components/WorkCard";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { PageHeader } from "@/components/PageHeader";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { demoWorks } from "@/mocks/demoData";
import { getDownloads, isTauriRuntime } from "@/services/dbApi";
import { openSingleDialog } from "@/services/dialogApi";
import { cancelEpubExport, exportEpubBatch, listEpubTemplates } from "@/services/epubApi";
import { subscribeTauriEvent } from "@/services/eventBus";
import { openFilesystemPath } from "@/services/openerApi";
import type { DownloadEntry } from "@/types/library";
import type { ExportBatchResult, ExportProgress, TemplateInfo } from "@/types/epub";
import { startOperation, type OperationController } from "@/features/jobs/operationJobs";
import { demoTemplates } from "./templateStudioDemo";
import { AUTO_TEMPLATE, readExportSettings, toCompressOptions, writeExportSettings, type EpubExportSettings } from "./exportSettings";

type EpubValues = EpubExportSettings;

export default function EpubPage() {
  const navigate = useAppNavigate();
  const runtime = isTauriRuntime();
  const { epubQueue, removeFromEpubQueue, clearEpubQueue } = useWorkspace();
  const [progress, setProgress] = useState<ExportProgress | null>(null);
  const [result, setResult] = useState<ExportBatchResult | null>(null);
  const exportOperationRef = useRef<OperationController | null>(null);
  const retryExportRef = useRef<(values: EpubValues) => void>(() => undefined);
  // 前に書き出したときの決めごとから始める。開くたびに初期値へ戻り、出力先
  // フォルダーまで毎回選び直しだった。
  const form = useForm<EpubValues>({ initialValues: readExportSettings(), validate: { templateName: isNotEmpty("テンプレートを選択してください"), outputDir: isNotEmpty("出力先を選択してください") }, validateInputOnBlur: true });
  // One request for the whole queue: a per-work query meant a queue of a few
  // hundred books fired that many IPC round trips on every visit.
  const queueQuery = useQuery({
    queryKey: ["epub-queue-works", epubQueue],
    queryFn: () => runtime
      ? getDownloads(epubQueue)
      : Promise.resolve(epubQueue.map((id) => demoWorks.find((work) => work.id === id)).filter((work): work is DownloadEntry => Boolean(work))),
    enabled: epubQueue.length > 0,
  });
  const works = useMemo(() => queueQuery.data ?? [], [queueQuery.data]);
  const templates = useQuery({
    queryKey: ["epub-templates"],
    queryFn: () => runtime ? listEpubTemplates() : Promise.resolve<TemplateInfo[]>(demoTemplates),
  });
  useEffect(() => {
    if (!runtime) return undefined;
    return subscribeTauriEvent<ExportProgress>("epub-export-progress", (event) => {
      setProgress(event.payload);
      exportOperationRef.current?.progress(event.payload.currentIndex, event.payload.totalCount, event.payload.message);
    });
  }, [runtime]);
  // Works deleted from the library after being queued simply do not come back
  // from the bulk fetch, so drop them from the queue instead of showing gaps.
  useEffect(() => {
    if (!queueQuery.isSuccess || !epubQueue.length) return;
    const found = new Set(works.map((work) => work.id));
    const staleIds = epubQueue.filter((id) => !found.has(id));
    if (staleIds.length) removeFromEpubQueue(staleIds);
  }, [epubQueue, queueQuery.isSuccess, removeFromEpubQueue, works]);

  const exportMutation = useMutation({
    mutationFn: async (values: EpubValues): Promise<ExportBatchResult> => {
      exportOperationRef.current = startOperation({
        kind: "epub",
        label: `${works.length}冊をEPUBへ書き出し`,
        detail: values.outputDir,
        total: works.length,
        onRetry: () => retryExportRef.current(values),
        // 数百冊を並べて実行したら、終わるまで止められなかった。作りかけの
        // 1 冊は書き切ってから止まるので、半端な EPUB は残らない。
        onCancel: runtime ? async () => { await cancelEpubExport(); } : undefined,
      });
      setResult(null); setProgress({ phase: "started", currentTitle: "", currentIndex: 0, totalCount: works.length, message: "書き出しを準備しています" });
      if (!runtime) {
        await new Promise((resolve) => window.setTimeout(resolve, 500));
        return { successCount: works.length, failedCount: 0, failedIds: [], invalidIds: [], outputFiles: works.map((work) => `${values.outputDir}/${work.title}.epub`), invalidCount: 0, issues: [], canceled: false, skippedIds: [] };
      }
      return exportEpubBatch<ExportBatchResult>({
        downloadIds: works.map((work) => work.id),
        templateName: values.templateName,
        outputDir: values.outputDir,
        writingMode: values.writingMode,
        compressOptions: toCompressOptions(values.compression),
      });
    },
    onSuccess: (data) => {
      if (data.canceled) exportOperationRef.current?.cancel(`成功 ${data.successCount} · 未着手 ${data.skippedIds.length}`);
      else exportOperationRef.current?.complete(`成功 ${data.successCount} · 失敗 ${data.failedCount}`);
      exportOperationRef.current = null;
      setResult(data);
      setProgress(null);
      // 書けなかったもの、検証を通らなかったもの、そして**一度も試して
      // いないもの**はキューに残す。止めた結果として棚から消えたのでは、
      // 中止が取り下げになってしまう。
      const keep = new Set([...data.failedIds, ...data.invalidIds, ...data.skippedIds]);
      removeFromEpubQueue(works.filter((work) => !keep.has(work.id)).map((work) => work.id));
      const needsReview = data.failedCount > 0 || data.invalidCount > 0 || data.issues.length > 0;
      notifications.show({
        color: data.canceled ? "yellow" : needsReview ? "yellow" : "green",
        title: data.canceled ? "EPUB書き出しを中止しました" : "EPUB書き出しが完了しました",
        message: data.canceled
          ? `成功 ${data.successCount} · 残り${data.skippedIds.length}件はキューに残しました`
          : `成功 ${data.successCount} · 失敗 ${data.failedCount}${data.invalidCount ? ` · 検証不合格 ${data.invalidCount}` : ""}`,
      });
    },
    onError: (error) => { exportOperationRef.current?.fail(error); exportOperationRef.current = null; setProgress(null); notifications.show({ color: "red", title: "EPUBを書き出せません", message: errorMessage(error) }); },
  });
  retryExportRef.current = (values) => exportMutation.mutate(values);
  const selectOutput = async () => {
    if (!runtime) return form.setFieldValue("outputDir", "C:/Users/preview/Documents/piep exports");
    const path = await openSingleDialog({ directory: true, title: "EPUBの出力先" });
    if (!path) return;
    form.setFieldValue("outputDir", path);
    // 選んだ先はその場で憶える。書き出す前に画面を離れても、次は選び直さずに済む。
    writeExportSettings({ ...form.getValues(), outputDir: path });
  };
  const totalSize = useMemo(() => works.reduce((sum, work) => sum + work.fileSizeBytes, 0), [works]);
  const templateOptions = useMemo(() => [
    { value: AUTO_TEMPLATE, label: "自動（作品の取得元に合わせる）" },
    ...(templates.data ?? []).map((item) => ({ value: item.name, label: `${item.settings.label || item.name}${item.isBuiltin ? "（標準）" : ""}` })),
  ], [templates.data]);

  return (
    <div className="page page--contained epub-page">
      <PageHeader title="EPUB書き出し" description="選んだ作品を、端末に合わせた高品質な電子書籍へ書き出します。" actions={<Button variant="default" leftSection={<Icons.epubTemplate size={IconSize.menu} />} onClick={() => navigate("/epub/templates")}>テンプレートスタジオ</Button>} />
      {!epubQueue.length ? <EmptyState icon={Icons.epub} title="EPUBキューは空です" description="ライブラリや作品詳細から、書き出したい作品をキューに追加してください。" action={<Button onClick={() => navigate("/library")}>ライブラリを開く</Button>} /> : (
        <form onSubmit={form.onSubmit((values) => { writeExportSettings(values); exportMutation.mutate(values); })}>
          <Grid gap="lg" align="flex-start">
            <Grid.Col span={{ base: 12, lg: 7 }}>
              <Stack gap="lg">
                <Card p="lg">
                  <Group justify="space-between" mb="md"><Box><Title order={3}>書き出す作品</Title><Text size="sm" c="dimmed">{formatNumber(works.length)}件 · 元データ {formatBytes(totalSize)}</Text></Box><Button variant="subtle" color="red" size="xs" onClick={() => modals.openConfirmModal({ title: "キューを空にしますか？", children: <Text size="sm">選択中の{epubQueue.length}件をキューから外します。</Text>, confirmProps: { color: "red" }, labels: { confirm: "空にする", cancel: "キャンセル" }, onConfirm: clearEpubQueue })}>すべて解除</Button></Group>
                  {queueQuery.isLoading ? <LoadingState label="作品情報を読み込んでいます" /> : queueQuery.error ? <ErrorState error={queueQuery.error} retry={() => queueQuery.refetch()} /> : <Stack gap="xs">{works.map((work, index) => <div key={work.id} className="epub-queue-item"><Text className="epub-queue-item__index" fw={700}>{index + 1}</Text><div className="epub-queue-item__body"><WorkCard work={work} compact /></div><Tooltip label="キューから外す"><ActionIcon variant="subtle" color="red" aria-label={`${work.title}をキューから外す`} onClick={() => removeFromEpubQueue(work.id)}><Icons.cancel size={IconSize.action} /></ActionIcon></Tooltip></div>)}</Stack>}
                </Card>
                <CompressionSettings form={form} />
              </Stack>
            </Grid.Col>
            <Grid.Col span={{ base: 12, lg: 5 }}>
              <Card p="lg" className="epub-export-card">
                <Stack gap="lg">
                  <Box><Title order={3}>出力設定</Title><Text size="sm" c="dimmed">EPUB 3形式で作品ごとに出力します。</Text></Box>
                  <Select label="テンプレート" description="組版と構成はテンプレートスタジオで編集できます" data={templateOptions} disabled={templates.isLoading} {...form.getInputProps("templateName")} error={form.errors.templateName || (templates.error ? "テンプレートを読み込めません" : undefined)} rightSection={<Icons.next size={IconSize.menu} />} />
                  {/* The icon sits inside the input, so its click also bubbles
                      to the field handler and opened two pickers in a row. */}
                  <TextInput label="出力先フォルダー" placeholder="フォルダーを選択" readOnly {...form.getInputProps("outputDir")} rightSection={<ActionIcon variant="subtle" aria-label="出力先を選択" onClick={(event) => { event.stopPropagation(); selectOutput(); }}><Icons.openFolder size={IconSize.action} /></ActionIcon>} onClick={selectOutput} />
                  {/* 読む画面には縦書きがあるのに、書き出す本は必ず横書きだった。
                      テンプレートを自分で作らないかぎり縦組みにできない、という
                      のは、日本語の小説を持ち出す道具として通らない。 */}
                  <Box>
                    <Text size="sm" fw={500} mb={6}>組み方</Text>
                    <SegmentedControl
                      fullWidth
                      size="xs"
                      aria-label="EPUBの組み方"
                      value={form.values.writingMode ?? "template"}
                      onChange={(value) => form.setFieldValue("writingMode", value === "template" ? null : value as "vertical" | "horizontal")}
                      data={[{ value: "template", label: "テンプレートに従う" }, { value: "vertical", label: "縦書き" }, { value: "horizontal", label: "横書き" }]}
                    />
                    <Text size="xs" c="dimmed" mt={6}>標準のテンプレートは横書きです。縦書きを選んだときだけ、綴じ方向も右から左になります。</Text>
                  </Box>
                  <Alert color="piep" icon={<Icons.epubImages size={IconSize.action} />}>{form.values.compression.enabled ? `画像を最大 ${form.values.compression.maxWidth} × ${form.values.compression.maxHeight}px に最適化します。` : "画像は元の品質のまま収録します。"}</Alert>
                  {progress && <Stack gap={6} role="status" aria-live="polite"><Group justify="space-between"><Text size="sm" className="line-clamp-1">{progress.currentTitle || progress.message}</Text><Text size="xs" c="dimmed">{progress.currentIndex}/{progress.totalCount}</Text></Group><Progress value={progress.totalCount ? progress.currentIndex / progress.totalCount * 100 : 5} animated aria-label={`EPUB書き出し ${progress.currentIndex}/${progress.totalCount}`} /><Text size="xs" c="dimmed">{progress.message}</Text></Stack>}
                  {result && <ExportResult result={result} outputDir={form.values.outputDir} runtime={runtime} />}
                  <Button type="submit" size="lg" leftSection={<Icons.epub size={IconSize.action} />} loading={exportMutation.isPending} disabled={!works.length}>{works.length}冊を書き出す</Button>
                </Stack>
              </Card>
            </Grid.Col>
          </Grid>
        </form>
      )}
    </div>
  );
}

/**
 * What came out, including what the validator found.
 *
 * Every exported book is opened again and checked, because a file that a reader
 * or Send to Kindle silently refuses looks exactly like a successful export.
 */
function ExportResult({ result, outputDir, runtime }: { result: ExportBatchResult; outputDir: string; runtime: boolean }) {
  const clean = !result.canceled && !result.failedCount && !result.invalidCount && !result.issues.length;
  return (
    <Alert color={clean ? "green" : "yellow"} icon={<Icons.confirm size={IconSize.action} />} title={result.canceled ? "書き出しを中止しました" : "書き出し完了"}>
      <Stack gap="xs">
        <Text size="sm">成功 {result.successCount}件 / 失敗 {result.failedCount}件{result.canceled ? ` / 未着手 ${result.skippedIds.length}件` : ""}</Text>
        {result.canceled && <Text size="xs" c="dimmed">中止した時点で作りかけだった1冊は書き切っています。残りはキューに残したので、そのまま続きから書き出せます。</Text>}
        {result.issues.length > 0 && (
          <Box>
            <Text size="sm" fw={600}>{result.invalidCount > 0 ? `${result.invalidCount}件はEPUBの検証を通過しませんでした` : "Kindle互換性について確認事項があります"}</Text>
            <Text size="xs" c="dimmed">エラーの作品はキューに残ります。警告は書き出せますが、品質を確認してください。</Text>
            <ScrollArea.Autosize mah={140} mt={6}>
              <Stack gap={2}>{result.issues.slice(0, 20).map((issue, index) => <Text key={index} size="xs" c="dimmed">{issue.location}：{issue.message}</Text>)}</Stack>
            </ScrollArea.Autosize>
          </Box>
        )}
        {outputDir && <Button size="xs" variant="light" w="fit-content" leftSection={<Icons.openFolder size={IconSize.inline} />} disabled={!runtime} onClick={() => openFilesystemPath(outputDir)}>出力先を開く</Button>}
      </Stack>
    </Alert>
  );
}

function CompressionSettings({ form }: { form: UseFormReturnType<EpubValues, EpubValues, any> }) {
  const enabled = form.values.compression.enabled;
  return (
    <Card p="lg"><Group justify="space-between"><Box><Title order={3}>画像最適化</Title><Text size="sm" c="dimmed">品質とファイルサイズのバランス</Text></Box><Switch checked={enabled} onChange={(event) => form.setFieldValue("compression.enabled", event.currentTarget.checked)} aria-label="画像最適化" /></Group>
      {enabled && <Stack gap="lg" mt="lg"><Grid><Grid.Col span={6}><NumberInput label="最大幅" hideControls suffix=" px" min={320} max={8000} {...form.getInputProps("compression.maxWidth")} /></Grid.Col><Grid.Col span={6}><NumberInput label="最大高さ" hideControls suffix=" px" min={320} max={8000} {...form.getInputProps("compression.maxHeight")} /></Grid.Col></Grid><Select label="出力形式" description="自動は元画像の形式を保ちます" data={[{ value: "auto", label: "自動" }, { value: "jpeg", label: "JPEG" }, { value: "png", label: "PNG" }, { value: "webp", label: "WebP" }]} value={form.values.compression.outputFormat ?? "auto"} onChange={(value) => form.setFieldValue("compression.outputFormat", value === "auto" ? null : value)} />
        <Accordion variant="separated">
          <Accordion.Item value="jpeg"><Accordion.Control>JPEG詳細</Accordion.Control><Accordion.Panel><Stack><LabeledSlider label="品質" value={form.values.compression.jpegQuality} onChange={(value) => form.setFieldValue("compression.jpegQuality", value)} min={40} max={100} /><Select label="クロマサブサンプリング" data={["4:2:0", "4:2:2", "4:4:4"]} {...form.getInputProps("compression.jpegChromaSubsampling")} /><Checkbox label="プログレッシブ" {...form.getInputProps("compression.jpegProgressive", { type: "checkbox" })} /><Checkbox label="リンギング低減" {...form.getInputProps("compression.jpegDeringing", { type: "checkbox" })} /><Checkbox label="自動最適化（高品質・低速）" {...form.getInputProps("compression.jpegAutoOptimize", { type: "checkbox" })} /></Stack></Accordion.Panel></Accordion.Item>
          <Accordion.Item value="png"><Accordion.Control>PNG詳細</Accordion.Control><Accordion.Panel><Stack><SegmentedControl fullWidth aria-label="PNG圧縮レベル" data={[{ value: "1", label: "高速" }, { value: "2", label: "標準" }, { value: "4", label: "高圧縮" }]} {...form.getInputProps("compression.pngCompression")} /><Checkbox label="不要なメタデータを削除" {...form.getInputProps("compression.pngStrip", { type: "checkbox" })} /><Checkbox label="ビット深度を最適化" {...form.getInputProps("compression.pngBitDepthReduction", { type: "checkbox" })} /><Checkbox label="カラーパレットを最適化" {...form.getInputProps("compression.pngPaletteReduction", { type: "checkbox" })} /></Stack></Accordion.Panel></Accordion.Item>
          <Accordion.Item value="webp"><Accordion.Control>WebP詳細</Accordion.Control><Accordion.Panel><Stack><LabeledSlider label="品質" value={form.values.compression.webpQuality} onChange={(value) => form.setFieldValue("compression.webpQuality", value)} min={30} max={100} /><LabeledSlider label="圧縮方法" value={form.values.compression.webpMethod} onChange={(value) => form.setFieldValue("compression.webpMethod", value)} min={0} max={6} /><Checkbox label="ロスレス" {...form.getInputProps("compression.webpLossless", { type: "checkbox" })} /><Checkbox label="正確な透過色を保持" {...form.getInputProps("compression.webpExact", { type: "checkbox" })} /></Stack></Accordion.Panel></Accordion.Item>
        </Accordion>
      </Stack>}
    </Card>
  );
}

function LabeledSlider({ label, value, onChange, min, max, step }: { label: string; value: number; onChange: (value: number) => void; min: number; max: number; step?: number }) { return <Box><Group justify="space-between" mb="xs"><Text size="sm" fw={500}>{label}</Text><Badge variant="light" color="gray">{value}</Badge></Group><Slider aria-label={label} value={value} onChange={onChange} min={min} max={max} step={step} /></Box>; }
