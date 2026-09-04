/**
 * EPUB を書き出すときの決めごとを、次に開いたときにも憶えている。
 *
 * これまでは画面を開くたびに初期値へ戻っていた。出力先フォルダーまで毎回
 * 選び直しで、数百冊を書き出す道具として手数が多すぎた。**同じ決めごとを
 * コレクションの一冊書き出しでも使う** ―― あちらは画像最適化を渡す口が
 * 塞がっていて、いちばん大きくなる本だけが無圧縮で出ていた。
 */

export interface EpubCompressionSettings {
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
}

/**
 * 組み方。テンプレートの決めに任せるときは `null`。
 *
 * 既定はこの `null` ―― 標準・pixiv・FANBOX のどのテンプレートも横書きなので、
 * 何も選ばなければ横書きの本が出る。縦書きは**選んだときだけ**の上書きで、
 * 既定にはしない。
 */
export type EpubWritingMode = "vertical" | "horizontal" | null;

export interface EpubExportSettings {
  templateName: string;
  outputDir: string;
  writingMode: EpubWritingMode;
  compression: EpubCompressionSettings;
}

/** Picks the template that claims each work's source, per work. */
export const AUTO_TEMPLATE = "__auto__";

export const defaultCompression: EpubCompressionSettings = {
  enabled: true, maxWidth: 2000, maxHeight: 2000, outputFormat: null,
  jpegQuality: 85, jpegProgressive: true, jpegChromaSubsampling: "4:2:0", jpegAutoOptimize: false, jpegDeringing: true, jpegSeparateChromaTables: true, jpegSharpYuv: false,
  pngCompression: "2", pngInterlace: false, pngStrip: true, pngOptimizeAlpha: false, pngBitDepthReduction: true, pngColorTypeReduction: true, pngPaletteReduction: true, pngGrayscaleReduction: true, pngIdatRecoding: true, pngFastEvaluation: false, pngForce: false, pngFixErrors: true,
  webpQuality: 78, webpLossless: false, webpMethod: 4, webpFilterStrength: 60, webpFilterSharpness: 0, webpFilterType: 1, webpSnsStrength: 50, webpNearLossless: 100, webpExact: false, webpUseSharpYuv: false,
};

export const defaultExportSettings: EpubExportSettings = {
  templateName: AUTO_TEMPLATE,
  outputDir: "",
  writingMode: null,
  compression: defaultCompression,
};

const STORAGE_KEY = "piep.epub-export.v1";

/**
 * 憶えていた決めごと。壊れた値は既定へ落として返す ―― 書き出しの設定が
 * 読めないことで、書き出しそのものが止まる理由はない。
 */
export function readExportSettings(): EpubExportSettings {
  try {
    const raw: unknown = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "null");
    if (!raw || typeof raw !== "object") return defaultExportSettings;
    const value = raw as Partial<EpubExportSettings>;
    const compression = { ...defaultCompression, ...(value.compression && typeof value.compression === "object" ? value.compression : {}) };
    return {
      templateName: typeof value.templateName === "string" && value.templateName ? value.templateName : AUTO_TEMPLATE,
      outputDir: typeof value.outputDir === "string" ? value.outputDir : "",
      writingMode: value.writingMode === "vertical" || value.writingMode === "horizontal" ? value.writingMode : null,
      compression,
    };
  } catch {
    return defaultExportSettings;
  }
}

export function writeExportSettings(settings: EpubExportSettings): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // 憶えられなくても書き出しは続く。
  }
}

/** 端末側へ渡す形。画面の値をそのまま送らないのは、`""` を数として渡さないため。 */
export function toCompressOptions(compression: EpubCompressionSettings) {
  return {
    enabled: compression.enabled,
    maxWidth: typeof compression.maxWidth === "number" ? compression.maxWidth : null,
    maxHeight: typeof compression.maxHeight === "number" ? compression.maxHeight : null,
    outputFormat: compression.outputFormat,
    jpegQuality: compression.jpegQuality,
    jpegProgressive: compression.jpegProgressive,
    jpegChromaSubsampling: compression.jpegChromaSubsampling,
    jpegAutoOptimize: compression.jpegAutoOptimize,
    jpegDeringing: compression.jpegDeringing,
    jpegSeparateChromaTables: compression.jpegSeparateChromaTables,
    jpegSharpYuv: compression.jpegSharpYuv,
    pngCompression: compression.pngCompression,
    pngInterlace: compression.pngInterlace,
    pngStrip: compression.pngStrip,
    pngOptimizeAlpha: compression.pngOptimizeAlpha,
    pngBitDepthReduction: compression.pngBitDepthReduction,
    pngColorTypeReduction: compression.pngColorTypeReduction,
    pngPaletteReduction: compression.pngPaletteReduction,
    pngGrayscaleReduction: compression.pngGrayscaleReduction,
    pngIdatRecoding: compression.pngIdatRecoding,
    pngFastEvaluation: compression.pngFastEvaluation,
    pngForce: compression.pngForce,
    pngFixErrors: compression.pngFixErrors,
    webpQuality: compression.webpQuality,
    webpLossless: compression.webpLossless,
    webpMethod: compression.webpMethod,
    webpFilterStrength: compression.webpFilterStrength,
    webpFilterSharpness: compression.webpFilterSharpness,
    webpFilterType: compression.webpFilterType,
    webpSnsStrength: compression.webpSnsStrength,
    webpNearLossless: compression.webpNearLossless,
    webpExact: compression.webpExact,
    webpUseSharpYuv: compression.webpUseSharpYuv,
  };
}
