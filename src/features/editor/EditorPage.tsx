import { forwardRef, memo, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Alert,
  Badge,
  Box,
  Button,
  Divider,
  Group,
  Image,
  Menu,
  Paper,
  ScrollArea,
  Select,
  Stack,
  Text,
  Textarea,
  TextInput,
  ThemeIcon,
  Tooltip,
} from "@mantine/core";
import { useForm, type UseFormReturnType } from "@mantine/form";
import { useHotkeys, useLocalStorage } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Icons, IconSize, type LucideIcon } from "@/lib/icons";
import { useReturnTo, useRouteParams } from "@/app/router";
import { ErrorState, LoadingState } from "@/components/AsyncState";
import { errorMessage } from "@/lib/format";
import { registerUnsavedGuard } from "@/lib/unsavedGuard";
import { getDemoEditor } from "@/mocks/demoData";
import { activateWorkEdit, getAssetUrl, getEditorDocument, importWorkAsset, isTauriRuntime, saveWorkDraft } from "@/services/dbApi";
import { openSingleDialog } from "@/services/dialogApi";
import type { AssetEntry, WorkBlockInput } from "@/types/library";

type EditorBlockType = "paragraph" | "heading" | "quote" | "image" | "link" | "separator" | "pageBreak";
type EditorBlockValue = WorkBlockInput & { clientId: string; blockType: EditorBlockType };
interface EditorValues { blocks: EditorBlockValue[] }
interface EditorSaveSnapshot {
  values: EditorValues;
  persistedBlocks: WorkBlockInput[];
  fingerprint: string;
}
interface PreviewHandle {
  updateBlock: (index: number, patch: Partial<EditorBlockValue>) => void;
}

const BLOCK_META: Record<EditorBlockType, { label: string; icon: LucideIcon; tone: string }> = {
  paragraph: { label: "文章", icon: Icons.paragraph, tone: "gray" },
  heading: { label: "見出し", icon: Icons.heading, tone: "gray" },
  quote: { label: "引用", icon: Icons.quote, tone: "gray" },
  image: { label: "画像", icon: Icons.insertImage, tone: "gray" },
  link: { label: "URLカード", icon: Icons.link, tone: "gray" },
  separator: { label: "区切り", icon: Icons.remove, tone: "gray" },
  pageBreak: { label: "改ページ", icon: Icons.read, tone: "piep" },
};

const editorType = (value: string): EditorBlockType => value === "page_break" ? "pageBreak" : value in BLOCK_META ? value as EditorBlockType : "paragraph";
const persistedType = (value: EditorBlockType): string => value === "pageBreak" ? "page_break" : value;
export const isSafeDocumentLink = (value: unknown): boolean => {
  try {
    const url = new URL(String(value ?? ""));
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
};
const estimateEditorBlockSize = (type?: EditorBlockType) => {
  if (type === "image" || type === "link") return 220;
  if (type === "separator" || type === "pageBreak") return 118;
  return 150;
};
const makeBlock = (blockType: EditorBlockType = "paragraph"): EditorBlockValue => ({
  clientId: crypto.randomUUID(),
  blockType,
  text: blockType === "separator" || blockType === "pageBreak" || blockType === "image" ? null : "",
  assetId: null,
  attrsJson: blockType === "link" ? JSON.stringify({ label: "" }) : null,
});

function createEditorSaveSnapshot(values: EditorValues): EditorSaveSnapshot {
  const clonedValues = { blocks: values.blocks.map((block) => ({ ...block })) };
  return {
    values: clonedValues,
    persistedBlocks: clonedValues.blocks.map(({ clientId: _clientId, blockType, ...block }) => ({ ...block, blockType: persistedType(blockType) })),
    fingerprint: JSON.stringify(clonedValues),
  };
}

export default function EditorPage() {
  const returnTo = useReturnTo();
  const { workId } = useRouteParams("/editor/:workId");
  const id = Number(workId);
  const runtime = isTauriRuntime();
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: ["editor-document", id], queryFn: () => runtime ? getEditorDocument(id) : Promise.resolve(getDemoEditor(id)), enabled: Number.isFinite(id) });
  const [dirty, setDirty] = useState(false);
  const [blocks, setBlocks] = useState<EditorBlockValue[]>([]);
  const form = useForm<EditorValues>({
    mode: "uncontrolled",
    initialValues: { blocks: [] },
    validate: {
      blocks: {
        text: (value, values, path) => {
          const block = values.blocks[Number(path.split(".")[1])];
          if (!block) return null;
          if (["paragraph", "heading", "quote"].includes(block.blockType) && !String(value ?? "").trim()) return `${BLOCK_META[block.blockType].label}を入力してください`;
          if (block.blockType === "link") {
            if (!isSafeDocumentLink(value)) return "http:// または https:// から始まるURLを入力してください";
          }
          return null;
        },
      },
    },
    validateInputOnBlur: true,
    onValuesChange: () => setDirty(true),
  });
  const [syncScroll, setSyncScroll] = useLocalStorage({ key: "piep.editor-scroll-sync", defaultValue: true });
  const [activeBlock, setActiveBlock] = useState<string | null>(null);
  const editorScrollRef = useRef<HTMLDivElement>(null);
  const previewScrollRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<PreviewHandle>(null);
  const syncLockRef = useRef<"editor" | "preview" | null>(null);
  const syncTimerRef = useRef<number | null>(null);
  const editorVirtualizer = useVirtualizer({
    count: blocks.length,
    getScrollElement: () => editorScrollRef.current,
    getItemKey: (index) => blocks[index]?.clientId ?? index,
    estimateSize: (index) => estimateEditorBlockSize(blocks[index]?.blockType),
    initialRect: { width: 760, height: 800 },
    overscan: 6,
  });

  const alignMatchingBlock = useCallback((target: "editor" | "preview", clientId: string) => {
    if (target === "editor") {
      const index = blocks.findIndex((block) => block.clientId === clientId);
      if (index >= 0) editorVirtualizer.scrollToIndex(index, { align: "start" });
      return;
    }
    const viewport = previewScrollRef.current;
    if (!viewport) return;
    const attribute = "data-preview-block-id";
    const element = viewport.querySelector<HTMLElement>(`[${attribute}="${CSS.escape(clientId)}"]`);
    if (!element) return;
    const viewportRect = viewport.getBoundingClientRect();
    const elementRect = element.getBoundingClientRect();
    viewport.scrollTop += elementRect.top - viewportRect.top - 20;
  }, [blocks, editorVirtualizer]);

  const activateBlock = useCallback((clientId: string, origin: "editor" | "preview") => {
    setActiveBlock(clientId);
    if (!syncScroll) return;
    syncLockRef.current = origin;
    alignMatchingBlock(origin === "editor" ? "preview" : "editor", clientId);
    if (syncTimerRef.current) window.clearTimeout(syncTimerRef.current);
    syncTimerRef.current = window.setTimeout(() => { syncLockRef.current = null; }, 120);
  }, [alignMatchingBlock, syncScroll]);

  useEffect(() => {
    if (!query.data || form.initialized) return;
    const nextBlocks = query.data.blocks.map((block) => ({ clientId: crypto.randomUUID(), blockType: editorType(block.blockType), text: block.text, assetId: block.assetId, attrsJson: block.attrsJson }));
    form.initialize({ blocks: nextBlocks });
    setBlocks(nextBlocks);
    setDirty(false);
  }, [form, query.data]);
  useEffect(() => {
    const guard = (event: BeforeUnloadEvent) => { if (dirty) { event.preventDefault(); event.returnValue = ""; } };
    window.addEventListener("beforeunload", guard);
    return () => window.removeEventListener("beforeunload", guard);
  }, [dirty]);
  // `beforeunload` never fires when the desktop window is closed, so the same
  // state is registered with the native close guard as well.
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  useEffect(() => registerUnsavedGuard(() => dirtyRef.current), []);
  useEffect(() => {
    if (!syncScroll) return;
    const editorViewport = editorScrollRef.current;
    const previewViewport = previewScrollRef.current;
    if (!editorViewport || !previewViewport) return;
    let frame = 0;
    const observe = (origin: "editor" | "preview") => {
      if (syncLockRef.current && syncLockRef.current !== origin) return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const viewport = origin === "editor" ? editorViewport : previewViewport;
        const attribute = origin === "editor" ? "data-editor-block-id" : "data-preview-block-id";
        const rect = viewport.getBoundingClientRect();
        const point = document.elementFromPoint(rect.left + Math.min(40, rect.width / 2), rect.top + Math.min(42, rect.height / 2));
        const current = point?.closest<HTMLElement>(`[${attribute}]`);
        const clientId = current?.getAttribute(attribute);
        if (current && clientId && viewport.contains(current)) activateBlock(clientId, origin);
      });
    };
    const onEditorScroll = () => observe("editor");
    const onPreviewScroll = () => observe("preview");
    editorViewport.addEventListener("scroll", onEditorScroll, { passive: true });
    previewViewport.addEventListener("scroll", onPreviewScroll, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      editorViewport.removeEventListener("scroll", onEditorScroll);
      previewViewport.removeEventListener("scroll", onPreviewScroll);
    };
  }, [activateBlock, blocks.length, syncScroll]);
  useEffect(() => () => {
    if (syncTimerRef.current) window.clearTimeout(syncTimerRef.current);
  }, []);

  const persistSnapshot = async (snapshot: EditorSaveSnapshot) => {
    if (!query.data) throw new Error("編集データがありません");
    if (!runtime) return { id: 1, downloadId: id, baseVersion: query.data.baseVersion, status: "draft", title: null, contentHash: null, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() };
    return saveWorkDraft(id, query.data.baseVersion, snapshot.persistedBlocks);
  };
  const clearDirtyIfCurrent = (snapshot: EditorSaveSnapshot) => {
    if (JSON.stringify(form.getValues()) !== snapshot.fingerprint) return;
    form.resetDirty(snapshot.values);
    setDirty(false);
  };
  const saveMutation = useMutation({
    mutationFn: persistSnapshot,
    onSuccess: (_revision, snapshot) => { clearDirtyIfCurrent(snapshot); notifications.show({ color: "green", icon: <Icons.confirm size={IconSize.menu} />, title: "下書きを保存しました", message: "公開中の本文はまだ変わりません" }); queryClient.invalidateQueries({ queryKey: ["editor-document", id] }); },
    onError: (error) => notifications.show({ color: "red", title: "下書きを保存できません", message: errorMessage(error) }),
  });
  const publishMutation = useMutation({
    mutationFn: async (snapshot: EditorSaveSnapshot) => {
      const revision = await persistSnapshot(snapshot);
      return runtime ? activateWorkEdit(revision.id) : revision;
    },
    // 反映は本文を差し替えるので、カードが出している字数と版の印も変わる。
    // それらは棚と作者ページの一覧から引いているため、そこも古くしておく。
    onSuccess: (_revision, snapshot) => { clearDirtyIfCurrent(snapshot); notifications.show({ color: "green", title: "編集版を反映しました", message: "リーダーとEPUBに編集内容が使われます" }); queryClient.invalidateQueries({ queryKey: ["reader-metadata", id] }); queryClient.invalidateQueries({ queryKey: ["reader-content-page", id] }); queryClient.invalidateQueries({ queryKey: ["reader-content-search", id] }); queryClient.invalidateQueries({ queryKey: ["editor-document", id] }); queryClient.invalidateQueries({ queryKey: ["library"] }); queryClient.invalidateQueries({ queryKey: ["entity-works"] }); },
    onError: (error) => notifications.show({ color: "red", title: "編集版を反映できません", message: errorMessage(error) }),
  });
  useHotkeys([["mod+S", (event) => { event.preventDefault(); if (!saveMutation.isPending && !publishMutation.isPending) form.onSubmit((values) => saveMutation.mutate(createEditorSaveSnapshot(values)))(); }]]);
  const replaceBlocks = useCallback((nextBlocks: EditorBlockValue[]) => {
    form.setFieldValue("blocks", nextBlocks);
    setBlocks(nextBlocks);
    setDirty(true);
  }, [form]);
  const insertBlock = useCallback((blockType: EditorBlockType, index = blocks.length) => {
    const nextBlocks = [...form.getValues().blocks];
    nextBlocks.splice(index, 0, makeBlock(blockType));
    replaceBlocks(nextBlocks);
  }, [blocks.length, form, replaceBlocks]);
  const moveBlock = useCallback((index: number, direction: -1 | 1) => {
    const nextBlocks = [...form.getValues().blocks];
    const target = index + direction;
    if (!nextBlocks[index] || target < 0 || target >= nextBlocks.length) return;
    [nextBlocks[index], nextBlocks[target]] = [nextBlocks[target], nextBlocks[index]];
    replaceBlocks(nextBlocks);
  }, [form, replaceBlocks]);
  const removeBlock = useCallback((index: number) => {
    replaceBlocks(form.getValues().blocks.filter((_, blockIndex) => blockIndex !== index));
  }, [form, replaceBlocks]);
  const updateBlock = useCallback((index: number, patch: Partial<EditorBlockValue>) => previewRef.current?.updateBlock(index, patch), []);
  const addAsset = useCallback(async (index = blocks.length) => {
    if (!runtime) return notifications.show({ color: "piep", message: "画像の追加はデスクトップアプリで利用できます" });
    const doc = query.data;
    if (!doc) return;
    const path = await openSingleDialog({ title: "画像を追加", filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif", "avif"] }] });
    if (!path) return;
    try {
      const asset = await importWorkAsset(id, path);
      queryClient.setQueryData(["editor-document", id], { ...doc, assets: [...doc.assets, asset] });
      const nextBlocks = [...form.getValues().blocks];
      nextBlocks.splice(index, 0, { ...makeBlock("image"), assetId: asset.id });
      replaceBlocks(nextBlocks);
    } catch (error) { notifications.show({ color: "red", title: "画像を追加できません", message: errorMessage(error) }); }
  }, [blocks.length, form, id, query.data, queryClient, replaceBlocks, runtime]);
  if (query.isLoading) return <div className="page"><LoadingState label="エディタを準備しています" /></div>;
  if (query.error || !query.data) return <div className="page"><ErrorState error={query.error ?? "作品がありません"} retry={() => query.refetch()} /></div>;
  const doc = query.data;
  const assets = doc.assets;
  const allowPageBreak = doc.download.source === "pixiv";
  const virtualItems = editorVirtualizer.getVirtualItems();
  const renderedEditorItems = virtualItems.length ? virtualItems : blocks.slice(0, 8).map((block, index) => ({ index, key: block.clientId, start: blocks.slice(0, index).reduce((sum, item) => sum + estimateEditorBlockSize(item.blockType), 0) }));
  const virtualHeight = editorVirtualizer.getTotalSize() || blocks.reduce((sum, block) => sum + estimateEditorBlockSize(block.blockType), 0);
  // Returns to the detail screen the editor was opened from rather than
  // pushing a second copy of it, which left the header's back button pointing
  // at the editor the user had just closed.
  // 訊くのはルータのガードに任せる。ここでも訊いていたとき、破棄を選んだ直後に
  // まだ dirty のまま navigate へ入るので、同じ問いがもう一枚開いていた。
  // 未保存の登録（177行）は既定スコープで navigate も見ているため、この一行で
  // 確認は変わらず出る。テンプレート編集の画面も同じ作りになっている。
  const goBack = () => returnTo(`/works/${id}`);
  return (
    <div className="editor-page">
      <header className="editor-toolbar">
        <Group h="100%" px="md" justify="space-between" wrap="nowrap" className="editor-toolbar__inner">
          <Group wrap="nowrap" miw={0} className="editor-toolbar__identity"><Tooltip label="作品詳細へ戻る"><ActionIcon variant="subtle" color="gray" aria-label="作品詳細へ戻る" onClick={goBack}><Icons.back size={IconSize.nav} /></ActionIcon></Tooltip><Divider orientation="vertical" h={24} /><Box miw={0}><Text size="sm" fw={700} className="editor-toolbar__title line-clamp-1" title={doc.download.title}>{doc.download.title}</Text><Group gap="xs"><Text size="xs" c="dimmed">編集とプレビュー</Text>{dirty && <Badge size="xs" color="yellow" variant="light">未保存</Badge>}</Group></Box></Group>
          <Group gap="xs" wrap="nowrap" className="editor-toolbar__actions">
            <Tooltip label={syncScroll ? "位置同期をオフ" : "位置同期をオン"}><ActionIcon size="lg" variant={syncScroll ? "light" : "default"} color="piep" aria-label="編集ブロックとプレビューの位置を同期" aria-pressed={syncScroll} onClick={() => setSyncScroll(!syncScroll)}><Icons.separator size={IconSize.action} /></ActionIcon></Tooltip>
            <Button size="sm" variant="light" leftSection={<Icons.save size={IconSize.menu} />} loading={saveMutation.isPending} disabled={publishMutation.isPending || (!dirty && Boolean(doc.draftRevision))} onClick={() => form.onSubmit((values) => saveMutation.mutate(createEditorSaveSnapshot(values)))()}>下書き保存</Button>
            <Tooltip label="保存して、リーダーとEPUBで使う本文を更新"><Button size="sm" leftSection={<Icons.confirm size={IconSize.menu} />} loading={publishMutation.isPending} disabled={saveMutation.isPending} onClick={() => form.onSubmit((values) => publishMutation.mutate(createEditorSaveSnapshot(values)))()}>反映</Button></Tooltip>
          </Group>
        </Group>
      </header>

      <div className="editor-layout">
        <ScrollArea className="editor-main" viewportRef={editorScrollRef} type="scroll" scrollbarSize={8}>
          <form onSubmit={form.onSubmit((values) => saveMutation.mutate(createEditorSaveSnapshot(values)))} className="editor-document">
            <BlockInsertMenu label="先頭にブロックを追加" allowPageBreak={allowPageBreak} onInsert={(type) => insertBlock(type, 0)} onImage={() => addAsset(0)} />
            {blocks.length ? <Box className="editor-virtual-list" style={{ height: virtualHeight }}>
              {renderedEditorItems.map((item) => {
                const block = blocks[item.index];
                if (!block) return null;
                return <Box key={block.clientId} ref={editorVirtualizer.measureElement} data-index={item.index} className="editor-virtual-row" style={{ transform: `translateY(${item.start}px)` }}>
                  <EditorBlock index={item.index} block={block} assets={assets} total={blocks.length} form={form} active={activeBlock === block.clientId} allowPageBreak={allowPageBreak} onActivate={activateBlock} onInsert={insertBlock} onImage={addAsset} onMove={moveBlock} onRemove={removeBlock} onUpdate={updateBlock} />
                </Box>;
              })}
            </Box> : <Alert color="gray" mt="sm">本文ブロックがありません。上のボタンから追加してください。</Alert>}
          </form>
        </ScrollArea>

        <EditorPreviewPane ref={previewRef} viewportRef={previewScrollRef} blocks={blocks} assets={assets} activeBlock={activeBlock} allowPageBreak={allowPageBreak} seriesTitle={doc.download.seriesTitle} title={doc.download.title} authorName={doc.download.authorName} onActivate={activateBlock} />
      </div>
    </div>
  );
}

const EditorBlock = memo(function EditorBlock({ index, block, assets, total, form, active, allowPageBreak, onActivate, onInsert, onImage, onMove, onRemove, onUpdate }: { index: number; block: EditorBlockValue; assets: AssetEntry[]; total: number; form: UseFormReturnType<EditorValues, EditorValues, any>; active: boolean; allowPageBreak: boolean; onActivate: (clientId: string, origin: "editor" | "preview") => void; onInsert: (type: EditorBlockType, index: number) => void; onImage: (index: number) => void; onMove: (index: number, direction: -1 | 1) => void; onRemove: (index: number) => void; onUpdate: (index: number, patch: Partial<EditorBlockValue>) => void }) {
  const meta = BLOCK_META[block.blockType];
  const Icon = meta.icon;
  const asset = assets.find((item) => item.id === block.assetId);
  const textInputProps = form.getInputProps(`blocks.${index}.text`);
  /**
   * いま form が持っている、このブロックの値。
   *
   * `blocks` は打鍵では更新しない（毎打鍵で一覧全体が再描画されるため）。
   * 一方この一覧は仮想化されていて、窓から出た行は本当にアンマウントされる。
   * 画像とリンクのブロックは props から一度だけ状態の種を取るので、`blocks`
   * を渡していると、スクロールして戻ったときに入力前の値へ巻き戻って見えた。
   * 文章ブロックが `form.key` + defaultValue で form を読み直しているのと
   * 同じ考え方で、種も form から取る。
   */
  const liveBlock = form.getValues().blocks?.[index] ?? block;
  const onTextChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    textInputProps.onChange?.(event);
    onUpdate(index, { text: event.currentTarget.value });
  };
  return (
    <Paper className="editor-block" withBorder data-type={block.blockType} data-active={active || undefined} data-editor-block-id={block.clientId} onClick={() => onActivate(block.clientId, "editor")} onFocusCapture={() => onActivate(block.clientId, "editor")}>
      <Group className="editor-block__bar" px="sm" py={6} justify="space-between" wrap="nowrap">
        <Group gap={6} wrap="nowrap"><Icons.drag className="editor-block__handle" size={15} /><ThemeIcon size={22} radius="sm" variant="light" color={meta.tone}><Icon size={13} /></ThemeIcon><Text size="xs" fw={750}>{meta.label}</Text><Text size="10px" c="dimmed">{index + 1}</Text></Group>
        <Group gap={2} wrap="nowrap">
          <BlockMenuTarget allowPageBreak={allowPageBreak} onInsert={(type) => onInsert(type, index + 1)} onImage={() => onImage(index + 1)} />
          <Tooltip label="上へ"><ActionIcon size="sm" variant="subtle" color="gray" disabled={index === 0} aria-label={`ブロック ${index + 1}を上へ`} onClick={() => onMove(index, -1)}><Icons.up size={IconSize.menu} /></ActionIcon></Tooltip>
          <Tooltip label="下へ"><ActionIcon size="sm" variant="subtle" color="gray" disabled={index === total - 1} aria-label={`ブロック ${index + 1}を下へ`} onClick={() => onMove(index, 1)}><Icons.down size={IconSize.menu} /></ActionIcon></Tooltip>
          <Tooltip label="削除"><ActionIcon size="sm" variant="subtle" color="red" aria-label={`ブロック ${index + 1}を削除`} onClick={() => onRemove(index)}><Icons.delete size={IconSize.menu} /></ActionIcon></Tooltip>
        </Group>
      </Group>
      <Box className="editor-block__body" p="sm">
        {block.blockType === "paragraph" && <Textarea autosize minRows={2} maxRows={24} placeholder="本文を入力…" variant="unstyled" key={form.key(`blocks.${index}.text`)} {...textInputProps} onChange={onTextChange} aria-label={`文章 ${index + 1}`} />}
        {block.blockType === "heading" && <Textarea autosize minRows={1} maxRows={4} placeholder="見出し" variant="unstyled" className="editor-heading-input" key={form.key(`blocks.${index}.text`)} {...textInputProps} onChange={onTextChange} aria-label={`見出し ${index + 1}`} />}
        {block.blockType === "quote" && <Textarea autosize minRows={2} maxRows={16} placeholder="引用文" variant="unstyled" className="editor-quote-input" key={form.key(`blocks.${index}.text`)} {...textInputProps} onChange={onTextChange} aria-label={`引用 ${index + 1}`} />}
        {block.blockType === "separator" && <Divider my="md" label="区切り" labelPosition="center" />}
        {block.blockType === "pageBreak" && <Group className="editor-page-break" justify="center" gap="xs"><Icons.separator size={IconSize.menu} /><Text size="xs" fw={700}>ここから次のpixiv原稿ページ</Text></Group>}
        {block.blockType === "image" && <ImageBlock asset={asset} assets={assets} value={liveBlock.assetId} caption={liveBlock.text} onChange={(assetId) => { form.setFieldValue(`blocks.${index}.assetId`, assetId); onUpdate(index, { assetId }); }} onCaptionChange={(text) => { form.setFieldValue(`blocks.${index}.text`, text); onUpdate(index, { text }); }} />}
        {block.blockType === "link" && <LinkBlock url={liveBlock.text ?? ""} label={linkLabel(liveBlock.attrsJson)} onUrlChange={(text) => { form.setFieldValue(`blocks.${index}.text`, text); onUpdate(index, { text }); }} onLabelChange={(label) => { const attrsJson = JSON.stringify({ label }); form.setFieldValue(`blocks.${index}.attrsJson`, attrsJson); onUpdate(index, { attrsJson }); }} error={form.errors[`blocks.${index}.text`]} />}
      </Box>
    </Paper>
  );
});

function BlockInsertMenu({ label, allowPageBreak, onInsert, onImage }: { label: string; allowPageBreak: boolean; onInsert: (type: EditorBlockType) => void; onImage: () => void }) {
  return <Group justify="center" className="editor-insert-row"><Menu position="bottom"><Menu.Target><Button size="compact-xs" variant="subtle" color="gray" leftSection={<Icons.add size={IconSize.inline} />}>{label}</Button></Menu.Target><BlockMenuDropdown allowPageBreak={allowPageBreak} onInsert={onInsert} onImage={onImage} /></Menu></Group>;
}

function BlockMenuTarget({ allowPageBreak, onInsert, onImage }: { allowPageBreak: boolean; onInsert: (type: EditorBlockType) => void; onImage: () => void }) {
  return <Menu position="bottom-end"><Menu.Target><Tooltip label="この下に追加"><ActionIcon size="sm" variant="subtle" color="gray" aria-label="このブロックの下に追加"><Icons.add size={IconSize.menu} /></ActionIcon></Tooltip></Menu.Target><BlockMenuDropdown allowPageBreak={allowPageBreak} onInsert={onInsert} onImage={onImage} /></Menu>;
}

function BlockMenuDropdown({ allowPageBreak, onInsert, onImage }: { allowPageBreak: boolean; onInsert: (type: EditorBlockType) => void; onImage: () => void }) {
  return <Menu.Dropdown><Menu.Label>ブロックを追加</Menu.Label><Menu.Item leftSection={<Icons.paragraph size={IconSize.menu} />} onClick={() => onInsert("paragraph")}>文章</Menu.Item><Menu.Item leftSection={<Icons.heading size={IconSize.menu} />} onClick={() => onInsert("heading")}>見出し</Menu.Item><Menu.Item leftSection={<Icons.quote size={IconSize.menu} />} onClick={() => onInsert("quote")}>引用</Menu.Item><Menu.Item leftSection={<Icons.link size={IconSize.menu} />} onClick={() => onInsert("link")}>URLカード</Menu.Item><Menu.Item leftSection={<Icons.remove size={IconSize.menu} />} onClick={() => onInsert("separator")}>区切り線</Menu.Item>{allowPageBreak && <Menu.Item leftSection={<Icons.separator size={IconSize.menu} />} onClick={() => onInsert("pageBreak")}>pixiv改ページ</Menu.Item>}<Menu.Divider /><Menu.Item leftSection={<Icons.insertImage size={IconSize.menu} />} onClick={onImage}>画像ファイルを追加</Menu.Item></Menu.Dropdown>;
}

function ImageBlock({ asset, assets, value, caption, onChange, onCaptionChange }: { asset?: AssetEntry; assets: AssetEntry[]; value: number | null | undefined; caption: string | null | undefined; onChange: (value: number | null) => void; onCaptionChange: (value: string) => void }) {
  const [assetId, setAssetId] = useState(value ?? null);
  const [captionValue, setCaptionValue] = useState(caption ?? "");
  const selectedAsset = assets.find((item) => item.id === assetId) ?? asset;
  return <Group wrap="nowrap" align="flex-start"><Box className="editor-image-preview">{selectedAsset ? <Image src={getAssetUrl(selectedAsset.localPath)} alt="" w="100%" h="100%" fit="contain" /> : <Icons.imageFile size={IconSize.hero} />}</Box><Stack flex={1} gap="xs"><Select label="画像アセット" placeholder="選択してください" searchable data={assets.filter((item) => item.mimeType?.startsWith("image/")).map((item) => ({ value: String(item.id), label: item.filename }))} value={assetId ? String(assetId) : null} onChange={(next) => { const nextId = next ? Number(next) : null; setAssetId(nextId); onChange(nextId); }} clearable /><TextInput label="キャプション（任意）" value={captionValue} onChange={(event) => { setCaptionValue(event.currentTarget.value); onCaptionChange(event.currentTarget.value); }} /></Stack></Group>;
}

function LinkBlock({ url, label, onUrlChange, onLabelChange, error }: { url: string; label: string; onUrlChange: (value: string) => void; onLabelChange: (value: string) => void; error?: React.ReactNode }) {
  const [urlValue, setUrlValue] = useState(url);
  const [labelValue, setLabelValue] = useState(label);
  return <Stack gap="xs"><TextInput label="URL" placeholder="https://example.com/…" value={urlValue} onChange={(event) => { setUrlValue(event.currentTarget.value); onUrlChange(event.currentTarget.value); }} error={error} leftSection={<Icons.link size={IconSize.menu} />} /><TextInput label="表示名（任意）" placeholder="空欄ならURLを表示" value={labelValue} onChange={(event) => { setLabelValue(event.currentTarget.value); onLabelChange(event.currentTarget.value); }} /></Stack>;
}

const EditorPreviewPane = forwardRef<PreviewHandle, {
  viewportRef: React.RefObject<HTMLDivElement | null>;
  blocks: EditorBlockValue[];
  assets: AssetEntry[];
  activeBlock: string | null;
  allowPageBreak: boolean;
  seriesTitle?: string | null;
  title: string;
  authorName: string;
  onActivate: (clientId: string, origin: "editor" | "preview") => void;
}>(function EditorPreviewPane({ viewportRef, blocks, assets, activeBlock, allowPageBreak, seriesTitle, title, authorName, onActivate }, ref) {
  const [previewBlocks, setPreviewBlocks] = useState(blocks);
  const pendingRef = useRef(new Map<number, Partial<EditorBlockValue>>());
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    pendingRef.current.clear();
    setPreviewBlocks(blocks);
  }, [blocks]);
  useEffect(() => () => { if (timerRef.current !== null) window.clearTimeout(timerRef.current); }, []);
  useImperativeHandle(ref, () => ({
    updateBlock(index, patch) {
      pendingRef.current.set(index, { ...pendingRef.current.get(index), ...patch });
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(() => {
        const pending = new Map(pendingRef.current);
        pendingRef.current.clear();
        setPreviewBlocks((current) => current.map((block, blockIndex) => pending.has(blockIndex) ? { ...block, ...pending.get(blockIndex) } : block));
      }, 110);
    },
  }), []);

  const stats = useMemo(() => ({
    blocks: previewBlocks.length,
    chars: previewBlocks.reduce((sum, block) => sum + (block.text?.length ?? 0), 0),
    images: previewBlocks.filter((block) => block.blockType === "image").length,
    pages: previewBlocks.filter((block) => block.blockType === "pageBreak").length + 1,
  }), [previewBlocks]);
  const assetMap = useMemo(() => new Map(assets.map((asset) => [asset.id, asset])), [assets]);
  let pageNumber = 1;

  return <section className="editor-preview-pane" aria-label="本文プレビュー">
    <Group className="editor-preview-toolbar" px="md" justify="space-between" wrap="nowrap">
      <Group gap="xs"><Icons.read size={IconSize.action} /><Text size="sm" fw={750}>ライブプレビュー</Text></Group>
      <Group gap="md" wrap="nowrap" className="editor-preview-stats">{allowPageBreak && <Text size="xs" c="dimmed" fw={650}>{stats.pages}頁</Text>}<Text size="xs" c="dimmed" fw={650}>{stats.blocks.toLocaleString("ja-JP")}ブロック</Text><Text size="xs" c="dimmed" fw={650}>{stats.chars.toLocaleString("ja-JP")}字</Text><Text size="xs" c="dimmed" fw={650}>{stats.images}画像</Text></Group>
    </Group>
    <ScrollArea className="editor-preview-scroll" viewportRef={viewportRef} type="scroll" scrollbarSize={8}>
      <article className="editor-preview-document">
        <Text className="editor-preview-series" size="xs" c="dimmed">{seriesTitle}</Text>
        <Text component="h1" className="editor-preview-title">{title}</Text>
        <Text size="sm" c="dimmed" mb="xl">{authorName}</Text>
        {!previewBlocks.length ? <Text c="dimmed" ta="center" py="xl">ブロックを追加するとここに表示されます。</Text> : previewBlocks.map((block) => {
          if (block.blockType === "pageBreak") pageNumber += 1;
          return <PreviewBlock key={block.clientId} block={block} asset={block.assetId ? assetMap.get(block.assetId) : undefined} pageNumber={pageNumber} active={activeBlock === block.clientId} onActivate={onActivate} />;
        })}
      </article>
    </ScrollArea>
  </section>;
});

const PreviewBlock = memo(function PreviewBlock({ block, asset, pageNumber, active, onActivate }: { block: EditorBlockValue; asset?: AssetEntry; pageNumber: number; active: boolean; onActivate: (clientId: string, origin: "editor" | "preview") => void }) {
  let content: React.ReactNode;
  if (block.blockType === "heading") content = <h2>{block.text}</h2>;
  else if (block.blockType === "quote") content = <blockquote>{block.text}</blockquote>;
  else if (block.blockType === "separator") content = <hr />;
  else if (block.blockType === "pageBreak") content = <div className="editor-preview-page-break"><span>pixiv page {pageNumber}</span></div>;
  else if (block.blockType === "image") content = <figure>{asset ? <img src={getAssetUrl(asset.localPath) ?? undefined} alt={block.text || asset.filename} /> : <Text c="dimmed">画像を選択してください</Text>}{block.text && <figcaption>{block.text}</figcaption>}</figure>;
  else if (block.blockType === "link") content = <a className="editor-preview-link" href={block.text || undefined} onClick={(event) => event.preventDefault()}><Icons.link size={IconSize.nav} /><span><strong>{linkLabel(block.attrsJson) || block.text || "URLを入力"}</strong><small>{block.text}</small></span></a>;
  else {
    const lines = block.text?.split("\n") ?? [];
    content = <p>{lines.map((line, index) => <span key={index}>{line}{index < lines.length - 1 && <br />}</span>)}</p>;
  }
  return <div className="editor-preview-block" data-active={active || undefined} data-preview-block-id={block.clientId} onClick={() => onActivate(block.clientId, "preview")}>{content}</div>;
});

function linkLabel(attrsJson: string | null | undefined): string {
  try { const value = JSON.parse(attrsJson || "{}"); return typeof value.label === "string" ? value.label : ""; } catch { return ""; }
}
