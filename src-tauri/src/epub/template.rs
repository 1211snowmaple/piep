//! MiniJinja テンプレートエンジン統合モジュール。
//!
//! テンプレートは XHTML と OPF、つまり XML を書き出す。MiniJinja の既定では
//! `.xhtml.j2` も `.opf.j2` も自動エスケープの対象外で、`&` を含む題名ひとつで
//! 生成物が解析不能になる。ここでは拡張子ごとにエスケープ方針を与え、
//! 本文のように「すでに整形式である」と分かっているものだけを素通しする。

use crate::epub::intermediate::*;
use crate::epub::meta;
use crate::epub::xhtml;
use minijinja::{context, AutoEscape, Environment, Value as MjValue};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

// ============================================================
// デフォルトテンプレート (バイナリ埋め込み)
// ============================================================

/// テンプレートが持ちうるファイルのすべて。並びは編集画面の並びでもある。
pub const TEMPLATE_FILES: &[&str] = &[
    "style.css.j2",
    "_base_style.css.j2",
    "cover_page.xhtml.j2",
    "info_page.xhtml.j2",
    "page_wrapper.xhtml.j2",
    "nav.xhtml.j2",
    "toc.ncx.j2",
    "content.opf.j2",
];

/// 何をするファイルなのかの一行説明。編集画面がそのまま出す。
pub fn template_file_purpose(filename: &str) -> &'static str {
    match filename {
        "style.css.j2" => "本全体の組版。テーマ固有の指定はここに書く",
        "_base_style.css.j2" => "すべてのテンプレートが土台にする基本スタイル",
        "cover_page.xhtml.j2" => "表紙のページ",
        "info_page.xhtml.j2" => "作品情報のページ。並べる項目は設定で決まる",
        "page_wrapper.xhtml.j2" => "本文ページの外枠",
        "nav.xhtml.j2" => "目次 (EPUB 3 のナビゲーション文書)",
        "toc.ncx.j2" => "EPUB 2 互換の目次。古い端末向け",
        "content.opf.j2" => "書誌情報とファイル一覧 (パッケージ文書)",
        _ => "",
    }
}

pub struct DefaultTemplates;
impl DefaultTemplates {
    pub fn get_all() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "_base_style.css.j2",
                include_str!("templates/default/_base_style.css.j2"),
            ),
            (
                "style.css.j2",
                include_str!("templates/default/style.css.j2"),
            ),
            (
                "content.opf.j2",
                include_str!("templates/default/content.opf.j2"),
            ),
            (
                "nav.xhtml.j2",
                include_str!("templates/default/nav.xhtml.j2"),
            ),
            ("toc.ncx.j2", include_str!("templates/default/toc.ncx.j2")),
            (
                "cover_page.xhtml.j2",
                include_str!("templates/default/cover_page.xhtml.j2"),
            ),
            (
                "info_page.xhtml.j2",
                include_str!("templates/default/info_page.xhtml.j2"),
            ),
            (
                "page_wrapper.xhtml.j2",
                include_str!("templates/default/page_wrapper.xhtml.j2"),
            ),
        ]
    }

    pub fn settings() -> &'static str {
        include_str!("templates/default/template.json")
    }
}

pub struct PixivTemplates;
impl PixivTemplates {
    pub fn get_all() -> Vec<(&'static str, &'static str)> {
        vec![
            ("style.css.j2", include_str!("templates/pixiv/style.css.j2")),
            (
                "info_page.xhtml.j2",
                include_str!("templates/pixiv/info_page.xhtml.j2"),
            ),
        ]
    }

    pub fn settings() -> &'static str {
        include_str!("templates/pixiv/template.json")
    }
}

pub struct FanboxTemplates;
impl FanboxTemplates {
    pub fn get_all() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "style.css.j2",
                include_str!("templates/fanbox/style.css.j2"),
            ),
            (
                "info_page.xhtml.j2",
                include_str!("templates/fanbox/info_page.xhtml.j2"),
            ),
        ]
    }

    pub fn settings() -> &'static str {
        include_str!("templates/fanbox/template.json")
    }
}

const BUILTIN_TEMPLATES: &[&str] = &["default", "pixiv", "fanbox"];

fn builtin_files(name: &str) -> Option<Vec<(&'static str, &'static str)>> {
    match name {
        "default" => Some(DefaultTemplates::get_all()),
        "pixiv" => Some(PixivTemplates::get_all()),
        "fanbox" => Some(FanboxTemplates::get_all()),
        _ => None,
    }
}

fn builtin_settings(name: &str) -> Option<&'static str> {
    match name {
        "default" => Some(DefaultTemplates::settings()),
        "pixiv" => Some(PixivTemplates::settings()),
        "fanbox" => Some(FanboxTemplates::settings()),
        _ => None,
    }
}

/// そのテンプレートで、あるファイルの出荷時の中身。
///
/// 組み込みテンプレートが持たないファイルは default の中身にあたる。
pub fn builtin_file_content(template_name: &str, filename: &str) -> Option<&'static str> {
    builtin_files(template_name)
        .and_then(|files| {
            files
                .iter()
                .find(|(name, _)| *name == filename)
                .map(|(_, content)| *content)
        })
        .or_else(|| {
            DefaultTemplates::get_all()
                .iter()
                .find(|(name, _)| *name == filename)
                .map(|(_, content)| *content)
        })
}

// ============================================================
// テンプレートマネージャ
// ============================================================

const SETTINGS_FILE: &str = "template.json";

pub struct TemplateManager {
    templates_dir: PathBuf,
}

impl TemplateManager {
    pub fn new(templates_dir: PathBuf) -> Self {
        Self { templates_dir }
    }

    pub fn initialize_defaults(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.templates_dir).map_err(|e| e.to_string())?;
        for name in BUILTIN_TEMPLATES {
            let dir = self.templates_dir.join(name);
            let fresh = !dir.exists();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // 組み込みは利用者が編集できないので、版が上がったら常に書き戻す。
            // 派生テンプレートは複製なので、この上書きに巻き込まれない。
            // 派生元が default のファイルも含め、組み込み一式を毎回更新する。
            // 以前は pixiv / fanbox 固有の2ファイルしか上書きせず、旧版で作成済み
            // の content.opf や nav が永遠に残り続けていた。
            for filename in TEMPLATE_FILES {
                let content = builtin_file_content(name, filename)
                    .ok_or_else(|| format!("組み込みファイル '{}' がありません", filename))?;
                std::fs::write(dir.join(filename), content).map_err(|e| e.to_string())?;
            }
            if let Some(settings) = builtin_settings(name) {
                std::fs::write(dir.join(SETTINGS_FILE), settings).map_err(|e| e.to_string())?;
            }
            if fresh {
                log::info!("EPUB テンプレート '{}' を用意しました", name);
            }
        }
        Ok(())
    }

    pub fn list_templates(&self) -> Result<Vec<TemplateInfo>, String> {
        let mut templates = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.templates_dir) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if validate_template_name(&name).is_err()
                    || self.existing_template_dir(&name).is_err()
                {
                    continue;
                }
                let file_count = self
                    .get_template_files(&name)
                    .map(|files| files.len() as u32)
                    .unwrap_or(0);
                templates.push(TemplateInfo {
                    settings: self.read_settings(&name),
                    is_builtin: is_builtin_template(&name),
                    name,
                    file_count,
                });
            }
        }
        templates.sort_by(|a, b| b.is_builtin.cmp(&a.is_builtin).then(a.name.cmp(&b.name)));
        Ok(templates)
    }

    pub fn get_template_files(&self, template_name: &str) -> Result<Vec<TemplateFile>, String> {
        let dir = self.existing_template_dir(template_name)?;
        let mut files = Vec::new();
        // 一覧の順序は TEMPLATE_FILES に従える。ディレクトリの並びは環境で変わる。
        for filename in TEMPLATE_FILES {
            let Ok(path) = self.existing_template_file(template_name, filename) else {
                continue;
            };
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            files.push(TemplateFile {
                filename: filename.to_string(),
                size_bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                customized: builtin_file_content(template_name, filename)
                    .is_some_and(|original| original != content),
            });
        }
        let _ = dir;
        Ok(files)
    }

    pub fn read_template_file(
        &self,
        template_name: &str,
        filename: &str,
    ) -> Result<String, String> {
        let path = self.existing_template_file(template_name, filename)?;
        std::fs::read_to_string(path)
            .map_err(|e| format!("テンプレートファイルの読み込みに失敗: {}", e))
    }

    pub fn save_template_file(
        &self,
        template_name: &str,
        filename: &str,
        content: &str,
    ) -> Result<(), String> {
        validate_template_name(template_name)?;
        validate_template_filename(filename)?;
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは変更できません".to_string());
        }
        // 書き込む前に構文を確かめる。壊れたテンプレートを保存させると、
        // 書き出しのときになって初めて失敗し、原因が見えなくなる。
        check_template_syntax(filename, content)?;
        let dir = self.existing_template_dir(template_name)?;
        std::fs::write(dir.join(filename), content).map_err(|e| format!("保存に失敗: {}", e))
    }

    /// ファイルを出荷時の内容に戻す。
    pub fn reset_template_file(
        &self,
        template_name: &str,
        filename: &str,
    ) -> Result<String, String> {
        validate_template_filename(filename)?;
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは変更できません".to_string());
        }
        let content = builtin_file_content(template_name, filename)
            .ok_or_else(|| format!("'{}' に既定の内容がありません", filename))?;
        let dir = self.existing_template_dir(template_name)?;
        std::fs::write(dir.join(filename), content).map_err(|e| format!("保存に失敗: {}", e))?;
        Ok(content.to_string())
    }

    pub fn read_settings(&self, template_name: &str) -> TemplateSettings {
        let stored = self
            .existing_template_dir(template_name)
            .ok()
            .map(|dir| dir.join(SETTINGS_FILE))
            .filter(|path| path.is_file())
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str::<TemplateSettings>(&raw).ok());
        let mut settings = stored.unwrap_or_default().normalized();
        if settings.label.trim().is_empty() {
            settings.label = template_name.to_string();
        }
        settings
    }

    pub fn save_settings(
        &self,
        template_name: &str,
        settings: TemplateSettings,
    ) -> Result<TemplateSettings, String> {
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは変更できません".to_string());
        }
        let dir = self.existing_template_dir(template_name)?;
        let settings = settings.normalized();
        let raw = serde_json::to_string_pretty(&settings)
            .map_err(|e| format!("設定の書き出しに失敗: {}", e))?;
        std::fs::write(dir.join(SETTINGS_FILE), raw)
            .map_err(|e| format!("設定の保存に失敗: {}", e))?;
        Ok(settings)
    }

    /// 既存のテンプレートを丸ごと複製して新しいテンプレートを作る。
    pub fn create_template(&self, template_name: &str, base: &str) -> Result<(), String> {
        validate_template_name(template_name)?;
        if is_builtin_template(template_name) {
            return Err("ビルトイン名は新規テンプレートに使用できません".to_string());
        }
        let dest = self.templates_dir.join(template_name);
        if dest.exists() {
            return Err(format!("テンプレート '{}' は既に存在します", template_name));
        }
        let source = self.existing_template_dir(base)?;
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        for filename in TEMPLATE_FILES {
            let from = source.join(filename);
            if from.is_file() {
                std::fs::copy(&from, dest.join(filename))
                    .map_err(|e| format!("コピー失敗: {}", e))?;
            }
        }
        let mut settings = self.read_settings(base);
        settings.label = template_name.to_string();
        // 複製が元と同じ取得元を主張すると、自動選択がどちらを選ぶか定まらない。
        settings.applies_to.clear();
        let raw = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        std::fs::write(dest.join(SETTINGS_FILE), raw).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn rename_template(&self, template_name: &str, next_name: &str) -> Result<(), String> {
        validate_template_name(next_name)?;
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは変更できません".into());
        }
        if is_builtin_template(next_name) {
            return Err("ビルトイン名は使用できません".into());
        }
        let dir = self.existing_template_dir(template_name)?;
        let dest = self.templates_dir.join(next_name);
        if dest.exists() {
            return Err(format!("テンプレート '{}' は既に存在します", next_name));
        }
        std::fs::rename(&dir, &dest).map_err(|e| format!("名前の変更に失敗: {}", e))
    }

    pub fn delete_template(&self, template_name: &str) -> Result<(), String> {
        validate_template_name(template_name)?;
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは削除できません".into());
        }
        let dir = self.existing_template_dir(template_name)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("削除に失敗: {}", e))
    }

    /// 取得元に合うテンプレートを選ぶ。設定で名乗り出たものを優先する。
    pub fn resolve_for_source(&self, source: &str) -> String {
        let templates = self.list_templates().unwrap_or_default();
        templates
            .iter()
            .find(|template| {
                !template.is_builtin
                    && template
                        .settings
                        .applies_to
                        .iter()
                        .any(|value| value == source)
            })
            .or_else(|| {
                templates.iter().find(|template| {
                    template
                        .settings
                        .applies_to
                        .iter()
                        .any(|value| value == source)
                })
            })
            .map(|template| template.name.clone())
            .unwrap_or_else(|| "default".to_string())
    }

    fn resolve_template_content(
        &self,
        template_name: &str,
        filename: &str,
    ) -> Result<String, String> {
        validate_template_name(template_name)?;
        validate_template_filename(filename)?;
        if let Ok(primary) = self.existing_template_file(template_name, filename) {
            return std::fs::read_to_string(&primary)
                .map_err(|e| format!("テンプレート読み込みエラー: {}", e));
        }
        if let Ok(fallback) = self.existing_template_file("default", filename) {
            return std::fs::read_to_string(&fallback)
                .map_err(|e| format!("テンプレート読み込みエラー: {}", e));
        }
        // 利用者がファイルを消していても書き出しは通したい。埋め込みが最後の砦。
        builtin_file_content(template_name, filename)
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "テンプレート '{}' が見つかりません (テンプレート: {})",
                    filename, template_name
                )
            })
    }

    /// テンプレートコンテンツをすべてロード。CSS の include を事前解決する。
    pub fn load_template_contents(
        &self,
        template_name: &str,
    ) -> Result<HashMap<String, String>, String> {
        let mut contents = HashMap::new();
        for filename in TEMPLATE_FILES {
            let content = self.resolve_template_content(template_name, filename)?;
            contents.insert(filename.to_string(), content);
        }

        // CSS の {% include %} を事前解決 (MiniJinja の include 対応)
        // base_style の内容をすでに別テンプレートとして登録するので MiniJinja が自動解決する
        // ただし安全のため、include が解決できない場合にフォールバックとして手動解決も行う
        if let (Some(base), Some(style)) = (
            contents.get("_base_style.css.j2").cloned(),
            contents.get("style.css.j2").cloned(),
        ) {
            if style.contains("{% include") {
                let resolved = style.replace("{% include \"_base_style.css.j2\" %}", &base);
                contents.insert("style.css.j2".to_string(), resolved);
            }
        }

        Ok(contents)
    }

    fn existing_template_dir(&self, template_name: &str) -> Result<PathBuf, String> {
        validate_template_name(template_name)?;
        let root = self
            .templates_dir
            .canonicalize()
            .map_err(|e| format!("テンプレートルートの解決に失敗: {e}"))?;
        let lexical = self.templates_dir.join(template_name);
        let metadata = std::fs::symlink_metadata(&lexical)
            .map_err(|_| format!("テンプレート '{}' が見つかりません", template_name))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("テンプレートディレクトリが不正です".to_string());
        }
        let dir = lexical
            .canonicalize()
            .map_err(|e| format!("テンプレートパスの解決に失敗: {e}"))?;
        if dir.parent() != Some(root.as_path()) {
            return Err("テンプレートパスがテンプレートルート外です".to_string());
        }
        Ok(dir)
    }

    fn existing_template_file(
        &self,
        template_name: &str,
        filename: &str,
    ) -> Result<PathBuf, String> {
        validate_template_filename(filename)?;
        let dir = self.existing_template_dir(template_name)?;
        let lexical = dir.join(filename);
        let metadata = std::fs::symlink_metadata(&lexical)
            .map_err(|_| format!("テンプレートファイル '{}' が見つかりません", filename))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("テンプレートファイルが不正です".to_string());
        }
        let file = lexical
            .canonicalize()
            .map_err(|e| format!("テンプレートファイルの解決に失敗: {e}"))?;
        if file.parent() != Some(dir.as_path()) {
            return Err("テンプレートファイルがテンプレート外です".to_string());
        }
        Ok(file)
    }
}

fn is_builtin_template(name: &str) -> bool {
    BUILTIN_TEMPLATES.contains(&name)
}

fn validate_template_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("テンプレート名は64文字以内の英数字・_・-のみ使用できます".to_string());
    }
    Ok(())
}

fn validate_template_filename(filename: &str) -> Result<(), String> {
    let path = Path::new(filename);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err("テンプレートファイル名にはディレクトリを指定できません".to_string());
    }
    if !TEMPLATE_FILES.contains(&filename) {
        return Err("許可されていないテンプレートファイル名です".to_string());
    }
    Ok(())
}

/// 保存前の構文検査。描画までは行わないので、変数の有無は見ない。
fn check_template_syntax(filename: &str, content: &str) -> Result<(), String> {
    let mut env = Environment::new();
    env.add_template(filename, content)
        .map_err(|e| format!("テンプレートの構文が不正です: {}", e))?;
    Ok(())
}

// ============================================================
// テンプレートレンダリング
// ============================================================

pub struct EpubRenderer {
    template_contents: HashMap<String, String>,
    settings: TemplateSettings,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpineItemref {
    pub idref: String,
    pub linear: bool,
    pub properties: Option<String>,
}

/// 目次の一項目。`children` は章見出し。
///
/// `order` は文書順の通し番号。NCX の `playOrder` がこれを要求する。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NavEntry {
    pub id: String,
    pub order: u32,
    pub href: String,
    pub title: String,
    pub children: Vec<NavEntry>,
}

impl EpubRenderer {
    pub fn new(template_contents: HashMap<String, String>, settings: TemplateSettings) -> Self {
        Self {
            template_contents,
            settings: settings.normalized(),
        }
    }

    pub fn settings(&self) -> &TemplateSettings {
        &self.settings
    }

    fn create_env(&self) -> Result<Environment<'_>, String> {
        let mut env = Environment::new();
        // ここが要。XML を書き出すテンプレートは必ずエスケープする。
        env.set_auto_escape_callback(|name| {
            let name = name.strip_suffix(".j2").unwrap_or(name);
            match name.rsplit('.').next() {
                Some("xhtml" | "html" | "opf" | "ncx" | "xml") => AutoEscape::Html,
                _ => AutoEscape::None,
            }
        });
        // 既定の HTML エスケープは `/` まで数値参照にするので、href が
        // `..&#x2f;style.css` になる。規格上は読めても、生成物を読む人にも
        // 素朴な取り込み側にも優しくない。XML に必要な 5 文字だけを落とす。
        env.set_formatter(|out, state, value| {
            use std::fmt::Write;
            if value.is_safe() || matches!(state.auto_escape(), AutoEscape::None) {
                return write!(out, "{value}").map_err(minijinja::Error::from);
            }
            let mut rendered = String::new();
            write!(rendered, "{value}").map_err(minijinja::Error::from)?;
            out.write_str(&xhtml::escape_xml(&rendered))
                .map_err(minijinja::Error::from)
        });
        env.add_filter("format_number", |value: MjValue| -> String {
            if let Some(n) = value.as_i64() {
                format_number_with_commas(n)
            } else {
                value.to_string()
            }
        });
        env.add_filter("date_ja", |value: MjValue| -> String {
            meta::format_date_japanese(&value.to_string())
        });
        env.add_filter("strip_html", |value: MjValue| -> String {
            xhtml::strip_tags(&value.to_string())
        });
        for (name, content) in &self.template_contents {
            env.add_template_owned(name.clone(), content.clone())
                .map_err(|e| format!("テンプレート '{}' の登録に失敗: {}", name, e))?;
        }
        Ok(env)
    }

    /// テンプレートすべてに渡る共通の文脈。
    ///
    /// 呼び出し側では必ず `context! { key => …, ..self.base_context(…) }` の形で
    /// 使うこと。名前を直接書いたものが共通の文脈より優先される。順序を逆にすると
    /// `content` のように同名の項目が共通側に食われる。
    fn base_context(&self, manifest: &EpubManifest) -> MjValue {
        context! {
            manifest => MjValue::from_serialize(manifest),
            core => MjValue::from_serialize(&manifest.core),
            provider => MjValue::from_serialize(&manifest.provider),
            stats => MjValue::from_serialize(&manifest.stats),
            content => MjValue::from_serialize(&manifest.content),
            settings => MjValue::from_serialize(&self.settings),
            strings => MjValue::from_serialize(&self.settings.strings),
            text_length => manifest.stats.text_length,
            // 紹介文はすでに整形式 XHTML なので、素通しさせないとタグが字面で出る。
            description_html => MjValue::from_safe_string(
                manifest.core.description.clone().unwrap_or_default()
            ),
            formatted_date => meta::format_date_japanese(&manifest.core.date_published),
            formatted_modified => manifest
                .core
                .date_modified
                .as_deref()
                .map(meta::format_date_japanese),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_content_opf(
        &self,
        manifest: &EpubManifest,
        manifest_items: &[ManifestItem],
        spine_itemrefs: &[SpineItemref],
        cover_image_id: Option<&str>,
        modified: &str,
        published: Option<&str>,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("content.opf.j2")
            .map_err(|e| format!("content.opf.j2: {}", e))?;
        let provider_ids = context! {
            novel_id => manifest.provider.novel_id,
            post_id => manifest.provider.post_id,
            series_id => manifest.provider.series_id,
            source => manifest.provider.source,
        };
        let ctx = context! {
            manifest_items => MjValue::from_serialize(manifest_items),
            spine_itemrefs => MjValue::from_serialize(spine_itemrefs),
            provider_ids => provider_ids,
            cover_image_id => cover_image_id,
            plain_description => manifest.core.description_text.clone().unwrap_or_default(),
            modified => modified,
            published => published,
            language => meta::normalize_language(&manifest.core.language, "ja"),
            ..self.base_context(manifest),
        };
        tmpl.render(ctx)
            .map_err(|e| format!("content.opf レンダリングエラー: {}", e))
    }

    pub fn render_nav(
        &self,
        manifest: &EpubManifest,
        entries: &[NavEntry],
        has_cover: bool,
        start_href: Option<&str>,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("nav.xhtml.j2")
            .map_err(|e| format!("nav.xhtml.j2: {}", e))?;
        let ctx = context! {
            entries => MjValue::from_serialize(entries),
            has_cover => has_cover,
            start_page_href => start_href,
            css_path => "style/style.css",
            ..self.base_context(manifest),
        };
        tmpl.render(ctx)
            .map_err(|e| format!("nav.xhtml レンダリングエラー: {}", e))
    }

    pub fn render_ncx(
        &self,
        manifest: &EpubManifest,
        entries: &[NavEntry],
        identifier: &str,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("toc.ncx.j2")
            .map_err(|e| format!("toc.ncx.j2: {}", e))?;
        let depth = if entries.iter().any(|entry| !entry.children.is_empty()) {
            2
        } else {
            1
        };
        let ctx = context! {
            entries => MjValue::from_serialize(entries),
            depth => depth,
            identifier => identifier,
            ..self.base_context(manifest),
        };
        tmpl.render(ctx)
            .map_err(|e| format!("toc.ncx レンダリングエラー: {}", e))
    }

    pub fn render_cover_page(
        &self,
        manifest: &EpubManifest,
        cover_image_href: &str,
        width: u32,
        height: u32,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("cover_page.xhtml.j2")
            .map_err(|e| format!("cover_page: {}", e))?;
        let ctx = context! {
            cover_image_href => cover_image_href,
            cover_width => width,
            cover_height => height,
            css_path => "../style/style.css",
            ..self.base_context(manifest),
        };
        tmpl.render(ctx)
            .map_err(|e| format!("cover レンダリングエラー: {}", e))
    }

    pub fn render_info_page(
        &self,
        manifest: &EpubManifest,
        cover_href: Option<&str>,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("info_page.xhtml.j2")
            .map_err(|e| format!("info_page: {}", e))?;
        let ctx = context! {
            cover_href => cover_href,
            css_path => "../style/style.css",
            fields => MjValue::from_serialize(self.settings.enabled_info_fields()),
            ..self.base_context(manifest),
        };
        tmpl.render(ctx)
            .map_err(|e| format!("info_page レンダリングエラー: {}", e))
    }

    pub fn render_page(
        &self,
        manifest: &EpubManifest,
        page: &EpubPage,
        html_content: &str,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("page_wrapper.xhtml.j2")
            .map_err(|e| format!("page_wrapper: {}", e))?;
        let title = page
            .title
            .clone()
            .unwrap_or_else(|| format!("ページ {}", page.order));
        let ctx = context! {
            title => title,
            // 本文はすでに整形式 XHTML。ここで再エスケープするとタグが字面で出る。
            content => MjValue::from_safe_string(html_content.to_string()),
            page => MjValue::from_serialize(page),
            css_path => "../style/style.css",
            ..self.base_context(manifest),
        };
        tmpl.render(ctx)
            .map_err(|e| format!("page レンダリングエラー: {}", e))
    }

    pub fn render_style(&self, manifest: &EpubManifest) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("style.css.j2")
            .map_err(|e| format!("style.css: {}", e))?;
        tmpl.render(self.base_context(manifest))
            .map_err(|e| format!("style レンダリングエラー: {}", e))
    }
}

// ============================================================
// ヘルパー
// ============================================================

fn format_number_with_commas(n: i64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::new();
    let len = bytes.len();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn manager() -> (PathBuf, TemplateManager) {
        let root = std::env::temp_dir().join(format!("piep_template_{}", rand::random::<u64>()));
        let templates = root.join("templates");
        let manager = TemplateManager::new(templates);
        manager.initialize_defaults().unwrap();
        (root, manager)
    }

    #[test]
    fn initializing_again_replaces_stale_inherited_builtin_files() {
        let (root, manager) = manager();
        let stale = root.join("templates").join("pixiv").join("content.opf.j2");
        fs::write(&stale, "old package").unwrap();

        manager.initialize_defaults().unwrap();

        assert_eq!(
            fs::read_to_string(stale).unwrap(),
            builtin_file_content("pixiv", "content.opf.j2").unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn template_paths_cannot_escape_the_template_root() {
        let (root, manager) = manager();
        let outside = root.join("outside.xhtml.j2");
        fs::write(&outside, "secret").unwrap();

        assert!(manager.get_template_files("..").is_err());
        assert!(manager
            .read_template_file("default", "../../outside.xhtml.j2")
            .is_err());
        assert!(manager
            .save_template_file("custom", "C:\\outside.xhtml.j2", "overwrite")
            .is_err());
        assert!(manager.create_template("../escape", "default").is_err());
        assert!(manager.delete_template("..").is_err());
        assert!(manager.rename_template("default", "../escape").is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builtins_are_read_only_and_custom_templates_edit_known_files_only() {
        let (root, manager) = manager();
        assert!(manager
            .save_template_file("default", "style.css.j2", "changed")
            .is_err());

        manager.create_template("my_template-1", "pixiv").unwrap();
        manager
            .save_template_file("my_template-1", "style.css.j2", "body {}")
            .unwrap();
        assert_eq!(
            manager
                .read_template_file("my_template-1", "style.css.j2")
                .unwrap(),
            "body {}"
        );
        assert!(manager
            .save_template_file("my_template-1", "new.xhtml.j2", "new")
            .is_err());
        assert!(manager
            .save_template_file("my_template-1", "notes.txt", "new")
            .is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn broken_templates_are_refused_at_save_time() {
        let (root, manager) = manager();
        manager.create_template("draft", "default").unwrap();
        // 壊れた構文を保存できると、失敗するのは何冊も書き出した後になる。
        assert!(manager
            .save_template_file("draft", "page_wrapper.xhtml.j2", "{% for x in %}")
            .is_err());
        assert!(manager
            .save_template_file("draft", "page_wrapper.xhtml.j2", "{{ title }}")
            .is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn files_report_whether_they_still_match_the_shipped_copy() {
        let (root, manager) = manager();
        manager.create_template("tweaked", "default").unwrap();
        let before = manager.get_template_files("tweaked").unwrap();
        assert!(before.iter().all(|file| !file.customized));

        manager
            .save_template_file("tweaked", "style.css.j2", "body { color: red; }")
            .unwrap();
        let after = manager.get_template_files("tweaked").unwrap();
        let style = after
            .iter()
            .find(|file| file.filename == "style.css.j2")
            .unwrap();
        assert!(style.customized);

        manager
            .reset_template_file("tweaked", "style.css.j2")
            .unwrap();
        let restored = manager.get_template_files("tweaked").unwrap();
        assert!(restored.iter().all(|file| !file.customized));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn settings_survive_a_round_trip_and_gain_new_fields() {
        let (root, manager) = manager();
        manager.create_template("styled", "default").unwrap();
        let mut settings = manager.read_settings("styled");
        settings.page_progression = "rtl".into();
        settings.info_fields.retain(|field| field.key == "author");
        manager.save_settings("styled", settings).unwrap();

        let reloaded = manager.read_settings("styled");
        assert_eq!(reloaded.page_progression, "rtl");
        // 保存されていた並びは先頭に残り、知らない項目は後ろに足される。
        assert_eq!(reloaded.info_fields[0].key, "author");
        assert_eq!(reloaded.info_fields.len(), default_info_fields().len());
        assert!(manager
            .save_settings("default", TemplateSettings::default())
            .is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_template_can_claim_a_source_for_automatic_selection() {
        let (root, manager) = manager();
        assert_eq!(manager.resolve_for_source("pixiv"), "pixiv");
        assert_eq!(manager.resolve_for_source("fanbox"), "fanbox");
        assert_eq!(manager.resolve_for_source("unknown"), "default");

        manager.create_template("mine", "pixiv").unwrap();
        let mut settings = manager.read_settings("mine");
        settings.applies_to = vec!["pixiv".into()];
        manager.save_settings("mine", settings).unwrap();
        // 自作が名乗り出たら、組み込みより自作を選ぶ。
        assert_eq!(manager.resolve_for_source("pixiv"), "mine");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn renaming_moves_the_whole_template() {
        let (root, manager) = manager();
        manager.create_template("before", "default").unwrap();
        manager.rename_template("before", "after").unwrap();
        assert!(manager.read_template_file("after", "style.css.j2").is_ok());
        assert!(manager
            .read_template_file("before", "style.css.j2")
            .is_err());
        assert!(manager.rename_template("after", "default").is_err());
        let _ = fs::remove_dir_all(root);
    }
}
