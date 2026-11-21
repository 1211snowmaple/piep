//! 切り替えた棚の作品を更新確認する。ジョブには取得元の鍵だけを残し、処理時に現在版を読む。

use super::*;
use crate::library::{EntityKind, Library, WorkCard, WorkFilter, WorkListQuery};

#[path = "update_jobs_shelf_listings.rs"]
mod listings;
pub(super) use listings::{
    author_cards, fanbox_scan_posts, listed_candidate, scan_pixiv_revisions,
};

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

pub(super) fn library(app: &tauri::AppHandle) -> Result<Arc<Library>, String> {
    app.try_state::<Arc<super::super::shelf::Shelf>>()
        .ok_or_else(|| "棚は開いていません".to_string())?
        .library()
}

pub(crate) fn saved_card(
    library: &Library,
    source: &str,
    key: &str,
) -> Result<Option<WorkCard>, String> {
    let Some(work) = library
        .work_by_key(source, key)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if library
        .library_item_generation(work)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Ok(None);
    }
    library.work_card(work).map(Some).map_err(|e| e.to_string())
}

fn cards(library: &Library, filter: WorkFilter) -> Result<Vec<WorkCard>, String> {
    let mut query = WorkListQuery {
        filter,
        limit: 500,
        count: false,
        ..Default::default()
    };
    let mut cards = Vec::new();
    loop {
        let page = library.list_works(&query).map_err(|e| e.to_string())?;
        cards.extend(page.items);
        query.after = page.next;
        if query.after.is_none() {
            return Ok(cards);
        }
    }
}

/// 棚の範囲で確認する作品の上限。画面の `SHELF_SCOPE_LIMIT` と同じ。
const SHELF_SCOPE_LIMIT: usize = 500;

pub(super) fn work_items(
    library: &Library,
    request: &StartUpdateJobRequest,
    eligible_targets: &HashSet<(String, String, String)>,
) -> Result<Vec<UpdateJobItemInput>, String> {
    if request.new_works_only.unwrap_or(false) {
        return Ok(Vec::new());
    }
    let mut selected = Vec::new();
    let mut fanbox_author_checks = HashSet::new();
    if matches!(request.scope.as_str(), "all" | "work") {
        let filter = if let Some(ids) = &request.work_ids {
            // 以前の画面の番号も移行先の対応で解く。旧 DB の作品は読まない。
            let mut works = ids
                .iter()
                .filter_map(|id| match id {
                    crate::database::WorkId::Shelf(work) => Some(*work),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            let legacy = ids
                .iter()
                .filter_map(|id| match id {
                    crate::database::WorkId::Legacy(number) => Some(*number),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            if !legacy.is_empty() {
                works.extend(
                    library
                        .legacy_work_numbers()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .filter_map(|(work, number)| legacy.contains(&number).then_some(work)),
                );
            }
            WorkFilter {
                works: Some(works.into_iter().collect()),
                ..Default::default()
            }
        } else {
            WorkFilter {
                watching: Some(true),
                ..Default::default()
            }
        };
        selected.extend(cards(library, filter)?);
    }
    // 棚の範囲（お気に入り・読みかけ）は、監視の有無に関わらず今その棚にある作品を見る。
    // 画面が知らせる上限（SHELF_SCOPE_LIMIT）と同じ件数までにする。
    let shelf = match request.scope.as_str() {
        "favorite" => Some(WorkFilter {
            favorite: Some(true),
            ..Default::default()
        }),
        "reading" => Some(WorkFilter {
            reading: Some(true),
            ..Default::default()
        }),
        _ => None,
    };
    if let Some(filter) = shelf {
        let page = library
            .list_works(&WorkListQuery {
                filter,
                limit: SHELF_SCOPE_LIMIT,
                count: false,
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        selected.extend(page.items);
    }
    if matches!(request.scope.as_str(), "all" | "author") {
        for target in request
            .adhoc_targets
            .iter()
            .flatten()
            .filter(|t| t.target_type == "author")
        {
            let matches = author_cards(library, &target.source, &target.source_key)?;
            if target.source == "fanbox" {
                fanbox_author_checks.extend(matches.iter().map(|card| card.id));
            }
            selected.extend(matches);
        }
    }
    let mut seen = HashSet::new();
    for card in selected.iter().filter(|card| card.provider == "fanbox") {
        let author = library
            .work_source_state(card.id, card.snapshot_id)
            .map_err(|e| e.to_string())?
            .author_key;
        if eligible_targets.contains(&("fanbox".into(), "author".into(), author)) {
            fanbox_author_checks.insert(card.id);
        }
    }
    selected
        .into_iter()
        .filter(|card| matches!(card.provider.as_str(), "pixiv" | "fanbox") && seen.insert(card.id))
        .map(|card| {
            item_from_payload(
                "work",
                "queued",
                Some(card.provider.clone()),
                Some(card.external_key.clone()),
                Some("work".into()),
                card.title,
                serde_json::json!({ "source": card.provider, "sourceId": card.external_key,
                    "fanboxAuthorCheck": fanbox_author_checks.contains(&card.id) }),
            )
        })
        .collect()
}

/// 本文の比較は原応答で行う。利用者が反映した題や本文を取得元の改稿として扱わない。
fn content_changed(library: &Library, card: &WorkCard, fetched: &Value) -> Result<bool, String> {
    let original = library
        .work_original(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?;
    let Some(original) = original else {
        return Ok(true);
    };
    let original: Value = serde_json::from_str(&original).map_err(|e| e.to_string())?;
    let (old, _, old_date) =
        super::super::downloader::compute_content_details(&original, &card.provider);
    let (new, _, new_date) =
        super::super::downloader::compute_content_details(fetched, &card.provider);
    Ok(old != new
        || (card.provider == "fanbox"
            && !crate::source_time::same_instant(old_date.as_deref(), new_date.as_deref())))
}

fn can_skip_pixiv_body(
    library: &Library,
    card: &WorkCard,
    metadata: &crate::downloader::pixiv::PixivNovelDetail,
) -> Result<bool, String> {
    let checked = library
        .source_entity_state(card.id)
        .map_err(|e| e.to_string())?
        .last_deep_checked_at;
    let recent = checked
        .and_then(chrono::DateTime::from_timestamp_millis)
        .is_some_and(|at| {
            let age = chrono::Utc::now().signed_duration_since(at);
            age >= chrono::Duration::zero()
                && age < chrono::Duration::days(DEEP_CHECK_INTERVAL_DAYS)
        });
    if !recent {
        return Ok(false);
    }
    let Some(original) = library
        .work_original(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    let original: Value = serde_json::from_str(&original).map_err(|e| e.to_string())?;
    if metadata
        .cover_url
        .as_deref()
        .is_some_and(crate::source_assets::is_pixiv_placeholder_image)
    {
        return Ok(false);
    }
    let tags = metadata
        .tags
        .iter()
        .map(|t| t.name.clone())
        .collect::<Vec<_>>();
    let signature = super::super::downloader::pixiv_meta_signature(
        &metadata.title,
        &metadata.caption,
        metadata.cover_url.as_deref().unwrap_or(""),
        &tags,
        metadata.series_id.as_deref().unwrap_or(""),
        u64::from(metadata.text_length),
    );
    if super::super::downloader::pixiv_meta_signature_from_json(&original).as_deref()
        != Some(&signature)
    {
        return Ok(false);
    }
    pixiv_copy_ready_for_shortcut(library, card, &original)
}

/// 一覧と作品情報の省略判定で、同じ保存状態と有効な編集を検査する。
fn pixiv_copy_ready_for_shortcut(
    library: &Library,
    card: &WorkCard,
    original: &Value,
) -> Result<bool, String> {
    let cover = original
        .get("cover_url")
        .or_else(|| original.pointer("/detail/cover_url"))
        .and_then(Value::as_str);
    if cover.is_some_and(crate::source_assets::is_pixiv_placeholder_image)
        || original
            .get("missing_illustrations")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        || !super::super::downloader::shelf::unchanged_on_shelf(
            library, card.id, original, "pixiv",
        )?
    {
        return Ok(false);
    }
    let missing = match library.pixiv_source_image_gaps(card.id, card.snapshot_id) {
        Ok(missing) => missing,
        Err(crate::library::Error::ResourceUnavailable) => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    library
        .pixiv_references_left_by_active_edit(card.id, card.snapshot_id, missing)
        .map(|missing| missing.is_empty())
        .map_err(|error| error.to_string())
}

fn record_revision(
    state: &Arc<AppState>,
    job_id: &str,
    card: &WorkCard,
    fetched: &Value,
) -> Result<bool, String> {
    let detail = fetched.get("detail").unwrap_or(fetched);
    let title = string_at(detail, &[&["title"]]).unwrap_or_else(|| card.title.clone());
    let author_key = string_at(detail, &[&["creatorId"], &["user", "id"]]).unwrap_or_default();
    let saved_at = chrono::DateTime::from_timestamp_millis(card.saved_at)
        .ok_or("保存日時が不正です")?
        .to_rfc3339();
    let target = crate::database::UpdateTarget {
        id: 0.into(),
        watch_generation: None,
        target_type: "work".into(),
        source: card.provider.clone(),
        source_key: card.external_key.clone(),
        display_name: card.author_name.clone(),
        enabled: true,
        last_checked_at: None,
        last_seen_source_id: None,
        last_seen_source_updated_at: None,
        metadata_json: None,
        created_at: saved_at.clone(),
        updated_at: saved_at.clone(),
        last_hit_at: None,
        consecutive_errors: 0,
        watch_groups: Vec::new(),
        icon_path: None,
    };
    record_candidate(
        state,
        job_id,
        &target,
        &serde_json::json!({
            "id": card.external_key, "title": title, "user": { "id": author_key, "name": card.author_name },
            "creatorId": (card.provider == "fanbox").then_some(&author_key),
            "localTitle": card.title, "localVersion": card.version_number, "localSavedAt": saved_at,
        }),
        CandidateKind::Revision,
        false,
    )
}

pub(super) async fn process_work(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    library: &Arc<Library>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
    indexes: (&mut WebUpdateIndex, &mut FanboxUpdateIndex),
) -> Result<ItemOutcome, String> {
    let (web_index, fanbox_index) = indexes;
    let payload: Value = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    // 旧形式の項目も取得元の鍵で読む。旧版の内容やパスを保存の基準にしない。
    let source = item
        .source
        .as_deref()
        .or_else(|| payload.get("source").and_then(Value::as_str))
        .ok_or("取得元がありません")?;
    let key = item
        .source_id
        .as_deref()
        .or_else(|| payload.get("sourceId").and_then(Value::as_str))
        .ok_or("作品 ID がありません")?;
    let Some(card) = saved_card(library, source, key)? else {
        return Ok(ItemOutcome::LocalSkipped(format!(
            "棚にないためスキップ: {}",
            item.title
        )));
    };
    let context = library
        .work_source_state(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?;
    let mut listed_update = None;
    let (fetched, cookie, user_agent) = match source {
        "pixiv" => {
            let Some(token) = pixiv_token(credentials) else {
                return Ok(ItemOutcome::AuthRequired(
                    "設定でpixivに接続してください".into(),
                ));
            };
            let cached = web_index.asked_authors.contains(&context.author_key);
            if !cached && web_index.api.is_some() && !context.author_key.is_empty() {
                announce_author_listing(app, state, job_id, &card.author_name).await?;
            }
            match web_index
                .lookup_key(state, &context.author_key, key, job_id)
                .await
            {
                WebLookup::Failed(error) => return Err(error),
                WebLookup::Unavailable => {
                    return Ok(ItemOutcome::Held(unavailable_saved_pixiv_work_message(
                        &card.title,
                        key,
                    )))
                }
                WebLookup::Found(entry) => listed_update = entry.update_date,
                WebLookup::Unknown => (),
            }
            if let Some(listed) = listed_update.as_deref() {
                let (library, check_card, listed) =
                    (Arc::clone(library), card.clone(), listed.to_owned());
                if blocking(move || listings::can_skip_listing(&library, &check_card, &listed))
                    .await?
                {
                    return Ok(if cached {
                        ItemOutcome::LocalSkipped(format!("最新: {}", card.title))
                    } else {
                        ItemOutcome::Skipped(format!("最新: {}", card.title))
                    });
                }
            }
            let metadata = match super::super::downloader::fetch_pixiv_novel_metadata(
                key.to_owned(),
                token.clone(),
            )
            .await
            {
                Ok(metadata) => metadata,
                Err(error) => return unavailable_saved_pixiv_work(&card.title, key, error),
            };
            let can_skip = {
                let (library, card) = (Arc::clone(library), card.clone());
                blocking(move || {
                    let skip = can_skip_pixiv_body(&library, &card, &metadata)?;
                    if skip {
                        library
                            .record_work_check(
                                card.id,
                                card.snapshot_id,
                                chrono::Utc::now().timestamp_millis(),
                                false,
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(skip)
                })
                .await?
            };
            if can_skip {
                listings::remember_verified_listing(library, &card, listed_update.as_deref())?;
                return Ok(ItemOutcome::Skipped(format!("最新: {}", card.title)));
            }
            let data =
                match super::super::downloader::fetch_pixiv_novel(key.to_owned(), token).await {
                    Ok(data) => data,
                    Err(error) => return unavailable_saved_pixiv_work(&card.title, key, error),
                };
            (
                serde_json::to_value(data).map_err(|e| e.to_string())?,
                None,
                None,
            )
        }
        "fanbox" => {
            let Some(cookie) = fanbox_cookie(credentials) else {
                return Ok(ItemOutcome::AuthRequired(
                    "設定でFANBOXに接続してください".into(),
                ));
            };
            let user_agent = fanbox_user_agent(credentials);
            if payload.get("fanboxAuthorCheck").and_then(Value::as_bool) == Some(true)
                && !context.author_key.is_empty()
            {
                if !fanbox_index.attempted_authors.contains(&context.author_key) {
                    announce_author_listing(app, state, job_id, &card.author_name).await?;
                }
                if let Some(listed) = fanbox_index
                    .lookup(&context.author_key, key, &cookie, &user_agent)
                    .await?
                {
                    if !listed.is_restricted {
                        let (library, check_card, date) = (
                            Arc::clone(library),
                            card.clone(),
                            listed.updated_datetime.clone(),
                        );
                        if blocking(move || {
                            listings::can_skip_listing(&library, &check_card, &date)
                        })
                        .await?
                        {
                            return Ok(ItemOutcome::LocalSkipped(format!("最新: {}", card.title)));
                        }
                    }
                }
            }
            let data =
                match crate::downloader::fanbox::get_post_detail(key, &cookie, &user_agent).await {
                    Ok(data) => data,
                    Err(error) => return unavailable_saved_fanbox_post(&card.title, error),
                };
            (
                serde_json::to_value(data).map_err(|e| e.to_string())?,
                Some(cookie),
                Some(user_agent),
            )
        }
        _ => {
            return Ok(ItemOutcome::LocalSkipped(format!(
                "未対応ソースをスキップ: {source}"
            )))
        }
    };
    if source == "fanbox" && fetched.get("isRestricted").and_then(Value::as_bool) == Some(true) {
        let reason = super::super::downloader::fanbox_missing_material_error(&fetched);
        hold_candidate(state, item, Some(&fetched), &reason).await?;
        return Ok(ItemOutcome::Held(reason));
    }
    if !super::super::downloader::fetched_has_material_content(&fetched, source) {
        return Err(
            "取得した作品に本文も挿絵もありません。手元の版は変更せずに保護しました".into(),
        );
    }
    // 一覧を作ったあとに保存や削除が進んでいても、処理時の現在版で判断する。
    let Some(card) = saved_card(library, source, key)? else {
        return Ok(ItemOutcome::LocalSkipped(format!(
            "棚にないためスキップ: {}",
            item.title
        )));
    };
    if state.jobs().update_job_status_value(job_id)? != "running" {
        return Err("更新確認を中断しました".into());
    }
    let changed = {
        let (library, card, fetched) = (Arc::clone(library), card.clone(), fetched.clone());
        blocking(move || {
            let changed = content_changed(&library, &card, &fetched)?;
            library
                .record_work_check(
                    card.id,
                    card.snapshot_id,
                    chrono::Utc::now().timestamp_millis(),
                    !changed,
                )
                .map_err(|e| e.to_string())?;
            Ok(changed)
        })
        .await?
    };
    if state.jobs().update_job_mode_value(job_id)? != "auto_save" && changed {
        let recorded = {
            let _guard = state.library_gate.write().await;
            record_revision(state, job_id, &card, &fetched)?
        };
        return Ok(if recorded {
            ItemOutcome::Done(format!("改稿を検出: {}", card.title))
        } else {
            ItemOutcome::Skipped(format!("確認済みの候補: {}", card.title))
        });
    }
    if !changed {
        listings::remember_verified_listing(library, &card, listed_update.as_deref())?;
    }
    let saved = super::super::downloader::download_and_save(
        app.clone(),
        fetched,
        source.to_owned(),
        key.to_owned(),
        cookie,
        user_agent,
    )
    .await?;
    let reference = saved.work;
    if !saved.missing_references.is_empty() {
        return Ok(ItemOutcome::PartialSaved(
            reference,
            format!("[一部保存] {}: 参照先の挿絵を閲覧できません", saved.title),
        ));
    }
    Ok(if saved.revised {
        ItemOutcome::Saved(
            reference,
            format!(
                "更新を保存: {}（{}, v{}）",
                saved.title,
                describe_source_change(
                    &card.title,
                    &saved.title,
                    card.text_length as i64,
                    saved.text_length
                ),
                saved.version
            ),
        )
    } else {
        ItemOutcome::Skipped(format!("最新: {}", saved.title))
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::database::AdhocUpdateTarget;
    use crate::library::{LibraryId, UserStateChange, WorkSave};
    use serde_json::json;
    use std::path::PathBuf;

    pub(in crate::commands) struct Temp(pub(super) PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(in crate::commands) fn fixture() -> (Temp, Arc<AppState>, Arc<Library>) {
        let root = std::env::temp_dir().join(format!("piep-update-shelf-{}", LibraryId::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let library = Arc::new(Library::create(&root.join("shelf")).unwrap());
        library.switch_over().unwrap();
        let state = AppState::new(Arc::new(
            super::super::super::shelf::Shelf::with_test_library(library.clone()),
        ));
        (Temp(root), Arc::new(state), library)
    }

    pub(in crate::commands) fn request(scope: &str) -> StartUpdateJobRequest {
        StartUpdateJobRequest {
            scope: scope.into(),
            mode: "check_only".into(),
            work_ids: None,
            target_ids: None,
            watch_saved: None,
            new_works_only: None,
            adhoc_targets: None,
        }
    }

    pub(in crate::commands) fn data(source: &str, text: &str, author: &str) -> Value {
        if source == "fanbox" {
            json!({ "title": "投稿", "creatorId": author, "user": { "name": "作者" }, "type": "text",
                "updatedDatetime": "2026-11-01T00:00:00Z", "body": { "text": text } })
        } else {
            json!({ "detail": { "title": "作品", "user": { "id": author, "name": "作者" },
                "caption": "説明", "tags": [], "text": text } })
        }
    }

    pub(in crate::commands) fn save(
        library: &Library,
        source: &str,
        key: &str,
        data: &Value,
        watch: bool,
    ) -> WorkCard {
        let saved = library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: source,
                key,
                route: "save",
                data,
                assets: Vec::new(),
            })
            .unwrap();
        let user = library.user_state(saved.entity_id, library.id).unwrap();
        library
            .set_user_state(
                LibraryId::now_v7(),
                saved.entity_id,
                library.id,
                user.generation,
                UserStateChange {
                    watching: Some(watch),
                    ..Default::default()
                },
            )
            .unwrap();
        library.work_card(saved.entity_id).unwrap()
    }

    #[test]
    fn favorite_and_reading_scopes_pick_the_shelf_works_whether_watched_or_not() {
        use crate::library::{ReadingAnchor, ReadingLocation, ReadingUpdate};
        let (_temp, state, library) = fixture();
        let liked = save(
            &library,
            "pixiv",
            "liked",
            &data("pixiv", "本文", "77"),
            false,
        );
        let read = save(
            &library,
            "fanbox",
            "read",
            &data("fanbox", "本文", "c"),
            false,
        );
        save(
            &library,
            "pixiv",
            "watched",
            &data("pixiv", "本文", "77"),
            true,
        );
        let user = library.user_state(liked.id, library.id).unwrap();
        library
            .set_user_state(
                LibraryId::now_v7(),
                liked.id,
                library.id,
                user.generation,
                UserStateChange {
                    favorite: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        library
            .set_reading_position(ReadingUpdate {
                request: LibraryId::now_v7(),
                work: read.id,
                user: library.id,
                device: LibraryId::now_v7(),
                expected: 0,
                location: Some(ReadingLocation::Source {
                    snapshot: read.snapshot_id,
                    anchor: ReadingAnchor::Page {
                        page: 0,
                        ratio_ppm: 500_000,
                    },
                }),
                finished: false,
            })
            .unwrap();
        let keys = |scope: &str| {
            build_initial_items(&state, &request(scope), &library)
                .unwrap()
                .into_iter()
                .filter_map(|i| i.source_id)
                .collect::<Vec<_>>()
        };
        // 監視していなくても、その範囲にある作品だけを見る。監視中の作品や作者は混ぜない。
        assert_eq!(keys("favorite"), ["liked"]);
        assert_eq!(keys("reading"), ["read"]);
        // 新作だけの確認は作品を見ない。
        let req = StartUpdateJobRequest {
            new_works_only: Some(true),
            ..request("favorite")
        };
        assert!(build_initial_items(&state, &req, &library)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn watched_works_come_from_the_shelf_and_removed_or_unwatched_works_stay_out() {
        let (_temp, state, library) = fixture();
        let first = save(&library, "pixiv", "p", &data("pixiv", "本文", "77"), true);
        save(
            &library,
            "fanbox",
            "f",
            &data("fanbox", "本文", "creator"),
            true,
        );
        save(
            &library,
            "pixiv",
            "unwatched",
            &data("pixiv", "本文", "77"),
            false,
        );
        let removed = save(
            &library,
            "pixiv",
            "removed",
            &data("pixiv", "本文", "77"),
            true,
        );
        library
            .remove_work(
                LibraryId::now_v7(),
                removed.id,
                library
                    .library_item_generation(removed.id)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        let items = build_initial_items(&state, &request("all"), &library).unwrap();
        let keys: HashSet<_> = items
            .iter()
            .filter_map(|i| i.source_id.as_deref())
            .collect();
        assert_eq!(keys, HashSet::from(["p", "f"]));
        let registered = HashSet::from([("fanbox".into(), "author".into(), "creator".into())]);
        let shared = work_items(&library, &request("all"), &registered).unwrap();
        let fanbox = shared
            .iter()
            .find(|item| item.source_id.as_deref() == Some("f"))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&fanbox.payload_json).unwrap()["fanboxAuthorCheck"],
            true
        );
        assert!(items
            .iter()
            .all(|i| serde_json::from_str::<Value>(&i.payload_json)
                .unwrap()
                .get("jsonPath")
                .is_none()));
        assert!(saved_card(&library, "pixiv", "removed").unwrap().is_none());
        assert_eq!(
            saved_card(&library, "pixiv", "p").unwrap().unwrap().id,
            first.id
        );
        assert!(build_initial_items(
            &state,
            &StartUpdateJobRequest {
                new_works_only: Some(true),
                ..request("all")
            },
            &library
        )
        .unwrap()
        .is_empty());
        // 空の明示範囲は全監視作品へ広げない。
        assert!(work_items(
            &library,
            &StartUpdateJobRequest {
                work_ids: Some(vec![]),
                ..request("work")
            },
            &HashSet::new(),
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn explicit_work_uuids_select_only_saved_members_and_never_expand_an_empty_range() {
        let (_temp, state, library) = fixture();
        let wanted = save(
            &library,
            "pixiv",
            "wanted",
            &data("pixiv", "本文", "77"),
            false,
        );
        let other = save(
            &library,
            "pixiv",
            "other",
            &data("pixiv", "本文", "88"),
            true,
        );
        let person = library
            .entity_by_key(EntityKind::Person, "pixiv", "77")
            .unwrap()
            .unwrap();
        let request: StartUpdateJobRequest = serde_json::from_value(serde_json::json!({
            "scope":"work","mode":"check_only","workIds":[wanted.id,wanted.id,person,LibraryId::now_v7()]
        })).unwrap();
        let items = build_initial_items(&state, &request, &library).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_id.as_deref(), Some("wanted"));
        for id in [person, LibraryId::now_v7(), other.id] {
            if id == other.id {
                library
                    .remove_work(
                        LibraryId::now_v7(),
                        id,
                        library.library_item_generation(id).unwrap().unwrap(),
                    )
                    .unwrap();
            }
            let request = StartUpdateJobRequest {
                work_ids: Some(vec![crate::database::WorkId::Shelf(id)]),
                ..request.clone()
            };
            assert!(build_initial_items(&state, &request, &library)
                .unwrap()
                .is_empty());
        }
        assert!(serde_json::from_value::<StartUpdateJobRequest>(
            serde_json::json!({"scope":"work","mode":"check_only","workIds":["invalid-id"]})
        )
        .is_err());
    }

    #[test]
    fn one_off_author_scope_uses_shelf_membership_without_requiring_work_watches() {
        let (_temp, state, library) = fixture();
        save(&library, "pixiv", "a", &data("pixiv", "本文", "77"), false);
        save(&library, "pixiv", "b", &data("pixiv", "本文", "88"), true);
        let req = StartUpdateJobRequest {
            adhoc_targets: Some(vec![AdhocUpdateTarget {
                target_type: "author".into(),
                source: "pixiv".into(),
                source_key: "77".into(),
                display_name: "作者".into(),
            }]),
            ..request("author")
        };
        let items = build_initial_items(&state, &req, &library).unwrap();
        let work_keys: Vec<_> = items
            .iter()
            .filter(|i| i.item_type == "work")
            .map(|i| i.source_id.as_deref())
            .collect();
        assert_eq!(work_keys, [Some("a")]);
    }

    #[test]
    fn checks_compare_the_current_source_and_record_candidates_without_legacy_work_rows() {
        let (_temp, state, library) = fixture();
        for source in ["pixiv", "fanbox"] {
            let original = data(source, "本文", "77");
            let mut card = save(&library, source, "one", &original, true);
            // カードの題は利用者の編集で変わりうる。比較するのは今の版の原応答。
            card.title = "利用者の題".into();
            assert!(!content_changed(&library, &card, &original).unwrap());
            let changed = data(source, "改稿", "77");
            assert!(content_changed(&library, &card, &changed).unwrap());
            let job = make_job_id();
            state
                .jobs()
                .create_update_job(&job, &request("work"), &[])
                .unwrap();
            assert!(record_revision(&state, &job, &card, &changed).unwrap());
            let snapshot = state.jobs().update_job_snapshot(&job).unwrap();
            assert_eq!(snapshot.candidate_count, 1);
            let candidates = state
                .candidates()
                .list_update_candidates_by_status("pending", -1)
                .unwrap();
            let candidate = candidates.iter().find(|c| c.source == source).unwrap();
            let payload: Value = serde_json::from_str(&candidate.payload_json).unwrap();
            assert_eq!(payload["sourceId"], "one");
            assert_eq!(payload["originalData"]["localVersion"], 1);
            let newer = save(&library, source, "one", &changed, true);
            assert!(!content_changed(&library, &newer, &changed).unwrap());
            assert_eq!(newer.version_number, 2);
        }
    }

    #[test]
    fn check_times_never_verify_a_different_or_superseded_snapshot_or_move_backwards() {
        let (_temp, _state, library) = fixture();
        let first = save(&library, "pixiv", "one", &data("pixiv", "本文", "77"), true);
        library
            .record_work_check(first.id, first.snapshot_id, 100, true)
            .unwrap();
        library
            .record_work_check(first.id, first.snapshot_id, 50, true)
            .unwrap();
        library
            .record_work_check(first.id, first.snapshot_id, 200, false)
            .unwrap();
        let check = library.source_entity_state(first.id).unwrap();
        assert_eq!(check.last_checked_at, Some(200));
        assert_eq!(check.last_deep_checked_at, Some(100));
        let other = save(
            &library,
            "pixiv",
            "other",
            &data("pixiv", "本文", "77"),
            true,
        );
        assert!(library
            .record_work_check(first.id, other.snapshot_id, 300, true)
            .is_err());
        let newer = save(&library, "pixiv", "one", &data("pixiv", "改稿", "77"), true);
        assert!(matches!(
            library.record_work_check(first.id, first.snapshot_id, 300, true),
            Err(crate::library::Error::Superseded)
        ));
        assert_eq!(library.source_entity_state(first.id).unwrap(), check);
        assert!(library
            .record_work_check(first.id, newer.snapshot_id, i64::MAX, true)
            .is_err());
        library
            .remove_work(
                LibraryId::now_v7(),
                first.id,
                library.library_item_generation(first.id).unwrap().unwrap(),
            )
            .unwrap();
        assert!(library
            .record_work_check(first.id, newer.snapshot_id, 300, true)
            .is_err());
        library.verify().unwrap();
    }

    #[test]
    fn update_shortcuts_use_the_active_edit_and_keep_source_reported_gaps_visible() {
        use crate::library::{EditBlock, EditContent, EditPointerChange, EditRequest, WorkSave};
        let (_temp, _state, library) = fixture();
        let mut original = data("pixiv", "本文[uploadedimage:1]", "77");
        original["detail"]["text_length"] = json!(2);
        let saved = library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "pixiv",
                    key: "one",
                    route: "save",
                    data: &original,
                    assets: vec![],
                },
                None,
            )
            .unwrap();
        let card = library.work_card(saved.entity_id).unwrap();
        let listed_at = card.saved_at - 1000;
        let listed = chrono::DateTime::from_timestamp_millis(listed_at)
            .unwrap()
            .to_rfc3339();
        library
            .remember_work_source_update(card.id, card.snapshot_id, listed_at)
            .unwrap();
        library
            .record_work_check(card.id, card.snapshot_id, card.saved_at, true)
            .unwrap();
        let metadata = serde_json::from_value(json!({
            "id": 1, "title": "作品", "caption": "説明", "tags": [], "text_length": 2,
            "create_date": "2026-11-01T00:00:00Z", "user": { "id": 77, "name": "作者", "account": "a" }
        })).unwrap();
        assert!(!can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        assert!(!listings::can_skip_listing(&library, &card, &listed).unwrap());
        let content = EditContent {
            format: "piep-edit-v1".into(),
            title: None,
            media: vec![],
            provenance: None,
            blocks: vec![EditBlock {
                id: LibraryId::now_v7(),
                ordinal: 0,
                kind: "paragraph".into(),
                text: Some("本文".into()),
                attrs: Value::Null,
                media_slot: None,
            }],
        };
        library
            .save_edit(EditRequest {
                request_id: LibraryId::now_v7(),
                work: card.id,
                user: library.id,
                base: card.snapshot_id,
                parent: None,
                expected: 0,
                content: &content,
                activate: true,
            })
            .unwrap();
        assert!(can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        assert!(listings::can_skip_listing(&library, &card, &listed).unwrap());
        original["missing_illustrations"] = json!(["[uploadedimage:1]"]);
        assert!(!pixiv_copy_ready_for_shortcut(&library, &card, &original).unwrap());
        let edit = library.edit_state(card.id, library.id).unwrap();
        library
            .change_edit_pointers(
                LibraryId::now_v7(),
                card.id,
                library.id,
                edit.generation,
                EditPointerChange::Unpublish,
            )
            .unwrap();
        assert!(!can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        assert!(!listings::can_skip_listing(&library, &card, &listed).unwrap());
    }

    #[test]
    fn metadata_skips_only_a_recent_complete_copy_and_missing_objects_force_a_save() {
        let (temp, _state, library) = fixture();
        let mut original = data("pixiv", "本文", "77");
        original["detail"]["text_length"] = json!(2);
        let card = save(&library, "pixiv", "one", &original, true);
        let metadata = serde_json::from_value(json!({
            "id": 1, "title": "作品", "caption": "説明", "tags": [], "text_length": 2,
            "create_date": "2026-11-01T00:00:00Z", "user": { "id": 77, "name": "作者", "account": "a" }
        })).unwrap();
        assert!(!can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        let old = (chrono::Utc::now() - chrono::Duration::days(DEEP_CHECK_INTERVAL_DAYS + 1))
            .timestamp_millis();
        library
            .record_work_check(card.id, card.snapshot_id, old, true)
            .unwrap();
        assert!(!can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        library
            .record_work_check(
                card.id,
                card.snapshot_id,
                chrono::Utc::now().timestamp_millis(),
                true,
            )
            .unwrap();
        assert!(can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        let mut changed = metadata.clone();
        changed.caption = "変更".into();
        assert!(!can_skip_pixiv_body(&library, &card, &changed).unwrap());

        let body = library
            .resource_manifest(
                card.id,
                crate::library::ResourceScope::Snapshot(card.snapshot_id),
            )
            .unwrap()
            .resources
            .into_iter()
            .find(|r| r.reference == crate::library::ResourceReference::Body)
            .unwrap()
            .file
            .unwrap();
        let chunk = library.file(body.id).unwrap().chunks[0].id;
        let object = library
            .objects
            .listing()
            .unwrap()
            .into_iter()
            .find(|object| object.entry == piep_storage::StoredEntry::Object(chunk))
            .unwrap();
        assert!(object
            .path
            .starts_with(std::fs::canonicalize(&temp.0).unwrap()));
        std::fs::remove_file(object.path).unwrap();
        assert!(!can_skip_pixiv_body(&library, &card, &metadata).unwrap());
        assert!(!super::super::super::downloader::shelf::unchanged_on_shelf(
            &library, card.id, &original, "pixiv"
        )
        .unwrap());
        // 保存し直すと失った実体を戻す。作者の改稿や保存版は増えない。
        let restored = save(&library, "pixiv", "one", &original, true);
        assert_eq!(restored.snapshot_id, card.snapshot_id);
        assert_eq!(restored.version_number, 1);
        library.verify().unwrap();
    }

    #[test]
    fn registered_watch_scope_and_series_deduplication_use_only_the_authoritative_catalogue() {
        let (_temp, state, library) = fixture();
        let mut response = data("pixiv", "本文", "77");
        response["detail"]["series_id"] = json!(7);
        save(&library, "pixiv", "11", &response, false);
        let create = |kind: &str, key: &str| {
            library
                .edit_watch(
                    LibraryId::now_v7(),
                    library.watch_generation().unwrap(),
                    crate::library::WatchEdit::Upsert {
                        target_type: kind.into(),
                        provider: "pixiv".into(),
                        key: key.into(),
                        name: key.into(),
                        enabled: true,
                        options: json!({}),
                    },
                )
                .unwrap()
                .id
        };
        let author = create("author", "77");
        let series = create("series", "7");
        let req = StartUpdateJobRequest {
            target_ids: Some(vec![
                crate::database::WatchId::Shelf(author),
                crate::database::WatchId::Shelf(series),
            ]),
            ..request("targets")
        };
        let items = build_initial_items(&state, &req, &library).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_id.as_deref(), Some("77"));
        let payload: Value = serde_json::from_str(&items[0].payload_json).unwrap();
        assert_eq!(payload["id"], author.to_string());
        let req = StartUpdateJobRequest {
            target_ids: Some(vec![crate::database::WatchId::Shelf(series)]),
            ..request("targets")
        };
        let items = build_initial_items(&state, &req, &library).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_id.as_deref(), Some("7"));
    }
}
