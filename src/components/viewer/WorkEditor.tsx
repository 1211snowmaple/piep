import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  BookOpen,
  Check,
  GripVertical,
  Heading2,
  ImagePlus,
  Plus,
  Save,
  SeparatorHorizontal,
  Trash2,
  Type,
} from "lucide-react";
import {
  activateWorkEdit,
  getAssetUrl,
  getEditorDocument,
  importWorkAsset,
  saveWorkDraft,
} from "@/services/dbApi";
import { openSingleDialog } from "@/services/dialogApi";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { AssetEntry, WorkBlock, WorkBlockInput } from "@/types/library";

interface WorkEditorProps {
  downloadId: number;
  showToast: (msg: string, type: "success" | "error" | "info") => void;
  onBack: () => void;
  onRead: () => void;
  onSaved?: () => void;
}

type EditableBlock = {
  localId: string;
  blockType: "paragraph" | "heading" | "image" | "separator";
  text: string;
  assetId: number | null;
  attrsJson: string | null;
};

function toEditableBlocks(blocks: WorkBlock[]): EditableBlock[] {
  if (blocks.length === 0) return [newBlock("paragraph")];
  return blocks.map((block, idx) => ({
    localId: `${block.id || "new"}-${idx}-${crypto.randomUUID()}`,
    blockType: block.blockType === "heading" || block.blockType === "image" || block.blockType === "separator"
      ? block.blockType
      : "paragraph",
    text: block.text ?? "",
    assetId: block.assetId,
    attrsJson: block.attrsJson,
  }));
}

function newBlock(blockType: EditableBlock["blockType"], assetId: number | null = null): EditableBlock {
  return {
    localId: crypto.randomUUID(),
    blockType,
    text: "",
    assetId,
    attrsJson: null,
  };
}

function toInputs(blocks: EditableBlock[]): WorkBlockInput[] {
  return blocks.map(block => ({
    blockType: block.blockType,
    text: block.text,
    assetId: block.assetId,
    attrsJson: block.attrsJson,
  }));
}

function assetLabel(asset: AssetEntry | undefined): string {
  if (!asset) return "画像が未選択です";
  return asset.filename;
}

export function WorkEditor({ downloadId, showToast, onBack, onRead, onSaved }: WorkEditorProps) {
  const queryClient = useQueryClient();
  const editorQuery = useQuery({
    queryKey: ["editor-document", downloadId],
    queryFn: () => getEditorDocument(downloadId),
  });
  const [blocks, setBlocks] = useState<EditableBlock[]>([newBlock("paragraph")]);
  const [saving, setSaving] = useState(false);
  const [publishing, setPublishing] = useState(false);

  useEffect(() => {
    if (editorQuery.data) {
      setBlocks(toEditableBlocks(editorQuery.data.blocks));
    }
  }, [editorQuery.data]);

  const imageAssets = useMemo(
    () => (editorQuery.data?.assets ?? []).filter(asset => asset.mimeType?.startsWith("image/")),
    [editorQuery.data?.assets],
  );

  const assetById = useMemo(() => {
    const map = new Map<number, AssetEntry>();
    for (const asset of editorQuery.data?.assets ?? []) map.set(asset.id, asset);
    return map;
  }, [editorQuery.data?.assets]);

  const updateBlock = (localId: string, patch: Partial<EditableBlock>) => {
    setBlocks(current => current.map(block => block.localId === localId ? { ...block, ...patch } : block));
  };

  const insertBlock = (index: number, blockType: EditableBlock["blockType"]) => {
    setBlocks(current => {
      const next = [...current];
      next.splice(index + 1, 0, newBlock(blockType));
      return next;
    });
  };

  const removeBlock = (localId: string) => {
    setBlocks(current => current.length <= 1 ? current : current.filter(block => block.localId !== localId));
  };

  const moveBlock = (index: number, direction: -1 | 1) => {
    setBlocks(current => {
      const target = index + direction;
      if (target < 0 || target >= current.length) return current;
      const next = [...current];
      const [item] = next.splice(index, 1);
      next.splice(target, 0, item);
      return next;
    });
  };

  const handleImportImage = async (index: number) => {
    const file = await openSingleDialog({
      multiple: false,
      filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
    });
    if (!file || Array.isArray(file)) return;

    try {
      const asset = await importWorkAsset(downloadId, file);
      await queryClient.invalidateQueries({ queryKey: ["editor-document", downloadId] });
      setBlocks(current => {
        const next = [...current];
        next.splice(index + 1, 0, newBlock("image", asset.id));
        return next;
      });
      showToast("挿絵を追加しました", "success");
    } catch (error) {
      showToast(`挿絵の追加に失敗しました: ${error}`, "error");
    }
  };

  const handleSaveDraft = async () => {
    if (!editorQuery.data) return null;
    setSaving(true);
    try {
      const revision = await saveWorkDraft(downloadId, editorQuery.data.baseVersion, toInputs(blocks));
      await queryClient.invalidateQueries({ queryKey: ["editor-document", downloadId] });
      await queryClient.invalidateQueries({ queryKey: ["reader-document", downloadId] });
      showToast("下書きを保存しました", "success");
      onSaved?.();
      return revision;
    } catch (error) {
      showToast(`保存に失敗しました: ${error}`, "error");
      return null;
    } finally {
      setSaving(false);
    }
  };

  const handlePublish = async () => {
    setPublishing(true);
    try {
      const revision = await handleSaveDraft();
      if (!revision) return;
      await activateWorkEdit(revision.id);
      await queryClient.invalidateQueries({ queryKey: ["editor-document", downloadId] });
      await queryClient.invalidateQueries({ queryKey: ["reader-document", downloadId] });
      showToast("編集版を有効にしました", "success");
      onSaved?.();
    } catch (error) {
      showToast(`有効化に失敗しました: ${error}`, "error");
    } finally {
      setPublishing(false);
    }
  };

  if (editorQuery.isLoading) {
    return <div className="work-editor work-editor-loading"><div className="spinner" /></div>;
  }

  if (editorQuery.isError || !editorQuery.data) {
    return (
      <div className="work-editor work-editor-error">
        <Button type="button" variant="ghost" size="icon" onClick={onBack}><ArrowLeft size={18} /></Button>
        <p>編集データの読み込みに失敗しました。</p>
      </div>
    );
  }

  const doc = editorQuery.data;

  return (
    <div className="work-editor">
      <header className="work-editor-topbar">
        <Button type="button" variant="ghost" size="icon" onClick={onBack} title="戻る">
          <ArrowLeft size={18} />
        </Button>
        <div className="work-editor-title">
          <span>{doc.activeRevision ? "編集版あり" : "原本から編集"}</span>
          <h1>{doc.download.title}</h1>
          <p>{doc.download.authorName}</p>
        </div>
        <div className="work-editor-actions">
          <Button type="button" variant="ghost" size="icon" onClick={onRead} title="読む">
            <BookOpen size={18} />
          </Button>
          <Button type="button" variant="outline" size="sm" className="gap-2" onClick={handleSaveDraft} disabled={saving || publishing}>
            <Save size={16} />
            下書き保存
          </Button>
          <Button type="button" size="sm" className="gap-2" onClick={handlePublish} disabled={saving || publishing}>
            <Check size={16} />
            有効化
          </Button>
        </div>
      </header>

      <main className="work-editor-canvas">
        {blocks.map((block, index) => {
          const asset = block.assetId ? assetById.get(block.assetId) : undefined;
          const src = getAssetUrl(asset?.localPath);
          return (
            <Card key={block.localId} className={`editor-block editor-block-${block.blockType}`}>
              <div className="editor-block-grip">
                <GripVertical size={16} />
                <Button type="button" variant="ghost" size="icon" onClick={() => moveBlock(index, -1)} disabled={index === 0}>↑</Button>
                <Button type="button" variant="ghost" size="icon" onClick={() => moveBlock(index, 1)} disabled={index === blocks.length - 1}>↓</Button>
              </div>
              <div className="editor-block-main">
                <div className="editor-block-typebar">
                  <Button type="button" variant={block.blockType === "paragraph" ? "secondary" : "ghost"} size="sm" className={cn(block.blockType === "paragraph" && "active")} onClick={() => updateBlock(block.localId, { blockType: "paragraph" })}>
                    <Type size={14} /> 段落
                  </Button>
                  <Button type="button" variant={block.blockType === "heading" ? "secondary" : "ghost"} size="sm" className={cn(block.blockType === "heading" && "active")} onClick={() => updateBlock(block.localId, { blockType: "heading" })}>
                    <Heading2 size={14} /> 見出し
                  </Button>
                  <Button type="button" variant={block.blockType === "separator" ? "secondary" : "ghost"} size="sm" className={cn(block.blockType === "separator" && "active")} onClick={() => updateBlock(block.localId, { blockType: "separator" })}>
                    <SeparatorHorizontal size={14} /> 区切り
                  </Button>
                </div>

                {block.blockType === "image" ? (
                  <div className="editor-image-block">
                    {src ? <img src={src} alt="" /> : <div className="editor-image-placeholder"><ImagePlus size={22} /></div>}
                    <Select
                      value={block.assetId == null ? "__none__" : String(block.assetId)}
                      onValueChange={value => updateBlock(block.localId, { assetId: value === "__none__" ? null : Number(value) })}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="__none__">{assetLabel(undefined)}</SelectItem>
                        {imageAssets.map(item => <SelectItem key={item.id} value={String(item.id)}>{item.filename}</SelectItem>)}
                      </SelectContent>
                    </Select>
                    <span>{assetLabel(asset)}</span>
                  </div>
                ) : block.blockType === "separator" ? (
                  <div className="editor-separator-preview" />
                ) : (
                  <Textarea
                    value={block.text}
                    onChange={event => updateBlock(block.localId, { text: event.target.value })}
                    rows={block.blockType === "heading" ? 1 : 5}
                    className={block.blockType === "heading" ? "editor-heading-input" : ""}
                    placeholder={block.blockType === "heading" ? "見出し" : "本文を入力"}
                  />
                )}

                <div className="editor-insert-row">
                  <Button type="button" variant="outline" size="sm" onClick={() => insertBlock(index, "paragraph")}><Plus size={14} /> 段落</Button>
                  <Button type="button" variant="outline" size="sm" onClick={() => insertBlock(index, "heading")}><Heading2 size={14} /> 見出し</Button>
                  <Button type="button" variant="outline" size="sm" onClick={() => handleImportImage(index)}><ImagePlus size={14} /> 挿絵</Button>
                  <Button type="button" variant="outline" size="sm" onClick={() => insertBlock(index, "separator")}><SeparatorHorizontal size={14} /> 区切り</Button>
                  <Button type="button" variant="destructive" size="icon" className="danger" onClick={() => removeBlock(block.localId)} disabled={blocks.length <= 1}><Trash2 size={14} /></Button>
                </div>
              </div>
            </Card>
          );
        })}
      </main>
    </div>
  );
}
