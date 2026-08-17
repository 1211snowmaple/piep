import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Card,
  Checkbox,
  ColorInput,
  Grid,
  Group,
  Modal,
  NumberInput,
  Paper,
  ScrollArea,
  SegmentedControl,
  Select,
  Slider,
  Stack,
  Switch,
  Table,
  Tabs,
  Text,
  Textarea,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { useForm } from "@mantine/form";
import { useDisclosure } from "@mantine/hooks";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Icons, IconSize } from "@/lib/icons";
import { useAppNavigate, useAppSearchParams } from "@/app/router";
import { EmptyState, ErrorState, LoadingState } from "@/components/AsyncState";
import { PageHeader } from "@/components/PageHeader";
import { ProviderMark } from "@/lib/providers";
import { errorMessage } from "@/lib/format";
import { getDownloads, isTauriRuntime, searchDownloadsV2 } from "@/services/dbApi";
import {
  createEpubTemplate,
  deleteEpubTemplate,
  getTemplateFiles,
  listEpubTemplates,
  listTemplateFileKinds,
  previewEpubTemplate,
  readTemplateFile,
  renameEpubTemplate,
  resetTemplateFile,
  saveTemplateFile,
  saveTemplateSettings,
} from "@/services/epubApi";
import type { DownloadEntry } from "@/types/library";
import type {
  DataField,
  InfoField,
  TemplateFile,
  TemplateFileKind,
  TemplateInfo,
  TemplatePreview,
  TemplateSettings,
} from "@/types/epub";
import {
  readVisualSettings,
  visualTemplateDefaults,
  writeVisualSettings,
  type VisualTemplateValues,
} from "./visualTemplate";
import { demoFileContent, demoFileKinds, demoFiles, demoPreview, demoTemplates } from "./templateStudioDemo";

const STYLE_FILE = "style.css.j2";

type PreviewTarget = "info" | "cover" | "page" | "nav" | "opf" | "ncx";

const PREVIEW_TARGETS: { value: PreviewTarget; label: string }[] = [
  { value: "info", label: "作品情報" },
  { value: "cover", label: "表紙" },
  { value: "page", label: "本文" },
  { value: "nav", label: "目次" },
  { value: "opf", label: "書誌" },
];

export default function TemplateStudioPage() {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const [params, setParams] = useAppSearchParams();
  const [newOpened, newModal] = useDisclosure(false);

  const templates = useQuery({
    queryKey: ["epub-templates"],
    queryFn: () => (runtime ? listEpubTemplates() : Promise.resolve(demoTemplates)),
  });
  const list = useMemo(() => templates.data ?? [], [templates.data]);
  const selectedName = params.get("template") ?? list[0]?.name ?? "";
  const selected = list.find((template) => template.name === selectedName) ?? list[0];
  const select = useCallback((name: string) => {
    const next = new URLSearchParams(params);
    next.set("template", name);
    setParams(next, { replace: true });
  }, [params, setParams]);

  // The work the preview renders. Kept in the address so a reload comes back to
  // the same comparison rather than to an arbitrary book.
  const sampleParam = Number.parseInt(params.get("sample") ?? "", 10);
  const sampleId = Number.isSafeInteger(sampleParam) ? sampleParam : null;
  const setSample = (id: number | null) => {
    const next = new URLSearchParams(params);
    if (id === null) next.delete("sample");
    else next.set("sample", String(id));
    setParams(next, { replace: true });
  };

  const createMutation = useMutation({
    mutationFn: ({ name, base }: { name: string; base: string }) =>
      runtime ? createEpubTemplate(name, base) : Promise.resolve(),
    onSuccess: (_, { name }) => {
      queryClient.invalidateQueries({ queryKey: ["epub-templates"] });
      select(name);
      newModal.close();
      notifications.show({ color: "green", message: `テンプレート「${name}」を作成しました` });
    },
    onError: (error) => notifications.show({ color: "red", title: "テンプレートを作成できません", message: errorMessage(error) }),
  });

  return (
    <div className="page template-studio">
      <PageHeader
        title="テンプレートスタジオ"
        description="EPUBの見た目と構成をつくる場所です。作品の情報をどこに、どう並べるかを決められます。"
        actions={
          <Group gap="xs">
            <Button variant="default" leftSection={<Icons.epub size={IconSize.menu} />} onClick={() => navigate("/epub")}>
              書き出しへ
            </Button>
            <Button leftSection={<Icons.add size={IconSize.menu} />} onClick={newModal.open}>
              テンプレートを作る
            </Button>
          </Group>
        }
      />

      {templates.isLoading ? (
        <LoadingState label="テンプレートを読み込んでいます" />
      ) : templates.error ? (
        <ErrorState error={templates.error} retry={() => templates.refetch()} />
      ) : !selected ? (
        <EmptyState icon={Icons.epubTemplate} title="テンプレートがありません" description="標準テンプレートを複製して作りはじめられます。" action={<Button onClick={newModal.open}>テンプレートを作る</Button>} />
      ) : (
        <Grid gap="lg" align="flex-start">
          <Grid.Col span={{ base: 12, lg: 3 }}>
            <TemplateList templates={list} selected={selected.name} onSelect={select} runtime={runtime} onChanged={() => queryClient.invalidateQueries({ queryKey: ["epub-templates"] })} onRenamed={select} />
          </Grid.Col>
          <Grid.Col span={{ base: 12, lg: 9 }}>
            <TemplateWorkspace
              key={selected.name}
              template={selected}
              runtime={runtime}
              sampleId={sampleId}
              onSampleChange={setSample}
              onChanged={() => queryClient.invalidateQueries({ queryKey: ["epub-templates"] })}
            />
          </Grid.Col>
        </Grid>
      )}

      <NewTemplateModal
        opened={newOpened}
        onClose={newModal.close}
        templates={list}
        pending={createMutation.isPending}
        onCreate={(name, base) => createMutation.mutate({ name, base })}
      />
    </div>
  );
}

// ============================================================
// テンプレート一覧
// ============================================================

function TemplateList({ templates, selected, onSelect, runtime, onChanged, onRenamed }: {
  templates: TemplateInfo[];
  selected: string;
  onSelect: (name: string) => void;
  runtime: boolean;
  onChanged: () => void;
  onRenamed: (name: string) => void;
}) {
  const remove = (template: TemplateInfo) => modals.openConfirmModal({
    title: "テンプレートを削除しますか？",
    children: <Text size="sm">「{template.settings.label || template.name}」を削除します。この操作は元に戻せません。</Text>,
    confirmProps: { color: "red" },
    labels: { confirm: "削除", cancel: "キャンセル" },
    onConfirm: async () => {
      try {
        await deleteEpubTemplate(template.name);
        onChanged();
        notifications.show({ color: "green", message: "テンプレートを削除しました" });
      } catch (error) {
        notifications.show({ color: "red", message: errorMessage(error) });
      }
    },
  });

  const rename = (template: TemplateInfo) => modals.open({
    title: "テンプレート名を変更",
    children: <RenameForm current={template.name} onSubmit={async (next) => {
      try {
        await renameEpubTemplate(template.name, next);
        modals.closeAll();
        onChanged();
        onRenamed(next);
      } catch (error) {
        notifications.show({ color: "red", message: errorMessage(error) });
      }
    }} />,
  });

  return (
    <Stack gap="xs">
      {templates.map((template) => {
        const active = template.name === selected;
        return (
          <Card key={template.name} p="sm" withBorder className="template-studio__item" data-active={active || undefined} onClick={() => onSelect(template.name)}>
            <Group justify="space-between" wrap="nowrap" align="flex-start">
              <Box miw={0}>
                <Group gap={6} wrap="nowrap">
                  <Text fw={700} size="sm" className="line-clamp-1">{template.settings.label || template.name}</Text>
                  {template.isBuiltin && <Badge size="xs" variant="light">標準</Badge>}
                </Group>
                <Text size="xs" c="dimmed" className="line-clamp-2">{template.settings.description || template.name}</Text>
                <Group gap={4} mt={6}>
                  {template.settings.appliesTo.map((source) => <ProviderMark key={source} provider={source} compact />)}
                  <Text size="xs" c="dimmed">{template.fileCount}ファイル</Text>
                </Group>
              </Box>
              {!template.isBuiltin && runtime && (
                <Group gap={2} wrap="nowrap">
                  <Tooltip label="名前を変更"><ActionIcon size="sm" variant="subtle" color="gray" aria-label={`${template.name}の名前を変更`} onClick={(event) => { event.stopPropagation(); rename(template); }}><Icons.edit size={IconSize.menu} /></ActionIcon></Tooltip>
                  <Tooltip label="削除"><ActionIcon size="sm" variant="subtle" color="red" aria-label={`${template.name}を削除`} onClick={(event) => { event.stopPropagation(); remove(template); }}><Icons.delete size={IconSize.menu} /></ActionIcon></Tooltip>
                </Group>
              )}
            </Group>
          </Card>
        );
      })}
    </Stack>
  );
}

function RenameForm({ current, onSubmit }: { current: string; onSubmit: (next: string) => void }) {
  const form = useForm({ initialValues: { name: current }, validate: { name: (value) => (/^[a-zA-Z0-9_-]+$/.test(value) ? null : "英数字、_、-だけを使用してください") } });
  return (
    <form onSubmit={form.onSubmit(({ name }) => onSubmit(name))}>
      <Stack><TextInput label="テンプレート名" {...form.getInputProps("name")} /><Button type="submit">変更する</Button></Stack>
    </form>
  );
}

function NewTemplateModal({ opened, onClose, templates, pending, onCreate }: {
  opened: boolean;
  onClose: () => void;
  templates: TemplateInfo[];
  pending: boolean;
  onCreate: (name: string, base: string) => void;
}) {
  const form = useForm({
    initialValues: { name: "", base: "default" },
    validate: { name: (value) => (/^[a-zA-Z0-9_-]+$/.test(value) ? null : "英数字、_、-だけを使用してください") },
  });
  return (
    <Modal opened={opened} onClose={onClose} title="テンプレートを作る">
      <form onSubmit={form.onSubmit(({ name, base }) => onCreate(name, base))}>
        <Stack>
          <TextInput label="テンプレート名" description="ファイル名に使われます" placeholder="my-template" {...form.getInputProps("name")} />
          <Select label="複製元" description="選んだテンプレートの見た目と構成をそのまま引き継ぎます" data={templates.map((template) => ({ value: template.name, label: `${template.settings.label || template.name}${template.isBuiltin ? "（標準）" : ""}` }))} {...form.getInputProps("base")} />
          <Button type="submit" loading={pending}>作成</Button>
        </Stack>
      </form>
    </Modal>
  );
}

// ============================================================
// 編集の中身
// ============================================================

function TemplateWorkspace({ template, runtime, sampleId, onSampleChange, onChanged }: {
  template: TemplateInfo;
  runtime: boolean;
  sampleId: number | null;
  onSampleChange: (id: number | null) => void;
  onChanged: () => void;
}) {
  const [tab, setTab] = useState<string | null>("structure");
  const [previewTarget, setPreviewTarget] = useState<PreviewTarget>("info");
  const readOnly = template.isBuiltin || !runtime;

  const preview = useQuery({
    queryKey: ["epub-template-preview", template.name, sampleId],
    queryFn: () => (runtime ? previewEpubTemplate(template.name, sampleId) : Promise.resolve(demoPreview)),
    // Rendering a book is cheap but not free; do not refetch on every focus.
    staleTime: 5_000,
  });

  return (
    <Grid gap="lg" align="flex-start">
      <Grid.Col span={{ base: 12, xl: 7 }}>
        <Card p="lg">
          {readOnly && (
            <Alert color="piep" mb="md" icon={<Icons.info size={IconSize.action} />}>
              {template.isBuiltin
                ? "標準テンプレートは書き換えられません。複製すると、ここでの操作がすべて使えるようになります。"
                : "ブラウザプレビューでは保存できません。"}
            </Alert>
          )}
          <Tabs value={tab} onChange={setTab}>
            <Tabs.List mb="md">
              <Tabs.Tab value="structure" leftSection={<Icons.epubStructure size={IconSize.menu} />}>構成</Tabs.Tab>
              <Tabs.Tab value="visual" leftSection={<Icons.appearance size={IconSize.menu} />}>見た目</Tabs.Tab>
              <Tabs.Tab value="code" leftSection={<Icons.epubTemplate size={IconSize.menu} />}>コード</Tabs.Tab>
              <Tabs.Tab value="data" leftSection={<Icons.epubDataField size={IconSize.menu} />}>差し込める項目</Tabs.Tab>
            </Tabs.List>
            <Tabs.Panel value="structure"><StructureEditor template={template} readOnly={readOnly} onSaved={onChanged} /></Tabs.Panel>
            <Tabs.Panel value="visual"><VisualEditor template={template} readOnly={readOnly} onSaved={() => preview.refetch()} /></Tabs.Panel>
            <Tabs.Panel value="code"><CodeEditor template={template} readOnly={readOnly} onSaved={() => { preview.refetch(); onChanged(); }} /></Tabs.Panel>
            <Tabs.Panel value="data"><DataDictionary fields={preview.data?.fields ?? []} loading={preview.isLoading} /></Tabs.Panel>
          </Tabs>
        </Card>
      </Grid.Col>
      <Grid.Col span={{ base: 12, xl: 5 }}>
        <PreviewPanel
          preview={preview.data}
          loading={preview.isFetching}
          error={preview.error}
          target={previewTarget}
          onTarget={setPreviewTarget}
          sampleId={sampleId}
          onSampleChange={onSampleChange}
          runtime={runtime}
          onRefresh={() => preview.refetch()}
        />
      </Grid.Col>
    </Grid>
  );
}

// ============================================================
// 構成
// ============================================================

function StructureEditor({ template, readOnly, onSaved }: { template: TemplateInfo; readOnly: boolean; onSaved: () => void }) {
  const [settings, setSettings] = useState<TemplateSettings>(template.settings);
  const [dirty, setDirty] = useState(false);
  const update = (patch: Partial<TemplateSettings>) => { setSettings((current) => ({ ...current, ...patch })); setDirty(true); };
  const updateField = (index: number, patch: Partial<InfoField>) => {
    setSettings((current) => ({ ...current, infoFields: current.infoFields.map((field, position) => (position === index ? { ...field, ...patch } : field)) }));
    setDirty(true);
  };
  const move = (index: number, delta: number) => {
    setSettings((current) => {
      const next = [...current.infoFields];
      const target = index + delta;
      if (target < 0 || target >= next.length) return current;
      [next[index], next[target]] = [next[target], next[index]];
      return { ...current, infoFields: next };
    });
    setDirty(true);
  };

  const save = useMutation({
    mutationFn: () => saveTemplateSettings(template.name, settings),
    onSuccess: (saved) => { setSettings(saved); setDirty(false); onSaved(); notifications.show({ color: "green", message: "構成を保存しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "構成を保存できません", message: errorMessage(error) }),
  });

  return (
    <Stack gap="lg">
      <Group grow align="flex-start">
        <TextInput label="表示名" value={settings.label} disabled={readOnly} onChange={(event) => update({ label: event.currentTarget.value })} />
        <TextInput label="説明" value={settings.description} disabled={readOnly} onChange={(event) => update({ description: event.currentTarget.value })} />
      </Group>

      <Box>
        <Text size="sm" fw={600} mb={4}>自動で使う取得元</Text>
        <Text size="xs" c="dimmed" mb="xs">書き出しでテンプレートを「自動」にしたとき、ここで選んだ取得元の作品にこのテンプレートが使われます。</Text>
        <Group gap="md">
          {(["pixiv", "fanbox"] as const).map((source) => (
            <Checkbox
              key={source}
              label={source === "pixiv" ? "pixiv の小説" : "FANBOX の投稿"}
              disabled={readOnly}
              checked={settings.appliesTo.includes(source)}
              onChange={(event) => update({ appliesTo: event.currentTarget.checked ? [...settings.appliesTo, source] : settings.appliesTo.filter((value) => value !== source) })}
            />
          ))}
        </Group>
      </Box>

      <Box>
        <Text size="sm" fw={600} mb="xs">本に入れるページ</Text>
        <Stack gap="xs">
          <Switch label="表紙のページを作る" disabled={readOnly} checked={settings.includeCoverPage} onChange={(event) => update({ includeCoverPage: event.currentTarget.checked })} />
          <Switch label="表紙を読み進む順に含める" description="外すと本文から始まり、表紙は目次からだけ開けます" disabled={readOnly || !settings.includeCoverPage} checked={settings.coverInReadingOrder} onChange={(event) => update({ coverInReadingOrder: event.currentTarget.checked })} />
          <Switch label="作品情報のページを作る" disabled={readOnly} checked={settings.includeInfoPage} onChange={(event) => update({ includeInfoPage: event.currentTarget.checked })} />
          <Switch label="章を目次に並べる" disabled={readOnly} checked={settings.chapterToc} onChange={(event) => update({ chapterToc: event.currentTarget.checked })} />
          <Switch label="EPUB 2 互換の目次も入れる" description="Send to Kindle など、古い経路の取り込みはこちらを読みます。外す理由がなければ入れたままに" disabled={readOnly} checked={settings.includeNcx} onChange={(event) => update({ includeNcx: event.currentTarget.checked })} />
        </Stack>
      </Box>

      <Group grow>
        <Select label="綴じ方向" description="縦書きなら右から左" data={[{ value: "ltr", label: "左から右" }, { value: "rtl", label: "右から左" }]} value={settings.pageProgression} disabled={readOnly} onChange={(value) => update({ pageProgression: (value as "ltr" | "rtl") ?? "ltr" })} />
        <TextInput label="言語" description="BCP 47 の言語タグ" value={settings.language} disabled={readOnly} onChange={(event) => update({ language: event.currentTarget.value })} />
      </Group>

      <Box>
        <Text size="sm" fw={600}>作品情報に並べる項目</Text>
        <Text size="xs" c="dimmed" mb="xs">上から順に並びます。値のない項目は自動的に省かれます。</Text>
        <Stack gap={2}>
          {settings.infoFields.map((field, index) => (
            <Paper key={field.key} p={6} withBorder>
              <Group gap="xs" wrap="nowrap">
                <Checkbox aria-label={`${field.label}を表示`} disabled={readOnly} checked={field.enabled} onChange={(event) => updateField(index, { enabled: event.currentTarget.checked })} />
                <TextInput size="xs" flex={1} aria-label={`${field.key}の見出し`} value={field.label} disabled={readOnly || !field.enabled} onChange={(event) => updateField(index, { label: event.currentTarget.value })} />
                <Text size="xs" c="dimmed" w={110} className="line-clamp-1">{field.key}</Text>
                <ActionIcon size="sm" variant="subtle" color="gray" aria-label={`${field.label}を上へ`} disabled={readOnly || index === 0} onClick={() => move(index, -1)}><Icons.up size={IconSize.menu} /></ActionIcon>
                <ActionIcon size="sm" variant="subtle" color="gray" aria-label={`${field.label}を下へ`} disabled={readOnly || index === settings.infoFields.length - 1} onClick={() => move(index, 1)}><Icons.down size={IconSize.menu} /></ActionIcon>
              </Group>
            </Paper>
          ))}
        </Stack>
      </Box>

      <Box>
        <Text size="sm" fw={600} mb="xs">見出しの文言</Text>
        <Group grow>
          {["TOC_TITLE", "COVER_TITLE", "INFO_TITLE", "BODY_MATTER_TITLE"].map((key) => (
            <TextInput key={key} size="xs" label={key} value={settings.strings[key] ?? ""} disabled={readOnly} onChange={(event) => update({ strings: { ...settings.strings, [key]: event.currentTarget.value } })} />
          ))}
        </Group>
      </Box>

      <Button w="fit-content" leftSection={<Icons.save size={IconSize.action} />} disabled={readOnly || !dirty} loading={save.isPending} onClick={() => save.mutate()}>構成を保存</Button>
    </Stack>
  );
}

// ============================================================
// 見た目
// ============================================================

function VisualEditor({ template, readOnly, onSaved }: { template: TemplateInfo; readOnly: boolean; onSaved: () => void }) {
  const [values, setValues] = useState<VisualTemplateValues>(visualTemplateDefaults);
  const [source, setSource] = useState("");
  const [dirty, setDirty] = useState(false);
  const runtime = isTauriRuntime();
  const style = useQuery({
    queryKey: ["template-file", template.name, STYLE_FILE],
    queryFn: () => (runtime ? readTemplateFile(template.name, STYLE_FILE) : Promise.resolve(demoFileContent(STYLE_FILE))),
  });
  useEffect(() => {
    if (style.data === undefined) return;
    setSource(style.data);
    setValues(readVisualSettings(style.data));
    setDirty(false);
  }, [style.data]);

  const set = <K extends keyof VisualTemplateValues>(key: K, value: VisualTemplateValues[K]) => {
    setValues((current) => ({ ...current, [key]: value }));
    setDirty(true);
  };

  const save = useMutation({
    mutationFn: () => saveTemplateFile(template.name, STYLE_FILE, writeVisualSettings(source, values)),
    onSuccess: () => { setDirty(false); style.refetch(); onSaved(); notifications.show({ color: "green", message: "見た目を保存しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "保存できません", message: errorMessage(error) }),
  });

  if (style.isLoading) return <LoadingState label="スタイルを読み込んでいます" />;

  return (
    <Stack gap="lg">
      <Section title="本文" description="読み手が最も長く見る部分です。">
        <Group grow>
          <Select label="書体" data={[{ value: "serif", label: "明朝体" }, { value: "sans", label: "ゴシック体" }]} value={values.fontFamily} disabled={readOnly} onChange={(value) => set("fontFamily", (value as VisualTemplateValues["fontFamily"]) ?? "serif")} />
          <NumberInput label="文字サイズ" hideControls suffix=" px" min={10} max={32} value={values.fontSize} disabled={readOnly} onChange={(value) => set("fontSize", Number(value) || visualTemplateDefaults.fontSize)} />
          <NumberInput label="ページ余白" hideControls suffix=" px" min={0} max={64} value={values.pagePadding} disabled={readOnly} onChange={(value) => set("pagePadding", Number(value) || 0)} />
        </Group>
        <LabeledSlider label="行間" value={values.lineHeight} min={1.2} max={2.6} step={0.05} disabled={readOnly} onChange={(value) => set("lineHeight", value)} />
        <LabeledSlider label="段落の間隔 (em)" value={values.paragraphSpacing} min={0} max={2} step={0.1} disabled={readOnly} onChange={(value) => set("paragraphSpacing", value)} />
        <LabeledSlider label="行頭の字下げ (em)" value={values.textIndent} min={0} max={2} step={0.5} disabled={readOnly} onChange={(value) => set("textIndent", value)} />
        <Group>
          <Switch label="両端揃え" disabled={readOnly} checked={values.justify} onChange={(event) => set("justify", event.currentTarget.checked)} />
          <Switch label="縦書き" description="綴じ方向も「右から左」にしてください" disabled={readOnly} checked={values.verticalWriting} onChange={(event) => set("verticalWriting", event.currentTarget.checked)} />
        </Group>
        <LabeledSlider label="ルビの大きさ (em)" value={values.rubySize} min={0.3} max={0.8} step={0.05} disabled={readOnly} onChange={(value) => set("rubySize", value)} />
      </Section>

      <Section title="配色">
        <Group grow>
          <ColorInput label="本文色" value={values.textColor} disabled={readOnly} onChange={(value) => set("textColor", value)} />
          <ColorInput label="背景色" value={values.backgroundColor} disabled={readOnly} onChange={(value) => set("backgroundColor", value)} />
        </Group>
        <Group grow>
          <ColorInput label="タグ・リンク色" value={values.accentColor} disabled={readOnly} onChange={(value) => set("accentColor", value)} />
          <ColorInput label="補助テキスト色" value={values.mutedColor} disabled={readOnly} onChange={(value) => set("mutedColor", value)} />
        </Group>
      </Section>

      <Section title="タイトルと見出し">
        <Group grow>
          <Select label="タイトル位置" data={[{ value: "left", label: "左揃え" }, { value: "center", label: "中央" }]} value={values.titleAlign} disabled={readOnly} onChange={(value) => set("titleAlign", (value as "left" | "center") ?? "left")} />
          <NumberInput label="タイトルの大きさ" hideControls suffix=" em" min={1} max={3} step={0.1} decimalScale={1} value={values.titleSize} disabled={readOnly} onChange={(value) => set("titleSize", Number(value) || 1.5)} />
        </Group>
        <Group grow>
          <Select label="章見出しの位置" data={[{ value: "left", label: "左揃え" }, { value: "center", label: "中央" }]} value={values.headingAlign} disabled={readOnly} onChange={(value) => set("headingAlign", (value as "left" | "center") ?? "left")} />
          <NumberInput label="章見出しの大きさ" hideControls suffix=" em" min={1} max={2.4} step={0.1} decimalScale={1} value={values.headingSize} disabled={readOnly} onChange={(value) => set("headingSize", Number(value) || 1.2)} />
        </Group>
        <Switch label="章見出しに罫線を引く" disabled={readOnly} checked={values.headingRule} onChange={(event) => set("headingRule", event.currentTarget.checked)} />
      </Section>

      <Section title="表紙と挿絵">
        <LabeledSlider label="作品情報の表紙の横幅 (%)" value={values.coverWidth} min={20} max={100} disabled={readOnly} onChange={(value) => set("coverWidth", value)} />
        <LabeledSlider label="表紙の角丸 (px)" value={values.coverRadius} min={0} max={32} disabled={readOnly} onChange={(value) => set("coverRadius", value)} />
        <LabeledSlider label="挿絵の横幅 (%)" value={values.illustrationWidth} min={30} max={100} disabled={readOnly} onChange={(value) => set("illustrationWidth", value)} />
      </Section>

      <Group>
        <Button leftSection={<Icons.save size={IconSize.action} />} disabled={readOnly || !dirty} loading={save.isPending} onClick={() => save.mutate()}>見た目を保存</Button>
        <Button variant="subtle" color="gray" leftSection={<Icons.undo size={IconSize.action} />} disabled={readOnly || !dirty} onClick={() => { setValues(readVisualSettings(source)); setDirty(false); }}>変更を捨てる</Button>
      </Group>
      <Text size="xs" c="dimmed">ここでの設定は {STYLE_FILE} の末尾にまとめて書き込まれます。それ以外の行には触れません。</Text>
    </Stack>
  );
}

function Section({ title, description, children }: { title: string; description?: string; children: React.ReactNode }) {
  return (
    <Box>
      <Text size="sm" fw={600}>{title}</Text>
      {description && <Text size="xs" c="dimmed" mb="xs">{description}</Text>}
      <Stack gap="sm" mt={description ? 0 : "xs"}>{children}</Stack>
    </Box>
  );
}

function LabeledSlider({ label, value, onChange, min, max, step, disabled }: { label: string; value: number; onChange: (value: number) => void; min: number; max: number; step?: number; disabled?: boolean }) {
  return (
    <Box>
      <Group justify="space-between" mb={4}><Text size="sm">{label}</Text><Badge variant="light" color="gray">{value}</Badge></Group>
      <Slider aria-label={label} value={value} onChange={onChange} min={min} max={max} step={step} disabled={disabled} />
    </Box>
  );
}

// ============================================================
// コード
// ============================================================

function CodeEditor({ template, readOnly, onSaved }: { template: TemplateInfo; readOnly: boolean; onSaved: () => void }) {
  const [filename, setFilename] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [dirty, setDirty] = useState(false);
  const runtime = isTauriRuntime();
  const files = useQuery({ queryKey: ["template-files", template.name], queryFn: () => (runtime ? getTemplateFiles(template.name) : Promise.resolve(demoFiles)) });
  const kinds = useQuery({ queryKey: ["template-file-kinds"], queryFn: () => (runtime ? listTemplateFileKinds() : Promise.resolve(demoFileKinds)), staleTime: Infinity });
  const active = filename ?? files.data?.[0]?.filename ?? null;
  const file = useQuery({
    queryKey: ["template-file", template.name, active],
    queryFn: () => (runtime ? readTemplateFile(template.name, active as string) : Promise.resolve(demoFileContent(active as string))),
    enabled: Boolean(active),
  });
  useEffect(() => { if (file.data !== undefined) { setContent(file.data); setDirty(false); } }, [file.data]);

  const save = useMutation({
    mutationFn: () => saveTemplateFile(template.name, active as string, content),
    onSuccess: () => { setDirty(false); files.refetch(); onSaved(); notifications.show({ color: "green", message: "保存しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "保存できません", message: errorMessage(error) }),
  });
  const reset = useMutation({
    mutationFn: () => resetTemplateFile(template.name, active as string),
    onSuccess: (restored) => { setContent(restored); setDirty(false); files.refetch(); onSaved(); notifications.show({ color: "green", message: "既定の内容に戻しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "戻せません", message: errorMessage(error) }),
  });

  const purpose = kinds.data?.find((kind: TemplateFileKind) => kind.filename === active)?.purpose;

  if (files.isLoading) return <LoadingState label="ファイルを読み込んでいます" />;

  return (
    <Grid gap="md" align="flex-start">
      <Grid.Col span={{ base: 12, sm: 4 }}>
        <Stack gap={2}>
          {(files.data ?? []).map((entry: TemplateFile) => (
            <Button key={entry.filename} variant={entry.filename === active ? "light" : "subtle"} color="gray" size="compact-sm" justify="flex-start" onClick={() => { setFilename(entry.filename); setDirty(false); }}>
              <Group gap={6} wrap="nowrap" w="100%">
                <Text size="xs" className="line-clamp-1" flex={1} ta="left">{entry.filename}</Text>
                {entry.customized && <Badge size="xs" variant="light" color="piep">変更済</Badge>}
              </Group>
            </Button>
          ))}
        </Stack>
      </Grid.Col>
      <Grid.Col span={{ base: 12, sm: 8 }}>
        <Stack gap="xs">
          {purpose && <Text size="xs" c="dimmed">{purpose}</Text>}
          <Textarea autosize minRows={18} maxRows={34} value={content} disabled={readOnly} aria-label={`${active ?? ""}の内容`} onChange={(event) => { setContent(event.currentTarget.value); setDirty(true); }} styles={{ input: { fontFamily: "var(--mantine-font-family-monospace)", fontSize: 12 } }} />
          <Group>
            <Button size="xs" leftSection={<Icons.save size={IconSize.inline} />} disabled={readOnly || !dirty} loading={save.isPending} onClick={() => save.mutate()}>保存</Button>
            <Button size="xs" variant="default" leftSection={<Icons.undo size={IconSize.inline} />} disabled={readOnly} loading={reset.isPending} onClick={() => reset.mutate()}>既定に戻す</Button>
          </Group>
        </Stack>
      </Grid.Col>
    </Grid>
  );
}

// ============================================================
// 差し込める項目
// ============================================================

function DataDictionary({ fields, loading }: { fields: DataField[]; loading: boolean }) {
  const [filter, setFilter] = useState("");
  const groups = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const matched = needle
      ? fields.filter((field) => field.path.toLowerCase().includes(needle) || field.label.toLowerCase().includes(needle))
      : fields;
    return matched.reduce<Record<string, DataField[]>>((acc, field) => {
      (acc[field.group] ??= []).push(field);
      return acc;
    }, {});
  }, [fields, filter]);

  if (loading) return <LoadingState label="項目を読み込んでいます" />;

  return (
    <Stack gap="sm">
      <Text size="xs" c="dimmed">プレビュー中の作品から取り出せる値の一覧です。コード編集で <code>{"{{ core.name }}"}</code> のように書くと差し込めます。</Text>
      <TextInput size="xs" placeholder="項目を絞り込む" leftSection={<Icons.search size={IconSize.inline} />} value={filter} onChange={(event) => setFilter(event.currentTarget.value)} />
      <ScrollArea.Autosize mah={520}>
        <Stack gap="md">
          {Object.entries(groups).map(([group, entries]) => (
            <Box key={group}>
              <Text size="xs" fw={700} c="dimmed" mb={4}>{group}</Text>
              <Table striped withTableBorder verticalSpacing={4} horizontalSpacing="xs" fz="xs">
                <Table.Tbody>
                  {entries.map((field) => (
                    <Table.Tr key={field.path} opacity={field.available ? 1 : 0.55}>
                      <Table.Td w="34%"><Text size="xs" fw={600}>{field.label}</Text></Table.Td>
                      <Table.Td w="30%"><code>{field.path}</code></Table.Td>
                      <Table.Td><Text size="xs" c="dimmed" className="line-clamp-1">{field.sample}</Text></Table.Td>
                      <Table.Td w={34}>
                        <Tooltip label="式をコピー">
                          <ActionIcon size="sm" variant="subtle" color="gray" aria-label={`${field.path}をコピー`} onClick={() => { void navigator.clipboard?.writeText(`{{ ${field.path} }}`); notifications.show({ message: `{{ ${field.path} }} をコピーしました` }); }}>
                            <Icons.epubDuplicate size={IconSize.inline} />
                          </ActionIcon>
                        </Tooltip>
                      </Table.Td>
                    </Table.Tr>
                  ))}
                </Table.Tbody>
              </Table>
            </Box>
          ))}
        </Stack>
      </ScrollArea.Autosize>
    </Stack>
  );
}

// ============================================================
// プレビュー
// ============================================================

function PreviewPanel({ preview, loading, error, target, onTarget, sampleId, onSampleChange, runtime, onRefresh }: {
  preview: TemplatePreview | undefined;
  loading: boolean;
  error: unknown;
  target: PreviewTarget;
  onTarget: (target: PreviewTarget) => void;
  sampleId: number | null;
  onSampleChange: (id: number | null) => void;
  runtime: boolean;
  onRefresh: () => void;
}) {
  const document = useMemo(() => {
    if (!preview) return null;
    switch (target) {
      case "cover": return preview.cover;
      case "info": return preview.info;
      case "page": return preview.page;
      case "nav": return preview.nav;
      case "ncx": return preview.ncx;
      default: return preview.opf;
    }
  }, [preview, target]);
  const rendered = target === "opf" || target === "ncx";

  return (
    <Card p="lg" className="template-studio__preview">
      <Stack gap="sm">
        <Group justify="space-between">
          <Box><Text fw={700}>プレビュー</Text><Text size="xs" c="dimmed" className="line-clamp-1">{preview?.sampleTitle ?? "—"}</Text></Box>
          <Tooltip label="描き直す"><ActionIcon variant="subtle" color="gray" aria-label="プレビューを描き直す" loading={loading} onClick={onRefresh}><Icons.retry size={IconSize.action} /></ActionIcon></Tooltip>
        </Group>

        <SamplePicker sampleId={sampleId} onChange={onSampleChange} runtime={runtime} />

        <SegmentedControl fullWidth size="xs" aria-label="プレビューする部分" value={target} onChange={(value) => onTarget(value as PreviewTarget)} data={PREVIEW_TARGETS.filter((item) => item.value !== "ncx" || preview?.ncx)} />

        {preview?.issues.length ? (
          <Alert color="red" icon={<Icons.warning size={IconSize.action} />} title="このテンプレートは壊れた EPUB を作ります">
            <Stack gap={2}>{preview.issues.map((issue, index) => <Text key={index} size="xs">{issue.location}：{issue.message}</Text>)}</Stack>
          </Alert>
        ) : null}

        {error ? (
          <ErrorState error={error} retry={onRefresh} />
        ) : !document ? (
          <Text size="sm" c="dimmed" py="xl" ta="center">このテンプレートでは作られない部分です。</Text>
        ) : rendered ? (
          <ScrollArea.Autosize mah={620}>
            <pre className="template-studio__source">{document}</pre>
          </ScrollArea.Autosize>
        ) : (
          <PreviewFrame html={document} css={preview?.css ?? ""} label={PREVIEW_TARGETS.find((item) => item.value === target)?.label ?? ""} />
        )}
      </Stack>
    </Card>
  );
}

/**
 * The rendered document, in the isolated frame it will live in on a device.
 *
 * The stylesheet is inlined rather than linked because the document refers to
 * it by a path that only exists inside the finished book.
 */
function PreviewFrame({ html, css, label }: { html: string; css: string; label: string }) {
  const frame = useRef<HTMLIFrameElement | null>(null);
  const source = useMemo(() => {
    const withoutLink = html.replace(/<link[^>]*rel="stylesheet"[^>]*\/>/g, "");
    return withoutLink.replace("</head>", `<style>${css}</style></head>`);
  }, [css, html]);
  return <iframe ref={frame} className="template-studio__frame" title={`${label}のプレビュー`} sandbox="" srcDoc={source} />;
}

function SamplePicker({ sampleId, onChange, runtime }: { sampleId: number | null; onChange: (id: number | null) => void; runtime: boolean }) {
  const [query, setQuery] = useState("");
  const recent = useQuery({
    queryKey: ["template-preview-works", query],
    queryFn: () => (runtime
      ? searchDownloadsV2({ text: query || null, limit: 20, sortBy: "downloaded_at", sortOrder: "desc" })
      : Promise.resolve({ items: [] as DownloadEntry[] })),
    enabled: runtime,
    staleTime: 30_000,
  });
  const current = useQuery({
    queryKey: ["template-preview-work", sampleId],
    queryFn: () => (runtime && sampleId ? getDownloads([sampleId]) : Promise.resolve([] as DownloadEntry[])),
    enabled: Boolean(runtime && sampleId),
  });
  const options = useMemo(() => {
    const items = [...(current.data ?? []), ...(recent.data?.items ?? [])];
    const seen = new Set<number>();
    return items.filter((item) => (seen.has(item.id) ? false : seen.add(item.id)));
  }, [current.data, recent.data]);

  return (
    <Select
      size="xs"
      label="プレビューに使う作品"
      placeholder="見本の作品"
      searchable
      clearable
      nothingFoundMessage="該当する作品がありません"
      data={options.map((work) => ({ value: String(work.id), label: work.title }))}
      value={sampleId === null ? null : String(sampleId)}
      searchValue={query}
      onSearchChange={setQuery}
      onChange={(value) => onChange(value ? Number(value) : null)}
      disabled={!runtime}
    />
  );
}
