//! EPUB エクスポート用 Tauri コマンド（進捗イベント付き）。

use crate::epub::builder::{sanitize_epub_filename, EpubBuilder};
use crate::epub::converter;
use crate::epub::intermediate::*;
use crate::epub::template::{template_file_purpose, EpubRenderer, TemplateManager, TEMPLATE_FILES};
use crate::epub::validate;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{Emitter, Manager};

/// 書き出しごとに集める検査結果の上限。何百冊でも通知が壊れないようにする。
const MAX_REPORTED_ISSUES: usize = 50;

/// 書き出しを途中でやめるための合図。
///
/// 数百冊をキューに入れて実行したら、終わるまで止められなかった。更新の
/// 確認には一時停止も中止もあるのに、こちらには何も無かった。1 冊の書き出し
/// は中断しない ―― 半端な EPUB を残さないよう、いま作っている本は書き切って
/// から止まる。
static EPUB_EXPORT_CANCEL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn epub_export_canceled() -> bool {
    EPUB_EXPORT_CANCEL.load(std::sync::atomic::Ordering::Acquire)
}

#[tauri::command]
pub async fn cancel_epub_export() -> Result<(), String> {
    EPUB_EXPORT_CANCEL.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

fn get_templates_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {}", e))?;
    Ok(app_data.join("templates"))
}

fn get_template_manager(app: &tauri::AppHandle) -> Result<TemplateManager, String> {
    let dir = get_templates_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let manager = TemplateManager::new(dir);
    manager.initialize_defaults()?;
    Ok(manager)
}

fn emit_progress(app: &tauri::AppHandle, progress: &ExportProgress) {
    let _ = app.emit("epub-export-progress", progress);
}

/// 反映済みの編集を、書き出し用の JSON へ差し込む。
///
/// 取得元が書いていたのと**同じ形**で渡す。平文へ均して渡していたころは、
/// 挿絵・改ページ・見出し・区切りがまとめて落ち、編集した作品の EPUB には
/// 絵が 1 枚も入らないまま「成功」と表示されていた。
fn apply_active_edit_to_epub_data(
    state: &Arc<AppState>,
    download_id: i64,
    source: &str,
    data: &mut serde_json::Value,
) {
    let Ok(Some(edit)) = state.db.active_edit_source_form(download_id) else {
        return;
    };
    if edit.plain_text.trim().is_empty() {
        return;
    }

    if source == "pixiv" {
        data["text"] = serde_json::Value::String(edit.pixiv_text.clone());
        if let Some(detail) = data
            .get_mut("detail")
            .and_then(|value| value.as_object_mut())
        {
            detail.insert(
                "text".to_string(),
                serde_json::Value::String(edit.pixiv_text),
            );
        }
    } else if source == "fanbox" {
        if let Some(body) = data.get_mut("body").and_then(|value| value.as_object_mut()) {
            body.insert(
                "blocks".to_string(),
                serde_json::Value::Array(edit.fanbox_blocks),
            );
            // 古いプレーンテキスト形式で読む経路のための控え。
            body.insert(
                "text".to_string(),
                serde_json::Value::String(edit.plain_text),
            );
        }
    }
}

/// 保存済みの JSON を読んで中間形式に変換する。書き出しとプレビューの共通経路。
fn load_manifest(
    state: &Arc<AppState>,
    download_id: i64,
) -> Result<(EpubManifest, String, String), String> {
    let dl = state.db.get_download(download_id)?;
    let target_json_path = dl
        .original_json_path
        .clone()
        .filter(|path| Path::new(path).exists())
        .unwrap_or_else(|| dl.json_path.clone());

    let json_content = std::fs::read_to_string(&target_json_path)
        .map_err(|e| format!("JSONの読み込みに失敗: {}", e))?;
    let mut data: serde_json::Value =
        serde_json::from_str(&json_content).map_err(|e| format!("JSON パースエラー: {}", e))?;
    if dl.source == "fanbox" {
        data = crate::fanbox_api::payload::into_post(data).ok_or_else(|| {
            "FANBOX投稿JSONの形式を解釈できないためEPUBを作成できません".to_string()
        })?;
    }
    apply_active_edit_to_epub_data(state, download_id, &dl.source, &mut data);

    let assets_dir = Path::new(&target_json_path)
        .parent()
        .ok_or_else(|| "JSONパスに親ディレクトリがありません".to_string())?
        .join("data_assets");

    crate::downloader::asset_downloader::inject_local_paths(
        &mut data,
        "data_assets",
        dl.source == "fanbox",
    );

    let manifest = converter::convert_to_manifest(&data, &dl.source, &assets_dir)?;
    Ok((manifest, dl.source, dl.title))
}

/// ページ内移動のリンクを、束ねた本での通し番号へずらす。
///
/// 作品ごとに変換した時点では、その作品の中での番号で書かれている。束ねると
/// ページは通しで振り直されるので、同じだけずらさないと隣の作品を指す。
/// 作品の中でのページ番号を、束ねた本での番号へ読み替える。
///
/// 単純に足し算でずらしていたが、長いページは XHTML を割ってあるので、
/// 「N 番目のページ」と「ページ N」はもう同じではない。作品ごとに作った
/// 対応表で引き直す ―― 割られた作品を束ねると、足し算では別の場所を指した。
fn remap_page_jump_targets(html: &str, mapping: &HashMap<u32, u32>) -> String {
    static PAGE_HREF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"href="page_(\d{3})(?:_(\d+))?\.xhtml""#)
            .expect("valid page href regex")
    });
    PAGE_HREF
        .replace_all(html, |captures: &regex::Captures| {
            let page: u32 = captures[1].parse().unwrap_or(0);
            match mapping.get(&page) {
                Some(next) => format!(r#"href="page_{:03}.xhtml""#, next),
                // 行き先の分からない番号は、そのままにしておく。書き換えて
                // 別の作品を指すより、そこで止まったほうがまだ分かる。
                None => captures[0].to_string(),
            }
        })
        .into_owned()
}

/// One source manifest becomes one or more chapters in a collection EPUB.
/// Image references and chapter anchors are namespaced before concatenation so
/// two posts that both contain `image-0` or `chapter-001` cannot cross-link.
fn namespace_manifest_for_collection(
    manifest: &mut EpubManifest,
    work_index: usize,
    work_title: &str,
) {
    let prefix = format!("w{:04}", work_index + 1);
    let mut image_keys: HashMap<String, String> = HashMap::new();
    for (image_index, image) in manifest
        .content
        .cover_image
        .iter_mut()
        .chain(manifest.content.illustrations.iter_mut())
        .enumerate()
    {
        let stem = Path::new(&image.local_path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("image")
            .to_string();
        let unique = format!("{prefix}-img{:04}", image_index + 1);
        image_keys.insert(stem.clone(), unique.clone());
        for asset_prefix in ["illust_", "inline_"] {
            if let Some(rest) = stem.strip_prefix(asset_prefix) {
                image_keys.insert(rest.to_string(), unique.clone());
            }
        }
        image_keys.insert(image.id.clone(), unique.clone());
        image.id = unique;
    }
    let image_regex = regex::Regex::new(r#"(?i)(<img\b[^>]*\bsrc\s*=\s*[\"'])([^\"']+)([\"'])"#)
        .expect("collection EPUB image regex");
    let source_url = manifest.core.main_entity_of_page.clone();
    let author = manifest.core.author.name.clone();
    let source = manifest.provider.source.clone();
    for (page_index, page) in manifest.content.pages.iter_mut().enumerate() {
        let rewritten =
            image_regex.replace_all(&page.html_content, |captures: &regex::Captures<'_>| {
                let src = captures
                    .get(2)
                    .map(|value| value.as_str())
                    .unwrap_or_default();
                let stem = src
                    .rsplit('/')
                    .next()
                    .and_then(|value| Path::new(value).file_stem())
                    .and_then(|value| value.to_str())
                    .unwrap_or(src);
                match image_keys.get(stem) {
                    Some(unique) => format!("{}{}{}", &captures[1], unique, &captures[3]),
                    None => captures[0].to_string(),
                }
            });
        page.html_content = rewritten.into_owned();
        for chapter in &mut page.chapters {
            let previous = chapter.id.clone();
            let next = format!("{prefix}-{previous}");
            page.html_content = page
                .html_content
                .replace(&format!("id=\"{previous}\""), &format!("id=\"{next}\""))
                .replace(
                    &format!("href=\"#{previous}\""),
                    &format!("href=\"#{next}\""),
                );
            chapter.id = next;
        }
        page.title = Some(if page_index == 0 {
            work_title.to_string()
        } else {
            format!("{work_title}（{}）", page_index + 1)
        });
        if page_index == 0 {
            let heading = format!(
                "<section class=\"collection-work-heading\"><h1>{}</h1><p>{} · {}</p><p><a href=\"{}\">元ページ</a></p></section><hr />",
                crate::epub::xhtml::escape_text(work_title),
                crate::epub::xhtml::escape_text(&author),
                crate::epub::xhtml::escape_text(&source),
                crate::epub::xhtml::escape_attr(&source_url),
            );
            page.html_content = format!("{heading}{}", page.html_content);
        }
    }
}

fn merge_collection_manifests(
    collection: &crate::database::WorkCollection,
    mut works: Vec<(i64, EpubManifest)>,
) -> Result<EpubManifest, String> {
    if works.is_empty() {
        return Err("コレクションに書き出せる作品がありません".to_string());
    }
    let first = works[0].1.clone();
    let mut authors = works
        .iter()
        .map(|(_, manifest)| manifest.core.author.name.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    authors.sort();
    authors.dedup();
    let author = if authors.len() == 1 {
        first.core.author.clone()
    } else {
        EpubAuthor {
            name: "複数作者".to_string(),
            id: format!("piep:collection:{}:authors", collection.summary.id),
            account: None,
            url: None,
            icon_url: None,
        }
    };
    let mut keywords = Vec::new();
    let mut tags = Vec::new();
    let mut seen_tags = HashSet::new();
    let mut pages = Vec::new();
    let mut illustrations = Vec::new();
    let mut attachments = Vec::new();
    let mut cover_image = None;
    let mut stats = EpubStats::default();
    let mut published_dates = Vec::new();
    let mut modified_dates = Vec::new();
    for (work_index, (download_id, mut manifest)) in works.drain(..).enumerate() {
        let work_title = manifest.core.name.clone();
        namespace_manifest_for_collection(&mut manifest, work_index, &work_title);
        for keyword in manifest.core.keywords.drain(..) {
            if !keywords.contains(&keyword) {
                keywords.push(keyword);
            }
        }
        for tag in manifest.core.tags.drain(..) {
            if seen_tags.insert(tag.name.clone()) {
                tags.push(tag);
            }
        }
        if !manifest.core.date_published.trim().is_empty() {
            published_dates.push(manifest.core.date_published.clone());
        }
        if let Some(value) = manifest.core.date_modified.clone() {
            modified_dates.push(value);
        }
        if let Some(image) = manifest.content.cover_image.take() {
            if collection.summary.cover_download_id == Some(download_id) || cover_image.is_none() {
                if let Some(previous) = cover_image.replace(image) {
                    illustrations.push(previous);
                }
            } else {
                illustrations.push(image);
            }
        }
        illustrations.append(&mut manifest.content.illustrations);
        attachments.append(&mut manifest.content.attachments);
        // 束ねた本ではページ番号を通しで振り直す。作品ごとに焼き込まれた
        // ページ内移動のリンクも一緒にずらさないと、2 作目の「2ページへ」が
        // 1 作目の 2 ページ目を指す ―― 行き先はあるのに、別の作品になる。
        // 割られたページも、束ねた本では 1 枚ずつの通し番号を持つ。作品の中で
        // 使われていたページ番号との対応表を先に作ってから、本文のリンクを
        // 引き直す。
        let base = pages.len() as u32;
        let work_pages = std::mem::take(&mut manifest.content.pages);
        let mut mapping: HashMap<u32, u32> = HashMap::new();
        for (index, page) in work_pages.iter().enumerate() {
            if page.part == 0 {
                mapping.insert(page.order, base + index as u32 + 1);
            }
        }
        for (index, mut page) in work_pages.into_iter().enumerate() {
            page.order = base + index as u32 + 1;
            page.part = 0;
            page.html_content = remap_page_jump_targets(&page.html_content, &mapping);
            pages.push(page);
        }
        stats.text_length = stats.text_length.saturating_add(manifest.stats.text_length);
        stats.image_count = stats.image_count.saturating_add(manifest.stats.image_count);
        stats.attachment_count = stats
            .attachment_count
            .saturating_add(manifest.stats.attachment_count);
        stats.adult |= manifest.stats.adult;
    }
    stats.page_count = pages.len() as u32;
    stats.chapter_count = pages.iter().map(|page| page.chapters.len() as u32).sum();
    stats.image_count = illustrations.len() as u32 + u32::from(cover_image.is_some());
    stats.attachment_count = attachments.len() as u32;
    published_dates.sort();
    modified_dates.sort();
    let description_text = collection.summary.description.clone();
    let description = description_text
        .as_deref()
        .map(|value| format!("<p>{}</p>", crate::epub::xhtml::escape_text(value)));
    Ok(EpubManifest {
        core: EpubCore {
            id_: crate::epub::meta::uuid_urn(&format!(
                "piep:collection:{}:revision:{}",
                collection.summary.id, collection.summary.revision
            )),
            name: collection.summary.name.clone(),
            author,
            description,
            description_text,
            keywords,
            tags,
            date_published: published_dates
                .first()
                .cloned()
                .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string()),
            date_modified: modified_dates.last().cloned(),
            main_entity_of_page: format!("piep://collections/{}", collection.summary.id),
            is_part_of: Some(EpubSeries {
                name: collection.summary.name.clone(),
                order: None,
                id: Some(collection.summary.id.clone()),
                url: None,
            }),
            language: first.core.language,
            publisher: "piep".to_string(),
        },
        provider: ProviderData {
            source: "collection".to_string(),
            novel_id: None,
            post_id: None,
            series_id: Some(collection.summary.id.clone()),
            post_type: None,
        },
        content: EpubContent {
            pages,
            cover_image,
            illustrations,
            attachments,
            text_length: stats.text_length,
        },
        stats,
    })
}

fn resolve_template(manager: &TemplateManager, template_name: &str, source: &str) -> String {
    if template_name == "__auto__" {
        manager.resolve_for_source(source)
    } else {
        template_name.to_string()
    }
}

/// 一括書き出し用の衝突しないファイル名。
///
/// タイトルだけでは、別作者の同名作品や同一シリーズの同名話が互いを上書き
/// してしまう。配信元と作品 ID を末尾に残しつつ、Windows でも扱える 120 文字
/// 以内の名前にする。
fn batch_output_filename(
    title: &str,
    source: &str,
    manifest: &EpubManifest,
    download_id: i64,
) -> String {
    let provider_id = manifest
        .provider
        .novel_id
        .as_deref()
        .or(manifest.provider.post_id.as_deref())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| download_id.to_string());

    let clean_stem = |value: &str| {
        sanitize_epub_filename(value)
            .strip_suffix(".epub")
            .unwrap_or("epub_export")
            .to_string()
    };
    let source = clean_stem(source);
    let provider_id = clean_stem(&provider_id);
    let suffix = format!("[{}-{}]", source, provider_id);
    let title = clean_stem(title);
    let title_limit = 120usize.saturating_sub(suffix.chars().count() + 1);
    let title: String = title.chars().take(title_limit.max(1)).collect();

    format!("{} {}.epub", title, suffix)
}

/// 収録できなかった画像を、書き出しの結果へ載せる。
///
/// 除いたことはログにしか残っていなかった。挿絵が数枚落ちた本でも、画面には
/// 「成功」としか出ない ―― 落ちたことを知る手立てが利用者側に無かった。
fn report_skipped_images(
    builder: &EpubBuilder,
    reported_path: &Path,
    issues: &mut Vec<EpubValidationIssue>,
) {
    let skipped = builder.skipped_images();
    if skipped.is_empty() || issues.len() >= MAX_REPORTED_ISSUES {
        return;
    }
    let name = reported_path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let shown = skipped
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join(" / ");
    let more = skipped.len().saturating_sub(5);
    issues.push(EpubValidationIssue::warning(
        "EPUB-IMAGES-SKIPPED",
        &name,
        format!(
            "{}枚の画像を収録できませんでした: {}{}",
            skipped.len(),
            shown,
            if more > 0 {
                format!(" ほか{more}枚")
            } else {
                String::new()
            }
        ),
    ));
}

/// 出来上がった EPUB を開き直して検査し、問題を積む。
fn validate_and_collect(
    path: &Path,
    reported_path: &Path,
    issues: &mut Vec<EpubValidationIssue>,
) -> bool {
    match validate::validate_epub(path) {
        Ok(report) => {
            let name = reported_path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            for issue in report.issues {
                if issue.severity == "error" {
                    log::warn!("EPUB 検証 [{}] {}: {}", name, issue.code, issue.message);
                }
                if issues.len() < MAX_REPORTED_ISSUES {
                    issues.push(EpubValidationIssue {
                        location: format!("{} › {}", name, issue.location),
                        ..issue
                    });
                }
            }
            report.valid
        }
        Err(error) => {
            log::warn!("EPUB を検証できませんでした: {}", error);
            if issues.len() < MAX_REPORTED_ISSUES {
                issues.push(EpubValidationIssue::error(
                    "EPUB-VALIDATION-FAILED",
                    reported_path.to_string_lossy().as_ref(),
                    error,
                ));
            }
            false
        }
    }
}

/// A same-directory staging file. Dropping it after any build/validation error
/// removes the partial EPUB and leaves an existing destination untouched.
struct StagedEpub {
    path: PathBuf,
    published: bool,
}

impl StagedEpub {
    fn create(destination: &Path) -> Result<Self, String> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = destination
            .file_name()
            .ok_or_else(|| "出力ファイル名がありません".to_string())?
            .to_string_lossy();
        for _ in 0..32 {
            let candidate = parent.join(format!(
                ".{name}.piep-{}-{:016x}.tmp",
                std::process::id(),
                rand::random::<u64>()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(_) => {
                    return Ok(Self {
                        path: candidate,
                        published: false,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("一時EPUBを作成できません: {error}")),
            }
        }
        Err("一時EPUBの名前を確保できません".to_string())
    }

    fn publish(mut self, destination: &Path) -> Result<(), String> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("一時EPUBを同期できません: {error}"))?;
        atomic_replace_file(&self.path, destination)?;
        self.published = true;
        sync_output_parent(destination)
    }
}

impl Drop for StagedEpub {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn build_validate_and_publish(
    destination: &Path,
    build: impl FnOnce(&Path) -> Result<(), String>,
    validate: impl FnOnce(&Path) -> bool,
) -> Result<bool, String> {
    let staged = StagedEpub::create(destination)?;
    build(&staged.path)?;
    if !validate(&staged.path) {
        return Ok(false);
    }
    staged.publish(destination)?;
    Ok(true)
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        REPLACEFILE_IGNORE_MERGE_ERRORS,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        if destination.exists() {
            ReplaceFileW(
                PCWSTR(destination_wide.as_ptr()),
                PCWSTR(source_wide.as_ptr()),
                PCWSTR::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                None,
                None,
            )
        } else {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    result.map_err(|error| {
        format!(
            "EPUBを原子的に置き換えられません ({}): {error}",
            destination.display()
        )
    })
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| {
        format!(
            "EPUBを原子的に置き換えられません ({}): {error}",
            destination.display()
        )
    })
}

#[cfg(unix)]
fn sync_output_parent(destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("EPUB出力先を同期できません: {error}"))
}

#[cfg(not(unix))]
fn sync_output_parent(_destination: &Path) -> Result<(), String> {
    Ok(())
}

fn deduplicate_download_ids(download_ids: Vec<i64>) -> Vec<i64> {
    let mut seen = HashSet::with_capacity(download_ids.len());
    download_ids
        .into_iter()
        .filter(|download_id| seen.insert(*download_id))
        .collect()
}

// ============================================================
// 単体エクスポート
// ============================================================

/// コレクションの順序をそのまま一冊の reading order / 目次へ変換する。
/// 作品ごとの出所と作者は各作品の先頭にも残し、混在取得元でも追跡できる。
#[tauri::command]
pub async fn export_collection_epub(
    app: tauri::AppHandle,
    collection_id: String,
    template_name: String,
    output_dir: String,
    compress_options: Option<ImageCompressOptions>,
    skip_missing: Option<bool>,
    writing_mode: Option<String>,
) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let _library_snapshot_guard = state.library_gate.clone().read_owned().await;
    let collection_state = state.clone();
    let collection = tokio::task::spawn_blocking(move || {
        collection_state.db.get_work_collection(&collection_id)
    })
    .await
    .map_err(|error| format!("コレクション読み込みワーカーが予期せず終了しました: {error}"))??;
    if collection.members.is_empty() {
        return Err("コレクションに作品がありません".to_string());
    }
    // 本文が0字の作品も「収録できない」側に数える。手元に行はあるが中身が
    // 無い（有料記事を取り切れていない）ので、そのまま入れると空の章になる。
    // 実測で 29 件あった。
    let missing = collection
        .members
        .iter()
        .filter(|member| member.download_id.is_none() || member.text_length == 0)
        .map(|member| member.title.clone())
        .collect::<Vec<_>>();
    // 欠落は既定では中止だが、利用者が承知のうえで「除外して続行」を選べる。
    // ライブラリから作品を 1 件消しただけでコレクションが永久に書き出せなく
    // なるのを避けるための分岐で、除外した件数は完了メッセージにも残す。
    if !missing.is_empty() && !skip_missing.unwrap_or(false) {
        return Err(format!(
            "本文を収録できない作品が{}件あるため、内容を欠かさず一冊にできません: {}",
            missing.len(),
            missing
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    let exportable = collection
        .members
        .iter()
        .filter(|member| member.download_id.is_some() && member.text_length > 0)
        .collect::<Vec<_>>();
    if exportable.is_empty() {
        return Err("保存済みの作品がないため一冊にできません".to_string());
    }
    let total = exportable.len() as u32;
    emit_progress(
        &app,
        &ExportProgress {
            phase: "started".into(),
            current_title: collection.summary.name.clone(),
            current_index: 0,
            total_count: total,
            message: format!("「{}」を一冊にまとめています", collection.summary.name),
        },
    );
    let mut manifests = Vec::with_capacity(exportable.len());
    for (index, member) in exportable.iter().enumerate() {
        let download_id = member.download_id.expect("filtered to saved works above");
        emit_progress(
            &app,
            &ExportProgress {
                phase: "converting".into(),
                current_title: member.title.clone(),
                current_index: (index + 1) as u32,
                total_count: total,
                message: format!("[{}/{}] 「{}」を変換中", index + 1, total, member.title),
            },
        );
        let load_state = state.clone();
        let loaded = tokio::task::spawn_blocking(move || load_manifest(&load_state, download_id))
            .await
            .map_err(|error| format!("EPUB変換ワーカーが予期せず終了しました: {error}"))?;
        let (manifest, _, _) =
            loaded.map_err(|error| format!("「{}」を変換できません: {error}", member.title))?;
        manifests.push((download_id, manifest));
    }
    let manifest = merge_collection_manifests(&collection, manifests)?;
    let manager = get_template_manager(&app)?;
    // 取得元固有の情報ページは混在本に合わないため、自動時は共通テンプレート。
    let resolved_template = if template_name == "__auto__" {
        "default".to_string()
    } else {
        template_name
    };
    let contents = manager.load_template_contents(&resolved_template)?;
    let settings = manager.read_settings(&resolved_template);
    let compress = compress_options.unwrap_or_default();
    let output_base = PathBuf::from(&output_dir);
    let destination = output_base.join(sanitize_epub_filename(&collection.summary.name));
    let reported_destination = destination.clone();
    let app_clone = app.clone();
    let title = collection.summary.name.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&output_base)
            .map_err(|error| format!("出力先を作成できません: {error}"))?;
        let builder = EpubBuilder::new(manifest, contents, settings, compress)
            .with_writing_mode(writing_mode.as_deref());
        let mut issues = Vec::new();
        let valid = build_validate_and_publish(
            &destination,
            |staged_path| builder.build(staged_path),
            |staged_path| validate_and_collect(staged_path, &destination, &mut issues),
        )?;
        report_skipped_images(&builder, &destination, &mut issues);
        for issue in &issues {
            if issue.code == "EPUB-IMAGES-SKIPPED" {
                log::warn!("{}: {}", issue.location, issue.message);
            }
        }
        if valid {
            Ok(())
        } else {
            let detail = issues
                .iter()
                .take(3)
                .map(|issue| format!("{}: {}", issue.code, issue.message))
                .collect::<Vec<_>>()
                .join(" / ");
            Err(format!("生成後のEPUB検証を通過しませんでした: {detail}"))
        }
    })
    .await
    .map_err(|error| format!("EPUBワーカーがパニックしました: {error}"))??;
    emit_progress(
        &app_clone,
        &ExportProgress {
            phase: "completed".into(),
            current_title: title.clone(),
            current_index: total,
            total_count: total,
            message: if missing.is_empty() {
                format!("「{title}」を一冊のEPUBに書き出しました")
            } else {
                format!(
                    "「{title}」を一冊のEPUBに書き出しました（未保存の{}件は除外）",
                    missing.len()
                )
            },
        },
    );
    Ok(reported_destination.to_string_lossy().to_string())
}

// ============================================================
// バッチエクスポート
// ============================================================

/// Image decoding/compression and validation both have a large per-book peak.
/// Run one book at a time: parallel books made batch export compete with the
/// UI for RAM and CPU even though each individual build is already parallel
/// where the image codec benefits from it.
const MAX_CONCURRENT_EPUB_BUILDS: usize = 1;

enum BatchItemStatus {
    Published(String),
    Invalid,
    Failed(String),
}

struct BatchItemResult {
    index: u32,
    download_id: i64,
    status: BatchItemStatus,
    issues: Vec<EpubValidationIssue>,
}

fn aggregate_batch_results(mut completed: Vec<BatchItemResult>) -> ExportBatchResult {
    // Completion order is intentionally concurrent; result arrays retain the
    // caller's order so retry queues and file lists remain deterministic.
    completed.sort_by_key(|item| item.index);
    let mut result = ExportBatchResult {
        success_count: 0,
        failed_count: 0,
        failed_ids: Vec::new(),
        invalid_ids: Vec::new(),
        output_files: Vec::new(),
        invalid_count: 0,
        issues: Vec::new(),
        canceled: false,
        skipped_ids: Vec::new(),
    };
    for item in completed {
        for issue in item.issues {
            if result.issues.len() >= MAX_REPORTED_ISSUES {
                break;
            }
            result.issues.push(issue);
        }
        match item.status {
            BatchItemStatus::Published(path) => {
                result.success_count += 1;
                result.output_files.push(path);
            }
            BatchItemStatus::Invalid => {
                // Invalid is a failure (and a more specific subset of it), not
                // a successful output with a warning. No file was published.
                result.failed_count += 1;
                result.invalid_count += 1;
                result.invalid_ids.push(item.download_id);
            }
            BatchItemStatus::Failed(error) => {
                log::debug!("EPUB ID {} failed: {}", item.download_id, error);
                result.failed_count += 1;
                result.failed_ids.push(item.download_id);
            }
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn export_batch_item(
    app: tauri::AppHandle,
    state: Arc<AppState>,
    templates_dir: PathBuf,
    download_id: i64,
    index: u32,
    total: u32,
    template_name: String,
    output_base: PathBuf,
    compress: ImageCompressOptions,
    writing_mode: Option<String>,
) -> BatchItemResult {
    let title = state
        .db
        .get_download(download_id)
        .map(|download| download.title)
        .unwrap_or_else(|_| format!("ID {download_id}"));
    emit_progress(
        &app,
        &ExportProgress {
            phase: "converting".into(),
            current_title: title.clone(),
            current_index: index,
            total_count: total,
            message: format!("[{index}/{total}] 「{title}」を変換中..."),
        },
    );

    let mut issues = Vec::new();
    let result = (|| {
        let (manifest, source, loaded_title) = load_manifest(&state, download_id)?;
        let manager = TemplateManager::new(templates_dir);
        let resolved = resolve_template(&manager, &template_name, &source);
        let contents = manager.load_template_contents(&resolved)?;
        let settings = manager.read_settings(&resolved);
        let filename = batch_output_filename(&loaded_title, &source, &manifest, download_id);
        let destination = output_base.join(filename);
        let builder = EpubBuilder::new(manifest, contents, settings, compress)
            .with_writing_mode(writing_mode.as_deref());
        let valid = build_validate_and_publish(
            &destination,
            |staged_path| builder.build(staged_path),
            |staged_path| validate_and_collect(staged_path, &destination, &mut issues),
        )?;
        // 置いていった絵は、ログではなく書き出しの結果に出す。挿絵が数枚
        // 落ちても「成功」としか見えなかった。
        report_skipped_images(&builder, &destination, &mut issues);
        Ok::<_, String>((destination, valid))
    })();

    let status = match result {
        Ok((destination, true)) => {
            emit_progress(
                &app,
                &ExportProgress {
                    phase: "completed".into(),
                    current_title: title.clone(),
                    current_index: index,
                    total_count: total,
                    message: format!("[{index}/{total}] 「{title}」完了"),
                },
            );
            BatchItemStatus::Published(destination.to_string_lossy().to_string())
        }
        Ok((_destination, false)) => {
            emit_progress(
                &app,
                &ExportProgress {
                    phase: "failed".into(),
                    current_title: title.clone(),
                    current_index: index,
                    total_count: total,
                    message: format!("[{index}/{total}] 「{title}」失敗: EPUB検証エラー"),
                },
            );
            BatchItemStatus::Invalid
        }
        Err(error) => {
            log::error!("EPUB export error (ID {}): {}", download_id, error);
            emit_progress(
                &app,
                &ExportProgress {
                    phase: "failed".into(),
                    current_title: title.clone(),
                    current_index: index,
                    total_count: total,
                    message: format!("[{index}/{total}] 「{title}」失敗: {error}"),
                },
            );
            BatchItemStatus::Failed(error)
        }
    };

    BatchItemResult {
        index,
        download_id,
        status,
        issues,
    }
}

#[tauri::command]
pub async fn export_epub_batch(
    app: tauri::AppHandle,
    download_ids: Vec<i64>,
    template_name: String,
    output_dir: String,
    compress_options: Option<ImageCompressOptions>,
    writing_mode: Option<String>,
) -> Result<ExportBatchResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let _library_snapshot_guard = state.library_gate.clone().read_owned().await;
    let templates_dir = get_templates_dir(&app)?;
    TemplateManager::new(templates_dir.clone()).initialize_defaults()?;
    let compress = compress_options.unwrap_or_default();
    let output_base = PathBuf::from(&output_dir);
    std::fs::create_dir_all(&output_base).map_err(|e| e.to_string())?;

    let download_ids = deduplicate_download_ids(download_ids);
    let total = download_ids.len() as u32;

    emit_progress(
        &app,
        &ExportProgress {
            phase: "started".into(),
            current_title: String::new(),
            current_index: 0,
            total_count: total,
            message: format!("{}件のEPUBエクスポートを開始", total),
        },
    );

    EPUB_EXPORT_CANCEL.store(false, std::sync::atomic::Ordering::Release);
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = download_ids.into_iter().enumerate();
    let mut completed = Vec::with_capacity(total as usize);
    loop {
        while tasks.len() < MAX_CONCURRENT_EPUB_BUILDS {
            // 止めると言われたら、そこから先は始めない。
            if epub_export_canceled() {
                break;
            }
            let Some((index, download_id)) = pending.next() else {
                break;
            };
            let app = app.clone();
            let state = state.clone();
            let templates_dir = templates_dir.clone();
            let template_name = template_name.clone();
            let output_base = output_base.clone();
            let compress = compress.clone();
            let writing_mode = writing_mode.clone();
            tasks.spawn_blocking(move || {
                export_batch_item(
                    app,
                    state,
                    templates_dir,
                    download_id,
                    (index + 1) as u32,
                    total,
                    template_name,
                    output_base,
                    compress,
                    writing_mode,
                )
            });
        }
        if tasks.is_empty() {
            break;
        }
        let joined = tasks
            .join_next()
            .await
            .ok_or_else(|| "EPUBワーカーが予期せず終了しました".to_string())?
            .map_err(|error| format!("EPUBワーカーがパニックしました: {error}"))?;
        completed.push(joined);
    }

    let canceled = epub_export_canceled();
    EPUB_EXPORT_CANCEL.store(false, std::sync::atomic::Ordering::Release);
    let mut result = aggregate_batch_results(completed);
    result.canceled = canceled;
    // 一度も試していない作品はキューに残す。止めた結果として棚から
    // 消えてしまっては、中止が「取り下げ」になってしまう。
    result.skipped_ids = pending.map(|(_, download_id)| download_id).collect();

    emit_progress(
        &app,
        &ExportProgress {
            phase: if canceled { "canceled" } else { "completed" }.into(),
            current_title: String::new(),
            current_index: total,
            total_count: total,
            message: if canceled {
                format!(
                    "書き出しを中止しました: {}件成功, {}件失敗（残りはキューに戻します）",
                    result.success_count, result.failed_count
                )
            } else {
                format!(
                    "エクスポート完了: {}件成功, {}件失敗",
                    result.success_count, result.failed_count
                )
            },
        },
    );

    Ok(result)
}

// ============================================================
// テンプレート管理コマンド
// ============================================================

#[tauri::command]
pub async fn list_epub_templates(app: tauri::AppHandle) -> Result<Vec<TemplateInfo>, String> {
    get_template_manager(&app)?.list_templates()
}

#[tauri::command]
pub async fn get_template_files(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<Vec<TemplateFile>, String> {
    get_template_manager(&app)?.get_template_files(&template_name)
}

#[tauri::command]
pub async fn read_template_file(
    app: tauri::AppHandle,
    template_name: String,
    filename: String,
) -> Result<String, String> {
    get_template_manager(&app)?.read_template_file(&template_name, &filename)
}

#[tauri::command]
pub async fn save_template_file(
    app: tauri::AppHandle,
    template_name: String,
    filename: String,
    content: String,
) -> Result<(), String> {
    get_template_manager(&app)?.save_template_file(&template_name, &filename, &content)
}

#[tauri::command]
pub async fn reset_template_file(
    app: tauri::AppHandle,
    template_name: String,
    filename: String,
) -> Result<String, String> {
    get_template_manager(&app)?.reset_template_file(&template_name, &filename)
}

#[tauri::command]
pub async fn create_epub_template(
    app: tauri::AppHandle,
    template_name: String,
    base_template: Option<String>,
) -> Result<(), String> {
    get_template_manager(&app)?.create_template(
        &template_name,
        base_template.as_deref().unwrap_or("default"),
    )
}

#[tauri::command]
pub async fn rename_epub_template(
    app: tauri::AppHandle,
    template_name: String,
    next_name: String,
) -> Result<(), String> {
    get_template_manager(&app)?.rename_template(&template_name, &next_name)
}

#[tauri::command]
pub async fn delete_epub_template(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<(), String> {
    get_template_manager(&app)?.delete_template(&template_name)
}

#[tauri::command]
pub async fn save_template_settings(
    app: tauri::AppHandle,
    template_name: String,
    settings: TemplateSettings,
) -> Result<TemplateSettings, String> {
    get_template_manager(&app)?.save_settings(&template_name, settings)
}

/// 編集画面が出す、ファイルごとの役割の一覧。
#[tauri::command]
pub async fn list_template_file_kinds() -> Result<Vec<TemplateFileKind>, String> {
    Ok(TEMPLATE_FILES
        .iter()
        .map(|filename| TemplateFileKind {
            filename: filename.to_string(),
            purpose: template_file_purpose(filename).to_string(),
            language: if filename.ends_with(".css.j2") {
                "css".into()
            } else {
                "xml".into()
            },
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFileKind {
    pub filename: String,
    pub purpose: String,
    /// 編集画面の色分け用。
    pub language: String,
}

// ============================================================
// プレビューとデータ辞書
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePreview {
    /// プレビューに使った作品。
    pub sample_title: String,
    pub sample_source: String,
    pub sample_download_id: Option<i64>,
    pub css: String,
    pub cover: Option<String>,
    pub info: Option<String>,
    pub page: Option<String>,
    pub nav: String,
    pub opf: String,
    pub ncx: Option<String>,
    /// 描画した文書を整形式かどうか検査した結果。
    pub issues: Vec<EpubValidationIssue>,
    pub fields: Vec<DataField>,
}

/// テンプレートから差し込める値ひとつ。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataField {
    /// テンプレートに書く式 (`core.name` など)。
    pub path: String,
    pub group: String,
    pub label: String,
    pub sample: String,
    /// この作品で値が入っているか。
    pub available: bool,
}

#[tauri::command]
pub async fn preview_epub_template(
    app: tauri::AppHandle,
    template_name: String,
    download_id: Option<i64>,
) -> Result<TemplatePreview, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let _library_snapshot_guard = state.library_gate.clone().read_owned().await;
    let tm = get_template_manager(&app)?;

    let (manifest, source, title, resolved_id) = match download_id {
        Some(id) => {
            let load_state = state.clone();
            let (manifest, source, title) =
                tokio::task::spawn_blocking(move || load_manifest(&load_state, id))
                    .await
                    .map_err(|error| {
                        format!("EPUBプレビューワーカーが予期せず終了しました: {error}")
                    })??;
            (manifest, source, title, Some(id))
        }
        None => {
            let manifest = sample_manifest();
            (
                manifest,
                "sample".to_string(),
                "見本の作品".to_string(),
                None,
            )
        }
    };

    let contents = tm.load_template_contents(&template_name)?;
    let settings = tm.read_settings(&template_name);
    let renderer = EpubRenderer::new(contents, settings.clone());

    // 画像はプレビューの中で完結させたいので、その場でデータ URI に変える。
    let cover_uri = manifest
        .content
        .cover_image
        .as_ref()
        .and_then(|image| thumbnail_data_uri(Path::new(&image.local_path)));

    let css = renderer.render_style(&manifest)?;
    let cover = match (&cover_uri, settings.include_cover_page) {
        (Some(uri), true) => Some(renderer.render_cover_page(&manifest, uri, 0, 0)?),
        _ => None,
    };
    let info = settings
        .include_info_page
        .then(|| renderer.render_info_page(&manifest, cover_uri.as_deref()))
        .transpose()?;

    let page = match manifest.content.pages.first() {
        Some(first) => {
            let html = preview_page_html(first, &manifest);
            Some(renderer.render_page(&manifest, first, &html)?)
        }
        None => None,
    };

    let entries = preview_nav_entries(&manifest, &settings);
    let nav = renderer.render_nav(
        &manifest,
        &entries,
        cover.is_some(),
        Some("text/page_001.xhtml"),
    )?;
    let ncx = settings
        .include_ncx
        .then(|| renderer.render_ncx(&manifest, &entries, &manifest.core.id_))
        .transpose()?;
    let opf = renderer.render_content_opf(
        &manifest,
        &[],
        &[],
        cover_uri.as_ref().map(|_| "cover-image"),
        &crate::epub::meta::to_utc_timestamp(&manifest.core.date_published)
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()),
        crate::epub::meta::to_utc_timestamp(&manifest.core.date_published).as_deref(),
    )?;

    // 描いたそばから整形式かどうかを見る。テンプレートの書き間違いは
    // 書き出してからではなく、編集している最中に分かるのが望ましい。
    let mut issues = Vec::new();
    for (name, document) in [
        ("cover_page.xhtml.j2", cover.as_deref()),
        ("info_page.xhtml.j2", info.as_deref()),
        ("page_wrapper.xhtml.j2", page.as_deref()),
        ("nav.xhtml.j2", Some(nav.as_str())),
        ("toc.ncx.j2", ncx.as_deref()),
        ("content.opf.j2", Some(opf.as_str())),
    ] {
        let Some(document) = document else { continue };
        if let Err(error) = validate::check_well_formed(document) {
            issues.push(EpubValidationIssue::error(
                "TEMPLATE-NOT-WELL-FORMED",
                name,
                format!("整形式ではない出力になります: {}", error),
            ));
        }
    }

    Ok(TemplatePreview {
        sample_title: title,
        sample_source: source,
        sample_download_id: resolved_id,
        css,
        cover,
        info,
        page,
        nav,
        opf,
        ncx,
        issues,
        fields: describe_manifest(&manifest),
    })
}

/// プレビュー用の本文。挿絵はデータ URI に置き換え、長すぎる本文は切り詰める。
fn preview_page_html(page: &EpubPage, manifest: &EpubManifest) -> String {
    let html = crate::epub::xhtml::sanitize_fragment_with(&page.html_content, &mut |src| {
        let stem = Path::new(src.rsplit('/').next().unwrap_or(src))
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let matched = manifest
            .content
            .illustrations
            .iter()
            .find(|image| image.local_path.contains(&stem))
            .and_then(|image| thumbnail_data_uri(Path::new(&image.local_path)));
        match matched {
            Some(uri) => crate::epub::xhtml::ImageRef::Keep(uri),
            None => crate::epub::xhtml::ImageRef::Drop,
        }
    });
    // 一画面ぶん見えれば足りる。全文を渡すと編集のたびに重くなる。
    if html.chars().count() > 6000 {
        let truncated: String = html.chars().take(6000).collect();
        return crate::epub::xhtml::sanitize_fragment(&truncated);
    }
    html
}

fn preview_nav_entries(
    manifest: &EpubManifest,
    settings: &TemplateSettings,
) -> Vec<crate::epub::template::NavEntry> {
    use crate::epub::template::NavEntry;
    let mut entries = Vec::new();
    let mut order = 0u32;
    if settings.include_info_page {
        order += 1;
        entries.push(NavEntry {
            id: "info".into(),
            order,
            href: "text/info.xhtml".into(),
            title: settings
                .strings
                .get("INFO_TITLE")
                .cloned()
                .unwrap_or_else(|| "作品情報".into()),
            children: Vec::new(),
        });
    }
    for page in &manifest.content.pages {
        order += 1;
        let page_order = order;
        let href = format!("text/page_{:03}.xhtml", page.order);
        let mut children = Vec::new();
        if settings.chapter_toc {
            for chapter in &page.chapters {
                order += 1;
                children.push(NavEntry {
                    id: chapter.id.clone(),
                    order,
                    href: format!("{}#{}", href, chapter.id),
                    title: chapter.title.clone(),
                    children: Vec::new(),
                });
            }
        }
        entries.push(NavEntry {
            id: format!("page-{:03}", page.order),
            order: page_order,
            href,
            title: page
                .title
                .clone()
                .unwrap_or_else(|| format!("ページ {}", page.order)),
            children,
        });
    }
    entries
}

/// プレビューに埋める小さな画像。
fn thumbnail_data_uri(path: &Path) -> Option<String> {
    use base64::Engine;
    let image = image::ImageReader::open(path).ok()?.decode().ok()?;
    let thumbnail = image.thumbnail(480, 720);
    let mut buffer = std::io::Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut buffer, image::ImageFormat::Jpeg)
        .ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(buffer.into_inner());
    Some(format!("data:image/jpeg;base64,{}", encoded))
}

// ============================================================
// データ辞書
// ============================================================

/// テンプレートに書ける値の一覧を、実際の中身つきで作る。
///
/// 一覧を手で持つと、抽出側が増えたときに必ず食い違う。中間形式そのものから
/// 起こすことで、「取り出せるもの」と「差し込めるもの」を常に一致させる。
fn describe_manifest(manifest: &EpubManifest) -> Vec<DataField> {
    let Ok(value) = serde_json::to_value(manifest) else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    walk_value(&value, "", "", &mut fields);
    fields.sort_by(|a, b| a.group.cmp(&b.group).then(a.path.cmp(&b.path)));
    fields
}

fn walk_value(value: &serde_json::Value, path: &str, group: &str, out: &mut Vec<DataField>) {
    // 入れ子は 3 段まで。それより深いものはテンプレートから触る対象ではない。
    if path.matches('.').count() > 3 {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let next = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                let group = if group.is_empty() {
                    key.as_str()
                } else {
                    group
                };
                walk_value(child, &next, group, out);
            }
        }
        serde_json::Value::Array(items) => {
            out.push(DataField {
                label: format!("{}（{}件）", field_label(path), items.len()),
                sample: if items.is_empty() {
                    "（空）".into()
                } else {
                    format!("{}件", items.len())
                },
                available: !items.is_empty(),
                group: group.to_string(),
                path: path.to_string(),
            });
            if let Some(first) = items.first() {
                walk_value(first, &format!("{}[0]", path), group, out);
            }
        }
        serde_json::Value::Null => out.push(DataField {
            label: field_label(path),
            sample: "（なし）".into(),
            available: false,
            group: group.to_string(),
            path: path.to_string(),
        }),
        other => {
            let text = match other {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let sample: String = text.chars().take(120).collect();
            out.push(DataField {
                label: field_label(path),
                available: !sample.trim().is_empty(),
                sample: if sample.trim().is_empty() {
                    "（空）".into()
                } else {
                    sample
                },
                group: group.to_string(),
                path: path.to_string(),
            });
        }
    }
}

fn field_label(path: &str) -> String {
    let known = match path {
        "core.name" => "作品タイトル",
        "core.id_" => "識別子 (URN)",
        "core.author.name" => "作者名",
        "core.author.id" => "作者ID",
        "core.author.account" => "アカウント名",
        "core.author.url" => "作者ページURL",
        "core.author.iconUrl" => "作者アイコンURL",
        "core.description" => "紹介文 (XHTML)",
        "core.descriptionText" => "紹介文 (テキスト)",
        "core.keywords" => "タグ名の一覧",
        "core.tags" => "タグ (英訳つき)",
        "core.datePublished" => "公開日時",
        "core.dateModified" => "更新日時",
        "core.mainEntityOfPage" => "配信元URL",
        "core.isPartOf.name" => "シリーズ名",
        "core.isPartOf.order" => "シリーズ内の話数",
        "core.isPartOf.url" => "シリーズURL",
        "core.language" => "言語",
        "core.publisher" => "配信元",
        "provider.source" => "取得元 (pixiv / fanbox)",
        "provider.novelId" => "pixiv 小説ID",
        "provider.novel_id" => "pixiv 小説ID",
        "provider.post_id" => "FANBOX 投稿ID",
        "provider.series_id" => "シリーズID",
        "provider.postType" => "投稿種別",
        "stats.textLength" => "文字数",
        "stats.pageCount" => "ページ数",
        "stats.chapterCount" => "章の数",
        "stats.imageCount" => "画像点数",
        "stats.attachmentCount" => "添付ファイル数",
        "stats.likeCount" => "いいね / ブックマーク",
        "stats.commentCount" => "コメント数",
        "stats.feeRequired" => "必要な支援額",
        "stats.adult" => "年齢制限",
        "content.pages" => "本文ページ",
        "content.illustrations" => "挿絵",
        "content.attachments" => "添付ファイル",
        "content.cover_image" => "表紙画像",
        "content.text_length" => "文字数",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    path.rsplit('.').next().unwrap_or(path).to_string()
}

/// 作品を選ばずにテンプレートを見比べるための見本。
fn sample_manifest() -> EpubManifest {
    let pages = vec![EpubPage {
        title: Some("第一章 はじまり".into()),
        html_content: "<h2 id=\"chapter-001\">第一章 はじまり</h2>\n<p>雨の匂いがした。<ruby>硝子<rp>(</rp><rt>ガラス</rt><rp>)</rp></ruby>越しの街は、いつもより遠く見える。</p>\n<p class=\"blank-line\"><br /></p>\n<p>「行こうか」と、彼女は言った。</p>".into(),
        order: 1,
        chapters: vec![EpubChapter {
            id: "chapter-001".into(),
            title: "第一章 はじまり".into(),
        }],
        part: 0,
    }];
    EpubManifest {
        core: EpubCore {
            id_: "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
            name: "見本の作品 ― 雨と硝子".into(),
            author: EpubAuthor {
                name: "見本 作者".into(),
                id: "0".into(),
                account: Some("sample".into()),
                url: Some("https://www.pixiv.net/users/0".into()),
                icon_url: None,
            },
            description: Some("<p>テンプレートの見え方を確かめるための紹介文です。</p>".into()),
            description_text: Some("テンプレートの見え方を確かめるための紹介文です。".into()),
            keywords: vec!["見本".into(), "テンプレート".into()],
            tags: vec![
                EpubTag {
                    name: "見本".into(),
                    translated: Some("sample".into()),
                },
                EpubTag {
                    name: "テンプレート".into(),
                    translated: None,
                },
            ],
            date_published: "2024-04-01T09:00:00+09:00".into(),
            date_modified: Some("2024-05-02T12:00:00+09:00".into()),
            main_entity_of_page: "https://www.pixiv.net/novel/show.php?id=0".into(),
            is_part_of: Some(EpubSeries {
                name: "見本のシリーズ".into(),
                order: Some(2),
                id: Some("0".into()),
                url: Some("https://www.pixiv.net/novel/series/0".into()),
            }),
            language: "ja".into(),
            publisher: "pixiv".into(),
        },
        provider: ProviderData {
            source: "sample".into(),
            novel_id: Some("0".into()),
            post_id: None,
            series_id: Some("0".into()),
            post_type: None,
        },
        content: EpubContent {
            pages,
            cover_image: None,
            illustrations: Vec::new(),
            attachments: Vec::new(),
            text_length: 1234,
        },
        stats: EpubStats {
            text_length: 1234,
            page_count: 1,
            chapter_count: 1,
            image_count: 0,
            attachment_count: 0,
            like_count: Some(321),
            comment_count: Some(12),
            fee_required: None,
            adult: false,
        },
    }
}

// ============================================================
// 検証
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 束ねた本の中で、ページ内移動が自分の作品を指し続けること。
    ///
    /// 作品ごとに焼き込まれた番号のままだと、2 作目の「2ページへ」が
    /// 1 作目の 2 ページ目を開く。行き先はあるのに別の作品なので、
    /// 検証では捕まらない種類の壊れ方をする。
    #[test]
    fn page_jumps_follow_their_work_when_books_are_bound_together() {
        let html = r#"<p><a href="page_002.xhtml">2ページへ</a></p>"#;
        let mapping = HashMap::from([(1u32, 6u32), (2, 7)]);
        assert_eq!(
            remap_page_jump_targets(html, &mapping),
            r#"<p><a href="page_007.xhtml">2ページへ</a></p>"#
        );
        // 対応表に無い番号は書き換えない。別の作品を指すより、そこで止まる。
        assert_eq!(
            remap_page_jump_targets(r#"<a href="page_009.xhtml">9</a>"#, &mapping),
            r#"<a href="page_009.xhtml">9</a>"#
        );
        // 割られたページの続きも、元のページの行き先へ読み替える。
        assert_eq!(
            remap_page_jump_targets(r#"<a href="page_002_2.xhtml">2</a>"#, &mapping),
            r#"<a href="page_007.xhtml">2</a>"#
        );
        // 画像や章のリンクには触れない。
        let other = r##"<img src="../images/w0001-img0001.jpg" /><a href="#chapter-001">章</a>"##;
        assert_eq!(remap_page_jump_targets(other, &mapping), other);
    }

    #[test]
    fn the_data_dictionary_covers_everything_a_template_can_place() {
        let fields = describe_manifest(&sample_manifest());
        let paths: Vec<&str> = fields.iter().map(|field| field.path.as_str()).collect();
        for expected in [
            "core.name",
            "core.author.name",
            "core.isPartOf.name",
            "core.tags",
            "stats.textLength",
            "stats.likeCount",
            "content.pages",
            "provider.source",
        ] {
            assert!(paths.contains(&expected), "{expected} が一覧にありません");
        }
        let title = fields
            .iter()
            .find(|field| field.path == "core.name")
            .unwrap();
        assert_eq!(title.label, "作品タイトル");
        assert!(title.available);
        assert_eq!(title.group, "core");

        // 値の無い項目は「差し込めるが今は空」と分かるように残す。
        let cover = fields
            .iter()
            .find(|field| field.path == "content.cover_image")
            .unwrap();
        assert!(!cover.available);
    }

    #[test]
    fn batch_filenames_keep_provider_ids_and_do_not_collide_on_equal_titles() {
        let pixiv = sample_manifest();
        let pixiv_name = batch_output_filename("同じ/題名", "pixiv", &pixiv, 10);
        assert_eq!(pixiv_name, "同じ_題名 [pixiv-0].epub");

        let mut fanbox = sample_manifest();
        fanbox.provider.novel_id = None;
        fanbox.provider.post_id = Some("99".into());
        let fanbox_name = batch_output_filename("同じ/題名", "fanbox", &fanbox, 11);
        assert_eq!(fanbox_name, "同じ_題名 [fanbox-99].epub");
        assert_ne!(pixiv_name, fanbox_name);

        let long_name = batch_output_filename(&"あ".repeat(300), "pixiv", &pixiv, 10);
        assert!(long_name.ends_with(" [pixiv-0].epub"));
        assert_eq!(long_name.trim_end_matches(".epub").chars().count(), 120);
    }

    #[test]
    fn duplicate_batch_ids_are_removed_without_reordering() {
        assert_eq!(
            deduplicate_download_ids(vec![7, 3, 7, 9, 3, 11]),
            vec![7, 3, 9, 11]
        );
    }

    #[test]
    fn collection_merge_namespaces_chapters_and_preserves_work_order() {
        let mut first = sample_manifest();
        first.core.name = "前編".into();
        first.core.author.name = "作者A".into();
        first.content.illustrations.push(EpubImage {
            id: "image-0".into(),
            local_path: "C:/missing/first/image.png".into(),
            mime_type: "image/png".into(),
            alt_text: Some("前編の挿絵".into()),
            caption: None,
            width: None,
            height: None,
        });
        first.content.pages[0]
            .html_content
            .push_str(r#"<img src="image.png" alt="前編の挿絵">"#);
        let mut second = sample_manifest();
        second.core.name = "後編".into();
        second.core.author.name = "作者B".into();
        second.content.illustrations.push(EpubImage {
            id: "image-0".into(),
            local_path: "C:/missing/second/image.png".into(),
            mime_type: "image/png".into(),
            alt_text: Some("後編の挿絵".into()),
            caption: None,
            width: None,
            height: None,
        });
        second.content.pages[0]
            .html_content
            .push_str(r#"<img src="image.png" alt="後編の挿絵">"#);
        let collection = crate::database::WorkCollection {
            summary: crate::database::WorkCollectionSummary {
                id: "collection-test".into(),
                name: "前後編まとめ".into(),
                description: Some("二作品を読む順に収録".into()),
                collection_kind: "ordered".into(),
                cover_download_id: None,
                cover_path: None,
                cover_mode: "mosaic".into(),
                cover_image_path: None,
                cover_tiles: Vec::new(),
                name_source: "manual".into(),
                track: "manual".into(),
                revision: 3,
                member_count: 2,
                available_count: 2,
                total_text_length: 2468,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-02T00:00:00Z".into(),
            },
            members: Vec::new(),
        };
        let merged =
            merge_collection_manifests(&collection, vec![(10, first), (11, second)]).unwrap();
        assert_eq!(merged.core.name, "前後編まとめ");
        assert_eq!(merged.core.author.name, "複数作者");
        assert_eq!(merged.content.pages.len(), 2);
        assert_eq!(merged.content.pages[0].title.as_deref(), Some("前編"));
        assert_eq!(merged.content.pages[1].title.as_deref(), Some("後編"));
        assert_eq!(merged.content.pages[0].order, 1);
        assert_eq!(merged.content.pages[1].order, 2);
        assert_ne!(
            merged.content.pages[0].chapters[0].id,
            merged.content.pages[1].chapters[0].id
        );
        assert_eq!(merged.content.illustrations[0].id, "w0001-img0001");
        assert_eq!(merged.content.illustrations[1].id, "w0002-img0001");
        assert!(merged.content.pages[0]
            .html_content
            .contains("src=\"w0001-img0001\""));
        assert!(merged.content.pages[1]
            .html_content
            .contains("src=\"w0002-img0001\""));
        assert!(merged.content.pages[0]
            .html_content
            .contains("class=\"collection-work-heading\""));

        let dir = atomic_test_dir("collection-merge");
        let templates = dir.join("templates");
        let manager = TemplateManager::new(templates);
        manager.initialize_defaults().unwrap();
        let contents = manager.load_template_contents("default").unwrap();
        let settings = manager.read_settings("default");
        let output = dir.join("collection.epub");
        EpubBuilder::new(merged, contents, settings, ImageCompressOptions::default())
            .build(&output)
            .unwrap();
        let report = validate::validate_epub(&output).unwrap();
        assert!(report.valid, "{:?}", report.issues);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn atomic_test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "piep_epub_atomic_{label}_{}_{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&path).expect("test directory");
        path
    }

    #[test]
    fn a_failed_build_never_truncates_an_existing_output() {
        let dir = atomic_test_dir("failure");
        let destination = dir.join("existing.epub");
        std::fs::write(&destination, b"known-good").unwrap();

        let result = build_validate_and_publish(
            &destination,
            |staged| {
                std::fs::write(staged, b"partial").unwrap();
                Err("injected build failure".into())
            },
            |_| true,
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"known-good");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_staged_epub_is_not_published_or_counted_as_success() {
        let dir = atomic_test_dir("invalid");
        let destination = dir.join("existing.epub");
        std::fs::write(&destination, b"known-good").unwrap();
        let published = build_validate_and_publish(
            &destination,
            |staged| std::fs::write(staged, b"invalid").map_err(|error| error.to_string()),
            |_| false,
        )
        .unwrap();
        assert!(!published);
        assert_eq!(std::fs::read(&destination).unwrap(), b"known-good");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        let result = aggregate_batch_results(vec![BatchItemResult {
            index: 1,
            download_id: 42,
            status: BatchItemStatus::Invalid,
            issues: vec![EpubValidationIssue::error("TEST", "content.opf", "invalid")],
        }]);
        assert_eq!(result.success_count, 0);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.invalid_count, 1);
        assert_eq!(result.invalid_ids, vec![42]);
        assert!(result.output_files.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn valid_staged_epub_replaces_the_destination() {
        let dir = atomic_test_dir("success");
        let destination = dir.join("existing.epub");
        std::fs::write(&destination, b"old").unwrap();
        let published = build_validate_and_publish(
            &destination,
            |staged| std::fs::write(staged, b"new").map_err(|error| error.to_string()),
            |_| true,
        )
        .unwrap();
        assert!(published);
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
