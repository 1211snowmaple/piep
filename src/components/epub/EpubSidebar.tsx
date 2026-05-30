import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save, open, ask } from "@tauri-apps/plugin-dialog";
import { PlusIcon, SaveIcon, TrashIcon, BookIcon, PanelRightIcon } from "../icons/Icons";

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

    const unlisten = listen<ExportProgress>("epub-export-progress", (event) => {
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
      const list = await invoke<TemplateInfo[]>("list_epub_templates");
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
        const filePath = await save({ filters: [{ name: "EPUB", extensions: ["epub"] }], defaultPath: `export.epub` });
        if (!filePath) { setExporting(false); return; }
        await invoke("export_epub", { downloadId: ids[0], templateName: selectedTemplate, outputPath: filePath, compressOptions });
        showToast("EPUBエクスポート完了！", "success");
      } else {
        const dirPath = await open({ directory: true, title: "EPUB出力先フォルダを選択" });
        if (!dirPath) { setExporting(false); return; }
        const result = await invoke<ExportBatchResult>("export_epub_batch", {
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
      const files = await invoke<TemplateFileInfo[]>("get_template_files", { templateName: name });
      setTmFiles(files);
      setTmSelectedFile(null);
      setTmFileContent("");
      setTmDirty(false);
    } catch (e) { console.error(e); }
  };

  const loadTmFileContent = async (tplName: string, filename: string) => {
    try {
      const content = await invoke<string>("read_template_file", { templateName: tplName, filename });
      setTmFileContent(content);
      setTmDirty(false);
    } catch (e) { console.error(e); }
  };

  const saveTmFile = async () => {
    if (!tmSelectedTemplate || !tmSelectedFile) return;
    try {
      await invoke("save_template_file", { templateName: tmSelectedTemplate, filename: tmSelectedFile, content: tmFileContent });
      setTmDirty(false);
      showToast("テンプレートを保存しました", "success");
    } catch (e: any) { showToast(`保存エラー: ${e}`, "error"); }
  };

  const createTemplate = async () => {
    if (!tmNewName.trim()) return;
    try {
      await invoke("create_epub_template", { templateName: tmNewName.trim() });
      setTmNewName(""); setTmShowNew(false);
      loadTemplates();
      showToast("テンプレートを作成しました", "success");
    } catch (e: any) { showToast(`作成エラー: ${e}`, "error"); }
  };

  const deleteTemplate = async (name: string) => {
    const isConfirmed = await ask(
      `テンプレート「${name}」を本当に削除しますか？\nこの操作は取り消せません。`,
      { title: "テンプレート削除の確認", kind: "warning", okLabel: "削除する", cancelLabel: "キャンセル" }
    );
    if (!isConfirmed) return;

    try {
      await invoke("delete_epub_template", { templateName: name });
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
          <button
            className="epub-sidebar-close-btn"
            onClick={onClose}
            title="サイドバーを閉じる"
          >
            <PanelRightIcon />
          </button>
        </div>

        {/* Export Settings */}
        <div className="epub-sidebar-body">
          <div className="epub-sidebar-section">
            <h4>エクスポート設定</h4>

            <div className="epub-section">
              <label className="epub-label">テンプレート</label>
              <select className="epub-select" value={selectedTemplate} onChange={e => setSelectedTemplate(e.target.value)}>
                <option value="__auto__">自動判別 (ソースに応じて自動適用)</option>
                {templates.map(t => <option key={t.name} value={t.name}>{t.name}{t.isBuiltin ? " (ビルトイン)" : ""}</option>)}
              </select>
            </div>

            <div className="epub-section">
              <label className="epub-label">
                <input type="checkbox" checked={compressEnabled} onChange={e => setCompressEnabled(e.target.checked)} />
                画像圧縮を有効にする
              </label>
              {compressEnabled && (
                <div className="epub-compress-options">
                  <div className="epub-compress-desc">
                    画像の容量を削減し、EPUBのファイルサイズを軽量化します。
                  </div>

                  <div className="epub-compress-section-card">
                    {/* JPEG Settings */}
                    <div className="epub-slider-row">
                      <div className="epub-slider-label-row">
                        <label>JPEG 品質</label>
                        <div style={{ display: 'flex', alignItems: 'center', gap: '0.25rem' }}>
                          <input
                            type="number"
                            min="10"
                            max="100"
                            value={jpegQuality}
                            onChange={e => setJpegQuality(Math.max(10, Math.min(100, Number(e.target.value))))}
                            className="epub-number-input"
                          />
                          <span style={{ fontSize: '0.75rem', color: 'var(--color-text-muted)' }}>%</span>
                        </div>
                      </div>
                      <input type="range" min="10" max="100" value={jpegQuality} onChange={e => setJpegQuality(Number(e.target.value))} />
                    </div>

                    {/* WebP Settings */}
                    <div className="epub-slider-row">
                      <div className="epub-slider-label-row">
                        <label>WebP 品質</label>
                        <div style={{ display: 'flex', alignItems: 'center', gap: '0.25rem' }}>
                          <input
                            type="number"
                            min="10"
                            max="100"
                            value={webpQuality}
                            onChange={e => setWebpQuality(Math.max(10, Math.min(100, Number(e.target.value))))}
                            className="epub-number-input"
                          />
                          <span style={{ fontSize: '0.75rem', color: 'var(--color-text-muted)' }}>%</span>
                        </div>
                      </div>
                      <input type="range" min="10" max="100" value={webpQuality} onChange={e => setWebpQuality(Number(e.target.value))} />
                    </div>

                    {/* PNG Settings */}
                    <div className="epub-slider-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '0.35rem' }}>
                      <label style={{ fontSize: '0.75rem', fontWeight: 600, color: 'var(--color-text-secondary)' }}>PNG 圧縮レベル</label>
                      <select
                        value={pngCompression}
                        onChange={e => setPngCompression(e.target.value)}
                        className="epub-select-sm"
                        style={{ width: '100%' }}
                      >
                        <option value="0">レベル 0 (圧縮なし)</option>
                        <option value="1">レベル 1 (高速・低圧縮)</option>
                        <option value="2">レベル 2 (推奨・標準)</option>
                        <option value="3">レベル 3 (バランス)</option>
                        <option value="4">レベル 4 (高圧縮)</option>
                        <option value="5">レベル 5 (超高圧縮)</option>
                        <option value="6">レベル 6 (最大圧縮・低速)</option>
                      </select>
                    </div>
                  </div>





                  <div style={{ marginTop: '0.75rem', borderTop: '1px solid var(--color-border)', paddingTop: '0.75rem' }}>
                    <button
                      type="button"
                      className={`filter-toggle-btn ${showAdvanced ? 'expanded' : ''}`}
                      onClick={() => setShowAdvanced(!showAdvanced)}
                      style={{ width: '100%', justifyContent: 'space-between', padding: '0.4rem 0.5rem', fontSize: '0.85rem' }}
                    >
                      <span style={{ display: 'flex', alignItems: 'center', gap: '0.4rem', fontWeight: 600 }}>
                        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                          <circle cx="12" cy="12" r="3"></circle>
                          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
                        </svg>
                        高度な詳細設定を表示する
                      </span>
                      <span className={`toggle-arrow ${showAdvanced ? 'up' : 'down'}`} />
                    </button>

                    {showAdvanced && (
                      <div className="epub-compress-advanced-panel" style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem', marginTop: '0.75rem', paddingLeft: '0.25rem' }}>
                        {/* Image Resize & Format Settings */}
                        <div className="epub-compress-section-card advanced-card">
                          <h5 style={{ margin: '0 0 0.5rem 0', fontSize: '0.8rem', fontWeight: 700, color: 'var(--color-text-secondary)', letterSpacing: '0.05em' }}>画像リサイズ・フォーマット変換</h5>
                          <div className="two-cols" style={{ gap: '0.5rem' }}>
                            <div className="epub-input-row-vertical">
                              <label style={{ fontSize: '0.75rem', color: 'var(--color-text-secondary)' }}>最大幅 (px)</label>
                              <input type="number" min="1" step="1" value={maxWidth} onChange={e => setMaxWidth(e.target.value)} placeholder="制限なし" />
                            </div>
                            <div className="epub-input-row-vertical">
                              <label style={{ fontSize: '0.75rem', color: 'var(--color-text-secondary)' }}>最大高さ (px)</label>
                              <input type="number" min="1" step="1" value={maxHeight} onChange={e => setMaxHeight(e.target.value)} placeholder="制限なし" />
                            </div>
                          </div>
                          <div className="epub-option-tip" style={{ marginTop: '0.25rem', marginBottom: '0.5rem' }}>
                            指定したピクセル数を超える画像がある場合、アスペクト比を維持したまま縮小します。空欄で制限なしとなります。
                          </div>
                          <div className="epub-input-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '0.15rem' }}>
                            <label style={{ fontSize: '0.75rem', color: 'var(--color-text-secondary)', fontWeight: 600 }}>出力アセット形式</label>
                            <select value={outputFormat} onChange={e => setOutputFormat(e.target.value)} className="epub-select-sm" style={{ width: '100%' }}>
                              <option value="">元の形式を維持</option>
                              <option value="jpeg">すべての画像を JPEG に強制変換</option>
                              <option value="png">すべての画像を PNG に強制変換</option>
                              <option value="webp">すべての画像を WebP に強制変換</option>
                            </select>
                          </div>
                          <div className="epub-option-tip" style={{ marginTop: '0.25rem' }}>
                            EPUB内の画像を特定のフォーマットに統一します。WebPに変換するとファイルサイズを大幅に削減できます。
                          </div>
                        </div>
                        {/* JPEG Advanced */}
                        <div className="epub-compress-section-card advanced-card">
                          <h5 style={{ margin: '0 0 0.5rem 0', fontSize: '0.8rem', fontWeight: 700, color: 'var(--color-text-secondary)', letterSpacing: '0.05em' }}>JPEG 詳細</h5>
                          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={jpegProgressive} onChange={e => setJpegProgressive(e.target.checked)} />
                              <span>プログレッシブエンコード</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={jpegAutoOptimize} onChange={e => setJpegAutoOptimize(e.target.checked)} />
                              <span>トレリス最適化 (自動テーブル最適化)</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={jpegDeringing} onChange={e => setJpegDeringing(e.target.checked)} />
                              <span>リンギングノイズ低減 (Deringing)</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={jpegSeparateChromaTables} onChange={e => setJpegSeparateChromaTables(e.target.checked)} />
                              <span>カラー量子化テーブルの分離</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={jpegSharpYuv} onChange={e => setJpegSharpYuv(e.target.checked)} />
                              <span>SharpYUV ダウンサンプリング</span>
                            </label>
                            <div className="epub-input-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '0.15rem', marginTop: '0.25rem' }}>
                              <label style={{ fontSize: '0.75rem', color: 'var(--color-text-muted)' }}>クロマサブサンプリング</label>
                              <select value={jpegChromaSubsampling} onChange={e => setJpegChromaSubsampling(e.target.value)} className="epub-select-sm" style={{ width: '100%' }}>
                                <option value="4:2:0">4:2:0 (標準・最高圧縮)</option>
                                <option value="4:2:2">4:2:2 (バランス)</option>
                                <option value="4:4:4">4:4:4 (無劣化ダウンサンプリング・高画質)</option>
                              </select>
                            </div>
                          </div>
                        </div>

                        {/* PNG Advanced */}
                        <div className="epub-compress-section-card advanced-card">
                          <h5 style={{ margin: '0 0 0.5rem 0', fontSize: '0.8rem', fontWeight: 700, color: 'var(--color-text-secondary)', letterSpacing: '0.05em' }}>PNG 詳細</h5>
                          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.5rem', marginBottom: '0.5rem', borderBottom: '1px dashed var(--color-border)', paddingBottom: '0.5rem' }}>
                            <div>
                              <label className="epub-compress-option-checkbox-row">
                                <input
                                  type="checkbox"
                                  checked={pngInterlace}
                                  onChange={e => setPngInterlace(e.target.checked)}
                                />
                                <span>インターレース表示を有効にする (段階表示)</span>
                              </label>
                              <div className="epub-option-tip" style={{ marginTop: '0.1rem', marginLeft: '1.25rem' }}>
                                画像を読み込みながら徐々に鮮明に表示させます（リーダー側の対応が必要です）。
                              </div>
                            </div>
                            <div>
                              <label className="epub-compress-option-checkbox-row">
                                <input
                                  type="checkbox"
                                  checked={pngStrip}
                                  onChange={e => setPngStrip(e.target.checked)}
                                />
                                <span>メタデータを削除する (Strip)</span>
                              </label>
                              <div className="epub-option-tip" style={{ marginTop: '0.1rem', marginLeft: '1.25rem' }}>
                                撮影情報（Exifなど）や不要な色プロファイルを削除して軽量化します（画質への影響なし）。
                              </div>
                            </div>
                          </div>
                          <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5rem' }}>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngOptimizeAlpha} onChange={e => setPngOptimizeAlpha(e.target.checked)} />
                              <span>透過色の最適化</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngBitDepthReduction} onChange={e => setPngBitDepthReduction(e.target.checked)} />
                              <span>ビット深度の削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngColorTypeReduction} onChange={e => setPngColorTypeReduction(e.target.checked)} />
                              <span>カラータイプの削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngPaletteReduction} onChange={e => setPngPaletteReduction(e.target.checked)} />
                              <span>パレットカラーの削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngGrayscaleReduction} onChange={e => setPngGrayscaleReduction(e.target.checked)} />
                              <span>グレースケール削減</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngIdatRecoding} onChange={e => setPngIdatRecoding(e.target.checked)} />
                              <span>IDAT再エンコード</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngFastEvaluation} onChange={e => setPngFastEvaluation(e.target.checked)} />
                              <span>高速評価モード</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngForce} onChange={e => setPngForce(e.target.checked)} />
                              <span>強制書き出し</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row" style={{ flex: '1 1 45%' }}>
                              <input type="checkbox" checked={pngFixErrors} onChange={e => setPngFixErrors(e.target.checked)} />
                              <span>破損画像の修復</span>
                            </label>
                          </div>
                        </div>

                        {/* WebP Advanced */}
                        <div className="epub-compress-section-card advanced-card">
                          <h5 style={{ margin: '0 0 0.5rem 0', fontSize: '0.8rem', fontWeight: 700, color: 'var(--color-text-secondary)', letterSpacing: '0.05em' }}>WebP 詳細</h5>
                          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.5rem' }}>
                            <div style={{ borderBottom: '1px dashed var(--color-border)', paddingBottom: '0.5rem', marginBottom: '0.25rem' }}>
                              <label className="epub-compress-option-checkbox-row">
                                <input
                                  type="checkbox"
                                  checked={webpLossless}
                                  onChange={e => setWebpLossless(e.target.checked)}
                                />
                                <span>WebP可逆圧縮 (ロスレス・無劣化)</span>
                              </label>
                              <div className="epub-option-tip" style={{ marginTop: '0.1rem', marginLeft: '1.25rem' }}>
                                イラスト等の画質を一切劣化させずに圧縮します。有効時、上記の「WebP品質」は無視されます。
                              </div>
                            </div>
                            <div className="epub-slider-row" style={{ margin: 0 }}>
                              <div className="epub-slider-label-row">
                                <label style={{ fontSize: '0.75rem' }}>圧縮速度 (Method)</label>
                                <span style={{ fontSize: '0.75rem', fontWeight: 'bold' }}>{webpMethod}</span>
                              </div>
                              <input type="range" min="0" max="6" value={webpMethod} onChange={e => setWebpMethod(Number(e.target.value))} />
                            </div>
                            <div className="epub-slider-row" style={{ margin: 0 }}>
                              <div className="epub-slider-label-row">
                                <label style={{ fontSize: '0.75rem' }}>デブロッキング強度</label>
                                <span style={{ fontSize: '0.75rem', fontWeight: 'bold' }}>{webpFilterStrength}</span>
                              </div>
                              <input type="range" min="0" max="100" value={webpFilterStrength} onChange={e => setWebpFilterStrength(Number(e.target.value))} />
                            </div>
                            <div className="epub-slider-row" style={{ margin: 0 }}>
                              <div className="epub-slider-label-row">
                                <label style={{ fontSize: '0.75rem' }}>デブロッキング鋭度</label>
                                <span style={{ fontSize: '0.75rem', fontWeight: 'bold' }}>{webpFilterSharpness}</span>
                              </div>
                              <input type="range" min="0" max="7" value={webpFilterSharpness} onChange={e => setWebpFilterSharpness(Number(e.target.value))} />
                            </div>
                            <div className="epub-slider-row" style={{ margin: 0 }}>
                              <div className="epub-slider-label-row">
                                <label style={{ fontSize: '0.75rem' }}>空間ノイズ強度 (SNS)</label>
                                <span style={{ fontSize: '0.75rem', fontWeight: 'bold' }}>{webpSnsStrength}</span>
                              </div>
                              <input type="range" min="0" max="100" value={webpSnsStrength} onChange={e => setWebpSnsStrength(Number(e.target.value))} />
                            </div>
                            <div className="epub-slider-row" style={{ margin: 0 }}>
                              <div className="epub-slider-label-row">
                                <label style={{ fontSize: '0.75rem' }}>準ロスレス強度</label>
                                <span style={{ fontSize: '0.75rem', fontWeight: 'bold' }}>{webpNearLossless}</span>
                              </div>
                              <input type="range" min="0" max="100" value={webpNearLossless} onChange={e => setWebpNearLossless(Number(e.target.value))} />
                            </div>
                            <div className="epub-input-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '0.15rem' }}>
                              <label style={{ fontSize: '0.75rem', color: 'var(--color-text-muted)' }}>フィルタータイプ</label>
                              <select value={webpFilterType} onChange={e => setWebpFilterType(Number(e.target.value))} className="epub-select-sm" style={{ width: '100%' }}>
                                <option value={0}>シンプル (高速)</option>
                                <option value={1}>ストロング (高画質・輪郭維持)</option>
                              </select>
                            </div>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={webpExact} onChange={e => setWebpExact(e.target.checked)} />
                              <span>透過RGB値を維持 (Exact)</span>
                            </label>
                            <label className="epub-compress-option-checkbox-row">
                              <input type="checkbox" checked={webpUseSharpYuv} onChange={e => setWebpUseSharpYuv(e.target.checked)} />
                              <span>高精度カラー変換 (SharpYUV)</span>
                            </label>
                          </div>
                        </div>
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
                        <span className="epub-preview-source-tag pixiv">Pixiv</span>
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
                        <span className="epub-preview-source-tag fanbox">FANBOX</span>
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
          <button className="epub-export-action-btn" onClick={handleExport} disabled={exporting || selectedIds.size === 0}>
            {exporting ? "エクスポート中..." : `${selectedIds.size > 1 ? `${selectedIds.size}件を` : ""}EPUBにエクスポート`}
          </button>
        </div>

      </aside>

      {/* Template Manager Modal */}
      {showTemplateManager && (
        <div className="template-manager-modal-overlay" onClick={() => { setShowTemplateManager(false); onCloseTemplateManager(); }}>
          <div className="template-manager-modal" onClick={e => e.stopPropagation()}>
            <div className="template-manager-modal-header">
              <h3>テンプレート管理</h3>
              <button className="template-manager-close-btn" onClick={() => { setShowTemplateManager(false); onCloseTemplateManager(); }}>✕</button>
            </div>
            <div className="template-manager-body">
              {/* Pane 1: Template List */}
              <div className="template-list-panel">
                <div className="template-list-header">
                  <span>テンプレート</span>
                  <button className="template-add-btn" onClick={() => setTmShowNew(!tmShowNew)} style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '0.2rem' }}>
                    <PlusIcon />
                  </button>
                </div>
                {tmShowNew && (
                  <div className="template-new-form">
                    <input value={tmNewName} onChange={e => setTmNewName(e.target.value)} placeholder="テンプレート名" onKeyDown={e => e.key === "Enter" && createTemplate()} />
                    <button onClick={createTemplate} style={{ display: 'flex', alignItems: 'center', gap: '0.2rem' }}>
                      <PlusIcon />
                      <span>作成</span>
                    </button>
                  </div>
                )}
                <div className="template-list-items">
                  {templates.map(t => (
                    <div key={t.name} className={`template-list-item ${tmSelectedTemplate === t.name ? "selected" : ""}`}
                      onClick={() => { setTmSelectedTemplate(t.name); loadTmFiles(t.name); }}>
                      <div className="template-item-info">
                        <span className="template-item-name">{t.name}</span>
                        {t.isBuiltin && <span className="template-builtin-badge">BUILTIN</span>}
                        <span className="template-file-count">{t.fileCount} ファイル</span>
                      </div>
                      {!t.isBuiltin && (
                        <button className="template-delete-btn" onClick={e => { e.stopPropagation(); deleteTemplate(t.name); }} style={{ display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
                          <TrashIcon />
                        </button>
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
                    <span>{tmSelectedFile} {tmDirty && <span className="unsaved-badge">未保存</span>}</span>
                    <button className="template-save-btn" disabled={!tmDirty || isBuiltin(tmSelectedTemplate!)} onClick={saveTmFile} style={{ display: 'flex', alignItems: 'center', gap: '0.3rem' }}>
                      <SaveIcon />
                      <span>保存</span>
                    </button>
                  </div>
                  <textarea className="template-editor-textarea" value={tmFileContent}
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
