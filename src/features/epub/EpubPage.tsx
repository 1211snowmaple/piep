import { useEffect, useMemo, useState } from "react";
import {
  Accordion,
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  ColorInput,
  Drawer,
  Grid,
  Group,
  Modal,
  NumberInput,
  Paper,
  Progress,
  SegmentedControl,
  Select,
  Slider,
  Stack,
  Switch,
  Text,
  Textarea,
  TextInput,
  ThemeIcon,
  Title,
  Tooltip,
} from "@mantine/core";
import { useForm, isNotEmpty, type UseFormReturnType } from "@mantine/form";
import { useDisclosure } from "@mantine/hooks";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { BookOpen, Check, ChevronRight, FileCode2, FolderOpen, ImageDown, Plus, Save, Trash2, X } from "lucide-react";
import { useAppNavigate } from "@/app/router";
import { useWorkspace } from "@/app/WorkspaceContext";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { PageHeader } from "@/components/PageHeader";
import { ProviderMark } from "@/lib/providers";
import { errorMessage, formatBytes, formatNumber } from "@/lib/format";
import { getDemoWork } from "@/mocks/demoData";
import { getDownloads, isTauriRuntime } from "@/services/dbApi";
import { openSingleDialog } from "@/services/dialogApi";
import { createEpubTemplate, deleteEpubTemplate, exportEpubBatch, getTemplateFiles, listEpubTemplates, readTemplateFile, saveTemplateFile } from "@/services/epubApi";
import { onTauriEvent } from "@/services/eventBus";
import { openFilesystemPath } from "@/services/openerApi";
import type { DownloadEntry } from "@/types/library";

interface TemplateInfo { name: string; isBuiltin: boolean; fileCount: number }
interface TemplateFile { filename: string; sizeBytes: number }
interface ExportBatchResult { successCount: number; failedCount: number; failedIds: number[]; outputFiles: string[] }
interface ExportProgress { phase: string; currentTitle: string; currentIndex: number; totalCount: number; message: string }

interface VisualTemplateValues {
  fontFamily: "serif" | "sans";
  fontSize: number;
  lineHeight: number;
  pagePadding: number;
  textColor: string;
  backgroundColor: string;
  accentColor: string;
  coverWidth: number;
  coverRadius: number;
  titleAlign: "left" | "center";
}

const visualTemplateDefaults: VisualTemplateValues = { fontFamily: "serif", fontSize: 16, lineHeight: 1.8, pagePadding: 16, textColor: "#252525", backgroundColor: "#ffffff", accentColor: "#0073bb", coverWidth: 50, coverRadius: 10, titleAlign: "left" };

interface EpubValues {
  templateName: string;
  outputDir: string;
  compression: {
    enabled: boolean;
    maxWidth: number | string;
    maxHeight: number | string;
    outputFormat: string | null;
    jpegQuality: number;
    jpegProgressive: boolean;
    jpegChromaSubsampling: string;
    jpegAutoOptimize: boolean;
    jpegDeringing: boolean;
    jpegSeparateChromaTables: boolean;
    jpegSharpYuv: boolean;
    pngCompression: string;
    pngInterlace: boolean;
    pngStrip: boolean;
    pngOptimizeAlpha: boolean;
    pngBitDepthReduction: boolean;
    pngColorTypeReduction: boolean;
    pngPaletteReduction: boolean;
    pngGrayscaleReduction: boolean;
    pngIdatRecoding: boolean;
    pngFastEvaluation: boolean;
    pngForce: boolean;
    pngFixErrors: boolean;
    webpQuality: number;
    webpLossless: boolean;
    webpMethod: number;
    webpFilterStrength: number;
    webpFilterSharpness: number;
    webpFilterType: number;
    webpSnsStrength: number;
    webpNearLossless: number;
    webpExact: boolean;
    webpUseSharpYuv: boolean;
  };
}

const initialValues: EpubValues = {
  templateName: "default", outputDir: "",
  compression: {
    enabled: true, maxWidth: 2000, maxHeight: 2000, outputFormat: null,
    jpegQuality: 85, jpegProgressive: true, jpegChromaSubsampling: "4:2:0", jpegAutoOptimize: false, jpegDeringing: true, jpegSeparateChromaTables: true, jpegSharpYuv: false,
    pngCompression: "2", pngInterlace: false, pngStrip: true, pngOptimizeAlpha: false, pngBitDepthReduction: true, pngColorTypeReduction: true, pngPaletteReduction: true, pngGrayscaleReduction: true, pngIdatRecoding: true, pngFastEvaluation: false, pngForce: false, pngFixErrors: true,
    webpQuality: 78, webpLossless: false, webpMethod: 4, webpFilterStrength: 60, webpFilterSharpness: 0, webpFilterType: 1, webpSnsStrength: 50, webpNearLossless: 100, webpExact: false, webpUseSharpYuv: false,
  },
};

export default function EpubPage() {
  const navigate = useAppNavigate();
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const { epubQueue, removeFromEpubQueue, clearEpubQueue } = useWorkspace();
  const [progress, setProgress] = useState<ExportProgress | null>(null);
  const [result, setResult] = useState<ExportBatchResult | null>(null);
  const [templatesOpened, templatesDrawer] = useDisclosure(false);
  const [newTemplateOpened, newTemplateModal] = useDisclosure(false);
  const form = useForm<EpubValues>({ initialValues, validate: { templateName: isNotEmpty("テンプレートを選択してください"), outputDir: isNotEmpty("出力先を選択してください") }, validateInputOnBlur: true });
  const templateForm = useForm({ initialValues: { name: "" }, validate: { name: (value) => /^[a-zA-Z0-9_-]+$/.test(value) ? null : "英数字、_、-だけを使用してください" } });
  // One request for the whole queue: a per-work query meant a queue of a few
  // hundred books fired that many IPC round trips on every visit.
  const queueQuery = useQuery({
    queryKey: ["epub-queue-works", epubQueue],
    queryFn: () => runtime
      ? getDownloads(epubQueue)
      : Promise.resolve(epubQueue.map((id) => getDemoWork(id)).filter((work): work is DownloadEntry => Boolean(work))),
    enabled: epubQueue.length > 0,
  });
  const works = useMemo(() => queueQuery.data ?? [], [queueQuery.data]);
  const templates = useQuery({
    queryKey: ["epub-templates"],
    queryFn: () => runtime ? listEpubTemplates<TemplateInfo[]>() : Promise.resolve<TemplateInfo[]>([{ name: "default", isBuiltin: true, fileCount: 7 }, { name: "pixiv", isBuiltin: true, fileCount: 2 }]),
  });
  useEffect(() => {
    let dispose: (() => void) | undefined;
    if (runtime) onTauriEvent<ExportProgress>("epub-export-progress", (event) => setProgress(event.payload)).then((fn) => { dispose = fn; }).catch(() => undefined);
    return () => dispose?.();
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
      setResult(null); setProgress({ phase: "started", currentTitle: "", currentIndex: 0, totalCount: works.length, message: "書き出しを準備しています" });
      if (!runtime) {
        await new Promise((resolve) => window.setTimeout(resolve, 500));
        return { successCount: works.length, failedCount: 0, failedIds: [], outputFiles: works.map((work) => `${values.outputDir}/${work.title}.epub`) };
      }
      const c = values.compression;
      return exportEpubBatch<ExportBatchResult>({
        downloadIds: works.map((work) => work.id), templateName: values.templateName, outputDir: values.outputDir,
        compressOptions: {
          enabled: c.enabled, maxWidth: typeof c.maxWidth === "number" ? c.maxWidth : null, maxHeight: typeof c.maxHeight === "number" ? c.maxHeight : null, outputFormat: c.outputFormat,
          jpegQuality: c.jpegQuality, jpegProgressive: c.jpegProgressive, jpegChromaSubsampling: c.jpegChromaSubsampling, jpegAutoOptimize: c.jpegAutoOptimize, jpegDeringing: c.jpegDeringing, jpegSeparateChromaTables: c.jpegSeparateChromaTables, jpegSharpYuv: c.jpegSharpYuv,
          pngCompression: c.pngCompression, pngInterlace: c.pngInterlace, pngStrip: c.pngStrip, pngOptimizeAlpha: c.pngOptimizeAlpha, pngBitDepthReduction: c.pngBitDepthReduction, pngColorTypeReduction: c.pngColorTypeReduction, pngPaletteReduction: c.pngPaletteReduction, pngGrayscaleReduction: c.pngGrayscaleReduction, pngIdatRecoding: c.pngIdatRecoding, pngFastEvaluation: c.pngFastEvaluation, pngForce: c.pngForce, pngFixErrors: c.pngFixErrors,
          webpQuality: c.webpQuality, webpLossless: c.webpLossless, webpMethod: c.webpMethod, webpFilterStrength: c.webpFilterStrength, webpFilterSharpness: c.webpFilterSharpness, webpFilterType: c.webpFilterType, webpSnsStrength: c.webpSnsStrength, webpNearLossless: c.webpNearLossless, webpExact: c.webpExact, webpUseSharpYuv: c.webpUseSharpYuv,
        },
      });
    },
    onSuccess: (data) => {
      setResult(data);
      setProgress(null);
      const failed = new Set(data.failedIds);
      removeFromEpubQueue(works.filter((work) => !failed.has(work.id)).map((work) => work.id));
      notifications.show({ color: data.failedCount ? "yellow" : "green", title: "EPUB書き出しが完了しました", message: `成功 ${data.successCount} · 失敗 ${data.failedCount}` });
    },
    onError: (error) => { setProgress(null); notifications.show({ color: "red", title: "EPUBを書き出せません", message: errorMessage(error) }); },
  });
  const createTemplateMutation = useMutation({
    mutationFn: (name: string) => runtime ? createEpubTemplate(name) : Promise.resolve(),
    onSuccess: (_, name) => { queryClient.invalidateQueries({ queryKey: ["epub-templates"] }); form.setFieldValue("templateName", name); newTemplateModal.close(); templateForm.reset(); notifications.show({ color: "green", message: "テンプレートを作成しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "テンプレートを作成できません", message: errorMessage(error) }),
  });
  const selectOutput = async () => {
    if (!runtime) return form.setFieldValue("outputDir", "C:/Users/preview/Documents/piep exports");
    const path = await openSingleDialog({ directory: true, title: "EPUBの出力先" });
    if (path) form.setFieldValue("outputDir", path);
  };
  const totalSize = useMemo(() => works.reduce((sum, work) => sum + work.fileSizeBytes, 0), [works]);

  return (
    <div className="page page--contained epub-page">
      <PageHeader title="EPUB Studio" description="選んだ作品を、端末に合わせた高品質な電子書籍へ書き出します。" actions={<Button variant="default" leftSection={<FileCode2 size={15} />} onClick={templatesDrawer.open}>テンプレート管理</Button>} />
      {!epubQueue.length ? <EmptyState icon={BookOpen} title="EPUBキューは空です" description="ライブラリや作品詳細から、書き出したい作品をキューに追加してください。" action={<Button onClick={() => navigate("/library")}>ライブラリを開く</Button>} /> : (
        <form onSubmit={form.onSubmit((values) => exportMutation.mutate(values))}>
          <Grid gap="lg" align="flex-start">
            <Grid.Col span={{ base: 12, lg: 7 }}>
              <Stack gap="lg">
                <Card p="lg">
                  <Group justify="space-between" mb="md"><Box><Title order={3}>書き出す作品</Title><Text size="sm" c="dimmed">{formatNumber(works.length)}件 · 元データ {formatBytes(totalSize)}</Text></Box><Button variant="subtle" color="red" size="xs" onClick={() => modals.openConfirmModal({ title: "キューを空にしますか？", children: <Text size="sm">選択中の{epubQueue.length}件をキューから外します。</Text>, confirmProps: { color: "red" }, labels: { confirm: "空にする", cancel: "キャンセル" }, onConfirm: clearEpubQueue })}>すべて解除</Button></Group>
                  {queueQuery.isLoading && <LoadingState label="作品情報を読み込んでいます" />}
                  <Stack gap="xs">{works.map((work, index) => <Paper key={work.id} p="sm" withBorder><Group wrap="nowrap"><ThemeIcon variant="light" color="gray" size="lg"><Text size="xs" fw={800}>{index + 1}</Text></ThemeIcon><Stack gap={2} flex={1} miw={0}><Group gap="xs" wrap="nowrap"><ProviderMark provider={work.source} compact /><Text size="sm" fw={650} className="line-clamp-1">{work.title}</Text></Group><Text size="xs" c="dimmed">{work.authorName} · {formatNumber(work.textLength)}字 · {work.assetCount}画像</Text></Stack><Tooltip label="キューから外す"><ActionIcon variant="subtle" color="red" aria-label={`${work.title}をキューから外す`} onClick={() => removeFromEpubQueue(work.id)}><X size={16} /></ActionIcon></Tooltip></Group></Paper>)}</Stack>
                </Card>
                <CompressionSettings form={form} />
              </Stack>
            </Grid.Col>
            <Grid.Col span={{ base: 12, lg: 5 }}>
              <Card p="lg" className="epub-export-card">
                <Stack gap="lg">
                  <Box><Title order={3}>出力設定</Title><Text size="sm" c="dimmed">EPUB 3形式で作品ごとに出力します。</Text></Box>
                  <Select label="テンプレート" data={(templates.data ?? []).map((item) => ({ value: item.name, label: `${item.name}${item.isBuiltin ? " · built-in" : ""}` }))} {...form.getInputProps("templateName")} rightSection={<ChevronRight size={14} />} />
                  {/* The icon sits inside the input, so its click also bubbles
                      to the field handler and opened two pickers in a row. */}
                  <TextInput label="出力先フォルダー" placeholder="フォルダーを選択" readOnly {...form.getInputProps("outputDir")} rightSection={<ActionIcon variant="subtle" aria-label="出力先を選択" onClick={(event) => { event.stopPropagation(); selectOutput(); }}><FolderOpen size={16} /></ActionIcon>} onClick={selectOutput} />
                  <Alert color="blue" icon={<ImageDown size={17} />}>{form.values.compression.enabled ? `画像を最大 ${form.values.compression.maxWidth} × ${form.values.compression.maxHeight}px に最適化します。` : "画像は元の品質のまま収録します。"}</Alert>
                  {progress && <Stack gap={6}><Group justify="space-between"><Text size="sm" className="line-clamp-1">{progress.currentTitle || progress.message}</Text><Text size="xs" c="dimmed">{progress.currentIndex}/{progress.totalCount}</Text></Group><Progress value={progress.totalCount ? progress.currentIndex / progress.totalCount * 100 : 5} animated /><Text size="xs" c="dimmed">{progress.message}</Text></Stack>}
                  {result && <Alert color={result.failedCount ? "yellow" : "green"} icon={<Check size={17} />} title="書き出し完了"><Stack gap="xs"><Text size="sm">成功 {result.successCount}件 / 失敗 {result.failedCount}件</Text>{form.values.outputDir && <Button size="xs" variant="light" w="fit-content" leftSection={<FolderOpen size={13} />} disabled={!runtime} onClick={() => openFilesystemPath(form.values.outputDir)}>出力先を開く</Button>}</Stack></Alert>}
                  <Button type="submit" size="lg" leftSection={<BookOpen size={17} />} loading={exportMutation.isPending} disabled={!works.length}>{works.length}冊を書き出す</Button>
                </Stack>
              </Card>
            </Grid.Col>
          </Grid>
        </form>
      )}

      <Drawer opened={templatesOpened} onClose={templatesDrawer.close} position="right" size={560} title="EPUBテンプレート">
        <Stack gap="md"><Group justify="space-between"><Text size="sm" c="dimmed">組版・表紙・目次をカスタマイズできます。</Text><Button size="xs" leftSection={<Plus size={14} />} onClick={newTemplateModal.open}>新規作成</Button></Group>{templates.isLoading ? <LoadingState /> : templates.error ? <ErrorState error={templates.error} retry={() => templates.refetch()} /> : (templates.data ?? []).map((template) => <TemplateEditor key={template.name} template={template} runtime={runtime} onChanged={() => queryClient.invalidateQueries({ queryKey: ["epub-templates"] })} />)}</Stack>
      </Drawer>
      <Modal opened={newTemplateOpened} onClose={newTemplateModal.close} title="テンプレートを作成">
        <form onSubmit={templateForm.onSubmit(({ name }) => createTemplateMutation.mutate(name))}><Stack><TextInput label="テンプレート名" description="defaultテンプレートを複製して作成します" placeholder="my-template" {...templateForm.getInputProps("name")} /><Button type="submit" loading={createTemplateMutation.isPending}>作成</Button></Stack></form>
      </Modal>
    </div>
  );
}

function CompressionSettings({ form }: { form: UseFormReturnType<EpubValues, EpubValues, any> }) {
  const enabled = form.values.compression.enabled;
  return (
    <Card p="lg"><Group justify="space-between"><Box><Title order={3}>画像最適化</Title><Text size="sm" c="dimmed">品質とファイルサイズのバランス</Text></Box><Switch checked={enabled} onChange={(event) => form.setFieldValue("compression.enabled", event.currentTarget.checked)} aria-label="画像最適化" /></Group>
      {enabled && <Stack gap="lg" mt="lg"><Grid><Grid.Col span={6}><NumberInput label="最大幅" suffix=" px" min={320} max={8000} {...form.getInputProps("compression.maxWidth")} /></Grid.Col><Grid.Col span={6}><NumberInput label="最大高さ" suffix=" px" min={320} max={8000} {...form.getInputProps("compression.maxHeight")} /></Grid.Col></Grid><Select label="出力形式" description="自動は元画像の形式を保ちます" data={[{ value: "auto", label: "自動" }, { value: "jpeg", label: "JPEG" }, { value: "png", label: "PNG" }, { value: "webp", label: "WebP" }]} value={form.values.compression.outputFormat ?? "auto"} onChange={(value) => form.setFieldValue("compression.outputFormat", value === "auto" ? null : value)} />
        <Accordion variant="separated">
          <Accordion.Item value="jpeg"><Accordion.Control>JPEG詳細</Accordion.Control><Accordion.Panel><Stack><LabeledSlider label="品質" value={form.values.compression.jpegQuality} onChange={(value) => form.setFieldValue("compression.jpegQuality", value)} min={40} max={100} /><Select label="クロマサブサンプリング" data={["4:2:0", "4:2:2", "4:4:4"]} {...form.getInputProps("compression.jpegChromaSubsampling")} /><Checkbox label="プログレッシブ" {...form.getInputProps("compression.jpegProgressive", { type: "checkbox" })} /><Checkbox label="リンギング低減" {...form.getInputProps("compression.jpegDeringing", { type: "checkbox" })} /><Checkbox label="自動最適化（高品質・低速）" {...form.getInputProps("compression.jpegAutoOptimize", { type: "checkbox" })} /></Stack></Accordion.Panel></Accordion.Item>
          <Accordion.Item value="png"><Accordion.Control>PNG詳細</Accordion.Control><Accordion.Panel><Stack><SegmentedControl fullWidth data={[{ value: "1", label: "高速" }, { value: "2", label: "標準" }, { value: "4", label: "高圧縮" }]} {...form.getInputProps("compression.pngCompression")} /><Checkbox label="不要なメタデータを削除" {...form.getInputProps("compression.pngStrip", { type: "checkbox" })} /><Checkbox label="ビット深度を最適化" {...form.getInputProps("compression.pngBitDepthReduction", { type: "checkbox" })} /><Checkbox label="カラーパレットを最適化" {...form.getInputProps("compression.pngPaletteReduction", { type: "checkbox" })} /></Stack></Accordion.Panel></Accordion.Item>
          <Accordion.Item value="webp"><Accordion.Control>WebP詳細</Accordion.Control><Accordion.Panel><Stack><LabeledSlider label="品質" value={form.values.compression.webpQuality} onChange={(value) => form.setFieldValue("compression.webpQuality", value)} min={30} max={100} /><LabeledSlider label="圧縮方法" value={form.values.compression.webpMethod} onChange={(value) => form.setFieldValue("compression.webpMethod", value)} min={0} max={6} /><Checkbox label="ロスレス" {...form.getInputProps("compression.webpLossless", { type: "checkbox" })} /><Checkbox label="正確な透過色を保持" {...form.getInputProps("compression.webpExact", { type: "checkbox" })} /></Stack></Accordion.Panel></Accordion.Item>
        </Accordion>
      </Stack>}
    </Card>
  );
}

function LabeledSlider({ label, value, onChange, min, max, step }: { label: string; value: number; onChange: (value: number) => void; min: number; max: number; step?: number }) { return <Box><Group justify="space-between" mb="xs"><Text size="sm" fw={500}>{label}</Text><Badge variant="light" color="gray">{value}</Badge></Group><Slider value={value} onChange={onChange} min={min} max={max} step={step} /></Box>; }

function TemplateEditor({ template, runtime, onChanged }: { template: TemplateInfo; runtime: boolean; onChanged: () => void }) {
  const [opened, setOpened] = useState(false);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [editorMode, setEditorMode] = useState<"visual" | "code">("visual");
  const visualForm = useForm<VisualTemplateValues>({ initialValues: visualTemplateDefaults });
  const files = useQuery({ queryKey: ["template-files", template.name], queryFn: () => runtime ? getTemplateFiles<TemplateFile[]>(template.name) : Promise.resolve<TemplateFile[]>([{ filename: "style.css.j2", sizeBytes: 840 }, { filename: "page_wrapper.xhtml.j2", sizeBytes: 1200 }]), enabled: opened });
  const loadFile = async (filename: string) => {
    const next = runtime ? await readTemplateFile(template.name, filename) : "/* Preview template */\nbody { line-height: 1.9; }";
    setSelectedFile(filename);
    setContent(next);
    if (filename.endsWith(".css.j2")) visualForm.setValues(readVisualSettings(next));
  };
  useEffect(() => {
    if (!opened || selectedFile || !files.data?.length) return;
    const style = files.data.find((file) => file.filename === "style.css.j2") ?? files.data.find((file) => file.filename.endsWith(".css.j2")) ?? files.data[0];
    loadFile(style.filename).catch((error) => notifications.show({ color: "red", message: errorMessage(error) }));
  }, [files.data, opened, selectedFile]);
  const save = async () => { if (!selectedFile || !runtime) return; try { await saveTemplateFile(template.name, selectedFile, content); notifications.show({ color: "green", message: "テンプレートを保存しました" }); } catch (error) { notifications.show({ color: "red", message: errorMessage(error) }); } };
  const applyVisual = (values: VisualTemplateValues) => {
    setContent((current) => writeVisualSettings(current, values));
    notifications.show({ color: "blue", message: "見た目の設定をコードへ反映しました。保存するとEPUBに使用されます。" });
  };
  const remove = () => modals.openConfirmModal({ title: "テンプレートを削除しますか？", children: <Text size="sm">{template.name}を削除します。</Text>, confirmProps: { color: "red" }, labels: { confirm: "削除", cancel: "キャンセル" }, onConfirm: async () => { await deleteEpubTemplate(template.name); onChanged(); } });
  const visualAvailable = Boolean(selectedFile?.endsWith(".css.j2"));
  return <Card p="md">
    <Group justify="space-between"><Box><Group gap="xs"><Text fw={700}>{template.name}</Text>{template.isBuiltin && <Badge size="xs" variant="light">built-in</Badge>}</Group><Text size="xs" c="dimmed">{template.fileCount}ファイル</Text></Box><Group gap="xs"><Button size="xs" variant="light" onClick={() => setOpened(!opened)}>{opened ? "閉じる" : "編集"}</Button>{!template.isBuiltin && <Tooltip label="削除"><ActionIcon size="sm" color="red" variant="subtle" aria-label={`${template.name}を削除`} onClick={remove}><Trash2 size={14} /></ActionIcon></Tooltip>}</Group></Group>
    {opened && <Stack mt="md" gap="md">
      <Group justify="space-between"><SegmentedControl value={editorMode} onChange={(value) => setEditorMode(value as "visual" | "code")} data={[{ value: "visual", label: "見た目で編集", disabled: !visualAvailable }, { value: "code", label: "コード編集" }]} /><Group gap="xs"><Button size="xs" variant="default" leftSection={<Save size={13} />} disabled={!runtime || !selectedFile} onClick={save}>テンプレートを保存</Button></Group></Group>
      {editorMode === "visual" && visualAvailable ? <form onSubmit={visualForm.onSubmit(applyVisual)}><Grid>
        <Grid.Col span={{ base: 12, sm: 7 }}><Stack gap="md"><Group grow><Select label="本文書体" data={[{ value: "serif", label: "明朝体" }, { value: "sans", label: "ゴシック体" }]} {...visualForm.getInputProps("fontFamily")} /><Select label="タイトル位置" data={[{ value: "left", label: "左揃え" }, { value: "center", label: "中央" }]} {...visualForm.getInputProps("titleAlign")} /></Group><Group grow><NumberInput label="文字サイズ" suffix=" px" min={12} max={28} {...visualForm.getInputProps("fontSize")} /><NumberInput label="ページ余白" suffix=" px" min={0} max={64} {...visualForm.getInputProps("pagePadding")} /></Group><LabeledSlider label="行間" value={visualForm.values.lineHeight} onChange={(value) => visualForm.setFieldValue("lineHeight", value)} min={1.2} max={2.6} step={0.1} /><Group grow><ColorInput label="本文色" {...visualForm.getInputProps("textColor")} /><ColorInput label="背景色" {...visualForm.getInputProps("backgroundColor")} /><ColorInput label="タグ色" {...visualForm.getInputProps("accentColor")} /></Group><LabeledSlider label="表紙の横幅 (%)" value={visualForm.values.coverWidth} onChange={(value) => visualForm.setFieldValue("coverWidth", value)} min={25} max={100} /><LabeledSlider label="表紙の角丸 (px)" value={visualForm.values.coverRadius} onChange={(value) => visualForm.setFieldValue("coverRadius", value)} min={0} max={32} /><Button type="submit" w="fit-content">見た目をコードへ反映</Button></Stack></Grid.Col>
        <Grid.Col span={{ base: 12, sm: 5 }}><Paper withBorder p="lg" className="epub-template-preview" style={{ fontFamily: visualForm.values.fontFamily === "serif" ? '"Yu Mincho", serif' : '"Yu Gothic UI", sans-serif', fontSize: visualForm.values.fontSize, lineHeight: visualForm.values.lineHeight, padding: visualForm.values.pagePadding, color: visualForm.values.textColor, background: visualForm.values.backgroundColor }}><Text component="h3" ta={visualForm.values.titleAlign} fw={700} m={0}>作品タイトル</Text><Text size="sm" mt="xs">作者名</Text><Box className="epub-template-preview__cover" my="md" mx="auto" style={{ width: `${visualForm.values.coverWidth}%`, borderRadius: visualForm.values.coverRadius }} /><Text>これは本文のプレビューです。文字サイズや行間、余白を確認しながら調整できます。</Text><Text mt="sm" style={{ color: visualForm.values.accentColor }}>#創作　#小説</Text></Paper></Grid.Col>
      </Grid></form> : <Grid><Grid.Col span={4}><Stack gap={4}>{files.data?.map((file) => <Button key={file.filename} variant={selectedFile === file.filename ? "light" : "subtle"} color="gray" size="compact-xs" justify="flex-start" onClick={() => loadFile(file.filename)}>{file.filename}</Button>)}</Stack></Grid.Col><Grid.Col span={8}>{selectedFile ? <Textarea autosize minRows={16} maxRows={28} value={content} onChange={(event) => setContent(event.currentTarget.value)} styles={{ input: { fontFamily: "var(--mantine-font-family-monospace)", fontSize: 12 } }} aria-label={`${selectedFile}の内容`} /> : <Text size="xs" c="dimmed">ファイルを選択してください</Text>}</Grid.Col></Grid>}
    </Stack>}
  </Card>;
}

export function readVisualSettings(content: string): VisualTemplateValues {
  const managed = content.match(/\/\* piep-visual:start \*\/([\s\S]*?)\/\* piep-visual:end \*\//)?.[1] ?? content;
  const body = managed.match(/(?:^|\})\s*body\s*\{([^}]*)\}/s)?.[1] ?? managed;
  const cover = managed.match(/\.cover-container\s+img[^\{]*\{([^}]*)\}/s)?.[1] ?? managed;
  const heading = managed.match(/(?:^|\})\s*h3\s*\{([^}]*)\}/s)?.[1] ?? managed;
  const number = (block: string, property: string, fallback: number) => Number(block.match(new RegExp(`(?:^|[;{\\s])${property}\\s*:\\s*([0-9.]+)`))?.[1] ?? fallback);
  const color = (block: string, property: string, fallback: string) => block.match(new RegExp(`(?:^|[;{\\s])${property}\\s*:\\s*(#[0-9a-fA-F]{3,8})`))?.[1] ?? fallback;
  return {
    fontFamily: /font-family\s*:\s*[^;]*(sans-serif|Yu Gothic)/i.test(body) ? "sans" : "serif",
    fontSize: number(body, "font-size", visualTemplateDefaults.fontSize),
    lineHeight: number(body, "line-height", visualTemplateDefaults.lineHeight),
    pagePadding: number(body, "padding", visualTemplateDefaults.pagePadding),
    textColor: color(body, "color", visualTemplateDefaults.textColor),
    backgroundColor: color(body, "background-color", visualTemplateDefaults.backgroundColor),
    accentColor: managed.match(/--tag-color\s*:\s*(#[0-9a-fA-F]{3,8})/)?.[1] ?? visualTemplateDefaults.accentColor,
    coverWidth: number(cover, "max-width", visualTemplateDefaults.coverWidth),
    coverRadius: number(cover, "border-radius", visualTemplateDefaults.coverRadius),
    titleAlign: /text-align\s*:\s*center/i.test(heading) ? "center" : "left",
  };
}

export function writeVisualSettings(content: string, values: VisualTemplateValues): string {
  const clean = content.replace(/\n?\/\* piep-visual:start \*\/[\s\S]*?\/\* piep-visual:end \*\/\n?/g, "\n").trimEnd();
  const font = values.fontFamily === "serif" ? '"Yu Mincho", "Noto Serif JP", serif' : '"Yu Gothic", "Noto Sans JP", sans-serif';
  return `${clean}\n\n/* piep-visual:start */\n:root { --tag-color: ${values.accentColor}; }\nbody { font-family: ${font}; font-size: ${values.fontSize}px; line-height: ${values.lineHeight}; padding: ${values.pagePadding}px; color: ${values.textColor}; background-color: ${values.backgroundColor}; }\nh3 { text-align: ${values.titleAlign}; }\n.cover-container img, .cover-cell img { max-width: ${values.coverWidth}%; border-radius: ${values.coverRadius}px; }\n/* piep-visual:end */\n`;
}
