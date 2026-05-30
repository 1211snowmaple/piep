//! EPUB中間形式データ構造。

use serde::{Deserialize, Serialize};

// ============================================================
// メイン構造体
// ============================================================

/// EPUB 中間形式 — テンプレートに渡す統一データモデル
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubManifest {
    pub core: EpubCore,
    pub provider: ProviderData,
    pub content: EpubContent,
}

// ============================================================
// コアメタデータ
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubCore {
    pub id_: String,
    pub name: String,
    pub author: EpubAuthor,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    #[serde(rename = "datePublished")]
    pub date_published: String,
    #[serde(rename = "dateModified")]
    pub date_modified: Option<String>,
    #[serde(rename = "mainEntityOfPage")]
    pub main_entity_of_page: String,
    #[serde(rename = "isPartOf")]
    pub is_part_of: Option<EpubSeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubAuthor {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubSeries {
    pub name: String,
    pub order: Option<u32>,
}

// ============================================================
// プロバイダー固有データ
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderData {
    pub source: String,
    pub novel_id: Option<String>,
    pub post_id: Option<String>,
    pub series_id: Option<String>,
}

// ============================================================
// コンテンツデータ
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubContent {
    pub pages: Vec<EpubPage>,
    pub cover_image: Option<EpubImage>,
    pub illustrations: Vec<EpubImage>,
    pub text_length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubPage {
    pub title: Option<String>,
    pub html_content: String,
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubImage {
    pub id: String,
    pub local_path: String,
    pub mime_type: String,
    pub alt_text: Option<String>,
}

// ============================================================
// エクスポート設定 (JS ↔ Rust boundary — camelCase 必須)
// ============================================================

/// 画像圧縮オプション
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCompressOptions {
    pub enabled: bool,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub output_format: Option<String>,

    // --- JPEG (zenjpeg) options ---
    pub jpeg_quality: u8,
    pub jpeg_progressive: bool,
    pub jpeg_chroma_subsampling: String, // "4:2:0" | "4:2:2" | "4:4:4"
    pub jpeg_auto_optimize: bool,        // trellis quantization
    pub jpeg_deringing: bool,            // overshoot deringing
    pub jpeg_separate_chroma_tables: bool,
    pub jpeg_sharp_yuv: bool, // SharpYUV chroma downsampling

    // --- PNG (oxipng) options ---
    pub png_compression: Option<String>,
    pub png_interlace: bool,
    pub png_strip: bool,
    pub png_optimize_alpha: bool,
    pub png_bit_depth_reduction: bool,
    pub png_color_type_reduction: bool,
    pub png_palette_reduction: bool,
    pub png_grayscale_reduction: bool,
    pub png_idat_recoding: bool,
    pub png_fast_evaluation: bool,
    pub png_force: bool,
    pub png_fix_errors: bool,

    // --- WebP options ---
    pub webp_quality: u8,
    pub webp_lossless: bool,
    pub webp_method: i32,           // 0 to 6
    pub webp_filter_strength: i32,  // 0 to 100
    pub webp_filter_sharpness: i32, // 0 to 7
    pub webp_filter_type: i32,      // 0 (simple) | 1 (strong)
    pub webp_sns_strength: i32,     // 0 to 100
    pub webp_near_lossless: i32,    // 0 to 100
    pub webp_exact: bool,
    pub webp_use_sharp_yuv: bool,
}

impl Default for ImageCompressOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            max_width: None,
            max_height: None,
            output_format: None,

            // JPEG
            jpeg_quality: 85,
            jpeg_progressive: true,
            jpeg_chroma_subsampling: "4:2:0".to_string(),
            jpeg_auto_optimize: false,
            jpeg_deringing: true,
            jpeg_separate_chroma_tables: true,
            jpeg_sharp_yuv: false,

            // PNG
            png_compression: Some("2".into()),
            png_interlace: false,
            png_strip: true,
            png_optimize_alpha: false,
            png_bit_depth_reduction: true,
            png_color_type_reduction: true,
            png_palette_reduction: true,
            png_grayscale_reduction: true,
            png_idat_recoding: true,
            png_fast_evaluation: false,
            png_force: false,
            png_fix_errors: true,

            // WebP
            webp_quality: 75,
            webp_lossless: false,
            webp_method: 4,
            webp_filter_strength: 60,
            webp_filter_sharpness: 0,
            webp_filter_type: 1,
            webp_sns_strength: 50,
            webp_near_lossless: 100,
            webp_exact: false,
            webp_use_sharp_yuv: false,
        }
    }
}

/// バッチエクスポート結果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportBatchResult {
    pub success_count: u32,
    pub failed_count: u32,
    pub failed_ids: Vec<i64>,
    pub output_files: Vec<String>,
}

/// テンプレート情報
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    pub name: String,
    pub is_builtin: bool,
    pub file_count: u32,
}

/// テンプレートファイル情報
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFile {
    pub filename: String,
    pub size_bytes: u64,
}

// ============================================================
// 進捗通知イベント
// ============================================================

/// EPUB エクスポート進捗イベントペイロード
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    /// "started" | "converting" | "building" | "compressing" | "completed" | "failed"
    pub phase: String,
    pub current_title: String,
    pub current_index: u32,
    pub total_count: u32,
    pub message: String,
}
