//! 作者の一覧を、切り替えた棚の現在版へ照合する。原応答を全作品ぶん開かない。

use super::*;

pub(in crate::commands::update_jobs) fn author_cards(
    library: &Library,
    source: &str,
    author: &str,
) -> Result<Vec<WorkCard>, String> {
    let Some(person) = library
        .entity_by_key(EntityKind::Person, source, author)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    cards(
        library,
        WorkFilter {
            person: Some(person),
            provider: Some(source.into()),
            ..Default::default()
        },
    )
}

fn timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.timestamp_millis())
}

pub(super) fn remember_verified_listing(
    library: &Library,
    card: &WorkCard,
    listed: Option<&str>,
) -> Result<(), String> {
    let Some(listed) = listed.and_then(timestamp) else {
        return Ok(());
    };
    let context = library
        .work_source_state(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?;
    if listed
        <= context
            .last_deep_checked_at
            .unwrap_or(card.saved_at)
            .max(card.saved_at)
    {
        library
            .remember_work_source_update(card.id, card.snapshot_id, listed)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 知っている基準と一致するか、手元の本文の照合より前の更新だと言える場合だけ候補にしない。
pub(super) fn needs_revision(
    library: &Library,
    card: &WorkCard,
    listed: &str,
) -> Result<bool, String> {
    let Some(listed_at) = timestamp(listed) else {
        return Ok(true);
    };
    let context = library
        .work_source_state(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?;
    if context.source_updated_at == Some(listed_at) {
        return Ok(false);
    }
    let verified = context
        .last_deep_checked_at
        .unwrap_or(card.saved_at)
        .max(card.saved_at);
    if listed_at <= verified {
        library
            .remember_work_source_update(card.id, card.snapshot_id, listed_at)
            .map_err(|e| e.to_string())?;
        return Ok(false);
    }
    Ok(true)
}

pub(super) fn can_skip_listing(
    library: &Library,
    card: &WorkCard,
    listed: &str,
) -> Result<bool, String> {
    let Some(listed) = timestamp(listed) else {
        return Ok(false);
    };
    let context = library
        .work_source_state(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?;
    let verified = context
        .last_deep_checked_at
        .or_else(|| (card.provider == "fanbox").then_some(card.saved_at));
    if !verified
        .and_then(chrono::DateTime::from_timestamp_millis)
        .is_some_and(|v| {
            let age = chrono::Utc::now().signed_duration_since(v);
            age >= chrono::Duration::zero()
                && age < chrono::Duration::days(DEEP_CHECK_INTERVAL_DAYS)
        })
    {
        return Ok(false);
    }
    let Some(original) = library
        .work_original(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    let original: Value = serde_json::from_str(&original).map_err(|e| e.to_string())?;
    let stored = context.source_updated_at.or_else(|| {
        (card.provider == "fanbox")
            .then(|| {
                super::super::super::downloader::compute_content_details(&original, "fanbox").2
            })
            .flatten()
            .as_deref()
            .and_then(timestamp)
    });
    if stored != Some(listed)
        || !super::super::super::downloader::fetched_has_material_content(&original, &card.provider)
    {
        return Ok(false);
    }
    if card.provider == "pixiv" {
        if original
            .get("missing_illustrations")
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
        {
            return Ok(false);
        }
        let cover = original
            .get("cover_url")
            .or_else(|| original.pointer("/detail/cover_url"))
            .and_then(Value::as_str);
        if cover.is_some_and(crate::source_assets::is_pixiv_placeholder_image) {
            return Ok(false);
        }
        let resources = library
            .resource_manifest(
                card.id,
                crate::library::ResourceScope::Snapshot(card.snapshot_id),
            )
            .map_err(|e| e.to_string())?;
        let names = resources
            .resources
            .iter()
            .filter(|r| r.role == "illustration" && r.file.is_some())
            .filter_map(|r| r.original_name.clone());
        if !crate::database::queries::pixiv_missing_references_for_filenames(&original, names)
            .is_empty()
        {
            return Ok(false);
        }
    } else if original.get("isRestricted").and_then(Value::as_bool) == Some(true) {
        return Ok(false);
    }
    super::super::super::downloader::shelf::unchanged_on_shelf(
        library,
        card.id,
        &original,
        &card.provider,
    )
}

fn local_item(library: &Library, card: &WorkCard, title: &str) -> Result<Value, String> {
    let context = library
        .work_source_state(card.id, card.snapshot_id)
        .map_err(|e| e.to_string())?;
    let saved_at = chrono::DateTime::from_timestamp_millis(card.saved_at)
        .ok_or("保存日時が不正です")?
        .to_rfc3339();
    Ok(serde_json::json!({
        "id": card.external_key, "title": title,
        "user": { "id": context.author_key, "name": card.author_name },
        "creatorId": (card.provider == "fanbox").then_some(&context.author_key),
        "localTitle": context.title, "localVersion": card.version_number, "localSavedAt": saved_at,
    }))
}

pub(in crate::commands::update_jobs) fn listed_candidate(
    library: &Library,
    source: &str,
    key: &str,
    source_item: &Value,
    default_kind: CandidateKind,
    new_works_only: bool,
) -> Result<Option<(CandidateKind, Value)>, String> {
    let Some(card) = saved_card(library, source, key)? else {
        return Ok(Some((default_kind, source_item.clone())));
    };
    if new_works_only || source == "pixiv" {
        return Ok(None);
    }
    let listed = string_at(source_item, &[&["updatedDatetime"], &["updated_datetime"]]);
    let restricted = source_item.get("isRestricted").and_then(Value::as_bool) == Some(true);
    if !restricted
        && !listed
            .as_deref()
            .map(|v| needs_revision(library, &card, v))
            .transpose()?
            .unwrap_or(false)
    {
        return Ok(None);
    }
    let title = source_item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&card.title);
    let mut decorated = source_item.clone();
    let local = local_item(library, &card, title)?;
    for field in ["localTitle", "localVersion", "localSavedAt"] {
        decorated[field] = local[field].clone();
    }
    Ok(Some((CandidateKind::Revision, decorated)))
}

pub(in crate::commands::update_jobs) fn fanbox_scan_posts(
    library: &Library,
    all: &[FanboxPost],
    new: &[FanboxPost],
) -> Result<Vec<FanboxPost>, String> {
    let new_ids = new.iter().map(|p| p.id.as_str()).collect::<HashSet<_>>();
    let mut selected = Vec::new();
    for post in all {
        if new_ids.contains(post.id.as_str()) || saved_card(library, "fanbox", &post.id)?.is_some()
        {
            selected.push(post.clone());
        }
    }
    Ok(selected)
}

pub(super) fn apply_pixiv_listing(
    state: &Arc<AppState>,
    library: &Library,
    job_id: &str,
    target: &crate::database::UpdateTarget,
    auto_save: bool,
    listing: &CachedWebListing,
) -> Result<i64, String> {
    let saved = author_cards(library, "pixiv", &target.source_key)?;
    let available = listing
        .entries
        .values()
        .filter(|entry| !entry.is_masked && entry.update_date.is_some())
        .map(|e| e.id.clone())
        .collect::<Vec<_>>();
    state
        .db
        .clear_resolved_unavailable_candidates("pixiv", &available)?;
    let mut found = 0;
    for card in saved {
        if listing.unavailable_ids.contains(&card.external_key) {
            let reason = unavailable_saved_pixiv_work_message(&card.title, &card.external_key);
            let original = local_item(library, &card, &card.title)?;
            let item = item_from_payload(
                "work",
                "held",
                Some("pixiv".into()),
                Some(card.external_key.clone()),
                Some("work".into()),
                card.title.clone(),
                serde_json::json!({"source":"pixiv","sourceId":card.external_key}),
            )?;
            if state
                .db
                .insert_update_job_unavailable_item(job_id, &item, &original, &reason)?
            {
                state.db.append_update_job_log(
                    job_id,
                    "warn",
                    &format!("{}: {reason}", target.display_name),
                )?;
            }
            continue;
        }
        let Some(entry) = listing
            .entries
            .get(&card.external_key)
            .filter(|e| !e.is_masked)
        else {
            continue;
        };
        let Some(listed) = entry.update_date.as_deref() else {
            continue;
        };
        if needs_revision(library, &card, listed)? {
            let mut item = local_item(
                library,
                &card,
                entry.title.as_deref().unwrap_or(&card.title),
            )?;
            item["create_date"] = serde_json::json!(entry.create_date);
            if record_candidate(
                state,
                job_id,
                target,
                &item,
                CandidateKind::Revision,
                auto_save,
            )? {
                found += 1;
            }
        }
    }
    Ok(found)
}

pub(in crate::commands::update_jobs) async fn scan_pixiv_revisions(
    state: &Arc<AppState>,
    library: &Arc<Library>,
    job_id: &str,
    target: &crate::database::UpdateTarget,
    auto_save: bool,
    web_index: &mut WebUpdateIndex,
) -> Result<i64, String> {
    let (lib, author) = (Arc::clone(library), target.source_key.clone());
    if blocking(move || author_cards(&lib, "pixiv", &author))
        .await?
        .is_empty()
    {
        return Ok(0);
    }
    let listing = web_index
        .author_listing(state, &target.source_key, job_id)
        .await?
        .ok_or("pixiv の改稿一覧を完全に確認できません")?
        .clone();
    let (state, library, job_id, target) = (
        Arc::clone(state),
        Arc::clone(library),
        job_id.to_owned(),
        target.clone(),
    );
    let _guard = state.library_gate.clone().write_owned().await;
    blocking(move || apply_pixiv_listing(&state, &library, &job_id, &target, auto_save, &listing))
        .await
}

#[cfg(test)]
mod tests {
    use super::super::tests::{data, fixture, request, save};
    use super::*;
    use crate::library::{LibraryId, ResourceReference, ResourceScope};
    use crate::pixiv_api::web::NovelListEntryWeb;
    use serde_json::json;

    fn date(at: i64) -> String {
        chrono::DateTime::from_timestamp_millis(at)
            .unwrap()
            .to_rfc3339()
    }

    fn target(source: &str, author: &str) -> crate::database::UpdateTarget {
        crate::database::UpdateTarget {
            id: 0.into(),
            watch_generation: None,
            target_type: "author".into(),
            source: source.into(),
            source_key: author.into(),
            display_name: "作者".into(),
            enabled: true,
            last_checked_at: None,
            last_seen_source_id: None,
            last_seen_source_updated_at: None,
            metadata_json: None,
            created_at: date(0),
            updated_at: date(0),
            last_hit_at: None,
            consecutive_errors: 0,
            watch_groups: Vec::new(),
            icon_path: None,
        }
    }

    fn listing(card: &WorkCard, at: i64) -> CachedWebListing {
        CachedWebListing {
            entries: HashMap::from([(
                card.external_key.clone(),
                NovelListEntryWeb {
                    id: card.external_key.clone(),
                    title: Some("作者の題".into()),
                    update_date: Some(date(at)),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }
    }

    fn job(state: &AppState) -> String {
        let id = make_job_id();
        state
            .db
            .create_update_job(&id, &request("author"), &[])
            .unwrap();
        id
    }

    #[test]
    fn source_timestamps_are_snapshot_guarded_and_never_claim_an_unverified_revision() {
        let (_temp, _state, library) = fixture();
        let card = save(&library, "pixiv", "11", &data("pixiv", "本文", "77"), false);
        let old = card.saved_at - 1000;
        library
            .remember_work_source_update(card.id, card.snapshot_id, old)
            .unwrap();
        assert_eq!(
            library
                .work_source_state(card.id, card.snapshot_id)
                .unwrap()
                .source_updated_at,
            Some(old)
        );
        assert_eq!(library.work_card(card.id).unwrap().updated_at, Some(old));
        assert!(library
            .remember_work_source_update(card.id, card.snapshot_id, card.saved_at + 60_000)
            .is_err());
        let other = save(
            &library,
            "pixiv",
            "12",
            &data("pixiv", "別の本文", "77"),
            false,
        );
        assert!(library
            .remember_work_source_update(card.id, other.snapshot_id, old)
            .is_err());
        let newer = save(&library, "pixiv", "11", &data("pixiv", "改稿", "77"), false);
        assert!(matches!(
            library.remember_work_source_update(card.id, card.snapshot_id, old),
            Err(crate::library::Error::Superseded)
        ));
        assert!(library
            .remember_work_source_update(card.id, newer.snapshot_id, i64::MAX)
            .is_err());
        library
            .remove_work(
                LibraryId::now_v7(),
                card.id,
                library.library_item_generation(card.id).unwrap().unwrap(),
            )
            .unwrap();
        assert!(library
            .remember_work_source_update(card.id, newer.snapshot_id, old)
            .is_err());
        library.verify().unwrap();
    }

    #[tokio::test]
    async fn pixiv_author_scan_uses_new_shelf_works_and_the_same_cached_listing_as_work_checks() {
        let (_temp, state, library) = fixture();
        let card = save(&library, "pixiv", "11", &data("pixiv", "本文", "77"), false);
        save(
            &library,
            "pixiv",
            "22",
            &data("pixiv", "別の作者", "88"),
            true,
        );
        let library = Arc::new(library);
        let old = card.saved_at - 1000;
        let cached = listing(&card, old);
        let mut index = WebUpdateIndex::new(&snapshot_credentials_missing());
        index.library = Some(Arc::clone(&library));
        index.asked_authors.insert("77".into());
        index.entries.insert("77".into(), cached);
        let first = job(&state);
        assert_eq!(
            scan_pixiv_revisions(
                &state,
                &library,
                &first,
                &target("pixiv", "77"),
                false,
                &mut index
            )
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            library
                .work_source_state(card.id, card.snapshot_id)
                .unwrap()
                .source_updated_at,
            Some(old)
        );
        assert!(state.db.pixiv_works_for_author("77").unwrap().is_empty());
        assert!(matches!(
            index.lookup_key(&state, "77", "11", &first).await,
            WebLookup::Found(_)
        ));
        assert_eq!(index.asked_authors.len(), 1);
        index
            .entries
            .get_mut("77")
            .unwrap()
            .entries
            .get_mut("11")
            .unwrap()
            .is_masked = true;
        assert!(matches!(
            index.lookup_key(&state, "77", "11", &first).await,
            WebLookup::Unknown
        ));
        assert_eq!(
            apply_pixiv_listing(
                &state,
                &library,
                &job(&state),
                &target("pixiv", "77"),
                false,
                &index.entries["77"]
            )
            .unwrap(),
            0
        );

        let newer = card.saved_at + 60_000;
        index.entries.insert("77".into(), listing(&card, newer));
        let second = job(&state);
        assert_eq!(
            scan_pixiv_revisions(
                &state,
                &library,
                &second,
                &target("pixiv", "77"),
                true,
                &mut index
            )
            .await
            .unwrap(),
            1
        );
        let item = state.db.next_update_job_item(&second).unwrap().unwrap();
        assert_eq!(item.source_id.as_deref(), Some("11"));
        let payload: Value = serde_json::from_str(&item.payload_json).unwrap();
        assert_eq!(payload["kind"], "revision");
        assert_eq!(payload["originalData"]["localVersion"], 1);
        assert_eq!(
            library
                .work_source_state(card.id, card.snapshot_id)
                .unwrap()
                .source_updated_at,
            Some(old)
        );
        state
            .db
            .set_update_candidate_status("pixiv", "11", "dismissed")
            .unwrap();
        assert_eq!(
            apply_pixiv_listing(
                &state,
                &library,
                &job(&state),
                &target("pixiv", "77"),
                false,
                &listing(&card, newer)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn listing_scans_read_database_metadata_and_exclude_removed_works() {
        let (temp, state, library) = fixture();
        let card = save(&library, "pixiv", "11", &data("pixiv", "本文", "77"), false);
        let resources = library
            .resource_manifest(card.id, ResourceScope::Snapshot(card.snapshot_id))
            .unwrap();
        let raw = resources
            .resources
            .iter()
            .find(|r| matches!(r.reference, ResourceReference::Original { .. }))
            .unwrap()
            .file
            .as_ref()
            .unwrap();
        let chunk = library.file(raw.id).unwrap().chunks[0].id;
        let object = library
            .objects
            .listing()
            .unwrap()
            .into_iter()
            .find(|o| o.entry == piep_storage::StoredEntry::Object(chunk))
            .unwrap();
        assert!(object
            .path
            .starts_with(std::fs::canonicalize(&temp.0).unwrap()));
        std::fs::remove_file(object.path).unwrap();
        let newer = card.saved_at + 60_000;
        assert_eq!(
            apply_pixiv_listing(
                &state,
                &library,
                &job(&state),
                &target("pixiv", "77"),
                false,
                &listing(&card, newer)
            )
            .unwrap(),
            1
        );
        library
            .remove_work(
                LibraryId::now_v7(),
                card.id,
                library.library_item_generation(card.id).unwrap().unwrap(),
            )
            .unwrap();
        assert!(author_cards(&library, "pixiv", "77").unwrap().is_empty());
        assert_eq!(
            apply_pixiv_listing(
                &state,
                &library,
                &job(&state),
                &target("pixiv", "77"),
                false,
                &listing(&card, newer)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn unavailable_shelf_work_is_kept_and_counted_once_without_a_legacy_row() {
        let (_temp, state, library) = fixture();
        let card = save(&library, "pixiv", "11", &data("pixiv", "本文", "77"), false);
        let missing = CachedWebListing {
            unavailable_ids: HashSet::from(["11".into()]),
            ..Default::default()
        };
        let first = job(&state);
        apply_pixiv_listing(
            &state,
            &library,
            &first,
            &target("pixiv", "77"),
            false,
            &missing,
        )
        .unwrap();
        apply_pixiv_listing(
            &state,
            &library,
            &first,
            &target("pixiv", "77"),
            false,
            &missing,
        )
        .unwrap();
        let snapshot = state.db.update_job_snapshot(&first).unwrap();
        assert_eq!(snapshot.totals, 1);
        assert_eq!(snapshot.held_count, 1);
        assert!(library
            .work_original(card.id, card.snapshot_id)
            .unwrap()
            .is_some());
        let next = job(&state);
        apply_pixiv_listing(
            &state,
            &library,
            &next,
            &target("pixiv", "77"),
            false,
            &missing,
        )
        .unwrap();
        let snapshot = state.db.update_job_snapshot(&next).unwrap();
        assert_eq!(snapshot.held_count, 0);
        assert_eq!(snapshot.known_held_count, 1);
        let recovered = listing(&card, card.saved_at - 1000);
        apply_pixiv_listing(
            &state,
            &library,
            &job(&state),
            &target("pixiv", "77"),
            false,
            &recovered,
        )
        .unwrap();
        assert_eq!(
            state.db.update_candidate_status("pixiv", "11").unwrap(),
            None
        );
    }

    #[test]
    fn listing_shortcuts_require_recent_complete_and_matching_saved_content() {
        let (_temp, _state, library) = fixture();
        let original = data("pixiv", "本文", "77");
        let card = save(&library, "pixiv", "11", &original, true);
        let old = card.saved_at - 1000;
        library
            .remember_work_source_update(card.id, card.snapshot_id, old)
            .unwrap();
        assert!(!can_skip_listing(&library, &card, &date(old)).unwrap());
        let expired = card.saved_at - (DEEP_CHECK_INTERVAL_DAYS + 1) * 86_400_000;
        library
            .record_work_check(card.id, card.snapshot_id, expired, true)
            .unwrap();
        assert!(!can_skip_listing(&library, &card, &date(old)).unwrap());
        library
            .record_work_check(card.id, card.snapshot_id, card.saved_at, true)
            .unwrap();
        assert!(can_skip_listing(&library, &card, &date(old)).unwrap());
        assert!(!can_skip_listing(&library, &card, &date(card.saved_at + 1000)).unwrap());
        assert!(!can_skip_listing(&library, &card, "unknown").unwrap());
        let missing = save(
            &library,
            "pixiv",
            "12",
            &data("pixiv", "本文[uploadedimage:1]", "77"),
            true,
        );
        library
            .record_work_check(missing.id, missing.snapshot_id, missing.saved_at, true)
            .unwrap();
        library
            .remember_work_source_update(missing.id, missing.snapshot_id, old)
            .unwrap();
        assert!(!can_skip_listing(&library, &missing, &date(old)).unwrap());
        let fanbox = save(
            &library,
            "fanbox",
            "21",
            &data("fanbox", "本文", "creator"),
            true,
        );
        assert!(can_skip_listing(&library, &fanbox, "2026-11-01T09:00:00+09:00").unwrap());
    }

    #[test]
    fn fanbox_cached_scan_checks_old_saved_posts_without_rediscovering_old_unsaved_posts() {
        let (_temp, state, library) = fixture();
        let card = save(
            &library,
            "fanbox",
            "saved",
            &data("fanbox", "本文", "creator"),
            false,
        );
        let all = ["new", "checkpoint", "saved", "old-unsaved"].map(|key| serde_json::from_value::<FanboxPost>(json!({
            "id":key,"title":key,"creatorId":"creator","publishedDatetime":"2026-11-01T00:00:00Z","updatedDatetime":date(card.saved_at + 60_000)
        })).unwrap()).to_vec();
        let mut cache = FanboxUpdateIndex::default();
        cache.posts.insert("creator".into(), all.clone());
        let new = cache.posts_since("creator", Some("checkpoint")).unwrap();
        let chosen = fanbox_scan_posts(&library, &all, &new).unwrap();
        assert_eq!(
            chosen.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["new", "saved"]
        );
        assert_eq!(new[0].id, "new");
        let previous = serde_json::to_value(&chosen[1]).unwrap();
        let (kind, payload) = listed_candidate(
            &library,
            "fanbox",
            "saved",
            &previous,
            CandidateKind::New,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(kind, CandidateKind::Revision));
        assert_eq!(payload["localVersion"], 1);
        assert!(listed_candidate(
            &library,
            "fanbox",
            "saved",
            &previous,
            CandidateKind::New,
            true
        )
        .unwrap()
        .is_none());
        let mut same = previous.clone();
        same["updatedDatetime"] = json!("2026-11-01T00:00:00Z");
        assert!(listed_candidate(
            &library,
            "fanbox",
            "saved",
            &same,
            CandidateKind::New,
            false
        )
        .unwrap()
        .is_none());
        assert_eq!(
            state
                .db
                .get_download_by_source("fanbox", "saved")
                .unwrap()
                .map(|d| d.id),
            None
        );
        let new = cache.posts_since("creator", Some("new")).unwrap();
        assert!(new.is_empty());
        assert_eq!(
            fanbox_scan_posts(&library, &all, &new)
                .unwrap()
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["saved"]
        );
    }
}
