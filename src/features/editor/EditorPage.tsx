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
import { useDisclosure, useHotkeys, useLocalStorage } from "@mantine/hooks";
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
  /** 取得元の題を書き換えるとき。同じなら端末側で落とすので残らない。 */
  title?: string;
  /** 自動保存。押していないものを知らせても、手が止まるだけ。 */
  silent?: boolean;
}

/** 保存に出せる形かどうか。自動保存が、書きかけを弾かれて騒がないための判断。 */
function blocksAreSavable(values: EditorValues): boolean {
  return values.blocks.every((block) => {
    if (["paragraph", "heading", "quote"].includes(block.blockType)) return Boolean(String(block.text ?? "").trim());
    if (block.blockType === "link") return isSafeDocumentLink(block.text);
    return true;
  });
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
const estimatePreviewBlockSize = (type?: EditorBlockType) => {
  if (type === "image") return 300;
  if (type === "link") return 96;
  if (type === "separator" || type === "pageBreak") return 60;
  if (type === "heading") return 78;
  return 110;
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
  const [autoSave, setAutoSave] = useLocalStorage({ key: "piep.editor-autosave", defaultValue: true });
  // 取り消しのための控え。**文章を扱う画面に取り消しが無い**というのが、
  // この編集画面でいちばん痛い欠けだった。段落を一つ消すと戻す手立ては
  // どこにも無く、書き直すしかなかった。
  const historyRef = useRef<EditorBlockValue[][]>([]);
  const futureRef = useRef<EditorBlockValue[][]>([]);
  const [historyDepth, setHistoryDepth] = useState({ undo: 0, redo: 0 });
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const [savedAt, setSavedAt] = useState<Date | null>(null);
  const [findOpened, findPanel] = useDisclosure(false);
  const [findTerm, setFindTerm] = useState("");
  const [replaceTerm, setReplaceTerm] = useState("");
  // 取得元が付けた題を直せるようにする。誤字のある題を直す手立てが無かった。
  const [title, setTitle] = useState("");
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
    // 下書きが題を持っていればそれ、無ければいま表示されている題。
    // `download.title` は反映済みの編集を含んだ「読み手に見えている題」。
    setTitle(query.data.draftRevision?.title ?? query.data.download.title);
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
    return saveWorkDraft(id, query.data.baseVersion, snapshot.title ?? null, snapshot.persistedBlocks);
  };
  const clearDirtyIfCurrent = (snapshot: EditorSaveSnapshot) => {
    if (JSON.stringify(form.getValues()) !== snapshot.fingerprint) return;
    form.resetDirty(snapshot.values);
    setDirty(false);
  };
  const saveMutation = useMutation({
    mutationFn: persistSnapshot,
    onSuccess: (_revision, snapshot) => {
      clearDirtyIfCurrent(snapshot);
      setSavedAt(new Date());
      // 自動保存は黙って済ませる。押していない保存をそのつど知らせても、
      // 目と手が止まるだけで、伝わるものが増えるわけではない。
      if (!snapshot.silent) notifications.show({ color: "green", icon: <Icons.confirm size={IconSize.menu} />, title: "下書きを保存しました", message: "公開中の本文はまだ変わりません" });
      queryClient.invalidateQueries({ queryKey: ["editor-document", id] });
    },
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
  /**
   * 出せなかった理由を、その場で言う。
   *
   * 一覧は仮想化されているので、画面の外にあるブロックの赤字は誰にも見えない。
   * 何も起きないボタンになっていたのを、どのブロックが止めているかを告げて、
   * そこへ連れて行くように変えた。
   */
  const reportInvalid = useCallback((errors: Record<string, React.ReactNode>) => {
    const path = Object.keys(errors)[0];
    const index = Number(path?.split(".")[1]);
    const detail = typeof errors[path] === "string" ? errors[path] : "入力を確かめてください";
    notifications.show({
      color: "red",
      title: "まだ保存できません",
      message: Number.isSafeInteger(index) ? `${index + 1}番目のブロック：${detail}` : String(detail),
    });
    if (!Number.isSafeInteger(index)) return;
    editorVirtualizer.scrollToIndex(index, { align: "center" });
    const block = form.getValues().blocks[index];
    if (block) setActiveBlock(block.clientId);
  }, [editorVirtualizer, form]);
  const submitDraft = useCallback(() => {
    form.onSubmit((values) => saveMutation.mutate({ ...createEditorSaveSnapshot(values), title }), reportInvalid)();
  }, [form, reportInvalid, saveMutation, title]);
  const submitPublish = useCallback(() => {
    form.onSubmit((values) => publishMutation.mutate({ ...createEditorSaveSnapshot(values), title }), reportInvalid)();
  }, [form, publishMutation, reportInvalid, title]);
  const busy = saveMutation.isPending || publishMutation.isPending;

  // 一定時間さわらなければ下書きへ落とす。取り消しがあっても、閉じてしまえば
  // 戻れない ―― 書いたものが残る道を、押し忘れに頼らせない。
  const save = saveMutation.mutate;
  useEffect(() => {
    if (!autoSave || !dirty || !query.data || busy) return undefined;
    const timer = window.setTimeout(() => {
      const values = form.getValues();
      // 書きかけで整っていないうちは、黙って待つ。赤字を出しに行かない。
      if (!blocksAreSavable(values)) return;
      save({ ...createEditorSaveSnapshot(values), title, silent: true });
    }, 6_000);
    return () => window.clearTimeout(timer);
    // `saveMutation` そのものは毎描画で別物になる。丸ごと見張ると、関係のない
    // 再描画のたびに数え直しになって、いつまでも保存されない。
  }, [autoSave, busy, dirty, form, query.data, save, title]);

  useHotkeys([
    ["mod+F", (event) => { event.preventDefault(); findPanel.open(); }],
    ["mod+S", (event) => { event.preventDefault(); if (!busy) submitDraft(); }],
    // 入力欄の中では効かせない（Mantine が既定で input/textarea を外す）。
    // 文字の打ち消しは入力欄自身の取り消しに任せ、ここではブロックの
    // 出入りと並べ替えを戻す。
    ["mod+Z", (event) => { event.preventDefault(); undo(); }],
    ["mod+shift+Z", (event) => { event.preventDefault(); redo(); }],
    ["mod+Y", (event) => { event.preventDefault(); redo(); }],
  ]);
  /**
   * `rekey` は、中身を丸ごと差し替えたときに使う。
   *
   * 入力欄は非制御なので、同じ行が同じ id のまま値だけ変わっても、画面の
   * 文字は書き換わらない。取り消しと一括置換はまさにそれをやるので、
   * 行に新しい id を振って組み立て直させる。並べ替えや削除では振らない
   * ―― 打鍵の途中で id が変わると、入力中の欄から焦点が外れる。
   */
  const applyBlocks = useCallback((nextBlocks: EditorBlockValue[], rekey = false) => {
    const applied = rekey ? nextBlocks.map((block) => ({ ...block, clientId: crypto.randomUUID() })) : nextBlocks;
    form.setFieldValue("blocks", applied);
    setBlocks(applied);
    setDirty(true);
  }, [form]);
  /** いまの中身を控える。文字の打鍵は入力欄自身の取り消しに任せ、ここでは
   *  ブロックの出入りと並べ替えだけを憶える。消えた段落が戻せればよい。 */
  const snapshotBlocks = useCallback(() => form.getValues().blocks.map((block) => ({ ...block })), [form]);
  const replaceBlocks = useCallback((nextBlocks: EditorBlockValue[], rekey = false) => {
    historyRef.current.push(snapshotBlocks());
    // 際限なく貯めない。深いところまで戻れることより、重くならないこと。
    if (historyRef.current.length > 120) historyRef.current.shift();
    futureRef.current = [];
    setHistoryDepth({ undo: historyRef.current.length, redo: 0 });
    applyBlocks(nextBlocks, rekey);
  }, [applyBlocks, snapshotBlocks]);

  /**
   * 全文の置き換え。長い作品の誤字を直す手立てが、これまでは目視しかなかった。
   *
   * 探すのは文字を持つブロックだけ。URL やキャプションまで巻き込むと、
   * 「本文の誤字を直したらリンクが壊れた」が起こる。
   */
  const findMatches = useMemo(() => {
    const term = findTerm;
    if (!term) return 0;
    return blocks.reduce((sum, block) => {
      if (!["paragraph", "heading", "quote"].includes(block.blockType)) return sum;
      return sum + (String(block.text ?? "").split(term).length - 1);
    }, 0);
  }, [blocks, findTerm]);
  const replaceAll = useCallback(() => {
    if (!findTerm) return;
    const nextBlocks = form.getValues().blocks.map((block) => {
      if (!["paragraph", "heading", "quote"].includes(block.blockType)) return block;
      const text = String(block.text ?? "");
      if (!text.includes(findTerm)) return block;
      return { ...block, text: text.split(findTerm).join(replaceTerm) };
    });
    const changed = nextBlocks.filter((block, index) => block !== blocks[index]).length;
    if (!changed) return;
    replaceBlocks(nextBlocks, true);
    notifications.show({ color: "piep", message: `${changed}個のブロックで置き換えました（元に戻すで取り消せます）` });
  }, [blocks, findTerm, form, replaceBlocks, replaceTerm]);
  const jumpToMatch = useCallback(() => {
    if (!findTerm) return;
    const index = form.getValues().blocks.findIndex((block) => ["paragraph", "heading", "quote"].includes(block.blockType) && String(block.text ?? "").includes(findTerm));
    if (index < 0) return;
    editorVirtualizer.scrollToIndex(index, { align: "center" });
    const block = form.getValues().blocks[index];
    if (block) setActiveBlock(block.clientId);
  }, [editorVirtualizer, findTerm, form]);

  const undo = useCallback(() => {
    const previous = historyRef.current.pop();
    if (!previous) return;
    futureRef.current.push(snapshotBlocks());
    applyBlocks(previous, true);
    setHistoryDepth({ undo: historyRef.current.length, redo: futureRef.current.length });
  }, [applyBlocks, snapshotBlocks]);
  const redo = useCallback(() => {
    const next = futureRef.current.pop();
    if (!next) return;
    historyRef.current.push(snapshotBlocks());
    applyBlocks(next, true);
    setHistoryDepth({ undo: historyRef.current.length, redo: futureRef.current.length });
  }, [applyBlocks, snapshotBlocks]);
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
  /** 掴んで落とす並べ替え。上下ボタンだけだったころは、300 番目のブロックを
   *  先頭へ動かすのに 299 回押す必要があった。掴む取っ手は前から描いてあり、
   *  掴めないことだけが本当だった。 */
  const moveBlockTo = useCallback((from: number, to: number) => {
    const nextBlocks = [...form.getValues().blocks];
    if (from === to || !nextBlocks[from] || to < 0 || to > nextBlocks.length - 1) return;
    const [moved] = nextBlocks.splice(from, 1);
    nextBlocks.splice(to, 0, moved);
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
      // 画像は作品に属する。エディタの手元だけ増やすと、作品詳細のアセット欄と
      // タブの件数が古いままになる。
      queryClient.invalidateQueries({ queryKey: ["work-assets", id] });
      queryClient.invalidateQueries({ queryKey: ["reader-metadata", id] });
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
          <Group wrap="nowrap" miw={0} className="editor-toolbar__identity"><Tooltip label="作品詳細へ戻る"><ActionIcon variant="subtle" color="gray" aria-label="作品詳細へ戻る" onClick={goBack}><Icons.back size={IconSize.nav} /></ActionIcon></Tooltip><Divider orientation="vertical" h={24} /><Box miw={0} className="editor-toolbar__titlebox"><TextInput size="xs" variant="unstyled" className="editor-toolbar__title" aria-label="この作品のタイトル" value={title} placeholder="タイトル" onChange={(event) => { setTitle(event.currentTarget.value); setDirty(true); }} /><Group gap="xs"><Text size="xs" c="dimmed">編集とプレビュー</Text>{dirty ? <Badge size="xs" color="yellow" variant="light">未保存</Badge> : savedAt && <Text size="xs" c="dimmed">{savedAt.toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit" })}に保存</Text>}</Group></Box></Group>
          <Group gap="xs" wrap="nowrap" className="editor-toolbar__actions">
            <Tooltip label="元に戻す (Ctrl+Z)"><ActionIcon size="lg" variant="default" aria-label="元に戻す" disabled={historyDepth.undo === 0} onClick={undo}><Icons.undo size={IconSize.action} /></ActionIcon></Tooltip>
            <Tooltip label="やり直す (Ctrl+Shift+Z)"><ActionIcon size="lg" variant="default" aria-label="やり直す" disabled={historyDepth.redo === 0} onClick={redo}><Icons.redo size={IconSize.action} /></ActionIcon></Tooltip>
            <Tooltip label="本文を検索・置換 (Ctrl+F)"><ActionIcon size="lg" variant={findOpened ? "light" : "default"} color="piep" aria-label="本文を検索・置換" aria-pressed={findOpened} onClick={findPanel.toggle}><Icons.search size={IconSize.action} /></ActionIcon></Tooltip>
            <Tooltip label={syncScroll ? "位置同期をオフ" : "位置同期をオン"}><ActionIcon size="lg" variant={syncScroll ? "light" : "default"} color="piep" aria-label="編集ブロックとプレビューの位置を同期" aria-pressed={syncScroll} onClick={() => setSyncScroll(!syncScroll)}><Icons.separator size={IconSize.action} /></ActionIcon></Tooltip>
            <Tooltip label={autoSave ? "自動保存をオフ" : "自動保存をオン"}><ActionIcon size="lg" variant={autoSave ? "light" : "default"} color="piep" aria-label="自動保存" aria-pressed={autoSave} onClick={() => setAutoSave(!autoSave)}><Icons.save size={IconSize.action} /></ActionIcon></Tooltip>
            <Button size="sm" variant="light" leftSection={<Icons.save size={IconSize.menu} />} loading={saveMutation.isPending} disabled={publishMutation.isPending || (!dirty && Boolean(doc.draftRevision))} onClick={submitDraft}>下書き保存</Button>
            <Tooltip label="保存して、リーダーとEPUBで使う本文を更新"><Button size="sm" leftSection={<Icons.confirm size={IconSize.menu} />} loading={publishMutation.isPending} disabled={saveMutation.isPending} onClick={submitPublish}>反映</Button></Tooltip>
          </Group>
        </Group>
      </header>

      {/* 長い作品の誤字を直す手立てが、これまでは目視しかなかった。 */}
      {findOpened && <Paper className="editor-find" withBorder shadow="sm" p="xs">
        <Group gap="xs" wrap="nowrap">
          <TextInput size="xs" w={190} placeholder="本文から探す" aria-label="本文から探す" value={findTerm} onChange={(event) => setFindTerm(event.currentTarget.value)} leftSection={<Icons.search size={IconSize.menu} />} />
          <TextInput size="xs" w={190} placeholder="置き換える文字（空でも可）" aria-label="置き換える文字" value={replaceTerm} onChange={(event) => setReplaceTerm(event.currentTarget.value)} />
          <Text size="xs" c="dimmed" w={72}>{findTerm ? `${findMatches}件` : "—"}</Text>
          <Button size="compact-xs" variant="default" disabled={!findMatches} onClick={jumpToMatch}>最初へ</Button>
          <Button size="compact-xs" disabled={!findMatches} onClick={replaceAll}>すべて置換</Button>
          <ActionIcon size="sm" variant="subtle" color="gray" aria-label="検索を閉じる" onClick={findPanel.close}><Icons.cancel size={IconSize.menu} /></ActionIcon>
        </Group>
      </Paper>}

      <div className="editor-layout">
        <ScrollArea className="editor-main" viewportRef={editorScrollRef} type="scroll" scrollbarSize={8}>
          <form onSubmit={(event) => { event.preventDefault(); submitDraft(); }} className="editor-document">
            <BlockInsertMenu label="先頭にブロックを追加" allowPageBreak={allowPageBreak} onInsert={(type) => insertBlock(type, 0)} onImage={() => addAsset(0)} />
            {blocks.length ? <Box className="editor-virtual-list" style={{ height: virtualHeight }}>
              {renderedEditorItems.map((item) => {
                const block = blocks[item.index];
                if (!block) return null;
                return <Box key={block.clientId} ref={editorVirtualizer.measureElement} data-index={item.index} className="editor-virtual-row" style={{ transform: `translateY(${item.start}px)` }}>
                  <EditorBlock index={item.index} block={block} assets={assets} total={blocks.length} form={form} active={activeBlock === block.clientId} allowPageBreak={allowPageBreak} dragging={dragFrom === item.index} dropTarget={dragFrom !== null && dragOver === item.index && dragOver !== dragFrom} onActivate={activateBlock} onInsert={insertBlock} onImage={addAsset} onMove={moveBlock} onRemove={removeBlock} onUpdate={updateBlock} onDragBegin={setDragFrom} onDragEnter={setDragOver} onDragFinish={() => { if (dragFrom !== null && dragOver !== null) moveBlockTo(dragFrom, dragOver); setDragFrom(null); setDragOver(null); }} />
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

const EditorBlock = memo(function EditorBlock({ index, block, assets, total, form, active, allowPageBreak, dragging, dropTarget, onActivate, onInsert, onImage, onMove, onRemove, onUpdate, onDragBegin, onDragEnter, onDragFinish }: { index: number; block: EditorBlockValue; assets: AssetEntry[]; total: number; form: UseFormReturnType<EditorValues, EditorValues, any>; active: boolean; allowPageBreak: boolean; dragging: boolean; dropTarget: boolean; onActivate: (clientId: string, origin: "editor" | "preview") => void; onInsert: (type: EditorBlockType, index: number) => void; onImage: (index: number) => void; onMove: (index: number, direction: -1 | 1) => void; onRemove: (index: number) => void; onUpdate: (index: number, patch: Partial<EditorBlockValue>) => void; onDragBegin: (index: number) => void; onDragEnter: (index: number) => void; onDragFinish: () => void }) {
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
    <Paper
      className="editor-block"
      withBorder
      data-type={block.blockType}
      data-active={active || undefined}
      data-dragging={dragging || undefined}
      data-drop-target={dropTarget || undefined}
      data-editor-block-id={block.clientId}
      onClick={() => onActivate(block.clientId, "editor")}
      onFocusCapture={() => onActivate(block.clientId, "editor")}
      onDragOver={(event) => { if (dragging) return; event.preventDefault(); onDragEnter(index); }}
      onDrop={(event) => { event.preventDefault(); onDragFinish(); }}
    >
      <Group className="editor-block__bar" px="sm" py={6} justify="space-between" wrap="nowrap">
        <Group gap={6} wrap="nowrap">
          {/* 取っ手はずっと描いてあったのに掴めなかった。掴めるようにする。 */}
          <Box
            component="span"
            className="editor-block__handle"
            draggable
            role="button"
            tabIndex={-1}
            aria-label={`ブロック ${index + 1}を掴んで並べ替え`}
            onDragStart={(event) => { event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", String(index)); onDragBegin(index); }}
            onDragEnd={onDragFinish}
          >
            <Icons.drag size={15} />
          </Box>
          <ThemeIcon size={22} radius="sm" variant="light" color={meta.tone}><Icon size={13} /></ThemeIcon><Text size="xs" fw={750}>{meta.label}</Text><Text size="10px" c="dimmed">{index + 1}</Text>
        </Group>
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
  // 改ページは前から数えないと番号が出せない。仮想化すると前のブロックは
  // 描かれていないので、番号だけ先に通しで用意しておく。
  const pageNumbers = useMemo(() => {
    let page = 1;
    return previewBlocks.map((block) => {
      if (block.blockType === "pageBreak") page += 1;
      return page;
    });
  }, [previewBlocks]);
  // 編集側だけを仮想化していたので、仮想化が要るほど長い作品では
  // プレビューが同じだけ重かった。両側とも窓の分しか描かない。
  const previewVirtualizer = useVirtualizer({
    count: previewBlocks.length,
    getScrollElement: () => viewportRef.current,
    getItemKey: (index) => previewBlocks[index]?.clientId ?? index,
    estimateSize: (index) => estimatePreviewBlockSize(previewBlocks[index]?.blockType),
    initialRect: { width: 620, height: 800 },
    overscan: 8,
  });
  const previewItems = previewVirtualizer.getVirtualItems();
  const renderedPreviewItems = previewItems.length
    ? previewItems
    : previewBlocks.slice(0, 12).map((block, index) => ({ index, key: block.clientId, start: previewBlocks.slice(0, index).reduce((sum, item) => sum + estimatePreviewBlockSize(item.blockType), 0) }));
  const previewHeight = previewVirtualizer.getTotalSize() || previewBlocks.reduce((sum, block) => sum + estimatePreviewBlockSize(block.blockType), 0);

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
        {!previewBlocks.length ? <Text c="dimmed" ta="center" py="xl">ブロックを追加するとここに表示されます。</Text> : <div className="editor-preview-virtual-list" style={{ height: previewHeight }}>
          {renderedPreviewItems.map((item) => {
            const block = previewBlocks[item.index];
            if (!block) return null;
            return <div key={block.clientId} ref={previewVirtualizer.measureElement} data-index={item.index} className="editor-preview-virtual-row" style={{ transform: `translateY(${item.start}px)` }}>
              <PreviewBlock block={block} asset={block.assetId ? assetMap.get(block.assetId) : undefined} pageNumber={pageNumbers[item.index] ?? 1} active={activeBlock === block.clientId} onActivate={onActivate} />
            </div>;
          })}
        </div>}
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
  else if (block.blockType === "image") content = <figure>{asset ? <img src={getAssetUrl(asset.localPath) ?? undefined} alt={block.text || asset.filename} loading="lazy" decoding="async" /> : <Text c="dimmed">画像を選択してください</Text>}{block.text && <figcaption>{block.text}</figcaption>}</figure>;
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
