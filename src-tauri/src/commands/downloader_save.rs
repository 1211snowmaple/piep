//! 棚への保存。取り寄せた応答を、取得元の形のまま棚へ入れる。
//!
//! 版を増やすかどうかは棚が決める（応答から作った項目・本文・画像と添付を今の版と比べる）。
//! ここで決めるのは、画像と添付を落とし直さなくてよいか（応答の中身が今の版の原本と同じで、
//! 取得元の画像・添付が今の版に全部ある）だけ。同じ作品でも、未作成のプロフィールの控えは補う。

use super::{
    collect_assets_recursive, compute_content_details, fanbox_missing_material_error,
    fetched_has_material_content, work_save_mutex,
};
use crate::commands::shelf::Shelf;
use crate::library::{
    asset_reference, EntityKind, Library, LibraryId, ResourceReference, ResourceScope, StagedAsset,
    WorkSave,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[path = "downloader_assets.rs"]
mod repairs;

/// 棚へ保存した結果。
#[derive(Clone, Debug)]
pub struct ShelfSaved {
    pub work: LibraryId,
    pub title: String,
    pub text_length: i64,
    /// 版の番号（1 から）。
    pub version: i64,
    /// この保存で版が増えたか。
    pub revised: bool,
    /// 初めて棚へ入ったか。
    pub created: bool,
    /// pixiv の本文が指す挿絵のうち、手元に無いもの（閲覧できない・取得できなかった）。
    pub missing_references: Vec<String>,
}

enum ShelfSaveError {
    Superseded,
    Failed(String),
}

impl From<String> for ShelfSaveError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<crate::library::Error> for ShelfSaveError {
    fn from(error: crate::library::Error) -> Self {
        match error {
            crate::library::Error::Superseded => Self::Superseded,
            error => Self::Failed(error.to_string()),
        }
    }
}

pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// 取得元の画像・添付の種類（保存フォルダーと同じ名前）。
fn target_role(sub_folder: &str) -> &'static str {
    match sub_folder {
        "illustrations" => "illustration",
        "cover" => "cover",
        _ => "file",
    }
}

fn slot_of(reference: &ResourceReference) -> Option<&str> {
    match reference {
        ResourceReference::Media { slot } | ResourceReference::File { slot } => Some(slot),
        _ => None,
    }
}

/// 本文が指す挿絵のうち、手元にないもの。取得元の「閲覧できません」の画像は数えない。
fn missing_references<'a>(
    data: &Value,
    source: &str,
    present: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Vec<String> {
    if source != "pixiv" {
        return Vec::new();
    }
    let names = present
        .into_iter()
        .filter(|(_, url)| !url.is_some_and(crate::source_assets::is_pixiv_placeholder_image))
        .map(|(name, _)| name.to_owned());
    crate::analysis::pixiv_missing_references_for_filenames(data, names)
}

async fn references_left_by_edit(
    library: Arc<Library>,
    work: LibraryId,
    snapshot: LibraryId,
    missing: Vec<String>,
) -> Vec<String> {
    if missing.is_empty() {
        return missing;
    }
    let original = missing.clone();
    match blocking(move || {
        library
            .pixiv_references_left_by_active_edit(work, snapshot, missing)
            .map_err(|e| e.to_string())
    })
    .await
    {
        Ok(missing) => missing,
        Err(error) => {
            // 原本の保存は済んでいる。編集を確認できなければ不足を残し、完了へ読み替えない。
            log::warn!("編集で挿絵の欠けが埋まったか確認できません: {error}");
            original
        }
    }
}

/// 応答の中身が今の版の原本と同じで、取得元の画像・添付が今の版に全部あるか。棚から外した作品は
/// 保存し直して戻すので、同じとは見なさない。
pub(in crate::commands) fn unchanged_on_shelf(
    library: &Library,
    work: LibraryId,
    data: &Value,
    source: &str,
) -> Result<bool, String> {
    skip_save_on_shelf(library, work, data, source, false)
}

fn skip_save_on_shelf(
    library: &Library,
    work: LibraryId,
    data: &Value,
    source: &str,
    respect_repair_limit: bool,
) -> Result<bool, String> {
    let fail = |e: crate::library::Error| e.to_string();
    if library
        .library_item_generation(work)
        .map_err(fail)?
        .is_none()
    {
        return Ok(false);
    }
    let snapshot = library.current_snapshot(work).map_err(fail)?;
    let Some(previous) = library.work_original(work, snapshot).map_err(fail)? else {
        return Ok(false);
    };
    let Ok(previous) = serde_json::from_str::<Value>(&previous) else {
        return Ok(false);
    };
    let (old_hash, _, old_updated) = compute_content_details(&previous, source);
    let (new_hash, _, new_updated) = compute_content_details(data, source);
    if (source == "fanbox" && !repairs::same_source(&previous, data)?)
        || (source == "pixiv"
            && crate::library::masked_pixiv_metadata(&previous)
                != crate::library::masked_pixiv_metadata(data))
        || old_hash != new_hash
        || (source == "fanbox"
            && !crate::source_time::same_instant(old_updated.as_deref(), new_updated.as_deref()))
    {
        return Ok(false);
    }
    let resources = library
        .resource_manifest(work, ResourceScope::Snapshot(snapshot))
        .map_err(fail)?
        .resources;
    // 台帳の参照だけで「ある」としない。実体を失った棚は、同じ応答でも保存し直す。
    for info in &resources {
        if matches!(info.reference, ResourceReference::Original { .. }) {
            continue;
        }
        if let Some(file) = &info.file {
            for chunk in library.file(file.id).map_err(fail)?.chunks {
                if !library
                    .objects
                    .has_object(chunk.id)
                    .map_err(|e| e.to_string())?
                {
                    return Ok(false);
                }
            }
        }
    }
    let held: HashSet<String> = resources
        .iter()
        .filter(|info| info.file.is_some())
        .filter_map(|info| slot_of(&info.reference).map(str::to_owned))
        .collect();
    let mut targets = Vec::new();
    crate::downloader::asset_downloader::extract_download_targets(
        data,
        source == "fanbox",
        &mut targets,
    );
    // 以前の保存名（FANBOX の添付に取得元の ID を付ける前）で持っている版も、同じ添付を持つ。
    let former = if source == "fanbox" {
        crate::downloader::asset_downloader::fanbox_former_file_names(data)
    } else {
        HashMap::new()
    };
    let mut complete = true;
    for target in &targets {
        let role = target_role(target.sub_folder);
        let mut names = vec![target.filename.as_str()];
        if role == "file" {
            names.extend(former.get(&target.filename).map(String::as_str));
        }
        let mut held_here = false;
        for name in names {
            let reference = asset_reference(role, name).map_err(fail)?;
            held_here |= slot_of(&reference).is_some_and(|slot| held.contains(slot));
        }
        if !held_here {
            complete = false;
        }
    }
    if complete {
        return Ok(true);
    }
    if source == "fanbox" && respect_repair_limit {
        let urls = targets.iter().map(|t| t.url.clone()).collect::<Vec<_>>();
        let exhausted = library
            .asset_repair_plan(work, snapshot, &urls)
            .map_err(fail)?
            .attempts
            >= crate::library::asset_repairs::MAX_REPAIR_ATTEMPTS;
        if exhausted {
            for info in &resources {
                if let Some(file) = &info.file {
                    library
                        .objects
                        .verify(&library.file(file.id).map_err(fail)?)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        return Ok(exhausted);
    }
    Ok(false)
}

/// 落とした画像・添付を、保存の入力へ。取得元の URL は応答の指定から引く。
fn staged_from(dir: &Path, data: &Value, source: &str) -> Result<Vec<StagedAsset>, String> {
    let mut targets = Vec::new();
    crate::downloader::asset_downloader::extract_download_targets(
        data,
        source == "fanbox",
        &mut targets,
    );
    let origins: HashMap<(&str, String), String> = targets
        .into_iter()
        .map(|t| ((target_role(t.sub_folder), t.filename), t.url))
        .collect();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let (mut entries, mut size, mut cover) = (Vec::new(), 0i64, None);
    collect_assets_recursive(dir, &mut entries, &mut size, &mut cover, 0)?;
    // 棚の枠の順は名前で決まる。読み取りの順（ファイルシステム任せ）に左右されないよう並べる。
    entries.sort_by(|a, b| (&a.asset_type, &a.filename).cmp(&(&b.asset_type, &b.filename)));
    Ok(entries
        .into_iter()
        .map(|asset| StagedAsset {
            url: origins
                .get(&(asset.asset_type.as_str(), asset.filename.clone()))
                .cloned(),
            role: asset.asset_type,
            name: asset.filename,
            path: PathBuf::from(asset.local_path),
            mime_type: asset.mime_type,
        })
        .collect())
}

/// 保存フォルダーの一つの版（応答の JSON と `data_assets/`）を、取り寄せずに棚へ保存する。
/// 保存フォルダーの再取り込みに使う。画像・添付はフォルダーにあるものだけを使う。
pub(crate) async fn save_folder_to_shelf(
    library: Arc<Library>,
    data: Value,
    source: String,
    key: String,
    assets: PathBuf,
) -> Result<LibraryId, String> {
    let work_lock = work_save_mutex(&source, &key);
    let _work_guard = work_lock.lock().await;
    if !fetched_has_material_content(&data, &source) {
        return Err("本文も挿絵もありません".into());
    }
    blocking(move || {
        let request = LibraryId::now_v7();
        library
            .begin_acquisition(request, &source, EntityKind::Work, &key, "reimport")
            .map_err(|e| e.to_string())?;
        let saved = (|| {
            let input = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
            library
                .record_acquisition_payload(
                    request,
                    "work-save-input",
                    &crate::library::CaptureSource::Bytes(input),
                )
                .map_err(|e| e.to_string())?;
            let assets = staged_from(&assets, &data, &source)?;
            let observation = (source == "fanbox").then(|| repairs::observation(&data, &assets));
            let save = WorkSave {
                request,
                provider: &source,
                key: &key,
                route: "reimport",
                data: &data,
                assets,
            };
            library
                .save_readable_work(save, observation)
                .map_err(|e| e.to_string())
        })();
        match saved {
            Ok(saved) => Ok(saved.entity_id),
            Err(error) => {
                // 棚へ渡す前に止まった取得を閉じる（渡した後なら棚が閉じていて、これは何もしない）。
                let _ = library.cancel_acquisition(request);
                Err(error)
            }
        }
    })
    .await
}

/// 取り寄せた応答を棚へ保存する。
pub(super) async fn save_to_shelf(
    shelf: Arc<Shelf>,
    library: Arc<Library>,
    data: Value,
    source: String,
    source_id: String,
    cookie: Option<String>,
    user_agent: Option<String>,
) -> Result<ShelfSaved, String> {
    save_to_shelf_as(
        shelf, library, data, source, source_id, cookie, user_agent, None,
    )
    .await
    .map_err(|error| match error {
        ShelfSaveError::Superseded => crate::library::Error::Superseded.to_string(),
        ShelfSaveError::Failed(error) => error,
    })
}

/// 保存済みの応答（pixiv・FANBOX）に原寸の在り処を足したものを、保存し直す（縮小版の画像の取り直し）。
///
/// 手元の版と同じ応答でも保存する（表紙の枠は名前によらず一つなので、普段の保存は「持っている」と
/// みて何もしない）。URL が変わらない画像・添付は棚から戻し、原寸に変わった画像だけを取りに行く。
/// 棚は画質の違いを版の違いとして扱わないので、版は増えない。
/// 応答を読んだときの保存版が現在版でなくなった場合は `None` を返す。
pub(crate) async fn save_originals_to_shelf(
    shelf: Arc<Shelf>,
    library: Arc<Library>,
    data: Value,
    source: String,
    source_id: String,
    base_snapshot: LibraryId,
) -> Result<Option<ShelfSaved>, String> {
    match save_to_shelf_as(
        shelf,
        library,
        data,
        source,
        source_id,
        None,
        None,
        Some(base_snapshot),
    )
    .await
    {
        Ok(saved) => Ok(Some(saved)),
        Err(ShelfSaveError::Superseded) => Ok(None),
        Err(ShelfSaveError::Failed(error)) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_to_shelf_as(
    shelf: Arc<Shelf>,
    library: Arc<Library>,
    data: Value,
    source: String,
    source_id: String,
    cookie: Option<String>,
    user_agent: Option<String>,
    base_snapshot: Option<LibraryId>,
) -> Result<ShelfSaved, ShelfSaveError> {
    let mut data = data;
    let upgrade_images = base_snapshot.is_some();
    let work_lock = work_save_mutex(&source, &source_id);
    let _work_guard = work_lock.lock().await;
    // 原寸の取り直しが渡すのは、棚がもう持っている応答そのもの（原寸の在り処を足しただけ）。
    // 中身の無い取得で手元の版を上書きしないための守りは、ここでは要らない。支援していない
    // 投稿を表紙と説明だけで持っているときも、その表紙を原寸に替えられるようにする。
    if !upgrade_images
        && source == "fanbox"
        && data.get("isRestricted").and_then(Value::as_bool) == Some(true)
    {
        return Err(fanbox_missing_material_error(&data).into());
    }
    // 一覧や関係で識別だけ先にできた作品（版が無い）は、初めての保存として扱う。
    let existing = {
        let (library, source, key) = (Arc::clone(&library), source.clone(), source_id.clone());
        blocking(move || {
            library
                .saved_work_by_key(&source, &key)
                .map_err(|e| e.to_string())
        })
        .await?
    };
    // 本文の無い作品（支援していない投稿を表紙と説明だけで持っているもの）の表紙を取り直すときは、
    // 読む形を作らずに保存する。読む本文が無いので、読む形を作る保存は受け付けない。
    let bodyless_upgrade = upgrade_images && !fetched_has_material_content(&data, &source);
    // 空の応答は保存しない（初めてなら棚に入れない、あれば手元の版を守る）。
    if !upgrade_images && !fetched_has_material_content(&data, &source) {
        let error = match (existing.is_some(), source.as_str()) {
            (_, "fanbox") => fanbox_missing_material_error(&data),
            (false, _) => {
                "この作品の本文を取得できませんでした。閲覧制限がかかっている可能性があります"
                    .to_string()
            }
            (true, _) => {
                "取得した作品に本文も挿絵もありません。手元の版は変更せずに保護しました".to_string()
            }
        };
        return Err(error.into());
    }
    let card = |library: Arc<Library>, work: LibraryId| async move {
        blocking(move || library.work_card(work).map_err(|e| e.to_string())).await
    };
    let mut reader_only = false;
    if let Some(work) = existing.filter(|_| !upgrade_images) {
        let plan = {
            let (library, data, source) = (Arc::clone(&library), data.clone(), source.clone());
            blocking(move || repairs::save_plan(&library, work, &data, &source)).await?
        };
        let unchanged = match plan {
            repairs::SavePlan::Unchanged => true,
            repairs::SavePlan::Save => false,
            repairs::SavePlan::ReaderOnly(previous) => {
                data = previous;
                reader_only = true;
                false
            }
        };
        if unchanged {
            if super::shelf_profiles::save(Arc::clone(&library), &source, &data).await {
                shelf.forget_derived();
            }
            let card = card(Arc::clone(&library), work).await?;
            let mut targets = Vec::new();
            crate::downloader::asset_downloader::extract_download_targets(
                &data,
                source == "fanbox",
                &mut targets,
            );
            let missing = missing_references(
                &data,
                &source,
                targets
                    .iter()
                    .filter(|t| t.sub_folder == "illustrations")
                    .map(|t| (t.filename.as_str(), Some(t.url.as_str()))),
            );
            let missing = if source == "pixiv" {
                references_left_by_edit(Arc::clone(&library), work, card.snapshot_id, missing).await
            } else {
                missing
            };
            return Ok(ShelfSaved {
                work,
                title: card.title,
                text_length: card.text_length as i64,
                version: card.version_number as i64,
                revised: false,
                created: false,
                missing_references: missing,
            });
        }
    }

    // 原寸は、実際に保存すると決まったここで探す。
    if matches!(source.as_str(), "pixiv" | "fanbox") && !reader_only && !upgrade_images {
        use crate::downloader::pixiv_originals as originals;
        // 棚がもう原寸を持っている表紙（同じシリーズの表紙）は、画像サーバーに確かめない。
        let filled = {
            let (library, filled, provider) = (Arc::clone(&library), data.clone(), source.clone());
            blocking(move || {
                let mut filled = filled;
                originals::fill_cover_original_from_shelf(&library, &mut filled, &provider)
                    .map(|_| filled)
            })
            .await
        };
        match filled {
            Ok(filled) => data = filled,
            Err(error) => log::warn!("棚の原寸の表紙を確かめられません: {error}"),
        }
        match originals::cdn_client() {
            Ok(client) if source == "pixiv" => {
                originals::add_original_urls(&client, &mut data).await
            }
            Ok(client) => originals::add_fanbox_original_cover(&client, &mut data).await,
            Err(error) => log::warn!("原寸の画像を探さずに保存します: {error}"),
        }
    }

    // 画像を落とす前に取得を始めた記録を作る（同じ作品の後から始まった取得に負けない）。
    let repair = if source == "fanbox" {
        let (library, data) = (Arc::clone(&library), data.clone());
        blocking(move || repairs::can_record_attempt(&library, existing, &data)).await?
    } else {
        false
    };
    let request = LibraryId::now_v7();
    let route = if reader_only {
        "prepare-reader"
    } else if upgrade_images {
        "original-image"
    } else {
        "save"
    };
    {
        let (library, source, key) = (Arc::clone(&library), source.clone(), source_id.clone());
        let input = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(snapshot) = base_snapshot {
                library.begin_work_acquisition_from_snapshot(
                    request, &source, &key, route, snapshot,
                )?;
            } else {
                library.begin_acquisition(request, &source, EntityKind::Work, &key, route)?;
            }
            if let Err(error) = library.record_acquisition_payload(
                request,
                "work-save-input",
                &crate::library::CaptureSource::Bytes(input),
            ) {
                let _ = library.cancel_acquisition(request);
                return Err(error);
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())??;
    }
    let staging = std::env::temp_dir().join(format!("piep-save-{}", request.simple()));
    let saved = async {
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| e.to_string())?;
        let aliases = if matches!(source.as_str(), "fanbox" | "pixiv") || reader_only {
            let (library, data, dir) = (
                Arc::clone(&library),
                data.clone(),
                staging.join("data_assets"),
            );
            let provider = source.clone();
            blocking(move || {
                let aliases = repairs::restore_held(&library, existing, &data, &dir, &provider)?;
                // ほかの作品が持つ同じ原寸の表紙も使い、取りに行かない（シリーズの表紙）。
                repairs::restore_shared_originals(&library, &data, &dir, &provider)?;
                Ok::<_, String>(aliases)
            })
            .await?
        } else {
            HashMap::new()
        };
        let mut linked = data.clone();
        let download = if reader_only {
            Ok(())
        } else {
            crate::downloader::asset_downloader::download_and_link_assets(
                &mut linked,
                &staging.join("data.json"),
                source == "fanbox",
                cookie,
                user_agent,
            )
            .await
        };
        let asset_failure = repairs::download_result(download, repair)?;
        let mut assets = {
            let (dir, data, source) = (staging.join("data_assets"), data.clone(), source.clone());
            blocking(move || staged_from(&dir, &data, &source)).await?
        };
        let observation =
            (source == "fanbox" && !reader_only).then(|| repairs::observation(&data, &assets));
        for asset in &mut assets {
            if let Some(held) = aliases.get(&(asset.role.clone(), asset.name.clone())) {
                asset.name.clone_from(&held.name);
                asset.mime_type.clone_from(&held.mime_type);
                if let Some(url) = &held.url_override {
                    asset.url = Some(url.clone());
                }
            }
        }
        if reader_only {
            let (library, held) = (Arc::clone(&library), assets.clone());
            let work = existing.ok_or_else(|| "読書の対応を補う対象がありません".to_string())?;
            blocking(move || repairs::require_all_held(&library, work, &held)).await?;
        }
        let present: Vec<(String, Option<String>)> = assets
            .iter()
            .filter(|a| a.role == "illustration")
            .map(|a| (a.name.clone(), a.url.clone()))
            .collect();
        let missing = missing_references(
            &data,
            &source,
            present.iter().map(|(n, u)| (n.as_str(), u.as_deref())),
        );
        let (library, data, source, key) = (
            Arc::clone(&library),
            data.clone(),
            source.clone(),
            source_id.clone(),
        );
        let result = tauri::async_runtime::spawn_blocking(move || {
            let save = WorkSave {
                request,
                provider: &source,
                key: &key,
                route,
                data: &data,
                assets,
            };
            if bodyless_upgrade {
                library.save_work(save)
            } else {
                library.save_readable_work(save, observation)
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok::<_, ShelfSaveError>((result, missing, asset_failure))
    }
    .await;
    if let Err(error) = tokio::fs::remove_dir_all(&staging).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "保存の作業フォルダーを消せません: {}: {error}",
                staging.display()
            );
        }
    }
    let (result, mut missing, asset_failure) = match saved {
        Ok(saved) => saved,
        Err(error) => {
            // 画像を落とせなかったなど、棚へ渡す前に止まった取得を閉じる（渡した後は棚が閉じている）。
            let library = Arc::clone(&library);
            let _ = blocking(move || {
                library
                    .cancel_acquisition(request)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .await;
            return Err(error);
        }
    };
    super::shelf_profiles::save(Arc::clone(&library), &source, &data).await;
    shelf.forget_derived();
    {
        let library = Arc::clone(&library);
        let work = result.entity_id;
        blocking(move || {
            if let Err(error) = library.sync_search_index(&mut |_, _| {}) {
                log::error!("保存した作品を検索の索引へ入れられません: {error}");
            }
            // まとまりの提案が使う本文リンクを、保存した版から取り直す。
            if let Err(error) = library.refresh_shelf_work_links(work) {
                log::warn!("保存した作品の本文リンクを取れません: {error}");
            }
            crate::commands::library_health::schedule_semantic_sync(library);
            Ok(())
        })
        .await?;
    }
    if let Some(error) = asset_failure {
        return Err(error.into());
    }
    if source == "pixiv" {
        missing = references_left_by_edit(
            Arc::clone(&library),
            result.entity_id,
            result.snapshot_id,
            missing,
        )
        .await;
    }
    let card = card(Arc::clone(&library), result.entity_id).await?;
    Ok(ShelfSaved {
        work: result.entity_id,
        title: card.title,
        text_length: card.text_length as i64,
        version: card.version_number as i64,
        revised: result.created_revision && existing.is_some(),
        created: existing.is_none(),
        missing_references: missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_body_points_at_and_could_not_be_seen_is_missing() {
        let data = serde_json::json!({ "text": "a[uploadedimage:1]b[uploadedimage:2]" });
        let held = [
            (
                "inline_1.png",
                Some("https://i.pximg.net/img-original/1.png"),
            ),
            (
                "inline_2.png",
                Some("https://s.pximg.net/common/images/limit_mypixiv_s.png"),
            ),
        ];
        let missing = missing_references(&data, "pixiv", held);
        assert_eq!(missing.len(), 1);
        assert!(missing_references(&data, "fanbox", []).is_empty());
    }
}
