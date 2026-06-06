import { useState, useEffect, useMemo } from "react";
import { Settings2 } from "lucide-react";
import { PlusIcon, SaveIcon, TrashIcon, BookIcon, PanelRightIcon } from "../icons/Icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import {
  createEpubTemplate,
  deleteEpubTemplate,
  exportEpub,
  exportEpubBatch,
  getTemplateFiles,
  listEpubTemplates,
  readTemplateFile,
  saveTemplateFile,
} from "@/services/epubApi";
import { askDialog, openSingleDialog, saveDialog } from "@/services/dialogApi";
import { onTauriEvent } from "@/services/eventBus";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// --- Interfaces ---

interface TemplateInfo {
  name: string;
  isBuiltin: boolean;
  fileCount: number;
}

interface TemplateFileInfo {
  filename: string;
  sizeBytes: number;
}

interface ExportProgress {
  phase: string;
  currentTitle: string;
  currentIndex: number;
  totalCount: number;
  message: string;
}

interface ExportBatchResult {
  successCount: number;
  failedCount: number;
  failedIds: number[];
  outputFiles: string[];
}



interface EpubSidebarProps {
  selectedIds: Set<number>;
  selectedItems?: any[];
  showToast: (text: string, type: "success" | "error" | "info") => void;
  isOpen: boolean;
  onClose: () => void;
  showTemplateManager: boolean;
  onCloseTemplateManager: () => void;
}

export function EpubSidebar({ selectedIds, selectedItems = [], showToast, isOpen, onClose, showTemplateManager: externalShowTM, onCloseTemplateManager }: EpubSidebarProps) {
  // --- Export settings state ---
  const [templates, setTemplates] = useState<TemplateInfo[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState("__auto__");

  // 自動判定によるグループ分けロジック
  const autoGroups = useMemo(() => {
    if (selectedTemplate !== "__auto__" || !selectedItems || selectedItems.length === 0) {
      return null;
    }
    const pixivList = selectedItems.filter(item => item.source === "pixiv");
    const defaultList = selectedItems.filter(item => item.source !== "pixiv");
    return {
      pixiv: pixivList,
      default: defaultList
    };
  }, [selectedTemplate, selectedItems]);
  const [compressEnabled, setCompressEnabled] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  // JPEG options
  const [jpegQuality, setJpegQuality] = useState(85);
  const [jpegProgressive, setJpegProgressive] = useState(true);
  const [jpegChromaSubsampling, setJpegChromaSubsampling] = useState("4:2:0");
  const [jpegAutoOptimize, setJpegAutoOptimize] = useState(false);
  const [jpegDeringing, setJpegDeringing] = useState(true);
  const [jpegSeparateChromaTables, setJpegSeparateChromaTables] = useState(true);
  const [jpegSharpYuv, setJpegSharpYuv] = useState(false);

  // PNG options
  const [pngCompression, setPngCompression] = useState("2");
  const [pngInterlace, setPngInterlace] = useState(false);
  const [pngStrip, setPngStrip] = useState(true);
  const [pngOptimizeAlpha, setPngOptimizeAlpha] = useState(false);
  const [pngBitDepthReduction, setPngBitDepthReduction] = useState(true);
  const [pngColorTypeReduction, setPngColorTypeReduction] = useState(true);
  const [pngPaletteReduction, setPngPaletteReduction] = useState(true);
  const [pngGrayscaleReduction, setPngGrayscaleReduction] = useState(true);
  const [pngIdatRecoding, setPngIdatRecoding] = useState(true);
  const [pngFastEvaluation, setPngFastEvaluation] = useState(false);
  const [pngForce, setPngForce] = useState(false);
  const [pngFixErrors, setPngFixErrors] = useState(true);

  // WebP options
  const [webpQuality, setWebpQuality] = useState(75);
  const [webpLossless, setWebpLossless] = useState(false);
  const [webpMethod, setWebpMethod] = useState(4);
  const [webpFilterStrength, setWebpFilterStrength] = useState(60);
  const [webpFilterSharpness, setWebpFilterSharpness] = useState(0);
  const [webpFilterType, setWebpFilterType] = useState(1);
  const [webpSnsStrength, setWebpSnsStrength] = useState(50);
  const [webpNearLossless, setWebpNearLossless] = useState(100);
  const [webpExact, setWebpExact] = useState(false);
  const [webpUseSharpYuv, setWebpUseSharpYuv] = useState(false);

  const [maxWidth, setMaxWidth] = useState<string>("");
  const [maxHeight, setMaxHeight] = useState<string>("");
  const [outputFormat, setOutputFormat] = useState("");
  const [exporting, setExporting] = useState(false);
  const [progress, setProgress] = useState<ExportProgress | null>(null);

  // --- Template Manager state ---
  const [showTemplateManager, setShowTemplateManager] = useState(false);

  // Sync external template manager trigger
  useEffect(() => {
    if (externalShowTM) {
      setShowTemplateManager(true);
      loadTemplates();
    }
  }, [externalShowTM]);
  const [tmSelectedTemplate, setTmSelectedTemplate] = useState<string | null>(null);
  const [tmFiles, setTmFiles] = useState<TemplateFileInfo[]>([]);
  const [tmSelectedFile, setTmSelectedFile] = useState<string | null>(null);
  const [tmFileContent, setTmFileContent] = useState("");
  const [tmDirty, setTmDirty] = useState(false);
  const [tmNewName, setTmNewName] = useState("");
  const [tmShowNew, setTmShowNew] = useState(false);

  // --- Data Loading ---

  useEffect(() => {
    loadTemplates();
  }, []);

  // Listen for progress events
  useEffect(() => {
    if (!isTauriRuntime()) return;

    const unlisten = onTauriEvent<ExportProgress>("epub-export-progress", (event) => {
      setProgress(event.payload);
    });
    return () => { unlisten.then(fn => fn()).catch(() => {}); };
  }, []);

  const loadTemplates = async () => {
    if (!isTauriRuntime()) {
      setTemplates([]);
      return;
    }

    try {
      const list = await listEpubTemplates<TemplateInfo[]>();
      setTemplates(list);
    } catch (e) { console.error(e); }
  };

  // --- Export ---

  const handleExport = async () => {
    if (selectedIds.size === 0) { showToast("作品を選択してください", "error"); return; }
    const parsedMaxWidth = maxWidth.trim() ? Number(maxWidth) : null;
    const parsedMaxHeight = maxHeight.trim() ? Number(maxHeight) : null;
    if (
      (parsedMaxWidth !== null && (!Number.isInteger(parsedMaxWidth) || parsedMaxWidth <= 0)) ||
      (parsedMaxHeight !== null && (!Number.isInteger(parsedMaxHeight) || parsedMaxHeight <= 0))
    ) {
      showToast("最大幅・最大高さは1以上の整数で入力してください", "error");
      return;
    }
    setExporting(true);
    const ids = Array.from(selectedIds);
    const compressOptions = compressEnabled ? {
      enabled: true,
      maxWidth: parsedMaxWidth,
      maxHeight: parsedMaxHeight,
      outputFormat: outputFormat || null,

      // JPEG
      jpegQuality,
      jpegProgressive,
      jpegChromaSubsampling,
      jpegAutoOptimize,
      jpegDeringing,
      jpegSeparateChromaTables,
      jpegSharpYuv,

      // PNG
      pngCompression,
      pngInterlace,
      pngStrip,
      pngOptimizeAlpha,
      pngBitDepthReduction,
      pngColorTypeReduction,
      pngPaletteReduction,
      pngGrayscaleReduction,
      pngIdatRecoding,
      pngFastEvaluation,
      pngForce,
      pngFixErrors,

      // WebP
      webpQuality,
      webpLossless,
      webpMethod,
      webpFilterStrength,
      webpFilterSharpness,
      webpFilterType,
      webpSnsStrength,
      webpNearLossless,
      webpExact,
      webpUseSharpYuv,
    } : {
      enabled: false,
      maxWidth: null,
      maxHeight: null,
      outputFormat: null,

      jpegQuality: 85,
      jpegProgressive: true,
      jpegChromaSubsampling: "4:2:0",
      jpegAutoOptimize: false,
      jpegDeringing: true,
      jpegSeparateChromaTables: true,
      jpegSharpYuv: false,

      pngCompression: "2",
      pngInterlace: false,
      pngStrip: true,
      pngOptimizeAlpha: false,
      pngBitDepthReduction: true,
      pngColorTypeReduction: true,
      pngPaletteReduction: true,
      pngGrayscaleReduction: true,
      pngIdatRecoding: true,
      pngFastEvaluation: false,
      pngForce: false,
      pngFixErrors: true,

      webpQuality: 75,
      webpLossless: false,
      webpMethod: 4,
      webpFilterStrength: 60,
      webpFilterSharpness: 0,
      webpFilterType: 1,
      webpSnsStrength: 50,
      webpNearLossless: 100,
      webpExact: false,
      webpUseSharpYuv: false,
    };

    try {
      if (ids.length === 1) {
        const filePath = await saveDialog({ filters: [{ name: "EPUB", extensions: ["epub"] }], defaultPath: `export.epub` });
        if (!filePath) { setExporting(false); return; }
        await exportEpub({ downloadId: ids[0], templateName: selectedTemplate, outputPath: filePath, compressOptions });
        showToast("EPUBエクスポート完了！", "success");
      } else {
        const dirPath = await openSingleDialog({ directory: true, title: "EPUB出力先フォルダを選択" });
        if (!dirPath) { setExporting(false); return; }
        const result = await exportEpubBatch<ExportBatchResult>({
          downloadIds: ids, templateName: selectedTemplate, outputDir: dirPath, compressOptions
        });
        showToast(`完了: ${result.successCount}件成功, ${result.failedCount}件失敗`, result.failedCount > 0 ? "error" : "success");
      }
    } catch (e: any) {
      showToast(`エラー: ${e}`, "error");
    } finally {
      setExporting(false);
      setProgress(null);
    }
  };

  // --- Template Manager ---

  const loadTmFiles = async (name: string) => {
    try {
      const files = await getTemplateFiles<TemplateFileInfo[]>(name);
      setTmFiles(files);
      setTmSelectedFile(null);
      setTmFileContent("");
      setTmDirty(false);
    } catch (e) { console.error(e); }
  };

  const loadTmFileContent = async (tplName: string, filename: string) => {
    try {
      const content = await readTemplateFile(tplName, filename);
      setTmFileContent(content);
      setTmDirty(false);
    } catch (e) { console.error(e); }
  };

  const saveTmFile = async () => {
    if (!tmSelectedTemplate || !tmSelectedFile) return;
    try {
      await saveTemplateFile(tmSelectedTemplate, tmSelectedFile, tmFileContent);
      setTmDirty(false);
      showToast("テンプレートを保存しました", "success");
    } catch (e: any) { showToast(`保存エラー: ${e}`, "error"); }
  };

  const createTemplate = async () => {
    if (!tmNewName.trim()) return;
    try {
      await createEpubTemplate(tmNewName.trim());
      setTmNewName(""); setTmShowNew(false);
      loadTemplates();
      showToast("テンプレートを作成しました", "success");
    } catch (e: any) { showToast(`作成エラー: ${e}`, "error"); }
  };

  const deleteTemplate = async (name: string) => {
    const isConfirmed = await askDialog(
      `テンプレート「${name}」を本当に削除しますか？\nこの操作は取り消せません。`,
      { title: "テンプレート削除の確認", kind: "warning", okLabel: "削除する", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;

    try {
      await deleteEpubTemplate(name);
      if (tmSelectedTemplate === name) { setTmSelectedTemplate(null); setTmFiles([]); }
      loadTemplates();
      showToast("テンプレートを削除しました", "success");
    } catch (e: any) { showToast(`削除エラー: ${e}`, "error"); }
  };

  const isBuiltin = (name: string) => templates.find(t => t.name === name)?.isBuiltin ?? false;

  // --- Render ---

  const progressPct = progress ? (progress.totalCount > 0 ? (progress.currentIndex / progress.totalCount) * 100 : 0) : 0;

  return (
    <>
      <aside className={`epub-sidebar ${isOpen ? 'open' : ''}`}>
        {/* Header */}
        <div className="epub-sidebar-header">
          <div className="epub-sidebar-title">
            <BookIcon />
            <span>EPUB エクスポート</span>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="epub-sidebar-close-btn"
            onClick={onClose}
            title="サイドバーを閉じる"
          >
            <PanelRightIcon />
          </Button>
        </div>

        {/* Export Settings */}
        <div className="epub-sidebar-body">
          <div className="epub-sidebar-section">
            <h4>エクスポート設定</h4>

            <div className="epub-section">
              <label className="epub-label">テンプレート</label>
              <Select value={selectedTemplate} onValueChange={setSelectedTemplate}>
                <SelectTrigger className="epub-select">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__auto__">自動判別 (ソースに応じて自動適用)</SelectItem>
                  {templates.map(t => <SelectItem key={t.name} value={t.name}>{t.name}{t.isBuiltin ? " (ビルトイン)" : ""}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>

            <div className="epub-section">
              <label className="epub-label">
                <Checkbox checked={compressEnabled} onCheckedChange={checked => setCompressEnabled(checked === true)} />
                画像圧縮を有効にする
              </label>
              {compressEnabled && (
                <div className="epub-compress-options">
                  <div className="epub-compress-desc">
                    画像の容量を削減し、EPUBのファイルサイズを軽量化します。
                  </div>

                  <Card className="epub-compress-section-card">
                    {/* JPEG Settings */}
                    <div className="epub-slider-row">
                      <div className="epub-slider-label-row">
                        <label>JPEG 品質</label>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min="10"
                            max="100"
                            value={jpegQuality}
                            onChange={e => setJpegQuality(Math.max(10, Math.min(100, Number(e.target.value))))}
                            className="epub-number-input"
                          />
                          <span className="text-xs text-muted-foreground">%</span>
                        </div>
                      </div>
                      <Slider min={10} max={100} value={[jpegQuality]} onValueChange={([next]) => setJpegQuality(next)} />
                    </div>

                    {/* WebP Settings */}
                    <div className="epub-slider-row">
                      <div className="epub-slider-label-row">
                        <label>WebP 品質</label>
                        <div className="flex items-center gap-1">
                          <Input
                            type="number"
                            min="10"
                            max="100"
                            value={webpQuality}
                            onChange={e => setWebpQuality(Math.max(10, Math.min(100, Number(e.target.value))))}
                            className="epub-number-input"
                          />
                          <span className="text-xs text-muted-foreground">%</span>
                        </div>
                      </div>
                      <Slider min={10} max={100} value={[webpQuality]} onValueChange={([next]) => setWebpQuality(next)} />
                    </div>

                    {/* PNG Settings */}
                    <div className="epub-slider-row flex-col items-stretch gap-1.5">
                      <label className="text-xs font-semibold text-muted-foreground">PNG 圧縮レベル</label>
                      <Select
                        value={pngCompression}
                        onValueChange={setPngCompression}
                      >
                        <SelectTrigger className="epub-select-sm w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="0">レベル 0 (圧縮なし)</SelectItem>
                          <SelectItem value="1">レベル 1 (高速・低圧縮)</SelectItem>
                          <SelectItem value="2">レベル 2 (推奨・標準)</SelectItem>
                          <SelectItem value="3">レベル 3 (バランス)</SelectItem>
                          <SelectItem value="4">レベル 4 (高圧縮)</SelectItem>
                          <SelectItem value="5">レベル 5 (超高圧縮)</SelectItem>
                          <SelectItem value="6">レベル 6 (最大圧縮・低速)</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  </Card>





                  <div className="mt-3 border-t border-border pt-3">
                    <Button
                      type="button"
                      variant={showAdvanced ? "secondary" : "outline"}
                      size="sm"
                      className={cn("filter-toggle-btn w-full justify-between px-2 py-1.5 text-sm", showAdvanced && 'expanded')}
                      onClick={() => setShowAdvanced(!showAdvanced)}
                    >
                      <span className="flex items-center gap-1.5 font-semibold">
                        <Settings2 size={14} />
                        高度な詳細設定を表示する
                      </span>
                      <span className={`toggle-arrow ${showAdvanced ? 'up' : 'down'}`} />
                    </Button>

                    {showAdvanced && (
                      <div className="epub-compress-advanced-panel mt-3 flex flex-col gap-3 pl-1">
                        {/* Image Resize & Format Settings */}
                        <Card className="epub-compress-section-card advanced-card">
                          <h5 className="mb-2 text-xs font-bold tracking-wide text-muted-foreground">画像リサイズ・フォーマット変換</h5>
                          <div className="two-cols gap-2">
                            <div className="epub-input-row-vertical">
                              <label className="text-xs text-muted-foreground">最大幅 (px)</label>
                              <Input type="number" min="1" step="1" value={maxWidth} onChange={e => setMaxWidth(e.target.value)} placeholder="制限なし" />
                            </div>
                            <div className="epub-input-row-vertical">
                              <label className="text-xs text-muted-foreground">最大高さ (px)</label>
                              <Input type="number" min="1" step="1" value={maxHeight} onChange={e => setMaxHeight(e.target.value)} placeholder="制限なし" />
                            </div>
                          </div>
                          <div className="epub-option-tip mb-2 mt-1">
                            指定したピクセル数を超える画像がある場合、アスペクト比を維持したまま縮小します。空欄で制限なしとなります。
                          </div>
                          <div className="epub-input-row flex-col items-stretch gap-1">
                            <label className="text-xs font-semibold text-muted-foreground">出力アセット形式</label>
                            <Select value={outputFormat || "__original__"} onValueChange={value => setOutputFormat(value === "__original__" ? "" : value)}>
                              <SelectTrigger className="epub-select-sm w-full">
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                <SelectItem value="__original__">元の形式を維持</SelectItem>
                                <SelectItem value="jpeg">すべての画像を JPEG に強制変換</SelectItem>
                                <SelectItem value="png">すべての画像を PNG に強制変換</SelectItem>
                                <SelectItem value="webp">すべての画像を WebP に強制変換</SelectItem>
                              </SelectContent>
                            </Select>
                          </div>
                          <div className="epub-option-tip mt-1">
                            EPUB内の画像を特定のフォーマットに統一します。WebPに変換するとファイルサイズを大幅に削減できます。
                          </div>
                        </Card>
                        {/* JPEG Advanced */}
                        <Card className="epub-compress-section-card advanced-card">
                          <h5 className="mb-2 text-xs font-bold tracking-wide text-muted-foreground">JPEG 詳細</h5>
                          <div className="flex flex-col gap-2">
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={jpegProgressive} onCheckedChange={checked => setJpegProgressive(checked === true)} />
                              <span>プログレッシブエンコード</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={jpegAutoOptimize} onCheckedChange={checked => setJpegAutoOptimize(checked === true)} />
                              <span>トレリス最適化 (自動テーブル最適化)</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={jpegDeringing} onCheckedChange={checked => setJpegDeringing(checked === true)} />
                              <span>リンギングノイズ低減 (Deringing)</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={jpegSeparateChromaTables} onCheckedChange={checked => setJpegSeparateChromaTables(checked === true)} />
                              <span>カラー量子化テーブルの分離</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={jpegSharpYuv} onCheckedChange={checked => setJpegSharpYuv(checked === true)} />
                              <span>SharpYUV ダウンサンプリング</span>
                            </label>
                            <div className="epub-input-row mt-1 flex-col items-stretch gap-1">
                              <label className="text-xs text-muted-foreground">クロマサブサンプリング</label>
                              <Select value={jpegChromaSubsampling} onValueChange={setJpegChromaSubsampling}>
                                <SelectTrigger className="epub-select-sm w-full">
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  <SelectItem value="4:2:0">4:2:0 (標準・最高圧縮)</SelectItem>
                                  <SelectItem value="4:2:2">4:2:2 (バランス)</SelectItem>
                                  <SelectItem value="4:4:4">4:4:4 (無劣化ダウンサンプリング・高画質)</SelectItem>
                                </SelectContent>
                              </Select>
                            </div>
                          </div>
                        </Card>

                        {/* PNG Advanced */}
                        <Card className="epub-compress-section-card advanced-card">
                          <h5 className="mb-2 text-xs font-bold tracking-wide text-muted-foreground">PNG 詳細</h5>
                          <div className="mb-2 flex flex-col gap-2 border-b border-dashed border-border pb-2">
                            <div>
                              <label className="epub-compress-option-checkbox-row">
                                <Checkbox
                                  checked={pngInterlace}
                                  onCheckedChange={checked => setPngInterlace(checked === true)}
                                />
                                <span>インターレース表示を有効にする (段階表示)</span>
                              </label>
                              <div className="epub-option-tip ml-5 mt-0.5">
                                画像を読み込みながら徐々に鮮明に表示させます（リーダー側の対応が必要です）。
                              </div>
                            </div>
                            <div>
                              <label className="epub-compress-option-checkbox-row">
                                <Checkbox
                                  checked={pngStrip}
                                  onCheckedChange={checked => setPngStrip(checked === true)}
                                />
                                <span>メタデータを削除する (Strip)</span>
                              </label>
                              <div className="epub-option-tip ml-5 mt-0.5">
                                撮影情報（Exifなど）や不要な色プロファイルを削除して軽量化します（画質への影響なし）。
                              </div>
                            </div>
                          </div>
                          <div className="flex flex-wrap gap-2">
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngOptimizeAlpha} onCheckedChange={checked => setPngOptimizeAlpha(checked === true)} />
                              <span>透過色の最適化</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngBitDepthReduction} onCheckedChange={checked => setPngBitDepthReduction(checked === true)} />
                              <span>ビット深度の削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngColorTypeReduction} onCheckedChange={checked => setPngColorTypeReduction(checked === true)} />
                              <span>カラータイプの削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngPaletteReduction} onCheckedChange={checked => setPngPaletteReduction(checked === true)} />
                              <span>パレットカラーの削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngGrayscaleReduction} onCheckedChange={checked => setPngGrayscaleReduction(checked === true)} />
                              <span>グレースケール削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngIdatRecoding} onCheckedChange={checked => setPngIdatRecoding(checked === true)} />
                              <span>IDAT再エンコード</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngFastEvaluation} onCheckedChange={checked => setPngFastEvaluation(checked === true)} />
                              <span>高速評価モード</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngForce} onCheckedChange={checked => setPngForce(checked === true)} />
                              <span>強制書き出し</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row flex-1 basis-[45%]">
                              <Checkbox checked={pngFixErrors} onCheckedChange={checked => setPngFixErrors(checked === true)} />
                              <span>破損画像の修復</span>
                            </label>
                          </div>
                        </Card>

                        {/* WebP Advanced */}
                        <Card className="epub-compress-section-card advanced-card">
                          <h5 className="mb-2 text-xs font-bold tracking-wide text-muted-foreground">WebP 詳細</h5>
                          <div className="flex flex-col gap-2">
                            <div className="mb-1 border-b border-dashed border-border pb-2">
                              <label className="epub-compress-option-checkbox-row">
                                <Checkbox
                                  checked={webpLossless}
                                  onCheckedChange={checked => setWebpLossless(checked === true)}
                                />
                                <span>WebP可逆圧縮 (ロスレス・無劣化)</span>
                              </label>
                              <div className="epub-option-tip ml-5 mt-0.5">
                                イラスト等の画質を一切劣化させずに圧縮します。有効時、上記の「WebP品質」は無視されます。
                              </div>
                            </div>
                            <div className="epub-slider-row m-0">
                              <div className="epub-slider-label-row">
                                <label className="text-xs">圧縮速度 (Method)</label>
                                <span className="text-xs font-bold">{webpMethod}</span>
                              </div>
                              <Slider min={0} max={6} value={[webpMethod]} onValueChange={([next]) => setWebpMethod(next)} />
                            </div>
                            <div className="epub-slider-row m-0">
                              <div className="epub-slider-label-row">
                                <label className="text-xs">デブロッキング強度</label>
                                <span className="text-xs font-bold">{webpFilterStrength}</span>
                              </div>
                              <Slider min={0} max={100} value={[webpFilterStrength]} onValueChange={([next]) => setWebpFilterStrength(next)} />
                            </div>
                            <div className="epub-slider-row m-0">
                              <div className="epub-slider-label-row">
                                <label className="text-xs">デブロッキング鋭度</label>
                                <span className="text-xs font-bold">{webpFilterSharpness}</span>
                              </div>
                              <Slider min={0} max={7} value={[webpFilterSharpness]} onValueChange={([next]) => setWebpFilterSharpness(next)} />
                            </div>
                            <div className="epub-slider-row m-0">
                              <div className="epub-slider-label-row">
                                <label className="text-xs">空間ノイズ強度 (SNS)</label>
                                <span className="text-xs font-bold">{webpSnsStrength}</span>
                              </div>
                              <Slider min={0} max={100} value={[webpSnsStrength]} onValueChange={([next]) => setWebpSnsStrength(next)} />
                            </div>
                            <div className="epub-slider-row m-0">
                              <div className="epub-slider-label-row">
                                <label className="text-xs">準ロスレス強度</label>
                                <span className="text-xs font-bold">{webpNearLossless}</span>
                              </div>
                              <Slider min={0} max={100} value={[webpNearLossless]} onValueChange={([next]) => setWebpNearLossless(next)} />
                            </div>
                            <div className="epub-input-row flex-col items-stretch gap-1">
                              <label className="text-xs text-muted-foreground">フィルタータイプ</label>
                              <Select value={String(webpFilterType)} onValueChange={value => setWebpFilterType(Number(value))}>
                                <SelectTrigger className="epub-select-sm w-full">
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  <SelectItem value="0">シンプル (高速)</SelectItem>
                                  <SelectItem value="1">ストロング (高画質・輪郭維持)</SelectItem>
                                </SelectContent>
                              </Select>
                            </div>
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={webpExact} onCheckedChange={checked => setWebpExact(checked === true)} />
                              <span>透過RGB値を維持 (Exact)</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <Checkbox checked={webpUseSharpYuv} onCheckedChange={checked => setWebpUseSharpYuv(checked === true)} />
                              <span>高精度カラー変換 (SharpYUV)</span>
                            </label>
                          </div>
                        </Card>
                      </div>
                    )}
                  </div>
                </div>
              )}
            </div>
          </div>

          {/* 自動判定のグループプレビュー */}
          {autoGroups && (
            <div className="epub-auto-preview-container">
              <div className="epub-preview-title">自動判別プレビュー</div>
              
              {autoGroups.pixiv.length > 0 && (
                <div className="epub-preview-group">
                  <div className="epub-preview-group-header">
                    <span className="epub-preview-group-name">pixiv テンプレート</span>
                    <span className="epub-preview-group-count">{autoGroups.pixiv.length} 件</span>
                  </div>
                  <div className="epub-preview-item-list">
                    {autoGroups.pixiv.map(item => (
                      <div key={item.id} className="epub-preview-item">
                        <Badge className="epub-preview-source-tag pixiv">Pixiv</Badge>
                        <span className="epub-preview-item-title" title={item.title}>{item.title}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {autoGroups.default.length > 0 && (
                <div className="epub-preview-group">
                  <div className="epub-preview-group-header">
                    <span className="epub-preview-group-name">default テンプレート</span>
                    <span className="epub-preview-group-count">{autoGroups.default.length} 件</span>
                  </div>
                  <div className="epub-preview-item-list">
                    {autoGroups.default.map(item => (
                      <div key={item.id} className="epub-preview-item">
                        <Badge className="epub-preview-source-tag fanbox">FANBOX</Badge>
                        <span className="epub-preview-item-title" title={item.title}>{item.title}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* Progress */}
          {exporting && progress && (
            <div className="epub-progress-section">
              <div className="epub-progress">
                <div className="epub-progress-bar" style={{ width: `${progressPct}%` }} />
              </div>
              <div className="epub-progress-text">{progress.message}</div>
            </div>
          )}

          {/* Export Button */}
          <Button type="button" className="epub-export-action-btn" onClick={handleExport} disabled={exporting || selectedIds.size === 0}>
            {exporting ? "エクスポート中..." : `${selectedIds.size > 1 ? `${selectedIds.size}件を` : ""}EPUBにエクスポート`}
          </Button>
        </div>

      </aside>

      {/* Template Manager Modal */}
      {showTemplateManager && (
        <div className="template-manager-modal-overlay" onClick={() => { setShowTemplateManager(false); onCloseTemplateManager(); }}>
          <div className="template-manager-modal" onClick={e => e.stopPropagation()}>
            <div className="template-manager-modal-header">
              <h3>テンプレート管理</h3>
              <Button type="button" variant="ghost" size="icon" className="template-manager-close-btn" onClick={() => { setShowTemplateManager(false); onCloseTemplateManager(); }}>✕</Button>
            </div>
            <div className="template-manager-body">
              {/* Pane 1: Template List */}
              <div className="template-list-panel">
                <div className="template-list-header">
                  <span>テンプレート</span>
                  <Button type="button" variant="outline" size="icon" className="template-add-btn p-1" onClick={() => setTmShowNew(!tmShowNew)}>
                    <PlusIcon />
                  </Button>
                </div>
                {tmShowNew && (
                  <div className="template-new-form">
                    <Input value={tmNewName} onChange={e => setTmNewName(e.target.value)} placeholder="テンプレート名" onKeyDown={e => e.key === "Enter" && createTemplate()} />
                    <Button type="button" variant="outline" size="sm" className="gap-1" onClick={createTemplate}>
                      <PlusIcon />
                      <span>作成</span>
                    </Button>
                  </div>
                )}
                <div className="template-list-items">
                  {templates.map(t => (
                    <div key={t.name} className={`template-list-item ${tmSelectedTemplate === t.name ? "selected" : ""}`}
                      onClick={() => { setTmSelectedTemplate(t.name); loadTmFiles(t.name); }}>
                      <div className="template-item-info">
                        <span className="template-item-name">{t.name}</span>
                        {t.isBuiltin && <Badge variant="secondary" className="template-builtin-badge">BUILTIN</Badge>}
                        <span className="template-file-count">{t.fileCount} ファイル</span>
                      </div>
                      {!t.isBuiltin && (
                        <Button type="button" variant="ghost" size="icon" className="template-delete-btn" onClick={e => { e.stopPropagation(); deleteTemplate(t.name); }}>
                          <TrashIcon />
                        </Button>
                      )}
                    </div>
                  ))}
                </div>
              </div>
              {/* Pane 2: File List */}
              <div className="template-files-panel">
                <div className="template-files-header">{tmSelectedTemplate ? `${tmSelectedTemplate}/` : "ファイル"}</div>
                {tmSelectedTemplate ? (
                  <div className="template-files-list">
                    {tmFiles.map(f => (
                      <div key={f.filename} className={`template-file-item ${tmSelectedFile === f.filename ? "selected" : ""}`}
                        onClick={() => { setTmSelectedFile(f.filename); loadTmFileContent(tmSelectedTemplate, f.filename); }}>
                        <span className="template-file-name">{f.filename}</span>
                        <span className="template-file-size">{(f.sizeBytes / 1024).toFixed(1)}KB</span>
                      </div>
                    ))}
                  </div>
                ) : <div className="template-files-empty">テンプレートを選択</div>}
              </div>
              {/* Pane 3: Editor */}
              <div className="template-editor-panel">
                {tmSelectedFile ? (<>
                  <div className="template-editor-header">
                    <span>{tmSelectedFile} {tmDirty && <Badge variant="secondary" className="unsaved-badge">未保存</Badge>}</span>
                    <Button type="button" variant="outline" size="sm" className="template-save-btn gap-1.5" disabled={!tmDirty || isBuiltin(tmSelectedTemplate!)} onClick={saveTmFile}>
                      <SaveIcon />
                      <span>保存</span>
                    </Button>
                  </div>
                  <Textarea className="template-editor-textarea" value={tmFileContent}
                    onChange={e => { setTmFileContent(e.target.value); setTmDirty(true); }}
                    readOnly={isBuiltin(tmSelectedTemplate!)} spellCheck={false} />
                  {isBuiltin(tmSelectedTemplate!) && <div className="template-editor-readonly-notice">⚠ ビルトインテンプレートは読み取り専用です。編集するには「＋」で複製してください。</div>}
                </>) : <div className="template-editor-empty">ファイルを選択してください</div>}
              </div>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
