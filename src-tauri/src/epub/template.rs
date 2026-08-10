//! MiniJinja テンプレートエンジン統合モジュール。

use crate::epub::intermediate::*;
use minijinja::{context, Environment, Value as MjValue};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

// ============================================================
// デフォルトテンプレート (バイナリ埋め込み)
// ============================================================

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
}

// ============================================================
// テンプレートマネージャ
// ============================================================

pub struct TemplateManager {
    templates_dir: PathBuf,
}

impl TemplateManager {
    pub fn new(templates_dir: PathBuf) -> Self {
        Self { templates_dir }
    }

    pub fn initialize_defaults(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.templates_dir).map_err(|e| e.to_string())?;
        let default_dir = self.templates_dir.join("default");
        if !default_dir.exists() {
            std::fs::create_dir_all(&default_dir).map_err(|e| e.to_string())?;
            for (name, content) in DefaultTemplates::get_all() {
                std::fs::write(default_dir.join(name), content).map_err(|e| e.to_string())?;
            }
        }
        let pixiv_dir = self.templates_dir.join("pixiv");
        if !pixiv_dir.exists() {
            std::fs::create_dir_all(&pixiv_dir).map_err(|e| e.to_string())?;
            for (name, content) in PixivTemplates::get_all() {
                std::fs::write(pixiv_dir.join(name), content).map_err(|e| e.to_string())?;
            }
            let base = default_dir.join("_base_style.css.j2");
            if base.exists() {
                std::fs::copy(&base, pixiv_dir.join("_base_style.css.j2"))
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn list_templates(&self) -> Result<Vec<TemplateInfo>, String> {
        let mut templates = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.templates_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if validate_template_name(&name).is_err()
                        || self.existing_template_dir(&name).is_err()
                    {
                        continue;
                    }
                    let is_builtin = name == "default" || name == "pixiv";
                    let file_count = std::fs::read_dir(entry.path())
                        .map(|e| e.flatten().filter(|f| f.path().is_file()).count() as u32)
                        .unwrap_or(0);
                    templates.push(TemplateInfo {
                        name,
                        is_builtin,
                        file_count,
                    });
                }
            }
        }
        templates.sort_by(|a, b| b.is_builtin.cmp(&a.is_builtin).then(a.name.cmp(&b.name)));
        Ok(templates)
    }

    pub fn get_template_files(&self, template_name: &str) -> Result<Vec<TemplateFile>, String> {
        let dir = self.existing_template_dir(template_name)?;
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let filename = entry.file_name().to_string_lossy().to_string();
                if validate_template_filename(&filename).is_ok()
                    && self
                        .existing_template_file(template_name, &filename)
                        .is_ok()
                {
                    files.push(TemplateFile {
                        filename,
                        size_bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
                    });
                }
            }
        }
        files.sort_by(|a, b| a.filename.cmp(&b.filename));
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
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは変更できません".to_string());
        }
        let path = self.existing_template_file(template_name, filename)?;
        std::fs::write(path, content).map_err(|e| format!("保存に失敗: {}", e))
    }

    pub fn create_template(&self, template_name: &str) -> Result<(), String> {
        validate_template_name(template_name)?;
        if is_builtin_template(template_name) {
            return Err("ビルトイン名は新規テンプレートに使用できません".to_string());
        }
        let dest = self.templates_dir.join(template_name);
        if dest.exists() {
            return Err(format!("テンプレート '{}' は既に存在します", template_name));
        }
        let source = self.existing_template_dir("default")?;
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        if let Ok(entries) = std::fs::read_dir(&source) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    std::fs::copy(entry.path(), dest.join(entry.file_name()))
                        .map_err(|e| format!("コピー失敗: {}", e))?;
                }
            }
        }
        Ok(())
    }

    pub fn delete_template(&self, template_name: &str) -> Result<(), String> {
        validate_template_name(template_name)?;
        if is_builtin_template(template_name) {
            return Err("ビルトインテンプレートは削除できません".into());
        }
        let dir = self.existing_template_dir(template_name)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("削除に失敗: {}", e))
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
        Err(format!(
            "テンプレート '{}' が見つかりません (テンプレート: {})",
            filename, template_name
        ))
    }

    /// テンプレートコンテンツをすべてロード。CSS の include を事前解決する。
    pub fn load_template_contents(
        &self,
        template_name: &str,
    ) -> Result<HashMap<String, String>, String> {
        let required_files = [
            "_base_style.css.j2",
            "style.css.j2",
            "content.opf.j2",
            "nav.xhtml.j2",
            "cover_page.xhtml.j2",
            "info_page.xhtml.j2",
            "page_wrapper.xhtml.j2",
        ];

        let mut contents = HashMap::new();
        for filename in &required_files {
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
    matches!(name, "default" | "pixiv")
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
    if filename.is_empty()
        || filename.len() > 128
        || !filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || !matches!(
            filename,
            value if value.ends_with(".css.j2")
                || value.ends_with(".xhtml.j2")
                || value.ends_with(".opf.j2")
        )
    {
        return Err("許可されていないテンプレートファイル名です".to_string());
    }
    Ok(())
}

// ============================================================
// テンプレートレンダリング
// ============================================================

pub struct EpubRenderer {
    template_contents: HashMap<String, String>,
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
}

impl EpubRenderer {
    pub fn new(template_contents: HashMap<String, String>) -> Self {
        Self { template_contents }
    }

    fn create_env(&self) -> Result<Environment<'_>, String> {
        let mut env = Environment::new();
        env.add_filter("format_number", |value: MjValue| -> String {
            if let Some(n) = value.as_i64() {
                format_number_with_commas(n)
            } else {
                value.to_string()
            }
        });
        for (name, content) in &self.template_contents {
            env.add_template_owned(name.clone(), content.clone())
                .map_err(|e| format!("テンプレート '{}' の登録に失敗: {}", name, e))?;
        }
        Ok(env)
    }

    pub fn render_content_opf(
        &self,
        manifest: &EpubManifest,
        manifest_items: &[ManifestItem],
        spine_itemrefs: &[SpineItemref],
        cover_image_id: Option<&str>,
    ) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("content.opf.j2")
            .map_err(|e| format!("content.opf.j2: {}", e))?;
        let provider_ids = context! {
            novel_id => manifest.provider.novel_id,
            post_id => manifest.provider.post_id,
            series_id => manifest.provider.series_id,
        };
        let plain_description = manifest
            .core
            .description
            .as_ref()
            .map(|d| strip_html_tags(d))
            .unwrap_or_default();
        let ctx = context! {
            manifest => minijinja::Value::from_serialize(manifest),
            manifest_items => minijinja::Value::from_serialize(manifest_items),
            spine_itemrefs => minijinja::Value::from_serialize(spine_itemrefs),
            provider_ids => provider_ids,
            cover_image_id => cover_image_id,
            plain_description => plain_description,
        };
        tmpl.render(ctx)
            .map_err(|e| format!("content.opf レンダリングエラー: {}", e))
    }

    pub fn render_nav(&self, manifest: &EpubManifest, has_cover: bool) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("nav.xhtml.j2")
            .map_err(|e| format!("nav.xhtml.j2: {}", e))?;
        let mut toc_pages: Vec<MjValue> = Vec::new();
        for page in &manifest.content.pages {
            toc_pages.push(context! {
                href => format!("text/page_{:03}.xhtml", page.order),
                title => page.title.clone().unwrap_or_else(|| format!("ページ {}", page.order)),
            });
        }
        let ctx = context! {
            toc => context! { has_info_page => true, info_page => context! { href => "text/info.xhtml", title => "作品情報" }, pages => toc_pages },
            landmarks => context! { has_cover => has_cover, info_page => context! { href => "text/info.xhtml", title => "作品情報" }, start_page_href => "text/page_001.xhtml" },
            strings => context! { TOC_TITLE => "目次", LANDMARKS_TITLE => "ランドマーク", COVER_TITLE => "表紙", BODY_MATTER_TITLE => "本文" },
        };
        tmpl.render(ctx)
            .map_err(|e| format!("nav.xhtml レンダリングエラー: {}", e))
    }

    pub fn render_cover_page(&self, cover_image_href: &str) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("cover_page.xhtml.j2")
            .map_err(|e| format!("cover_page: {}", e))?;
        tmpl.render(context! { cover_image_href => cover_image_href })
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
        let formatted_date = format_date_japanese(&manifest.core.date_published);
        let ctx = context! {
            manifest => minijinja::Value::from_serialize(manifest),
            cover_href => cover_href,
            formatted_date => formatted_date,
            css_path => "../style/style.css",
            text_length => manifest.content.text_length,
        };
        tmpl.render(ctx)
            .map_err(|e| format!("info_page レンダリングエラー: {}", e))
    }

    pub fn render_page(&self, title: &str, html_content: &str) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("page_wrapper.xhtml.j2")
            .map_err(|e| format!("page_wrapper: {}", e))?;
        tmpl.render(
            context! { title => title, content => html_content, css_path => "../style/style.css" },
        )
        .map_err(|e| format!("page レンダリングエラー: {}", e))
    }

    pub fn render_style(&self) -> Result<String, String> {
        let env = self.create_env()?;
        let tmpl = env
            .get_template("style.css.j2")
            .map_err(|e| format!("style.css: {}", e))?;
        tmpl.render(context! {})
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

fn format_date_japanese(iso_date: &str) -> String {
    if let Some(date_part) = iso_date.split('T').next() {
        let parts: Vec<&str> = date_part.split('-').collect();
        if parts.len() >= 3 {
            return format!(
                "{}年{}月{}日",
                parts[0],
                parts[1].trim_start_matches('0'),
                parts[2].trim_start_matches('0')
            );
        }
    }
    iso_date.to_string()
}

fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
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
        assert!(manager.create_template("../escape").is_err());
        assert!(manager.delete_template("..").is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builtins_are_read_only_and_custom_templates_edit_existing_files_only() {
        let (root, manager) = manager();
        assert!(manager
            .save_template_file("default", "style.css.j2", "changed")
            .is_err());

        manager.create_template("my_template-1").unwrap();
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
}
