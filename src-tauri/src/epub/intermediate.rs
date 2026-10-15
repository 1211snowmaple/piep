//! EPUB中間形式データ構造。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ============================================================
// メイン構造体
// ============================================================

/// EPUB 中間形式 — テンプレートに渡す統一データモデル
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubManifest {
    pub core: EpubCore,
    pub provider: ProviderData,
    pub content: EpubContent,
    pub stats: EpubStats,
}

// ============================================================
// コアメタデータ
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubCore {
    pub id_: String,
    pub name: String,
    pub author: EpubAuthor,
    /// 整形式 XHTML に正規化済みの紹介文。そのまま本文に差し込める。
    pub description: Option<String>,
    /// タグを落とした紹介文。`dc:description` や要約表示に使う。
    #[serde(rename = "descriptionText")]
    pub description_text: Option<String>,
    pub keywords: Vec<String>,
    pub tags: Vec<EpubTag>,
    #[serde(rename = "datePublished")]
    pub date_published: String,
    #[serde(rename = "dateModified")]
    pub date_modified: Option<String>,
    #[serde(rename = "mainEntityOfPage")]
    pub main_entity_of_page: String,
    #[serde(rename = "isPartOf")]
    pub is_part_of: Option<EpubSeries>,
    pub language: String,
    pub publisher: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubAuthor {
    pub name: String,
    pub id: String,
    /// pixiv のユーザー名 / FANBOX のクリエイター ID。URL に使われる短い名前。
    pub account: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "iconUrl")]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubSeries {
    pub name: String,
    pub order: Option<u32>,
    pub id: Option<String>,
    pub url: Option<String>,
}

/// タグ。pixiv は英訳を持つことがある。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubTag {
    pub name: String,
    pub translated: Option<String>,
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
    /// FANBOX の投稿種別 (article / text / image / file)。
    #[serde(rename = "postType")]
    pub post_type: Option<String>,
}

// ============================================================
// 数値でとれる情報
// ============================================================

/// 作品まわりの数値。情報ページに出すかはテンプレート側が決める。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpubStats {
    pub text_length: u64,
    pub page_count: u32,
    pub chapter_count: u32,
    pub image_count: u32,
    pub attachment_count: u32,
    pub like_count: Option<u64>,
    pub comment_count: Option<u64>,
    /// FANBOX の閲覧に必要な支援額（円）。
    pub fee_required: Option<u64>,
    pub adult: bool,
}

// ============================================================
// コンテンツデータ
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubContent {
    pub pages: Vec<EpubPage>,
    pub cover_image: Option<EpubImage>,
    pub illustrations: Vec<EpubImage>,
    /// 本文から参照される添付ファイル。EPUB には収録せず、存在だけを伝える。
    pub attachments: Vec<EpubAttachment>,
    pub text_length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubPage {
    pub title: Option<String>,
    pub html_content: String,
    pub order: u32,
    /// ページ内の見出し。目次を二階層にするために使う。
    pub chapters: Vec<EpubChapter>,
    /// 長すぎるページを割ったときの、何枚目か。0 はページそのもの。
    ///
    /// 割った先も `order` は変えない。`[jump:5]` は `page_005.xhtml` を指して
    /// おり、番号を振り直すと**本文のページ内リンクが軒並みずれる**。
    #[serde(default)]
    pub part: u32,
}

/// ページ内の見出しひとつ。`id` は同じページの XHTML 内のアンカー。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubChapter {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubImage {
    pub id: String,
    pub local_path: String,
    pub mime_type: String,
    pub alt_text: Option<String>,
    pub caption: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpubAttachment {
    pub name: String,
    pub extension: Option<String>,
    pub size_bytes: Option<u64>,
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
    /// 一時ファイルは生成できたが検証を通らず、公開せずキューに残す作品。
    pub invalid_ids: Vec<i64>,
    pub output_files: Vec<String>,
    /// 一時生成後の検査でエラーが見つかり、出力しなかった冊数。
    pub invalid_count: u32,
    pub issues: Vec<EpubValidationIssue>,
    /// 途中で止めた。残りは手つかずなので、キューから外してはいけない。
    pub canceled: bool,
    /// 止めたために一度も試していない作品。キューに残す。
    pub skipped_ids: Vec<i64>,
}

/// テンプレート情報
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    pub name: String,
    pub is_builtin: bool,
    pub file_count: u32,
    pub settings: TemplateSettings,
}

/// テンプレートファイル情報
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFile {
    pub filename: String,
    pub size_bytes: u64,
    /// 既定のテンプレートから変更されているか。戻せるかどうかの判断に使う。
    pub customized: bool,
}

// ============================================================
// テンプレート設定 (template.json)
// ============================================================

/// 組版そのものではなく「何をどこに出すか」を決める設定。
///
/// CSS と XHTML はテンプレートファイルが受け持ち、こちらは構成 — 表紙を作るか、
/// 情報ページにどの項目をどの順で並べるか、目次を何階層にするか — を受け持つ。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TemplateSettings {
    pub label: String,
    pub description: String,
    /// 自動選択でこのテンプレートが選ばれる取得元。
    pub applies_to: Vec<String>,
    pub language: String,
    /// 綴じ方向。縦書きの日本語なら "rtl"。
    pub page_progression: String,
    pub include_cover_page: bool,
    pub include_info_page: bool,
    /// EPUB 2 互換の目次。古い端末や取り込み側のために既定で入れる。
    pub include_ncx: bool,
    /// 目次に章見出しまで並べるか。
    pub chapter_toc: bool,
    /// 表紙を読み進み順に含めるか。外すと本文から始まる。
    pub cover_in_reading_order: bool,
    /// 情報ページに並べる項目と順序。
    pub info_fields: Vec<InfoField>,
    /// 見出し語。目次や項目名の文言をテンプレートごとに変えられる。
    pub strings: BTreeMap<String, String>,
}

/// 情報ページに出す項目ひとつ。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoField {
    pub key: String,
    pub label: String,
    pub enabled: bool,
}

impl InfoField {
    fn new(key: &str, label: &str, enabled: bool) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            enabled,
        }
    }
}

/// 情報ページに置ける項目のすべて。UI の一覧と既定値の両方がここから作られる。
pub fn default_info_fields() -> Vec<InfoField> {
    vec![
        InfoField::new("series", "シリーズ", true),
        InfoField::new("title", "タイトル", true),
        InfoField::new("author", "作者", true),
        InfoField::new("cover", "表紙", true),
        InfoField::new("textLength", "文字数", true),
        InfoField::new("description", "紹介文", true),
        InfoField::new("tags", "タグ", true),
        InfoField::new("datePublished", "公開日", true),
        InfoField::new("dateModified", "更新日", false),
        InfoField::new("source", "配信元URL", true),
        InfoField::new("pageCount", "ページ数", false),
        InfoField::new("imageCount", "画像点数", false),
        InfoField::new("likeCount", "いいね", false),
        InfoField::new("commentCount", "コメント数", false),
        InfoField::new("feeRequired", "支援プラン", false),
        InfoField::new("adult", "年齢制限", false),
        InfoField::new("account", "アカウント名", false),
        InfoField::new("attachments", "添付ファイル", false),
    ]
}

pub fn default_strings() -> BTreeMap<String, String> {
    [
        ("TOC_TITLE", "目次"),
        ("LANDMARKS_TITLE", "ランドマーク"),
        ("COVER_TITLE", "表紙"),
        ("INFO_TITLE", "作品情報"),
        ("BODY_MATTER_TITLE", "本文"),
        ("BODY_PAGE_TITLE", "本文"),
        ("PAGE_LABEL", "ページ"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

impl Default for TemplateSettings {
    fn default() -> Self {
        Self {
            label: String::new(),
            description: String::new(),
            applies_to: Vec::new(),
            language: "ja".to_string(),
            page_progression: "ltr".to_string(),
            include_cover_page: true,
            include_info_page: true,
            include_ncx: true,
            chapter_toc: true,
            cover_in_reading_order: true,
            info_fields: default_info_fields(),
            strings: default_strings(),
        }
    }
}

impl TemplateSettings {
    /// 設定に載っていない項目は既定の並びの末尾に足す。
    ///
    /// 版を上げて項目が増えても、利用者が保存した順序を壊さずに新項目が現れる。
    pub fn normalized(mut self) -> Self {
        let defaults = default_info_fields();
        for field in &defaults {
            if !self.info_fields.iter().any(|item| item.key == field.key) {
                self.info_fields.push(field.clone());
            }
        }
        self.info_fields
            .retain(|field| defaults.iter().any(|item| item.key == field.key));
        for (key, value) in default_strings() {
            self.strings.entry(key).or_insert(value);
        }
        if self.language.trim().is_empty() {
            self.language = "ja".to_string();
        }
        if !matches!(self.page_progression.as_str(), "ltr" | "rtl") {
            self.page_progression = "ltr".to_string();
        }
        self
    }

    pub fn enabled_info_fields(&self) -> Vec<&InfoField> {
        self.info_fields
            .iter()
            .filter(|item| item.enabled)
            .collect()
    }
}

// ============================================================
// 検証レポート
// ============================================================

/// 生成した EPUB を開き直して検査した結果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpubValidationReport {
    pub path: String,
    pub valid: bool,
    pub file_size_bytes: u64,
    pub issues: Vec<EpubValidationIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpubValidationIssue {
    /// "error" | "warning"
    pub severity: String,
    /// 規格上の根拠がわかる短い符号 (例: "OPF-ID-DUPLICATE")。
    pub code: String,
    pub location: String,
    pub message: String,
}

impl EpubValidationIssue {
    pub fn error(code: &str, location: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "error".into(),
            code: code.into(),
            location: location.into(),
            message: message.into(),
        }
    }

    pub fn warning(code: &str, location: &str, message: impl Into<String>) -> Self {
        Self {
            severity: "warning".into(),
            code: code.into(),
            location: location.into(),
            message: message.into(),
        }
    }
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

#[cfg(test)]
mod template_settings_tests {
    use super::*;

    /// 版を上げて項目が増えても、利用者が保存した並びと選択を壊さない。
    ///
    /// 既定を丸ごと当て直すと、並べ替えも「出さない」の選択も消える。
    /// 足りないものだけを末尾に足す、という約束をここで留める。
    #[test]
    fn a_new_field_appears_without_disturbing_the_saved_order() {
        let saved = TemplateSettings {
            info_fields: vec![
                InfoField::new("tags", "タグ", false),
                InfoField::new("title", "タイトル", true),
            ],
            ..TemplateSettings::default()
        };

        let normalized = saved.normalized();

        assert_eq!(normalized.info_fields[0].key, "tags", "保存した並びが先頭");
        assert_eq!(normalized.info_fields[1].key, "title");
        assert!(
            !normalized.info_fields[0].enabled,
            "切ってあった選択が戻っている"
        );
        assert_eq!(
            normalized.info_fields.len(),
            default_info_fields().len(),
            "足りない項目が末尾に足されていない"
        );
    }

    /// 無くなった項目は落とす。残すと、描く側が知らない鍵を渡されることになる。
    #[test]
    fn a_field_that_no_longer_exists_is_dropped() {
        let mut saved = TemplateSettings::default();
        saved
            .info_fields
            .push(InfoField::new("obsolete", "むかしの項目", true));

        let normalized = saved.normalized();

        assert!(
            !normalized.info_fields.iter().any(|f| f.key == "obsolete"),
            "知らない項目が残っている"
        );
    }

    /// 書き換えた文言は残す。足りないものだけ既定で埋める。
    #[test]
    fn saved_wording_is_kept_and_only_the_gaps_are_filled() {
        let mut saved = TemplateSettings::default();
        let (key, default_value) = default_strings()
            .into_iter()
            .next()
            .expect("既定の文言が一つも無い");
        saved.strings.clear();
        saved
            .strings
            .insert(key.clone(), "自分で書いた文言".to_string());

        let normalized = saved.normalized();

        assert_eq!(
            normalized.strings.get(&key).map(String::as_str),
            Some("自分で書いた文言"),
            "書き換えた文言が既定で上書きされている"
        );
        assert_ne!(default_value, "自分で書いた文言");
        assert_eq!(
            normalized.strings.len(),
            default_strings().len(),
            "足りない文言が埋まっていない"
        );
    }

    /// 空や知らない値は、開ける形へ倒す。読めない EPUB を作らない。
    #[test]
    fn an_empty_language_and_an_unknown_direction_fall_back() {
        let saved = TemplateSettings {
            language: "   ".to_string(),
            page_progression: "sideways".to_string(),
            ..TemplateSettings::default()
        };

        let normalized = saved.normalized();

        assert_eq!(normalized.language, "ja");
        assert_eq!(normalized.page_progression, "ltr");
    }

    /// 選んだ向きは残す。既定へ倒すのは、知らない値のときだけ。
    #[test]
    fn a_known_direction_is_left_alone() {
        let saved = TemplateSettings {
            page_progression: "rtl".to_string(),
            ..TemplateSettings::default()
        };

        assert_eq!(saved.normalized().page_progression, "rtl");
    }

    /// 出す項目だけを並べる。
    #[test]
    fn only_enabled_fields_are_listed() {
        let saved = TemplateSettings {
            info_fields: vec![
                InfoField::new("title", "タイトル", true),
                InfoField::new("tags", "タグ", false),
            ],
            ..TemplateSettings::default()
        };

        let enabled = saved.enabled_info_fields();

        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].key, "title");
    }
}
