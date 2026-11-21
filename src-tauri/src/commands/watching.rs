//! 監視の画面とジョブが使う口。切り替えた棚では旧 DB の監視表を読まず、書かない。

use crate::database::{UpdateTarget, WatchGroup, WatchId};
use crate::library::{
    EntityKind, Library, LibraryId, ResourceReference, ResourceScope, WatchEdit, WatchResult,
};
use std::sync::Arc;
use tauri::Manager;

pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

pub(super) fn active(app: &tauri::AppHandle) -> Result<Arc<Library>, String> {
    app.try_state::<Arc<super::shelf::Shelf>>()
        .ok_or_else(|| "棚は開いていません".to_string())?
        .library()
}

fn date(value: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(value)
        .map(|v| v.to_rfc3339())
        .ok_or("監視の日時が不正です".into())
}

fn icon(library: &Library, target: &crate::library::WatchTarget) -> Result<Option<String>, String> {
    let kind = match target.kind.as_str() {
        "author" => EntityKind::Person,
        "series" => EntityKind::Series,
        _ => return Ok(None),
    };
    let Some(entity) = library
        .entity_by_key(kind, &target.provider, &target.key)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let snapshot = match library.current_snapshot(entity) {
        Ok(v) => v,
        Err(crate::library::Error::Sql(rusqlite::Error::QueryReturnedNoRows)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let slot = if target.kind == "author" {
        "icon"
    } else {
        "cover"
    };
    Ok(library
        .resource_manifest(entity, ResourceScope::Snapshot(snapshot))
        .map_err(|e| e.to_string())?
        .resources
        .into_iter()
        .find(|r| r.reference == ResourceReference::Media { slot: slot.into() })
        .and_then(|r| r.file)
        .map(|file| format!("shelf:{}", file.id)))
}

pub(super) fn targets(
    library: &Library,
    kind: Option<&str>,
    enabled: bool,
) -> Result<Vec<UpdateTarget>, String> {
    for _ in 0..3 {
        let generation = library.watch_generation().map_err(|e| e.to_string())?;
        let saved = library.watch_targets().map_err(|e| e.to_string())?;
        if library.watch_generation().map_err(|e| e.to_string())? != generation {
            continue;
        }
        return saved
            .into_iter()
            .filter(|t| {
                (!enabled || t.enabled)
                    && kind.is_none_or(|k| k.is_empty() || k == "all" || k == t.kind)
            })
            .map(|target| {
                let icon_path = icon(library, &target)?;
                Ok(UpdateTarget {
                    id: WatchId::Shelf(target.id),
                    watch_generation: Some(generation),
                    target_type: target.kind,
                    source: target.provider,
                    source_key: target.key,
                    display_name: target.name,
                    enabled: target.enabled,
                    last_checked_at: target.checked_at.map(date).transpose()?,
                    last_seen_source_id: target.seen_key,
                    last_seen_source_updated_at: target.seen_updated_at.map(date).transpose()?,
                    metadata_json: Some(target.options),
                    created_at: date(target.created_at)?,
                    updated_at: date(target.updated_at)?,
                    last_hit_at: target.hit_at.map(date).transpose()?,
                    consecutive_errors: target.errors,
                    watch_groups: target.groups,
                    icon_path,
                })
            })
            .collect();
    }
    Err("監視の設定が変更されています。読み直してください".into())
}

pub(super) fn find(
    library: &Library,
    kind: &str,
    source: &str,
    key: &str,
) -> Result<Option<UpdateTarget>, String> {
    Ok(targets(library, Some(kind), false)?
        .into_iter()
        .find(|t| t.source == source && t.source_key == key))
}

pub(super) fn groups(library: &Library) -> Result<Vec<WatchGroup>, String> {
    for _ in 0..3 {
        let generation = library.watch_generation().map_err(|e| e.to_string())?;
        let saved = library.watch_groups().map_err(|e| e.to_string())?;
        if library.watch_generation().map_err(|e| e.to_string())? != generation {
            continue;
        }
        return saved
            .into_iter()
            .map(|g| {
                Ok(WatchGroup {
                    id: WatchId::Shelf(g.id),
                    legacy_id: g.legacy_id,
                    watch_generation: Some(generation),
                    name: g.name,
                    sort_order: g.position,
                    created_at: date(g.created_at)?,
                    updated_at: date(g.updated_at)?,
                    target_ids: g.targets.into_iter().map(WatchId::Shelf).collect(),
                })
            })
            .collect();
    }
    Err("監視グループが変更されています。読み直してください".into())
}

pub(super) fn target_id(library: &Library, key: WatchId) -> Result<LibraryId, String> {
    library
        .watch_targets()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|t| match key {
            WatchId::Shelf(id) => t.id == id,
            WatchId::Legacy(id) => t.legacy_id == Some(id),
        })
        .map(|t| t.id)
        .ok_or("監視対象が見つかりません".into())
}
pub(super) fn group_id(library: &Library, key: WatchId) -> Result<LibraryId, String> {
    library
        .watch_groups()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|t| match key {
            WatchId::Shelf(id) => t.id == id,
            WatchId::Legacy(id) => t.legacy_id == Some(id),
        })
        .map(|t| t.id)
        .ok_or("監視グループが見つかりません".into())
}

pub(super) fn edit(
    library: &Library,
    request: Option<LibraryId>,
    expected: Option<i64>,
    change: WatchEdit,
) -> Result<WatchResult, String> {
    let request = request.unwrap_or_else(LibraryId::now_v7);
    let expected = expected
        .or(library
            .watch_request_expected(request)
            .map_err(|e| e.to_string())?)
        .map(Ok)
        .unwrap_or_else(|| library.watch_generation().map_err(|e| e.to_string()))?;
    library
        .edit_watch(request, expected, change)
        .map_err(|e| e.to_string())
}

pub(super) fn selected_targets(
    library: &Library,
    ids: Option<&[WatchId]>,
) -> Result<Vec<UpdateTarget>, String> {
    let selected = ids
        .map(|ids| {
            ids.iter()
                .map(|id| target_id(library, *id))
                .collect::<Result<std::collections::HashSet<_>, _>>()
        })
        .transpose()?;
    Ok(targets(library, None, true)?
        .into_iter()
        .filter(|t| {
            selected
                .as_ref()
                .is_none_or(|ids| matches!(t.id,WatchId::Shelf(id) if ids.contains(&id)))
        })
        .collect())
}

pub(super) fn series_authors(
    library: &Library,
    source: &str,
    key: &str,
) -> Result<Vec<String>, String> {
    let Some(series) = library
        .entity_by_key(EntityKind::Series, source, key)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    let mut query = crate::library::WorkListQuery {
        limit: 500,
        count: false,
        filter: crate::library::WorkFilter {
            series: Some(series),
            provider: Some(source.into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut authors = std::collections::HashSet::new();
    loop {
        let page = library.list_works(&query).map_err(|e| e.to_string())?;
        for card in page.items {
            let author = library
                .work_source_state(card.id, card.snapshot_id)
                .map_err(|e| e.to_string())?
                .author_key;
            if !author.is_empty() {
                authors.insert(author);
            }
        }
        query.after = page.next;
        if query.after.is_none() {
            break;
        }
    }
    Ok(authors.into_iter().collect())
}

pub(super) fn record(
    library: &Library,
    target: &UpdateTarget,
    seen: Option<&str>,
    updated: Option<&str>,
    found: i64,
    failed: bool,
) -> Result<(), String> {
    let Some(current) = find(
        library,
        &target.target_type,
        &target.source,
        &target.source_key,
    )?
    else {
        return Ok(());
    };
    let matches = match target.id {
        WatchId::Shelf(id) => current.id == WatchId::Shelf(id),
        WatchId::Legacy(id) => target_id(library, WatchId::Legacy(id))
            .ok()
            .is_some_and(|id| current.id == WatchId::Shelf(id)),
    };
    if !matches {
        return Ok(());
    }
    let updated = updated
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v)
                .map(|v| v.timestamp_millis())
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    if let WatchId::Shelf(id) = current.id {
        library
            .record_watch_check(id, seen, updated, found, failed)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, UpdateTargetInput};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn migrated_ids_and_groups_are_resolved_in_the_shelf_without_writing_the_old_database() {
        let root = std::env::temp_dir().join(format!("piep-watch-adapter-{}", LibraryId::now_v7()));
        std::fs::create_dir_all(root.join("old")).unwrap();
        {
            let old =
                Database::open(&root.join("old/piep.db"), &root.join("old/downloads")).unwrap();
            let before = old
                .upsert_update_target(&UpdateTargetInput {
                    target_type: "author".into(),
                    source: "pixiv".into(),
                    source_key: "77".into(),
                    display_name: "作者".into(),
                    enabled: true,
                    metadata_json: None,
                })
                .unwrap();
            let group = old.create_watch_group("毎日").unwrap();
            old.set_watch_group_members(group, &[before.id.legacy().unwrap()], true)
                .unwrap();
            // 開発用 DB の初期化が作った preview の写しは、監視の移行試験から分けて残す。
            std::fs::create_dir_all(root.join("fixture-backups")).unwrap();
            for entry in std::fs::read_dir(root.join("old")).unwrap() {
                let entry = entry.unwrap();
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("piep.db.preview-backup-")
                {
                    assert_eq!(entry.path().parent(), Some(root.join("old").as_path()));
                    std::fs::rename(
                        entry.path(),
                        root.join("fixture-backups").join(entry.file_name()),
                    )
                    .unwrap();
                }
            }
            let library = Library::create(&root.join("shelf")).unwrap();
            let source = crate::library::LegacyImporter::open(&root.join("old")).unwrap();
            let report = source
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
            library.switch_over().unwrap();
            let selected = selected_targets(&library, Some(&[before.id])).unwrap();
            assert_eq!(selected.len(), 1);
            let after = selected[0].clone();
            assert!(matches!(after.id, WatchId::Shelf(_)));
            let saved_groups = groups(&library).unwrap();
            assert_eq!(saved_groups[0].legacy_id, Some(group));
            assert_eq!(saved_groups[0].target_ids, [after.id]);
            let request = LibraryId::now_v7();
            let id = target_id(&library, after.id).unwrap();
            let result = edit(
                &library,
                Some(request),
                None,
                WatchEdit::Enable {
                    target: id,
                    enabled: false,
                },
            )
            .unwrap();
            assert_eq!(
                edit(
                    &library,
                    Some(request),
                    None,
                    WatchEdit::Enable {
                        target: id,
                        enabled: false
                    }
                )
                .unwrap(),
                result
            );
            assert!(
                !find(&library, "author", "pixiv", "77")
                    .unwrap()
                    .unwrap()
                    .enabled
            );
            assert!(
                old.find_update_target("author", "pixiv", "77")
                    .unwrap()
                    .unwrap()
                    .enabled
            );
            let captured = after;
            let disabled = find(&library, "author", "pixiv", "77").unwrap().unwrap();
            edit(
                &library,
                None,
                disabled.watch_generation,
                WatchEdit::RemoveTarget { target: id },
            )
            .unwrap();
            let recreated = edit(
                &library,
                None,
                None,
                WatchEdit::Upsert {
                    target_type: "author".into(),
                    provider: "pixiv".into(),
                    key: "77".into(),
                    name: "作り直した作者".into(),
                    enabled: true,
                    options: serde_json::json!({}),
                },
            )
            .unwrap();
            assert_ne!(recreated.id, id);
            record(&library, &captured, Some("late"), None, 1, false).unwrap();
            assert!(find(&library, "author", "pixiv", "77")
                .unwrap()
                .unwrap()
                .last_seen_source_id
                .is_none());
            assert_eq!(old.list_watch_groups().unwrap()[0].target_ids, [before.id]);
            assert!(
                selected_targets(&library, Some(&[WatchId::Shelf(LibraryId::now_v7())])).is_err()
            );
            assert!(selected_targets(&library, Some(&[])).unwrap().is_empty());
            library.verify().unwrap();
        }
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
